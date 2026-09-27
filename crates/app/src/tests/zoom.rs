//! P313：键盘字号缩放（`Ctrl+=` / `Ctrl+-` / `Ctrl+Shift+0`）。
//!
//! 这条改动真正贵的不是三个动作，而是**热键语法放宽了一格**（`+ - =` 从
//! 「不可作热键的符号」变成规范名 `Plus/Minus/Equal`）。所以两头都要钉：
//! 符号语法不能悄悄变宽（其余符号必须仍被拒），三条默认键位必须真能分发到
//! 与「查看」菜单同一个消息（两处各写一份增量，早晚各说一套）。

use super::*;
use iced::keyboard::Modifiers;

fn ch(s: &str) -> keyboard::Key {
    keyboard::Key::Character(s.into())
}

#[test]
fn combo_grammar_opens_only_those_three_symbols() {
    use crate::hotkeys::combo_string;
    assert_eq!(
        combo_string(Modifiers::CTRL, &ch("=")),
        Some("Ctrl+Equal".to_owned())
    );
    assert_eq!(
        combo_string(Modifiers::CTRL | Modifiers::SHIFT, &ch("+")),
        Some("Ctrl+Shift+Plus".to_owned()),
        "主键盘的 `+` 要按 Shift 才出 ⇒ 与小键盘的 `+` 归到同一个规范名"
    );
    assert_eq!(
        combo_string(Modifiers::CTRL, &ch("-")),
        Some("Ctrl+Minus".to_owned())
    );
    // 其余符号照旧不可作热键（放行一个就多吞一次输入）
    for bad in ["%", ";", "/", "'", "."] {
        assert_eq!(
            combo_string(Modifiers::CTRL, &ch(bad)),
            None,
            "{bad} 不该入语法"
        );
    }
    // 无 Ctrl 的符号仍然不参与（打字正文）
    assert_eq!(combo_string(Modifiers::empty(), &ch("=")), None);
}

#[test]
fn zoom_defaults_dispatch_to_the_same_messages_as_the_menu() {
    let step = crate::editor::FONT_ZOOM_STEP;
    assert!(matches!(
        handle_key_defaults(ch("="), Modifiers::CTRL),
        Some(Message::FontSizeDelta(d)) if (d - step).abs() < f32::EPSILON
    ));
    assert!(matches!(
        handle_key_defaults(ch("+"), Modifiers::CTRL | Modifiers::SHIFT),
        Some(Message::FontSizeDelta(d)) if (d - step).abs() < f32::EPSILON
    ));
    assert!(matches!(
        handle_key_defaults(ch("-"), Modifiers::CTRL),
        Some(Message::FontSizeDelta(d)) if (d + step).abs() < f32::EPSILON
    ));
    assert!(matches!(
        handle_key_defaults(ch("0"), Modifiers::CTRL | Modifiers::SHIFT),
        Some(Message::ZoomResetDefault)
    ));
    // 裸 Ctrl+0 仍是「MD5」，复位没有把它抢走
    assert!(matches!(
        handle_key_defaults(ch("0"), Modifiers::CTRL),
        Some(Message::Edit(EditOp::ApplyTool(
            crate::editor::ToolKind::ToolMd5
        )))
    ));
}

#[test]
fn zoom_reset_returns_the_global_default_size() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FontSizeDelta(8.0));
    let raised = app.cur_handle.borrow().font_size();
    assert!(raised > 16.0, "用例前提：已放大到 {raised}");
    dispatch(&mut app, Message::ZoomResetDefault);
    let back = app.cur_handle.borrow().font_size();
    assert!(
        (back - 16.0).abs() < f32::EPSILON,
        "复位应回到规范默认 16，实得 {back}"
    );
    assert!(
        (app.settings.font_size - 16.0).abs() < f32::EPSILON,
        "复位改的是全局默认（与放大同一条链路），不只本页"
    );
}

#[test]
fn zoom_reset_on_a_tab_with_override_leaves_the_override_alone() {
    // 每页覆盖（P134 C7）不该被全局复位抹掉——那是用户按 Ctrl+滚轮 单独调的
    let mut app = Editpad::default();
    dispatch(&mut app, Message::TabFontSizeDelta(4.0));
    let overridden = app.tab().font_size_override;
    assert!(overridden.is_some(), "用例前提：本页有字号覆盖");
    dispatch(&mut app, Message::FontSizeDelta(8.0));
    dispatch(&mut app, Message::ZoomResetDefault);
    assert_eq!(app.tab().font_size_override, overridden, "覆盖必须原样留着");
}
