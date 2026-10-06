//! EditorCore 单测（P68 自 editor.rs tests 拆出；经 #[path] 挂为 core
//! 子模块，保留对私有字段的直接访问）。
use super::*;
// P9 断言用；非测试代码只经 Document::line_ending() 间接接触该类型
pub(super) use editpad_core::LineEnding;
// 横层引用（P33 字体候选/P59 诊断构造等）经 editor 模块再导出取用
pub(super) use super::super::metrics::is_wide;
pub(super) use super::super::scrollbars::{HScrollbar, VScrollbar, THUMB_MIN_H, THUMB_MIN_W};
pub(super) use super::super::*;
pub(super) use iced::advanced::text as core_text;
pub(super) use iced::advanced::text::Paragraph as _;
pub(super) use iced::alignment;
pub(super) use iced::{Pixels, Size};

pub(super) fn core_with(text: &str) -> EditorCore {
    let mut c = EditorCore::default();
    c.reset_document(Document::from_str(text));
    c
}
// 其余共享助手（measure_* / core_with_wide_line / strip_newlines / rope_line_bodies /
// assert_structural_invariants / wrap_core / wrap_converge）随各自测试区段迁入
// undo_tests / motion_tests（第 85 轮 Phase 3b 拆分），本文件不再保留副本。

#[test]
fn select_span_clamps_coordinates_from_stale_tables() {
    // 缺陷：`line`/`col` 来自外部表（「查找全部」面板、FIF 结果、书签），而正文可能
    // 已经变短——`undo()` 不清命中表是既有口径。旧写法把越界行号原样挂进
    // anchor/cursor：当下不崩，**下一个编辑动作**在 ropey 的 line_to_char 上 panic。
    let mut ed = core_with("one\ntwo\nthree");
    ed.select_span(9_999, 9, 4);
    let last = ed.doc.line_count() - 1;
    assert_eq!(ed.cursor.line, last, "越界行必须夹进文档，不许挂着 9999");
    assert_eq!(ed.anchor.map(|a| a.line), Some(last));
    assert!(
        ed.cursor.col <= ed.line_display_len(ed.cursor.line),
        "列也要夹在该行长度内，实际 {}",
        ed.cursor.col
    );

    // 后果那一半：夹紧之后接着打字不得炸
    ed.insert_str("x");
    assert_eq!(
        ed.doc.to_text(),
        "one\ntwo\nthreex",
        "落点应在末行行尾，且这一次插入必须活着走完"
    );
}
