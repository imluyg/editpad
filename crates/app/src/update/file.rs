//! 文件域消息处理（自 update.rs 拆出，纯移动零行为变更）。
//!
//! 承载文件的打开/加载/保存/另存为/重命名、最近文件与标签视图记忆、
//! 自动保存调度、外部修改巡检。

use super::*;

impl Editpad {
    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：文件（打开/保存/编码/行尾/拖放/外部变更）。臂体自原 update() 逐字搬移，零行为变更。
    pub(super) fn update_file(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 打开 ----------
            Message::OpenRequested => {
                if self.busy {
                    return Task::none();
                }
                self.enter_busy();
                self.status.clear();
                // P155：过滤器文案先取出（async 块不能捕获 &self——闭包
                // 要 Send，而编辑器句柄是 Rc<RefCell<..>>）
                let all_files = self.t(editpad_core::Key::StFileTypesAll);
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter(all_files, &["*"])
                            .pick_file()
                            .await
                    },
                    |handle| Message::FileChosen(handle.map(|f| f.path().to_path_buf())),
                )
            }
            Message::FileChosen(None) => {
                self.busy = false;
                Task::none()
            }
            Message::FileChosen(Some(path)) => {
                // 对话框阶段结束：busy 若不清零，会撞上后续守卫导致卡死
                self.busy = false;
                self.request_open(path)
            }
            // 拖拽文件进窗口 = 打开；同一路径重复拖拽也允许重新加载
            // （每次 job_seq 递增，无路径去重；dirty/busy 保护在 request_open 内）
            Message::FileDropped(path) => self.request_open(path),
            // P103：命令行排队打开的下一个文件。boot 首发；每次 Loaded
            // 结算（busy 归零）后续排，直到队列清空。request_open 的
            // dirty/busy 守卫照常生效——期间用户若编辑了当前页，后续
            // 文件改走打开确认条，与拖拽语义完全一致。
            Message::OpenNextCliFile => {
                if self.busy {
                    return Task::none();
                }
                let Some(path) = self.pending_cli.pop_front() else {
                    return Task::none();
                };
                self.request_open(path)
            }
            Message::PendingOpenTick => {
                // 单实例转发轮询：读实例目录批次文件里第二实例递来的待开
                // 路径，走既有 CLI 队列在新标签页逐个打开。busy（加载/
                // 对话框在途）时跳过本拍，下一拍自然重试。
                // P149：节拍已改订阅时钟驱动，本臂只做轮询。
                if !self.busy {
                    let paths = single_instance::take_pending_open();
                    if !paths.is_empty() {
                        self.pending_cli.extend(paths);
                        return self.update(Message::OpenNextCliFile);
                    }
                }
                Task::none()
            }
            Message::LoadProgress(job_id, bytes_read, total_bytes) => {
                if self.active_load.as_ref().is_some_and(|j| j.id == job_id) {
                    self.progress = Some((bytes_read, total_bytes));
                }
                Task::none()
            }
            Message::Loaded(job_id, result) => {
                // 过期任务的迟到消息直接丢弃
                let Some(job) = self.active_load.clone() else {
                    return Task::none();
                };
                if job.id != job_id {
                    return Task::none();
                }
                self.active_load = None;
                self.progress = None;
                // P21/P145：结果路由回发起加载的标签页——按登记时记下的
                // 页 id 解析当前位置（期间关页/换位会让下标漂移，id 不变）；
                // 页已被关则解析不到，结果走丢弃分支（曾按登记下标直接
                // 索引 tabs：加载中 Ctrl+W 关页后必越界 panic/串页）。
                let target = self.tabs.iter().position(|t| t.id == job.tab_id);
                // P30：恢复任务的待还原视图随任务号取出；None = 普通打开
                let pending_view = self.restore_views.remove(&job_id);
                let is_restore = pending_view.is_some();
                // P30 防串写护栏：恢复任务要求目标仍是空净无名占位页
                let target = target.filter(|&i| !is_restore || self.restore_placeholder_ready(i));
                let mut tasks: Vec<Task<Message>> = Vec::new();
                // P126：.LOG 首行时间戳是否已追加（Ok 臂内置位，装载尾部重新置脏）
                let mut log_appended = false;
                match (result, target) {
                    (Ok((doc, sample, encoding)), Some(target)) => {
                        // P22：语言解析下沉 core——扩展名别名层 + 无扩展名
                        // 内容嗅探（shebang/XML/JSON/YAML/约定文件名）
                        let language =
                            editpad_core::resolve_language(Some(job.path.as_path()), &sample);
                        let tab = &mut self.tabs[target];
                        {
                            let mut ed = tab.editor.borrow_mut();
                            // P19：rope 直入，不再有 from_str 的二次全文拷贝
                            ed.reset_document(doc);
                            ed.set_language_by_name(language.as_deref());
                            // 语法主题随应用明暗档（浅底/深底配色必须一致）
                            ed.apply_highlight_theme(self.dark_mode);
                            // P30：恢复任务的视图回填——光标与滚动回到
                            // 上次退出时的位置（无副作用定位入口）
                            if let Some((line, col, scroll_top, scroll_left)) = pending_view {
                                ed.restore_view(line, col, scroll_top, scroll_left);
                            }
                            // P32：普通打开命中最近文件记忆 → 光标/滚动就位。
                            // 恢复任务以会话清单视图为准（上方已应用），不覆盖。
                            if !is_restore {
                                if let Some(view) = self.settings.recent_view(&job.path) {
                                    ed.restore_view(view.line, view.col, view.scroll_top, 0.0);
                                }
                                // P126：.LOG 首行自动时间戳（经典记事本行为）：
                                // 首行恰为 .LOG 的文件在文末追加当前日期时间。
                                // 文本照常置脏（默认不自动写盘，落盘仍由用户
                                // 决定）；会话恢复路径不追加（防快照滚雪球）。
                                if ed
                                    .doc
                                    .line_str(0)
                                    .trim_end_matches(char::is_control)
                                    == ".LOG"
                                {
                                    let last = ed.doc.line_count() - 1;
                                    let tlen = ed.doc.text_len();
                                    let ends_nl = tlen > 0
                                        && ed
                                            .doc
                                            .slice_text(tlen - 1, tlen)
                                            .chars()
                                            .all(char::is_control);
                                    ed.cursor = crate::editor::CursorPos {
                                        line: last,
                                        col: ed.line_display_len(last),
                                    };
                                    ed.anchor = None;
                                    if !ends_nl {
                                        let nl = char::from_u32(10).unwrap().to_string();
                                        ed.insert_str(&nl);
                                    }
                                    ed.insert_date_time();
                                    log_appended = true;
                                }
                                // P130：监视重载归页——曾在底部则 tail 跟随
                                // （滚到文末+光标落尾），否则还原重载前视图
                                if let Some((tab_idx, follow, pre_view)) =
                                    self.monitor_pending.take()
                                {
                                    if tab_idx == target {
                                        if follow {
                                            let last = ed.doc.line_count() - 1;
                                            ed.cursor = crate::editor::CursorPos {
                                                line: last,
                                                col: ed.line_display_len(last),
                                            };
                                            ed.anchor = None;
                                            ed.scroll_top = f32::MAX;
                                            ed.clamp_scroll();
                                        } else if let Some((l, c, st, sl)) = pre_view {
                                            ed.restore_view(l, c, st, sl);
                                        }
                                    }
                                }
                            }
                        }
                        tab.path = Some(job.path.clone());
                        // P133：相对路径链接的解析基准 = 本页文件所在目录
                        tab.editor
                            .borrow_mut()
                            .set_base_dir(job.path.parent().map(|p| p.to_path_buf()));
                        tab.encoding_label = encoding;
                        // P67：新载入的文件回到默认 UTF-8 偏好（旧偏好属于
                        // 上一次打开的会话上下文）
                        tab.save_encoding = None;
                        tab.dirty = false;
                        // P126：.LOG 追加发生在装载链内、上方 dirty=false 之后——
                        // 重新置脏如实反映「磁盘内容与窗口内容已不同」
                        if log_appended {
                            tab.dirty = true;
                        }
                        // P50：载入成功即记外部修改比对戳
                        tab.file_stamp = file_stamp(&job.path);
                        // P25：真实文件已就位，未命名序号使命完成
                        tab.untitled_num = None;
                        // P31：页内容整体换血（打开/恢复回填）——已提交清单
                        // 对本页的描述过期，下一拍重写
                        self.touch_manifest_stale();
                        if let Some(path) = self.path_of_tab(target) {
                            self.record_recent(&path);
                        }
                        self.busy = false;
                        // 新文档已就位：旧文档的关闭确认语义过期
                        self.confirm_visible = false;
                        self.pending_close = false;
                        self.status.clear();
                        if self.find_visible && target == self.active_tab {
                            tasks.push(self.schedule_find_scan());
                        }
                    }
                    (Ok(_), _) => {
                        // P30/P145：占位页已被用户动过（关页/新页导致下标
                        // 漂移），或普通打开的目标页已被关——宁可丢弃结果
                        // 也不能覆盖用户内容；恢复链计入失败汇总
                        self.busy = false;
                        if is_restore {
                            self.restore_failed += 1;
                        } else {
                            self.set_status(self.t(editpad_core::Key::StTabGone).to_owned());
                        }
                    }
                    (Err(error), target) => {
                        if is_restore {
                            // P30：恢复页加载失败（文件被删等）——移除占位页
                            // 继续恢复其余页，不阻断（§3 P30 第 6 条）；
                            // 占位页已被关（id 解析不到）则无可移除，照常计数
                            if let Some(target) = target {
                                self.drop_restore_placeholder(target);
                            }
                            self.restore_failed += 1;
                            self.busy = false;
                        } else {
                            self.busy = false;
                            self.set_status_error(self.t_suffix(editpad_core::Key::StOpenFailed, &error.to_string()));
                        }
                    }
                }
                // P30 恢复链推进：本步收尾后队列非空则续排下一页。
                // 注意这里必须**同步调用**而非塞进 Task 延后——方法调用
                // 本身就完成「弹出下一页 + 登记任务」的全部状态变更，
                // 加载流由 subscription 依据 active_load 重建自然接管；
                // 返回的 Task 恒为 none，无需借道批处理。
                if is_restore && self.settle_restore_step() {
                    let _ = self.begin_restore_load();
                }
                // P103：CLI 排队续排——同上的同步调用语义：每个命令行文件
                // 结算（busy 归零）后立即弹出下一个；加载流由 subscription
                // 依据 active_load 重建自动接管，无需借道 Task。
                // （restore 与 CLI 两条链不可能同时存在——boot 二者取一）
                if !self.pending_cli.is_empty() && !self.busy {
                    let _ = self.update(Message::OpenNextCliFile);
                }
                // P133：链接点击的「打开后跳行」——装载结算后一次性消费
                //（1 起行号；行号越界由 jump_to_line 钳制兜底）
                if let Some(line) = self.pending_link_goto.take() {
                    self.cur_handle.borrow_mut().jump_to_line(line as usize);
                }
                // A8：FIF 命中点击的「打开后选中命中」——同上一次性消费
                if let Some((line, col, len)) = self.pending_fif_goto.take() {
                    self.cur_handle.borrow_mut().select_span(line, col, len);
                }
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
            }
            // ---------- P133：链接 Ctrl+点击（路线图 E2） ----------
            Message::LinkClicked(editor::LinkTarget::Url(url)) => {
                match open_external(&url) {
                    Ok(()) => self.set_status(format!("{}{url}", self.t(editpad_core::Key::StOpenedExternal))),
                    Err(e) => self.set_status_error(self.t_suffix(editpad_core::Key::StOpenFailed, &e.to_string())),
                }
                Task::none()
            }
            Message::LinkClicked(editor::LinkTarget::File { path, line }) => {
                // 行号暂存，装载结算（Loaded）后一次性消费跳行；打开
                // 守卫（dirty 确认/busy）与普通打开同管线
                self.pending_link_goto = line;
                self.request_open(path)
            }
            // ---------- 保存 ----------
            Message::SaveRequested => match self.tab().path.clone() {
                Some(_) => self.save(),
                None => self.save_as_dialog(),
            },
            Message::SaveAsRequested => self.save_as_dialog(),
            Message::SaveTargetChosen(None) => {
                self.busy = false;
                // 另存为被取消：若它属于「保存并关闭」，一并放弃关闭
                self.pending_close = false;
                Task::none()
            }
            Message::SaveTargetChosen(Some(path)) => {
                // P146：改路径前作废在途自动保存——曾以调度时刻的旧路径
                // 落盘，新内容被写进旧文件
                self.tab_mut().invalidate_autosave();
                let tab = self.tab_mut();
                tab.path = Some(path.clone());
                // P133：相对路径链接的解析基准 = 本页文件所在目录
                tab.editor
                    .borrow_mut()
                    .set_base_dir(path.parent().map(|p| p.to_path_buf()));
                // P25：另存为转正后未命名序号使命完成
                tab.untitled_num = None;
                // P67：新路径回到默认 UTF-8 偏好（旧偏好属于旧路径）
                tab.save_encoding = None;
                // P63：按目标磁盘现状重记戳（新文件 = None）——用户在
                // 对话框里显式选中的覆盖目标，不该被自家外部修改守卫拦下
                tab.file_stamp = file_stamp(&path);
                // 对话框阶段结束再交给 save() 的 busy 守卫（原实现在此卡死 busy）
                self.busy = false;
                self.save()
            }
            Message::Saved(tab_id, version, Ok(notice)) => {
                // P146 路由：按发起页 id 归账——保存异步期间切页/关页，
                // 曾按「完成时刻的活动页」记账（错清别页置脏标记 → 关页
                // 不再弹确认 → 未保存内容无声丢失）。
                let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                    // 页已被关：落盘已发生，账目无处可记——收口即可
                    self.busy = false;
                    self.pending_close = false;
                    return Task::none();
                };
                // P18 版本守卫：保存期间又有编辑则保持置脏，防止丢改动标记
                self.tabs[idx].dirty = self.tabs[idx].version != version;
                self.busy = false;
                if !self.tabs[idx].dirty {
                    // P38：落盘成功且期间无新编辑——当前内容即磁盘内容，
                    // 刷新撤销回基线的判定基准（版本不符时不得动基线：
                    // 那时磁盘上是旧快照）
                    self.tabs[idx].editor.borrow_mut().mark_saved();
                    // P31：内存态比已提交清单「更干净」——下一拍重写清单，
                    // 防崩溃恢复把已落盘内容按旧快照复活成置脏页
                    self.touch_manifest_stale();
                }
                if let Some(path) = self.tabs[idx].path.clone() {
                    self.record_recent(&path);
                }
                // P67：状态栏标签反映实际落盘编码（用户选择的偏好或默认
                // UTF-8），转码提示按「原标签 vs 实际目标」判定
                let target_label = self.tabs[idx]
                    .save_encoding
                    .unwrap_or(editpad_core::SaveEncoding::Utf8)
                    .label();
                let prev_label = self.tabs[idx].encoding_label.clone();
                self.tabs[idx].encoding_label = target_label.to_owned();
                // P6 编码知情权：发生转码/BOM 丢失/不可映射字符时明确告知；
                // 无转码时补显暂存的备份提示（备份消息写在异步落盘完成
                // 之前，直接进状态栏会被本分支立即覆盖/抹掉）
                let backup_note = self.pending_backup_notice.take();
                if let Some(text) =
                    transcode_notice(self.lang(), &prev_label, target_label, notice.unmappable)
                {
                    self.set_status(text);
                } else if let Some(text) = backup_note {
                    self.set_status(text);
                } else {
                    self.status.clear();
                }
                // P50：落盘成功即刷新外部修改比对戳（磁盘内容 = 刚写的内容）
                if let Some(path) = self.tabs[idx].path.clone() {
                    self.tabs[idx].file_stamp = file_stamp(&path);
                }
                if self.pending_close {
                    // P147：「保存并关闭」= 存完**全部**置脏页再关窗——曾只存
                    // 活动页即关窗，ASK 模式下后台页未存改动无声丢失。仍有
                    // 其他置脏页（且不走快照直退）时切过去继续存（pending_close
                    // 保持），全部干净才关窗；未命名置脏页经另存为对话框裁决
                    //（取消即放弃关窗，见 SaveTargetChosen(None)）。
                    // 快照直退模式无需逐页存：exit_via_snapshot 全量入快照。
                    let snapshot_exit = session_restore_allowed(
                        self.settings.enable_snapshots,
                        self.settings.remember_session,
                    ) && self.settings.exit_mode == editpad_core::EXIT_MODE_SNAPSHOT;
                    if !snapshot_exit {
                        if let Some(next) = self.tabs.iter().position(|t| t.dirty) {
                            self.pending_close = true;
                            self.set_active_tab(next);
                            return self.save();
                        }
                    }
                    // P146：仅当保存的仍是当前活动页才延续关窗——保存期间
                    // 切到别的（可能置脏的）页后照关会丢新页状态。
                    self.pending_close = false;
                    if idx == self.active_tab {
                        if snapshot_exit {
                            if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                                return self.exit_via_snapshot(&dir);
                            }
                        }
                        return self.close_window();
                    }
                    self.set_status(self.t(editpad_core::Key::StSwitchCancelledClose).to_owned());
                }
                Task::none()
            }
            Message::Saved(_, _, Err(error)) => {
                self.busy = false;
                // 保存失败不关窗：留在应用里让用户处理
                self.pending_close = false;
                self.set_status_error(self.t_suffix(editpad_core::Key::StSaveFailed, &error.to_string()));
                // 保存失败时备份已发生（写前备份），但「已备份」提示对
                // 失败的保存没有意义，弃置防陈旧
                self.pending_backup_notice = None;
                Task::none()
            }
            // ---------- 即时保存（P18，按页路由；P63 结局三分 + P146 代次） ----------
            Message::TabAutosaved(tab_id, version, path, outcome) => {
                // P146 路由守卫：回报按「页 id」定位——防抖睡眠期间关页/
                // 换位导致的下标漂移不再串页（曾按「下标 + 路径」双重核对）。
                // inflight 必须无条件清除：曾路径失配提前 return 漏清，
                // 该页此后 tab_autosave_ready 恒 false，本会话静默失去
                // 自动保存。
                let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                    return Task::none();
                };
                let path_matches = self.tabs[idx].path.as_deref() == Some(path.as_path());
                self.tabs[idx].autosave_inflight = false;
                if !path_matches {
                    // 页已另存/改名：代次已失效（写盘必被跳过），旧路径
                    // 账目整条丢弃
                    return Task::none();
                }
                let tab = &mut self.tabs[idx];
                match outcome {
                    AutosaveOutcome::Written => {
                        // 版本一致 = 快照之后没有新编辑：可以安全清脏
                        if tab.version == version {
                            tab.dirty = false;
                            // P38：当前内容即磁盘内容，刷新落盘基线
                            tab.editor.borrow_mut().mark_saved();
                            // P50：自动保存落盘成功，同步刷新比对戳
                            tab.file_stamp = tab.path.as_deref().and_then(file_stamp);
                            // P31：auto-save 成功清脏 = 内存比清单干净，
                            // 下一拍重写清单（§3 P31 第 3 条的顺带刷新）
                            self.touch_manifest_stale();
                        }
                    }
                    AutosaveOutcome::SkippedExternalChange => {
                        // 防误报：若当前记录戳与磁盘现状一致，「外部修改」
                        // 其实是本应用自己的手动保存——它改写了磁盘并刷新
                        // 了记录戳，而在途防抖线程还拿着调度时的旧期望戳。
                        // 此时拒写作废：静默忽略，不弹提示条不打扰。
                        // 真外部改动时记录戳 ≠ 磁盘 → 照常入队裁决，
                        // 且未裁决前不重记戳（磁盘现状还没被用户确认过）。
                        let own_save_superseded = match tab.path.as_deref() {
                            Some(p) => !file_changed_externally(tab.file_stamp, file_stamp(p)),
                            None => false,
                        };
                        if !own_save_superseded {
                            let queue = self.external_change.get_or_insert_with(Vec::new);
                            if !queue.contains(&idx) {
                                queue.push(idx);
                            }
                            self.status = self.t(editpad_core::Key::StExternallyModifiedSkip).to_owned();
                        }
                    }
                    AutosaveOutcome::Failed(error) => {
                        // 失败必须留痕（不能无声吞掉），但不打断编辑；
                        // 清掉 inflight 后，下一次编辑会重新排队
                        self.set_status_error(self.t_suffix(editpad_core::Key::StAutosaveFailed, &error.to_string()));
                    }
                    AutosaveOutcome::Superseded => {
                        // P146：调度后页被编辑/撤销回基线/改路径作废——本轮
                        // 不写盘。页仍就绪（置脏+命名）则立即重排一次防抖，
                        // 保持「停手后落盘」的最终一致（否则最后一次编辑
                        // 之后不再有新触发点，自动保存静默停摆）
                        return self.maybe_schedule_autosave();
                    }
                }
                Task::none()
            }
            // ---------- 外部修改检测（P50） ----------
            Message::WindowFocused => {
                self.check_external_changes();
                Task::none()
            }
            Message::ConfirmExternalReload(idx) => {
                // 出队后重载；Loaded 归页时重记戳（失败则下次聚焦再报）
                if let Some(queue) = self.external_change.as_mut() {
                    queue.retain(|i| *i != idx);
                    if queue.is_empty() {
                        self.external_change = None;
                    }
                }
                match self.tabs.get(idx).and_then(|t| t.path.clone()) {
                    Some(path) => self.start_loading(path, idx),
                    None => Task::none(),
                }
            }
            Message::IgnoreExternalChange(idx) => {
                // 以当前磁盘状态重记戳：此后直到文件再次变化都不再提示；
                // 队列还有剩余则条上自动切到下一页（P52 聚合语义）
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.file_stamp = tab.path.as_deref().and_then(file_stamp);
                }
                if let Some(queue) = self.external_change.as_mut() {
                    queue.retain(|i| *i != idx);
                    if queue.is_empty() {
                        self.external_change = None;
                    }
                }
                Task::none()
            }
            Message::IgnoreAllExternalChanges => {
                // P52 聚合态：队列内所有页一律按磁盘现状重记戳并收条
                if let Some(queue) = self.external_change.take() {
                    for idx in queue {
                        if let Some(tab) = self.tabs.get_mut(idx) {
                            tab.file_stamp = tab.path.as_deref().and_then(file_stamp);
                        }
                    }
                }
                Task::none()
            }
            // ---------- 打开确认 ----------
            Message::ConfirmOpenDiscard => {
                let Some(path) = self.open_confirm.take() else {
                    return Task::none();
                };
                // 明确放弃：不再触发下一次确认；若关闭确认条还开着，其前提已消失
                self.tab_mut().dirty = false;
                self.confirm_visible = false;
                self.pending_close = false;
                let tab = self.target_tab_for_open();
                self.start_loading(path, tab)
            }
            Message::ConfirmOpenCancel => {
                self.open_confirm = None;
                Task::none()
            }
            // ---------- 编码与行尾（P67） ----------
            Message::ToggleEncodingMenu => {
                if self.busy {
                    return Task::none();
                }
                // 互斥：开一个关另一个
                self.eol_menu = false;
                self.encoding_menu = !self.encoding_menu;
                Task::none()
            }
            Message::ToggleEolMenu => {
                if self.busy {
                    return Task::none();
                }
                self.encoding_menu = false;
                self.eol_menu = !self.eol_menu;
                Task::none()
            }
            Message::SaveWithEncoding(encoding) => {
                self.encoding_menu = false;
                if self.busy {
                    return Task::none();
                }
                if self.tab().path.is_none() {
                    self.set_status(self.t(editpad_core::Key::StUntitledPickEncoding).to_owned());
                    return Task::none();
                }
                // 记住偏好：此后本页每次保存（含自动保存）都沿用该编码
                self.tab_mut().save_encoding = Some(encoding);
                self.save()
            }
            Message::ConvertEol(target) => {
                self.eol_menu = false;
                if self.busy || self.active_load.is_some() {
                    return Task::none();
                }
                // 单遍重排是同步操作：超大文档先挡下并提示（FormatJson 同款
                // 防冻结思路；上限放宽到 800 万字符 ≈ 24MB 文本）
                const EOL_CONVERT_MAX_CHARS: usize = 8_000_000;
                let (text, chars, current) = {
                    let ed = self.cur_handle.borrow();
                    (ed.doc.to_text(), ed.doc.text_len(), ed.doc.line_ending())
                };
                if chars > EOL_CONVERT_MAX_CHARS {
                    self.set_status_error(format!(
                        "{}{chars}{}{EOL_CONVERT_MAX_CHARS}{}",
                        self.t(
editpad_core::Key::
StTooLargeEolPrefix),
                        self.t(
editpad_core::Key::
StTooLargeEolMiddle),
                        self.t(
editpad_core::Key::
StTooLargeEolSuffix)
                    ));
                    return Task::none();
                }
                if current == target {
                    self.set_status(format!("{}{}", self.t(editpad_core::Key::StEolAlreadyMiddle), eol_label(target)));
                    return Task::none();
                }
                // P9 的归一函数即行尾转换：CRLF/LF/孤立 CR 全部统一到目标
                let new_text = target.normalize(&text);
                // replace_whole_document 内部快照 → 可撤销（与全部替换同款）
                self.cur()
                    .borrow_mut()
                    .replace_whole_document(editpad_core::Document::from_str(&new_text));
                {
                    let tab = self.tab_mut();
                    tab.dirty = true;
                    // P18：内容版本与防抖起点同步推进
                    tab.note_mutation();
                }
                self.set_status(format!("{}{}", self.t(editpad_core::Key::StEolConverted), eol_label(target)));
                // 内容变了：命中表过期重扫（查找栏开着才扫）+ 排队自动保存
                if self.find_visible {
                    let find_task = self.schedule_find_scan();
                    return Task::batch([find_task, self.maybe_schedule_autosave()]);
                }
                self.maybe_schedule_autosave()
            }
            Message::TabSaved(tab_id, version, result) => {
                self.busy = false;
                // P146：按发起页 id 定位——存盘期间页集合变动导致的下标
                // 漂移不再让「保存并关闭」落到别的页上（版本巧合时曾把
                // 无关的置脏页静默移除、内容无声丢弃）
                let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
                    self.pending_close_tab = None;
                    return Task::none();
                };
                match result {
                    Ok(()) => {
                        // 版本守卫同款：期间又有编辑则保持置脏、不关闭
                        let clean = self.tabs[idx].version == version;
                        if clean {
                            self.tabs[idx].dirty = false;
                            // P38：落盘成功且版本守卫通过——内容即磁盘内容。
                            // 本页通常随即被移除，此处是 close_tab_now 失败
                            // 等幸存路径的基线兜底
                            self.tabs[idx].editor.borrow_mut().mark_saved();
                            if self.pending_close_tab == Some(tab_id)
                                && self.close_tab_now(idx)
                            {
                                self.cancel_find_scan();
                            }
                            self.pending_close_tab = None;
                        } else {
                            self.set_status(self.t(editpad_core::Key::StCancelledAutoClose).to_owned());
                        }
                    }
                    Err(error) => {
                        self.set_status_error(self.t_suffix(editpad_core::Key::StSaveFailed, &error.to_string()));
                        self.pending_close_tab = None;
                    }
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// 启动一次后台加载：登记任务后由 [`Editpad::subscription`] 的流接管。
    ///
    /// P21：`tab` 指明结果应落入的标签页（可能等于 `tabs.len()`，
    /// 表示「新开一页」——此处先占位创建，保证路由目标恒存在）；
    /// 期间用户切走标签也不影响结果归页。
    pub(crate) fn start_loading(&mut self, path: PathBuf, tab: usize) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        // P21 内存护栏（§3 P19 总则第 2 条的骨架实现）：全部页字符量
        // 按 3 字节/字符保守估算，加上待载文件大小，超上限即拒开并提示
        let incoming = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let existing: usize = self
            .tabs
            .iter()
            .map(|t| t.editor.borrow().doc.text_len())
            .sum();
        if !mem_guard_allows(existing, incoming, MULTI_TAB_MEM_CAP_BYTES) {
            self.status = self.t(editpad_core::Key::StMemoryGuard).to_owned();
            return Task::none();
        }
        if tab >= self.tabs.len() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
        }
        self.register_load_job(path, tab);
        // P21：加载落在新页时直接切过去（符合「打开即聚焦」直觉）。
        // ⚠️ 必须经 set_active_tab 同步 cur_handle 长期别名——直接赋值
        // active_tab 会造成「下标指向新页、别名仍指旧页」的失步：
        // 输入与渲染都走别名，表现为打开文件后敲字打进上一个文档
        // （P32 本轮发现并修复；既有测试均在断言前显式切换而未暴露）。
        self.set_active_tab(tab);
        self.status.clear();
        Task::none()
    }

    /// 登记一个后台加载任务（任务号分配 + busy 置位 + 进度条复位）。
    /// 内存守卫、占位页创建与焦点切换由调用方决定：用户打开要切过去
    /// （[`Self::start_loading`]），恢复链绝不抢焦点（P30）。
    pub(crate) fn register_load_job(&mut self, path: PathBuf, tab: usize) -> u64 {
        self.job_seq += 1;
        let id = self.job_seq;
        // P145：随任务记下目标页稳定 id（0 = 页不存在，归页必走丢弃分支）
        let tab_id = self.tabs.get(tab).map(|t| t.id).unwrap_or(0);
        self.active_load = Some(LoadJob { id, path, tab, tab_id });
        self.progress = Some((0, 0));
        self.enter_busy();
        id
    }

    /// 统一的换文档入口（打开对话框/拖拽/最近文件共用）：
    /// 当前页 dirty 时绝不静默丢弃修改（含撤销链），先弹打开确认条。
    /// P21：落点 = [`Self::target_tab_for_open`]（空净当前页就地打开，否则新页）。
    pub(super) fn request_open(&mut self, path: PathBuf) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        if self.tab().dirty {
            self.open_confirm = Some(path);
            return Task::none();
        }
        let tab = self.target_tab_for_open();
        self.start_loading(path, tab)
    }

    pub(super) fn check_external_changes(&mut self) {
        if self.busy || self.active_load.is_some() {
            return;
        }
        let mut queue: Vec<usize> = Vec::new();
        for (idx, tab) in self.tabs.iter().enumerate() {
            let Some(path) = tab.path.as_deref() else {
                continue;
            };
            let Some(recorded) = tab.file_stamp else {
                continue; // 从未记录（会话恢复占位等），无从比对
            };
            if !file_changed_externally(Some(recorded), file_stamp(path)) {
                continue;
            }
            if idx == self.active_tab && !tab.dirty {
                // P130：监视页——重载前捕获 tail 跟随判定与原视图
                if tab.monitor {
                    let ed = self.cur_handle.borrow();
                    let total_h = ed.visual_rows_total() as f32 * ed.line_height();
                    let max_scroll = (total_h - ed.viewport_h).max(0.0);
                    let follow = ed.scroll_top >= max_scroll - ed.line_height() * 1.5;
                    let pre_view =
                        (ed.cursor.line, ed.cursor.col, ed.scroll_top, ed.scroll_left);
                    self.monitor_pending = Some((idx, follow, Some(pre_view)));
                }
                let path = tab.path.clone().expect("上方已判 Some");
                // 加载流由 subscription 依据 active_load 重建接管，返回的
                // Task 恒为 none——显式弃置（加载管线语义见 start_loading）
                let _ = self.start_loading(path, idx);
                return;
            }
            queue.push(idx);
        }
        self.external_change = if queue.is_empty() { None } else { Some(queue) };
    }

    /// P55：就地重命名提交——校验名称 → 磁盘改名 → 页路径/比对戳/
    /// 最近文件与光标记忆随路径迁移。失败保持输入态让用户改（状态栏
    /// 留原因）；busy/加载中拒绝提交（在途 Loaded 会用旧路径覆写页路径）。
    pub(super) fn commit_tab_rename(&mut self) -> Task<Message> {
        let Some(idx) = self.renaming_tab else {
            return Task::none();
        };
        let Some(old) = self.tabs.get(idx).and_then(|t| t.path.clone()) else {
            // 页已关/未命名：输入态自然失效
            self.renaming_tab = None;
            self.rename_input.clear();
            return Task::none();
        };
        if self.busy || self.active_load.is_some() {
            self.set_status(self.t(editpad_core::Key::StBusyRetry).to_owned());
            return Task::none();
        }
        let Some(target) = rename_target_path(&old, &self.rename_input) else {
            self.set_status_error(self.t(editpad_core::Key::StRenameInvalid).to_owned());
            return Task::none(); // 保持输入态
        };
        if target == old {
            // 名字没变：静默收摊
            self.renaming_tab = None;
            self.rename_input.clear();
            self.status.clear();
            return Task::none();
        }
        if target.exists() {
            self.set_status_error(format!("{}{}", self.t(editpad_core::Key::StRenameExists), target.display()));
            return Task::none(); // 保持输入态
        }
        match fs::rename(&old, &target) {
            Ok(()) => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    // P146：改路径前作废在途自动保存——曾以调度时刻的旧
                    // 路径落盘，新内容被写进旧文件
                    tab.invalidate_autosave();
                    tab.path = Some(target.clone());
                    // P133：解析基准随改名迁移
                    tab.editor
                        .borrow_mut()
                        .set_base_dir(target.parent().map(|p| p.to_path_buf()));
                    // P50：路径变了旧戳作废，按新路径重记
                    tab.file_stamp = file_stamp(&target);
                }
                // P20/P32：最近文件与光标记忆随路径迁移（旧路径条目失效）
                let old_key = old.display().to_string();
                let new_key = target.display().to_string();
                if let Some(view) = self.settings.recent_views.remove(&old_key) {
                    self.settings.recent_views.insert(new_key.clone(), view);
                }
                if let Some(pos) = self
                    .settings
                    .recent_files
                    .iter()
                    .position(|p| *p == old_key)
                {
                    self.settings.recent_files[pos] = new_key.clone();
                }
                self.persist_settings();
                // 会话清单里记的是旧路径，下一拍重写
                self.touch_manifest_stale();
                self.renaming_tab = None;
                self.rename_input.clear();
                self.set_status(format!("{}{new_key}", self.t(editpad_core::Key::StRenamed)));
            }
            Err(error) => {
                self.set_status_error(self.t_suffix(editpad_core::Key::StRenameFailed, &error.to_string()));
            }
        }
        Task::none()
    }

    pub(super) fn save_as_dialog(&mut self) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        self.enter_busy();
        let suggested = self.suggested_name();
        Task::perform(
            async move {
                rfd::AsyncFileDialog::new()
                    .set_file_name(&suggested)
                    .save_file()
                    .await
            },
            |handle| Message::SaveTargetChosen(handle.map(|f| f.path().to_path_buf())),
        )
    }

    pub(super) fn save(&mut self) -> Task<Message> {
        if self.busy || self.tab().path.is_none() {
            return Task::none();
        }
        // P63 外部修改守卫：磁盘现状 ≠ 记录戳 → 不落盘。场景是页置脏且
        // 应用持续聚焦期间文件被外部改动（无焦点切换事件，P50 巡检不触
        // 发），此时 Ctrl+S 会无声覆盖。拦截后把裁决交给 P52 提示条：
        // 〔忽略〕按磁盘现状重记戳，再按一次 Ctrl+S = 两步的有意覆盖；
        // 〔重新加载〕放弃本地改动。干净页同样适用（写 = 无差别覆盖）。
        let path = self.tab().path.clone().expect("上方已确认非空");
        if let Some(recorded) = self.tab().file_stamp {
            if file_changed_externally(Some(recorded), file_stamp(&path)) {
                let idx = self.active_tab;
                let queue = self.external_change.get_or_insert_with(Vec::new);
                if !queue.contains(&idx) {
                    queue.push(idx);
                }
                self.status =
                    self.t(editpad_core::Key::StExternalPaused).to_owned();
                return Task::none();
            }
        }
        self.enter_busy();
        // 第 64 轮 ⑭：写前备份磁盘旧版（同步执行——显式保存本就用户等待
        // 语义，≤64MB 复制为一次性毫秒级开销）。提示暂存到落盘完成后
        // 补显：异步 Saved 分支会重写状态栏，先写必被覆盖/抹掉
        if let Some(note) =
            crate::perform_backup_before_overwrite(&path, &self.settings.backup_mode)
        {
            // P155：备份说明是语言无关的结果类型，展示时按当前语言取文
            let lang = self.lang();
            self.pending_backup_notice = Some(note.text(lang));
        }
        // P67：按页编码偏好落盘（None = 默认 UTF-8，历史行为）
        let encoding = self
            .tab()
            .save_encoding
            .unwrap_or(editpad_core::SaveEncoding::Utf8);
        // P146：手动保存接管本页写盘——先作废在途自动保存（双写者并发
        // 曾可交错写同一目标；且自动保存回报晚于手动保存落地会搅乱账目）
        self.tab_mut().invalidate_autosave();
        // P19 行动项 3：rope 结构共享克隆（O(1)），分块原子写盘，
        // 不再经 to_text() 产生全文 String（50MB 场景省 ~50MB 峰值）
        let doc = self.cur_handle.borrow().doc.clone();
        // P18 版本守卫：记录本次落盘对应的内容版本
        let version = self.tab().version;
        // P146：随回报携带发起页 id——保存期间切页/关页不再把账目
        // 记到「完成时刻的活动页」（错清别页置脏标记 → 关页无确认丢内容）
        let tab_id = self.tab().id;
        Task::perform(
            async move {
                let saved = editpad_core::save_document_encoded(&path, &doc, encoding)
                    .map_err(|e| e.to_string());
                (version, saved)
            },
            move |(version, result)| Message::Saved(tab_id, version, result),
        )
    }

    /// 单个标签页的自动保存是否就绪：已命名、有未存改动、
    /// 无在途 IO、不与手动保存互斥、本页没有挂起任务。
    pub(super) fn tab_autosave_ready(&self, idx: usize) -> bool {
        self.settings.autosave_enabled
            && self.tabs[idx].dirty
            && self.tabs[idx].path.is_some()
            && self.active_load.is_none()
            && !self.busy
            && !self.tabs[idx].autosave_inflight
    }

    /// 编辑后调用：遍历全部标签页，把所有就绪页各排一个「睡满防抖窗 →
    /// 落盘 → 回报版本」的专用任务（P5/P10 同构的 OS 线程桥接）。
    ///
    /// 至多每页一个挂起（autosave_inflight 去重）；任务醒来落盘的是
    /// **调度时刻**的快照——若期间又有编辑，版本守卫会保持置脏，
    /// 本次编辑结束后由新任务覆盖最新内容（最终一致）。
    pub(super) fn maybe_schedule_autosave(&mut self) -> Task<Message> {
        if !self.settings.autosave_enabled || self.busy || self.active_load.is_some() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        for idx in 0..self.tabs.len() {
            if !self.tab_autosave_ready(idx) {
                continue;
            }
            let Some(path) = self.tabs[idx].path.clone() else {
                continue;
            };
            let doc = self.tabs[idx].editor.borrow().doc.clone();
            let version = self.tabs[idx].version;
            // 落盘配置随任务快照下发：保存编码（与手动保存同参，防静默
            // 转码）、外部修改比对戳、防抖窗、备份模式（后台线程无
            // &Settings/&Tab 可用）
            let task = AutosaveTask {
                encoding: self.tabs[idx]
                    .save_encoding
                    .unwrap_or(editpad_core::SaveEncoding::Utf8),
                expected_stamp: self.tabs[idx].file_stamp,
                delay: std::time::Duration::from_secs(u64::from(self.settings.autosave_delay_secs)),
                backup_mode: self.settings.backup_mode.clone(),
            };
            self.tabs[idx].autosave_inflight = true;
            // P146：带走发起页 id 与调度时刻的代次——回报按 id 归页；
            // 醒来代次不符即作废（见 drive_autosave_once）
            let tab_id = self.tabs[idx].id;
            let gen = self.tabs[idx].autosave_gen.clone();
            let my_gen = gen.load(std::sync::atomic::Ordering::Relaxed);
            tasks.push(Task::perform(
                async move {
                    drive_autosave_once(tab_id, path, doc, version, gen, my_gen, task).await
                },
                |message| message,
            ));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    pub(super) fn record_recent(&mut self, path: &Path) {
        self.settings.push_recent(path);
        self.persist_settings();
    }

    /// P32：把指定页的当前光标/滚动写进最近文件记忆（未命名页无路径跳过），
    /// 任一页有实际变化才落盘一次 config.toml。
    pub(crate) fn remember_tab_views(&mut self, indices: &[usize]) {
        let mut changed = false;
        for &idx in indices {
            let Some(path) = self.path_of_tab(idx) else {
                continue;
            };
            let ed = self.tabs[idx].editor.borrow();
            let view = editpad_core::RecentView {
                line: ed.cursor.line,
                col: ed.cursor.col,
                scroll_top: ed.scroll_top,
            };
            drop(ed);
            changed |= self.settings.set_recent_view(&path, view);
        }
        if changed {
            self.persist_settings();
        }
    }

}
