use super::*;
use super::settings_ui::SettingsPage;

impl Editpad {
    // ---------- 多标签访问器（P21） ----------

    /// 新建标签页的唯一出口：`Tab::empty()` + 套用全局字号设置。
    ///
    /// 历史教训（P43）：曾有多处直接用 `Tab::empty()` 建页——`EditorCore`
    /// 默认字号恒为 16，恢复会话/新建页/关光补页全部脱离设置里的字号，
    /// 表现为「调到 24 生效、重启后字体回到原大小，但设置面板仍显示 24」
    /// （恢复路径整体替换 tabs，boot 仅对首个默认页应用过字号）。
    /// 今后凡「新建页」一律走这里；boot 首帧仍显式 set_font_size 兜底
    /// （fresh_tab 幂等，重复应用无害）。
    pub(crate) fn fresh_tab(&mut self) -> Tab {
        let tab = Tab::empty();
        // 设置里的字号可能未归一（旧配置/手改），与 boot 同一 clamp 规则
        tab.editor
            .borrow_mut()
            .set_font_size(editor::normalize_font_size(self.settings.font_size));
        // 第 64 轮：不可见字符标记与字号同口径下发（fresh_tab 幂等）
        tab.editor.borrow_mut().set_invisibles(
            self.settings.show_whitespace,
            self.settings.show_line_endings,
        );
        // 第 73 轮 ⑯：自动换行（软换行）同口径下发（fresh_tab 幂等）
        tab.editor.borrow_mut().set_word_wrap(self.settings.word_wrap);
        tab
    }

    /// 进入 busy（对话框/IO 互斥）。P39 浮层化后的新约束：同时收起
    /// 右键菜单浮层——它的透明背板会挡住整窗点击，busy 期间不能留它挡道
    /// （旧内嵌面板无此问题，故此前各 busy 置位点都无需理会菜单态）。
    pub(crate) fn enter_busy(&mut self) {
        self.busy = true;
        self.tab_context_menu = None;
        // P67：状态栏弹出菜单同样会被 busy 互斥挡住，直接收起
        self.encoding_menu = false;
        self.eol_menu = false;
    }

    /// 当前激活标签页。
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// 当前激活标签页（可变）。
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        let i = self.active_tab;
        &mut self.tabs[i]
    }

    /// 活动页句柄的长期引用（跨语句借用走 `cur_handle` 字段；
    /// 单表达式内的临时借用也可用 [`Self::cur`]）。
    fn cur(&self) -> EditorHandle {
        self.cur_handle.clone()
    }

    /// 为第 `idx` 页分配下一个未命名序号（P25）：全局单调、不复用
    /// 已关闭页的号码——杜绝两个同名未命名页。
    fn assign_untitled_num(&mut self, idx: usize) {
        let n = self.untitled_next;
        self.untitled_next += 1;
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.untitled_num = Some(n);
        }
    }

    /// 切换活动页并同步长期别名（所有 active_tab 变更必须经此或
    /// [`Self::refresh_cur_handle`])。
    pub(crate) fn set_active_tab(&mut self, idx: usize) {
        self.active_tab = idx.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 与 tabs 对齐刷新别名（增删页后调用）。
    pub(crate) fn refresh_cur_handle(&mut self) {
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 任一标签页有未保存改动（窗口关闭确认的聚合口径）。
    pub(crate) fn any_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    /// 第 `idx` 页的路径（存在该页时）。
    fn path_of_tab(&self, idx: usize) -> Option<PathBuf> {
        self.tabs.get(idx).and_then(|t| t.path.clone())
    }

    /// 第 64 轮：命名页关闭时进「上次关闭」记忆栈（会话内，最近期在前；
    /// 连续重复只保最近一位），容量 10 截断。单页/批量两条移除漏斗
    /// 都要在移除前调用。
    fn remember_closed_tab(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get(idx) {
            if let Some(p) = &tab.path {
                if self.closed_stack.first().map(|f| f == p).unwrap_or(false) {
                    return;
                }
                self.closed_stack.insert(0, p.clone());
                self.closed_stack.truncate(10);
            }
        }
    }

    /// 关闭第 `idx` 个标签页；关到最后一个时重置为新的空标签页
    /// （新页分配下一个未命名序号）。返回是否真的移除了页面。
    fn close_tab_now(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() {
            return false;
        }
        // P32：移除前把该页光标/滚动回写最近文件记忆
        self.remember_tab_views(&[idx]);
        // 第 64 轮：命名页进「上次关闭」栈（恢复入口见 ReopenLastClosedFile）
        self.remember_closed_tab(idx);
        self.tabs.remove(idx);
        // P112：悬停的页被关掉 → 悬停态清空（下一个指针移动事件会
        // 按新下标重新置位；不清的话陈旧下标会悬停染色到错页）
        self.hovered_tab = None;
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
            let last = self.tabs.len() - 1;
            self.assign_untitled_num(last);
        }
        // P31：页集合结构已变——下一拍重写清单，防崩溃恢复复活已关的页
        self.session_manifest_stale = true;
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
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
            let last = self.tabs.len() - 1;
            self.assign_untitled_num(last);
        }
        self.session_manifest_stale = true;
        self.refresh_cur_handle();
        removed
    }

    /// 开启批量关闭流程（P28）：目标列表非空且任一置脏 → 弹一次聚合
    /// 确认条（确认后统一放弃，§3 P28 第 3 条）；全部干净 → 直接移除。
    /// 无可关目标时静默 no-op。同一时刻只保留一条确认条。
    fn begin_batch_close(&mut self, scope: BatchCloseScope) {
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
    fn target_tab_for_open(&self) -> usize {
        let t = self.tab();
        let fresh = t.path.is_none() && !t.dirty && t.editor.borrow().doc.is_empty();
        if fresh {
            self.active_tab
        } else {
            self.tabs.len()
        }
    }

    pub(crate) fn new(cli_files: Vec<PathBuf>) -> (Self, Task<Message>) {
        // P33：先把 Family::Monospace 的解析目标钉到 CJK 等宽候选（幂等、
        // 进程内一次）——必须发生在首帧排版之前，否则排版缓存里已固化的
        // 逐字回退不会重排
        apply_default_cjk_mono_pin();
        let mut settings = editpad_core::Settings::load();
        // P62：热键重映射表消毒——动作 id 不在注册表中的条目删除
        // （core 只做组合串格式归一，动作清单是 app 层知识）
        sanitize_hotkeys(&mut settings);
        // P34：枚举系统字体清单（P33 钉字之后，同一 fontdb 全局），并解析
        // 配置的字体——未安装时只回退本次渲染并提示一次，**不抹掉配置**
        // （重装字体后自动恢复用户意图）。
        let available_fonts = enumerate_available_families();
        let (active_font_family, configured_font_missing) =
            resolve_startup_font(settings.font_family.as_deref(), &available_fonts);
        // P29：快照总开关关闭时清空存量快照区——只关开关不清数据等于没关
        // （对齐 P20「记住最近文件」先例）
        if !settings.enable_snapshots {
            if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                editpad_core::snapshot::clear_session(&dir);
            }
        }
        let dark_mode = settings.is_dark();
        // 设置里的字号可能未归一（旧配置/手改），boot 时按同一规则 clamp
        let font_size = editor::normalize_font_size(settings.font_size);
        let mut state = Self {
            settings,
            dark_mode,
            available_fonts,
            active_font_family: active_font_family.map(leak_font_family),
            ..Self::default()
        };
        state.cur().borrow_mut().set_font_size(font_size);
        if configured_font_missing {
            if let Some(name) = state.settings.font_family.as_deref() {
                state.status = format!("配置的字体「{name}」未安装，本次启动回退默认等宽");
            }
        }
        // P18 打磨：启动光标闪烁心跳链（自我续期，占用一个睡眠节拍）
        let caret_chain = Task::perform(
            async {
                std::thread::sleep(std::time::Duration::from_millis(
                    editor::CARET_BLINK_MS,
                ));
            },
            |_| Message::CaretTick,
        );
        // P31 周期快照心跳链：同一自我续期模式——运行中每隔
        // snapshot_interval_secs 巡检置脏页增量写快照（崩溃至多丢一个
        // 间隔的输入）；随窗口关闭/进程退出自然销毁。
        let heartbeat_chain = Task::perform(
            async move {
                std::thread::sleep(std::time::Duration::from_secs(u64::from(
                    state.settings.snapshot_interval_secs,
                )));
            },
            |_| Message::SnapshotHeartbeatTick,
        );
        // P30：启动会话恢复——读清单重建标签；命名干净页经加载管线回填。
        // 开关判定在 boot_restore 内部（关闭 = 空白启动 + 存量清场）。
        // P103：命令行文件（资源管理器双击 / 「打开方式」/ 多选打开）——
        // 用户意图优先于会话恢复：跳过恢复（快照原封留存，下次无参数
        // 启动仍可恢复），文件经打开管线加载。首个文件在 boot 期同步
        // 登记（只改状态，加载流由 subscription 依据 active_load 重建
        // 接管——与拖拽同一条已验证通路）；其余文件留 pending_cli，
        // 每个 Loaded 结算后由 update 内的同步续排弹出下一个。全程不
        // 依赖 boot 任务的消息投递（iced 0.14 实测：boot 任务「立即
        // 输出」在窗口创建初期丢失，CaretTick 类延时输出正常；详见
        // 归档第 79 轮）。
        let (state, boot_task) = if cli_files.is_empty() {
            let task = state.boot_restore();
            (state, task)
        } else {
            state.boot_cli_kickoff(cli_files);
            (state, Task::none())
        };
        // 注：窗口标题栏图标（第 76 轮用户点单）在 main() 的
        // `.window(Settings { icon })` 声明期下发（见 window_title_icon）
        (
            state,
            Task::batch([caret_chain, heartbeat_chain, boot_task]),
        )
    }

    /// P103：boot 期把命令行文件清单转成打开链路的初始状态——
    /// 首个文件**同步登记**加载任务（active_load/busy 只是状态变更，
    /// 加载流由 boot 后的 subscription 依据 active_load 重建自动接管，
    /// 与拖拽同一通路）；其余文件进 pending_cli 队列，由每个 Loaded
    /// 结算后的同步续排（[`Message::OpenNextCliFile`]）串行弹出。
    /// 纯状态操作，可在事件循环启动前安全调用。
    pub(crate) fn boot_cli_kickoff(&mut self, cli_files: Vec<PathBuf>) {
        debug_assert!(!cli_files.is_empty(), "空清单不应走到本函数");
        let mut rest = cli_files;
        let first = rest.remove(0);
        let tab = self.target_tab_for_open();
        // start_loading 返回恒为 none（登记即完成；加载流归订阅接管）
        let _ = self.start_loading(first, tab);
        self.pending_cli = rest.into();
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match &message {
        // ---------- 编辑器/剪贴板/光标/预览/高亮铺路 ----------
        Message::Edit(..) | Message::EditorNavChanged | Message::CopyRequested | Message::CutRequested
        | Message::PasteRequested | Message::Pasted(..) | Message::KeyPressed(..) | Message::HighlightPaveNeeded
        | Message::FormatJson | Message::HlPaveProgress(..) | Message::HlPaved(..) | Message::CaretTick
        | Message::PreviewToggled | Message::CursorMoved(..) | Message::ViewportResized(..)
        => self.update_editor(message),
        // ---------- 标签页/右键菜单/批关 ----------
        Message::CopyFilePath(..) | Message::CopyFileName(..) | Message::ReopenLastClosedFile | Message::NewTab
        | Message::TabStripBlankPressed | Message::SwitchTabNext | Message::SwitchTabPrev | Message::SwitchTab(..) | Message::CloseTabRequest
        | Message::ConfirmCloseTabDiscard(..) | Message::CancelCloseTab | Message::CloseTabSave(..) | Message::TabContextMenu(..)
        | Message::TabContextMenuClosed | Message::TogglePinTab(..) | Message::SaveTabFromMenu(..) | Message::RenameOrSaveAsTab(..)
        | Message::TabRenameInputChanged(..) | Message::TabRenameCommitted | Message::TabRenameCancelled | Message::CloseTabAt(..)
        | Message::CloseOtherTabs(..) | Message::CloseTabsRight(..) | Message::ConfirmBatchCloseDiscard | Message::CancelBatchCloseTabs
        | Message::TabHovered(..)
        => self.update_tabs(message),
        // ---------- 设置/字体/热键 ----------
        Message::SettingsShowWhitespaceToggled(..) | Message::SettingsShowLineEndingsToggled(..) | Message::SettingsWordWrapToggled(..) | Message::HotkeyCaptureStarted(..)
        | Message::HotkeyCaptureKey(..) | Message::HotkeyCaptureCancel | Message::HotkeysResetAll | Message::ThemeToggled
        | Message::FontSizeDelta(..) | Message::SettingsToggled | Message::SettingsPageSelected(..) | Message::SettingsSearchChanged(..)
        | Message::SettingsAutosaveToggled(..) | Message::SettingsAutosaveDelayDelta(..) | Message::SettingsRememberRecentToggled(..) | Message::SettingsSnapshotsToggled(..)
        | Message::SettingsBackupModeToggled | Message::SettingsRememberSessionToggled(..) | Message::SettingsExitModeToggled | Message::SettingsIntervalDelta(..)
        | Message::SettingsFontSelected(..) | Message::SettingsFontReset | Message::FontFilterChanged(..)
        => self.update_settings(message),
        // ---------- 文件：打开/保存/编码/行尾/拖放/外部变更 ----------
        Message::OpenRequested | Message::FileChosen(..) | Message::FileDropped(..) | Message::OpenNextCliFile
        | Message::LoadProgress(..) | Message::Loaded(..) | Message::SaveRequested | Message::SaveAsRequested
        | Message::SaveTargetChosen(..) | Message::Saved(..) | Message::TabAutosaved(..) | Message::WindowFocused
        | Message::ConfirmExternalReload(..) | Message::IgnoreExternalChange(..) | Message::IgnoreAllExternalChanges | Message::ConfirmOpenDiscard
        | Message::ConfirmOpenCancel | Message::ToggleEncodingMenu | Message::ToggleEolMenu | Message::SaveWithEncoding(..)
        | Message::ConvertEol(..) | Message::TabSaved(..)
        => self.update_file(message),
        // ---------- 菜单栏/最近文件/面板 ----------
        Message::MenuToggled(..) | Message::MenubarHovered(..) | Message::MenubarPressed | Message::RecentsToggled
        | Message::RecentSelected(..) | Message::RecentsCleared | Message::BarsDismissed
        => self.update_menus(message),
        // ---------- 会话/快照/关窗/窗口 ----------
        Message::CloseRequested(..) | Message::ConfirmSaveAndClose | Message::DiscardAndClose | Message::CancelClose
        | Message::SessionRecoverAccepted | Message::SessionRecoverDiscarded | Message::SnapshotHeartbeatTick | Message::HeartbeatDone(..)
        | Message::WindowMoved(..)
        => self.update_session(message),
        // ---------- 查找/替换/跳转/查找全部 ----------
        Message::FindToggled | Message::FindQueryChanged(..) | Message::FindNext | Message::FindPrev
        | Message::FindAllToggled | Message::FindAllGoto(..) | Message::CaseToggled(..) | Message::RegexToggled(..)
        | Message::WholeWordToggled(..)
        | Message::ReplaceQueryChanged(..) | Message::ReplaceCurrent | Message::ReplaceCurrentRegex | Message::ReplaceAll
        | Message::FindScanDone(..) | Message::GotoToggled | Message::GotoInputChanged(..) | Message::GotoSubmit
        => self.update_find(message),
        }
    }

    // ---------- 域方法（第 81 轮 Phase 1：update() 拆分） ----------
    /// 域：编辑器/剪贴板/光标/预览/高亮铺路。臂体自原 update() 逐字搬移，零行为变更。
    fn update_editor(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 编辑器 ----------
            // 第 60 轮：复制标记行——只读操作前置拦截，取文本直接写剪贴板
            //（不置脏、不排自动保存；加载中与普通编辑同口径拒收）
            Message::Edit(EditOp::CopyBookmarkedLines) => {
                if self.active_load.is_some() {
                    return Task::none();
                }
                match self.cur_handle.borrow().copy_bookmarked_lines() {
                    Some(text) => iced::clipboard::write(text),
                    None => Task::none(),
                }
            }
            Message::Edit(op) => {
                let changed = self.apply_edit(op);
                let mut tasks: Vec<Task<Message>> = Vec::new();
                if changed {
                    // P10：编辑后不再同步重扫（每键全文扫描会卡 UI），排队后台防抖扫描
                    if self.find_visible {
                        tasks.push(self.schedule_find_scan());
                    }
                    // P18：编辑置脏后排队一次防抖自动保存（inflight 去重）
                    tasks.push(self.maybe_schedule_autosave());
                }
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
            }
            Message::EditorNavChanged => Task::none(),
            // 视图重建即可刷新状态栏

            // ---------- 剪贴板（P4） ----------
            Message::CopyRequested => {
                // 第 67 轮 ⑮：列块态优先复制块内容（各行 \n 连接）
                if let Some(text) = self.cur_handle.borrow().block_copy_text() {
                    return iced::clipboard::write(text);
                }
                let Some(text) = self.cur_handle.borrow().selected_text() else {
                    return Task::none();
                };
                iced::clipboard::write(text)
            }
            Message::CutRequested => {
                // 第 67 轮 ⑮：列块剪切 = 复制块内容 + 经编辑入口删块
                // （同步递归 update，置脏/自动保存调度全继承——Pasted 先例）
                let block_text = self.cur_handle.borrow().block_copy_text();
                if let Some(text) = block_text {
                    let write: Task<Message> = iced::clipboard::write(text);
                    let edit =
                        self.update(Message::Edit(EditOp::Delete));
                    return write.chain(edit);
                }
                let Some(text) = self.cur_handle.borrow().selected_text() else {
                    return Task::none();
                };
                // 先写剪贴板，再走统一编辑入口删除选区（Delete 在有选区时只删选区）。
                // clipboard::write 是泛型 Task<T>，直接以 Message 实例化后 chain。
                let write: Task<Message> = iced::clipboard::write(text);
                write.chain(Task::done(Message::Edit(EditOp::Delete)))
            }
            Message::PasteRequested => {
                // clipboard::read 返回 Task<Option<String>>
                iced::clipboard::read()
                    .map(|content| Message::Pasted(content.unwrap_or_default()))
            }
            Message::Pasted(text) => {
                if text.is_empty() {
                    Task::none()
                } else {
                    self.update(Message::Edit(EditOp::InsertText(text)))
                }
            }
            // ---------- 按键分发与热键捕获（P62） ----------
            Message::KeyPressed(key, modifiers) => {
                // 热键捕获态拦截：Esc 直接取消（简单状态清除，无需消息
                // 往返）；可作热键的按键经 HotkeyCaptureKey 走校验提交
                if self.hotkey_capture.is_some() {
                    if let keyboard::Key::Named(Named::Escape) = &key {
                        self.hotkey_capture = None;
                        self.status.clear();
                        return Task::none();
                    }
                    if let Some(combo) = combo_string(modifiers, &key) {
                        return Task::done(Message::HotkeyCaptureKey(combo));
                    }
                    return Task::none();
                }
                // 第 67 轮 ⑮：列块选区时 Esc 先清块并消费按键
                // （不与热键捕获/状态栏菜单的 Esc 语义叠加）。同步清除 +
                // CancelBlock 消息幂等兜底（架构惯例走编辑入口）
                if let keyboard::Key::Named(Named::Escape) = &key {
                    if self.cur_handle.borrow().has_block() {
                        self.cur_handle.borrow_mut().clear_block();
                        return Task::done(Message::Edit(EditOp::CancelBlock));
                    }
                }
                match handle_key(key, modifiers, &self.settings.hotkeys) {
                    Some(message) => Task::done(message),
                    None => Task::none(),
                }
            }
            // ---------- 高亮后台分批补建（P12） ----------
            Message::HighlightPaveNeeded => self.schedule_highlight_pave(),
            Message::FormatJson => {
                const FORMAT_JSON_MAX_CHARS: usize = 4_000_000;
                // 仅当前语法为 JSON 时生效（P22 第二批：按当前语法判断）
                if self.cur_handle.borrow().highlight_syntax_name().as_deref() != Some("JSON") {
                    self.set_status("格式化 JSON 仅对 JSON 文件可用（Ctrl+Shift+F）".to_owned());
                    return Task::none();
                }
                let (text, chars) = {
                    let ed = self.cur_handle.borrow();
                    (ed.doc.to_text(), ed.doc.text_len())
                };
                if chars > FORMAT_JSON_MAX_CHARS {
                    // 单遍重排是同步操作，超大文件会冻结 UI——先挡下并提示
                    self.status =
                        format!("文档过大（{chars} 字符），暂不支持格式化（上限 {FORMAT_JSON_MAX_CHARS}）");
                    return Task::none();
                }
                match editpad_core::format_json(&text) {
                    Ok(pretty) => {
                        // replace_whole_document 内部快照 → 可撤销；光标复位到文首
                        self.cur()
                            .borrow_mut()
                            .replace_whole_document(editpad_core::Document::from_str(&pretty));
                        self.tab_mut().dirty = true;
                        // P18：内容版本与防抖起点同步推进
                        self.tab_mut().note_mutation();
                        self.set_status("已格式化 JSON".to_owned());
                        if self.find_visible {
                            // 内容变了：命中表过期，走后台防抖重扫（P10 同款）
                            let find_task = self.schedule_find_scan();
                            return Task::batch([
                                find_task,
                                self.maybe_schedule_autosave(),
                            ]);
                        }
                        self.maybe_schedule_autosave()
                    }
                    Err(error) => {
                        self.set_status_error(format!("JSON 格式化失败：{error}"));
                        Task::none()
                    }
                }
            }
            Message::HlPaveProgress(gen, strides_done) => {
                // 双重代次检查：任务登记一致且高亮器未换代（换文件后
                // 旧任务的迟到进度不得污染新会话的状态栏）
                if self.hl_paving == Some(gen)
                    && self.cur_handle.borrow().highlight_generation() == Some(gen)
                {
                    let total_strides = (self.cur_handle.borrow().doc.line_count()
                        / editpad_core::highlight::STRIDE)
                        .max(1);
                    let pct = (strides_done as usize).min(total_strides) * 100 / total_strides;
                    self.set_status(format!("语法分析中…{pct}%（后台）"));
                }
                Task::none()
            }
            Message::HlPaved(gen, paved) => {
                if self.hl_paving == Some(gen) {
                    self.hl_paving = None;
                    // 代次一致才安装；期间编辑过则整体丢弃——缺口由下一帧
                    // needs_paving 重新评估并续排（从存活检查点出发，代价小）
                    let installed = self
                        .cur_handle
                        .borrow_mut()
                        .install_highlighter_if_current(gen, paved);
                    let _ = installed;
                    if self.status.starts_with("语法分析") {
                        self.status.clear();
                    }
                }
                Task::none()
            }
            Message::CaretTick => {
                // 打磨项：翻转闪烁相位（update 本身会触发重绘）。
                // 心跳链在 new() 启动后自我续期，占用一个常驻睡眠节拍。
                // P53：一条链两用——竖直滚动条淡出动画期间切换 33ms 快拍
                // 驱动渐变（相位翻转由 tick_blink 按真实间隔门控，不受影响），
                // 其余时间维持 ~530ms 常规节拍。
                self.cur_handle.borrow_mut().tick_blink();
                let next_ms = if self.cur_handle.borrow().scrollbar_fading() {
                    editor::SCROLLBAR_FADE_TICK_MS
                } else {
                    editor::CARET_BLINK_MS
                };
                Task::perform(
                    async move {
                        std::thread::sleep(std::time::Duration::from_millis(next_ms));
                    },
                    |_| Message::CaretTick,
                )
            }
            Message::PreviewToggled => {
                // 仅 Markdown 语法页可开预览（按钮本身已禁用，此处双保险）
                if self.cur_handle.borrow().highlight_syntax_name().as_deref()
                    == Some("Markdown")
                {
                    self.preview_visible = !self.preview_visible;
                } else {
                    self.set_status("预览仅支持 Markdown 文件".to_owned());
                }
                Task::none()
            }
            // ---------- 浮层弹窗定位（P39/P40） ----------
            Message::CursorMoved(p) => {
                self.cursor_pos = (p.x, p.y);
                Task::none()
            }
            Message::ViewportResized(w, h) => {
                self.viewport_size = (w, h);
                // P102：窗口尺寸记忆（0,0 = 未知窗口，跳过；实测正常值
                // 才记录）。节流：拖动/拉伸每帧事件，2s 才落盘一次，
                // 关闭路径 handle_close_request 兜底落盘最后状态。
                if w > 0.0 && h > 0.0 {
                    self.settings.window_width = Some(w);
                    self.settings.window_height = Some(h);
                    self.persist_geometry_if_due();
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：标签页/右键菜单/批关。臂体自原 update() 逐字搬移，零行为变更。
    fn update_tabs(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 复制完整路径 / 文件名（第 63 轮） ----------
            // None = 活动页（热键），Some(i) = 指定页（标签右键菜单）；
            // 未命名页无路径可写，给状态栏提示
            Message::CopyFilePath(target) => {
                self.copy_tab_ident(target, true)
            }
            Message::CopyFileName(target) => {
                self.copy_tab_ident(target, false)
            }
            // ---------- 恢复上次关闭 / 显示标记（第 64 轮） ----------
            Message::ReopenLastClosedFile => {
                // busy 与打开确认流共用守卫语义；栈空静默
                if self.busy {
                    return Task::none();
                }
                let Some(path) = self.closed_stack.pop() else {
                    self.set_status("没有可恢复的已关闭文件".to_owned());
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
                    self.set_status("固定标签页需先取消固定再关闭".to_owned());
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
                    self.set_status("未命名标签页请先另存为再关闭".to_owned());
                    return Task::none();
                }
                self.enter_busy();
                let path = self.tabs[idx].path.clone().expect("上方已确认非空");
                let doc = self.tabs[idx].editor.borrow().doc.clone();
                let version = self.tabs[idx].version;
                self.pending_close_tab = Some(idx);
                Task::perform(
                    async move {
                        let saved = editpad_core::save_document_atomic(&path, &doc)
                            .map_err(|e| e.to_string());
                        (version, saved)
                    },
                    move |(version, result)| Message::TabSaved(idx, version, result),
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
                        // 被直接覆盖；操作在下一帧视图含该输入框后生效
                        return iced::widget::operation::focus(rename_input_id())
                            .chain(iced::widget::operation::select_all(
                                rename_input_id(),
                            ));
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
                Task::none()
            }
            Message::CloseTabAt(idx) => {
                self.tab_context_menu = None;
                if !self.busy && idx < self.tabs.len() {
                    if self.tabs[idx].pinned {
                        self.set_status("固定标签页需先取消固定再关闭".to_owned());
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

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：设置/字体/热键。臂体自原 update() 逐字搬移，零行为变更。
    fn update_settings(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SettingsShowWhitespaceToggled(value) => {
                self.settings.show_whitespace = value;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().set_invisibles(
                        value,
                        self.settings.show_line_endings,
                    );
                }
                self.persist_settings();
                Task::none()
            }
            Message::SettingsShowLineEndingsToggled(value) => {
                self.settings.show_line_endings = value;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().set_invisibles(
                        self.settings.show_whitespace,
                        value,
                    );
                }
                self.persist_settings();
                Task::none()
            }
            // 第 73 轮 ⑯：自动换行开关——全标签页即时生效（列块清、水平
            // 滚动锁 0、视觉行映射接管，关闭即恒等退化回现状路径）
            Message::SettingsWordWrapToggled(value) => {
                self.settings.word_wrap = value;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().set_word_wrap(value);
                }
                self.persist_settings();
                Task::none()
            }
            Message::HotkeyCaptureStarted(id) => {
                self.hotkey_capture = Some(id);
                self.set_status(format!(
                    "为「{}」按下新组合键（Esc 取消）",
                    HOTKEY_ACTIONS
                        .iter()
                        .find(|a| a.id == id)
                        .map(|a| a.desc)
                        .unwrap_or("")
                ));
                Task::none()
            }
            Message::HotkeyCaptureKey(combo) => self.commit_hotkey_capture(combo),
            Message::HotkeyCaptureCancel => {
                self.hotkey_capture = None;
                self.status.clear();
                Task::none()
            }
            Message::HotkeysResetAll => {
                self.settings.hotkeys.clear();
                self.persist_settings();
                self.hotkey_capture = None;
                self.set_status("已恢复默认热键".to_owned());
                Task::none()
            }
            // ---------- 外观 ----------
            Message::ThemeToggled => {
                self.dark_mode = !self.dark_mode;
                self.settings.set_theme(self.dark_mode);
                self.persist_settings();
                // 全部标签页的语法高亮随主题换档（新代次作废在途补建，
                // 下一帧按新主题重铺/重算可视区配色）
                let dark = self.dark_mode;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().apply_highlight_theme(dark);
                }
                Task::none()
            }
            Message::FontSizeDelta(delta) => {
                let next = editor::normalize_font_size(self.display_font_size() + delta);
                self.settings.font_size = next;
                self.persist_settings();
                self.cur_handle.borrow_mut().set_font_size(next);
                Task::none()
            }
            // ---------- 设置弹窗（P27） ----------
            Message::SettingsToggled => {
                // busy（加载/保存中）禁开，与工具栏其余按钮同一守卫语义
                if !self.busy {
                    self.settings_visible = !self.settings_visible;
                    if self.settings_visible {
                        // 第 64 轮用户点单：每次进入设置默认落在第一分类，
                        // 不记忆上次浏览位置（P51 撤销）
                        self.settings_page = SettingsPage::default();
                    } else {
                        // P47：关弹窗顺带清搜索词，下次打开回到分类浏览
                        self.settings_search.clear();
                    }
                }
                Task::none()
            }
            // P47：侧栏分类导航——点分类即离开搜索态（同款语义）。
            // 第 64 轮用户点单：分类位置不再持久化（P51 撤销），每次打开
            // 设置都回到第一分类；弹窗打开期间导航照常。
            Message::SettingsPageSelected(page) => {
                self.settings_page = page;
                self.settings_search.clear();
                Task::none()
            }
            Message::SettingsSearchChanged(query) => {
                self.settings_search = query;
                Task::none()
            }
            Message::SettingsAutosaveToggled(value) => {
                self.settings.autosave_enabled = value;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsAutosaveDelayDelta(delta) => {
                let next = (self.settings.autosave_delay_secs as i64 + delta as i64)
                    .clamp(
                        editpad_core::settings::MIN_AUTOSAVE_DELAY_SECS as i64,
                        editpad_core::settings::MAX_AUTOSAVE_DELAY_SECS as i64,
                    ) as u32;
                self.settings.autosave_delay_secs = next;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsRememberRecentToggled(value) => {
                self.settings.remember_recent_files = value;
                // P20：关闭开关即清空存量列表（只关开关不清数据等于没关）
                if !value {
                    self.settings.clear_recent_files();
                }
                self.persist_settings();
                Task::none()
            }
            Message::SettingsSnapshotsToggled(value) => {
                self.settings.enable_snapshots = value;
                self.persist_settings();
                Task::none()
            }
            // 第 64 轮 ⑭：备份模式三态循环（none→simple→timestamped）
            Message::SettingsBackupModeToggled => {
                use editpad_core::settings::{
                    BACKUP_MODE_NONE, BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED,
                };
                self.settings.backup_mode = if self.settings.backup_mode == BACKUP_MODE_SIMPLE {
                    BACKUP_MODE_TIMESTAMPED
                } else if self.settings.backup_mode == BACKUP_MODE_TIMESTAMPED {
                    BACKUP_MODE_NONE
                } else {
                    BACKUP_MODE_SIMPLE
                }
                .to_owned();
                self.persist_settings();
                self.set_status(match self.settings.backup_mode.as_str() {
                    BACKUP_MODE_SIMPLE => "保存时备份：同目录 name.bak 覆盖式".to_owned(),
                    BACKUP_MODE_TIMESTAMPED => {
                        "保存时备份：name.bak/ 目录按时间戳留存".to_owned()
                    }
                    _ => "保存时备份：已关闭".to_owned(),
                });
                Task::none()
            }
            Message::SettingsRememberSessionToggled(value) => {
                self.settings.remember_session = value;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsExitModeToggled => {
                use editpad_core::settings::{EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT};
                self.settings.exit_mode = if self.settings.exit_mode == EXIT_MODE_SNAPSHOT {
                    EXIT_MODE_ASK
                } else {
                    EXIT_MODE_SNAPSHOT
                }
                .to_string();
                self.persist_settings();
                Task::none()
            }
            Message::SettingsIntervalDelta(delta) => {
                let next = (self.settings.snapshot_interval_secs as i64 + delta as i64)
                    .clamp(
                        editpad_core::settings::MIN_SNAPSHOT_INTERVAL_SECS as i64,
                        editpad_core::settings::MAX_SNAPSHOT_INTERVAL_SECS as i64,
                    ) as u32;
                self.settings.snapshot_interval_secs = next;
                self.persist_settings();
                Task::none()
            }
            // ---------- 字体选择（P34） ----------
            Message::SettingsFontSelected(name) => {
                // 名字来自启动期枚举清单，必然可解析；仍走统一解析保持
                // 「设置值 ↔ 生效值」同源（清单为空的异常环境会回退默认）
                self.settings.set_font_family(Some(name));
                self.active_font_family = effective_font_family(
                    self.settings.font_family.as_deref(),
                    &self.available_fonts,
                )
                .map(leak_font_family);
                self.persist_settings();
                Task::none()
            }
            Message::SettingsFontReset => {
                self.settings.set_font_family(None);
                self.active_font_family = None;
                self.persist_settings();
                Task::none()
            }
            Message::FontFilterChanged(filter) => {
                self.font_filter = filter;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：文件（打开/保存/编码/行尾/拖放/外部变更）。臂体自原 update() 逐字搬移，零行为变更。
    fn update_file(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 打开 ----------
            Message::OpenRequested => {
                if self.busy {
                    return Task::none();
                }
                self.enter_busy();
                self.status.clear();
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("所有文件", &["*"])
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
                // P21：结果路由回发起加载的标签页——期间切走也不串页
                let target = job.tab;
                // P30：恢复任务的待还原视图随任务号取出；None = 普通打开
                let pending_view = self.restore_views.remove(&job_id);
                let is_restore = pending_view.is_some();
                let mut tasks: Vec<Task<Message>> = Vec::new();
                match result {
                    // P30 防串写护栏（第二半在 Ok(_) 分支）：普通打开照旧；
                    // 恢复任务要求目标仍是空净无名占位页，否则走丢弃分支
                    Ok((doc, sample, encoding))
                        if !is_restore || self.restore_placeholder_ready(target) =>
                    {
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
                            }
                        }
                        tab.path = Some(job.path.clone());
                        tab.encoding_label = encoding;
                        // P67：新载入的文件回到默认 UTF-8 偏好（旧偏好属于
                        // 上一次打开的会话上下文）
                        tab.save_encoding = None;
                        tab.dirty = false;
                        // P50：载入成功即记外部修改比对戳
                        tab.file_stamp = file_stamp(&job.path);
                        // P25：真实文件已就位，未命名序号使命完成
                        tab.untitled_num = None;
                        // P31：页内容整体换血（打开/恢复回填）——已提交清单
                        // 对本页的描述过期，下一拍重写
                        self.session_manifest_stale = true;
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
                    Ok(_) => {
                        // P30：占位页已被用户动过（关页/新页导致下标漂移）——
                        // 宁可丢弃结果也不能覆盖用户内容；计入失败汇总
                        self.busy = false;
                        self.restore_failed += 1;
                    }
                    Err(error) => {
                        if is_restore {
                            // P30：恢复页加载失败（文件被删等）——移除占位页
                            // 继续恢复其余页，不阻断（§3 P30 第 6 条）
                            self.drop_restore_placeholder(target);
                            self.restore_failed += 1;
                            self.busy = false;
                        } else {
                            self.busy = false;
                            self.set_status_error(format!("打开失败:{error}"));
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
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
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
                let tab = self.tab_mut();
                tab.path = Some(path.clone());
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
            Message::Saved(version, Ok(notice)) => {
                // P18 版本守卫：保存期间又有编辑则保持置脏，防止丢改动标记
                self.tab_mut().dirty = self.tab().version != version;
                self.busy = false;
                if !self.tab().dirty {
                    // P38：落盘成功且期间无新编辑——当前内容即磁盘内容，
                    // 刷新撤销回基线的判定基准（版本不符时不得动基线：
                    // 那时磁盘上是旧快照）
                    self.cur_handle.borrow_mut().mark_saved();
                    // P31：内存态比已提交清单「更干净」——下一拍重写清单，
                    // 防崩溃恢复把已落盘内容按旧快照复活成置脏页
                    self.session_manifest_stale = true;
                }
                if let Some(path) = self.tab().path.clone() {
                    self.record_recent(&path);
                }
                // P67：状态栏标签反映实际落盘编码（用户选择的偏好或默认
                // UTF-8），转码提示按「原标签 vs 实际目标」判定
                let target_label = self
                    .tab()
                    .save_encoding
                    .unwrap_or(editpad_core::SaveEncoding::Utf8)
                    .label();
                let prev_label = self.tab().encoding_label.clone();
                self.tab_mut().encoding_label = target_label.to_owned();
                // P6 编码知情权：发生转码/BOM 丢失/不可映射字符时明确告知；
                // 无转码时补显暂存的备份提示（备份消息写在异步落盘完成
                // 之前，直接进状态栏会被本分支立即覆盖/抹掉）
                let backup_note = self.pending_backup_notice.take();
                if let Some(text) =
                    transcode_notice(&prev_label, target_label, notice.unmappable)
                {
                    self.set_status(text);
                } else if let Some(text) = backup_note {
                    self.set_status(text);
                } else {
                    self.status.clear();
                }
                // P50：落盘成功即刷新外部修改比对戳（磁盘内容 = 刚写的内容）
                if let Some(path) = self.tab().path.clone() {
                    self.tab_mut().file_stamp = file_stamp(&path);
                }
                if self.pending_close {
                    // 落盘确认后才真正关窗。P29：保存的是活动页，
                    // 其余置脏页走快照直退（不再二次弹窗），快照失败才降级
                    self.pending_close = false;
                    if session_restore_allowed(
                        self.settings.enable_snapshots,
                        self.settings.remember_session,
                    ) && self.settings.exit_mode == editpad_core::EXIT_MODE_SNAPSHOT
                    {
                        if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                            return self.exit_via_snapshot(&dir);
                        }
                    }
                    return self.close_window();
                }
                Task::none()
            }
            Message::Saved(_, Err(error)) => {
                self.busy = false;
                // 保存失败不关窗：留在应用里让用户处理
                self.pending_close = false;
                self.set_status_error(format!("保存失败:{error}"));
                // 保存失败时备份已发生（写前备份），但「已备份」提示对
                // 失败的保存没有意义，弃置防陈旧
                self.pending_backup_notice = None;
                Task::none()
            }
            // ---------- 即时保存（P18，按页路由；P63 结局三分+路径守卫） ----------
            Message::TabAutosaved(idx, version, path, outcome) => {
                // P63 路由守卫：回报按「下标 + 路径」双重核对。防抖睡眠
                // 期间关掉前面的页会让下标漂移——路径不符说明这批账目属于
                // 已不存在的旧页（或页已换血），整条丢弃。磁盘写入本身用
                // 的是调度时刻克隆的路径，无损害；只是不能让错误的页记账。
                if self.tabs.get(idx).and_then(|t| t.path.as_deref())
                    != Some(path.as_path())
                {
                    return Task::none();
                }
                let tab = &mut self.tabs[idx];
                tab.autosave_inflight = false;
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
                            self.session_manifest_stale = true;
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
                            Some(p) => {
                                !file_changed_externally(tab.file_stamp, file_stamp(p))
                            }
                            None => false,
                        };
                        if !own_save_superseded {
                            let queue =
                                self.external_change.get_or_insert_with(Vec::new);
                            if !queue.contains(&idx) {
                                queue.push(idx);
                            }
                            self.status =
                                "文件已被外部修改，已跳过自动写盘".to_owned();
                        }
                    }
                    AutosaveOutcome::Failed(error) => {
                        // 失败必须留痕（不能无声吞掉），但不打断编辑；
                        // 清掉 inflight 后，下一次编辑会重新排队
                        self.set_status_error(format!("自动保存失败:{error}"));
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
                    self.set_status("未命名页请先「另存为」取得路径，再选择保存编码".to_owned());
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
                        "文档过大（{chars} 字符），暂不支持行尾转换（上限 {EOL_CONVERT_MAX_CHARS}）"
                    ));
                    return Task::none();
                }
                if current == target {
                    self.set_status(format!("行尾已是 {}", eol_label(target)));
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
                self.set_status(format!("已转换为 {}", eol_label(target)));
                // 内容变了：命中表过期重扫（查找栏开着才扫）+ 排队自动保存
                if self.find_visible {
                    let find_task = self.schedule_find_scan();
                    return Task::batch([
                        find_task,
                        self.maybe_schedule_autosave(),
                    ]);
                }
                self.maybe_schedule_autosave()
            }
            Message::TabSaved(idx, version, result) => {
                self.busy = false;
                if idx >= self.tabs.len() {
                    return Task::none();
                }
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
                            if self.pending_close_tab == Some(idx)
                                && self.close_tab_now(idx)
                            {
                                self.cancel_find_scan();
                            }
                            self.pending_close_tab = None;
                        } else {
                            self.set_status("保存后又有新改动，已取消自动关闭".to_owned());
                        }
                    }
                    Err(error) => {
                        self.set_status_error(format!("保存失败:{error}"));
                        self.pending_close_tab = None;
                    }
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：菜单栏/最近文件/面板。臂体自原 update() 逐字搬移，零行为变更。
    fn update_menus(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 顶部菜单栏（第 69 轮） ----------
            Message::MenuToggled(idx) => {
                // 同项再点关闭，异项切换（互斥展开）。
                // 第 70 轮：打开瞬间**冻结锚点**——悬停位置此后继续变化
                // （含背板 on_move）不再影响已展开浮层的位置（修「弹窗
                // 会移动」：旧实现锚点实时读悬停点，展开期间漂移）
                self.menubar_anchor = self.menubar_pos;
                self.menu_bar_open =
                    if self.menu_bar_open == Some(idx) { None } else { Some(idx) };
                Task::none()
            }
            Message::MenubarHovered(pos) => {
                self.menubar_pos = (pos.x, pos.y);
                Task::none()
            }
            // 背板点击：菜单栏条带内 = 横移切换到目标菜单；条带外 = 收起
            Message::MenubarPressed => {
                if self.menubar_pos.1 < MENU_BAR_H {
                    let idx = menubar_slot_idx(self.menubar_pos.0);
                    self.menubar_anchor = self.menubar_pos;
                    self.menu_bar_open =
                        if self.menu_bar_open == Some(idx) { None } else { Some(idx) };
                } else {
                    self.menu_bar_open = None;
                }
                Task::none()
            }
            // ---------- 最近文件 ----------
            Message::RecentsToggled => {
                self.recents_visible = !self.recents_visible;
                Task::none()
            }
            Message::RecentSelected(entry) => {
                // 存在性预检：文件已删除/移动时点开必然走完整加载失败
                // 管线，直接明示；条目保留（可能只是暂时移动/离线盘符）
                let path = PathBuf::from(&entry);
                if path.exists() {
                    self.request_open(path)
                } else {
                    self.set_status_error(format!("文件不存在或已被移动：{entry}"));
                    Task::none()
                }
            }
            Message::RecentsCleared => {
                // P20 隐私：立即写回空列表，config.toml 不再含历史路径
                self.settings.clear_recent_files();
                self.persist_settings();
                self.set_status("已清空最近文件记录".to_owned());
                Task::none()
            }
            Message::BarsDismissed => {
                self.find_visible = false;
                self.goto_visible = false;
                self.recents_visible = false;
                // P27：Esc 一并关闭设置弹窗
                self.settings_visible = false;
                // Esc 同时视作放弃关闭/打开确认
                self.confirm_visible = false;
                self.pending_close = false;
                self.open_confirm = None;
                // P21：Esc 也取消标签页关闭确认
                self.close_tab_confirm = None;
                // P28：Esc 同时收起右键菜单与批量关闭确认
                self.tab_context_menu = None;
                self.batch_close_confirm = None;
                // 第 69 轮：Esc 同时收起顶部菜单栏浮层
                self.menu_bar_open = None;
                // P50：Esc 一并收起外部修改提示条
                self.external_change = None;
                // P55：Esc 一并取消就地重命名（一切保持原状）
                self.renaming_tab = None;
                self.rename_input.clear();
                // P67：Esc 一并收起状态栏编码/行尾菜单
                self.encoding_menu = false;
                self.eol_menu = false;
                // P10：取消在途扫描 + 清结果（含序号失效）
                self.cancel_find_scan();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：会话/快照/关窗/窗口。臂体自原 update() 逐字搬移，零行为变更。
    fn update_session(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 关闭确认 ----------
            Message::CloseRequested(id) => {
                self.handle_close_request(id, editpad_core::snapshot::snapshot_dir())
            }
            Message::ConfirmSaveAndClose => {
                self.confirm_visible = false;
                self.pending_close = true;
                match self.tab().path.clone() {
                    Some(_) => self.save(),
                    // 未命名文档：先走另存为，落盘成功后自动关窗
                    None => self.save_as_dialog(),
                }
            }
            Message::DiscardAndClose => {
                self.discard_all_and_close(editpad_core::snapshot::snapshot_dir())
            }
            Message::CancelClose => {
                self.confirm_visible = false;
                self.pending_close = false;
                Task::none()
            }
            // ---------- 启动会话恢复（P30） ----------
            Message::SessionRecoverAccepted => {
                self.accept_session_recover(editpad_core::snapshot::snapshot_dir())
            }
            Message::SessionRecoverDiscarded => {
                self.discard_session_recover(editpad_core::snapshot::snapshot_dir())
            }
            Message::SnapshotHeartbeatTick => {
                // P31 自我续期：无论本轮是否干活，下一拍恒排队（与光标
                // 闪烁同一模式；进程退出即销毁，无残留计时器）。间隔取
                // 加载时已归一的设置值，运行期视为不变。
                let interval = std::time::Duration::from_secs(u64::from(
                    self.settings.snapshot_interval_secs,
                ));
                let rearm = Task::perform(
                    async move { std::thread::sleep(interval) },
                    |_| Message::SnapshotHeartbeatTick,
                );
                // 快照底座任一开关关闭 / ask 模式 = 心跳整体停摆：
                // 清单不写，退出流与启动恢复同样不依赖它（P29/P30 语义）
                if !session_restore_allowed(
                    self.settings.enable_snapshots,
                    self.settings.remember_session,
                ) || self.settings.exit_mode != editpad_core::EXIT_MODE_SNAPSHOT
                {
                    return rearm;
                }
                // 至多一个提交在途：本轮巡检跳过（页级账目不受影响）
                if self.heartbeat_inflight {
                    return rearm;
                }
                // 目录解析走注入点（生产为 None → 系统配置目录）
                let dir = self
                    .snapshot_dir_override
                    .clone()
                    .or_else(editpad_core::snapshot::snapshot_dir);
                let Some(dir) = dir else {
                    return rearm;
                };
                match self.prepare_heartbeat_commit(&dir) {
                    // 无变化且清单不过期：什么都不写（防无谓 IO）
                    None => rearm,
                    Some(payload) => {
                        self.heartbeat_inflight = true;
                        Task::batch([
                            rearm,
                            Task::perform(
                                async move { drive_heartbeat(payload).await },
                                |message| message,
                            ),
                        ])
                    }
                }
            }
            Message::HeartbeatDone(outcome) => {
                self.heartbeat_apply(outcome);
                Task::none()
            }
            Message::WindowMoved(p) => {
                // P102：窗口位置记忆（逻辑坐标，iced 与建窗 Specific 同空间）
                self.settings.window_x = Some(p.x as i32);
                self.settings.window_y = Some(p.y as i32);
                self.persist_geometry_if_due();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：查找/替换/跳转/查找全部。臂体自原 update() 逐字搬移，零行为变更。
    fn update_find(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 查找 / 替换 ----------
            Message::FindToggled => {
                self.find_visible = !self.find_visible;
                if self.find_visible {
                    self.goto_visible = false;
                    return self.schedule_find_scan();
                } else {
                    // 关栏即取消在途扫描并清结果（旧实现只清结果）
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::FindQueryChanged(query) => {
                self.find_query = query;
                // P10：查询变化只排队后台扫描（防抖），UI 线程零全文拷贝；
                // 查询为空时内部转为取消 + 清结果
                self.schedule_find_scan()
            }
            Message::FindNext => self.step_match(true),
            Message::FindPrev => self.step_match(false),
            // 第 62 轮：查找全部结果面板——纯 UI 开关，不动命中表；
            // 查找栏关闭时面板随栏隐藏（停靠在查找区内，无独立生命周期）
            Message::FindAllToggled => {
                if self.find_visible {
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
                self.schedule_find_scan()
            }
            Message::RegexToggled(value) => {
                // P70：查询语义切换（字面转义 ↔ 正则语法），必须重扫
                self.regex_enabled = value;
                if value {
                    self.set_status("正则模式：替换支持 $1/${1} 组引用，^$ 逐行锚定用 (?m)".to_owned());
                }
                self.schedule_find_scan()
            }
            Message::WholeWordToggled(value) => {
                self.whole_word = value;
                self.schedule_find_scan()
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
                            self.set_status_error(format!("正则无效：{e}"));
                            Task::none()
                        }
                    };
                };
                match editpad_core::compile_regex(&self.find_query, self.case_sensitive) {
                    Ok(re) => {
                        let expanded = re.replace(&matched, self.replace_query.as_str()).into_owned();
                        self.cur_handle.borrow_mut().replace_selection(&expanded);
                        self.tab_mut().dirty = true;
                        self.tab_mut().note_mutation();
                        // 命中表已过期：排队重扫（「下一个」等重扫完成）
                        self.schedule_find_scan()
                    }
                    Err(e) => {
                        self.set_status_error(format!("正则无效：{e}"));
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
                if self.regex_enabled {
                    const REGEX_REPLACE_MAX_CHARS: usize = 4_000_000;
                    let (text, chars) = {
                        let ed = self.cur_handle.borrow();
                        (ed.doc.to_text(), ed.doc.text_len())
                    };
                    if chars > REGEX_REPLACE_MAX_CHARS {
                        self.set_status_error(format!(
                            "文档过大（{chars} 字符），正则替换暂不支持（上限 {REGEX_REPLACE_MAX_CHARS}）；可改用字面模式"
                        ));
                        return Task::none();
                    }
                    let mut tasks: Vec<Task<Message>> = Vec::new();
                    match editpad_core::replace_all_regex(
                        &text,
                        &self.find_query,
                        &self.replace_query,
                        self.case_sensitive,
                    ) {
                        Ok((new_contents, count)) => {
                            if count > 0 {
                                // 替换结果按主导行尾归一后整体入主：正则替换
                                // 文本里的裸换行不得在 CRLF 文档里制造混合
                                // 行尾（字面路径的归一已在 core 内完成）
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
                            self.set_status(format!("已替换 {count} 处"));
                        }
                        Err(e) => {
                            self.set_status_error(format!("正则无效：{e}"));
                        }
                    }
                    if tasks.is_empty() {
                        return Task::none();
                    }
                    return Task::batch(tasks);
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
                            "文档过大（{chars} 字符），整词替换暂不支持（上限 {WHOLE_WORD_MAX_CHARS}）；可关闭整词后重试"
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
                    self.set_status(format!("已替换 {count} 处"));
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
                self.set_status(format!("已替换 {count} 处"));
                Task::batch(tasks)
            }
            Message::FindScanDone(seq, found) => {
                // 过期结果丢弃：只认当前排队中的那次扫描（P10 的 job 序号过滤，
                // 与 Loaded 按 job_id 过滤同构）
                if self.find_scan == Some(seq) {
                    self.find_scan = None;
                    self.matches = found;
                    self.match_idx = None;
                }
                Task::none()
            }
            // ---------- 跳转 ----------
            Message::GotoToggled => {
                self.goto_visible = !self.goto_visible;
                if self.goto_visible {
                    self.find_visible = false;
                }
                Task::none()
            }
            Message::GotoInputChanged(value) => {
                self.goto_input = value;
                Task::none()
            }
            Message::GotoSubmit => match self.goto_input.trim().parse::<usize>() {
                Ok(n) if n >= 1 => {
                    self.cur_handle.borrow_mut().jump_to_line(n);
                    self.goto_visible = false;
                    self.status.clear();
                    Task::none()
                }
                _ => {
                    self.set_status_error("请输入有效行号（从 1 开始）".to_owned());
                    Task::none()
                }
            },
            _ => Task::none(),
        }
    }


    // ---------- 编辑分发 ----------

    /// 执行一次按键编辑；返回是否真的改动了文本。
    fn apply_edit(&mut self, op: EditOp) -> bool {
        // 加载进行中不接收编辑，避免打到即将被替换的旧文档上
        if self.active_load.is_some() {
            return false;
        }

        use EditOp as E;
        // 第 63 轮起 hint 升级为 String：插入日期时间等动态反馈不再
        // 需要 'static（曾用 leak() 属内存泄漏，已纠正）
        let mut hint: Option<String> = None;
        // 第 67 轮 ⑮：列块态只在白名单三臂内存活（输入/退格/删除走块
        // 分支），其余任何编辑动作先清块——单点收口防漏清
        if !matches!(
            op,
            E::InsertText(_) | E::Backspace | E::Delete | E::CancelBlock
        ) {
            self.cur_handle.borrow_mut().clear_block();
        }
        // P38：撤销/重做后内容是否恰好回到落盘基线（打字/删除路径不查询，
        // 维持保守置脏，避免大文档每键全量比对）
        let mut back_to_saved = false;

        let mut editor = self.cur_handle.borrow_mut();
        let changed = match op {
            E::Motion(motion, extend) => {
                editor.apply_motion(motion, extend);
                false
            }
            E::SelectAll => {
                editor.select_all();
                false
            }
            E::Undo => {
                let changed = editor.undo();
                if !changed {
                    hint = Some("没有更多撤销历史".to_owned());
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::Redo => {
                let changed = editor.redo();
                if !changed {
                    hint = Some("已在最新状态".to_owned());
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::InsertText(text) => {
                // 第 67 轮 ⑮：列块态下输入 = 逐行替换块内容（v1 单行文本）
                if editor.has_block() {
                    editor.insert_into_block(&text)
                } else {
                    editor.insert_str(&text);
                    true
                }
            }
            E::Backspace => {
                // 列块态下退格 = 删块内容；返回值判定是否真删了内容
                //（文档原点是静默 no-op，不得触发置脏/自动保存）
                if editor.has_block() {
                    editor.delete_block_content()
                } else {
                    editor.backspace()
                }
            }
            E::Delete => {
                // 文档末尾的 Delete 同样可能是空操作
                if editor.has_block() {
                    editor.delete_block_content()
                } else {
                    editor.delete_forward()
                }
            }
            E::CancelBlock => {
                // 第 67 轮 ⑮：Esc 取消列块（固定语义不入注册表）
                editor.clear_block();
                false
            }
            // ---------- 行操作套件（第 57 轮） ----------
            E::DeleteLines => editor.delete_current_lines(),
            E::DuplicateLines => editor.duplicate_current_lines(),
            E::MoveLinesUp => editor.move_current_lines(true),
            E::MoveLinesDown => editor.move_current_lines(false),
            // ---------- 大小写转换与行首尾清理（第 58 轮） ----------
            E::ConvertCase(kind) => editor.convert_case(kind),
            E::TrimLines(mode) => editor.trim_touched_lines(mode),
            // ---------- 行排序与去重（第 59 轮） ----------
            E::SortLines(order) => editor.sort_lines(order),
            E::RemoveDuplicateLines => editor.remove_duplicate_lines(),
            // ---------- 书签套件（第 60 轮） ----------
            // 开关/跳转/清除不改动文本：恒返回 false（不置脏、不排自动
            // 保存）；书签状态本身随快照回滚，无需应用层善后
            E::ToggleBookmark => {
                editor.toggle_bookmark();
                false
            }
            E::BookmarkNext => {
                editor.next_bookmark(true);
                false
            }
            E::BookmarkPrev => {
                editor.next_bookmark(false);
                false
            }
            E::BookmarksClearAll => {
                editor.clear_bookmarks();
                false
            }
            // 删除标记行是真编辑：照常置脏 + 排自动保存 + 触发查找重扫
            E::RemoveBookmarkedLines => editor.remove_bookmarked_lines(),
            // 复制标记行在消息层前置拦截（apply_edit 只返回 bool，带不出
            // 剪贴板 Task）；本分支仅为 match 穷尽性兜底，正常路径不可达
            E::CopyBookmarkedLines => false,
            // ---------- 括号匹配（第 61 轮） ----------
            // 纯光标移动：恒返回 false（不置脏），失败给状态栏提示
            E::JumpToMatchingBracket => {
                if !editor.jump_to_matching_bracket() {
                    hint = Some("光标不在括号旁（或未找到配对）".to_owned());
                }
                false
            }
            // ---------- 行操作扩充（第 62 轮） ----------
            E::ConvertTabsSpaces(kind) => editor.convert_tabs_spaces(kind),
            E::MergeLines => editor.merge_lines(),
            E::SplitLine => editor.split_line(),
            E::DeleteEmptyLines(kind) => editor.delete_empty_lines(kind),
            // ---------- 行注释切换（第 64 轮） ----------
            E::ToggleLineComment => editor.toggle_line_comment(),
            // ---------- 插入日期时间（第 63 轮） ----------
            // 真编辑：走 insert_str 统一管线（置脏+快照+查找重扫由上层
            // changed 驱动）；时间戳文本给状态栏反馈
            E::InsertDateTime => {
                let stamp = editor.insert_date_time();
                hint = Some(format!("已插入 {stamp}"));
                true
            }
        };
        drop(editor);

        if changed {
            {
                let tab = self.tab_mut();
                // P38：撤销/重做按「内容是否回到最近落盘版本」重算置脏——
                // 退净到基线即与磁盘一致，● 消失、关窗不再无谓拦截；
                // 其余编辑路径 back_to_saved 恒 false，行为不变
                tab.dirty = !back_to_saved;
                // P18：内容版本 +1 并刷新防抖起点（自动保存的触发依据）
                tab.note_mutation();
            }
            if back_to_saved {
                // P31：内存态变得比已提交清单更干净（清单还记着置脏页），
                // 下一拍心跳重写清单，防崩溃恢复把已回清的内容按旧快照复活
                self.session_manifest_stale = true;
            }
            // 编辑噪声只清普通信息：错误提示（如「保存失败」）必须持久
            // 到用户做出下一个有效动作才让位，否则打一个字就消失
            if !self.status_is_error {
                self.status.clear();
            }
        } else if let Some(hint) = hint {
            self.set_status(hint);
        }
        changed
    }

    // ---------- 加载管线 ----------

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
            self.status =
                "内存保护：合计内容超过上限，请先关闭部分大文档再打开".to_owned();
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
        self.active_load = Some(LoadJob { id, path, tab });
        self.progress = Some((0, 0));
        self.enter_busy();
        id
    }

    /// 统一的换文档入口（打开对话框/拖拽/最近文件共用）：
    /// 当前页 dirty 时绝不静默丢弃修改（含撤销链），先弹打开确认条。
    /// P21：落点 = [`Self::target_tab_for_open`]（空净当前页就地打开，否则新页）。
    fn request_open(&mut self, path: PathBuf) -> Task<Message> {
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

    /// P50：窗口聚焦时的外部修改巡检。
    ///
    /// * busy / 加载中跳过（在途任务的结果马上会刷新戳，此时比对无意义）；
    /// * 干净的**活动页**被外部修改 → 静默重载（无未保存工作可丢，内容
    ///   以磁盘为准；走既有加载管线，Loaded 归页时重记戳）；
    /// * 其余被改页（置脏页 / 后台页）→ 进入提示条队列由用户逐个裁决
    ///   （置脏页绝不能静默重载——那等于丢弃用户未保存的工作）；
    ///   P52 起队列聚合多页，条上显示总数，可逐个处理或全部忽略。
    /// * 一次聚焦至多发起一个重载（防批量加载风暴）；聚焦即全量重算
    ///   队列——已忽略的页（重记戳）自然不再命中。
    ///
    /// P62：热键捕获提交——组合串先过 core 归一（防御直接消息调用），
    /// 冲突检测（其他动作已占用该组合则报错并保持捕获态）→ 写重映射表
    /// → 持久化 → 退出捕获态。
    fn commit_hotkey_capture(&mut self, combo: String) -> Task<Message> {
        let Some(id) = self.hotkey_capture else {
            return Task::none();
        };
        if editpad_core::normalize_combo(&combo).is_none() {
            self.set_status_error(format!("「{combo}」不是有效的热键组合"));
            return Task::none();
        }
        if let Some(other) = HOTKEY_ACTIONS.iter().find(|a| {
            a.id != id
                && self
                    .hotkey_capture_conflicts_with(a.id, &combo)
        }) {
            self.status =
                format!("「{combo}」已被「{}」占用，换一个组合再试（Esc 取消）", other.desc);
            return Task::none();
        }
        self.settings.hotkeys.insert(id.to_owned(), combo.clone());
        self.persist_settings();
        self.hotkey_capture = None;
        self.set_status(format!("「{}」已绑定 {combo}", {
            HOTKEY_ACTIONS
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.desc)
                .unwrap_or(id)
        }));
        Task::none()
    }

    /// 该动作的当前生效组合是否与 `combo` 相同（冲突判定用）。
    fn hotkey_capture_conflicts_with(&self, id: &str, combo: &str) -> bool {
        self.settings
            .hotkeys
            .get(id)
            .map(String::as_str)
            .or_else(|| default_combo_of(id))
            == Some(combo)
    }

    fn check_external_changes(&mut self) {
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
    fn commit_tab_rename(&mut self) -> Task<Message> {
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
            self.set_status("加载/保存进行中，请稍后再重命名".to_owned());
            return Task::none();
        }
        let Some(target) = rename_target_path(&old, &self.rename_input) else {
            self.set_status_error("名称不能为空或含 \\/:*?\"<>| 等字符".to_owned());
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
            self.set_status_error(format!("重命名失败:目标已存在「{}」", target.display()));
            return Task::none(); // 保持输入态
        }
        match fs::rename(&old, &target) {
            Ok(()) => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.path = Some(target.clone());
                    // P50：路径变了旧戳作废，按新路径重记
                    tab.file_stamp = file_stamp(&target);
                }
                // P20/P32：最近文件与光标记忆随路径迁移（旧路径条目失效）
                let old_key = old.display().to_string();
                let new_key = target.display().to_string();
                if let Some(view) = self.settings.recent_views.remove(&old_key) {
                    self.settings.recent_views.insert(new_key.clone(), view);
                }
                if let Some(pos) =
                    self.settings.recent_files.iter().position(|p| *p == old_key)
                {
                    self.settings.recent_files[pos] = new_key.clone();
                }
                self.persist_settings();
                // 会话清单里记的是旧路径，下一拍重写
                self.session_manifest_stale = true;
                self.renaming_tab = None;
                self.rename_input.clear();
                self.set_status(format!("已重命名为「{new_key}」"));
            }
            Err(error) => {
                self.set_status_error(format!("重命名失败:{error}"));
            }
        }
        Task::none()
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let load = match &self.active_load {
            Some(job) => Subscription::run_with(job.clone(), build_load_stream),
            None => Subscription::none(),
        };
        // P10 的查找扫描走 Task::perform（见 schedule_find_scan），不经订阅
        // 0.14 没有 keyboard::on_key_press 了，用 listen_with 手动过滤按键；
        // 同一条流顺带捕获拖拽文件（FileDropped；FileHovered 忽略）。
        // P62：按键转发为 KeyPressed 消息、分发挪到 update——重映射表与
        // 热键捕获态必须读活状态，订阅闭包捕获会陈旧。
        let events =
            iced::event::listen_with(|event, status, _window| match (event, status) {
                (
                    iced::Event::Keyboard(keyboard::Event::KeyPressed {
                        key, modifiers, ..
                    }),
                    iced::event::Status::Ignored,
                ) => Some(Message::KeyPressed(key, modifiers)),
                (
                    iced::Event::Window(window::Event::FileDropped(path)),
                    _,
                ) => Some(Message::FileDropped(path)),
                // P50：窗口重新聚焦 = 外部修改巡检时机（编辑器无常驻轮询，
                // 焦点回归是最自然的检查点——用户刚从外部工具切回来）
                (iced::Event::Window(window::Event::Focused), _) => {
                    Some(Message::WindowFocused)
                }
                // P39/P40：窗口逻辑尺寸（浮层贴边钳制依据；iced_winit 已
                // 换算成逻辑坐标，与 mouse_area 光标坐标同空间）
                (iced::Event::Window(window::Event::Resized(size)), _) => {
                    Some(Message::ViewportResized(size.width, size.height))
                }
                // P102：窗口移动（逻辑坐标）→ 几何记忆（节流落盘见 update）
                (iced::Event::Window(window::Event::Moved(position)), _) => {
                    Some(Message::WindowMoved(position))
                }
                _ => None,
            });
        // 窗口关闭请求：exit_on_close_request(false) 后以订阅事件流转
        let close_requests = window::close_requests().map(Message::CloseRequested);
        // P18 即时保存不走订阅：编辑后由 maybe_schedule_autosave 直接派发
        // 「睡眠防抖→落盘」的专用线程（inflight 去重，至多一个挂起）
        Subscription::batch([load, events, close_requests])
    }

    // ---------- 保存 ----------

    fn save_as_dialog(&mut self) -> Task<Message> {
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

    fn save(&mut self) -> Task<Message> {
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
                    "检测到外部修改，已暂停保存：请先在提示条选择「重新加载」或「忽略」"
                        .to_owned();
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
            self.pending_backup_notice = Some(note);
        }
        // P67：按页编码偏好落盘（None = 默认 UTF-8，历史行为）
        let encoding = self
            .tab()
            .save_encoding
            .unwrap_or(editpad_core::SaveEncoding::Utf8);
        // P19 行动项 3：rope 结构共享克隆（O(1)），分块原子写盘，
        // 不再经 to_text() 产生全文 String（50MB 场景省 ~50MB 峰值）
        let doc = self.cur_handle.borrow().doc.clone();
        // P18 版本守卫：记录本次落盘对应的内容版本
        let version = self.tab().version;
        Task::perform(
            async move {
                let saved = editpad_core::save_document_encoded(&path, &doc, encoding)
                    .map_err(|e| e.to_string());
                (version, saved)
            },
            move |(version, result)| Message::Saved(version, result),
        )
    }

    // ---------- 即时保存（P18，按页独立） ----------

    /// 单个标签页的自动保存是否就绪：已命名、有未存改动、
    /// 无在途 IO、不与手动保存互斥、本页没有挂起任务。
    fn tab_autosave_ready(&self, idx: usize) -> bool {
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
    fn maybe_schedule_autosave(&mut self) -> Task<Message> {
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
                delay: std::time::Duration::from_secs(u64::from(
                    self.settings.autosave_delay_secs,
                )),
                backup_mode: self.settings.backup_mode.clone(),
            };
            self.tabs[idx].autosave_inflight = true;
            tasks.push(Task::perform(
                async move { drive_autosave_once(idx, path, doc, version, task).await },
                |message| message,
            ));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn record_recent(&mut self, path: &Path) {
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

    /// 统一设置落盘入口：测试注入 `settings_path_override` 时写到
    /// 临时目录，绝不动真实 %APPDATA%；否则走系统配置目录（尽力而为）。
    pub(crate) fn persist_settings(&self) {
        if let Some(path) = &self.settings_path_override {
            let _ = self.settings.save_to(path);
        } else {
            self.settings.save();
        }
    }

    /// P102：窗口几何节流落盘——拖动/拉伸是每帧事件，至少隔
    /// [`GEOMETRY_PERSIST_INTERVAL`] 才写一次盘；关闭路径在
    /// `handle_close_request` 里兜底补一次（见 view.rs）。
    fn persist_geometry_if_due(&mut self) {
        let now = std::time::Instant::now();
        let due = self
            .last_geometry_persist
            .is_none_or(|t| now.duration_since(t) >= GEOMETRY_PERSIST_INTERVAL);
        if due {
            self.last_geometry_persist = Some(now);
            self.persist_settings();
        }
    }

}

/// P102：窗口几何落盘节流窗（2 秒一道；最后一次状态由关闭路径兜底）。
const GEOMETRY_PERSIST_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(2);
