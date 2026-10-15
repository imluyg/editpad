//! P312（路线图 C11）单测：光标跳转历史（Alt+← / Alt+→）。
//!
//! 两条设计约束各钉一处：①**只记远距离跳转**——打字与方向键不许污染历史，
//! 否则"跳回去"很快就不再是用户跳过的地方；②栈有上限且换文档即作废
//! （落点属于旧坐标系，留着就是悬空坐标）。

use super::tests::*;
use super::*;

fn cur(c: &EditorCore) -> CursorPos {
    c.cursor
}

#[test]
fn jump_then_back_and_forward_round_trip() {
    let mut c = core_with("aaa\nbbb\nccc\nddd");
    c.cursor = CursorPos { line: 0, col: 3 };
    c.jump_to_line(3);
    assert_eq!(cur(&c), CursorPos { line: 2, col: 0 });
    assert!(c.nav_back());
    assert_eq!(cur(&c), CursorPos { line: 0, col: 3 }, "跳回出发点");
    assert!(c.nav_forward());
    assert_eq!(cur(&c), CursorPos { line: 2, col: 0 }, "再前进回同一处");
    assert!(!c.nav_forward(), "前进栈已空 ⇒ 静默 no-op");
}

#[test]
fn empty_history_does_nothing() {
    let mut c = core_with("abc");
    assert!(!c.nav_back());
    assert!(!c.nav_forward());
    assert_eq!(cur(&c), CursorPos { line: 0, col: 0 });
}

#[test]
fn typing_and_arrow_keys_do_not_fill_the_history() {
    // 反向护栏：历史只吃"远距离跳转"。这条若红了，说明某个逐字符路径被
    // 接进了 note_nav_origin —— 用户按几下方向键就把回退栈冲满
    let mut c = core_with("hello world");
    c.cursor = CursorPos { line: 0, col: 5 };
    for _ in 0..8 {
        c.apply_motion(Motion::Left, false);
        c.apply_motion(Motion::Right, false);
    }
    c.insert_str("x");
    assert!(c.nav_back.is_empty(), "逐字符移动/打字不该记进跳转历史");
    assert!(!c.nav_back());
}

#[test]
fn same_origin_is_recorded_once() {
    let mut c = core_with("a\nb\nc\nd");
    c.cursor = CursorPos { line: 0, col: 1 };
    c.jump_to_line(3);
    c.cursor = CursorPos { line: 0, col: 1 }; // 回到同一出发点
    c.jump_to_line(4);
    assert_eq!(c.nav_back.len(), 1, "连续从同一处跳走只算一个出发点");
    assert!(c.nav_back());
    assert_eq!(cur(&c), CursorPos { line: 0, col: 1 });
}

#[test]
fn a_new_jump_clears_the_forward_stack() {
    let mut c = core_with("a\nb\nc");
    c.cursor = CursorPos { line: 0, col: 0 };
    c.jump_to_line(3);
    assert!(c.nav_back());
    assert!(!c.nav_fwd.is_empty(), "用例前提：前进栈有货");
    c.jump_to_line(2); // 从回退位点另跳一处 ⇒ 前进链断掉
    assert!(c.nav_fwd.is_empty());
    assert!(!c.nav_forward());
}

#[test]
fn history_is_bounded_and_keeps_the_newest() {
    let mut c = core_with("x");
    for i in 0..(EditorCore::NAV_HISTORY_CAP as u32 + 50) {
        c.cursor = CursorPos {
            line: i as usize,
            col: 0,
        };
        c.note_nav_origin();
    }
    assert_eq!(c.nav_back.len(), EditorCore::NAV_HISTORY_CAP);
    // 丢的是最旧：栈顶仍是最后一次记的落点
    assert_eq!(
        *c.nav_back.last().unwrap(),
        CursorPos {
            line: EditorCore::NAV_HISTORY_CAP + 49,
            col: 0
        }
    );
    assert_eq!(c.nav_back.first().unwrap().line, 50, "最旧 50 条已被丢弃");
}

#[test]
fn nav_target_is_clamped_when_the_document_shrank() {
    let mut c = core_with("aaa\nbbb\nccc\nddd");
    c.cursor = CursorPos { line: 3, col: 3 };
    c.jump_to_line(1);
    assert_eq!(c.nav_back.len(), 1);
    // 跳走之后把后面的行删掉 ⇒ 回退落点越界，只能夹紧不能 panic
    c.cursor = CursorPos { line: 0, col: 0 };
    c.delete_current_lines();
    c.delete_current_lines();
    c.delete_current_lines();
    assert!(c.nav_back());
    assert_eq!(
        cur(&c),
        CursorPos { line: 0, col: 3 },
        "行夹到现存末行、列夹到该行行尾，而不是 panic"
    );
}

#[test]
fn bookmark_and_hit_jumps_record_the_origin() {
    // 两条"跳远处"的入口各自钉一格：书签跳转、查找命中定位（select_span）
    let mut c = core_with("a\nb\nc\nd");
    c.cursor = CursorPos { line: 0, col: 1 };
    c.jump_to_line(4); // 记 (0,1)
    assert_eq!(c.nav_back.len(), 1);
    c.toggle_bookmark(); // 书签落在第 4 行
    c.jump_to_line(1); // 记 (3,0)
    assert_eq!(c.nav_back.len(), 2);
    assert!(c.next_bookmark(true), "用例前提：存在他处书签");
    assert_eq!(c.nav_back.len(), 3, "书签跳转也记了出发点");
    c.select_span(1, 0, 1);
    assert_eq!(c.nav_back.len(), 4, "命中定位（select_span）同样记");
}

#[test]
fn reset_document_drops_the_history() {
    let mut c = core_with("a\nb\nc");
    c.cursor = CursorPos { line: 1, col: 1 };
    c.jump_to_line(3);
    assert!(!c.nav_back.is_empty());
    c.reset_document(Document::from_str("fresh"));
    assert!(c.nav_back.is_empty());
    assert!(c.nav_fwd.is_empty());
    assert!(!c.nav_back(), "旧坐标系的落点不得作用于新文档");
}

#[test]
fn bracket_jump_records_the_origin() {
    // 配对括号是远距离跳转，与书签／命中定位同一档：跳过去之后 Alt+← 该回得来
    let mut c = core_with("foo(bar)\nbaz");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.jump_to_matching_bracket());
    assert_eq!(
        cur(&c),
        CursorPos { line: 0, col: 8 },
        "用例前提：已落到配对括号的另一侧"
    );
    assert_eq!(c.nav_back.len(), 1, "括号跳转记出发点");
    assert!(c.nav_back());
    assert_eq!(
        cur(&c),
        CursorPos { line: 0, col: 3 },
        "退回首括号之前那一处"
    );
}

#[test]
fn a_failed_bracket_jump_records_nothing() {
    // 反面格：没跳成就不该留记录，否则用户按一次退回去是原地踏步的假出发点
    let mut c = core_with("plain text\nno brackets here");
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(!c.jump_to_matching_bracket(), "用例前提：光标不邻接括号");
    assert!(c.nav_back.is_empty(), "跳转失败不进历史");
}

#[test]
fn markdown_edits_do_not_record_the_origin() {
    // 反向护栏（与 typing_and_arrow_keys_do_not_fill_the_history 同族）：
    // 包行内标记与「空列表项回车退栏」内部都走选区，但那是编辑落点，不是用户跳过去的
    let mut c = core_with("甲 乙");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.wrap_selection(editpad_core::markdown::MdInlineMarker::Bold));
    assert_eq!(c.nav_back.len(), 0, "包粗体只是把正文重新选中，不该记跳转");

    let mut d = core_with("- 甲\n- \n");
    d.cursor = CursorPos { line: 1, col: 2 };
    d.enter_markdown(); // 空列表项 ⇒ 退出那一层（内部 select_span + 删标记 + 回车）
    assert_eq!(d.nav_back.len(), 0, "回车退栏不该记跳转");
}
