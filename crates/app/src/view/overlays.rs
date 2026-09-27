//! 浮层与菜单渲染：标签右键菜单、顶部菜单栏与各下拉菜单、命令面板、
//! 列编辑器对话框、状态栏弹出菜单。
//!
//! （P159 自 view.rs 的 impl 块拆出，纯移动零行为变更。）

use super::*;
use iced::widget::column;

/// P319：剪贴板历史条目在面板里显示多少个字符（超出补省略号）。
/// 与命令面板 detail 的 48 字符截断同一族：展示侧封顶，存储侧不截断。
const CLIP_PREVIEW_CHARS: usize = 60;

/// P155：菜单项文案组装——「（✓ ）<当前语言的主标签>  <键位提示>」。
///
/// 键位提示（`Ctrl+S` 之类）是**快捷键字面量**，不随界面语言变化，
/// 故与翻译无关；勾选前缀按开关态决定。抽出这一处是为了让菜单项
/// 在两种语言下排版一致（标签与键位之间恒为两个空格）。
/// P308 起菜单栏与正文右键菜单共用。
fn item_label(
    lang: editpad_core::Lang,
    key: editpad_core::Key,
    checked: bool,
    combo: Option<&str>,
) -> String {
    let mut s = String::with_capacity(48);
    if checked {
        s.push_str("✓ ");
    }
    s.push_str(key.text(lang));
    if let Some(combo) = combo {
        s.push_str("  ");
        s.push_str(combo);
    }
    s
}

impl Editpad {
    /// P39：标签右键菜单浮层——整窗透明背板（点击即收起）+ 锚在指针
    /// 位置、贴边钳制后的菜单卡片。opaque 双层防穿透：背板捕获菜单外
    /// 点击不落到正文，卡片捕获卡片内空白处点击不触发背板关闭。
    /// P43：卡片高度按窗口钳制（小窗口不溢出、不盖满全屏），内容超高
    /// 时内部滚动——与设置弹窗同款适配策略。
    pub(super) fn context_menu_overlay(&self, idx: usize) -> Element<'_, Message> {
        let vh = self.viewport_size.1;
        let card_h = ctx_menu_card_h(vh);
        let (ax, ay) = clamp_menu_anchor(self.menu_anchor, self.viewport_size, CTX_MENU_W, card_h);
        let card = opaque(
            // P56：宽度必须定宽 CTX_MENU_W——P46 的 Shrink 修法在 iced
            // 布局下不成立（Fill 子控件在 Shrink 测量中会撑到窗口宽，
            // 用户截图实测卡片 ~970px）；定宽与贴边钳制共用同一常量，
            // 估宽即实宽，不再有漂移
            container(
                scrollable(self.tab_context_panel(idx))
                    .height(card_h)
                    .width(CTX_MENU_W),
            )
            .padding(4)
            .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding {
                    top: ay,
                    right: 0.0,
                    bottom: 0.0,
                    left: ax,
                }),
        )
        .on_press(Message::TabContextMenuClosed)
        .on_right_press(Message::TabContextMenuClosed)
        .into()
    }
    /// P308：正文右键菜单浮层——背板、锚点钳制、卡片样式与标签右键菜单
    /// 完全同款（同一套 `menu_anchor` / `clamp_menu_anchor` / 高度适配）。
    pub(super) fn editor_context_menu_overlay(&self) -> Element<'_, Message> {
        let vh = self.viewport_size.1;
        let card_h = ctx_menu_card_h(vh);
        let (ax, ay) = clamp_menu_anchor(self.menu_anchor, self.viewport_size, CTX_MENU_W, card_h);
        let card = opaque(
            container(
                scrollable(self.editor_context_panel())
                    .height(card_h)
                    .width(CTX_MENU_W),
            )
            .padding(4)
            .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding {
                    top: ay,
                    right: 0.0,
                    bottom: 0.0,
                    left: ax,
                }),
        )
        .on_press(Message::EditorContextMenuClosed)
        .on_right_press(Message::EditorContextMenuClosed)
        .into()
    }

    /// P308：正文右键菜单面板。菜单项一律经 [`Message::EditorCtxCommand`]
    /// 转交**既有**消息（零新编辑逻辑），守卫口径与菜单栏一致：
    /// busy 时全灰，只读页只留纯读取与导航项。文案全部复用既有语言键
    /// （`Menu*` 与 `Hk*` 两族），不新增 i18n 条目。
    pub(super) fn editor_context_panel(&self) -> Element<'_, Message> {
        use editpad_core::Key as K;
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let lang = self.lang();
        let interactive = !self.busy;
        // 只读页：改内容与撤销/重做一律拒收（与 Ctrl+R 的拒绝口径一致）
        let writable = interactive && !self.cur_handle.borrow().read_only;
        let named = self.tab().path.is_some();
        let hits = self.cur_handle.borrow().has_find_hits();
        let item = |label: K, combo: Option<&str>, msg: Option<Message>| {
            button(
                container(
                    text(item_label(lang, label, false, combo))
                        .size(uipx)
                        .font(uifont),
                )
                .width(Fill),
            )
            .width(Fill)
            .padding([5, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe(msg)
        };
        // 菜单项 = 「收起自身 + 转交」，故每项只需包一层
        let cmd = |msg: Message| Some(Message::EditorCtxCommand(Box::new(msg)));
        let edit = |op: crate::editor::EditOp| cmd(Message::Edit(op));
        column![
            item(
                K::MenuUndo,
                Some("Ctrl+Z"),
                edit(EditOp::Undo).filter(|_| writable)
            ),
            item(
                K::MenuRedo,
                Some("Ctrl+Y"),
                edit(EditOp::Redo).filter(|_| writable)
            ),
            rule::horizontal(1),
            item(
                K::MenuCut,
                Some("Ctrl+X"),
                cmd(Message::CutRequested).filter(|_| writable)
            ),
            item(
                K::MenuCopy,
                Some("Ctrl+C"),
                cmd(Message::CopyRequested).filter(|_| interactive)
            ),
            item(
                K::MenuPaste,
                Some("Ctrl+V"),
                cmd(Message::PasteRequested).filter(|_| writable)
            ),
            item(
                K::MenuSelectAll,
                Some("Ctrl+A"),
                edit(EditOp::SelectAll).filter(|_| interactive)
            ),
            rule::horizontal(1),
            item(
                K::HkDelLine,
                Some("Ctrl+L"),
                edit(EditOp::DeleteLines).filter(|_| writable)
            ),
            item(
                K::HkDupLine,
                Some("Ctrl+D"),
                edit(EditOp::DuplicateLines).filter(|_| writable)
            ),
            item(
                K::MenuToggleComment,
                Some("Ctrl+Q"),
                edit(EditOp::ToggleLineComment).filter(|_| writable)
            ),
            item(
                K::HkBookmarkToggle,
                Some("Ctrl+F2"),
                edit(EditOp::ToggleBookmark).filter(|_| interactive)
            ),
            rule::horizontal(1),
            item(
                K::MenuFind,
                Some("Ctrl+F"),
                cmd(Message::FindToggled).filter(|_| interactive)
            ),
            item(
                K::MenuGoto,
                Some("Ctrl+G"),
                cmd(Message::GotoToggled).filter(|_| interactive)
            ),
            // P312（C11）：跳转历史。空栈时点了给一句状态栏提示（不静默）
            item(
                K::MenuNavBack,
                Some("Alt+←"),
                edit(EditOp::NavBack).filter(|_| interactive)
            ),
            item(
                K::MenuNavForward,
                Some("Alt+→"),
                edit(EditOp::NavForward).filter(|_| interactive)
            ),
            item(
                K::FifTitle,
                Some("F12"),
                cmd(Message::FindInFilesToggled).filter(|_| interactive)
            ),
            // P310（A6）：命中↔书签联动。只在有命中时放行（否则点了只会
            // 得到一句「当前没有查找命中」，不如直接灰掉）
            item(
                K::MenuMarkHitLines,
                None,
                edit(EditOp::MarkHitLinesAsBookmarks).filter(|_| interactive && hits)
            ),
            item(
                K::MenuCopyHitLines,
                None,
                edit(EditOp::CopyHitLines).filter(|_| interactive && hits)
            ),
            rule::horizontal(1),
            item(
                K::TabCopyPath,
                None,
                cmd(Message::CopyFilePath(None)).filter(|_| interactive && named)
            ),
        ]
        .spacing(2)
        .padding([4, 6])
        .into()
    }

    /// 菜单栏浮层：整窗透明背板 + 锚在触发按钮槽位下方的卡片。
    ///
    /// 第 70 轮修订（用户反馈「弹窗会移动」）：
    /// * 锚点读**展开瞬间冻结**的 menubar_anchor，且 x 对齐到按钮槽位
    ///   左缘（menubar_slot_idx 反推）——同一菜单无论点按钮哪个部位、
    ///   展开期间鼠标怎么动，浮层位置恒定；
    /// * 背板 on_move 跟踪指针 + on_press 时若落在菜单栏条带内 → 视为
    ///   点击另一菜单（MenuToggled 切换，主流横移手感），否则收起。
    pub(super) fn menubar_overlay(&self, idx: usize) -> Element<'_, Message> {
        const W: f32 = 240.0;
        let card_h = ctx_menu_card_h(self.viewport_size.1).min(380.0);
        // 锚点 x 对齐到按钮槽位左缘；反推越界（正常不会发生：展开的 idx 就
        // 来自同一个锚点）时退回本菜单自身的槽位
        let slot = menubar_slot_idx(self.menubar_anchor.0).unwrap_or(idx);
        let slot_x = MENU_BAR_LEFT + slot as f32 * MENU_SLOT_W;
        let (ax, ay) = clamp_menu_anchor((slot_x, MENU_BAR_H), self.viewport_size, W, card_h);
        let ay = ay.max(MENU_BAR_H); // 永不遮住菜单栏本身
        let card = opaque(
            container(scrollable(self.menubar_panel(idx)).width(W).height(card_h))
                .padding(4)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding {
                    top: ay,
                    right: 0.0,
                    bottom: 0.0,
                    left: ax,
                }),
        )
        .on_move(Message::MenubarHovered)
        .on_press(Message::MenubarPressed)
        .into()
    }
    /// 第 `idx` 个菜单的面板内容。全部复用既有消息（零新编辑逻辑）；
    /// 守卫口径与原工具栏按钮一致（busy / dirty / is_markdown）。
    pub(super) fn menubar_panel(&self, idx: usize) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let interactive = !self.busy;
        let item = |label: String, msg: Option<Message>| {
            button(container(text(label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .padding([5, 10])
                .style(chrome_menu_item_style)
                .on_press_maybe(msg)
        };
        // P155：菜单项文案组装见 [`item_label`]（P308 起正文右键菜单共用）。
        let lang = self.lang();
        let sep = || rule::horizontal(1);
        let is_markdown =
            self.cur_handle.borrow().highlight_syntax_name().as_deref() == Some("Markdown");
        // P310：命中↔书签两条菜单项的放行判据
        let hits = self.cur_handle.borrow().has_find_hits();
        let mut panel = column![].spacing(2).padding([4, 6]);
        match idx {
            // ---------- 文件 ----------
            0 => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuOpen, false, Some("Ctrl+O")),
                        interactive.then_some(Message::OpenRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuSave, false, Some("Ctrl+S")),
                        (interactive && self.tab().dirty).then_some(Message::SaveRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuSaveAs, false, None),
                        interactive.then_some(Message::SaveAsRequested),
                    ))
                    .push(sep())
                    .push(item(
                        format!(
                            "{}{}",
                            item_label(
                                lang,
                                editpad_core::Key::MenuReopenClosed,
                                false,
                                Some("Ctrl+Shift+W")
                            ),
                            if self.closed_stack.is_empty() {
                                self.t(editpad_core::Key::RecentsEmpty)
                            } else {
                                ""
                            }
                        ),
                        (!self.busy && !self.closed_stack.is_empty())
                            .then_some(Message::ReopenLastClosedFile),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuRecents, false, None),
                        interactive.then_some(Message::RecentsToggled),
                    ));
            }
            // ---------- 编辑 ----------
            1 => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuUndo, false, Some("Ctrl+Z")),
                        interactive.then_some(Message::Edit(EditOp::Undo)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuRedo, false, Some("Ctrl+Y")),
                        interactive.then_some(Message::Edit(EditOp::Redo)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuCut, false, Some("Ctrl+X")),
                        interactive.then_some(Message::CutRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuCopy, false, Some("Ctrl+C")),
                        interactive.then_some(Message::CopyRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuPaste, false, Some("Ctrl+V")),
                        interactive.then_some(Message::PasteRequested),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuSelectAll,
                            false,
                            Some("Ctrl+A"),
                        ),
                        interactive.then_some(Message::Edit(EditOp::SelectAll)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuFind, false, Some("Ctrl+F")),
                        interactive.then_some(Message::FindToggled),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuGoto, false, Some("Ctrl+G")),
                        interactive.then_some(Message::GotoToggled),
                    ))
                    // P310（A6）：命中↔书签联动，无命中时灰掉
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuMarkHitLines, false, None),
                        (interactive && hits)
                            .then_some(Message::Edit(EditOp::MarkHitLinesAsBookmarks)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuCopyHitLines, false, None),
                        (interactive && hits).then_some(Message::Edit(EditOp::CopyHitLines)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuInsertDateTime,
                            false,
                            Some("F5"),
                        ),
                        interactive.then_some(Message::Edit(EditOp::InsertDateTime)),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuToggleComment,
                            false,
                            Some("Ctrl+Q"),
                        ),
                        interactive.then_some(Message::Edit(EditOp::ToggleLineComment)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuColumnEditor, false, Some("F6")),
                        interactive.then_some(Message::ColumnEditorToggled),
                    ));
            }
            // ---------- 查看 ----------
            2 => {
                let step = editor::FONT_ZOOM_STEP;
                panel = panel
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuZoomIn,
                            false,
                            Some(editpad_core::Key::ComboWheel.text(lang)),
                        ),
                        interactive.then_some(Message::FontSizeDelta(step)),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuZoomOut,
                            false,
                            Some(editpad_core::Key::ComboWheel.text(lang)),
                        ),
                        interactive.then_some(Message::FontSizeDelta(-step)),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuZoomReset,
                            false,
                            Some("Ctrl+Shift+0"),
                        ),
                        interactive.then_some(Message::ZoomResetDefault),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuThemeToggle, false, None),
                        interactive.then_some(Message::ThemeToggled),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuShowWhitespace,
                            self.settings.show_whitespace,
                            None,
                        ),
                        interactive.then_some(Message::ToggleWhitespace),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuShowLineEndings,
                            self.settings.show_line_endings,
                            None,
                        ),
                        interactive.then_some(Message::ToggleLineEndings),
                    ))
                    // 第 73 轮 ⑯：自动换行开关（查看菜单入口，与设置页同消息）
                    // P316：三条"翻转当前态"都改走包装消息——菜单与命令面板
                    // 共用一份取反逻辑（`dispatch_action` 是纯函数拿不到现值）
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuWordWrap,
                            self.settings.word_wrap,
                            None,
                        ),
                        interactive.then_some(Message::ToggleWordWrap),
                    ))
                    // P134（C7）：本页自动换行三态循环——跟随全局/本页开/
                    // 本页关；有覆盖时全局项旁标注（标签条目自身即状态显示）
                    .push(item(
                        match self.tab().wrap_override {
                            None => {
                                item_label(lang, editpad_core::Key::MenuTabWrapFollow, false, None)
                            }
                            Some(true) => {
                                item_label(lang, editpad_core::Key::MenuTabWrapOn, true, None)
                            }
                            Some(false) => {
                                item_label(lang, editpad_core::Key::MenuTabWrapOff, false, None)
                            }
                        },
                        interactive.then_some(Message::TabWrapOverrideToggled),
                    ))
                    // P134（C7）：本页字号重置——仅在有覆盖时可用
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuTabFontReset, false, None),
                        (interactive && self.tab().font_size_override.is_some())
                            .then_some(Message::TabFontSizeReset),
                    ))
                    // 原「视图」菜单并入：MD 预览按当前语法门控（最近文件
                    // 面板与文件菜单的「最近文件」重复，不再单列）
                    .push(sep())
                    .push(item(
                        if self.preview_visible {
                            item_label(lang, editpad_core::Key::MenuPreviewClose, false, None)
                        } else {
                            item_label(lang, editpad_core::Key::MenuPreview, false, None)
                        },
                        (interactive && is_markdown).then_some(Message::PreviewToggled),
                    ));
            }
            // ---------- 设置 ----------
            _ => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuOpenSettings, false, None),
                        interactive.then_some(Message::SettingsToggled),
                    ))
                    .push(item(
                        format!(
                            "{}：{}",
                            self.t(editpad_core::Key::MenuBackupMode),
                            match self.settings.backup_mode.as_str() {
                                editpad_core::settings::BACKUP_MODE_SIMPLE => {
                                    self.t(editpad_core::Key::BackupSimpleShort)
                                }
                                editpad_core::settings::BACKUP_MODE_TIMESTAMPED => {
                                    self.t(editpad_core::Key::BackupTimestampedShort)
                                }
                                _ => self.t(editpad_core::Key::BackupOff),
                            }
                        ),
                        interactive.then_some(Message::SettingsBackupModeToggled),
                    ));
            }
        }
        panel.into()
    }
    /// 面板全部条目（未过滤）：命令模式 = 注册表全量；标签模式 = 当前
    /// 会话全部页；P319 剪贴板模式 = 会话内历史（预览 + 规模，不带全文）。
    /// title 参与模糊匹配，detail 仅展示。
    pub(crate) fn palette_all_entries(&self) -> Vec<crate::view::PaletteEntry> {
        match self.palette_mode {
            crate::state::PaletteMode::Commands => palette_commands(self.lang())
                .into_iter()
                .map(|c| PaletteEntry {
                    command_id: Some(c.id),
                    tab_index: None,
                    clip_index: None,
                    title: c.title.to_owned(),
                    detail: self.palette_combo_detail(c.id),
                })
                .collect(),
            crate::state::PaletteMode::Tabs => self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| PaletteEntry {
                    command_id: None,
                    tab_index: Some(i),
                    clip_index: None,
                    title: t.display_name_in(self.lang()),
                    detail: t
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| self.t(editpad_core::Key::TabUnsaved).to_owned()),
                })
                .collect(),
            // P319：会话内剪贴板历史。条目只带预览与规模——
            // 本函数每帧重建，把全文抄进条目就是每帧抄一遍全文。
            crate::state::PaletteMode::Clipboard => self
                .clip_history
                .iter()
                .enumerate()
                .map(|(i, t)| PaletteEntry {
                    command_id: None,
                    tab_index: None,
                    clip_index: Some(i),
                    title: self.clip_preview(t),
                    detail: self.clip_meta(t),
                })
                .collect(),
        }
    }
    /// P319：一条历史的单行预览——前 [`CLIP_PREVIEW_CHARS`] 个字符，
    /// 换行折成 `⏎`、制表折成 `⇥`、`\r` 丢掉（面板一行只显示一行）。
    /// 超出则补一个省略号，所以「预览里没有 ⏎」等价于「这条本来就一行」。
    pub(crate) fn clip_preview(&self, text: &str) -> String {
        let mut out = String::new();
        let mut clipped = false;
        for (n, ch) in text.chars().enumerate() {
            if n >= CLIP_PREVIEW_CHARS {
                clipped = true;
                break;
            }
            match ch {
                '\n' => out.push('⏎'),
                '\r' => {}
                '\t' => out.push('⇥'),
                c => out.push(c),
            }
        }
        if clipped {
            out.push('…');
        }
        out
    }
    /// P320：面板的键位列 = **生效**键位的首个（窄列放不下同义键串），一个都没有
    /// 显 `—`。口径与设置页同源（[`crate::hotkeys::effective_combos`]）：改前这里
    /// 读 `default_combo_of`（只看注册表默认值），于是用户把 `Ctrl+S` 重映射成
    /// `F10` 之后面板仍写 `Ctrl+S`——而面板正是「忘了这招按什么」时去看的地方，
    /// 报一个错键比留空更糟。
    fn palette_combo_detail(&self, id: &str) -> String {
        HOTKEY_ACTIONS
            .iter()
            .find(|a| a.id == id)
            .map(|a| effective_combos(&self.settings.hotkeys, a))
            .and_then(|combos| combos.first().map(|combo| combo.to_string()))
            .unwrap_or_else(|| "—".to_owned())
    }
    /// P321：面板底部那行手势提示。只在命令模式给——标签模式没有"给命令赋键"
    /// 这件事，写上去就是假提示。抽成方法（与 [`Self::palette_empty_label`] 同理）：
    /// 视图树夹具读得到几何、读不到文本，这句"哪个模式说哪句话"得有地方能被钉。
    pub(crate) fn palette_hint_line(&self) -> Option<&'static str> {
        match self.palette_mode {
            crate::state::PaletteMode::Commands => Some(self.t(editpad_core::Key::PaletteHintKeys)),
            _ => None,
        }
    }
    /// P319：面板空态那句话。剪贴板模式说"还没复制过"而不是"搜不到"——
    /// 第一次开这个面板必然两手空空，报"无匹配"会让人以为面板坏了。
    /// 抽成方法而非写在绘制里，是因为视图树夹具读不到文本、只读得到几何，
    /// 这句"哪个模式说哪句话"的裁决得有地方能被用例钉住。
    pub(crate) fn palette_empty_label(&self) -> &'static str {
        if self.palette_mode == crate::state::PaletteMode::Clipboard {
            self.t(editpad_core::Key::ClipHistoryEmpty)
        } else {
            self.t(editpad_core::Key::PaletteNoMatch)
        }
    }
    /// P319：一条历史的规模摘要「N 行 · M 字节」。行数按 `lines()`（正文
    /// 末尾换行不多算一行），字节按**实际 UTF-8 长度**——与逐出侧同一个口径。
    fn clip_meta(&self, text: &str) -> String {
        use editpad_core::Key as K;
        format!(
            "{} {} · {} {}",
            text.lines().count(),
            self.t(K::ClipUnitLines),
            text.len(),
            self.t(K::ClipUnitBytes)
        )
    }
    /// 面板过滤条目（fuzzy_filter 稳定降序）。查询串对 title 与 detail
    /// 拼接匹配——命令可用 id 片段检索（如 readonly），标签可用路径检索。
    pub(crate) fn palette_filtered(&self) -> Vec<PaletteEntry> {
        let pairs: Vec<(PaletteEntry, String)> = self
            .palette_all_entries()
            .into_iter()
            .map(|e| {
                let hay = format!("{} {}", e.title, e.detail);
                (e, hay)
            })
            .collect();
        fuzzy_filter(&pairs, &self.palette_input)
            .into_iter()
            .map(|(e, _)| e)
            .collect()
    }
    /// P321：当前选中条目的**命令 id**（标签行／剪贴板行／空列表 ⇒ None）。
    /// 下标算式与 [`Self::palette_execute`] 同一条，不在这里另起一份口径。
    pub(crate) fn palette_selected_command_id(&self) -> Option<&'static str> {
        let entries = self.palette_filtered();
        if entries.is_empty() {
            return None;
        }
        let idx = self.palette_idx.min(entries.len() - 1);
        entries[idx].command_id
    }
    /// 执行当前选中条目并关闭面板。命令经 dispatch_action 复用既有
    /// 映射（空修饰键——Shift 选区透传不适用面板执行）；标签模式直接
    /// SwitchTab；P319 剪贴板模式投 ClipPick（**存储**下标，过滤只改了
    /// 显示顺序，不改历史本身的次序）；列表为空时 no-op（面板保持打开）。
    pub(crate) fn palette_execute(&mut self) -> Task<Message> {
        let entries = self.palette_filtered();
        if entries.is_empty() {
            return Task::none();
        }
        let idx = self.palette_idx.min(entries.len() - 1);
        let entry = &entries[idx];
        let msg = match (entry.command_id, entry.tab_index, entry.clip_index) {
            (Some(id), ..) => dispatch_action(id, keyboard::Modifiers::empty()),
            (_, Some(i), _) => Some(Message::SwitchTab(i)),
            (_, _, Some(i)) => Some(Message::ClipPick(i)),
            _ => None,
        };
        self.palette_visible = false;
        // P151：面板关闭即把输入焦点还给正文（否则中文输入法在正文里失效）
        self.focus_editor();
        match msg {
            Some(m) => self.update(m),
            None => Task::none(),
        }
    }
    /// 面板浮层：顶部居中卡片 = 查询输入框 + 过滤结果（选中行 ▶ 标记，
    /// 滑动窗口最多 12 行保证选中可见）。opaque 背板点击即收起，
    /// 卡片自身 opaque 防穿透（右键菜单同款双层）。
    pub(super) fn palette_overlay(&self) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let entries = self.palette_filtered();
        let total = entries.len();
        let sel = self.palette_idx.min(total.saturating_sub(1));
        let win_start = sel.saturating_sub(11);
        let mut rows = column![].spacing(0).width(Fill);
        if total == 0 {
            let empty = self.palette_empty_label();
            rows = rows.push(text(empty).size(uipx).font(uifont).width(Fill));
        }
        for (i, e) in entries.iter().enumerate().skip(win_start).take(12) {
            let marker = if i == sel { "▶ " } else { "　 " };
            let detail: String = if e.detail.chars().count() > 48 {
                let t: String = e.detail.chars().take(47).collect();
                format!("{t}…")
            } else {
                e.detail.clone()
            };
            rows = rows.push(
                button(
                    text(format!("{marker}{}　{}", e.title, detail))
                        .size(uipx)
                        .font(uifont)
                        .width(Fill),
                )
                .width(Fill)
                .padding([3, 10])
                .style(chrome_menu_item_style)
                .on_press(Message::PalettePick(i)),
            );
        }
        let mut col = column![
            text_input(
                self.t(editpad_core::Key::PalettePlaceholder),
                &self.palette_input
            )
            .id(palette_input_id())
            .size(uipx)
            .font(uifont)
            .on_input(Message::PaletteInputChanged)
            .on_submit(Message::PaletteExecute)
            .padding([4, 8]),
            rule::horizontal(1),
            scrollable(rows).height(360.0),
        ]
        .spacing(4)
        .padding(6)
        .width(Fill);
        // P321：底部一行手势提示。F4 是这轮新加的动作，不写出来就等于没加——
        // 面板里"这条命令没有键"恰恰是它 most 需要被发现的那一刻。
        if let Some(hint) = self.palette_hint_line() {
            col = col.push(text(hint).size(uipx * 0.85).font(uifont));
        }
        let card = opaque(container(col).width(560).style(popup_card_style));
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding {
                    top: 64.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                }),
        )
        .on_press(Message::PaletteToggled(self.palette_mode))
        .into()
    }
    /// B9：列编辑器对话框——整窗背板（点击关闭）+ 居中卡片（设置弹窗
    /// P40 同款结构、命令面板同款 opaque 防穿透）。文本/序号两模式；
    /// Enter 确认 / Esc 取消在 KeyPressed 层拦截，输入框字符键不串层
    /// （P129 同口径）。数值输入以字符串承载、确认时统一校验。
    pub(super) fn column_editor_overlay(&self) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let d = &self.column_editor;
        let text_input_w = |placeholder: &'static str, value: &str, msg: fn(String) -> Message| {
            text_input(placeholder, value)
                .size(uipx)
                .font(uifont)
                .width(Fill)
                .on_input(msg)
                .padding([3, 8])
        };
        let mode_btn = |label: &str, active: bool| {
            button(
                text(format!("{} {label}", if active { "●" } else { "○" }))
                    .size(uipx)
                    .font(uifont),
            )
            .padding([3, 10])
            .style(chrome_button_style)
            .on_press(Message::ColumnEditorModeToggled)
        };
        fn labeled<'a>(
            label: &str,
            control: impl Into<iced::Element<'a, Message>>,
            uipx: f32,
            uifont: iced::Font,
        ) -> iced::Element<'a, Message> {
            row![
                text(label.to_owned()).size(uipx).font(uifont),
                control.into(),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Fill)
            .into()
        }
        let mut body = column![].spacing(8).width(Fill);
        body = body.push(
            row![
                text(self.t(editpad_core::Key::ColumnEditorTitle))
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
                button(text("×").size(uipx).font(uifont))
                    .padding([1, 6])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorToggled),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        body = body.push(
            row![
                mode_btn(self.t(editpad_core::Key::CeModeText), !d.number_mode),
                mode_btn(self.t(editpad_core::Key::CeModeNumber), d.number_mode)
            ]
            .spacing(6),
        );
        if d.number_mode {
            body = body
                .push(labeled(
                    self.t(editpad_core::Key::CeStart),
                    text_input_w(
                        self.t(editpad_core::Key::CeStartHint),
                        &d.start,
                        Message::ColumnEditorStartChanged,
                    ),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CeStep),
                    text_input_w(
                        self.t(editpad_core::Key::CeStepHint),
                        &d.step,
                        Message::ColumnEditorStepChanged,
                    ),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CeBase),
                    button(
                        text(match d.base {
                            editor::NumBase::Dec => self.t(editpad_core::Key::CeBaseDec).to_owned(),
                            editor::NumBase::Hex => self.t(editpad_core::Key::CeBaseHex).to_owned(),
                            editor::NumBase::Bin => self.t(editpad_core::Key::CeBaseBin).to_owned(),
                            editor::NumBase::Oct => self.t(editpad_core::Key::CeBaseOct).to_owned(),
                        })
                        .size(uipx)
                        .font(uifont),
                    )
                    .padding([3, 10])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorBaseCycled),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CePadWidth),
                    text_input_w(
                        self.t(editpad_core::Key::CePadHint),
                        &d.pad_width,
                        Message::ColumnEditorWidthChanged,
                    ),
                    uipx,
                    uifont,
                ));
            if d.base == editor::NumBase::Hex {
                body = body.push(
                    iced::widget::Checkbox::new(d.hex_upper)
                        .label(self.t(editpad_core::Key::CeHexUpper))
                        .font(uifont)
                        .text_size(uipx)
                        .on_toggle(|_| Message::ColumnEditorHexUpperToggled),
                );
            }
        } else {
            body = body.push(labeled(
                self.t(editpad_core::Key::CeModeText),
                text_input_w(
                    self.t(editpad_core::Key::CeTextContent),
                    &d.text,
                    Message::ColumnEditorTextChanged,
                ),
                uipx,
                uifont,
            ));
        }
        // 目标块实时反馈：行数提示替代打开守卫的二次检查
        let rows_info = match self.cur_handle.borrow().active_block() {
            Some((r0, r1, _, _)) => format!(
                "{}{}{}",
                self.t(editpad_core::Key::CeTargetBlockPrefix),
                r1 - r0 + 1,
                self.t(editpad_core::Key::CeTargetBlockSuffix)
            ),
            None => self.t(editpad_core::Key::CeNoBlockWarn).to_owned(),
        };
        body = body.push(text(rows_info).size(uipx).font(uifont));
        body = body.push(
            row![
                button(
                    text(self.t(editpad_core::Key::ButtonOk))
                        .size(uipx)
                        .font(uifont)
                )
                .padding([3, 14])
                .style(chrome_button_style)
                .on_press(Message::ColumnEditorConfirmed),
                button(
                    text(self.t(editpad_core::Key::ButtonCancel))
                        .size(uipx)
                        .font(uifont)
                )
                .padding([3, 14])
                .style(chrome_button_style)
                .on_press(Message::ColumnEditorToggled),
            ]
            .spacing(8),
        );
        let card = opaque(
            container(body)
                .padding(14)
                .width(420)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(Alignment::Center)
                .padding(16),
        )
        .on_press(Message::ColumnEditorToggled)
        .into()
    }
    /// P67：状态栏弹出菜单的通用浮层骨架——背板点击关闭 + 右下角贴
    /// 状态栏上缘的定宽卡片（锚点按窗口尺寸现算，无需指针跟踪）。
    fn status_menu_overlay<'a>(
        &self,
        card_w: f32,
        card_h: f32,
        panel: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let ax = (self.viewport_size.0 - card_w - 12.0).max(8.0);
        // 底缘贴状态栏上方（状态栏高 ≈ 32px），小窗口钳到 8px
        let ay = (self.viewport_size.1 - card_h - 36.0).max(8.0);
        let card = opaque(
            container(panel)
                .width(card_w)
                .height(card_h)
                .padding(STATUS_MENU_CARD_PAD)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding {
                    top: ay,
                    right: 0.0,
                    bottom: 0.0,
                    left: ax,
                }),
        )
        .on_press(Message::BarsDismissed)
        .into()
    }
    /// P67：编码弹出菜单——三种目标编码，未命名页禁用（无路径可写）。
    pub(super) fn encoding_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 200.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let named = self.tab().path.is_some();
        let item = |label: &str, enc: editpad_core::SaveEncoding| {
            button(text(label.to_owned()).size(uipx).font(uifont).width(Fill))
                .width(Fill)
                .padding([6, 10])
                .style(chrome_menu_item_style)
                .on_press_maybe(named.then_some(Message::SaveWithEncoding(enc)))
        };
        // 目标编码全集见 [`ENCODING_MENU_ITEMS`]。**菜单条目与卡片高度同源
        // 于那一张表**——P259 的教训：编码从 3 种扩到 7 种时卡片高度没跟着改，
        // 后四种被剪在固定 height 容器外，整半功能在界面上摸不到。
        const SPACING: f32 = 2.0;
        let items: Vec<Element<'_, Message>> = ENCODING_MENU_ITEMS
            .iter()
            .map(|(label, enc)| item(&enc_label(self.lang(), label), *enc).into())
            .collect();
        let panel = column(items).spacing(SPACING);
        self.status_menu_overlay(
            W,
            status_menu_card_h(ENCODING_MENU_ITEMS.len(), ITEM_H, SPACING),
            panel.into(),
        )
    }
    /// P67：行尾弹出菜单——当前主导行尾标头 + 两个转换项（已是目标
    /// 则禁用）。转换为可撤销的文档编辑。
    pub(super) fn eol_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 220.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let current = self.cur_handle.borrow().doc.line_ending();
        let item = |label: &str, target: editpad_core::LineEnding| {
            let disabled = current == target;
            button(text(label.to_owned()).size(uipx).font(uifont).width(Fill))
                .width(Fill)
                .padding([6, 10])
                .style(chrome_menu_item_style)
                .on_press_maybe((!disabled).then_some(Message::ConvertEol(target)))
        };
        let panel = column![
            text(format!(
                "{}{}",
                self.t(editpad_core::Key::EolCurrentPrefix),
                eol_label(current)
            ))
            .size(uipx)
            .font(uifont),
            item(
                &format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ConvertEolPrefix),
                    "CRLF",
                    self.t(editpad_core::Key::ConvertEolCrLfSuffix)
                ),
                editpad_core::LineEnding::CrLf
            ),
            item(
                &format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ConvertEolPrefix),
                    "LF",
                    self.t(editpad_core::Key::ConvertEolLfSuffix)
                ),
                editpad_core::LineEnding::Lf,
            ),
        ]
        .spacing(4);
        self.status_menu_overlay(W, ITEM_H * 2.0 + 28.0, panel.into())
    }
}

/// `Editpad::status_menu_overlay` 里卡片容器的内边距。**必须与卡片高度算法共用**：
/// 高度按条目数算时漏掉这份内边距，最后一项就会被剪在卡片外。
pub(crate) const STATUS_MENU_CARD_PAD: f32 = 4.0;

/// 编码弹层的全部目标编码——**菜单条目与卡片高度唯一的共同来源**。
///
/// 放成模块级 `const` 就是为了让 headless 用例能拿到条目数去核对卡片高度
/// （见 `tests/chrome.rs` 的 `status_menu_card_height_covers_*`）：原先条目
/// 长在 `column![]` 里、高度写死 `ITEM_H * 3.0 + 12.0`，两处之间没有任何
/// 结构联系，加一项就剪一项。
pub(crate) const ENCODING_MENU_ITEMS: &[(&str, editpad_core::SaveEncoding)] = &[
    ("UTF-8", editpad_core::SaveEncoding::Utf8),
    ("UTF-8(BOM)", editpad_core::SaveEncoding::Utf8Bom),
    ("GBK", editpad_core::SaveEncoding::Gbk),
    // P127：CJK 传统编码扩展（无法映射字符照旧按数值实体写入）
    ("Big5", editpad_core::SaveEncoding::Big5),
    ("Shift_JIS", editpad_core::SaveEncoding::ShiftJis),
    ("EUC-JP", editpad_core::SaveEncoding::EucJp),
    ("EUC-KR", editpad_core::SaveEncoding::EucKr),
];

/// 状态栏弹层卡片的**固定高度**：条目数 × 行高 + (条目数-1) × 间距 + 上下内边距。
///
/// 为什么单列成一个函数：卡片容器是固定 `height` 且**没有** `scrollable`
/// （见 `Editpad::status_menu_overlay` 的构造），所以"高度写死、条目加多"这一类
/// 脱钩不会报错，只会把多出来的条目**剪到卡片外面**——而锚点是
/// `vh - card_h - 36`，被剪的部分正好压到状态栏和窗口下缘之外，表现为
/// "点了没反应"。实测踩过一次：编码菜单从 3 项扩到 7 项（P127 补 Big5 /
/// Shift_JIS / EUC-JP / EUC-KR）时高度仍是 `ITEM_H * 3.0 + 12.0 = 108px`，
/// 后四项整个摸不到 ⇒ 等于 P127 那半功能在界面上不存在（P259）。
/// 现在高度一律由"构建条目用的那个集合"推导，两者结构上无法再脱钩。
pub(crate) fn status_menu_card_h(items: usize, item_h: f32, spacing: f32) -> f32 {
    let n = items.max(1) as f32;
    n * item_h + (n - 1.0) * spacing + STATUS_MENU_CARD_PAD * 2.0
}
