//! P310（路线图 A6）应用层单测：命中↔书签两条动作经消息链的行为。
//!
//! core 侧的三条不变量在 `editor::hits_tests` 钉；这里钉的是**应用层**才看得见的
//! 东西：状态栏反馈的三种分叉、只读页放行（白名单登记对了没有）、
//! 以及「点了不置脏、不排自动保存」。

use super::*;

/// 给当前页装一批命中（夹具入口＝测试专用造表口）。
fn set_hits(app: &mut Editpad, lines: &[usize]) {
    let hits: Vec<editpad_core::MatchPos> = lines
        .iter()
        .map(|&l| editpad_core::MatchPos {
            line: l,
            col: 0,
            len_chars: 1,
        })
        .collect();
    app.cur_handle.borrow_mut().set_find_highlights(hits);
}

fn put_text(app: &mut Editpad, text: &str) {
    dispatch(app, Message::Edit(EditOp::InsertText(text.to_owned())));
}

#[test]
fn mark_hit_lines_reports_the_new_count() {
    let mut app = Editpad::default();
    put_text(&mut app, "alpha\nbeta\ngamma");
    let dirty_before = app.tab().dirty;
    set_hits(&mut app, &[0, 2, 2]);
    dispatch(&mut app, Message::Edit(EditOp::MarkHitLinesAsBookmarks));
    assert_eq!(app.cur_handle.borrow().bookmarked_lines(), vec![0, 2]);
    assert!(
        app.status.contains('2') && !app.status_is_error,
        "状态栏应报新增 2 行，实得 {:?}",
        app.status
    );
    assert_eq!(app.tab().dirty, dirty_before, "标注不改正文 ⇒ 不该新增置脏");

    // 再点一次：全部已带书签 ⇒ 换一条口径不同的提示（不是"新增 0 行"）
    dispatch(&mut app, Message::Edit(EditOp::MarkHitLinesAsBookmarks));
    assert!(
        !app.status_is_error && !app.status.chars().any(|c| c.is_ascii_digit()),
        "重复标记该说「已在书签里」而不是再报一个数，实得 {:?}",
        app.status
    );
}

#[test]
fn mark_and_copy_without_hits_say_so() {
    let mut app = Editpad::default();
    put_text(&mut app, "alpha\nbeta");
    for op in [EditOp::MarkHitLinesAsBookmarks, EditOp::CopyHitLines] {
        dispatch(&mut app, Message::Edit(op));
        assert!(app.status_is_error, "无命中必须有提示");
        assert!(app.cur_handle.borrow().bookmarks.is_empty());
    }
}

#[test]
fn read_only_page_still_allows_marking_hit_lines() {
    // 用户可见契约：只读页禁的是**改内容**，标注命中行不是。
    // ⚠️ 这条测的是消息路径（两条动作都在消息层前置拦截，根本不进
    // apply_edit 的只读总闸）——白名单登记本身由
    // `hit_ops_stay_out_of_the_mutating_whitelist_by_design` 直接点名核对，
    // 别把这条当那道闸门看。
    let mut app = Editpad::default();
    put_text(&mut app, "alpha\nbeta\ngamma");
    let dirty_before = app.tab().dirty;
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "用例前提：已切只读");
    set_hits(&mut app, &[1]);
    dispatch(&mut app, Message::Edit(EditOp::MarkHitLinesAsBookmarks));
    assert_eq!(
        app.cur_handle.borrow().bookmarked_lines(),
        vec![1],
        "只读页应仍可标注命中行"
    );
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "alpha\nbeta\ngamma");
    assert_eq!(app.tab().dirty, dirty_before);
}

#[test]
fn hit_ops_stay_out_of_the_mutating_whitelist_by_design() {
    use crate::update::edit_op_mutates as m;
    assert!(!m(&EditOp::MarkHitLinesAsBookmarks));
    assert!(!m(&EditOp::CopyHitLines));
}
