//! 标签页域消息处理（自 update.rs 拆出，纯移动零行为变更）。
//!
//! 承载标签的开关与批量关闭：新建/切换/固定/右键菜单、单页与批量关闭的
//! 确认流转、待关闭状态机、新页落点选择。

use super::*;

impl Editpad {
    /// 关闭第 `idx` 个标签页；关到最后一个时重置为新的空标签页
    /// （新页分配下一个未命名序号）。返回是否真的移除了页面。
    pub(super) fn close_tab_now(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() {
            return false;
        }
        // P32：移除前把该页光标/滚动回写最近文件记忆
        self.remember_tab_views(&[idx]);
        // 第 64 轮：命名页进「上次关闭」栈（恢复入口见 ReopenLastClosedFile）
        self.remember_closed_tab(idx);
        // P146：作废在途自动保存——「放弃更改并关闭」的页曾被人写盘复活
        self.tabs[idx].invalidate_autosave();
        self.tabs.remove(idx);
        // P112：悬停的页被关掉 → 悬停态清空（下一个指针移动事件会
        // 按新下标重新置位；不清的话陈旧下标会悬停染色到错页）
        self.hovered_tab = None;
        // P145：关闭确认条存的是裸下标——页集合变动后陈旧下标会让视图
        // 侧 tabs[idx] 越界 panic 或指向错页。确认页自身被关 → 清；
        // 其前的页被关 → 随左移平移（视图侧每帧消费该下标）。
        self.close_tab_confirm = match self.close_tab_confirm {
            Some(c) if c == idx => None,
            Some(c) if c > idx => Some(c - 1),
            other => other,
        };
        if let Some(last) = self.ensure_nonempty_tabs() {
            self.assign_untitled_num(last);
        }
        // P31：页集合结构已变——下一拍重写清单，防崩溃恢复复活已关的页
        self.touch_manifest_stale();
        // 与 tabs 对齐（含越界夹紧），并同步活动页句柄别名
        self.refresh_cur_handle();
        true
    }

    /// 批量移除多个标签页（P28）：按下标从大到小逐个 remove，
    /// 保证剩余下标始终有效；全部移光时重置一个新的空标签页
    /// （tabs 恒非空不变式，同 [`Self::close_tab_now`]）。
    /// 返回实际移除的页数。越界/重复下标安全跳过。
    pub(crate) fn close_tabs_now(&mut self, indices: &[usize]) -> usize {
        // P32：移除前按原下标批量回写光标/滚动记忆
        self.remember_tab_views(indices);
        let mut idxs = indices.to_vec();
        idxs.sort_unstable();
        idxs.dedup();
        // 第 64 轮：命名页进「上次关闭」记忆栈（单页/批量共用 remember_
        // closed_tab；逆序遍历与下方移除同序，先关的更「近」）
        for &idx in idxs.iter().rev() {
            self.remember_closed_tab(idx);
        }
        let mut removed = 0usize;
        for &idx in idxs.iter().rev() {
            if idx < self.tabs.len() {
                // P146：同 close_tab_now——批量移除也作废在途自动保存
                self.tabs[idx].invalidate_autosave();
                self.tabs.remove(idx);
                removed += 1;
            }
        }
        if removed == 0 {
            return 0;
        }
        // P112：悬停页可能在被移除之列——批量移除后悬停态统一清空
        // （下一个指针移动事件按新下标重新置位）
        self.hovered_tab = None;
        // P145：批量移除同样修正关闭确认条的下标（确认页在移除集内 →
        // 清；否则按其前方被移除的个数左移）——理由同 close_tab_now。
        self.close_tab_confirm = self.close_tab_confirm.and_then(|c| {
            if idxs.contains(&c) {
                None
            } else {
                Some(c - idxs.iter().filter(|&&i| i < c).count())
            }
        });
        if let Some(last) = self.ensure_nonempty_tabs() {
            self.assign_untitled_num(last);
        }
        self.touch_manifest_stale();
        self.refresh_cur_handle();
        removed
    }

    /// 开启批量关闭流程（P28）：目标列表非空且任一置脏 → 弹一次聚合
    /// 确认条（确认后统一放弃，§3 P28 第 3 条）；全部干净 → 直接移除。
    /// 无可关目标时静默 no-op。同一时刻只保留一条确认条。
    pub(super) fn begin_batch_close(&mut self, scope: BatchCloseScope) {
        // 菜单项无论走向如何都算「已选中」，右键菜单随之收起
        self.tab_context_menu = None;
        if self.busy {
            return;
        }
        let targets = batch_close_targets(&self.tabs, scope);
        if targets.is_empty() {
            return;
        }
        if targets.iter().any(|&i| self.tabs[i].dirty) {
            self.close_tab_confirm = None;
            self.batch_close_confirm = Some(targets);
        } else if self.close_tabs_now(&targets) > 0 {
            self.cancel_find_scan();
        }
    }

    /// 打开文件应落入的标签下标：当前页「未命名且干净且为空」→
    /// 就地打开；否则新开一页。
    pub(super) fn target_tab_for_open(&self) -> usize {
        let t = self.tab();
        let fresh = t.path.is_none() && !t.dirty && t.editor.borrow().doc.is_empty();
        if fresh {
            self.active_tab
        } else {
            self.tabs.len()
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：标签页/右键菜单/批关。臂体自原 update() 逐字搬移，零行为变更。
    pub(super) fn update_tabs(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 复制完整路径 / 文件名（第 63 轮） ----------
            // None = 活动页（热键），Some(i) = 指定页（标签右键菜单）；
            // 未命名页无路径可写，给状态栏提示
            Message::CopyFilePath(target) => self.copy_tab_ident(target, true),
            Message::CopyFileName(target) => self.copy_tab_ident(target, false),
            // P126：资源管理器定位当前文件（未命名页提示先保存）
            Message::OpenContainingFolder => {
                let path = self.tab().path.clone();
                match path {
                    Some(path) => match reveal_in_explorer(&path) {
                        Ok(()) => self.set_status(self.t(editpad_core::Key::StRevealedInExplorer).to_owned()),
                        Err(e) => self.set_status_error(self.t_suffix(editpad_core::Key::StRevealFailed, &e.to_string())),
                    },
                    None => self.set_status(self.t(editpad_core::Key::StRevealNeedsSave).to_owned()),
                }
                Task::none()
            }
            // ---------- 恢复上次关闭 / 显示标记（第 64 轮） ----------
            Message::ReopenLastClosedFile => {
                // busy 与打开确认流共用守卫语义；栈空静默
                if self.busy {
                    return Task::none();
                }
                let Some(path) = self.closed_stack.pop() else {
                    self.set_status(self.t(editpad_core::Key::StNoClosedTab).to_owned());
                    return Task::none();
                };
                // 复用打开管线：置脏走既有确认流，光标记忆由 P32 免费找回
                let tab = self.target_tab_for_open();
                self.recents_visible = false;
                self.start_loading(path, tab)
            }
            // ---------- 多标签（P21） ----------
            Message::NewTab => {
                let tab = self.fresh_tab();
                self.tabs.push(tab);
                let last = self.tabs.len() - 1;
                self.assign_untitled_num(last);
                self.set_active_tab(last);
                // 查找态全局：切页即作废旧命中，防串页
                self.cancel_find_scan();
                // P28：页集合已变，右键菜单随之下收
                self.tab_context_menu = None;
                Task::none()
            }
            // 第 76 轮：标签条空白区双击 → 新建标签页。双击判定由控件层完成
            // （strip 外层 mouse_area 的 on_double_click，iced 内核 Click
            // 时间+位置窗口）；标签自身点击被 button 消费不会产生本消息，
            // 与 P65 重命名互不干扰。busy 时忽略（与标签禁用一致）。
            Message::TabStripBlankPressed => {
                if self.busy {
                    return Task::none();
                }
                self.update(Message::NewTab)
            }
            Message::SwitchTabNext => {
                let next = (self.active_tab + 1) % self.tabs.len();
                self.set_active_tab(next);
                self.cancel_find_scan();
                self.tab_context_menu = None;
                Task::none()
            }
            Message::SwitchTabPrev => {
                let prev = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                self.set_active_tab(prev);
                self.cancel_find_scan();
                self.tab_context_menu = None;
                Task::none()
            }
            Message::SwitchTab(i) => {
                // P65：双击检测——同页在双击窗内连点两次 = 重命名意图。
                // 第二次点击照常走切换（已在活动页则无操作），随后进入
                // 就地重命名（内部自带 busy/越界守卫）。正在重命名的页
                // 其标签按钮已被输入框替换，不会再产生 SwitchTab。
                let now = std::time::Instant::now();
                let dbl = is_double_click(self.last_tab_click, i, now);
                if i < self.tabs.len() && i != self.active_tab {
                    self.set_active_tab(i);
                    self.cancel_find_scan();
                    self.tab_context_menu = None;
                }
                if dbl {
                    self.last_tab_click = None;
                    return self.update(Message::RenameOrSaveAsTab(i));
                }
                self.last_tab_click = Some((i, now));
                Task::none()
            }
            Message::TabHovered(target) => {
                // P112：页签悬停底色数据源。越界下标/离开一律复位为
                // None——关闭路径虽会清空，但此处再兜一层，防陈旧
                // 下标把悬停染色挂到错页。
                self.hovered_tab = target.filter(|&i| i < self.tabs.len());
                Task::none()
            }
            Message::CloseTabRequest => {
                let idx = self.active_tab;
                // P28：固定页对键盘路径（Ctrl+W）同样豁免，与右键菜单一致
                if self.tabs[idx].pinned {
                    self.set_status(self.t(editpad_core::Key::StPinnedMustUnpin).to_owned());
                } else if self.tabs[idx].dirty {
                    // 置脏页先确认（骨架版仅提供「放弃更改」出口）
                    self.close_tab_confirm = Some(idx);
                } else if self.close_tab_now(idx) {
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::ConfirmCloseTabDiscard(idx) => {
                self.close_tab_confirm = None;
                if idx < self.tabs.len() {
                    // 关最后一页时槽位会被复用：先清空内容与路径
                    let tab = &mut self.tabs[idx];
                    tab.dirty = false;
                    tab.path = None;
                    tab.editor.borrow_mut().set_base_dir(None);
                    tab.editor
                        .borrow_mut()
                        .reset_document(editpad_core::Document::new());
                }
                if self.close_tab_now(idx) {
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::CancelCloseTab => {
                self.close_tab_confirm = None;
                Task::none()
            }
            Message::CloseTabSave(idx) => {
                // 「保存并关闭」：已命名的置脏页先落盘，
                // TabSaved 成功且清脏后再真正移除页面
                if idx >= self.tabs.len() || self.busy {
                    return Task::none();
                }
                if self.tabs[idx].path.is_none() {
                    self.set_status(self.t(editpad_core::Key::StRenameUntitledFirst).to_owned());
                    return Task::none();
                }
                self.enter_busy();
                let path = self.tabs[idx].path.clone().expect("上方已确认非空");
                let doc = self.tabs[idx].editor.borrow().doc.clone();
                let version = self.tabs[idx].version;
                // P146：记发起页 id（存盘期间下标漂移曾致错页无确认关闭）
                // 并作废在途自动保存（手动保存接管本页写盘）
                let tab_id = self.tabs[idx].id;
                self.tabs[idx].invalidate_autosave();
                self.pending_close_tab = Some(tab_id);
                Task::perform(
                    async move {
                        let saved = editpad_core::save_document_atomic(&path, &doc)
                            .map_err(|e| e.to_string());
                        (version, saved)
                    },
                    move |(version, result)| Message::TabSaved(tab_id, version, result),
                )
            }
            // ---------- 标签右键菜单（P28；P39 起为浮层） ----------
            Message::TabContextMenu(i) => {
                // busy（对话框/IO 中）不开菜单；越界下标（页刚被关掉）忽略
                if !self.busy && i < self.tabs.len() {
                    // P39：浮层锚点 = 打开那一刻的指针位置（标签条 mouse_area 跟踪）
                    self.menu_anchor = self.cursor_pos;
                    self.tab_context_menu = Some(i);
                }
                Task::none()
            }
            Message::TabContextMenuClosed => {
                self.tab_context_menu = None;
                Task::none()
            }
            Message::TogglePinTab(i) => {
                self.tab_context_menu = None;
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.pinned = !tab.pinned;
                }
                Task::none()
            }
            Message::SaveTabFromMenu(i) => {
                self.tab_context_menu = None;
                // v1 决策：右键保存 = 先切到目标页再走既有活动页保存流——
                // Saved 回报、最近文件记录、转码提示全部复用活动页语义，
                // 不为后台页另铺一条带 idx 的回报管线。未命名置脏页自动
                // 落另存为对话框（与 Ctrl+S 同语义）。
                if !self.busy && i < self.tabs.len() && self.tabs[i].dirty {
                    self.set_active_tab(i);
                    return match self.tab().path.clone() {
                        Some(_) => self.save(),
                        None => self.save_as_dialog(),
                    };
                }
                Task::none()
            }
            Message::RenameOrSaveAsTab(i) => {
                self.tab_context_menu = None;
                if !self.busy && i < self.tabs.len() {
                    self.set_active_tab(i);
                    // P55：命名页 → 就地重命名输入框（预填当前文件名）；
                    // 未命名页保留「另存为」对话框兜底（§3 P28 第 2 条）
                    if let Some(path) = self.tabs[i].path.clone() {
                        self.renaming_tab = Some(i);
                        self.rename_input = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        // P64：聚焦 + 全选——键盘流直达，预填旧名整体可
                        // 被直接覆盖；操作在下一帧视图含该输入框后生效。
                        // P151：同时正文交出 IME 焦点（否则组字两处渲染）
                        return self
                            .focus_text_field(rename_input_id())
                            .chain(iced::widget::operation::select_all(rename_input_id()));
                    }
                    return self.save_as_dialog();
                }
                Task::none()
            }
            Message::TabRenameInputChanged(value) => {
                self.rename_input = value;
                Task::none()
            }
            Message::TabRenameCommitted => self.commit_tab_rename(),
            Message::TabRenameCancelled => {
                self.renaming_tab = None;
                self.rename_input.clear();
                self.focus_editor(); // P151：焦点还给正文
                Task::none()
            }
            Message::CloseTabAt(idx) => {
                self.tab_context_menu = None;
                if !self.busy && idx < self.tabs.len() {
                    if self.tabs[idx].pinned {
                        self.set_status(self.t(editpad_core::Key::StPinnedMustUnpin).to_owned());
                    } else if self.tabs[idx].dirty {
                        // 置脏走既有单页确认条（含「保存并关闭」出口）
                        self.batch_close_confirm = None;
                        self.close_tab_confirm = Some(idx);
                    } else if self.close_tab_now(idx) {
                        self.cancel_find_scan();
                    }
                }
                Task::none()
            }
            Message::CloseOtherTabs(keep) => {
                self.begin_batch_close(BatchCloseScope::Others(keep));
                Task::none()
            }
            Message::CloseTabsRight(from) => {
                self.begin_batch_close(BatchCloseScope::RightOf(from));
                Task::none()
            }
            Message::ConfirmBatchCloseDiscard => {
                if let Some(targets) = self.batch_close_confirm.take() {
                    // 统一放弃：先清各页置脏与内容（与 ConfirmCloseTabDiscard
                    // 同款，防「已移除页的 rope 仍被别名引用」的错觉），再移除。
                    for &idx in &targets {
                        if let Some(tab) = self.tabs.get_mut(idx) {
                            tab.dirty = false;
                            tab.path = None;
                            tab.editor.borrow_mut().set_base_dir(None);
                            tab.editor
                                .borrow_mut()
                                .reset_document(editpad_core::Document::new());
                        }
                    }
                    if self.close_tabs_now(&targets) > 0 {
                        self.cancel_find_scan();
                    }
                }
                Task::none()
            }
            Message::CancelBatchCloseTabs => {
                self.batch_close_confirm = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

}
