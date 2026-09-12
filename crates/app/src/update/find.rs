//! 查找域消息处理（自 update.rs 拆出，纯移动零行为变更）。
//!
//! 承载查找/替换、查找全部面板、在文件中查找、命中跳转与扫描生命周期。

use super::*;

impl Editpad {
    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：查找/替换/跳转/查找全部。臂体自原 update() 逐字搬移，零行为变更。
    pub(super) fn update_find(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 查找 / 替换 ----------
            Message::FindToggled => {
                self.find_visible = !self.find_visible;
                // P153：开/关都清淡出态——重开查找框恒为不透明（下一条
                // 正文点击才会再淡出），关栏时也不留悬挂的半透明标记
                self.find_dimmed = false;
                if self.find_visible {
                    self.goto_visible = false;
                    // A8：重开查找栏时不自动恢复 FIF 面板（入口显式切换）
                    self.fif_visible = false;
                    // P123：有选区则带入其文本作为查询（上限 1 万字符——
                    // 防全选大文档把查询框与扫描撑爆；正则模式下原样带入，
                    // 元字符由用户自行调整，主流「选中即查」同口径）
                    let sel_len = self.cur_handle.borrow().selection_display_len();
                    if matches!(sel_len, Some(n) if (1..=10_000).contains(&n)) {
                        let text = self.cur_handle.borrow().selected_text();
                        if let Some(text) = text {
                            self.find_query = text;
                            self.match_idx = None;
                        }
                    }
                    self.sync_find_highlights();
                    // P150/P151：打开查找栏即聚焦查询框，同时正文交出 IME 焦点
                    return Task::batch([
                        self.schedule_find_scan(),
                        self.focus_text_field(crate::view::find_input_widget_id()),
                    ]);
                } else {
                    // 关栏即取消在途扫描并清结果（旧实现只清结果）；
                    // A8：FIF 面板随栏隐藏，目录扫描一并取消
                    self.cancel_find_scan();
                    self.fif_visible = false;
                    self.cancel_fif_scan();
                    // 用户复报：关栏后左下角仍留着「第 N/M 处匹配」——该文案
                    // 由查找流程写入状态栏，关栏即一并清掉（只清自己写的）
                    if self.find_status {
                        self.status.clear();
                        self.status_is_error = false;
                        self.find_status = false;
                    }
                    self.find_drag = None;
                    // P151：焦点还给正文（否则中文输入法在正文里失效）
                    self.focus_editor();
                }
                Task::none()
            }
            // ---------- 查找浮层拖动（用户点单：浮层挡住正文时必须能拖开） ----------
            Message::FindCursorMoved(point) => {
                self.find_cursor = point;
                if let Some(last) = self.find_drag {
                    // 逐帧增量平移：位置由应用层持有，无需知道卡片布局尺寸
                    let delta = point - last;
                    let cur = self.find_pos.unwrap_or_else(|| self.default_find_pos());
                    self.find_pos = Some(self.clamp_find_pos(cur + delta));
                    self.find_drag = Some(point);
                }
                Task::none()
            }
            Message::FindDragStart => {
                // on_press 不带坐标：锚点取最近一次 on_move 的位置
                self.find_drag = Some(self.find_cursor);
                Task::none()
            }
            Message::FindDragEnd => {
                self.find_drag = None;
                Task::none()
            }
            Message::FindDragReset => {
                self.find_pos = None;
                self.find_drag = None;
                Task::none()
            }
            // P153：点回查找卡片（含拖动条/空白处、**以及输入框/按钮本身**）
            // → 淡出态复位，并把正文的键盘/IME 焦点收回来交给查找框。
            //
            // 为什么必须在这里收焦点（用户复报第四轮的操作序列：开查找框 →
            // 打进数据 → 点正文 → 回查找框打字，组字串在正文与查找框各画一份）：
            // 点正文时正文控件的 `pointer_focus(true)` 会置位（正文请求 IME
            // 并内联画组字串）；此后点查找框的按下被 PressObserver 消费
            // （必须消费，否则事件下传正文会再次夺焦，见 press_observer.rs），
            // 正文控件**再也收不到**「点区外 = 交出焦点」的信号 → `focused`
            // 一直是 true → 回到查找框打拼音时，同一组字事件被两处消费。
            // 这里用 P151 的同一把钥匙 `focus_text_field`：让出正文焦点
            // （清残留组字 + 打断打字组）+ 程序化聚焦查询框。
            Message::FindBoxPressed => {
                // 只「让出正文焦点」——**不**改焦点归属：框内被点的控件
                // 自己已经在事件派发阶段拿好焦点（输入框聚焦/按钮就绪），
                // 这里只补上正文控件因事件被消费而收不到的那次「区外按下
                // = 交出焦点」。`pointer_focus(false)` 会清掉残留组字并
                // 打断打字组，正文随即停止请求 IME（候选框不再跳正文）。
                self.cur_handle.borrow_mut().pointer_focus(false);
                self.find_dimmed = false;
                Task::none()
            }
            Message::FindQueryChanged(query) => {
                self.find_query = query;
                // P153：能在查询框里打字 = 焦点已回到查找框，淡出态一并复位
                //（点查询框但未输入字符时的复位见 `FindBoxPressed` 注释）。
                self.find_dimmed = false;
                // P10：查询变化只排队后台扫描（防抖），UI 线程零全文拷贝；
                // 查询为空时内部转为取消 + 清结果。
                // A8：FIF 模式下同一个查询框驱动的是目录扫描
                self.schedule_active_scan()
            }
            Message::FindNext => self.step_match(true),
            Message::FindPrev => self.step_match(false),
            // 第 62 轮：查找全部结果面板——纯 UI 开关，不动命中表；
            // 查找栏关闭时面板随栏隐藏（停靠在查找区内，无独立生命周期）；
            // A8：与 FIF 面板同槽互斥，FIF 开态不打开
            Message::FindAllToggled => {
                if self.find_visible && !self.fif_visible {
                    self.find_all_visible = !self.find_all_visible;
                }
                Task::none()
            }
            // 点击结果条目：按索引直接选中该命中（与 step_match 同一
            // select_span 口径）；扫描在途时命中表是过期快照，拒绝跳转
            Message::FindAllGoto(index) => {
                if !self.busy && !self.find_scanning() {
                    self.goto_match_index(index);
                }
                Task::none()
            }
            Message::CaseToggled(value) => {
                self.case_sensitive = value;
                // A8：FIF 模式下开关驱动的是目录扫描
                self.schedule_active_scan()
            }
            Message::RegexToggled(value) => {
                // P70：查询语义切换（字面转义 ↔ 正则语法），必须重扫
                self.regex_enabled = value;
                if value {
                    self.set_status(
                        self.t(editpad_core::Key::StRegexModeHint).to_owned(),
                    );
                }
                self.schedule_active_scan()
            }
            Message::WholeWordToggled(value) => {
                self.whole_word = value;
                self.schedule_active_scan()
            }
            // ---------- A8：在文件中查找 ----------
            // 模式开关：开启 = 打开查找栏 + 目录锚当前页所在目录并立即
            // 扫描；关闭 = 取消在途目录扫描（结果跨开合保留不清理）
            Message::FindInFilesToggled => {
                self.fif_visible = !self.fif_visible;
                if self.fif_visible {
                    self.find_visible = true; // 面板停靠在查找区内
                    self.find_all_visible = false; // 同槽互斥
                    self.fif_dir = self
                        .tab()
                        .path
                        .as_ref()
                        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
                    // 查询为空时 schedule_fif_scan 内部转为取消（面板给
                    // 「输入查询」提示）；装载中静默拒（与编辑同口径）
                    return self.schedule_fif_scan();
                }
                self.cancel_fif_scan();
                Task::none()
            }
            // 「浏览…」换目录（open/save 对话框同款 busy 包裹防并发）
            Message::FifBrowseFolder => {
                if self.busy {
                    return Task::none();
                }
                self.enter_busy();
                self.status.clear();
                Task::perform(
                    async { rfd::AsyncFileDialog::new().pick_folder().await },
                    |handle| Message::FifDirPicked(handle.map(|f| f.path().to_path_buf())),
                )
            }
            Message::FifDirPicked(picked) => {
                // 对话框阶段结束：busy 若不清零，会撞上后续守卫导致卡死
                self.busy = false;
                if let Some(dir) = picked {
                    self.fif_dir = Some(dir);
                    return self.schedule_fif_scan();
                }
                Task::none()
            }
            // 目录扫描完成：seq 过期的结果丢弃（FindScanDone 同构）
            Message::FifScanDone(seq, results, truncated) => {
                if self.fif_scan == Some(seq) {
                    self.fif_scan = None;
                    let files = results.len();
                    let hits: usize = results.iter().map(|f| f.hits.len()).sum();
                    self.fif_results = results;
                    self.fif_truncated = truncated;
                    if truncated {
                        self.set_status(format!(
                            "{}{files}{}{hits}{}",
                            self.t(
editpad_core::Key::
StFifTruncatedPrefix),
                            self.t(
editpad_core::Key::
StFifTruncatedMiddle),
                            self.t(
editpad_core::Key::
StFifTruncatedSuffix)
                        ));
                    } else {
                        self.set_status(editpad_core::fmt_fif_summary(self.lang(), files, hits));
                    }
                }
                Task::none()
            }
            // 点击命中：已开页切换并选中该命中；未开页走打开管线，
            // 装载结算（Loaded）后一次性消费 select_span（P133 同构）
            Message::FifGoto(file_idx, hit_idx) => {
                if self.busy || self.active_load.is_some() {
                    return Task::none();
                }
                let Some(fh) = self.fif_results.get(file_idx) else {
                    return Task::none();
                };
                let Some(h) = fh.hits.get(hit_idx) else {
                    return Task::none();
                };
                let (path, line, col, len) =
                    (fh.path.clone(), h.pos.line, h.pos.col, h.pos.len_chars);
                if let Some(idx) = self
                    .tabs
                    .iter()
                    .position(|t| t.path.as_deref() == Some(path.as_path()))
                {
                    if idx != self.active_tab {
                        self.set_active_tab(idx);
                    }
                    self.cur_handle.borrow_mut().select_span(line, col, len);
                    return Task::none();
                }
                self.pending_fif_goto = Some((line, col, len));
                self.request_open(path)
            }
            Message::ReplaceQueryChanged(query) => {
                self.replace_query = query;
                Task::none()
            }
            Message::ReplaceCurrent => self.replace_current(),
            Message::ReplaceCurrentRegex => {
                // P70：正则模式的「替换当前」——重选当前命中跨度（命中表
                // 可能比选区新），用原始命中文本（含真实 \r\n）做单次展开
                // 替换。无当前命中时先定位第一个（FindScanDone 会清
                // match_idx，等价「按一次下一个」），与用户直觉一致。
                if self.busy || self.find_query.is_empty() {
                    return Task::none();
                }
                if self.match_idx.is_none() {
                    // step_match 恒返回 none，弃置安全
                    let _ = self.step_match(true);
                }
                let Some(pos) = self.match_idx.and_then(|i| self.matches.get(i).copied()) else {
                    return Task::none();
                };
                // P146 防护：pos 来自上一轮扫描的陈旧命中表——编辑删行后
                // 重扫完成前行数可能已少于 pos.line。越界时 select_span/
                // ropey line() 曾直接 panic（全库唯一未防护点）；宁可放弃
                // 本次替换并排队重扫，也不在夹紧后的错误位置写入文本
                if pos.line >= self.cur_handle.borrow().doc.line_count() {
                    return self.schedule_find_scan();
                }
                self.cur_handle
                    .borrow_mut()
                    .select_span(pos.line, pos.col, pos.len_chars);
                let matched = self.cur_handle.borrow().selected_text();
                let Some(matched) = matched else {
                    // 零宽命中（如 `a*` 的空匹配）：没有命中文本可供
                    // Regex::replace。不得回落字面 replace_current——那会
                    // 把正则串当字面量匹配。零宽命中必为单行，改在所在
                    // 行窗口内按位置展开替换文本（支持 $1）。
                    let expansion = {
                        let ed = self.cur_handle.borrow();
                        let line_text = ed.doc.line_str(pos.line);
                        let byte_in_line: usize = line_text
                            .chars()
                            .take(pos.col)
                            .map(char::len_utf8)
                            .sum();
                        editpad_core::expand_regex_at(
                            &line_text,
                            byte_in_line,
                            &self.find_query,
                            &self.replace_query,
                            self.case_sensitive,
                        )
                    };
                    return match expansion {
                        Ok(expanded) => {
                            self.cur_handle.borrow_mut().replace_selection(&expanded);
                            self.tab_mut().dirty = true;
                            self.tab_mut().note_mutation();
                            // 命中表已过期：排队重扫（「下一个」等重扫完成）
                            self.schedule_find_scan()
                        }
                        Err(e) => {
                            self.set_status_error(self.t_suffix(editpad_core::Key::StInvalidRegex, &e.to_string()));
                            Task::none()
                        }
                    };
                };
                match editpad_core::compile_regex(&self.find_query, self.case_sensitive) {
                    Ok(re) => {
                        let expanded = re
                            .replace(&matched, self.replace_query.as_str())
                            .into_owned();
                        self.cur_handle.borrow_mut().replace_selection(&expanded);
                        self.tab_mut().dirty = true;
                        self.tab_mut().note_mutation();
                        // 命中表已过期：排队重扫（「下一个」等重扫完成）
                        self.schedule_find_scan()
                    }
                    Err(e) => {
                        self.set_status_error(self.t_suffix(editpad_core::Key::StInvalidRegex, &e.to_string()));
                        Task::none()
                    }
                }
            }
            Message::ReplaceAll => {
                if self.busy || self.find_query.is_empty() || self.find_scanning() {
                    // 扫描在途时禁止全部替换：此刻的全文快照可能是过期的
                    return Task::none();
                }
                // P70：正则分支——全文 to_text + fancy-regex 替换（$1 组引用）。
                // 与 FormatJson 同款防冻结上限（to_text + 结果双份内存）。
                // P148：替换本体移出 UI 线程——回溯引擎对病态模式 + 大文档
                // 曾冻结整个应用（回溯限制的是单次尝试步数，全文逐位置尝试
                // 总量无界）。busy 包裹挡并发编辑 ⇒ 回报内容与发起时刻必然
                // 一致，无需版本复核；「必回一条消息」纪律由 Task 语义保证。
                if self.regex_enabled {
                    const REGEX_REPLACE_MAX_CHARS: usize = 4_000_000;
                    let (text, chars) = {
                        let ed = self.cur_handle.borrow();
                        (ed.doc.to_text(), ed.doc.text_len())
                    };
                    if chars > REGEX_REPLACE_MAX_CHARS {
                        self.set_status_error(format!(
                            "{}{chars}{}{REGEX_REPLACE_MAX_CHARS}{}",
                            self.t(
editpad_core::Key::
StTooLargeRegexPrefix),
                            self.t(
editpad_core::Key::
StTooLargeRegexMiddle),
                            self.t(
editpad_core::Key::
StTooLargeRegexSuffix)
                        ));
                        return Task::none();
                    }
                    let pattern = self.find_query.clone();
                    let replacement = self.replace_query.clone();
                    let case_sensitive = self.case_sensitive;
                    self.enter_busy();
                    self.set_status(self.t(editpad_core::Key::StRegexReplacing).to_owned());
                    return Task::perform(
                        async move {
                            editpad_core::replace_all_regex(
                                &text, &pattern, &replacement, case_sensitive,
                            )
                        },
                        Message::ReplaceAllRegexDone,
                    );
                }
                // 整词模式（仅字面查询）：rope 流式路径不做词边界判定，
                // 改走全文两遍法；文档上限与正则分支同口径防冻结
                if self.whole_word {
                    const WHOLE_WORD_MAX_CHARS: usize = 4_000_000;
                    let (text, chars, eol) = {
                        let ed = self.cur_handle.borrow();
                        (ed.doc.to_text(), ed.doc.text_len(), ed.doc.line_ending())
                    };
                    if chars > WHOLE_WORD_MAX_CHARS {
                        self.set_status_error(format!(
                            "{}{chars}{}{WHOLE_WORD_MAX_CHARS}{}",
                            self.t(
editpad_core::Key::
StTooLargeWordPrefix),
                            self.t(
editpad_core::Key::
StTooLargeWordMiddle),
                            self.t(
editpad_core::Key::
StTooLargeWordSuffix)
                        ));
                        return Task::none();
                    }
                    let query = eol.normalize(&unescape_query(&self.find_query));
                    let replacement = eol.normalize(&unescape_query(&self.replace_query));
                    let (new_contents, count) = editpad_core::replace_all_word(
                        &text,
                        &query,
                        &replacement,
                        self.case_sensitive,
                    );
                    let mut tasks: Vec<Task<Message>> = Vec::new();
                    if count > 0 {
                        self.cur().borrow_mut().replace_whole_document(
                            editpad_core::Document::from_str(&new_contents),
                        );
                        self.tab_mut().dirty = true;
                        self.tab_mut().note_mutation();
                        tasks.push(self.schedule_find_scan());
                        tasks.push(self.maybe_schedule_autosave());
                    }
                    self.set_status(editpad_core::fmt_replaced(self.lang(), count));
                    return Task::batch(tasks);
                }
                // P11：直接在 rope 上流式替换，省掉 to_text() 全文拷贝
                // P22 补充：查询与替换文本先做转义解析（\n \r \t \\）
                let (new_contents, count) = {
                    let editor = self.cur_handle.borrow();
                    editpad_core::replace_all_document(
                        &editor.doc,
                        &unescape_query(&self.find_query),
                        &unescape_query(&self.replace_query),
                        self.case_sensitive,
                    )
                };
                if count > 0 {
                    self.cur()
                        .borrow_mut()
                        .replace_whole_document(editpad_core::Document::from_str(&new_contents));
                    self.tab_mut().dirty = true;
                    // P18：内容版本与防抖起点同步推进
                    self.tab_mut().note_mutation();
                }
                // P10：替换后的重扫走后台防抖，不再同步刷
                let mut tasks = vec![self.schedule_find_scan()];
                if count > 0 {
                    // P18：内容变了 → 排队一次防抖自动保存
                    tasks.push(self.maybe_schedule_autosave());
                }
                self.set_status(editpad_core::fmt_replaced(self.lang(), count));
                Task::batch(tasks)
            }
            Message::ReplaceAllRegexDone(result) => {
                // P148：后台正则替换落账。busy 包裹期间文档不可变——回报
                // 内容与发起时刻一致，无需版本复核。替换结果按主导行尾归一
                // 后整体入主：正则替换文本里的裸换行不得在 CRLF 文档里
                // 制造混合行尾（字面路径的归一已在 core 内完成）。
                self.busy = false;
                match result {
                    Ok((new_contents, count)) => {
                        let mut tasks: Vec<Task<Message>> = Vec::new();
                        if count > 0 {
                            let eol = self.cur_handle.borrow().doc.line_ending();
                            let new_contents = eol.normalize(&new_contents);
                            self.cur().borrow_mut().replace_whole_document(
                                editpad_core::Document::from_str(&new_contents),
                            );
                            self.tab_mut().dirty = true;
                            self.tab_mut().note_mutation();
                            tasks.push(self.schedule_find_scan());
                            tasks.push(self.maybe_schedule_autosave());
                        }
                        self.set_status(editpad_core::fmt_replaced(self.lang(), count));
                        if tasks.is_empty() {
                            Task::none()
                        } else {
                            Task::batch(tasks)
                        }
                    }
                    Err(e) => {
                        self.set_status_error(self.t_suffix(editpad_core::Key::StRegexReplaceFailed, &e.to_string()));
                        Task::none()
                    }
                }
            }
            Message::FindScanDone(seq, found) => {
                // 过期结果丢弃：只认当前排队中的那次扫描（P10 的 job 序号过滤，
                // 与 Loaded 按 job_id 过滤同构）
                if self.find_scan == Some(seq) {
                    self.find_scan = None;
                    self.matches = found;
                    self.match_idx = None;
                    // P123：新命中表同步视口高亮层
                    self.sync_find_highlights();
                }
                Task::none()
            }
            // ---------- 跳转 ----------
            Message::GotoToggled => {
                self.goto_visible = !self.goto_visible;
                if self.goto_visible {
                    self.find_visible = false;
                    // P123：查找栏被跳转栏顶掉时同步清视口高亮（命中表
                    // 保留，重新开栏即恢复）
                    self.sync_find_highlights();
                }
                Task::none()
            }
            Message::GotoInputChanged(value) => {
                self.goto_input = value;
                Task::none()
            }
            // ---------- P130：文件监视（tail 跟随） ----------
            Message::ToggleMonitorFile => {
                if self.busy {
                    return Task::none();
                }
                let on = {
                    let tab = self.tab_mut();
                    tab.monitor = !tab.monitor;
                    tab.monitor
                };
                self.set_status(if on {
                    self.t(editpad_core::Key::StMonitorOn).to_owned()
                } else {
                    self.t(editpad_core::Key::StMonitorOff).to_owned()
                });
                // P149：监视节拍已改订阅时钟驱动（subscription 按「存在
                // 监视页」门控），开关翻转不再手动起链
                Task::none()
            }
            Message::MonitorTick => {
                // P149：节拍已改订阅时钟驱动（订阅随「无监视页」自动撤销），
                // 本臂只做巡检。busy/加载中的拍跳过，下一拍自然重试。
                if self.tabs.iter().any(|t| t.monitor)
                    && !self.busy
                    && self.active_load.is_none()
                {
                    self.check_external_changes();
                }
                Task::none()
            }
            // ---------- P129：命令面板 / 快速标签切换 ----------
            Message::PaletteToggled(mode) => {
                if self.palette_visible && self.palette_mode == mode {
                    // 同模式再按 = 关闭
                    self.palette_visible = false;
                } else {
                    self.palette_visible = true;
                    self.palette_mode = mode;
                    self.palette_input.clear(); // 模式各用各的查询语境
                    self.palette_idx = 0;
                }
                if self.palette_visible {
                    // P151：焦点交给面板输入框，同时正文交出 IME 焦点
                    self.focus_text_field(crate::palette_input_id())
                } else {
                    self.focus_editor();
                    Task::none()
                }
            }
            Message::PaletteInputChanged(value) => {
                self.palette_input = value;
                self.palette_idx = 0;
                Task::none()
            }
            Message::PaletteMove(down) => {
                let n = self.palette_filtered().len();
                if n > 0 {
                    self.palette_idx = if down {
                        (self.palette_idx + 1).min(n - 1)
                    } else {
                        self.palette_idx.saturating_sub(1)
                    };
                }
                Task::none()
            }
            Message::PaletteExecute => self.palette_execute(),
            Message::PalettePick(i) => {
                self.palette_idx = i;
                self.palette_execute()
            }
            // ---------- B9 列编辑器对话框 ----------
            Message::ColumnEditorToggled => {
                if self.column_editor_visible {
                    self.column_editor_visible = false;
                    return Task::none();
                }
                // 打开前置守卫：busy 与软换行拒绝；无列块拒绝（块是插入
                // 的唯一目标，预告提示比打开空对话框更省一步）
                if self.busy {
                    return Task::none();
                }
                if self.cur_handle.borrow().wrap_enabled() {
                    self.set_status_error(self.t(editpad_core::Key::StColumnEditorWrapOff));
                    return Task::none();
                }
                if !self.cur_handle.borrow().has_block() {
                    self.set_status_error(self.t(editpad_core::Key::StColumnEditorNeedBlock));
                    return Task::none();
                }
                // 浮层互斥：对话框与设置弹窗/命令面板不同框
                self.settings_visible = false;
                self.palette_visible = false;
                self.column_editor_visible = true;
                Task::none()
            }
            Message::ColumnEditorModeToggled => {
                self.column_editor.number_mode = !self.column_editor.number_mode;
                Task::none()
            }
            Message::ColumnEditorTextChanged(v) => {
                self.column_editor.text = v;
                Task::none()
            }
            Message::ColumnEditorStartChanged(v) => {
                self.column_editor.start = v;
                Task::none()
            }
            Message::ColumnEditorStepChanged(v) => {
                self.column_editor.step = v;
                Task::none()
            }
            Message::ColumnEditorWidthChanged(v) => {
                self.column_editor.pad_width = v;
                Task::none()
            }
            Message::ColumnEditorBaseCycled => {
                use crate::editor::NumBase as B;
                self.column_editor.base = match self.column_editor.base {
                    B::Dec => B::Hex,
                    B::Hex => B::Bin,
                    B::Bin => B::Oct,
                    B::Oct => B::Dec,
                };
                Task::none()
            }
            Message::ColumnEditorHexUpperToggled => {
                self.column_editor.hex_upper = !self.column_editor.hex_upper;
                Task::none()
            }
            Message::ColumnEditorConfirmed => self.column_editor_confirm(),
            Message::GotoSubmit => match self.goto_input.trim().parse::<usize>() {
                Ok(n) if n >= 1 => {
                    self.cur_handle.borrow_mut().jump_to_line(n);
                    self.goto_visible = false;
                    self.status.clear();
                    Task::none()
                }
                _ => {
                    self.set_status_error(self.t(editpad_core::Key::GotoInvalidLine).to_owned());
                    Task::none()
                }
            },
            _ => Task::none(),
        }
    }

}
