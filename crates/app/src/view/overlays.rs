//! 浮层与菜单渲染：标签右键菜单、顶部菜单栏与各下拉菜单、命令面板、
//! 列编辑器对话框、状态栏弹出菜单。
//!
//! （P159 自 view.rs 的 impl 块拆出，纯移动零行为变更。）

use super::*;
use iced::widget::column;

impl Editpad {
    /// P39：标签右键菜单浮层——整窗透明背板（点击即收起）+ 锚在指针
    /// 位置、贴边钳制后的菜单卡片。opaque 双层防穿透：背板捕获菜单外
    /// 点击不落到正文，卡片捕获卡片内空白处点击不触发背板关闭。
    /// P43：卡片高度按窗口钳制（小窗口不溢出、不盖满全屏），内容超高
    /// 时内部滚动——与设置弹窗同款适配策略。
    pub(super) fn context_menu_overlay(&self, idx: usize) -> Element<'_, Message> {
        let vh = self.viewport_size.1;
        let card_h = ctx_menu_card_h(vh);
        let (ax, ay) = clamp_menu_anchor(
            self.menu_anchor,
            self.viewport_size,
            CTX_MENU_W,
            card_h,
        );
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
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
        )
        .on_press(Message::TabContextMenuClosed)
        .on_right_press(Message::TabContextMenuClosed)
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
        let slot_x = MENU_BAR_LEFT + menubar_slot_idx(self.menubar_anchor.0) as f32 * MENU_SLOT_W;
        let (ax, ay) = clamp_menu_anchor(
            (slot_x, MENU_BAR_H),
            self.viewport_size,
            W,
            card_h,
        );
        let ay = ay.max(MENU_BAR_H); // 永不遮住菜单栏本身
        let card = opaque(
            container(
                scrollable(self.menubar_panel(idx))
                    .width(W)
                    .height(card_h),
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
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
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
        // P155：菜单项文案组装——「（✓ ）<当前语言的主标签>  <键位提示>」。
        //
        // 键位提示（`Ctrl+S` 之类）是**快捷键字面量**，不随界面语言变化，
        // 故与翻译无关；勾选前缀按开关态决定。抽出这一处是为了让菜单项
        // 在两种语言下排版一致（标签与键位之间恒为两个空格）。
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
        let lang = self.lang();
        let sep = || rule::horizontal(1);
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
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
                        item_label(lang, editpad_core::Key::MenuSelectAll, false, Some("Ctrl+A")),
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
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuInsertDateTime, false, Some("F5")),
                        interactive.then_some(Message::Edit(EditOp::InsertDateTime)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuToggleComment, false, Some("Ctrl+Q")),
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
                        item_label(lang, editpad_core::Key::MenuZoomReset, false, None),
                        interactive.then_some(Message::FontSizeDelta(
                            16.0 - self.display_font_size(),
                        )),
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
                        interactive.then_some(Message::SettingsShowWhitespaceToggled(
                            !self.settings.show_whitespace,
                        )),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuShowLineEndings,
                            self.settings.show_line_endings,
                            None,
                        ),
                        interactive.then_some(Message::SettingsShowLineEndingsToggled(
                            !self.settings.show_line_endings,
                        )),
                    ))
                    // 第 73 轮 ⑯：自动换行开关（查看菜单入口，与设置页同消息）
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuWordWrap,
                            self.settings.word_wrap,
                            None,
                        ),
                        interactive.then_some(Message::SettingsWordWrapToggled(
                            !self.settings.word_wrap,
                        )),
                    ))
                    // P134（C7）：本页自动换行三态循环——跟随全局/本页开/
                    // 本页关；有覆盖时全局项旁标注（标签条目自身即状态显示）
                    .push(item(
                        match self.tab().wrap_override {
                            None => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapFollow,
                                false,
                                None,
                            ),
                            Some(true) => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapOn,
                                true,
                                None,
                            ),
                            Some(false) => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapOff,
                                false,
                                None,
                            ),
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
    /// 会话全部页。title 参与模糊匹配，detail 仅展示。
    pub(crate) fn palette_all_entries(&self) -> Vec<crate::view::PaletteEntry> {
        match self.palette_mode {
            crate::state::PaletteMode::Commands => palette_commands(self.lang())
                .into_iter()
                .map(|c| PaletteEntry {
                    command_id: Some(c.id),
                    tab_index: None,
                    title: c.title.to_owned(),
                    detail: c.detail,
                })
                .collect(),
            crate::state::PaletteMode::Tabs => self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| PaletteEntry {
                    command_id: None,
                    tab_index: Some(i),
                    title: t.display_name_in(self.lang()),
                    detail: t
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| self.t(editpad_core::Key::TabUnsaved).to_owned()),
                })
                .collect(),
        }
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
    /// 执行当前选中条目并关闭面板。命令经 dispatch_action 复用既有
    /// 映射（空修饰键——Shift 选区透传不适用面板执行）；标签模式直接
    /// SwitchTab；列表为空时 no-op（面板保持打开）。
    pub(crate) fn palette_execute(&mut self) -> Task<Message> {
        let entries = self.palette_filtered();
        if entries.is_empty() {
            return Task::none();
        }
        let idx = self.palette_idx.min(entries.len() - 1);
        let entry = &entries[idx];
        let msg = match (entry.command_id, entry.tab_index) {
            (Some(id), _) => dispatch_action(id, keyboard::Modifiers::empty()),
            (_, Some(i)) => Some(Message::SwitchTab(i)),
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
            rows = rows.push(
                text(self.t(editpad_core::Key::PaletteNoMatch)).size(uipx).font(uifont).width(Fill),
            );
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
        let card = opaque(container(
            column![
                text_input(self.t(editpad_core::Key::PalettePlaceholder), &self.palette_input)
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
            .width(Fill)
        )
        .width(560)
        .style(popup_card_style));
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: 64.0, right: 0.0, bottom: 0.0, left: 0.0 }),
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
                text(self.t(editpad_core::Key::ColumnEditorTitle)).size(uipx).font(uifont).width(Fill),
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
                    text_input_w(self.t(editpad_core::Key::CeStartHint), &d.start, Message::ColumnEditorStartChanged),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CeStep),
                    text_input_w(self.t(editpad_core::Key::CeStepHint), &d.step, Message::ColumnEditorStepChanged),
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
                    text_input_w(self.t(editpad_core::Key::CePadHint), &d.pad_width, Message::ColumnEditorWidthChanged),
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
                button(text(self.t(editpad_core::Key::ButtonOk)).size(uipx).font(uifont))
                    .padding([3, 14])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorConfirmed),
                button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                    .padding([3, 14])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorToggled),
            ]
            .spacing(8),
        );
        let card = opaque(container(body).padding(14).width(420).style(popup_card_style));
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
                .padding(4)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
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
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe(named.then_some(Message::SaveWithEncoding(enc)))
        };
        let panel = column![
            item(&enc_label(self.lang(), "UTF-8"), editpad_core::SaveEncoding::Utf8),
            item(&enc_label(self.lang(), "UTF-8(BOM)"), editpad_core::SaveEncoding::Utf8Bom),
            item(&enc_label(self.lang(), "GBK"), editpad_core::SaveEncoding::Gbk),
            // P127：CJK 传统编码扩展（无法映射字符照旧按数值实体写入）
            item(&enc_label(self.lang(), "Big5"), editpad_core::SaveEncoding::Big5),
            item(&enc_label(self.lang(), "Shift_JIS"), editpad_core::SaveEncoding::ShiftJis),
            item(&enc_label(self.lang(), "EUC-JP"), editpad_core::SaveEncoding::EucJp),
            item(&enc_label(self.lang(), "EUC-KR"), editpad_core::SaveEncoding::EucKr),
        ]
        .spacing(2);
        self.status_menu_overlay(W, ITEM_H * 3.0 + 12.0, panel.into())
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
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe((!disabled).then_some(Message::ConvertEol(target)))
        };
        let panel = column![
            text(format!("{}{}", self.t(editpad_core::Key::EolCurrentPrefix), eol_label(current)))
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
