//! 编辑域消息处理（自 update.rs 拆出，纯移动零行为变更）。
//!
//! 承载编辑内核的入口：按键/编辑操作分发（[`Editpad::update_editor`]）、
//! 编辑总闸 [`Editpad::apply_edit`]、列编辑器序号生成与确认、
//! 热键捕获提交。方法体内依赖的状态与 helper 经 `use super::*` 解析。

use super::*;

/// P319：会话内剪贴板历史的条数封顶（保新弃旧）。
const CLIP_HISTORY_MAX: usize = 20;

/// P319：历史文本的**总量**封顶（字节）。刻意不按条封顶：单条一律存全文，
/// 因为「取回来的就是当时复制的那一份」是这项功能唯一的契约，截断即失信。
/// 超总量时从最旧一侧逐出，但至少保住栈顶那一条（极端情形下历史只剩一条）。
const CLIP_HISTORY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// P319：剪贴板历史里一条记录占用的字节数（按实际 UTF-8 字节，与 P300
/// 的内存记账口径一致——字符数×3 会把中文记录虚报三倍）。
pub(crate) fn clip_bytes(text: &str) -> usize {
    text.len()
}

/// P319：剪切的三种来源各自的后续删除（口径与改前逐条一致，只是收进了一个
/// 出口，见 [`Editpad::clip_write`]）。
enum CutFollow {
    /// 列块态：删块，且同步递归 update（置脏/自动保存调度当场走完）
    Block,
    /// 有选区：删选区（`Delete` 在有选区时只删选区）
    Range,
    /// 无选区：删触及行（`DeleteLines`，幻影末行由它兜底）
    Line,
}

impl Editpad {
    /// P319：记一条剪贴板历史。最近期在前；同内容只保栈顶一份（重复复制
    /// 同一段文字 = 把它挪到顶，不多出一条）；空文本不记（复制空行不该
    /// 挤掉有用的记录）。
    pub(crate) fn note_clip(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(pos) = self.clip_history.iter().position(|t| t == text) {
            self.clip_history.remove(pos);
        }
        self.clip_history.insert(0, text.to_owned());
        self.clip_history.truncate(CLIP_HISTORY_MAX);
        let mut total: usize = self.clip_history.iter().map(|t| clip_bytes(t)).sum();
        while total > CLIP_HISTORY_MAX_BYTES && self.clip_history.len() > 1 {
            if let Some(evicted) = self.clip_history.pop() {
                total -= clip_bytes(&evicted);
            }
        }
    }

    /// P319：**所有**剪贴板写入点的唯一出口——先记账，再写系统剪贴板。
    /// 收在一处是为了「一次复制手势 ⇒ 恰好一条历史」不可能被某个分支漏掉
    /// （此前五个写入点各自 `iced::clipboard::write`，新增任何一个都不会报警）。
    pub(crate) fn clip_write(&mut self, text: String) -> Task<Message> {
        self.note_clip(&text);
        iced::clipboard::write(text)
    }

    /// P321：命令面板里给**当前选中的那条命令**直接开录键态——收起面板，随后一次
    /// 按键走既有的 `HotkeyCaptureKey` 通道（冲突校验、写映射、持久化、状态栏反馈
    /// 全是设置页那一条实现，这里零新逻辑）。
    ///
    /// 为什么值得占一个键：一条命令"没键"往往就等于"不存在"。此前要给它一个键，
    /// 得先 `Ctrl+E` 认出它、关掉面板、打开设置 → 快捷键页、在近一百行里找到它、
    /// 点「修改」——而面板正是用户刚发现这个功能的那一屏。
    ///
    /// 选中的不是命令（标签行／剪贴板行／列表为空）⇒ **无操作且面板不关**：
    /// 那几类条目没有"要赋的键"，吞掉 F4 或收起面板都只会让人觉得键坏了。
    pub(crate) fn palette_assign_key(&mut self) -> Task<Message> {
        let Some(id) = self.palette_selected_command_id() else {
            return Task::none();
        };
        self.palette_visible = false;
        self.focus_editor();
        // 同步递归 update（`EditorCtxCommand`／列块剪切同例）：入口逻辑仍只有
        // `HotkeyCaptureStarted` 那一处，这里不复制一份"开始捕获"。
        self.update(Message::HotkeyCaptureStarted(id))
    }
    // ---------- 域方法（第 81 轮 Phase 1：update() 拆分） ----------
    /// 域：编辑器/剪贴板/光标/预览/高亮铺路。臂体自原 update() 逐字搬移，零行为变更。
    pub(super) fn update_editor(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 编辑器 ----------
            // 第 60 轮：复制标记行——只读操作前置拦截，取文本直接写剪贴板
            //（不置脏、不排自动保存；加载中与普通编辑同口径拒收）
            Message::Edit(EditOp::CopyBookmarkedLines) => {
                if self.active_load.is_some() {
                    return Task::none();
                }
                let text = self.cur_handle.borrow().copy_bookmarked_lines();
                match text {
                    Some(text) => self.clip_write(text),
                    None => Task::none(),
                }
            }
            // P310（路线图 A6）：命中行 ↔ 书签联动。两条都是只读操作
            //（书签是可回滚标注，不动正文），故与 CopyBookmarkedLines 同款
            // 在消息层前置拦截——apply_edit 只返回 bool，带不出「新增几条」
            // 与剪贴板 Task。
            Message::Edit(EditOp::MarkHitLinesAsBookmarks) => {
                use editpad_core::Key as K;
                if self.active_load.is_some() {
                    return Task::none();
                }
                let mut ed = self.cur_handle.borrow_mut();
                let none = ed.find_hl.is_empty();
                let n = ed.mark_hit_lines_as_bookmarks();
                drop(ed);
                if none {
                    self.set_status_error(self.t(K::StNoFindHits).to_owned());
                } else if n == 0 {
                    self.set_status(self.t(K::StHitsAlreadyMarked).to_owned());
                } else {
                    self.set_status(format!("{} {n}", self.t(K::StHitsMarkedPrefix)));
                }
                Task::none()
            }
            Message::Edit(EditOp::CopyHitLines) => {
                use editpad_core::Key as K;
                if self.active_load.is_some() {
                    return Task::none();
                }
                let text = self.cur_handle.borrow().copy_hit_lines_text();
                match text {
                    Some(text) => {
                        let n = text.matches('\n').count();
                        self.set_status(format!("{} {n}", self.t(K::StHitsCopiedPrefix)));
                        self.clip_write(text)
                    }
                    None => {
                        self.set_status_error(self.t(K::StNoFindHits).to_owned());
                        Task::none()
                    }
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

            // P153：正文被左键按下 → 查找框转半透明（用户点单：点正文编辑时
            // 别让浮层糊住内容）。关栏/未开栏时零动作；已淡出则幂等返回
            // （连续点正文不重复重建）。
            //
            // 焦点无需在此处理：正文控件的 pointer_focus(true) 已在同一事件
            // 里接管焦点（置位 + 请求 IME），且打断组字由它负责。**反向路径
            // 才需要补**：点查找框的按下被 PressObserver 消费，正文收不到
            // 「区外按下」，故 `FindBoxPressed` 里显式 `pointer_focus(false)`。
            Message::EditorBodyPressed => {
                if self.find_visible && !self.find_dimmed {
                    self.find_dimmed = true;
                }
                Task::none()
            }

            // P125：覆写/插入切换（busy 加载中拒收——编辑同口径）
            Message::ToggleOverwrite => {
                if self.busy {
                    return Task::none();
                }
                let now = {
                    let mut handle = self.cur_handle.borrow_mut();
                    handle.overwrite = !handle.overwrite;
                    handle.overwrite
                };
                self.set_status(if now {
                    self.t(editpad_core::Key::StOverwriteOn).to_owned()
                } else {
                    self.t(editpad_core::Key::StInsertMode).to_owned()
                });
                Task::none()
            }
            // P126：只读锁定切换（Ctrl+R；busy 加载中拒收——编辑同口径）
            Message::ToggleReadOnly => {
                if self.busy {
                    return Task::none();
                }
                let now = {
                    let mut handle = self.cur_handle.borrow_mut();
                    handle.read_only = !handle.read_only;
                    handle.read_only
                };
                self.set_status(if now {
                    self.t(editpad_core::Key::StReadOnlyOn).to_owned()
                } else {
                    self.t(editpad_core::Key::StReadOnlyOff).to_owned()
                });
                Task::none()
            }
            // ---------- 剪贴板（P4） ----------
            Message::CopyRequested => {
                // P319：三种来源收进**一个**出口（第 67 轮 ⑮ 列块态优先复制块内容
                // → 有选区复制选区 → P122 无选区复制整行含行尾）。分叉各写一次
                // 剪贴板的话，「一次手势恰好一条历史」就要靠三个分支都不漏。
                let text = {
                    let ed = self.cur_handle.borrow();
                    ed.block_copy_text()
                        .or_else(|| ed.selected_text())
                        .unwrap_or_else(|| ed.current_line_copy_text())
                };
                self.clip_write(text)
            }
            Message::CutRequested => {
                // P319：同 CopyRequested 的单出口口径；三种来源的**后续删除**
                // 分叉一字未改：列块/选区删选区（列块那支仍走同步递归 update，
                // 继承置脏与自动保存调度——Pasted 先例），无选区整行删触及行。
                let (text, follow) = {
                    let ed = self.cur_handle.borrow();
                    if let Some(text) = ed.block_copy_text() {
                        (text, CutFollow::Block)
                    } else if let Some(text) = ed.selected_text() {
                        (text, CutFollow::Range)
                    } else {
                        (ed.current_line_copy_text(), CutFollow::Line)
                    }
                };
                let write = self.clip_write(text);
                match follow {
                    CutFollow::Block => write.chain(self.update(Message::Edit(EditOp::Delete))),
                    CutFollow::Range => write.chain(Task::done(Message::Edit(EditOp::Delete))),
                    CutFollow::Line => write.chain(Task::done(Message::Edit(EditOp::DeleteLines))),
                }
            }
            Message::PasteRequested => {
                // clipboard::read 返回 Task<Option<String>>
                iced::clipboard::read().map(|content| Message::Pasted(content.unwrap_or_default()))
            }
            Message::Pasted(text) => {
                if text.is_empty() {
                    Task::none()
                } else {
                    self.update(Message::Edit(EditOp::InsertText(text)))
                }
            }
            // ---------- P319：从剪贴板历史面板取用一条 ----------
            Message::ClipPick(i) => {
                // 下标越界 = 面板那一帧之后历史变了（例如另一条消息逐出了旧项）。
                // 静默 no-op 比"插入不相干的一条"好，也不许 panic。
                let Some(text) = self.clip_history.get(i).cloned() else {
                    return Task::none();
                };
                // 取用也是一次"用"：挪到栈顶（MRU），条数不变
                self.clip_history.remove(i);
                self.clip_history.insert(0, text.clone());
                // 插入走既有粘贴路径 ⇒ 置脏、撤销、只读前置闸全部继承，
                // 本功能不长第二个写正文的入口。
                self.update(Message::Pasted(text))
            }
            // ---------- 正文右键菜单（P308） ----------
            Message::EditorContextMenu(x, y) => {
                // 与标签右键菜单互斥：同一时刻只允许一个浮层菜单在场，
                // 否则两层背板互相吞事件
                self.tab_context_menu = None;
                self.menu_anchor = (x, y);
                self.editor_context_menu = true;
                Task::none()
            }
            Message::EditorContextMenuClosed => {
                self.editor_context_menu = false;
                Task::none()
            }
            Message::EditorCtxCommand(inner) => {
                self.editor_context_menu = false;
                self.update(*inner)
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
                // P129：面板可见时拦截导航/执行/关闭。输入框捕获的字符
                // 键不会到达本层（订阅只转发 Ignored 事件）——打字不串。
                if self.palette_visible {
                    match (&key, modifiers) {
                        (keyboard::Key::Named(Named::ArrowUp), _) => {
                            return Task::done(Message::PaletteMove(false))
                        }
                        (keyboard::Key::Named(Named::ArrowDown), _) => {
                            return Task::done(Message::PaletteMove(true))
                        }
                        (keyboard::Key::Named(Named::Enter), _) => {
                            return Task::done(Message::PaletteExecute)
                        }
                        // P321：面板里选中一条命令按 F4 = 直接给它赋键
                        (keyboard::Key::Named(Named::F4), _) => return self.palette_assign_key(),
                        (keyboard::Key::Named(Named::Escape), _) => {
                            self.palette_visible = false;
                            // P151：焦点还给正文
                            self.focus_editor();
                            return Task::none();
                        }
                        _ => {}
                    }
                }
                // B9：列编辑器对话框可见时 Enter=确认 / Esc=取消（先于
                // 列块 Esc 清块分支——对话框开着时 Esc 语义属于对话框）。
                // 输入框字符键不到本层（订阅只转发 Ignored），不串打字。
                if self.column_editor_visible {
                    match (&key, modifiers) {
                        (keyboard::Key::Named(Named::Escape), _) => {
                            self.column_editor_visible = false;
                            return Task::none();
                        }
                        (keyboard::Key::Named(Named::Enter), _) => {
                            return Task::done(Message::ColumnEditorConfirmed);
                        }
                        _ => {}
                    }
                }
                // B10 多光标：Esc = 折叠为单光标（设计 §3.3），先于列块
                // Esc 清块分支消费按键
                if let keyboard::Key::Named(Named::Escape) = &key {
                    if self.cur_handle.borrow().has_multi() {
                        self.cur_handle.borrow_mut().collapse_multi();
                        return Task::none();
                    }
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
                    self.set_status(self.t(editpad_core::Key::StJsonOnly).to_owned());
                    return Task::none();
                }
                let (text, chars) = {
                    let ed = self.cur_handle.borrow();
                    (ed.doc.to_text(), ed.doc.text_len())
                };
                if chars > FORMAT_JSON_MAX_CHARS {
                    // 单遍重排是同步操作，超大文件会冻结 UI——先挡下并提示
                    self.status = format!(
                        "{}{chars}{}{FORMAT_JSON_MAX_CHARS}{}",
                        self.t(editpad_core::Key::StTooLargeJsonPrefix),
                        self.t(editpad_core::Key::StTooLargeJsonMiddle),
                        self.t(editpad_core::Key::StTooLargeJsonSuffix)
                    );
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
                        self.set_status(self.t(editpad_core::Key::StJsonFormatted).to_owned());
                        if self.find_visible {
                            // 内容变了：命中表过期，走后台防抖重扫（P10 同款）
                            let find_task = self.schedule_find_scan();
                            return Task::batch([find_task, self.maybe_schedule_autosave()]);
                        }
                        self.maybe_schedule_autosave()
                    }
                    Err(error) => {
                        self.set_status_error(
                            self.t_suffix(editpad_core::Key::StJsonFailed, &error.to_string()),
                        );
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
                    self.set_status(format!(
                        "{}{pct}{}",
                        self.t(editpad_core::Key::StSyntaxAnalyzing),
                        self.t(editpad_core::Key::StSyntaxAnalyzingMiddle)
                    ));
                }
                Task::none()
            }
            Message::HlPaved(gen, tab_id, paved) => {
                if self.hl_paving == Some(gen) {
                    self.hl_paving = None;
                    // P146：按发起页 id 归页安装——曾装进「回报时刻的活动
                    // 页」：A 页大文件铺建中切到 B 页（同语言小文件、代次
                    // 同为 0），A 的检查点状态被装进 B，B 全文按 A 的语法
                    // 状态错色。页已被关则结果整体丢弃。
                    let installed = self.tabs.iter().position(|t| t.id == tab_id).map(|idx| {
                        self.tabs[idx]
                            .editor
                            .borrow_mut()
                            .install_highlighter_if_current(gen, *paved)
                    });
                    // 代次一致才安装；期间编辑过则整体丢弃——缺口由下一帧
                    // needs_paving 重新评估并续排（从存活检查点出发，代价小）
                    let _ = installed;
                    if self
                        .status
                        .starts_with(self.t(editpad_core::Key::StSyntaxAnalyzing))
                    {
                        self.status.clear();
                    }
                }
                Task::none()
            }
            Message::CaretTick => {
                // 打磨项：翻转闪烁相位（update 本身会触发重绘）。
                // P53：一条链两用——竖直滚动条淡出动画期间切换 33ms 快拍
                // 驱动渐变（相位翻转由 tick_blink 按真实间隔门控，不受影响），
                // 其余时间维持 ~530ms 常规节拍。
                // P149：节拍已改订阅时钟驱动（见 subscription 的 tick_stream），
                // 间隔随淡出态重键订阅，本臂只做相位翻转。
                self.cur_handle.borrow_mut().tick_blink();
                Task::none()
            }
            Message::PreviewToggled => {
                // 仅 Markdown 语法页可开预览（按钮本身已禁用，此处双保险）
                if self.cur_handle.borrow().highlight_syntax_name().as_deref() == Some("Markdown") {
                    self.preview_visible = !self.preview_visible;
                } else {
                    self.set_status(self.t(editpad_core::Key::StPreviewMarkdownOnly).to_owned());
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

    /// B9：列编辑器「确定」——校验草稿并生成插入文本，关闭对话框后经
    /// `EditOp::InsertText` 进列块插入管线（`insert_into_block`：撤销
    /// 单快照/书签/失效汇点/busy/只读总闸全继承，零新编辑路径）。
    /// 校验失败 = 对话框保持打开 + 状态栏错误提示（不静默丢弃输入）。
    pub(super) fn column_editor_confirm(&mut self) -> Task<Message> {
        if !self.column_editor_visible {
            return Task::none();
        }
        let d = self.column_editor.clone();
        let payload = if !d.number_mode {
            if d.text.is_empty() {
                Err(self.t(editpad_core::Key::ColumnEditorNoText).to_owned())
            } else {
                Ok(d.text)
            }
        } else {
            self.column_editor_sequence(&d)
        };
        match payload {
            Ok(text) => {
                self.column_editor_visible = false;
                self.status.clear();
                // 递归 update（Pasted 同款先例）：busy/只读守卫在
                // apply_edit 总闸二次生效，这里不重复裁决
                self.update(Message::Edit(EditOp::InsertText(text)))
            }
            Err(msg) => {
                self.set_status_error(msg);
                Task::none()
            }
        }
    }

    /// 序号模式校验与生成：数值解析 + 块行数检查 + 封顶拒绝，文本由
    /// `sequence_lines` 纯函数生成（多行拼接后与文本模式共用插入路径）。
    pub(super) fn column_editor_sequence(&self, d: &ColumnEditorDraft) -> Result<String, String> {
        let start: i64 = d.start.trim().parse().map_err(|_| {
            self.t(editpad_core::Key::ColumnEditorStartInteger)
                .to_owned()
        })?;
        let step: i64 = d.step.trim().parse().map_err(|_| {
            self.t(editpad_core::Key::ColumnEditorStepInteger)
                .to_owned()
        })?;
        let width: usize = d
            .pad_width
            .trim()
            .parse()
            .map_err(|_| self.t(editpad_core::Key::ColumnEditorPadInteger).to_owned())?;
        if width > editor::MAX_COLUMN_SEQ_WIDTH {
            return Err(format!(
                "{}{}{}",
                self.t(editpad_core::Key::StColumnEditorPadCapPrefix),
                editor::MAX_COLUMN_SEQ_WIDTH,
                self.t(editpad_core::Key::StColumnEditorPadCapSuffix)
            ));
        }
        let rows = match self.cur_handle.borrow().active_block() {
            Some((r0, r1, _, _)) => r1 - r0 + 1,
            None => return Err(self.t(editpad_core::Key::ColumnEditorNoBlock).to_owned()),
        };
        if rows > editor::MAX_COLUMN_SEQ_ROWS {
            return Err(format!(
                "{}{rows}{}{}",
                self.t(editpad_core::Key::StColumnEditorRowsPrefix),
                self.t(editpad_core::Key::StColumnEditorRowsMiddle),
                editor::MAX_COLUMN_SEQ_ROWS
            ));
        }
        Ok(editor::sequence_lines(rows, start, step, d.base, width, d.hex_upper).join("\n"))
    }

    /// 执行一次按键编辑；返回是否真的改动了文本。
    pub(super) fn apply_edit(&mut self, op: EditOp) -> bool {
        // 加载进行中不接收编辑，避免打到即将被替换的旧文档上
        if self.active_load.is_some() {
            return false;
        }
        // P126：只读锁定总闸——改内容动作拒收（判定 fail-safe：未列入
        // 「纯导航/纯标注」白名单的变体一律视为可变拒绝），被拒动作
        // 不清列块、不留任何状态痕迹
        if self.cur_handle.borrow().read_only && edit_op_mutates(&op) {
            self.set_status(self.t(editpad_core::Key::StDocReadOnlyLocked).to_owned());
            return false;
        }

        use EditOp as E;
        // 第 63 轮起 hint 升级为 String：插入日期时间等动态反馈不再
        // 需要 'static（曾用 leak() 属内存泄漏，已纠正）
        let mut hint: Option<String> = None;
        // 第 67 轮 ⑮：列块态只在白名单内存活（输入/退格/删除/块内 Tab 走
        // 块分支），其余任何编辑动作先清块——单点收口防漏清
        if !matches!(
            op,
            E::InsertText(_) | E::Backspace | E::Delete | E::CancelBlock | E::TabKey(false)
        ) {
            self.cur_handle.borrow_mut().clear_block();
        }
        // B10 多光标存活白名单（设计 §3.3）：InsertText/Backspace/Delete
        // （Phase 2 同步编辑）+ 行内 Left/Right + CancelBlock + AddNextMatch；
        // 白名单外一律先折叠为单光标再走既有路径——单点收口防漏折
        if self.cur_handle.borrow().has_multi()
            && !matches!(
                op,
                E::InsertText(_)
                    | E::Backspace
                    | E::Delete
                    | E::CancelBlock
                    | E::AddNextMatch
                    | E::Motion(Motion::Left | Motion::Right, false)
            )
        {
            self.cur_handle.borrow_mut().collapse_multi();
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
                    hint = Some(self.t(editpad_core::Key::HintNoMoreUndo).to_owned());
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::Redo => {
                let changed = editor.redo();
                if !changed {
                    hint = Some(self.t(editpad_core::Key::HintAtLatest).to_owned());
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::InsertText(text) => {
                // 第 67 轮 ⑮：列块态下输入 = 逐行替换块内容（v1 单行文本）
                if editor.has_block() {
                    editor.insert_into_block(&text)
                } else if let Some(changed) =
                    editor.multi_edit(editor::MultiEditKind::Insert(&text))
                {
                    // B10 Phase 2：多光标同步插入（None = 触发折叠回退，
                    // 落回普通单光标路径）
                    changed
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
                } else if let Some(changed) = editor.multi_edit(editor::MultiEditKind::Backspace) {
                    changed
                } else {
                    editor.backspace()
                }
            }
            E::Delete => {
                // 文档末尾的 Delete 同样可能是空操作
                if editor.has_block() {
                    editor.delete_block_content()
                } else if let Some(changed) = editor.multi_edit(editor::MultiEditKind::Delete) {
                    changed
                } else {
                    editor.delete_forward()
                }
            }
            E::AddNextMatch => {
                // B10 Phase 2：添加下一匹配（Ctrl+M，纯光标集操作不置脏）；
                // Err = 具体原因上状态栏（不在词上/无匹配/封顶）——P155 起
                // 错误是类型化的，按当前界面语言取文
                if let Err(err) = editor.add_next_match() {
                    hint = Some(err.text(self.lang()));
                }
                false
            }
            E::CancelBlock => {
                // 第 67 轮 ⑮：Esc 取消列块（固定语义不入注册表）
                editor.clear_block();
                // P135：Esc 同时取消拖拽会话（选区保留）
                editor.cancel_dnd();
                false
            }
            // P135（路线图 B8）：拖拽释放——移动/复制当前选区到落点
            //（busy/只读守卫已在本函数头部把关）
            E::DropSelection { line, col, copy } => {
                editor.finish_drop_selection(crate::editor::CursorPos { line, col }, copy)
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
            // P310 两条同款：正常路径由消息层拦截（要带「新增几条」与剪贴板）
            E::MarkHitLinesAsBookmarks | E::CopyHitLines => false,
            // ---------- 括号匹配（第 61 轮） ----------
            // 纯光标移动：恒返回 false（不置脏），失败给状态栏提示
            E::JumpToMatchingBracket => {
                if !editor.jump_to_matching_bracket() {
                    hint = Some(self.t(editpad_core::Key::HintNoBracketPair).to_owned());
                }
                false
            }
            // ---------- P312（路线图 C11）：光标跳转历史 ----------
            // 同样纯光标移动：恒 false（不置脏）。空栈给一条状态栏提示——
            // 新按钮/新键位不该靠用户猜，与括号配对跳转同口径
            E::NavBack => {
                if !editor.nav_back() {
                    hint = Some(self.t(editpad_core::Key::HintNavNothingBack).to_owned());
                }
                false
            }
            E::NavForward => {
                if !editor.nav_forward() {
                    hint = Some(self.t(editpad_core::Key::HintNavNothingFwd).to_owned());
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
            // ---------- P121：智能缩进与选区块缩进 ----------
            // 回车不入列块白名单：列块态按回车先收块再智能缩进（原「向块
            // 内插换行」无实用语义）；Tab 分流——列块态加缩进仍是块内插
            // 制表符（与 InsertText 同路），其余走触及行整块缩进/反缩进
            // （无选区 Tab=插制表符、Shift+Tab=当前行反缩进，函数内兜底）
            E::Enter => {
                editor.enter();
                true
            }
            E::TabKey(outdent) => {
                if !outdent && editor.has_block() {
                    editor.insert_into_block("\t")
                } else {
                    editor.indent_touched_lines(outdent)
                }
            }
            // ---------- P122：词级删词（有选区退化为普通退格/删除） ----------
            E::DeleteWordLeft => editor.delete_word(true),
            E::DeleteWordRight => editor.delete_word(false),
            // ---------- P124：行操作扩展 ----------
            E::ReverseLines => editor.reverse_lines(),
            E::SortLinesNumeric(order) => editor.sort_lines_numeric(order),
            E::SortLinesLength(order) => editor.sort_lines_length(order),
            E::RemoveConsecutiveDuplicateLines => editor.remove_consecutive_duplicate_lines(),
            // ---------- 插入日期时间（第 63 轮） ----------
            // 真编辑：走 insert_str 统一管线（置脏+快照+查找重扫由上层
            // changed 驱动）；时间戳文本给状态栏反馈
            E::InsertDateTime => {
                let stamp = editor.insert_date_time();
                hint = Some(format!(
                    "{}{stamp}",
                    self.t(editpad_core::Key::StInsertedStampPrefix)
                ));
                true
            }
            // ---------- P128：选区文本工具（无选区/解码失败给状态栏提示） ----------
            E::ApplyTool(kind) => match editor.apply_tool(kind) {
                Ok(changed) => changed,
                Err(err) => {
                    hint = Some(err.text(self.lang()));
                    false
                }
            },
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
                self.touch_manifest_stale();
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
    pub(super) fn commit_hotkey_capture(&mut self, combo: String) -> Task<Message> {
        let Some(id) = self.hotkey_capture else {
            return Task::none();
        };
        if editpad_core::normalize_combo(&combo).is_none() {
            self.set_status_error(format!(
                "{}{combo}{}",
                editpad_core::Key::StHotkeyConflictPrefix.text(self.lang()),
                editpad_core::Key::StInvalidHotkey.text(self.lang())
            ));
            return Task::none();
        }
        if let Some(other) = HOTKEY_ACTIONS
            .iter()
            .find(|a| a.id != id && self.hotkey_capture_conflicts_with(a.id, &combo))
        {
            let lang = self.lang();
            self.status = format!(
                "{}{combo}{}{}{}",
                editpad_core::Key::StHotkeyConflictPrefix.text(lang),
                editpad_core::Key::StHotkeyConflictMiddle.text(lang),
                other.desc.text(lang),
                editpad_core::Key::StHotkeyConflictSuffix.text(lang),
            );
            return Task::none();
        }
        self.settings.hotkeys.insert(id.to_owned(), combo.clone());
        self.persist_settings();
        self.hotkey_capture = None;
        {
            let lang = self.lang();
            let name = HOTKEY_ACTIONS
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.desc.text(lang))
                .unwrap_or(id);
            self.set_status(format!(
                "{}{name}{combo}",
                editpad_core::Key::StHotkeyRebound.text(lang)
            ));
        }
        Task::none()
    }

    /// 该动作的当前生效组合是否与 `combo` 相同（冲突判定用）。
    pub(super) fn hotkey_capture_conflicts_with(&self, id: &str, combo: &str) -> bool {
        self.settings
            .hotkeys
            .get(id)
            .map(String::as_str)
            .or_else(|| default_combo_of(id))
            == Some(combo)
    }
}
