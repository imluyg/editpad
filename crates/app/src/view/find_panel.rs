//! 查找相关浮层与面板：查找/替换浮层、查找全部结果面板、在文件中查找
//! 面板，以及浮层位置钳制助手。
//!
//! （P159 自 view.rs 的 impl 块拆出，纯移动零行为变更。）

use super::*;
use iced::widget::column;

impl Editpad {
    pub(super) fn find_all_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let total = self.matches.len();
        let scanning = self.find_scanning();
        let shown = total.min(FIND_ALL_MAX_ROWS);
        // 标题行：文本 width(Fill) 把关闭按钮推到右缘
        let header = row![
            text(if scanning {
                self.t(editpad_core::Key::FindSearching).to_owned()
            } else if total == 0 {
                self.t(editpad_core::Key::FindNoMatch).to_owned()
            } else {
                editpad_core::fmt_match_total(self.lang(), total)
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::FindAllToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if shown > 0 {
            let editor = self.cur_handle.borrow();
            for i in 0..shown {
                let m = self.matches[i];
                let raw = editor.line_text(m.line);
                let excerpt = match_excerpt(&raw, m.col, FIND_ALL_EXCERPT_COLS);
                rows = rows.push(
                    button(
                        text(format!("{}:{}  {}", m.line + 1, m.col + 1, excerpt))
                            .size(uipx)
                            .font(uifont)
                            .width(Fill),
                    )
                    .width(Fill)
                    .padding([3, 10])
                    .style(chrome_menu_item_style)
                    .on_press(Message::FindAllGoto(i)),
                );
            }
        }
        if total > shown {
            rows = rows.push(
                text(format!(
                    "{}{}{}{}{}",
                    self.t(editpad_core::Key::FindAllShownPrefix),
                    shown,
                    self.t(editpad_core::Key::FindAllShownTotalMiddle),
                    total,
                    self.t(editpad_core::Key::FindAllShownTotalSuffix),
                ))
                .size(uipx)
                .font(uifont),
            );
        }
        // 高度按窗口钳制：小窗不溢出、大窗不占满（浮层卡片同策略）
        let h = (self.viewport_size.1 * 0.35).clamp(120.0, 300.0);
        container(
            column![header, rule::horizontal(1), scrollable(rows)]
                .spacing(0)
                .width(Fill),
        )
        .width(Fill)
        .height(h)
        .into()
    }
    /// A8：「在文件中查找」结果面板——与查找全部面板同槽停靠（互斥
    /// 由装配处的 fif_visible 裁决）。文件头分组行（相对路径 + 命中数）
    /// 与可点命中行（行:列 + 扫描时预计算的摘录）。显示行数与查找全
    /// 部面板同量级封顶，超出明示。
    pub(super) fn find_in_files_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let scanning = self.fif_scan.is_some();
        let total_files = self.fif_results.len();
        let total_hits: usize = self.fif_results.iter().map(|f| f.hits.len()).sum();
        let title = if scanning {
            format!(
                "{}{}{}",
                self.t(editpad_core::Key::ScanProgressPrefix),
                self.fif_progress.load(std::sync::atomic::Ordering::Relaxed),
                self.t(editpad_core::Key::ScanProgressSuffix)
            )
        } else if total_hits == 0 {
            self.t(editpad_core::Key::DirNoMatch).to_owned()
        } else {
            editpad_core::fmt_fif_panel_title(self.lang(), total_files, total_hits, false)
        };
        let header = row![
            text(if self.fif_truncated {
                format!("{title}{}", self.t(editpad_core::Key::FifReachedCapSuffix))
            } else {
                title
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::FindInFilesToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if !scanning && self.find_query.is_empty() {
            rows = rows.push(text(self.t(editpad_core::Key::FifQueryEmpty)).size(uipx).font(uifont));
        } else if !scanning {
            let mut budget = FIND_ALL_MAX_ROWS;
            'outer: for (fi, fh) in self.fif_results.iter().enumerate() {
                let rel = fh
                    .path
                    .strip_prefix(self.fif_dir.as_deref().unwrap_or(std::path::Path::new("")))
                    .unwrap_or(&fh.path)
                    .display()
                    .to_string();
                rows = rows.push(
                    text(editpad_core::fmt_file_hits(self.lang(), &rel, fh.hits.len()))
                        .size(uipx)
                        .font(uifont),
                );
                for (hi, h) in fh.hits.iter().enumerate() {
                    if budget == 0 {
                        break 'outer;
                    }
                    budget -= 1;
                    rows = rows.push(
                        button(
                            text(format!("{}:{}  {}", h.pos.line + 1, h.pos.col + 1, h.excerpt))
                                .size(uipx)
                                .font(uifont)
                                .width(Fill),
                        )
                        .width(Fill)
                        .padding([3, 10])
                        .style(chrome_menu_item_style)
                        .on_press(Message::FifGoto(fi, hi)),
                    );
                }
            }
            if budget == 0 {
                rows = rows.push(
                    text(format!(
                        "{}{}{}",
                        self.t(editpad_core::Key::FindAllShownPrefix),
                        FIND_ALL_MAX_ROWS,
                        self.t(editpad_core::Key::FindAllShownSuffix)
                    ))
                        .size(uipx)
                        .font(uifont),
                );
            }
        }
        let h = (self.viewport_size.1 * 0.35).clamp(120.0, 300.0);
        container(
            column![header, rule::horizontal(1), scrollable(rows)]
                .spacing(0)
                .width(Fill),
        )
        .width(Fill)
        .height(h)
        .into()
    }
    /// P150：查找/替换**轻浮层**——窗口居中定宽卡片（[`FIND_CARD_W`]），
    /// **无背板**：不遮正文、可继续编辑；Esc 或 × 关闭（用户点单：
    /// 查找 UI 不再占据下方整条，改到窗口中间且不要太宽）。
    ///
    /// 承载全部查找部件：①查询行（输入框带稳定 Id——Ctrl+F 后自动聚焦）；
    /// ②开关与计数行；③替换行（A8 目录模式换为目录/浏览行）；④面板开关行；
    /// ⑤结果面板（查找全部 / 目录命中，同槽互斥）——挂在浮层内下半区、
    /// 面板自身限高滚动。
    pub(super) fn find_overlay(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let total = self.matches.len();
        // P10：扫描在途时明确显示状态，按钮基于过期结果禁用
        let scanning = self.find_scanning();
        let position_label = if scanning {
            self.t(editpad_core::Key::FindSearching).to_owned()
        } else if total == 0 {
            self.t(editpad_core::Key::FindNoMatch).to_owned()
        } else {
            match self.match_idx {
                Some(i) => editpad_core::fmt_find_counter(self.lang(), Some(i), total),
                None => editpad_core::fmt_find_counter(self.lang(), None, total),
            }
        };
        let has_matches = !scanning && !self.matches.is_empty();
        // A8：FIF 开态下文档导航按钮禁用（命中表是另一套数据源）
        let doc_nav = !self.fif_visible;
        // P153：淡出系数（1.0 = 不淡出）。卡片背板、输入框、按钮、复选框
        // 共用同一系数——控件自带底色（输入框主题底 / 按钮白底 / 勾选点缀色），
        // 只压背板会留下「实心白块浮在淡卡片上」的口径分裂。
        let dim = find_dim_factor(self.find_dimmed);
        // P153：纯文本标签（无底色）的淡出用色——None = 不淡出（不设色，
        // 保持 iced 默认文字样式）。
        let label_color = find_label_color(&self.theme(), dim);

        // 拖动条（用户点单）：按住卡片顶部这条即可拖走浮层，双击复位到
        // 默认位置。`on_move`/`on_release` 挂在整层（见函数尾部）——按下
        // 只有这里捕获，其它位置点击照旧穿透到正文。
        let grip_text = text(self.t(editpad_core::Key::FindGrip))
            .size(uipx * 0.8)
            .font(uifont)
            .width(Fill)
            .align_x(iced::alignment::Horizontal::Center);
        let grip = mouse_area(
            container(match label_color {
                Some(c) => grip_text.color(c).into(),
                None => Element::from(grip_text),
            })
            .width(Fill)
            .padding([0, 0]),
        )
        .interaction(iced::mouse::Interaction::Grab)
        .on_press(Message::FindDragStart)
        .on_double_click(Message::FindDragReset);

        // ①查询行：P150 起输入框带 Id（打开查找栏即聚焦，修前焦点留正文）
        let query_row = row![
            text_input(self.t(editpad_core::Key::FindQueryPlaceholder), &self.find_query)
                .id(find_input_widget_id())
                .size(uipx)
                .font(uifont)
                .on_input(Message::FindQueryChanged)
                .on_submit(Message::FindNext)
                .style(move |theme, status| find_input_style(theme, status, dim))
                .width(Fill),
            button(text(self.t(editpad_core::Key::FindPrev)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(doc_nav.then_some(Message::FindPrev)),
            button(text(self.t(editpad_core::Key::FindNext)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(doc_nav.then_some(Message::FindNext)),
            button(text("×").size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press(Message::FindToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        // ②开关与计数行
        // P153：命中计数是纯文本、无底色可压 alpha——淡出时显式设色
        //（含「无匹配」「查找中…」，用户复报它们不跟着变淡）
        let position_widget = text(position_label).size(uipx).font(uifont);
        let position_text: Element<'_, Message> = match label_color {
            Some(c) => position_widget.color(c).into(),
            None => position_widget.into(),
        };
        let options_row = row![
            checkbox(self.case_sensitive)
                .label(self.t(editpad_core::Key::FindCaseSensitive))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::CaseToggled),
            // P70：正则模式开关（.* 是各编辑器通用的正则图标语义）
            checkbox(self.regex_enabled)
                .label(self.t(editpad_core::Key::FindRegexMode))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::RegexToggled),
            // 整词匹配：命中前后均非词字符（正则模式下不参与——扫描与
            // 替换路径均按 regex 分支先行返回）
            checkbox(self.whole_word)
                .label(self.t(editpad_core::Key::FindWholeWord))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::WholeWordToggled),
            // P123：命中计数（扫描在途给动态反馈；0 处也如实显示）
            position_text,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // ③替换行 / A8 目录行（两模式互斥）
        let third_row: Element<'_, Message> = if self.fif_visible {
            let dir_text = match &self.fif_dir {
                Some(d) => d.display().to_string(),
                None => self.t(editpad_core::Key::FifDirUnnamed).to_owned(),
            };
            let dir_label = text(self.t(editpad_core::Key::FindDir).to_owned()).size(uipx).font(uifont);
            let dir_label: Element<'_, Message> = match label_color {
                Some(c) => dir_label.color(c).into(),
                None => dir_label.into(),
            };
            let dir_path = text(dir_text).size(uipx).font(uifont);
            let dir_path: Element<'_, Message> = match label_color {
                Some(c) => dir_path.color(c).into(),
                None => dir_path.into(),
            };
            let scan_label = text(if self.fif_scan.is_some() {
                format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ScanProgressPrefix),
                    self.fif_progress.load(std::sync::atomic::Ordering::Relaxed),
                    self.t(editpad_core::Key::ScanProgressSuffix)
                )
            } else {
                String::new()
            })
            .size(uipx)
            .font(uifont);
            let scan_label: Element<'_, Message> = match label_color {
                Some(c) => scan_label.color(c).into(),
                None => scan_label.into(),
            };
            row![
                dir_label,
                // 长路径裁剪显示：不撑破定宽卡片（取舍：卡片内不可横向滚动）
                container(dir_path).width(Fill).clip(true),
                button(text(self.t(editpad_core::Key::FindBrowse)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press(Message::FifBrowseFolder),
                scan_label,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            row![
                text_input(self.t(editpad_core::Key::ReplaceQueryPlaceholder), &self.replace_query)
                    .size(uipx)
                    .font(uifont)
                    .on_input(Message::ReplaceQueryChanged)
                    .style(move |theme, status| find_input_style(theme, status, dim))
                    .width(Fill),
                // P70：正则模式替换当前 = 对命中做 $1 展开替换
                button(text(self.t(editpad_core::Key::FindReplaceCurrent)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press_maybe(has_matches.then_some(if self.regex_enabled {
                        Message::ReplaceCurrentRegex
                    } else {
                        Message::ReplaceCurrent
                    })),
                // 扫描在途时禁用：此刻的全文快照可能是过期的
                button(text(self.t(editpad_core::Key::FindReplaceAll)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press_maybe((!scanning).then_some(Message::ReplaceAll)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        };

        // ④面板开关行
        let panel_row = row![
            // 第 62 轮：查找全部结果面板开关（扫描在途/无命中时禁用；
            // A8：FIF 开态禁用——两套面板同槽互斥）
            button(text(self.t(editpad_core::Key::FindFindAll)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(
                    (has_matches && doc_nav).then_some(Message::FindAllToggled),
                ),
            // A8：在文件中查找模式开关（F12 同义入口）
            button(text(if self.fif_visible {
                self.t(editpad_core::Key::FindInFilesExit)
            } else {
                self.t(editpad_core::Key::FindInFilesMode)
            })
            .size(uipx)
            .font(uifont))
            .style(move |theme, status| find_button_style(theme, status, dim))
            .on_press(Message::FindInFilesToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut card_body = column![grip, query_row, options_row, third_row, panel_row]
            .spacing(8)
            .width(Fill);
        // ⑤结果面板：查找全部 / 目录命中（同槽互斥，面板自身限高滚动）
        if self.find_all_visible && !self.fif_visible {
            card_body = card_body
                .push(rule::horizontal(1))
                .push(self.find_all_panel(uipx, uifont));
        }
        if self.fif_visible {
            card_body = card_body
                .push(rule::horizontal(1))
                .push(self.find_in_files_panel(uipx, uifont));
        }

        // 定宽 + 小窗钳制；高度按窗口钳制（结果面板内部滚动承接溢出）
        let card_w = FIND_CARD_W.min((self.viewport_size.0 - 32.0).max(320.0));
        // P153：点过正文（find_dimmed）后整框转半透明——背板换淡出色，
        // 卡片内文字/控件沿用原样式按同一 alpha 淡出。
        let card_style = if self.find_dimmed {
            popup_card_dim_style
        } else {
            popup_card_style
        };
        let card = opaque(
            container(card_body)
                .width(card_w)
                .max_height((self.viewport_size.1 - 48.0).max(160.0))
                .padding(10)
                .style(card_style),
        );
        // P153：卡片（含拖动条、空白处，**以及输入框/按钮本身**）接住左键
        // 按下 → 半透明态复位。必须用 [`crate::press_observer::PressObserver`]
        // 而非 `mouse_area`：输入框/按钮会捕获按下事件，`mouse_area` 在内容
        // 已捕获时不再自查（上游 `MouseArea::update` 的 `is_event_captured`
        // 早退），导致「点半透明框里的输入框不恢复」（用户复报）。
        // 观察层同时**消费**该按下——否则会继续下传给正文自绘控件，正文
        // 重新持焦并请求 IME，拼音组字串在查找框与正文各画一份（P151 症状
        // 复现，用户复报「旧 bug 又回来了」）。
        let card =
            PressObserver::new(card, Message::FindBoxPressed).into_element();
        // 位置由应用层持有（用户点单：浮层遮住目标行要能拖开）：
        // 默认 = 窗口中间偏上（名义高度参与居中），拖动后 = 钳制过的左上角。
        // 整层挂 `on_move`/`on_release` 追踪拖动：该层**不设 on_press**，
        // 点击照旧穿透到正文；`ButtonReleased` 上游不捕获（只发消息），
        // 所以正文拖拽/滚动条拖动不受影响。
        let pos = self.find_pos.unwrap_or_else(|| self.default_find_pos());
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: pos.y, right: 0.0, bottom: 0.0, left: pos.x }),
        )
        .on_move(Message::FindCursorMoved)
        .on_release(Message::FindDragEnd)
        .into()
    }
    /// 查找浮层默认位置（窗口中间偏上；卡片高度按名义值 [`FIND_CARD_NOMINAL_H`]
    /// 参与居中——真实高度随结果面板开合变化，取名义值即可，且便于拖动钳制）。
    pub(crate) fn default_find_pos(&self) -> Point {
        let (w, h) = self.viewport_size;
        let card_w = FIND_CARD_W.min((w - 32.0).max(320.0));
        Point::new(
            ((w - card_w) * 0.5).max(8.0),
            ((h - FIND_CARD_NOMINAL_H) * 0.5).max(8.0),
        )
    }
    /// 拖动钳制：左上角留在窗口内（右下按名义尺寸留边，拖不出视野）。
    pub(crate) fn clamp_find_pos(&self, p: Point) -> Point {
        let (w, h) = self.viewport_size;
        let card_w = FIND_CARD_W.min((w - 32.0).max(320.0));
        Point::new(
            p.x.clamp(8.0, (w - card_w - 8.0).max(8.0)),
            p.y.clamp(8.0, (h - FIND_CARD_NOMINAL_H - 8.0).max(8.0)),
        )
    }
}
