//! 会话恢复：启动恢复、清单解析、逐页装载与占位页收尾、崩溃恢复确认。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

impl Editpad {
    /// 启动时会话恢复入口：开关任一关闭 = 空白启动（存量会话清场，
    /// 「只关开关不清数据等于没关」沿用 P20 先例）；清单缺失/损坏/空页
    /// = 无会话；clean_exit=false（上次异常退出，P31 心跳的中间态即此
    /// 形态）= 弹一次性恢复提示等用户裁决；否则静默全量还原。
    pub(crate) fn boot_restore(&mut self) -> Task<Message> {
        if !session_restore_allowed(
            self.settings.enable_snapshots,
            self.settings.remember_session,
        ) {
            // enable_snapshots=false 的清场已在 new() 做过；这里补上
            // remember_session=false 的清场
            if !self.settings.remember_session {
                if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                    editpad_core::snapshot::clear_session(&dir);
                }
            }
            return Task::none();
        }
        match editpad_core::snapshot::snapshot_dir() {
            Some(dir) => self.restore_from_dir(&dir),
            None => Task::none(),
        }
    }

    /// 从注入的快照目录执行恢复决策。生产路径经 [`Self::boot_restore`]；
    /// 测试注入项目内目录，避免触碰真实 %APPDATA%（同 P29 先例）。
    pub(crate) fn restore_from_dir(&mut self, dir: &Path) -> Task<Message> {
        let Some(manifest) = editpad_core::snapshot::read_manifest(dir) else {
            return Task::none(); // 无会话/损坏清单一律按空白启动，绝不 panic
        };
        if manifest.tabs.is_empty() {
            return Task::none();
        }
        if !manifest.clean_exit {
            // 孤儿检测 = 崩溃恢复入口（§3 P30 第 2 条）：弹一次性提示，
            // 数据原封留在磁盘，等用户选择恢复或丢弃
            self.recover_prompt = Some(manifest);
            return Task::none();
        }
        self.restore_from_manifest(dir, &manifest)
    }

    /// 按清单重建标签页（P30 主路径）：
    /// * 未命名页与置脏命名页从快照**同步**还原内容——v1 一律信快照
    ///   （§3 P30 第 3 条，主流编辑器「所见即所得」），不从磁盘重载覆盖；
    /// * 干净命名页建占位页排队，逐个经既有加载管线回填（失败跳过不阻断）；
    /// * 未命名编号延续单调性；激活页最后切换；
    /// * 内存护栏在规划期整体截断（先保激活页）。
    pub(crate) fn restore_from_manifest(
        &mut self,
        dir: &Path,
        manifest: &editpad_core::snapshot::SessionManifest,
    ) -> Task<Message> {
        let (kept, dropped) =
            plan_restore_order(manifest, dir, MULTI_TAB_MEM_CAP_BYTES);
        self.restore_dropped = dropped;
        self.restore_failed = 0;

        let mut tabs: Vec<Tab> = Vec::with_capacity(kept.len());
        let mut queue: Vec<RestoreLoad> = Vec::new();
        let mut next_untitled = 2u64;

        for &i in &kept {
            let meta = &manifest.tabs[i];
            let mut tab = self.fresh_tab();
            // P134：每页显示覆盖随会话恢复（C7）——fresh_tab 落的是全局
            // 有效值，此处按清单覆盖并回填编辑器（三个分支共用同一占位
            // 页编辑器；Loaded 路径不触碰换行/字号，无需再补发）
            tab.wrap_override = meta.wrap_override;
            tab.font_size_override = meta.font_size_override;
            {
                let mut ed = tab.editor.borrow_mut();
                ed.set_word_wrap(tab.wrap_override.unwrap_or(self.settings.word_wrap));
                ed.set_font_size(editor::normalize_font_size(
                    tab.font_size_override.unwrap_or(self.settings.font_size),
                ));
            }
            match (&meta.path, meta.file.as_deref()) {
                // ---- 未命名页：内容只可能来自快照 ----
                (None, _) => {
                    tab.untitled_num = meta.untitled_num;
                    if let Some(n) = meta.untitled_num {
                        next_untitled = next_untitled.max(n.saturating_add(1));
                    }
                    if meta.dirty {
                        match editpad_core::snapshot::read_page(dir, meta) {
                            Some(doc) => {
                                {
                                    let mut ed = tab.editor.borrow_mut();
                                    ed.reset_document(doc);
                                    // P38：快照内容不是磁盘内容，不得当落盘
                                    // 基线——否则撤销回快照态会错误清脏，
                                    // 未存改动在零询问退出后静默丢失
                                    ed.clear_saved_baseline();
                                    ed.restore_view(
                                        meta.cursor_line,
                                        meta.cursor_col,
                                        meta.scroll_top,
                                        meta.scroll_left,
                                    );
                                    // 内容嗅探让未命名草稿同样享受配色
                                    let sample = head_sample(&ed.doc);
                                    ed.set_language_by_name(
                                        editpad_core::resolve_language(None, &sample)
                                            .as_deref(),
                                    );
                                }
                                tab.dirty = true;
                            }
                            None => {
                                // 快照缺失/损坏：内容已不可得——留空页继续，
                                // 计入失败汇总（单页失败不阻断，§3 P30 第 6 条）
                                self.restore_failed += 1;
                            }
                        }
                    }
                }
                // ---- 置脏命名页：信快照（所见即所得） ----
                (Some(path), Some(_)) => {
                    match editpad_core::snapshot::read_page(dir, meta) {
                        Some(doc) => {
                            {
                                let mut ed = tab.editor.borrow_mut();
                                ed.reset_document(doc);
                                // P38：同未命名页——快照 ≠ 磁盘，基线保守置空，
                                // 撤销回快照态不得清脏（磁盘还是旧内容）
                                ed.clear_saved_baseline();
                                ed.restore_view(
                                    meta.cursor_line,
                                    meta.cursor_col,
                                    meta.scroll_top,
                                    meta.scroll_left,
                                );
                                let sample = head_sample(&ed.doc);
                                ed.set_language_by_name(
                                    editpad_core::resolve_language(
                                        Some(Path::new(path)),
                                        &sample,
                                    )
                                    .as_deref(),
                                );
                            }
                            tab.path = Some(PathBuf::from(path));
                            // 快照恒为 UTF-8 落盘；原文件编码知情权随下次保存归一
                            tab.encoding_label = "UTF-8".to_owned();
                            tab.dirty = true;
                        }
                        None => {
                            // 快照读不出（半截写盘等）：退化为按干净命名页
                            // 从磁盘加载——内容退回上次保存态的现实兜底，
                            // 但未存改动确实丢了，计入失败汇总如实告知
                            self.restore_failed += 1;
                            queue.push(RestoreLoad {
                                path: PathBuf::from(path),
                                tab: tabs.len(),
                                cursor_line: meta.cursor_line,
                                cursor_col: meta.cursor_col,
                                scroll_top: meta.scroll_top,
                                scroll_left: meta.scroll_left,
                            });
                        }
                    }
                }
                // ---- 干净命名页：占位 + 排队走既有加载管线 ----
                (Some(path), None) => {
                    if meta.dirty {
                        // P31 超限节流页（置脏但心跳从未落其内容）：
                        // 退回磁盘上次保存态，未存改动不可得——如实入汇总
                        self.restore_failed += 1;
                    }
                    queue.push(RestoreLoad {
                        path: PathBuf::from(path),
                        tab: tabs.len(),
                        cursor_line: meta.cursor_line,
                        cursor_col: meta.cursor_col,
                        scroll_top: meta.scroll_top,
                        scroll_left: meta.scroll_left,
                    });
                }
            }
            tabs.push(tab);
        }

        self.tabs = tabs;
        // P147：全部页超内存护栏被规划放弃时 `kept` 为空——必须补一个
        // 空页保持「tabs 恒非空」不变式，否则下方 tabs[active_tab] 直接
        // 越界（启动即崩，需手删快照目录才能恢复；触发场景：退出快照无
        // 单页上限，文档编辑超 256MB 后正常退出，或干净页被外部增大）
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
        }
        // 激活页最后还原；编号计数取「清单值」与「实际用号+1」的较大者
        self.active_tab = manifest.active.min(self.tabs.len().saturating_sub(1));
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
        next_untitled = next_untitled.max(manifest.next_untitled);
        self.untitled_next = next_untitled;

        self.restore_queue = queue;
        self.restore_pending = self.restore_queue.len();
        self.begin_restore_load()
    }

    /// 恢复链：载入队列中的下一个命名页（队列空则收尾出汇总）。
    /// 与用户打开的区别：绝不切焦点、不做逐次内存守卫（规划期已整体
    /// 截断）、登记待还原视图供 Loaded 回填光标滚动。
    pub(crate) fn begin_restore_load(&mut self) -> Task<Message> {
        // 队列语义必须 FIFO（remove(0) 而非 pop()）：加载顺序 = 清单下标
        // 顺序，占位页与排队项的下标对应关系才不会错位（LIFO 会让
        // 「失败移除」作用在错误的页上——回归测试现场抓过）
        if self.restore_queue.is_empty() {
            self.finish_restore_summary();
            return Task::none();
        }
        let entry = self.restore_queue.remove(0);
        if self.busy || self.active_load.is_some() {
            // 载入通道被占用（理论不可达：恢复链独占调度）：塞回队首等下轮
            self.restore_queue.insert(0, entry);
            return Task::none();
        }
        let id = self.register_load_job(entry.path, entry.tab);
        self.restore_views
            .insert(id, (entry.cursor_line, entry.cursor_col, entry.scroll_top, entry.scroll_left));
        Task::none()
    }

    /// 恢复链的一步收尾：待载数递减；队列排空时出汇总状态。
    /// 返回是否需要续排下一页（Loaded 处理器据此链接任务）。
    pub(crate) fn settle_restore_step(&mut self) -> bool {
        self.restore_pending = self.restore_pending.saturating_sub(1);
        if self.restore_queue.is_empty() {
            self.finish_restore_summary();
            false
        } else {
            true
        }
    }

    /// 恢复收尾汇总：只有出现值得告知的情况才打扰状态栏
    /// （失败页/截断页）；全部成功则保持安静，界面本身即是恢复事实。
    pub(super) fn finish_restore_summary(&mut self) {
        let mut notes: Vec<String> = Vec::new();
        if self.restore_failed > 0 {
            notes.push(editpad_core::fmt_restore_note(self.lang(), self.restore_failed, false));
        }
        if self.restore_dropped > 0 {
            notes.push(editpad_core::fmt_restore_note(self.lang(), self.restore_dropped, true));
        }
        if !notes.is_empty() {
            self.set_status(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StSessionRestored, &notes.join(self.t(editpad_core::Key::ListSeparator))));
        }
        self.restore_failed = 0;
        self.restore_dropped = 0;
        self.restore_pending = 0;
    }

    /// 移除第 idx 个恢复占位页，并把队列中大于 idx 的目标下标整体前移
    /// （页面移除后，后续排队页的下标随之左移）。仅当该页仍是空净无名
    /// 占位页才动手——防误删恢复期间用户产生的内容。
    pub(crate) fn drop_restore_placeholder(&mut self, idx: usize) {
        let placeholder_ok = self.tabs.get(idx).is_some_and(|t| {
            t.path.is_none()
                && !t.dirty
                && t.untitled_num.is_none()
                && t.encoding_label.is_empty()
                && t.editor.borrow().doc.is_empty()
        });
        if !placeholder_ok {
            return;
        }
        self.tabs.remove(idx);
        // 占位页也走同一收口（不变量见 Editpad::ensure_nonempty_tabs）：
        // 恢复期间删掉唯一占位页而队列尚有后续时，不能让 tabs 变空。
        let _ = self.ensure_nonempty_tabs();
        for entry in &mut self.restore_queue {
            if entry.tab > idx {
                entry.tab -= 1;
            }
        }
        // active_tab 可能越界：与 tabs 对齐并同步别名
        self.refresh_cur_handle();
    }

    /// 目标页是否仍是待填充的恢复占位页（防串写护栏）：恢复期间用户
    /// 关页/新建页会让队列下标漂移，宁可丢弃结果也不能覆盖用户内容。
    pub(crate) fn restore_placeholder_ready(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|t| {
            t.path.is_none()
                && !t.dirty
                && t.untitled_num.is_none()
                && t.encoding_label.is_empty()
                && t.editor.borrow().doc.is_empty()
        })
    }

    /// 崩溃恢复提示条「恢复」：按暂存清单全量重建（含脏页内容）。
    pub(crate) fn accept_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
        let Some(manifest) = self.recover_prompt.take() else {
            return Task::none();
        };
        match dir {
            Some(dir) => self.restore_from_manifest(&dir, &manifest),
            // 目录没了：无从恢复；数据仍在磁盘原处，本次空白起步
            None => Task::none(),
        }
    }

    /// 「丢弃」= 连快照一起丢（P29 放弃语义同族）：清场后空白起步，
    /// 防止下次启动把已放弃的内容再次当会话恢复回来。
    pub(crate) fn discard_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
        self.recover_prompt = None;
        if let Some(dir) = dir {
            editpad_core::snapshot::clear_session(&dir);
        }
        Task::none()
    }

}
