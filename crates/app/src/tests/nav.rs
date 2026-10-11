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

/// 「转到行」框开栏后键盘输入不许落进正文＝P151 那一族（查找框／命令面板／就地
/// 重命名都接了那把钥匙）剩下的第四个框。判据落在正文控件那一侧：开栏 ⇒ 正文交出
/// 输入焦点，于是预编辑串与上屏文本都不再被正文消费；三条关栏出路（提交成功／
/// 再按一次收起／Esc）各自把焦点还回正文。
#[test]
fn goto_bar_takes_keyboard_focus_from_the_document() {
    use crate::editor::ImeCommit;
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("aaa\nbbb\nccc".to_owned())),
    );
    let doc_before = app.cur_handle.borrow().doc.to_text();
    // 夹具自证：焦点原本在正文，否则下面那句「开栏后不 focused」是空断言
    assert!(app.cur_handle.borrow().focused, "用例前提：焦点原本在正文");

    // 开栏：正文让出焦点，IME 两半都不再被正文消费
    dispatch(&mut app, Message::GotoToggled);
    assert!(app.goto_visible, "用例前提：跳转栏已开");
    assert!(
        !app.cur_handle.borrow().focused,
        "跳转框接管后正文不得再持有输入焦点"
    );
    assert!(
        !app.cur_handle.borrow_mut().ime_preedit("1".to_owned()),
        "正文不得消费预编辑串（否则框与正文各画一份）"
    );
    assert_eq!(
        app.cur_handle.borrow_mut().ime_commit("120"),
        ImeCommit::Ignored,
        "正文不得吃上屏文本"
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        doc_before,
        "「120」绝不能落进正文"
    );

    // 框里的内容归框，同样不写正文
    dispatch(&mut app, Message::GotoInputChanged("2".to_owned()));
    assert_eq!(app.goto_input, "2", "用例前提：输入落在框里");
    assert_eq!(app.cur_handle.borrow().doc.to_text(), doc_before);

    // 出路①提交成功（跳完自动关栏）→ 焦点回正文
    dispatch(&mut app, Message::GotoSubmit);
    assert!(!app.goto_visible, "用例前提：提交后关栏");
    assert!(
        app.cur_handle.borrow().focused,
        "提交关栏后正文应重新持有焦点"
    );

    // 出路②再按一次收起（× 走同一条消息）→ 焦点回正文
    dispatch(&mut app, Message::GotoToggled);
    assert!(!app.cur_handle.borrow().focused, "用例前提：开栏交出焦点");
    dispatch(&mut app, Message::GotoToggled);
    assert!(app.cur_handle.borrow().focused, "关栏后正文应重新持有焦点");

    // 出路③ Esc（与查找栏共用那条收起出口，它自己就还焦点）
    dispatch(&mut app, Message::GotoToggled);
    assert!(!app.cur_handle.borrow().focused, "用例前提：开栏交出焦点");
    dispatch(&mut app, Message::BarsDismissed);
    assert!(!app.goto_visible, "用例前提：Esc 收起跳转栏");
    assert!(
        app.cur_handle.borrow().focused,
        "Esc 收起后正文应重新持有焦点"
    );

    // 反面对照：无效输入既不关栏也不还焦点（框还在等用户改），也不许写正文
    dispatch(&mut app, Message::GotoToggled);
    dispatch(&mut app, Message::GotoInputChanged("abc".to_owned()));
    dispatch(&mut app, Message::GotoSubmit);
    assert!(app.goto_visible, "无效输入不该关栏");
    assert!(
        !app.cur_handle.borrow().focused,
        "栏还在场上 ⇒ 焦点不该回正文"
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        doc_before,
        "无效提交同样不写正文"
    );
}

// ---------- 【测量轮】已聚焦输入框在**生产视图树里**的按键归属 ----------

/// 一条「浮层输入框拿到焦点之后，三种键各自归谁」的探针描述。
struct KeyRoute {
    /// 失败信息里的名字
    label: &'static str,
    /// 造一个"这个浮层本来就开得起来"的应用
    make: fn() -> Editpad,
    /// 用**生产消息**把浮层开起来
    open: fn(&mut Editpad),
    /// 浮层在场判据（读各浮层自己的标记）
    is_open: fn(&Editpad) -> bool,
    /// 该框的组件 Id（与 view 侧 `.id(..)` 同源的那一个）
    id: fn() -> iced::advanced::widget::Id,
    /// 该框 `on_input` 那条消息的形状。判据统一取「载荷以 `a` 收尾」而不是
    /// 等于 `"a"`：就地重命名那一个框**预填旧文件名**，插进去是「note.txta」——
    /// 第一版我按 `== "a"` 写，红的是自己的期望而不是代码。
    is_changed: fn(&Message) -> bool,
    /// 该框 `on_submit` 那条消息的形状
    is_submit: fn(&Message) -> bool,
}

/// 造一次"打出字符 `c`"的按键事件。`text` 是 iced 用来决定输入框要不要插入的
/// 那一份（命名键那一路必须是 `None`），`modified_key` 与 `key` 取同值＝无修饰。
fn pressed_char(c: &str, code: iced::keyboard::key::Code) -> iced::Event {
    let key = iced::keyboard::Key::Character(c.into());
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Code(code),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::empty(),
        text: Some(c.into()),
        repeat: false,
    })
}

/// 造一次命名键（Enter / Escape）的按键事件：`text` 恒为 `None`。
fn pressed_named(named: Named, code: iced::keyboard::key::Code) -> iced::Event {
    let key = iced::keyboard::Key::Named(named);
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Code(code),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    })
}

/// 已经接了 P151 那把钥匙的三个框，在**真实视图树**里把键吃掉给谁——读数不推测。
///
/// 为什么单独立这一条：本族既有用例全部按 `Message::KeyPressed` 或业务消息
/// **直接派发**（例如上面那条 `goto_bar_takes_keyboard_focus_from_the_document`
/// 的出路③就是 `dispatch(Message::BarsDismissed)`），**绕过了控件树**。于是
/// 「框真拿到焦点之后 iced 怎么处理键」这一层从来没有读数，而它恰恰是本族的地基：
///
/// * 上游 `iced_widget-0.14.2/src/text_input.rs:910`——整个 `KeyPressed` 支被
///   `if let Some(focus) = &mut state.is_focused` 罩住＝只有**聚焦中的那个框**处理键；
/// * `:1015-1036` 字符键插入后 `capture_event()`（＝打字进框、不落正文）；
/// * `:1051-1055` Enter **只在设了 `on_submit` 时** publish ＋ capture；
/// * `:1235-1243` **Escape 无条件** `is_focused = None` ＋ capture。
///
/// 而本应用把按键交给 `Editpad::update` 的唯一通道只收 `Status::Ignored`
/// （`update/mod.rs:1129-1133`），Esc 关栏因此只能经 `hotkeys.rs:913` 的
/// `Escape ⇒ BarsDismissed`。⇒ 读码推断「框聚焦时**第一下 Esc 到不了应用层**＝
/// 要按两下才关浮层」，在这一轮之前**只是推断**，且四个已上线框全都带着它跑。
///
/// 读数口径＝`Shell::event_status()`（0.14 的 `Widget::update` **不返回**状态，
/// 运行时遍历完整棵树后读的就是这个字段）——与生产同源，不是自造代理。
#[test]
fn focused_overlay_input_owns_its_keys_in_the_production_tree() {
    use iced::keyboard::key::Code;
    let cases = [
        KeyRoute {
            label: "查找框",
            make: Editpad::default,
            open: |app| dispatch(app, Message::FindToggled),
            is_open: |app| app.find_visible,
            id: crate::view::find_input_widget_id,
            is_changed: |m| matches!(m, Message::FindQueryChanged(v) if v.ends_with('a')),
            is_submit: |m| matches!(m, Message::FindNext),
        },
        KeyRoute {
            label: "转到行框",
            make: Editpad::default,
            open: |app| dispatch(app, Message::GotoToggled),
            is_open: |app| app.goto_visible,
            id: crate::view::goto_input_widget_id,
            is_changed: |m| matches!(m, Message::GotoInputChanged(v) if v.ends_with('a')),
            is_submit: |m| matches!(m, Message::GotoSubmit),
        },
        KeyRoute {
            label: "命令面板框",
            make: Editpad::default,
            open: |app| dispatch(app, Message::PaletteToggled(PaletteMode::Commands)),
            is_open: |app| app.palette_visible,
            id: crate::palette_input_id,
            is_changed: |m| matches!(m, Message::PaletteInputChanged(v) if v.ends_with('a')),
            is_submit: |m| matches!(m, Message::PaletteExecute),
        },
        KeyRoute {
            label: "就地重命名框",
            make: loaded_txt_app,
            open: |app| dispatch(app, Message::RenameOrSaveAsTab(0)),
            is_open: |app| app.renaming_tab.is_some(),
            id: crate::rename_input_id,
            is_changed: |m| matches!(m, Message::TabRenameInputChanged(v) if v.ends_with('a')),
            is_submit: |m| matches!(m, Message::TabRenameCommitted),
        },
    ];
    let at = iced::Point::new(640.0, 400.0);

    for case in cases {
        let mut app = (case.make)();
        (case.open)(&mut app);
        // 夹具自证①：浮层真的开着。否则下面跑的是"树上根本没有这个框"的树，
        // 三条读数会一起给出"什么都没被吃"的假绿。
        assert!(
            (case.is_open)(&app),
            "{}：用例前提——生产消息没能把浮层开起来",
            case.label
        );

        let mut ui = ViewTree::layout_default(&app);
        // 与生产同一条聚焦操作（`focus_text_field` 里那句 `operation::focus`），
        // 打在**同一棵树**上——`layout()` 每次新建 Tree，换树＝焦点归零。
        ui.focus_widget((case.id)());

        // ①字符键：必须被框吃掉并产出该框的 on_input（＝焦点真落在这个框、
        //   事件真走得到它）。这一条不通，后面两格一律不作数。
        let (changed, char_status) = ui.send_status(pressed_char("a", Code::KeyA), at);
        assert!(
            char_status == iced::event::Status::Captured,
            "{}：聚焦后字符键没被输入框捕获（状态 {char_status:?}）⇒ 焦点根本没落到框里",
            case.label
        );
        assert!(
            changed.iter().any(case.is_changed),
            "{}：字符键该产出该框的 on_input 消息且载荷为「a」，实得 {changed:?}",
            case.label
        );

        // ②Enter：四个框都设了 `.on_submit`，所以确认/执行**由控件产出**。
        let (submitted, enter_status) =
            ui.send_status(pressed_named(Named::Enter, Code::Enter), at);
        assert!(
            enter_status == iced::event::Status::Captured,
            "{}：聚焦后 Enter 没被捕获，实得 {enter_status:?}",
            case.label
        );
        assert!(
            submitted.iter().any(case.is_submit),
            "{}：Enter 该由控件产出 on_submit 那条消息，实得 {submitted:?}",
            case.label
        );

        // ③Escape：本条就是那句"两下才关浮层"的**读数**。控件吃掉它（Blur）且不
        //   产出任何消息 ⇒ 应用层永远看不见这一键 ⇒ `hotkeys.rs:913` 的
        //   `Escape ⇒ BarsDismissed` 只能等第二下。
        let (escaped, esc_status) = ui.send_status(pressed_named(Named::Escape, Code::Escape), at);
        assert!(
            esc_status == iced::event::Status::Captured,
            "{}：Escape 没被输入框捕获，实得 {esc_status:?}",
            case.label
        );
        assert!(
            escaped.is_empty(),
            "{}：Escape 被输入框吃掉后不该再产出任何消息，实得 {escaped:?}",
            case.label
        );
        // ⚠️ 这一格断的是**当前事实**，不是"应当如此"：浮层还开着（app 侧标记未变），
        // 因为这一键压根没进应用。它把"两下 Esc"这个后果指名道姓写出来，将来若要
        // 让第一下就关栏（改上游捕获或换控件），这里会红得有意义。
        assert!(
            (case.is_open)(&app),
            "{}：第一下 Escape 之后浮层仍该在场（这一键没进应用层）",
            case.label
        );
    }
}
