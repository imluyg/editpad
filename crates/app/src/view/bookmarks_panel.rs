//! P318：书签列表面板——「有圆点、能跳转，但看不到全貌」的那一块。
//!
//! 结构照「查找全部」结果面板（`find_panel.rs`）：标题行 + 规则线 + 可滚动
//! 行列表。差别是它**不依附查找栏**，是独立一层的非模态卡片：点正文仍可继续
//! 编辑，Esc 或标题行的「×」收起。
//!
//! 每行两个动作：行本身＝跳过去（`BookmarkGoto`），行尾「×」＝只摘这一条
//! （`BookmarkRemoveAt`）。标题行的「清除全部」直接复用既有
//! `EditOp::BookmarksClearAll` ⇒ 只读判定、撤销、状态栏都留在原处一份实现。

use super::*;
use iced::widget::column;

/// 一帧最多渲染多少条书签行。与 `FIND_ALL_MAX_ROWS` 同一条理由：chrome 行
/// 按钮没有虚拟化，万级书签全量建出来会拖垮这一帧。
const BOOKMARK_PANEL_MAX_ROWS: usize = 200;

impl Editpad {
    /// 面板的数据源：升序、剔除悬空行号（构建视图与测试共用这一份口径）。
    pub(crate) fn bookmark_panel_lines(&self) -> Vec<usize> {
        self.cur_handle.borrow().bookmarked_lines_in_doc()
    }

    pub(super) fn bookmarks_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let lines = self.bookmark_panel_lines();
        let total = lines.len();
        let shown = total.min(BOOKMARK_PANEL_MAX_ROWS);
        let header = row![
            text(if total == 0 {
                self.t(editpad_core::Key::StNoBookmarks).to_owned()
            } else {
                editpad_core::fmt_bookmark_total(self.lang(), total)
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(
                text(self.t(editpad_core::Key::HkBookmarkClearAll).to_owned())
                    .size(uipx)
                    .font(uifont),
            )
            .style(chrome_button_style)
            .on_press(Message::Edit(EditOp::BookmarksClearAll)),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::BookmarksToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if shown > 0 {
            // 一次借完整个循环（与 find_all_panel 同款）：逐行各借一次会让
            // 行内取文本的调用点散成一堆，且没有换来任何东西
            let editor = self.cur_handle.borrow();
            for &line in lines.iter().take(shown) {
                let raw = editor.line_text(line);
                let excerpt = match_excerpt(&raw, 0, FIND_ALL_EXCERPT_COLS);
                rows = rows.push(
                    container(
                        row![
                            button(
                                text(format!("{:>5}  {}", line + 1, excerpt))
                                    .size(uipx)
                                    .font(uifont)
                                    .width(Fill),
                            )
                            .width(Fill)
                            .padding([3, 10])
                            .style(chrome_menu_item_style)
                            .on_press(Message::BookmarkGoto(line)),
                            button(text("×").size(uipx).font(uifont))
                                .style(chrome_button_style)
                                .on_press(Message::BookmarkRemoveAt(line)),
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    )
                    .width(Fill)
                    .padding([0, 10]),
                );
            }
        }
        if total > shown {
            rows = rows.push(
                text(format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::StBookmarksMorePrefix),
                    total - shown,
                    self.t(editpad_core::Key::StBookmarksMoreSuffix),
                ))
                .size(uipx)
                .font(uifont),
            );
        }
        // 高度按窗口钳制：小窗不溢出、大窗不占满（查找卡片同策略）。
        // 外层容器只负责摆位，卡片本身 opaque ⇒ 卡片外的点击照旧落到正文。
        let h = (self.viewport_size.1 * 0.4).clamp(140.0, 360.0);
        let card = opaque(
            container(column![header, rule::horizontal(1), scrollable(rows)].spacing(0))
                .width(560)
                .height(h)
                .style(popup_card_style),
        );
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
            })
            .into()
    }
}
