//! P310（路线图 A6）单测：查找命中 ↔ 书签联动。
//!
//! 三条不变量各钉一处：①命中行**去重升序**且**丢掉悬空行号**（命中表是后台
//! 扫描的产物，正文可能已被动过）；②标记是**可回滚标注**（随撤销恢复，不动正文）；
//! ③复制出的行文本按文档主导行尾连接（CRLF 文档不该混进裸 `\n`）。

use super::tests::*;
use super::*;

fn hit(line: usize) -> editpad_core::MatchPos {
    editpad_core::MatchPos {
        line,
        col: 0,
        len_chars: 1,
    }
}

/// 装一批命中（夹具入口，与生产链同一张表）。
fn with_hits(text: &str, lines: &[usize]) -> EditorCore {
    let mut c = core_with(text);
    c.set_find_highlights(lines.iter().map(|&l| hit(l)).collect());
    c
}

#[test]
fn hit_lines_are_deduped_sorted_and_clamped_to_the_document() {
    let mut c = with_hits("a\nb\nc", &[2, 0, 0, 1, 99]);
    assert_eq!(
        c.hit_lines(),
        vec![0, 1, 2],
        "99 是正文被改过之后的悬空行号"
    );
    assert_eq!(c.mark_hit_lines_as_bookmarks(), 3);
    assert_eq!(c.bookmarked_lines(), vec![0, 1, 2]);
    // 幂等：再点一次不新增、也不该再压一次撤销快照
    assert_eq!(c.mark_hit_lines_as_bookmarks(), 0);
    assert_eq!(c.bookmarked_lines(), vec![0, 1, 2]);
}

#[test]
fn marking_hit_lines_is_undoable_and_does_not_touch_text() {
    let mut c = with_hits("keep me\nand me", &[0, 1]);
    let before = c.doc.to_text();
    assert_eq!(c.mark_hit_lines_as_bookmarks(), 2);
    assert_eq!(c.doc.to_text(), before, "标注不改正文");
    assert!(c.undo(), "书签随快照回滚 ⇒ 撤销应可撤销这一步标注");
    assert!(c.bookmarks.is_empty(), "撤销后书签集应回到空");
    assert_eq!(c.doc.to_text(), before, "撤销同样不该动正文");
}

#[test]
fn copy_hit_lines_joins_in_the_documents_line_ending() {
    let c = with_hits("one\r\ntwo\r\nthree", &[2, 0, 0]);
    assert_eq!(
        c.copy_hit_lines_text().as_deref(),
        Some("one\r\nthree\r\n"),
        "升序去重 + 文档主导行尾（CRLF 文档不混裸 \\n）"
    );
    // 无命中 ⇒ None（应用层据此给提示而不是写一个空剪贴板）
    let empty = with_hits("one\ntwo", &[]);
    assert_eq!(empty.copy_hit_lines_text(), None);
    assert!(!empty.has_find_hits());
}

#[test]
fn empty_hit_table_closes_the_gate() {
    let mut c = with_hits("a\nb", &[0]);
    assert!(c.has_find_hits());
    // 关栏/切页后重新下发一张空表 ⇒ 放行判据与两条动作一起收
    c.set_find_highlights(Vec::new());
    assert!(!c.has_find_hits());
    assert_eq!(c.mark_hit_lines_as_bookmarks(), 0);
    assert_eq!(c.copy_hit_lines_text(), None);
}
