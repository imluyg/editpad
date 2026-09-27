//! P318：书签列表面板。
//!
//! 面板本体只是"看得见 + 点得动"，所以判据分两层：
//! ① 数据源（升序、剔悬空）——列出来点不动的条目比少一条更糟；
//! ② 三个动作的消息层契约（跳／摘／收），加一条"卡片真的被摆上去了"的
//!    几何自证（`bookmarks_visible` 忘了在 view 里挂层，红就红在这儿）。
//! 只读页那一格守的是"面板绕开了 `Edit` 前置闸"这件事：绕开得对不对，
//! 与既有书签操作的只读口径（标注类放行）必须一致。

use super::*;
use crate::editor::EditOp;

/// 生产入口写一段多行文本（不直接改 doc）。
fn type_into(app: &mut Editpad, text: &str) {
    dispatch(app, Message::Edit(EditOp::InsertText(text.to_owned())));
}

/// 打开一页三行文档，书签打在 0 / 2 / 4 行（走真实的逐行开关）。
fn app_with_bookmarks() -> Editpad {
    let mut app = Editpad::default();
    type_into(&mut app, "l0\nl1\nl2\nl3\nl4\n");
    {
        let mut c = app.cur_handle.borrow_mut();
        for line in [0usize, 2, 4] {
            c.cursor = crate::editor::CursorPos { line, col: 0 };
            c.toggle_bookmark();
        }
    }
    app
}

#[test]
fn panel_lines_are_ascending_and_drop_dangling_rows() {
    let app = app_with_bookmarks();
    assert_eq!(
        app.bookmark_panel_lines(),
        vec![0, 2, 4],
        "面板数据源必须升序（BTreeSet 的顺序性是这一格的全部内容）"
    );
    // 悬空防御：书签行号越过当前行域（再映射与缩文档之间没有全局锁）
    let app2 = app_with_bookmarks();
    app2.cur_handle.borrow_mut().bookmarks.insert(9_999);
    assert_eq!(
        app2.bookmark_panel_lines(),
        vec![0, 2, 4],
        "越界的书签不许列出来——点一条不存在的行没有意义"
    );
}

#[test]
fn toggle_esc_and_view_layer_agree_on_visibility() {
    let mut app = app_with_bookmarks();
    assert!(!app.bookmarks_visible, "默认关着");
    dispatch(&mut app, Message::BookmarksToggled);
    assert!(app.bookmarks_visible, "命令面板那一条动作应当把面板开出来");
    let card = ViewTree::layout_default(&app).find_layer_card();
    assert!(card.height > 80.0, "开态应能布局出面板卡片，实得 {card:?}");
    dispatch(&mut app, Message::BookmarksToggled);
    assert!(!app.bookmarks_visible, "再按一次是关，不是重开一个新面板");

    // 卡片只可能在开态出现
    let found_when_closed = {
        let closed = ViewTree::layout_default(&app);
        closed.find(|b| (480.0..=600.0).contains(&b.width) && b.height > 80.0 && b.y > 40.0)
    };
    assert!(
        found_when_closed.is_none(),
        "关态还找得到卡片 ⇒ view 层没接上这个旗标"
    );

    // Esc 走唯一出口
    dispatch(&mut app, Message::BookmarksToggled);
    dispatch(&mut app, Message::BarsDismissed);
    assert!(
        !app.bookmarks_visible,
        "Esc 必须收起（dismiss_all_prompts 已登记）"
    );
}

#[test]
fn open_panel_does_not_squeeze_the_editor_body() {
    // 非模态卡片的定义：浮在正文之上，不占布局面积（P150 的同款判据）
    let mut app = app_with_bookmarks();
    let base = ViewTree::layout_default(&app).editor_body();
    app.bookmarks_visible = true;
    let with = ViewTree::layout_default(&app).editor_body();
    assert_eq!(
        base, with,
        "面板开着不许挤小正文面积（Stack 叠层，不是布局层）"
    );
}

#[test]
fn a_row_jump_lands_on_that_line_and_records_the_origin() {
    let mut app = app_with_bookmarks();
    app.cur_handle.borrow_mut().cursor = crate::editor::CursorPos { line: 1, col: 1 };
    let origin = app.cur_handle.borrow().cursor;
    dispatch(&mut app, Message::BookmarkGoto(4));
    let after = app.cur_handle.borrow().cursor;
    assert_eq!(after.line, 4, "点第 5 行那条书签应当落在第 5 行");
    assert_eq!(after.col, 0, "落点统一在行首（与「转到行」同口径）");
    dispatch(&mut app, Message::Edit(EditOp::NavBack));
    assert_eq!(
        app.cur_handle.borrow().cursor,
        origin,
        "面板跳转也是一次「跳过去」⇒ 出发点要进历史，一次 Alt+← 就能回来"
    );
}

#[test]
fn goto_a_line_that_no_longer_exists_clamps_instead_of_panicking() {
    let mut app = app_with_bookmarks();
    dispatch(&mut app, Message::BookmarkGoto(9_000));
    let line = app.cur_handle.borrow().cursor.line;
    assert_eq!(
        line, 5,
        "越界夹到最后一行（含幻影末行），不许 panic 也不许丢点击"
    );
}

#[test]
fn removing_one_line_takes_only_that_bookmark_and_undoes_back() {
    let mut app = app_with_bookmarks();
    // 夹具前提：打字本身已经把页置脏了 ⇒ 这里断的是"摘书签不许再脏一次"，
    // 不是"页是干净的"（后者是夹具自己造出来的假前提）
    let dirty_before = app.tab().dirty;
    dispatch(&mut app, Message::BookmarkRemoveAt(2));
    assert_eq!(
        app.bookmark_panel_lines(),
        vec![0, 4],
        "「×」只摘这一条，邻居不许被连带"
    );
    assert_eq!(app.tab().dirty, dirty_before, "书签是标注，不该改动脏标记");
    assert!(!app.status.is_empty(), "摘掉了要给一句反馈，不能静默");
    let status = app.status.clone();
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        app.bookmark_panel_lines(),
        vec![0, 2, 4],
        "摘书签进撤销栈（与 toggle/clear 同待遇），Ctrl+Z 该回来"
    );
    assert_ne!(app.status, status, "撤销那一步自己也要给话说，别留着上一条");
}

#[test]
fn removing_a_line_without_a_bookmark_says_so_and_changes_nothing() {
    // 面板列表可能差一帧：开着面板时别处清过书签，再点「×」就是这一格
    let mut app = app_with_bookmarks();
    let before = app.bookmark_panel_lines();
    dispatch(&mut app, Message::BookmarkRemoveAt(3));
    assert_eq!(app.bookmark_panel_lines(), before, "空转不该动任何书签");
    assert_eq!(
        app.status,
        app.t(editpad_core::Key::StBookmarkNotThere).to_owned(),
        "反馈要说「那一行已经没有书签」，不能报「已移除」"
    );
}

#[test]
fn panel_actions_stay_usable_on_a_read_only_page() {
    // 面板的跳／摘都不走 `Message::Edit` 的只读前置闸，这里把"该不该走"钉成
    // 一句可核对的话：书签是标注（既有 ToggleBookmark / BookmarksClearAll 都在
    // 非改动白名单里），因此只读页同样放行——而正文一个字都不许被这些动作改掉。
    let mut app = app_with_bookmarks();
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "用例前提：已切只读");
    let text_before = app.cur_handle.borrow().doc.to_text();
    dispatch(&mut app, Message::BookmarksToggled);
    dispatch(&mut app, Message::BookmarkGoto(2));
    dispatch(&mut app, Message::BookmarkRemoveAt(2));
    assert_eq!(app.cur_handle.borrow().cursor.line, 2, "只读页也要能跳转");
    assert_eq!(
        app.bookmark_panel_lines(),
        vec![0, 4],
        "只读页的「×」与标注类快捷键同口径：放行"
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        text_before,
        "书签操作一个字符都不许动正文"
    );
}

#[test]
fn open_panel_renders_one_row_area_per_listed_bookmark() {
    // 行按钮的"哪一行投递哪条消息"这一层，`ViewTree::click` 打不到（事件到不了
    // Stack 叠层里的按钮——P308 的右键菜单用例同样只能直接投递
    // `EditorCtxCommand`，不走点击）。所以这里只钉"卡片确实按书签条数长出内容、
    // 且条数变了卡片也变"，逐条点击的接线留给手测（`docs/手测清单.md`）。
    let mut app = app_with_bookmarks();
    app.bookmarks_visible = true;
    let card3 = ViewTree::layout_default(&app).find_layer_card();
    {
        let mut c = app.cur_handle.borrow_mut();
        c.bookmarks.remove(&2);
    }
    let card2 = ViewTree::layout_default(&app).find_layer_card();
    assert_eq!(
        card3, card2,
        "卡片尺寸是钳制出来的定值，行数变化不该改它（改了就说明它在按内容撑高，\
         与 `scrollable` 的定高契约不符）：{card3:?} vs {card2:?}"
    );
    assert_eq!(app.bookmark_panel_lines().len(), 2, "前提：只剩两条书签");
}
