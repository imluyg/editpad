//! 标签右键菜单面板的渲染。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;
use iced::widget::column;

impl Editpad {
    /// P28 标签右键菜单面板：固定/保存/另存为(重命名)/关闭/关闭其他/
    /// 关闭右侧。菜单项按页面状态禁用（busy、干净页的保存、固定页的
    /// 关闭、无可关目标的批量项）；调用方保证 idx < tabs.len()。
    pub(super) fn tab_context_panel(&self, idx: usize) -> Element<'_, Message> {
        let tab = &self.tabs[idx];
        let interactive = !self.busy;
        // P33/P36：UI 与正文同族，字号固定不随正文缩放；P34：族随设置
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();

        let mut panel = column![
            row![
                text(editpad_core::fmt_wrapped(
                    self.lang(),
                    editpad_core::Key::TabHeaderPrefix,
                    &tab.base_name_in(self.lang()),
                    editpad_core::Key::TabHeaderSuffix,
                ))
                    .size(uipx)
                    .font(uifont)
                    .color([0.5, 0.5, 0.5]),
                button(text("×").size(uipx).font(uifont))
                    .padding([2, 8])
                    .style(chrome_button_style)
                    .on_press(Message::TabContextMenuClosed),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(2)
        .padding([4, 10]);

        // 固定 / 取消固定（固定页豁免一切关闭路径）。
        // P56：菜单项一律中性列表样式（透明底+悬停淡染），不再用 iced
        // 默认实心蓝（用户截图：菜单项像全选中的高亮条）
        let pin_label = if tab.pinned {
            self.t(editpad_core::Key::TabUnpin)
        } else {
            self.t(editpad_core::Key::TabPin)
        };
        panel = panel.push(
            button(container(text(pin_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(interactive.then_some(Message::TogglePinTab(idx))),
        );
        // 保存：仅置脏可用（与工具栏「保存」同一口径）；
        // 未命名页在 update 层自动落另存为
        panel = panel.push(
            button(
                container(text(self.t(editpad_core::Key::TabSave)).size(uipx).font(uifont))
                    .width(Fill),
            )
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && tab.dirty).then_some(Message::SaveTabFromMenu(idx)),
                ),
        );
        // 另存为 / 重命名（v1 同一动作兜底，§3 P28 第 2 条）
        let rename_label = if tab.path.is_some() {
            self.t(editpad_core::Key::TabRename)
        } else {
            self.t(editpad_core::Key::TabSaveAs)
        };
        panel = panel.push(
            button(container(text(rename_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(interactive.then_some(Message::RenameOrSaveAsTab(idx))),
        );
        // 第 63 轮：复制完整路径 / 文件名（未命名页无路径，菜单项灰掉）
        let named = tab.path.is_some();
        panel = panel.push(
            button(
                container(text(self.t(editpad_core::Key::TabCopyPath)).size(uipx).font(uifont))
                    .width(Fill),
            )
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && named).then_some(Message::CopyFilePath(Some(idx))),
                ),
        );
        panel = panel.push(
            button(
                container(text(self.t(editpad_core::Key::TabCopyName)).size(uipx).font(uifont))
                    .width(Fill),
            )
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && named).then_some(Message::CopyFileName(Some(idx))),
                ),
        );

        panel = panel.push(rule::horizontal(1));

        // 关闭：固定页拒绝（update 层守卫 + 菜单项灰掉双保险）
        panel = panel.push(
            button(
                container(text(self.t(editpad_core::Key::TabClose)).size(uipx).font(uifont))
                    .width(Fill),
            )
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && !tab.pinned).then_some(Message::CloseTabAt(idx)),
                ),
        );
        // 关闭其他 / 关闭右侧：无可关目标（全部是固定页或没有其他页）时禁用
        let others = batch_close_targets(&self.tabs, BatchCloseScope::Others(idx));
        let right = batch_close_targets(&self.tabs, BatchCloseScope::RightOf(idx));
        panel = panel.push(
            button(
                container(text(format!(
                    "{}({})",
                    self.t(editpad_core::Key::TabCloseOthers),
                    others.len()
                ))
                    .size(uipx)
                    .font(uifont))
                    .width(Fill),
            )
            .width(Fill)
            .style(chrome_menu_item_style)
            .on_press_maybe(
                (!others.is_empty() && interactive).then_some(Message::CloseOtherTabs(idx)),
            ),
        );
        panel = panel.push(
            button(
                container(text(format!(
                    "{}({})",
                    self.t(editpad_core::Key::TabCloseRight),
                    right.len()
                ))
                    .size(uipx)
                    .font(uifont))
                    .width(Fill),
            )
            .width(Fill)
            .style(chrome_menu_item_style)
            .on_press_maybe(
                (!right.is_empty() && interactive).then_some(Message::CloseTabsRight(idx)),
            ),
        );
        panel.into()
    }

}
