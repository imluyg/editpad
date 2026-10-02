//! P308：正文右键菜单，以及 P307 连击选择在**真实视图树 + 真实事件流**上的
//! 接线核对。
//!
//! core 层的行为已在 `editor::click_tests` 钉住（20 条）；这里只补那条覆盖
//! 不到的东西：控件层按下分支的顺序（连击裁决先于拖拽候选）与右键入口。
//! 判据刻意取「选中了整词」而非「发过某条消息」——分支顺序错一次，
//! 双击就会被拖拽候选吃掉，本文件即红。
//! B10 二期（Alt+双击带选区）同样只在这里核对接线：core 层盯算式，这里盯
//! 修饰键状态真能穿过按下分支。

use super::*;

/// 正文里按生产入口写入一段文本（不直接改 doc，走应用层同一条链路）。
fn type_into(app: &mut Editpad, text: &str) {
    dispatch(
        app,
        Message::Edit(crate::editor::EditOp::InsertText(text.to_owned())),
    );
}

#[test]
fn headless_single_click_selects_nothing() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.click(at);
    }
    assert_eq!(app.cur_handle.borrow().anchor, None, "单击仍是定位光标");
    assert_eq!(app.cur_handle.borrow().selected_text(), None);
}

#[test]
fn headless_double_click_in_body_selects_a_whole_word() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let msgs = ui.double_click(at);
        assert!(
            msgs.iter().any(|m| matches!(m, Message::EditorNavChanged)),
            "双击应发导航变更消息（状态栏与查找联动）；实收 {msgs:?}"
        );
    }
    let sel = app.cur_handle.borrow().selected_text();
    assert!(
        matches!(sel.as_deref(), Some("alpha") | Some("beta")),
        "双击应选中整词，实得 {sel:?}"
    );
}

#[test]
fn headless_triple_click_in_body_selects_the_line() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta\ngamma");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.click(at);
        let _ = ui.click(at);
        let _ = ui.click(at);
    }
    let sel = app.cur_handle.borrow().selected_text();
    assert_eq!(sel.as_deref(), Some("alpha beta\n"), "三击 = 整行含换行");
}

#[test]
fn headless_right_click_in_body_opens_menu_at_pointer() {
    let app = Editpad::default();
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.4, body.y + 20.0)
    };
    let msgs = {
        let mut ui = ViewTree::layout_default(&app);
        ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Right,
            )),
            at,
        )
    };
    assert!(
        msgs.iter()
            .any(|m| matches!(m, Message::EditorContextMenu(x, y) if (*x, *y) == (at.x, at.y))),
        "右键应按指针位置弹正文菜单；实收 {msgs:?}"
    );
}

#[test]
fn right_click_outside_body_opens_nothing() {
    let app = Editpad::default();
    let msgs = {
        let mut ui = ViewTree::layout_default(&app);
        ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Right,
            )),
            Point::new(4.0, 4.0),
        )
    };
    assert!(
        !msgs
            .iter()
            .any(|m| matches!(m, Message::EditorContextMenu(..))),
        "菜单栏条带上的右键不该被正文菜单接管；实收 {msgs:?}"
    );
}

#[test]
fn opening_the_menu_is_pure_navigation() {
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    let dirty_before = app.tab().dirty;
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    assert!(app.editor_context_menu, "菜单已开");
    assert_eq!(app.menu_anchor, (300.0, 240.0));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "one\ntwo\n");
    assert_eq!(
        app.tab().dirty,
        dirty_before,
        "开菜单本身不改内容、不额外置脏"
    );
}

#[test]
fn editor_and_tab_context_menus_are_mutually_exclusive() {
    let mut app = Editpad::default();
    app.tab_context_menu = Some(0);
    dispatch(&mut app, Message::EditorContextMenu(10.0, 20.0));
    assert_eq!(app.tab_context_menu, None, "两层背板会互相吞事件");
    dispatch(&mut app, Message::TabContextMenu(0));
    assert!(!app.editor_context_menu, "反向同理");
}

#[test]
fn menu_item_runs_the_action_and_closes() {
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    // 插入后光标在文末（幻影末行），先把要删的那一行变成首行
    dispatch(&mut app, Message::Edit(crate::editor::EditOp::SelectAll));
    dispatch(
        &mut app,
        Message::Edit(crate::editor::EditOp::Motion(
            crate::editor::Motion::DocStart,
            false,
        )),
    );
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::DeleteLines))),
    );
    assert!(!app.editor_context_menu, "菜单项执行完即收起");
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "two\n");
    assert!(app.tab().dirty, "经菜单删行同样置脏");
}

#[test]
fn menu_is_not_a_new_bypass_for_read_only_pages() {
    let mut app = Editpad::default();
    type_into(&mut app, "keep me\n");
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "用例前提：已切只读");
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::DeleteLines))),
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "keep me\n",
        "只读页仍由 apply_edit 总闸拒收——菜单只是入口，不是放行口"
    );
}

#[test]
fn stray_command_after_close_does_not_reopen_menu() {
    // 「点菜单外」与「点某项」同一帧只会命中其一；这里断的是状态自洽——
    // 转交仍会发生（收起菜单不等于拒收动作），但绝不把菜单再打开
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    dispatch(&mut app, Message::EditorContextMenuClosed);
    assert!(!app.editor_context_menu);
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::SelectAll))),
    );
    assert_eq!(
        app.cur_handle.borrow().selected_text().as_deref(),
        Some("one\ntwo\n"),
        "转交照常执行"
    );
    assert!(!app.editor_context_menu, "执行菜单项不该把菜单再打开");
}

/// B10 二期「选区镜像同步」的**接线**核对：core 层那五格盯的是算式，这一格盯
/// 鼠标事件真能走到 Alt+双击那条分支。变异＝视图恒传 `ExtraTap::Point`
/// ⇒ core 层全绿、只有这里红（第 226 轮那条"注册了但点不动"的护栏同形）。
#[test]
fn headless_alt_double_click_adds_extra_cursor_owning_a_selection() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.send(
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
                iced::keyboard::Modifiers::ALT,
            )),
            at,
        );
        // 第一次按下＝点（加一条无选区光标），第二次＝双击（该词连选区进来）
        let msgs = ui.double_click(at);
        assert!(
            msgs.iter().any(|m| matches!(m, Message::EditorNavChanged)),
            "Alt+双击应发导航变更消息（状态栏与查找联动）；实收 {msgs:?}"
        );
    }
    let core = app.cur_handle.borrow();
    assert_eq!(core.extra_cursors.len(), 1, "Alt+双击后应恰有一条附加光标");
    assert!(
        core.extra_cursors[0].anchor.is_some(),
        "附加光标必须持有选区——这正是本轮补的那一格"
    );
    assert_eq!(core.doc.to_text(), "alpha beta", "手势本身一字不改文档");
}
