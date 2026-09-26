//! P307 单测：编辑区连击选择（双击选词 / 三击选行 / 按词按行拖选 /
//! Shift+点击扩展）。
//!
//! 控件层的分支顺序（连击裁决须先于 dnd 候选）没有无头事件流可驱动，
//! 与 P135 同口径：判定与选区计算全在 core，这里按「命中点直入」覆盖。

use super::tests::*;
use super::*;
use std::time::{Duration, Instant};

/// 当前选区文本（无选区 = 空串）。
fn sel(c: &EditorCore) -> String {
    let (s, e) = c.selection_offsets().unwrap_or((0, 0));
    c.doc.slice_text(s, e)
}

fn pos(line: usize, col: usize) -> CursorPos {
    CursorPos { line, col }
}

/// 同一个测试内多拍点击共用的基准时刻（各拍用 [`ms`] 取偏移，链才成立）。
fn base() -> Instant {
    Instant::now()
}

fn ms(b: Instant, millis: u64) -> Instant {
    b + Duration::from_millis(millis)
}

// ---------- 双击选词 ----------

#[test]
fn double_click_selects_the_whole_word_run() {
    let mut c = core_with("let alpha = beta();");
    assert!(c.select_word_at(pos(0, 5)));
    assert_eq!(sel(&c), "alpha");
    // 选区 = anchor..cursor，Ctrl+C 取的正是这条（用户主诉「双击复制」的前提）
    assert_eq!(c.anchor, Some(pos(0, 4)));
    assert_eq!(c.cursor, pos(0, 9));
    assert!(c.dragging, "按下后不松手继续拖应当处于拖选态");
}

#[test]
fn double_click_at_line_end_selects_last_word() {
    let mut c = core_with("hello");
    assert!(c.select_word_at(pos(0, 5)), "行尾处向左取最后一个字符");
    assert_eq!(sel(&c), "hello");
}

#[test]
fn double_click_treats_cjk_run_as_one_word() {
    let mut c = core_with("中文测试 abc");
    assert!(c.select_word_at(pos(0, 1)));
    assert_eq!(sel(&c), "中文测试");
}

#[test]
fn double_click_on_punctuation_selects_just_that_char() {
    // 与 word_neighbor（词移动「单个标点一步」）同口径
    let mut c = core_with("let alpha = beta();");
    assert!(c.select_word_at(pos(0, 10)));
    assert_eq!(sel(&c), "=");
}

#[test]
fn double_click_on_whitespace_selects_the_space_run() {
    let mut c = core_with("a    b");
    assert!(c.select_word_at(pos(0, 3)));
    assert_eq!(sel(&c), "    ");
}

#[test]
fn double_click_on_blank_line_selects_nothing() {
    let mut c = core_with("one\n\ntwo");
    assert!(!c.select_word_at(pos(1, 0)), "空行无从选词");
    assert!(!c.select_word_at(pos(1, 7)), "越界列同样不选");
    assert_eq!(c.anchor, None);
    assert!(!c.dragging);
}

// ---------- 三击选行 ----------

#[test]
fn triple_click_selects_line_with_its_line_break() {
    let mut c = core_with("one\ntwo\nthree");
    assert!(c.select_line_at(pos(1, 2)));
    assert_eq!(sel(&c), "two\n", "含行末换行符（与整行剪切同口径）");
    assert_eq!(c.anchor, Some(pos(1, 0)));
    assert_eq!(c.cursor, pos(2, 0));
}

#[test]
fn triple_click_on_last_line_stops_at_line_end() {
    let mut c = core_with("one\ntwo\nthree");
    assert!(c.select_line_at(pos(2, 99)));
    assert_eq!(sel(&c), "three");
}

#[test]
fn triple_click_with_crlf_takes_the_whole_line_ending() {
    // 只吃 `\n` 会留下一个孤零零的 `\r`——右端点取「下一行行首」，
    // 与行结尾是 LF 还是 CRLF 无关。
    let mut c = core_with("one\r\ntwo\r\nthree");
    assert!(c.select_line_at(pos(1, 1)));
    assert_eq!(sel(&c), "two\r\n");
}

// ---------- 连击计数 ----------

#[test]
fn click_count_chains_on_same_line_within_tolerance_and_window() {
    let b = base();
    let mut c = core_with("alpha beta gamma");
    assert_eq!(c.register_click(pos(0, 5), ms(b, 0)), 1);
    assert_eq!(
        c.register_click(pos(0, 6), ms(b, 120)),
        2,
        "同行微移 = 双击"
    );
    assert_eq!(c.register_click(pos(0, 6), ms(b, 240)), 3, "再一拍 = 三击");
    // 第四次起算重新从 1 开始（三击已是本层最大粒度，继续累加无语义可挂）
    assert_eq!(c.register_click(pos(0, 6), ms(b, 300)), 1);
}

#[test]
fn click_count_breaks_on_gap_line_or_timeout() {
    let b = base();
    // 列距超容差 = 换了目标
    let mut c = core_with("alpha beta gamma delta epsilon");
    assert_eq!(c.register_click(pos(0, 0), ms(b, 0)), 1);
    assert_eq!(c.register_click(pos(0, 20), ms(b, 50)), 1);

    // 换行
    let mut c = core_with("alpha\nbeta gamma");
    assert_eq!(c.register_click(pos(0, 1), ms(b, 0)), 1);
    assert_eq!(c.register_click(pos(1, 1), ms(b, 50)), 1);

    // 超出双击窗（与标签条就地重命名同值 = 500ms，边界含）
    let mut c = core_with("alpha");
    assert_eq!(c.register_click(pos(0, 1), ms(b, 0)), 1);
    assert_eq!(
        c.register_click(pos(0, 1), ms(b, 501)),
        1,
        "超窗 ⇒ 重新计为单击"
    );
    assert_eq!(
        c.register_click(pos(0, 1), ms(b, 700)),
        2,
        "与新基准重新成链"
    );

    let mut c = core_with("alpha");
    assert_eq!(c.register_click(pos(0, 1), ms(b, 0)), 1);
    assert_eq!(c.register_click(pos(0, 1), ms(b, 500)), 2, "窗口边界含");
}

// ---------- 按词 / 按行拖选 ----------

#[test]
fn word_drag_extends_forward_over_whole_words() {
    let mut c = core_with("alpha beta gamma");
    assert!(c.select_word_at(pos(0, 1)));
    assert_eq!(sel(&c), "alpha");
    // 拖到 beta 之后的空格上：空格段并入选择（与主流编辑器一致）
    assert!(c.apply_click_drag(pos(0, 10)), "拖过词界应整词并入");
    assert_eq!(sel(&c), "alpha beta ");
    assert!(c.apply_click_drag(pos(0, 13)));
    assert_eq!(sel(&c), "alpha beta gamma");
    assert!(!c.apply_click_drag(pos(0, 13)), "落点未变 = 不重复发变更");
}

#[test]
fn word_drag_extends_backward_over_whole_words() {
    let mut c = core_with("one two three");
    assert!(c.select_word_at(pos(0, 5)), "选中 two");
    assert_eq!(sel(&c), "two");
    assert!(c.apply_click_drag(pos(0, 1)));
    assert_eq!(sel(&c), "one two", "反向拖选含锚点词自身");
}

#[test]
fn word_drag_collapsing_back_onto_leading_whitespace() {
    // 锚点词之前的空格属于哪一侧：整段空格并入左侧选择
    let mut c = core_with("one two three");
    assert!(c.select_word_at(pos(0, 5)));
    assert!(c.apply_click_drag(pos(0, 3)));
    assert_eq!(sel(&c), " two");
    // 再往回到锚点词内部：缩回原词，不会翻成空选择
    assert!(c.apply_click_drag(pos(0, 5)));
    assert_eq!(sel(&c), "two");
}

#[test]
fn line_drag_extends_over_lines_in_both_directions() {
    let mut c = core_with("a\nb\nc\nd\ne");
    assert!(c.select_line_at(pos(2, 0)));
    assert_eq!(sel(&c), "c\n");
    assert!(c.apply_click_drag(pos(4, 0)));
    assert_eq!(sel(&c), "c\nd\ne");

    let mut c = core_with("a\nb\nc\nd\ne");
    assert!(c.select_line_at(pos(2, 0)));
    assert!(c.apply_click_drag(pos(0, 0)));
    assert_eq!(sel(&c), "a\nb\nc\n", "反向 = 覆盖到锚点行末");
}

#[test]
fn char_drag_is_left_to_the_existing_path() {
    // 普通单击（未连击）不得被按词/按行分支接管
    let mut c = core_with("alpha beta");
    c.cursor = pos(0, 0);
    c.dragging = true;
    assert!(!c.apply_click_drag(pos(0, 3)), "无连击基准 ⇒ 逐字符路径");

    // 释放之后粒度复位：同一个 core 再拖选按逐字符
    let mut c = core_with("alpha beta");
    assert!(c.select_word_at(pos(0, 1)));
    c.end_click_drag();
    assert!(!c.apply_click_drag(pos(0, 10)));
}

// ---------- Shift+点击扩展 ----------

#[test]
fn shift_click_extends_from_existing_anchor() {
    let mut c = core_with("one two three");
    c.anchor = Some(pos(0, 4));
    c.cursor = pos(0, 7);
    assert_eq!(sel(&c), "two");
    c.extend_selection_to(pos(0, 0));
    assert_eq!(c.anchor, Some(pos(0, 4)), "锚点不动");
    assert_eq!(c.cursor, pos(0, 0));
    assert_eq!(sel(&c), "one ", "归一后 = 0..4");
}

#[test]
fn shift_click_without_selection_uses_cursor_as_anchor() {
    let mut c = core_with("one two three");
    c.cursor = pos(0, 4);
    c.extend_selection_to(pos(0, 9));
    assert_eq!(c.anchor, Some(pos(0, 4)));
    assert_eq!(c.cursor, pos(0, 9));
    assert_eq!(sel(&c), "two t");
}

#[test]
fn shift_click_does_not_start_a_word_or_line_drag() {
    let mut c = core_with("one two three");
    c.cursor = pos(0, 0);
    c.extend_selection_to(pos(0, 7));
    c.dragging = true;
    // 扩展后按逐字符拖选（apply_click_drag 恒 false）
    assert!(!c.apply_click_drag(pos(0, 12)));
    assert_eq!(c.click_base, None);
}

// ---------- 换文档作废 ----------

#[test]
fn reset_document_drops_click_state() {
    let mut c = core_with("alpha beta");
    c.register_click(pos(0, 1), ms(base(), 0));
    assert!(c.select_word_at(pos(0, 1)));
    c.reset_document(Document::from_str("x"));
    assert_eq!(c.click_last, None);
    assert_eq!(c.click_base, None);
    assert_eq!(c.drag_gran, super::super::click::DragGran::Char);
    assert!(
        !c.apply_click_drag(pos(0, 0)),
        "旧坐标系的连击不得作用于新文档"
    );
}
