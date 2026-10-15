//! P449（路线图 B11）：打字自动补齐括号——**接线**这一档。
//!
//! 判据那张表（什么时候补、什么时候越过、什么时候整对删）住在
//! `editpad_core::pairing`，它自己的用例在 core 那一侧。这里钉的是接线：
//! - 默认关 ⇒ 打 `(` 就是一个 `(`，与引入本功能前逐字节相同（这一格最重要，
//!   它不是"次要路径"，是用户装完新版本没动设置时看到的形状）；
//! - 补完之后光标必须落在两个括号**中间**，且不能把打字那组撤销打碎
//!   （`insert_pair` 分两次单字符插入正是为了这一格）；
//! - 越过与整对删都只认"本功能自己补的那一个"，手打的括号一律原样；
//! - 打字不是跳转：一条跳转历史都不许记（P448 立的规矩）；
//! - 只有真按键那一路会补对——IME 上屏、列编辑器生成的文本、列块逐行插入
//!   那三条生产者仍发 `InsertText`，天生不补。
//!
//! 光标在括号间的闪烁、候选窗位置这类只能开真窗口看的东西记在
//! `docs/手测清单.md`，这里不冒充覆盖。

use super::*;
use crate::editor::{BlockSel, CursorPos, EditOp, Motion};

/// 走**真实按键通道**打一个字符：这样热键那一层的分流（哪些字符发 `TypedPair`）
/// 也被钉住，而不是只测直接投 `EditOp` 的捷径。
fn type_char(app: &mut Editpad, c: char) {
    let msg = handle_key_defaults(
        keyboard::Key::Character(c.to_string().into()),
        iced::keyboard::Modifiers::empty(),
    )
    .expect("打一个可打印字符必须产出一条编辑消息");
    dispatch(app, msg);
}

fn turn_on(app: &mut Editpad) {
    dispatch(app, Message::SettingsAutoPairToggled(true));
    assert!(
        app.settings.auto_pair_brackets,
        "用例前提：开关已开（默认关，所以每格都得自己开）"
    );
}

fn doc(app: &Editpad) -> String {
    app.cur_handle.borrow().doc.to_text()
}

fn caret(app: &Editpad) -> CursorPos {
    let core = app.cur_handle.borrow();
    core.cursor
}

/// 第 1 格：默认关——打 `(` 就是一个 `(`，用户没动设置时看到的正是这个形状。
#[test]
fn the_setting_defaults_off_and_a_typed_bracket_is_then_just_a_bracket() {
    let app = Editpad::default();
    assert!(
        !app.settings.auto_pair_brackets,
        "用例前提：默认必须关（这是用户点单定的默认值，它会写入没按下的字符）"
    );
    let mut app = app;
    type_char(&mut app, '(');
    assert_eq!(doc(&app), "(", "关着却补了右括号＝把行为塞进默认路径");
}

/// 第 2 格：开着之后：补成对，且光标必须停在两者**中间**（停在外面等于没补）。
#[test]
fn a_typed_opener_leaves_the_caret_between_the_pair() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    assert_eq!(doc(&app), "()");
    assert_eq!(
        caret(&app),
        CursorPos { line: 0, col: 1 },
        "光标该在括号中间，实得 {:?}",
        caret(&app)
    );
    // 三种括号各认自己那一个配对（表漏一格就是打 `{"` 时补出 `()`）
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        let mut a = Editpad::default();
        turn_on(&mut a);
        type_char(&mut a, open);
        assert_eq!(doc(&a), format!("{open}{close}"), "{open} 的配对不对");
    }
}

/// 第 3 格：右侧邻位是词字符时不补（中文正文里打括号不该多出一个 `)`）。
#[test]
fn an_opener_next_to_a_word_char_is_left_alone() {
    for tail in ["x", "中", "7", "_"] {
        // 夹具：一行 `«tail»y`，光标插在两者之间 ⇒ 右侧永远是词字符
        let mut app = Editpad::default();
        turn_on(&mut app);
        dispatch(
            &mut app,
            Message::Edit(EditOp::InsertText(format!("{tail}y"))),
        );
        // 列是**字符**单位（"中" 占 3 字节但是 1 列），别拿 len() 当列号
        let col = tail.chars().count();
        app.cur_handle.borrow_mut().cursor = CursorPos { line: 0, col };
        assert_eq!(caret(&app).col, col, "夹具自证：光标确实挨着 {tail:?} 右侧");
        type_char(&mut app, '(');
        assert_eq!(
            doc(&app),
            format!("{tail}(y"),
            "右侧是词字符时不许补，实得 {:?}",
            doc(&app)
        );
        // 正面对照：同一份正文里把光标挪到行尾（右侧为空）就打同样的括号 ⇒ 补
        let mut end = Editpad::default();
        turn_on(&mut end);
        dispatch(&mut end, Message::Edit(EditOp::InsertText(tail.to_owned())));
        type_char(&mut end, '(');
        assert_eq!(
            doc(&end),
            format!("{tail}()"),
            "行尾那一格必须补——否则上面那条红可以是「整套没接上」的假红"
        );
    }
}

/// 第 4 格：紧接着打闭括号＝越过它，不是重复一个（`(()` 那种脏尾巴）。
#[test]
fn typing_the_closer_skips_the_auto_closer_instead_of_doubling_it() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    type_char(&mut app, ')');
    assert_eq!(doc(&app), "()", "多出一个右括号＝跳过那格没接上");
    assert_eq!(caret(&app), CursorPos { line: 0, col: 2 });

    // 嵌套那格：`(` `(` 各补一个 ⇒ 连按两个 `)` 一路越过，不多不少
    let mut deep = Editpad::default();
    turn_on(&mut deep);
    type_char(&mut deep, '(');
    type_char(&mut deep, '(');
    assert_eq!(doc(&deep), "(())");
    assert_eq!(caret(&deep), CursorPos { line: 0, col: 2 });
    type_char(&mut deep, ')');
    assert_eq!(caret(&deep), CursorPos { line: 0, col: 3 }, "第一越");
    type_char(&mut deep, ')');
    assert_eq!(caret(&deep), CursorPos { line: 0, col: 4 }, "第二越");
    assert_eq!(doc(&deep), "(())", "两次越过都不许多吃字符");
    // 越过头了（右侧已无括号）⇒ 第三下就是普通插入
    type_char(&mut deep, ')');
    assert_eq!(doc(&deep), "(()))");
}

/// 第 5 格：反面格：用户手打的 `()` 里按 `)` 是普通插入，不越过——
/// 「跳过」只认本功能补上的那一个。
#[test]
fn a_hand_typed_closer_is_still_inserted() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("()".to_owned())));
    // 光标挪到两括号中间（不是打出来的，所以没有「刚补上」这份状态）
    app.cur_handle.borrow_mut().cursor = CursorPos { line: 0, col: 1 };
    type_char(&mut app, ')');
    assert_eq!(
        doc(&app),
        "())",
        "手打的右括号被越过＝吃掉了用户明确要的那一下"
    );
}

/// 第 6 格：补对必须留在打字那一组里：一次撤销撤掉整段，不碎成几步。
///
/// 这是这一轮唯一"看起来能填平、实际会退化"的一格：一次插 `"()"` 会凭空开
/// 一个新撤销快照，还会把打字组的结束偏移写成 `start+1`（只对单字符成立），
/// 于是后面打的字各占一步。
#[test]
fn a_paired_insert_stays_in_one_undo_group_with_the_typing_around_it() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, 'a');
    type_char(&mut app, '(');
    type_char(&mut app, 'b');
    assert_eq!(doc(&app), "a(b)");
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        doc(&app),
        "",
        "连续打字（含补上去的那两个字符）必须是一次撤销撤干净"
    );
    dispatch(&mut app, Message::Edit(EditOp::Redo));
    assert_eq!(doc(&app), "a(b)", "重做要整步回来");
}

/// 第 7 格：刚补完就退格＝整对删，且一次撤销整步回来。
#[test]
fn backspace_after_an_auto_pair_removes_both_halves() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(doc(&app), "", "整对没删干净，实得 {:?}", doc(&app));
    assert_eq!(caret(&app), CursorPos { line: 0, col: 0 });
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        doc(&app),
        "()",
        "删整对是一步操作，撤销也必须一步回来（两个快照会留下半个）"
    );
}

/// 第 8 格：反面格：手打的 `()` 中间退格只删左括号——右括号是用户自己按的。
#[test]
fn backspace_after_a_hand_typed_pair_removes_only_the_opener() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("()".to_owned())));
    app.cur_handle.borrow_mut().cursor = CursorPos { line: 0, col: 1 };
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(
        doc(&app),
        ")",
        "把用户手打的右括号顺带吃掉＝退格删了没按过的东西"
    );
}

/// 第 9 格：`(x|)` 里退格只删 `x`，再退一次才轮到整对；标记被消耗过就不认第二回。
#[test]
fn backspace_inside_a_pair_with_content_deletes_one_char_at_a_time() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    type_char(&mut app, 'x');
    assert_eq!(doc(&app), "(x)");
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(doc(&app), "()", "内容先走，括号留着");
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(doc(&app), "", "这时才轮到整对");
}

/// 第 10 格：打字不是跳转：一条跳转历史都不许记（P448 立的「一个手势一条记录」）。
#[test]
fn auto_pairing_never_records_a_jump_in_the_history() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, 'a');
    type_char(&mut app, '(');
    type_char(&mut app, ')'); // 这一步是「越过」，最像跳转
    assert!(
        app.cur_handle.borrow().nav_back.is_empty(),
        "补对与越过都在原地写字，不该往跳转历史里塞落点，实得 {:?}",
        app.cur_handle.borrow().nav_back.len()
    );
    assert!(app.cur_handle.borrow().nav_fwd.is_empty());
    // 对照：真跳转照旧入表（否则上一条断言可以是"整套机制没接上"喂绿的）
    dispatch(&mut app, Message::Edit(EditOp::JumpToMatchingBracket));
    assert!(
        !app.cur_handle.borrow().nav_back.is_empty(),
        "对照格：括号配对跳转必须仍往历史里记出发点"
    );
}

/// 第 11 格：从剪贴板进来的那一下不补——粘贴是 `Pasted`，不是逐字符按键。
#[test]
fn a_pasted_bracket_is_not_paired() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    let tab_id = app.tab().id;
    dispatch(&mut app, Message::Pasted(tab_id, "(".to_owned()));
    assert_eq!(doc(&app), "(");
}

/// 第 12 格：另一条生产者同样原样：直接投 `InsertText` 的字符不补。
///（IME 上屏、列编辑器对话框生成的文本、列块逐行插入都走这一条）
#[test]
fn insert_text_from_another_producer_is_not_paired() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("(".to_owned())));
    assert_eq!(
        doc(&app),
        "(",
        "补对只能来自真按键那一路，否则列编辑器生成的文本会被二次改写"
    );
}

/// 第 13 格：列块态打字＝逐行替换整列，一个右括号都不该多出来。
#[test]
fn column_block_typing_inserts_the_bracket_alone() {
    // 两向对照（开关关 vs 开）而不是硬编码期望文本：列块逐行插入的落点算式
    // 属于块那侧的既有语义，我这里只主张"自动配对不许改变它"。
    // ⚠️ 落点必须选在**判据真会补对**的位置（行尾，右侧是行界不是词字符），
    // 否则这条 A/B 会在"抑制根本没起作用"之上假装通过——第一版就把光标放在
    // 'b' 左侧，M1 摘掉全部抑制它照样绿，是假绿。
    let run = |on: bool| {
        let mut app = Editpad::default();
        dispatch(
            &mut app,
            Message::Edit(EditOp::InsertText("ab\ncd\n".to_owned())),
        );
        {
            let mut core = app.cur_handle.borrow_mut();
            core.cursor = CursorPos { line: 0, col: 2 };
            // 跨两行的零宽插入列：`is_empty` 要行列区间都为零才算空，
            // 所以这一份是"非空块"，走的是逐行插入那一条路
            core.block_sel = Some(BlockSel {
                anchor: CursorPos { line: 0, col: 2 },
                head: CursorPos { line: 1, col: 2 },
            });
        }
        if on {
            turn_on(&mut app);
        }
        type_char(&mut app, '(');
        doc(&app)
    };
    let off = run(false);
    let on = run(true);
    assert_eq!(off, on, "列块态下开关不许改变结果");
    assert!(!on.contains(')'), "列块里补对会把同一列拧歪，实得 {on:?}");
    assert_eq!(
        on.matches('(').count(),
        2,
        "前提：块内两行各落下一个左括号（塌回单光标插入就只有 1 个），实得 {on:?}"
    );
}

/// 第 14 格：多光标下不补：那条链的成组判据要求载荷恰为一个字符，双字符会把每一步
/// 打成碎撤销——宁可一格都不配，也不半工作。
#[test]
fn multi_caret_typing_inserts_the_bracket_alone() {
    // 两个光标都放在行尾（右侧是行界）⇒ 判据那一侧确实"想"补对，
    // 这条才有东西可拒绝（同上一条的假绿账）
    let run = |on: bool| {
        let mut app = Editpad::default();
        dispatch(
            &mut app,
            Message::Edit(EditOp::InsertText("ab\ncd\n".to_owned())),
        );
        {
            let mut core = app.cur_handle.borrow_mut();
            core.cursor = CursorPos { line: 0, col: 2 };
            assert!(
                core.toggle_extra_cursor(CursorPos { line: 1, col: 2 }),
                "用例前提：第二条光标要真的加上"
            );
            assert!(core.has_multi());
        }
        if on {
            turn_on(&mut app);
        }
        type_char(&mut app, '(');
        doc(&app)
    };
    let off = run(false);
    let on = run(true);
    assert_eq!(off, on, "多光标下开关不许改变结果");
    assert_eq!(
        on.matches('(').count(),
        2,
        "前提：两个光标各插了一个左括号（被折叠成单光标就只剩 1 个），实得 {on:?}"
    );
    assert!(
        !on.contains(')'),
        "多光标下不许补右括号（那会把同步编辑的成组语义换成另一套），实得 {on:?}"
    );
}

/// 第 15 格：只读页拒收这一击，用的是既有那道总闸（新变体不进白名单 ⇒ 默认拒绝）。
#[test]
fn a_read_only_page_refuses_an_auto_pair_insert() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    app.cur_handle.borrow_mut().read_only = true;
    type_char(&mut app, '(');
    assert_eq!(doc(&app), "", "只读页不许被补进任何字符");
    assert!(!app.tab().dirty, "被拒收的一击不得置脏");
}

/// 第 16 格：设置页那一行：出现在外观页、带控件、文案把「默认关」写出来。
#[test]
fn the_row_appears_on_the_appearance_page_with_a_control() {
    let app = Editpad::default();
    let row = app
        .rows_for(SettingsPage::Appearance)
        .into_iter()
        .find(|r| r.key == editpad_core::lang::ROW_AUTO_PAIR)
        .expect("设置页外观分类要有「打字时自动补齐括号」这一行");
    assert!(!row.title.trim().is_empty(), "标题取自本语言文案");
    assert!(
        row.desc.contains("默认关") || row.desc.contains("Off by default"),
        "文案要把「默认关」写出来——它写入的是用户没按下的字符，实得 {:?}",
        row.desc
    );
    assert!(
        app.settings_row_control(editpad_core::lang::ROW_AUTO_PAIR)
            .is_some(),
        "这一行必须有控件（复选框），否则开关只能在配置文件里改"
    );
}

/// 第 17 格：两向落盘（与其余偏好同一条持久化路）。
#[test]
fn toggling_the_setting_persists_it_like_the_other_preferences() {
    let dir = scratch_dir("p449-auto-pair");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    dispatch(&mut app, Message::SettingsAutoPairToggled(true));
    assert!(
        editpad_core::Settings::load_from(&config).auto_pair_brackets,
        "开了不落实盘＝重启就丢，用户会觉得这个开关没用"
    );
    dispatch(&mut app, Message::SettingsAutoPairToggled(false));
    assert!(
        !editpad_core::Settings::load_from(&config).auto_pair_brackets,
        "关回去也要落盘（两向都要能存）"
    );
}

/// 第 18 格：换文档时那份「刚补上的右括号」状态作废——不然上一页的标记会替下一页作案。
#[test]
fn a_stale_marker_from_the_previous_document_does_not_delete_the_new_page_bracket() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    assert_eq!(doc(&app), "()", "前提：本页刚补上一对");
    // 换一份正文里手打着 `()` 的文档，光标落在两括号之间
    let mut core = app.cur_handle.borrow_mut();
    core.reset_document(editpad_core::Document::from_str("()"));
    core.cursor = CursorPos { line: 0, col: 1 };
    drop(core);
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(
        doc(&app),
        ")",
        "新文档里那对括号是文件自带的，退格只能删用户按过的那一侧"
    );
}

/// 第 19 格：光标一旦走开，「刚补上的那一个」就作废——绕一圈回到括号中间退格，
/// 只许删掉左侧那一个字符。`break_typing` 里那份清空目前只有这一格看着。
#[test]
fn a_motion_away_and_back_retires_the_marker() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    type_char(&mut app, '(');
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::Right, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::Left, false)));
    assert_eq!(
        caret(&app),
        CursorPos { line: 0, col: 1 },
        "夹具自证：确实绕回了括号中间"
    );
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert_eq!(
        doc(&app),
        ")",
        "光标挪开过又挪回来，那对括号不再算「刚补的」，退格只该删一个字符"
    );
}
