//! P312（路线图 C11）应用层单测：Alt+←/→ 键位通道与只读守卫。
//!
//! 历史栈本身的行为在 `editor::nav_tests` 钉；这里钉两件应用层才有的事：
//! ①键位分发**接管了 Alt 组合但不吃掉 AltGr**（Windows 上 AltGr ＝ Ctrl+Alt，
//! 热键契约因此一律拒绝含 Alt 的可重映射组合，见 `combo_string` 文档）；
//! ②两条动作在只读页放行（白名单登记）。

use super::*;
use crate::editor::CursorPos;
use iced::keyboard::Modifiers;

fn key(named: Named) -> keyboard::Key {
    keyboard::Key::Named(named)
}

#[test]
fn alt_arrows_dispatch_the_navigation_history() {
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowLeft), Modifiers::ALT),
        Some(Message::Edit(EditOp::NavBack))
    ));
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowRight), Modifiers::ALT),
        Some(Message::Edit(EditOp::NavForward))
    ));
}

#[test]
fn altgr_and_plain_and_ctrl_shapes_are_not_hijacked() {
    // AltGr（Ctrl+Alt）：热键契约要求放行成"普通左移"，绝不变成跳回
    let altgr = Modifiers::CTRL | Modifiers::ALT;
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowLeft), altgr),
        Some(Message::Edit(EditOp::Motion(Motion::Left, false)))
    ));
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowRight), altgr),
        Some(Message::Edit(EditOp::Motion(Motion::Right, false)))
    ));
    // 裸方向键 / Shift+方向键 原样
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowLeft), Modifiers::empty()),
        Some(Message::Edit(EditOp::Motion(Motion::Left, false)))
    ));
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowRight), Modifiers::SHIFT),
        Some(Message::Edit(EditOp::Motion(Motion::Right, true)))
    ));
    // Ctrl+← 仍走注册表里的词左移（P122）
    assert!(matches!(
        handle_key_defaults(key(Named::ArrowLeft), Modifiers::CTRL),
        Some(Message::Edit(EditOp::Motion(Motion::WordLeft, false)))
    ));
}

#[test]
fn nav_ops_are_pure_navigation_for_the_read_only_gate() {
    use crate::update::edit_op_mutates as m;
    assert!(!m(&EditOp::NavBack));
    assert!(!m(&EditOp::NavForward));
}

#[test]
fn goto_then_alt_back_end_to_end() {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("aaa\nbbb\nccc".to_owned())),
    );
    let dirty_before = app.tab().dirty;
    // 插入完光标在文末（第 3 行行尾）——先送回文档首，否则"跳走"与
    // "出发点"同一行，这条用例就成了空转
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocStart, false)),
    );
    assert_eq!(
        app.cur_handle.borrow().cursor.line,
        0,
        "用例前提：从首行出发"
    );
    dispatch(&mut app, Message::GotoToggled);
    dispatch(&mut app, Message::GotoInputChanged("3".to_owned()));
    dispatch(&mut app, Message::GotoSubmit);
    assert_eq!(app.cur_handle.borrow().cursor.line, 2, "用例前提：已跳走");
    dispatch(&mut app, Message::Edit(EditOp::NavBack));
    assert_eq!(
        app.cur_handle.borrow().cursor.line,
        0,
        "Alt+← 该回到跳走之前的那一行"
    );
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "aaa\nbbb\nccc");
    assert_eq!(app.tab().dirty, dirty_before, "跳转不改内容、不额外置脏");
    dispatch(&mut app, Message::Edit(EditOp::NavForward));
    assert_eq!(app.cur_handle.borrow().cursor.line, 2);
}

#[test]
fn empty_history_gives_a_status_hint_instead_of_silence() {
    // 新键位不能靠猜：无历史可退时留一句话在状态栏
    let mut app = Editpad::default();
    dispatch(&mut app, Message::Edit(EditOp::NavBack));
    assert!(!app.status.is_empty(), "空栈该有提示");
    assert_eq!(app.cur_handle.borrow().cursor, CursorPos::default());
}
