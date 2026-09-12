//! EditorCore 单测（P68 自 editor.rs tests 拆出；经 #[path] 挂为 core
//! 子模块，保留对私有字段的直接访问）。
use super::*;
// P9 断言用；非测试代码只经 Document::line_ending() 间接接触该类型
pub(super) use editpad_core::LineEnding;
// 横层引用（P33 字体候选/P59 诊断构造等）经 editor 模块再导出取用
pub(super) use super::super::*;
pub(super) use iced::advanced::text as core_text;
pub(super) use iced::advanced::text::Paragraph as _;
pub(super) use iced::alignment;
pub(super) use iced::{Pixels, Size};
pub(super) use super::super::metrics::is_wide;
pub(super) use super::super::scrollbars::{
    HScrollbar, THUMB_MIN_H, THUMB_MIN_W, VScrollbar,
};

    pub(super) fn core_with(text: &str) -> EditorCore {
        let mut c = EditorCore::default();
        c.reset_document(Document::from_str(text));
        c
    }
    // 其余共享助手（measure_* / core_with_wide_line / strip_newlines / rope_line_bodies /
    // assert_structural_invariants / wrap_core / wrap_converge）随各自测试区段迁入
    // undo_tests / motion_tests（第 85 轮 Phase 3b 拆分），本文件不再保留副本。
