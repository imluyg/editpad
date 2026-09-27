//! P319：剪贴板历史面板（功能队列④）。
//!
//! 判据分三层：
//! ① **记账**——一次手势恰好一条记录；重复复制不涨条数只挪栈顶；两条封顶
//!    各管一头（条数上限、总量上限），且总量触顶时**宁可逐出旧条目也不截断
//!    内容**（"取回来的就是当时复制的那一份"是这项功能唯一的契约）；
//! ② **取用**——插入走既有粘贴路径，所以只读页拒收、撤销可回、越界不 panic
//!    三格都是继承来的口径，本功能不长第二个写正文的入口；
//! ③ **面板**——条目只带预览与规模、绝不带全文。`palette_all_entries` 每帧
//!    重建这个 Vec，把几兆的历史抄进条目就是每帧抄几兆（本仓性能池反复
//!    量到的那类无声退化，这里提前装一道闸）。
//!
//! 逐条"点第 N 行投递第 N 条"仍打不到（Stack 叠层里的按钮收不到注入的
//! 点击，与 P318 同一堵墙），所以面板接线用面板自己发的两条消息钉：
//! `dispatch_action("clip_history")` 打得开、`PalettePick(i)` 取得用。
//! 封顶那两格直接调 `clip_write`——它是六个复制写入点的公共出口，
//! 25 条手势造不出来，但走的是同一段记账代码。

use super::*;
use crate::editor::EditOp;
use crate::state::PaletteMode;
use iced::keyboard::Modifiers;

/// 与 `update/edit.rs` 里的两个封顶常量对齐（测试侧**故意重述一遍数字**：
/// 有人顺手改额度时，这里当场对不上）。
const MAX_ENTRIES: usize = 20;
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// 走产品出口记一条历史（Task 被丢弃，与 `dispatch` 同口径）。
fn clip(app: &mut Editpad, text: &str) {
    let _ = app.clip_write(text.to_owned());
}

/// 造一页已有文本，光标停在文末。
fn app_with(text: &str) -> Editpad {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::Edit(EditOp::InsertText(text.to_owned())));
    app
}

#[test]
fn one_copy_gesture_records_exactly_one_entry_newest_first() {
    let mut app = app_with("alpha");
    dispatch(&mut app, Message::Edit(EditOp::SelectAll));
    dispatch(&mut app, Message::CopyRequested);
    assert_eq!(
        app.clip_history,
        vec!["alpha".to_owned()],
        "一次 Ctrl+C 只该产一条历史，内容就是复制的那一份"
    );
    // 第二次复制另一段（内容变了）⇒ 新条目落在栈顶，旧的退到第二。
    // ⚠️ 先收拢选区再打字：SelectAll 之后直接 InsertText 会**替换**选区，
    // 第二段就成了 " beta" 而不是 "alpha beta"（第一版夹具就栽在这）。
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(crate::editor::Motion::DocEnd, false)),
    );
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText(" beta".to_owned())),
    );
    dispatch(&mut app, Message::Edit(EditOp::SelectAll));
    dispatch(&mut app, Message::CopyRequested);
    assert_eq!(
        app.clip_history,
        vec!["alpha beta".to_owned(), "alpha".to_owned()],
        "最近期在前"
    );
}

#[test]
fn block_cut_records_the_block_and_still_deletes_it() {
    // 剪切有三个来源分叉（列块／选区／整行），本仓 P319 把它们收进了一个出口。
    // 列块那一支的后续删除是**同步递归 update** ⇒ 这一格能同时钉住
    // "记了一条"与"块真的被删了"（另两支的删除走 Task::done，dispatch 会丢，
    // 那是夹具能力边界，已如实登记，不是产品缺陷）。
    let mut app = app_with("abcdef\n");
    app.cur_handle.borrow_mut().block_sel = Some(crate::editor::BlockSel {
        anchor: crate::editor::CursorPos { line: 0, col: 1 },
        head: crate::editor::CursorPos { line: 0, col: 4 },
    });
    dispatch(&mut app, Message::CutRequested);
    assert_eq!(
        app.clip_history,
        vec!["bcd".to_owned()],
        "列块剪切记的是块内容（列 1..4，末列不含）"
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "aef\n",
        "剪切仍要删掉块，历史不该把它变成纯复制"
    );
}

#[test]
fn copying_the_same_text_moves_it_to_top_without_growing() {
    let mut app = Editpad::default();
    clip(&mut app, "A");
    clip(&mut app, "B");
    clip(&mut app, "A");
    assert_eq!(
        app.clip_history,
        vec!["A".to_owned(), "B".to_owned()],
        "重复复制同一段文字不该多出一条，只需挪到栈顶（MRU）"
    );
    clip(&mut app, "");
    assert_eq!(
        app.clip_history.len(),
        2,
        "空文本不入历史：复制空行/空块不该挤掉有用的记录"
    );
}

#[test]
fn empty_selection_on_an_empty_page_leaves_no_record() {
    // 无选区 Ctrl+C = 复制当前整行（P122）；空正文那一行是空串 ⇒ 不该有历史
    let mut app = Editpad::default();
    dispatch(&mut app, Message::CopyRequested);
    assert!(
        app.clip_history.is_empty(),
        "实得 {:?}：空串入历史会让人以为复制成功，且第一条永远是垃圾",
        app.clip_history
    );
}

#[test]
fn entry_count_cap_keeps_newest_and_drops_oldest() {
    let mut app = Editpad::default();
    for i in 0..(MAX_ENTRIES + 5) {
        clip(&mut app, &format!("item-{i}"));
    }
    assert_eq!(
        app.clip_history.len(),
        MAX_ENTRIES,
        "条数封顶在 {MAX_ENTRIES}，超出的从最旧一侧弃"
    );
    assert_eq!(app.clip_history[0], format!("item-{}", MAX_ENTRIES + 4));
    assert!(
        !app.clip_history.iter().any(|t| t == "item-0"),
        "保新弃旧：最旧那条应当已被逐出"
    );
}

#[test]
fn byte_budget_evicts_olders_but_never_truncates_the_head() {
    let big = "x".repeat(MAX_BYTES + 1);
    let mut app = Editpad::default();
    clip(&mut app, "small-1");
    clip(&mut app, "small-2");
    clip(&mut app, &big);
    assert_eq!(
        app.clip_history.len(),
        1,
        "总量封顶：一条就吃满预算时，旧的逐出，但**头一条保住**"
    );
    assert_eq!(
        app.clip_history[0], big,
        "内容一字不截——历史里存的那份必须能原样取回，否则复制长文本的人\
         会在粘贴时拿到半份而毫不知情"
    );
    assert_eq!(app.clip_history[0].len(), MAX_BYTES + 1, "逐字节等长");
}

#[test]
fn picking_inserts_that_text_and_bumps_it_to_the_top() {
    let mut app = app_with("start\n");
    let before = app.cur_handle.borrow().doc.to_text();
    clip(&mut app, "first");
    clip(&mut app, "second");
    assert_eq!(
        app.clip_history,
        vec!["second".to_owned(), "first".to_owned()],
        "用例前提：最近期在前，所以下标 1 是「first」那条"
    );
    dispatch(&mut app, Message::ClipPick(1));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        format!("{before}first"),
        "取用下标 1 就该把**下标 1** 那条插到光标处（走的是既有粘贴路径）"
    );
    assert_eq!(
        app.clip_history,
        vec!["first".to_owned(), "second".to_owned()],
        "取用也算一次「用」：挪到栈顶，条数不变"
    );
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        before,
        "插入是一次可撤销的编辑，不是绕过撤销栈的旁路"
    );
}

#[test]
fn picking_an_index_that_no_longer_exists_is_a_silent_no_op() {
    let mut app = app_with("body");
    let before = app.cur_handle.borrow().doc.to_text();
    clip(&mut app, "only");
    for i in [1usize, 9_999, usize::MAX] {
        dispatch(&mut app, Message::ClipPick(i));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            before,
            "越界下标不许插入任何内容（面板那一帧之后历史可能已变）"
        );
    }
    assert_eq!(
        app.clip_history,
        vec!["only".to_owned()],
        "越界也不许动历史本身"
    );
}

#[test]
fn picking_on_a_read_only_page_changes_not_a_character() {
    // 取用经 `Pasted → Edit(InsertText)` ⇒ 只读前置闸自动继承，本功能没开新洞
    let mut app = app_with("locked");
    dispatch(&mut app, Message::Edit(EditOp::SelectAll));
    dispatch(&mut app, Message::CopyRequested);
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "用例前提：已切只读");
    let before = app.cur_handle.borrow().doc.to_text();
    dispatch(&mut app, Message::ClipPick(0));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        before,
        "只读页取历史不该写进正文"
    );
}

#[test]
fn panel_rows_carry_a_bounded_preview_never_the_full_text() {
    // 面板每帧重建条目：这一格钉的是"条目里不许有全文"，
    // 抄进去一次就是每帧抄一次几兆。
    let mut app = Editpad::default();
    let big = "long line of pasted text\n".repeat(8_000); // ≈ 200 KB
    clip(&mut app, &big);
    app.palette_mode = PaletteMode::Clipboard;
    let entries = app.palette_all_entries();
    assert_eq!(entries.len(), 1, "一条历史一行");
    let e = &entries[0];
    assert_eq!(e.clip_index, Some(0), "条目带的是存储下标");
    assert!(
        e.title.chars().count() <= 61,
        "预览封顶 60 字符＋一个省略号，实得 {} 字符",
        e.title.chars().count()
    );
    assert!(
        !e.title.contains('\n') && e.title.contains('⏎'),
        "一行只显示一行：换行折成可见符号，实得 {:?}",
        e.title
    );
    assert!(
        !e.detail.contains(&big),
        "detail 只该是规模摘要，不该带着正文"
    );
    assert!(
        e.detail.contains(&big.len().to_string()),
        "规模摘要要报真实字节数，实得 {:?}",
        e.detail
    );
}

#[test]
fn row_summary_counts_real_lines_and_real_utf8_bytes() {
    let mut app = Editpad::default();
    clip(&mut app, "one\ntwo\nthree");
    app.palette_mode = PaletteMode::Clipboard;
    let detail = app.palette_all_entries()[0].detail.clone();
    assert_eq!(
        detail,
        format!(
            "3 {} · 13 {}",
            app.t(editpad_core::Key::ClipUnitLines),
            app.t(editpad_core::Key::ClipUnitBytes)
        ),
        "末尾换行不多算一行（lines() 口径），字节按 UTF-8 实际长度"
    );
    // 非 ASCII 的第二格：三个希腊字母是 6 字节，既不是 3 也不是「字符×3」
    let mut app2 = Editpad::default();
    clip(&mut app2, "αβγ");
    app2.palette_mode = PaletteMode::Clipboard;
    let d2 = app2.palette_all_entries()[0].detail.clone();
    assert!(
        d2.contains("6 ") && !d2.contains("9 "),
        "字节数按实际 UTF-8 长度，实得 {d2:?}"
    );
}

#[test]
fn palette_action_opens_the_history_and_a_pick_closes_it() {
    // 端到端接线：注册表那条动作 → 面板开在剪贴板模式 → 面板发的
    // PalettePick(i) → 正文长出这一条，且面板收起（与命令面板同款一次性动作）
    let mut app = app_with("base\n");
    let msg = crate::hotkeys::dispatch_action("clip_history", Modifiers::empty())
        .expect("clip_history 已注册，dispatch 必须有分支");
    dispatch(&mut app, msg);
    assert!(app.palette_visible, "动作要把面板开出来");
    assert_eq!(
        app.palette_mode,
        PaletteMode::Clipboard,
        "开的是历史模式，不是命令模式"
    );
    clip(&mut app, "from-history");
    dispatch(&mut app, Message::PalettePick(0));
    assert!(!app.palette_visible, "取用后面板收起（命令面板同口径）");
    assert!(
        app.cur_handle
            .borrow()
            .doc
            .to_text()
            .contains("from-history"),
        "点第一条就该把它插进正文"
    );
}

#[test]
fn empty_history_still_opens_a_card_and_says_what_is_missing() {
    // 第一次开这个面板必然两手空空。两格分开钉：
    // ① 空历史也要能开出面板（"没内容"与"没开"在界面上必须分得开）；
    // ② 空态那句必须是剪贴板模式专用的话，不是「无匹配」——
    //    视图树夹具只读得到几何、读不到文本，所以钉的是那句裁决本身。
    let mut app = Editpad::default();
    assert!(app.clip_history.is_empty(), "用例前提：一条历史都没有");
    dispatch(&mut app, Message::PaletteToggled(PaletteMode::Clipboard));
    assert!(app.palette_visible, "空历史也要把面板开出来");
    assert!(
        ViewTree::layout_default(&app).find_layer_card().height > 80.0,
        "开态要真能布局出卡片（忘了挂层的红就红在这儿）"
    );
    let clip_label = app.palette_empty_label();
    app.palette_mode = PaletteMode::Commands;
    let generic = app.palette_empty_label();
    assert!(
        !clip_label.is_empty() && clip_label != generic,
        "剪贴板模式的空态该另说一句，实得「{clip_label}」，命令模式是「{generic}」"
    );
}
