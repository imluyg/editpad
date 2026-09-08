use super::tests::*;
use crate::editor::block::sequence_lines;
use super::*;

// ---------- 第 57 轮：行操作套件 ----------

#[test]
fn delete_current_line_removes_whole_line_and_is_undoable() {
    let mut c = core_with("alpha\nbeta\ngamma");
    c.cursor = CursorPos { line: 1, col: 2 }; // 行中任意列都删整行
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "alpha\ngamma");
    assert_eq!(c.cursor, CursorPos { line: 1, col: 0 }, "光标落到删除起点");
    assert!(c.undo(), "行删除必须可撤销");
    assert_eq!(c.doc.to_text(), "alpha\nbeta\ngamma");
}

#[test]
fn delete_current_line_handles_crlf_tail_and_phantom_last_line() {
    // CRLF 文档按整行（含 \r\n 单元）删除，不留孤立 \r
    let mut c = core_with("a\r\nb\r\nc");
    c.cursor = CursorPos { line: 0, col: 0 };
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "b\r\nc");
    // 尾行无行尾：删除后前一行行尾保持原样
    c.cursor = CursorPos { line: 1, col: 1 };
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "b\r\n");
    // 幻影末行（尾随行尾产生的空行）：退化为吃掉前面的换行单元
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "b");
    // 只剩一行内容：删成空文档（主流编辑器同款手感）
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "");
}

#[test]
fn delete_on_empty_document_is_noop() {
    let mut c = core_with("");
    assert!(!c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "");
}

#[test]
fn delete_current_line_on_single_newline_document_no_underflow() {
    // 文档恰为一个换行符（唯一内容就是行尾）：光标在幻影末行时
    // start == 1，旧实现直接回看 start-2 触发 usize 下溢崩溃。
    // 钉死正确形态：退化为吃掉这唯一的换行单元，删成空文档。
    let mut c = core_with("\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 0 }, "光标归零");
    assert!(c.undo(), "必须可撤销");
    assert_eq!(c.doc.to_text(), "\n");
    // 孤立 \r 同口径（ropey unicode_lines 把它当行界）
    let mut d = core_with("\r");
    d.cursor = CursorPos { line: 1, col: 0 };
    assert!(d.delete_current_lines());
    assert_eq!(d.doc.to_text(), "");
}

#[test]
fn delete_selection_spans_lines_and_col0_end_excludes_last_line() {
    let mut c = core_with("l1\nl2\nl3\nl4");
    // 选区从 l1 中间拉到 l3 行首：l3 不算触及（选区没盖到它的字符）
    c.anchor = Some(CursorPos { line: 0, col: 2 });
    c.cursor = CursorPos { line: 2, col: 0 };
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "l3\nl4");
}

#[test]
fn duplicate_line_inserts_copy_below_and_is_undoable() {
    let mut c = core_with("one\ntwo");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.duplicate_current_lines());
    assert_eq!(c.doc.to_text(), "one\none\ntwo");
    assert_eq!(c.cursor.line, 1, "光标落到副本首行");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "one\ntwo");
}

#[test]
fn duplicate_tail_line_without_newline_terminates_copy_properly() {
    // 尾块本身无行尾：副本必须独立成行（先补主导行尾）
    let mut c = core_with("one\ntail");
    c.cursor = CursorPos { line: 1, col: 1 };
    assert!(c.duplicate_current_lines());
    assert_eq!(c.doc.to_text(), "one\ntail\ntail");
    // 空文档 / 幻影末行：无内容可复制
    let mut empty = core_with("");
    assert!(!empty.duplicate_current_lines());
}

#[test]
fn duplicate_last_real_line_of_trailing_newline_doc_no_blank_line() {
    // P80 勘误（第 60 轮书签测试揪出）：文档以换行收尾时，末真实行
    // 的块文本自带行尾，旧判定 end<len 误判「块无行尾」误补行尾，
    // 副本前凭空多出一个空行。钉死正确形态防回退。
    let mut c = core_with("one\ntwo\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    assert!(c.duplicate_current_lines());
    assert_eq!(c.doc.to_text(), "one\ntwo\ntwo\n");
    assert_eq!(c.cursor.line, 2, "光标落到副本首行");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "one\ntwo\n");
    // 末真实行为空行时同样成立（复制出独立空行，而非两个）
    let mut d = core_with("x\n\n");
    d.cursor = CursorPos { line: 1, col: 0 };
    assert!(d.duplicate_current_lines());
    assert_eq!(d.doc.to_text(), "x\n\n\n");
}

#[test]
fn move_line_swaps_with_neighbor_respects_boundaries_and_undo() {
    let mut c = core_with("a\nb\nc");
    c.cursor = CursorPos { line: 1, col: 0 };
    assert!(c.move_current_lines(true));
    assert_eq!(c.doc.to_text(), "b\na\nc");
    assert_eq!(c.cursor.line, 0);
    assert!(!c.move_current_lines(true), "已在顶行不得再动");
    assert!(c.move_current_lines(false));
    assert_eq!(c.doc.to_text(), "a\nb\nc");
    assert!(c.move_current_lines(false));
    assert_eq!(c.doc.to_text(), "a\nc\nb");
    assert!(!c.move_current_lines(false), "已在底行不得再动");
    // 边界 no-op 不产生快照：一步撤销恰好回到三行换位前
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "a\nb\nc");
}

#[test]
fn move_multi_line_block_rotates_as_a_unit_preserving_order() {
    let mut c = core_with("p\nq1\nq2\nr");
    c.anchor = Some(CursorPos { line: 1, col: 0 });
    c.cursor = CursorPos { line: 2, col: 1 };
    assert!(c.move_current_lines(true));
    assert_eq!(c.doc.to_text(), "q1\nq2\np\nr", "上移=整块与上一行换位");
    assert!(c.undo());
    assert!(c.move_current_lines(false));
    assert_eq!(c.doc.to_text(), "p\nr\nq1\nq2", "下移=整块与下一行换位");
}

#[test]
fn move_line_rebuilds_with_dominant_crlf_and_keeps_col() {
    let mut c = core_with("abcdef\r\nsecond\r\nc");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.move_current_lines(false));
    assert_eq!(
        c.doc.to_text(),
        "second\r\nabcdef\r\nc",
        "CRLF 文档换位不产生混合行尾"
    );
    assert_eq!(c.cursor, CursorPos { line: 1, col: 3 }, "光标列尽量保持");
}

// ---------- 大小写转换与行首尾清理（第 58 轮） ----------

#[test]
fn convert_case_selection_scoped_and_crlf_preserved() {
    let mut c = core_with("abc\r\nDef G");
    // 跨行选区：第一行 col2 起盖到第二行 col3（"c\r\nDef"）
    c.anchor = Some(CursorPos { line: 0, col: 2 });
    c.cursor = CursorPos { line: 1, col: 3 };
    assert!(c.convert_case(CaseKind::Upper));
    // 只有选区内字符变大写；换行单元字节原样保留
    assert_eq!(c.doc.to_text(), "abC\r\nDEF G");
    assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "主导行尾元数据不变");
    // 转换结果保持选中：起点不动、终点按新偏移落位
    assert_eq!(c.anchor, Some(CursorPos { line: 0, col: 2 }));
    assert_eq!(
        c.cursor,
        CursorPos { line: 1, col: 3 },
        "终点=新文本结束处（'DEF'后）"
    );
    // 撤销逐字节还原
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "abc\r\nDef G");
}

#[test]
fn convert_case_without_selection_transforms_whole_document_idempotent_no_snapshot() {
    let mut c = core_with("abc\n123 中文\nx");
    c.cursor = CursorPos { line: 1, col: 1 };
    let depth = c.undo_stack.len();
    assert!(c.convert_case(CaseKind::Upper));
    assert_eq!(
        c.doc.to_text(),
        "ABC\n123 中文\nX",
        "数字/CJK 不参与大小写映射"
    );
    assert_eq!(c.cursor, CursorPos { line: 1, col: 1 }, "光标行列尽量保持");
    // 幂等 no-op：再转一次不动、不追加快照
    assert!(!c.convert_case(CaseKind::Upper));
    assert_eq!(c.undo_stack.len(), depth + 1, "幂等调用不产生撤销快照");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "abc\n123 中文\nx");
}

#[test]
fn convert_case_expanding_mapping_keeps_selection_over_result_and_undo_exact() {
    // ß→SS 是 Unicode 全量映射的加长案例：选区终点必须按新长度重算
    let mut c = core_with("aß b");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 2 }; // 选区 = "aß"
    assert!(c.convert_case(CaseKind::Upper));
    assert_eq!(c.doc.to_text(), "ASS b");
    assert_eq!(
        c.cursor,
        CursorPos { line: 0, col: 3 },
        "终点按 SS 新长度落位"
    );
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "aß b", "加长映射撤销后逐字节还原");
    // İ→i̇（i+U+0307 组合点）同样加长，且绝不产生换行
    let mut t = core_with("İX");
    assert!(t.convert_case(CaseKind::Lower));
    assert_eq!(t.doc.to_text(), "i\u{307}x");
    assert_eq!(t.doc.line_count(), 1);
}

#[test]
fn trim_modes_strip_unicode_whitespace_on_whole_document() {
    // 全角空格 U+3000 / NBSP / Tab 都属 Unicode White_Space 口径
    let mut c = core_with("\u{3000}a b\u{a0}\r\n\t中 文  \r\n  尾无行尾 ");
    assert!(c.trim_touched_lines(TrimMode::Trailing));
    assert_eq!(
        c.doc.to_text(),
        "\u{3000}a b\r\n\t中 文\r\n  尾无行尾",
        "去尾不动行首；文档末行无行尾的形态保持"
    );
    // 去首：LF 主导文档重建后统一主导行尾（P73 行操作同哲学）
    let mut c2 = core_with("  x\ty\r\n\t\tz  \n W");
    assert!(c2.trim_touched_lines(TrimMode::Leading));
    assert_eq!(c2.doc.to_text(), "x\ty\nz  \nW");
    // 两端都去
    let mut c3 = core_with("  a  \r\n\tb\t");
    assert!(c3.trim_touched_lines(TrimMode::Both));
    assert_eq!(c3.doc.to_text(), "a\r\nb");
}

#[test]
fn trim_selection_scope_only_touched_lines() {
    let mut c = core_with("  a\r\n  b\r\n  c");
    // 选区从第 1 行行首拉到第 2 行中间：触及行=0..=1，第 3 行不得波及
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 1, col: 2 };
    assert!(c.trim_touched_lines(TrimMode::Leading));
    assert_eq!(c.doc.to_text(), "a\r\nb\r\n  c");
}

#[test]
fn trim_noop_empty_and_phantom_edge_cases() {
    // 空文档：无可清理
    let mut c = core_with("");
    assert!(!c.trim_touched_lines(TrimMode::Both));
    // 全干净文档：no-op 不产快照
    let mut d = core_with("clean\r\ntoo");
    let depth = d.undo_stack.len();
    assert!(!d.trim_touched_lines(TrimMode::Both));
    assert_eq!(d.undo_stack.len(), depth, "幂等调用不产生撤销快照");
    // 幻影末行参与清理但「尾随换行」形态保持
    let mut e = core_with("x  \r\n");
    assert!(e.trim_touched_lines(TrimMode::Trailing));
    assert_eq!(e.doc.to_text(), "x\r\n");
    assert!(e.undo());
    assert_eq!(e.doc.to_text(), "x  \r\n");
}

// ---------- 第 59 轮：行排序与去重 ----------

#[test]
fn sort_lines_whole_document_both_directions_and_undo() {
    // 码点序大小写敏感：'B'(0x42) < 'a'(0x61)
    let mut c = core_with("pear\napple\nBanana");
    let depth = c.undo_stack.len();
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(c.doc.to_text(), "Banana\napple\npear");
    assert_eq!(c.doc.line_count(), 3, "行数不变");
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.sort_lines(SortOrder::Descending));
    assert_eq!(
        c.doc.to_text(),
        "pear\napple\nBanana",
        "对已升序文档降序=精确倒转"
    );
    assert_eq!(
        c.cursor,
        CursorPos { line: 0, col: 1 },
        "光标按索引钳回触及块内、列尽量保持"
    );
    // 撤销逐步逐字节还原
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "Banana\napple\npear");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "pear\napple\nBanana");
    assert_eq!(c.undo_stack.len(), depth);
}

#[test]
fn sort_lines_already_sorted_is_idempotent_no_snapshot() {
    let mut c = core_with("a\nb\nc");
    let depth = c.undo_stack.len();
    assert!(!c.sort_lines(SortOrder::Ascending), "已升序不得重复产快照");
    assert_eq!(c.undo_stack.len(), depth);
    // 单行 / 空文档 / 纯幻影：无从排序
    let mut one = core_with("solo");
    assert!(!one.sort_lines(SortOrder::Descending));
    let mut empty = core_with("");
    assert!(!empty.sort_lines(SortOrder::Ascending));
    let mut blank = core_with("\n");
    assert!(!blank.sort_lines(SortOrder::Ascending));
    assert_eq!(blank.doc.to_text(), "\n", "纯幻影文档原样保留");
}

#[test]
fn sort_lines_selection_scope_excludes_outside_lines() {
    let mut c = core_with("zeta\nalpha\nmid\nbeta\nlast");
    // 选区盖到第 2~4 行（0 起 1..=3）：首尾两行不得波及
    c.anchor = Some(CursorPos { line: 1, col: 0 });
    c.cursor = CursorPos { line: 3, col: 2 };
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(c.doc.to_text(), "zeta\nalpha\nbeta\nmid\nlast");
}

#[test]
fn sort_lines_preserves_trailing_shape_and_rebuilds_dominant_eol() {
    // 文档以换行收尾：幻影末行不参与排序，尾随换行仍留在文档末尾
    let mut c = core_with("b\r\n\r\na\r\n");
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(
        c.doc.to_text(),
        "\r\na\r\nb\r\n",
        "空行为真实内容排到最前，尾随换行形态保持"
    );
    // 无尾随换行的文档末行：排序后仍不以换行收尾
    let mut e = core_with("a\nzz\nb");
    assert!(e.sort_lines(SortOrder::Descending));
    assert_eq!(e.doc.to_text(), "zz\nb\na", "末行无换行的形态保持");
    // 混合行尾经重建归一到主导行尾（P9 同哲学）
    let mut f = core_with("b\r\nA\na\r\n");
    assert!(f.sort_lines(SortOrder::Ascending));
    assert_eq!(
        f.doc.to_text(),
        "A\r\na\r\nb\r\n",
        "混合行尾统一为主导 CRLF"
    );
    assert_eq!(f.doc.line_ending(), LineEnding::CrLf);
}

#[test]
fn sort_orders_by_codepoint_including_emoji_and_cjk() {
    // 4 字节 emoji（U+1F600）> CJK（U+4E2D）> ASCII，按码点一字节不乱
    let mut c = core_with("😀\n中\nA\na");
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(c.doc.to_text(), "A\na\n中\n😀");
    assert!(c.sort_lines(SortOrder::Descending));
    assert_eq!(c.doc.to_text(), "😀\n中\na\nA");
}

#[test]
fn dedupe_keeps_first_occurrence_preserving_order_and_undo() {
    let mut c = core_with("b\na\nb\nc\na\nb");
    let depth = c.undo_stack.len();
    assert!(c.remove_duplicate_lines());
    assert_eq!(c.doc.to_text(), "b\na\nc", "保留首次出现，其余相对次序不变");
    assert_eq!(c.cursor.line, 0, "光标钳回块内");
    assert_eq!(c.undo_stack.len(), depth + 1, "单次去重只产一个快照");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "b\na\nb\nc\na\nb");
    assert_eq!(c.undo_stack.len(), depth, "撤销后栈深回落");
    // 文档末尾无换行且重复发生在末行：收尾形态保持
    let mut d = core_with("x\nx");
    assert!(d.remove_duplicate_lines());
    assert_eq!(d.doc.to_text(), "x");
}

#[test]
fn dedupe_selection_scope_and_crlf_rebuild() {
    let mut c = core_with("keep\r\nDUP\r\nDUP\r\nDUP\r\nkeep2");
    // 选区只盖中间三行 DUP（1..=3）
    c.anchor = Some(CursorPos { line: 1, col: 1 });
    c.cursor = CursorPos { line: 3, col: 1 };
    assert!(c.remove_duplicate_lines());
    assert_eq!(
        c.doc.to_text(),
        "keep\r\nDUP\r\nkeep2",
        "范围外行不波及，CRLF 整单元重建"
    );
    assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "主导行尾元数据不变");
}

#[test]
fn dedupe_noop_unique_or_single_line_no_snapshot() {
    let mut c = core_with("one\ntwo\nthree");
    let depth = c.undo_stack.len();
    assert!(!c.remove_duplicate_lines(), "无重复不得产快照");
    assert_eq!(c.undo_stack.len(), depth);
    let mut one = core_with("only");
    assert!(!one.remove_duplicate_lines());
    let mut empty = core_with("");
    assert!(!empty.remove_duplicate_lines());
}

#[test]
fn sort_dedupe_on_phantom_line_selection_is_safe_noop() {
    // 幻影排除的守卫路径：选区恰好只有幻影末行 / 实线+幻影行首时
    // 不得整数下溢，一律安全 no-op
    let mut c = core_with("a\n");
    c.cursor = CursorPos { line: 1, col: 0 }; // 光标停在幻影行
    assert!(!c.sort_lines(SortOrder::Ascending));
    assert!(!c.remove_duplicate_lines());
    assert_eq!(c.doc.to_text(), "a\n", "文档原样保留");
    let mut d = core_with("x\ny\n");
    d.anchor = Some(CursorPos { line: 1, col: 0 });
    d.cursor = CursorPos { line: 2, col: 0 }; // 触及块=y 行（幻影行首不算触及）
    assert!(!d.sort_lines(SortOrder::Descending));
    assert!(!d.remove_duplicate_lines());
    assert_eq!(d.doc.to_text(), "x\ny\n");
}

#[test]
fn lone_carriage_return_is_a_line_break_in_ropey_terms() {
    // 主线 A（第 59 轮）口径钉子：审查中曾假设「孤立 \r 是内容、行操作
    // 吞它是缺陷」，实测证伪——ropey 把 `\r\n` / `\n` / 孤立 `\r` 都
    // 视为换行单元，P73/P75 剥离口径正确。本测试钉死该口径防回退。
    // ① 文档以孤立 \r 收尾：多出一个幻影空行，排序不得把它当真实
    //    内容搬到最前；尾随换行形态保持（归一到主导行尾；本文档
    //    CRLF×2 > 孤立\r×1，主导=CRLF）
    let mut c = core_with("c\r\nb\r\na\r");
    assert_eq!(c.doc.line_count(), 4, "孤立 \\r 是换行：其后是幻影空行");
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(
        c.doc.to_text(),
        "a\r\nb\r\nc\r\n",
        "幻影不参与排序，尾随换行保持"
    );
    // ② 行移动沿用同一剥离口径（P73 原行为不变）
    let mut d = core_with("p\r\nq\r\nz\r");
    d.cursor = CursorPos { line: 2, col: 0 };
    assert!(d.move_current_lines(true));
    assert_eq!(
        d.doc.to_text(),
        "p\r\nz\r\nq",
        "末行以孤立 \\r 收尾=有行尾的普通行"
    );
    // ③ 清理：去首只动行首；混合行尾块经主导行尾重建（P9 口径）
    let mut e = core_with(" x\nabc\r");
    assert!(e.trim_touched_lines(TrimMode::Leading));
    assert_eq!(e.doc.to_text(), "x\nabc\n");
    // ④ 去重逐字节比对行内容（不含换行单元）
    let mut f = core_with("dup\r\nx\r\ndup");
    assert!(f.remove_duplicate_lines());
    assert_eq!(f.doc.to_text(), "dup\r\nx", "去重同样逐字节保留行内容");
}

// ---------- 第 60 轮：书签套件 ----------

#[test]
fn bookmark_toggle_and_query_basics() {
    let mut c = core_with("alpha\nbeta\ngamma\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    assert!(c.toggle_bookmark());
    assert!(c.is_bookmarked(1));
    assert_eq!(c.bookmarked_lines(), vec![1]);
    // 再按一次 = 摘除
    assert!(!c.toggle_bookmark());
    assert!(!c.is_bookmarked(1));
    assert!(c.bookmarked_lines().is_empty());
    // 多行标记仍按升序返回
    c.cursor = CursorPos { line: 2, col: 0 };
    c.toggle_bookmark();
    c.cursor = CursorPos { line: 0, col: 0 };
    c.toggle_bookmark();
    assert_eq!(c.bookmarked_lines(), vec![0, 2]);
}

#[test]
fn bookmark_next_prev_wrap_and_stay() {
    let mut c = core_with("l0\nl1\nl2\nl3\nl4\n");
    // 无书签：跳转 no-op
    assert!(!c.next_bookmark(true));
    assert_eq!(c.cursor.line, 0);
    c.cursor = CursorPos { line: 1, col: 3 };
    c.toggle_bookmark(); // bm {1}
    c.cursor = CursorPos { line: 3, col: 0 };
    c.toggle_bookmark(); // bm {1,3}
                         // 向后：3 → 回绕 1，落行首、清选区
    assert!(c.next_bookmark(true));
    assert_eq!(c.cursor.line, 1);
    assert_eq!(c.cursor.col, 0, "跳转落行首");
    assert!(c.anchor.is_none());
    // 再向后：1 → 3（正常前进）
    assert!(c.next_bookmark(true));
    assert_eq!(c.cursor.line, 3);
    // 向前：3 → 1；再向前：1 → 无更前 → 回绕 3
    assert!(c.next_bookmark(false));
    assert_eq!(c.cursor.line, 1);
    assert!(c.next_bookmark(false));
    assert_eq!(c.cursor.line, 3);
    // 唯一书签恰为当前行：两个方向都原地不动
    c.clear_bookmarks();
    c.cursor = CursorPos { line: 2, col: 1 };
    c.toggle_bookmark(); // bm {2}，光标就在行 2
    assert!(!c.next_bookmark(true));
    assert_eq!(c.cursor.line, 2);
    assert!(!c.next_bookmark(false));
    assert_eq!(c.cursor.line, 2);
    // 清空后再跳 no-op；空集合清除幂等 false
    assert!(c.clear_bookmarks());
    assert!(!c.next_bookmark(true));
    assert!(!c.clear_bookmarks());
}

#[test]
fn bookmark_toggle_enters_undo_history() {
    let mut c = core_with("a\nb\n");
    c.cursor = CursorPos { line: 0, col: 0 };
    c.toggle_bookmark(); // 快照1 {bm:{}} → bm {0}
    c.insert_str("x"); // 快照2 {bm:{0}} → "xa\nb\n"
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark(); // 快照3 {bm:{0}} → bm {0,1}
    assert_eq!(c.bookmarked_lines(), vec![0, 1]);
    // 撤销书签开关：bm 回到 {0}（开关本身可撤销）
    c.undo();
    assert_eq!(c.bookmarked_lines(), vec![0]);
    // 撤销打字：打字快照同样携带书签，bm 保持 {0}
    c.undo();
    assert_eq!(c.bookmarked_lines(), vec![0]);
    assert_eq!(c.doc.to_text(), "a\nb\n");
    // 撤销第一个开关：bm 回到空
    c.undo();
    assert!(c.bookmarked_lines().is_empty());
    assert!(!c.undo(), "撤销栈已空");
}

#[test]
fn copy_marked_lines_order_dominant_eol_and_trailing_newline() {
    let mut c = core_with("one\r\ntwo\r\nthree\r\n"); // CRLF 主导
    c.cursor = CursorPos { line: 2, col: 0 };
    c.toggle_bookmark();
    c.cursor = CursorPos { line: 0, col: 0 };
    c.toggle_bookmark();
    // 升序输出、主导行尾、每行带行尾（粘贴到他处保持整行语义）
    assert_eq!(c.copy_bookmarked_lines().unwrap(), "one\r\nthree\r\n");
    // LF 主导 + emoji/CJK 行正文原样
    let mut d = core_with("🦀\n中\nx\n");
    d.cursor = CursorPos { line: 1, col: 0 };
    d.toggle_bookmark();
    assert_eq!(d.copy_bookmarked_lines().unwrap(), "中\n");
    // 无书签 None；空文档唯一空行被标 = 空正文行仍产行尾
    assert!(core_with("a\n").copy_bookmarked_lines().is_none());
    let mut e = core_with("");
    e.toggle_bookmark();
    assert_eq!(e.copy_bookmarked_lines().unwrap(), "\n");
}

#[test]
fn remove_marked_lines_disjoint_runs_and_undo_restores() {
    let mut c = core_with("a\nb\nc\nd\ne\nf\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark(); // 1
    c.cursor = CursorPos { line: 3, col: 0 };
    c.toggle_bookmark(); // 3（与 4 连续成段）
    c.cursor = CursorPos { line: 4, col: 0 };
    c.toggle_bookmark(); // bm {1,3,4}
    assert!(c.remove_bookmarked_lines());
    assert_eq!(c.doc.to_text(), "a\nc\nf\n", "不连续段整段删除");
    assert!(c.bookmarked_lines().is_empty(), "被删的正是全部书签行");
    assert_eq!(c.cursor.line, 1, "光标落在首个被删段起点");
    // 撤销：文本与书签一并找回
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "a\nb\nc\nd\ne\nf\n");
    assert_eq!(c.bookmarked_lines(), vec![1, 3, 4]);
    // 无书签 no-op 不产快照
    assert!(c.clear_bookmarks());
    let depth = c.undo_stack.len();
    assert!(!c.remove_bookmarked_lines());
    assert_eq!(c.undo_stack.len(), depth, "no-op 不得入栈");
}

#[test]
fn remove_marked_lines_crlf_last_real_line_and_phantom() {
    // 末真实行被标：块区间含其行尾，删除后前文的尾随换行保持
    //（与 Ctrl+L 同区间口径）
    let mut c = core_with("1\r\n2\r\n3\r\n");
    c.cursor = CursorPos { line: 2, col: 0 };
    c.toggle_bookmark();
    assert!(c.remove_bookmarked_lines());
    assert_eq!(c.doc.to_text(), "1\r\n2\r\n");
    // 幻影末行单独被标：退化为吃掉前面的换行单元
    let mut d = core_with("x\ny\n");
    d.cursor = CursorPos { line: 2, col: 0 };
    d.toggle_bookmark();
    assert!(d.remove_bookmarked_lines());
    assert_eq!(d.doc.to_text(), "x\ny");
    // 空文档唯一空行被标：无可删
    let mut e = core_with("");
    e.toggle_bookmark();
    assert!(!e.remove_bookmarked_lines());
    assert_eq!(e.doc.to_text(), "");
    // 文档恰为一个换行符：幻影末行被标时 start == 1，不得回看
    // start-2（旧实现 usize 下溢崩溃），退化为吃掉唯一换行单元
    let mut f = core_with("\n");
    f.cursor = CursorPos { line: 1, col: 0 };
    f.toggle_bookmark();
    assert!(f.remove_bookmarked_lines());
    assert_eq!(f.doc.to_text(), "");
}

#[test]
fn bookmark_remap_on_typing_newlines_and_selection_replace() {
    // 上方插行：书签下移
    let mut c = core_with("a\nb\nc\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark(); // bm{1}
    c.cursor = CursorPos { line: 0, col: 0 };
    c.insert_str("x\n");
    assert_eq!(c.doc.to_text(), "x\na\nb\nc\n");
    assert_eq!(c.bookmarked_lines(), vec![2]);
    // 下方插行：不动
    c.cursor = CursorPos { line: 4, col: 0 };
    c.insert_str("\n");
    assert_eq!(c.bookmarked_lines(), vec![2]);
    // 跨行选区替换变体 A：选区含终点行首字符（r3 后缀幸存）
    // → 终点行书签并入结果行，被替换内容（r1）的书签消失
    let mut d = core_with("r0\nr1\nr2\nr3\nr4\n");
    d.cursor = CursorPos { line: 1, col: 0 };
    d.toggle_bookmark(); // r1（将被替换掉的内容）
    d.cursor = CursorPos { line: 3, col: 0 };
    d.toggle_bookmark(); // r3（后缀幸存）
    d.cursor = CursorPos { line: 1, col: 0 };
    d.anchor = Some(CursorPos { line: 3, col: 1 });
    d.insert_str("Z");
    assert_eq!(d.doc.to_text(), "r0\nZ3\nr4\n");
    assert_eq!(
        d.bookmarked_lines(),
        vec![1],
        "r3 的书签并入结果行，r1 的随内容消失"
    );
    // 变体 B：选区止于终点行行首（该行不算触及、整行并入结果行）
    // → 其书签同样并入结果行；起点行整行被吃 → 自身书签丢弃
    let mut e = core_with("r0\nr1\nr2\nr3\nr4\n");
    e.cursor = CursorPos { line: 1, col: 0 };
    e.toggle_bookmark(); // r1
    e.cursor = CursorPos { line: 3, col: 0 };
    e.toggle_bookmark(); // r3
    e.cursor = CursorPos { line: 1, col: 0 };
    e.anchor = Some(CursorPos { line: 3, col: 0 });
    e.insert_str("Z");
    assert_eq!(e.doc.to_text(), "r0\nZr3\nr4\n");
    assert_eq!(
        e.bookmarked_lines(),
        vec![1],
        "r3 整行并入结果行 → 书签并入行1；r1 随内容消失"
    );
}

#[test]
fn bookmark_remap_on_line_suite_ops() {
    // 删行（选区跨行）：区间内书签消失，下方上移
    let mut c = core_with("0\n1\n2\n3\n4\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark();
    c.cursor = CursorPos { line: 4, col: 0 };
    c.toggle_bookmark(); // bm{1,4}
    c.cursor = CursorPos { line: 0, col: 0 };
    c.anchor = Some(CursorPos { line: 2, col: 1 });
    assert!(c.delete_current_lines());
    assert_eq!(c.doc.to_text(), "3\n4\n");
    assert_eq!(
        c.bookmarked_lines(),
        vec![1],
        "行1 书签随行消失，行4 上移到行1"
    );
    // 复制行：下方书签下推，副本不带书签
    let mut d = core_with("a\nb\nc\n");
    d.cursor = CursorPos { line: 0, col: 0 };
    d.toggle_bookmark();
    d.cursor = CursorPos { line: 2, col: 0 };
    d.toggle_bookmark(); // bm{0,2}
    assert!(d.duplicate_current_lines());
    assert_eq!(d.doc.to_text(), "a\nb\nc\nc\n");
    assert_eq!(d.bookmarked_lines(), vec![0, 2], "副本不带书签");
    // 移行：块内书签随内容轮转
    let mut e = core_with("p\nq\nr\ns\n");
    e.cursor = CursorPos { line: 2, col: 0 };
    e.toggle_bookmark(); // bm{2}（r）
    e.cursor = CursorPos { line: 1, col: 0 };
    assert!(e.move_current_lines(false)); // q 下移：p r q s
    assert_eq!(e.doc.to_text(), "p\nr\nq\ns\n");
    assert_eq!(e.bookmarked_lines(), vec![1], "r 的书签随内容上移");
    assert!(e.move_current_lines(true)); // r 上移回：p q r s
    assert_eq!(e.doc.to_text(), "p\nq\nr\ns\n");
    assert_eq!(e.bookmarked_lines(), vec![2]);
    // 相邻行书签换入块尾（上移时上行书签落到块尾位置）
    let mut f = core_with("p\nq\nr\n");
    f.cursor = CursorPos { line: 0, col: 0 };
    f.toggle_bookmark(); // bm{0}（p）
    f.cursor = CursorPos { line: 1, col: 0 };
    assert!(f.move_current_lines(true)); // q 上移：q p r
    assert_eq!(f.doc.to_text(), "q\np\nr\n");
    assert_eq!(f.bookmarked_lines(), vec![1], "p 的书签随内容下移");
}

#[test]
fn bookmark_remap_on_sort_and_dedup() {
    // 排序：书签跟随行内容（稳定排序排列反演映射）
    let mut c = core_with("c\na\nb\n");
    c.cursor = CursorPos { line: 0, col: 0 };
    c.toggle_bookmark(); // c 行
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(c.doc.to_text(), "a\nb\nc\n");
    assert_eq!(c.bookmarked_lines(), vec![2], "书签跟着 c 行走");
    assert!(c.sort_lines(SortOrder::Descending));
    assert_eq!(c.doc.to_text(), "c\nb\na\n");
    assert_eq!(c.bookmarked_lines(), vec![0]);
    // 去重：重复项的书签随行丢弃；首现保留项书签原位
    let mut d = core_with("x\ny\nx\nx\n");
    d.cursor = CursorPos { line: 2, col: 0 };
    d.toggle_bookmark(); // 第二个 x（重复项）
    assert!(d.remove_duplicate_lines());
    assert_eq!(d.doc.to_text(), "x\ny\n");
    assert!(d.bookmarked_lines().is_empty(), "重复项书签随行丢弃");
    let mut e = core_with("x\ny\nx\n");
    e.cursor = CursorPos { line: 0, col: 0 };
    e.toggle_bookmark(); // 首个 x（保留项）
    assert!(e.remove_duplicate_lines());
    assert_eq!(e.doc.to_text(), "x\ny\n");
    assert_eq!(e.bookmarked_lines(), vec![0], "保留项书签原位");
    // 幻影末行（块外）书签不受排序影响
    let mut f = core_with("b\na\n");
    f.cursor = CursorPos { line: 2, col: 0 };
    f.toggle_bookmark(); // 幻影行
    assert!(f.sort_lines(SortOrder::Ascending));
    assert_eq!(f.doc.to_text(), "a\nb\n");
    assert_eq!(f.bookmarked_lines(), vec![2], "幻影行号恒不变");
}

#[test]
fn bookmark_remap_on_merge_ops_and_replace_all() {
    // 回退并行：两行书签取并集落在幸存行
    let mut c = core_with("a\nb\n");
    c.cursor = CursorPos { line: 0, col: 0 };
    c.toggle_bookmark();
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark(); // bm{0,1}
    c.cursor = CursorPos { line: 1, col: 0 };
    c.backspace();
    assert_eq!(c.doc.to_text(), "ab\n", "b 并上行 a，幻影末行保持");
    assert_eq!(c.bookmarked_lines(), vec![0], "并集落在幸存行");
    // 行尾 Delete 吞换行：下一行书签并入当前行
    let mut d = core_with("a\nb\n");
    d.cursor = CursorPos { line: 0, col: 1 };
    d.toggle_bookmark();
    d.delete_forward();
    assert_eq!(d.doc.to_text(), "ab\n");
    assert_eq!(d.bookmarked_lines(), vec![0]);
    // 行中 Delete 不改结构：书签不动
    let mut e = core_with("ab\ncd\n");
    e.cursor = CursorPos { line: 1, col: 0 };
    e.toggle_bookmark();
    e.cursor = CursorPos { line: 0, col: 0 };
    e.delete_forward();
    assert_eq!(e.doc.to_text(), "b\ncd\n");
    assert_eq!(e.bookmarked_lines(), vec![1]);
    // 全部替换：书签清空，撤销经快照找回
    let mut f = core_with("a\nb\n");
    f.cursor = CursorPos { line: 1, col: 0 };
    f.toggle_bookmark();
    f.replace_whole_document(editpad_core::Document::from_str("zz\n"));
    assert!(f.bookmarked_lines().is_empty());
    assert!(f.undo());
    assert_eq!(f.bookmarked_lines(), vec![1], "撤销找回整体替换前的书签");
    // 加载新文档（reset）：书签清空且撤销链已清
    let mut g = core_with("a\nb\n");
    g.cursor = CursorPos { line: 1, col: 0 };
    g.toggle_bookmark();
    g.reset_document(editpad_core::Document::from_str("q\n"));
    assert!(g.bookmarked_lines().is_empty());
}

#[test]
fn bookmark_gutter_reserves_strip_without_moving_line_numbers() {
    // 第 60 轮：gutter = 书签条带 + 间距 + 数字宽。条带加在左侧，
    // 行号右缘 = gutter − GUTTER_MIN 的 P66附 对齐契约不受影响
    //（数字盒右缘相对 gutter 右缘的偏移公式未动）。
    let c = core_with("a\nb\nc\n"); // 行数 1 位 → 取 max(3) 位
    let expect = BOOKMARK_STRIP + GUTTER_MIN + 3.0 * c.char_width();
    assert!((c.gutter_width() - expect).abs() < 1e-3, "gutter 公式漂移");
}

// ---------- 第 62 轮：行操作扩充（Tab↔空格 / 合并拆分 / 删空行） ----------

#[test]
fn tab_space_pure_helpers_match_render_tab_stops() {
    // detab：与渲染同源（TAB_STOP_COLS=4、宽字符计 2 显示列）
    assert_eq!(expand_tabs_in("\t\tX", true), "        X");
    assert_eq!(
        expand_tabs_in(" \tX", true),
        "    X",
        "悬置空格后的 Tab 补齐到下一制表位"
    );
    assert_eq!(expand_tabs_in("a\tb", true), "a\tb", "行首模式不碰行中 Tab");
    assert_eq!(
        expand_tabs_in("a\tb", false),
        "a   b",
        "全部模式按显示列展开"
    );
    assert_eq!(
        expand_tabs_in("中\tX", false),
        "中  X",
        "宽字符占 2 列，Tab 只需补 2 格到列 4"
    );
    // entab：只有「恰好到制表位且攒够 ≥2 格」才收拢成 Tab
    assert_eq!(entab_leading_ws("    X"), "\tX");
    assert_eq!(entab_leading_ws("   X"), "   X", "不足一档保持原样（幂等）");
    assert_eq!(
        entab_leading_ws("      X"),
        "\t  X",
        "收一档、余两格悬置保留"
    );
    assert_eq!(
        entab_leading_ws(" \t X"),
        " \t X",
        "既有 Tab 前后的零星空格不收拢（不破坏既有对齐）"
    );
    assert_eq!(
        entab_leading_ws("\u{3000}X"),
        "\u{3000}X",
        "全角空格等其他空白不参与收拢"
    );
    assert_eq!(entab_leading_ws("  "), "  ", "整行短空白原样");
}

#[test]
fn convert_tabs_spaces_scopes_noop_and_bookmarks() {
    // 有选区只转触及行
    let mut c = core_with("\tX\nY\tZ\n");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
    assert_eq!(c.doc.to_text(), "    X\nY\tZ\n", "范围外行不被波及");
    // 无选区转全文
    assert!(c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
    assert_eq!(c.doc.to_text(), "    X\nY   Z\n");
    // 幂等 no-op 不产快照（第二次已无 Tab）
    let snaps = c.undo_stack.len();
    assert!(!c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
    assert_eq!(c.undo_stack.len(), snaps, "no-op 不得产快照");
    // 行数不变的恒等映射：书签原位保留
    let mut d = core_with("\ta\nb\n");
    d.cursor = CursorPos { line: 1, col: 0 };
    d.toggle_bookmark();
    assert!(d.convert_tabs_spaces(TabSpaceKind::LeadingTabsToSpaces));
    assert_eq!(d.doc.to_text(), "    a\nb\n");
    assert_eq!(d.bookmarked_lines(), vec![1], "恒等映射书签不动");
    // 空格→制表符：整档收拢、不足一档保持
    let mut e = core_with("    a\n  b\n");
    assert!(e.convert_tabs_spaces(TabSpaceKind::LeadingSpacesToTabs));
    assert_eq!(e.doc.to_text(), "\ta\n  b\n");
}

#[test]
fn merge_lines_block_next_line_and_phantom() {
    // 触及块合成一行：各段 trim 后单空格连接，空白行消失
    let mut c = core_with("alpha\n beta \n\ngamma\ndelta\n");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 2, col: 1 }; // 触及 0..=2 行
    assert!(c.merge_lines());
    assert_eq!(c.doc.to_text(), "alpha beta\ngamma\ndelta\n");
    // 无选区 = 当前行并入下一行；末行无从并 → no-op
    let mut d = core_with("one\ntwo\nthree");
    d.cursor = CursorPos { line: 1, col: 1 };
    assert!(d.merge_lines());
    assert_eq!(d.doc.to_text(), "one\ntwo three");
    assert!(!d.merge_lines(), "末行没有下一行可并");
    // 幻影末行不参与合并，尾随换行形态保持（光标落行 2 行首才触及
    // 行 0..=1——选区字节半开口径）
    let mut e = core_with("x\ny\n");
    e.anchor = Some(CursorPos { line: 0, col: 0 });
    e.cursor = CursorPos { line: 2, col: 0 };
    assert!(e.merge_lines());
    assert_eq!(e.doc.to_text(), "x y\n");
    // CRLF 文档：主导行尾重建不产混合尾
    let mut f = core_with("a\r\nb\r\n");
    f.cursor = CursorPos { line: 0, col: 0 };
    assert!(f.merge_lines());
    assert_eq!(f.doc.to_text(), "a b\r\n");
}

#[test]
fn merge_lines_bookmark_collapse_and_below_shift() {
    // 并入行的书签随内容消失；撤销经快照整体找回
    let mut c = core_with("a\nbb\nccc\nlow\n");
    c.cursor = CursorPos { line: 2, col: 0 };
    c.toggle_bookmark(); // 将被并入的行
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 2, col: 3 };
    assert!(c.merge_lines());
    assert_eq!(c.doc.to_text(), "a bb ccc\nlow\n");
    assert!(c.bookmarked_lines().is_empty(), "并入行书签丢弃");
    // 块外下方行书签随净减行数上移：low 原行 3 → 行 1
    let mut d = core_with("p\nq\nr\nlow\n");
    d.cursor = CursorPos { line: 3, col: 0 };
    d.toggle_bookmark();
    // 触及块 0..=2（光标列 >0 才「触及」第 2 行——选区字节半开口径）
    d.anchor = Some(CursorPos { line: 0, col: 0 });
    d.cursor = CursorPos { line: 2, col: 1 };
    assert!(d.merge_lines());
    assert_eq!(d.doc.to_text(), "p q r\nlow\n");
    assert_eq!(d.bookmarked_lines(), vec![1], "块外书签按行数增量平移");
    assert!(d.undo());
    assert_eq!(d.bookmarked_lines(), vec![3], "撤销整体回滚");
}

#[test]
fn split_line_cursor_selection_and_noop() {
    // 无选区：光标处断行，光标落后半段行首
    let mut c = core_with("abcd\n");
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.split_line());
    assert_eq!(c.doc.to_text(), "ab\ncd\n");
    assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });
    // 空行正中断行：与回车同款——凭空多出一个空行（不是 no-op）
    let mut d = core_with("a\n\nb\n");
    d.cursor = CursorPos { line: 1, col: 0 };
    assert!(d.split_line());
    assert_eq!(d.doc.to_text(), "a\n\n\nb\n", "原尾随换行保持");
    // 选区独立成行：前后残文各留原行
    let mut e = core_with("one two three\n");
    e.anchor = Some(CursorPos { line: 0, col: 4 });
    e.cursor = CursorPos { line: 0, col: 7 };
    assert!(e.split_line());
    assert_eq!(e.doc.to_text(), "one \ntwo\n three\n");
    assert_eq!(e.cursor, CursorPos { line: 1, col: 0 });
    // CRLF 文档用主导行尾断行
    let mut f = core_with("ab\r\ncd\r\n");
    f.cursor = CursorPos { line: 0, col: 1 };
    assert!(f.split_line());
    assert_eq!(f.doc.to_text(), "a\r\nb\r\ncd\r\n");
}

#[test]
fn split_line_bookmark_shifts_below_block() {
    // 摘出行首片段：块 [0..=0] 由 1 行变 3 行，块外 low 书签 +2
    let mut c = core_with("aXb\nlow\n");
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark();
    c.anchor = Some(CursorPos { line: 0, col: 1 });
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.split_line());
    assert_eq!(c.doc.to_text(), "a\nX\nb\nlow\n");
    assert_eq!(c.bookmarked_lines(), vec![3], "块外书签随行数增量下移");
    assert!(c.undo());
    assert_eq!(c.bookmarked_lines(), vec![1]);
}

#[test]
fn delete_empty_lines_modes_scope_and_bookmarks() {
    // 严格口径只删零字符行；幻影末行不参与、尾随换行保持
    let mut c = core_with("a\n\nb\n \n\n");
    assert!(c.delete_empty_lines(BlankKind::Empty));
    assert_eq!(c.doc.to_text(), "a\nb\n \n", "\" \" 行在严格口径下幸存");
    // 含空白口径把纯空白行一并删掉（删到文档尾：保留末行行尾形态）
    let mut d = core_with("a\n\nb\n \n\n");
    assert!(d.delete_empty_lines(BlankKind::Whitespace));
    assert_eq!(d.doc.to_text(), "a\nb\n");
    // 块夹在内容中间且全删空：必须补一个衔接换行，前后不得粘连
    let mut g = core_with("a\n\n\nb");
    assert!(g.delete_empty_lines(BlankKind::Empty));
    assert_eq!(g.doc.to_text(), "a\nb", "中段全删空补一个衔接符");
    // 选区限定作用域（光标落行 2 行首=字节止于行 1 末，触及 0..=1；
    // 半开口径：光标在行首不「触及」该行本身）
    let mut e = core_with("\n\nkeep\n\n\n");
    e.anchor = Some(CursorPos { line: 0, col: 0 });
    e.cursor = CursorPos { line: 2, col: 0 };
    assert!(e.delete_empty_lines(BlankKind::Empty));
    assert_eq!(e.doc.to_text(), "keep\n\n\n", "文档头空块全删不补衔接符");
    // 书签跟随幸存行压缩上移；无可删时 no-op
    let mut f = core_with("x\n\ny\nz\n");
    f.cursor = CursorPos { line: 2, col: 0 };
    f.toggle_bookmark(); // y 行
    assert!(f.delete_empty_lines(BlankKind::Empty));
    assert_eq!(f.doc.to_text(), "x\ny\nz\n");
    assert_eq!(f.bookmarked_lines(), vec![1], "幸存行书签随压缩上移");
    let snaps = f.undo_stack.len();
    assert!(!f.delete_empty_lines(BlankKind::Whitespace));
    assert_eq!(f.undo_stack.len(), snaps);
}

#[test]
fn selection_display_len_matches_len_chars_semantics() {
    // 单行：纯列差
    let mut c = core_with("hello world\n");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 5 };
    assert_eq!(c.selection_display_len(), Some(5));
    // 跨行 LF：换行计 1——「b+\n + cd+\n + e」= 6
    let mut d = core_with("ab\ncd\nef\n");
    d.anchor = Some(CursorPos { line: 0, col: 1 });
    d.cursor = CursorPos { line: 2, col: 1 };
    assert_eq!(
        d.selection_display_len(),
        Some(6),
        "与查找命中 len_chars 同口径"
    );
    // CRLF：\r\n 整体计 1——「b + \r\n + c」= 3
    let mut e = core_with("ab\r\ncd\r\n");
    e.anchor = Some(CursorPos { line: 0, col: 1 });
    e.cursor = CursorPos { line: 1, col: 1 };
    assert_eq!(e.selection_display_len(), Some(3));
    // 无选区 → None
    let g = core_with("abc");
    assert_eq!(g.selection_display_len(), None);
}

#[test]
fn insert_date_time_format_and_edit_pipeline() {
    let mut c = core_with("log: \n");
    c.cursor = CursorPos { line: 0, col: 5 };
    let stamp = c.insert_date_time();
    // 格式钉子：YYYY-MM-DD HH:MM（16 字符、分隔符位固定）
    assert_eq!(stamp.len(), 16, "{stamp}");
    let b = stamp.as_bytes();
    assert_eq!((b[4], b[7], b[10], b[13]), (b'-', b'-', b' ', b':'));
    assert!(stamp.as_bytes()[..16]
        .iter()
        .all(|&x| x.is_ascii_digit() || x == b'-' || x == b' ' || x == b':'));
    assert_eq!(c.doc.to_text(), format!("log: {stamp}\n"), "插在光标处");
    assert!(!c.undo_stack.is_empty(), "插入是真实编辑，产快照");
    // 撤销完整移除时间戳
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "log: \n");
}

#[test]
fn toggle_line_comment_wrap_unwrap_roundtrip() {
    // 默认语法（未设置语言）→ `//`；空行不参与
    let mut c = core_with("alpha\nbeta\n\ngamma");
    assert!(c.toggle_line_comment());
    assert_eq!(c.doc.to_text(), "// alpha\n// beta\n\n// gamma");
    // 全部已注释 → 去一层（往返幂等）
    assert!(c.toggle_line_comment());
    assert_eq!(c.doc.to_text(), "alpha\nbeta\n\ngamma");
    // 缩进保留、插在首个非空白前；混合块按「非全注释」统一加
    let mut d = core_with("  indented\nplain\n");
    assert!(d.toggle_line_comment());
    assert_eq!(d.doc.to_text(), "  // indented\n// plain\n");
    // 剥层时连紧随的一个空格一起剥，多余空格保留
    let mut e = core_with("// nospace\n//  keeps double\n");
    assert!(e.toggle_line_comment());
    assert_eq!(e.doc.to_text(), "nospace\n keeps double\n");
    // 已是注释态的判定看缩进后前缀：整块已注释才剥
    let mut f = core_with("// a\nb\n");
    assert!(f.toggle_line_comment());
    assert_eq!(f.doc.to_text(), "// // a\n// b\n");
    // 无变化 no-op 不产快照（空文档）
    let mut g = core_with("");
    assert!(!g.toggle_line_comment());
}

#[test]
fn toggle_line_comment_prefix_follows_syntax() {
    // Python → #；行数不变 → 书签原位
    let mut c = core_with("x = 1\ny = 2\n");
    c.set_language_by_name(Some("Python"));
    assert_eq!(c.highlight_syntax_name().as_deref(), Some("Python"));
    c.cursor = CursorPos { line: 1, col: 0 };
    c.toggle_bookmark();
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 1, col: 5 };
    assert!(c.toggle_line_comment());
    assert_eq!(c.doc.to_text(), "# x = 1\n# y = 2\n");
    assert_eq!(c.bookmarked_lines(), vec![1], "恒等映射书签不动");
}

#[test]
fn collect_block_keeps_last_real_row_when_selection_reaches_eof() {
    // 第 64 轮勘误回归（🟠 既有缺陷，本测试初版当场暴露）：选区触及
    // 「最后一个真实行」且文档以换行收尾时，该行曾被幻影排除误剔——
    // 排序把 a 行整行吞掉、注释切换只处理了首行。守卫 = 幻影只可能
    // 是 b == line_count()-1 那一行。
    let mut c = core_with("c\nb\na\n");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 1, col: 1 }; // 触及行 0..=1（a 行未选）
    assert!(c.sort_lines(SortOrder::Ascending));
    assert_eq!(c.doc.to_text(), "b\nc\na\n", "块外行 a 不得被吞");
    // 同一选区跑注释切换：两行都必须加上前缀
    let mut d = core_with("x = 1\ny = 2\n");
    d.set_language_by_name(Some("Python"));
    d.anchor = Some(CursorPos { line: 0, col: 0 });
    d.cursor = CursorPos { line: 1, col: 5 };
    assert!(d.toggle_line_comment());
    assert_eq!(d.doc.to_text(), "# x = 1\n# y = 2\n");
}

#[test]
fn apply_line_block_clamps_cursor_into_new_row_count() {
    // 第 66 轮勘误回归（随机对拍当场抓住）：块被压缩后旧光标行可能
    // 超出新行数域——全空白文档删空后只剩 1 行，旧光标行号必须收敛
    let mut c = core_with("\n\n\n\n");
    c.cursor = CursorPos { line: 3, col: 0 };
    assert!(c.delete_empty_lines(BlankKind::Empty));
    assert_eq!(c.doc.to_text(), "", "全空文档删空行 → 空文档");
    assert_eq!(
        c.cursor,
        CursorPos { line: 0, col: 0 },
        "光标必须收敛进新行数域"
    );
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "\n\n\n\n", "撤销完整还原");
}

// ---------- 第 67 轮 ⑮：列块编辑 ----------

// ---------- B9 Phase 1：列编辑器序号纯函数 ----------

#[test]
fn sequence_lines_formats_all_bases_with_pad_and_step() {
    // 十进制基础 + 负步长
    assert_eq!(sequence_lines(3, 5, -2, NumBase::Dec, 0, false), ["5", "3", "1"]);
    // 补零定宽（十进制）
    assert_eq!(sequence_lines(3, 1, 1, NumBase::Dec, 3, false), ["001", "002", "003"]);
    // 补零只作用于数字部分，负号在补零之外
    assert_eq!(sequence_lines(2, -3, 1, NumBase::Dec, 3, false), ["-003", "-002"]);
    // 十六进制大小写两形态
    assert_eq!(sequence_lines(2, 9, 1, NumBase::Hex, 2, false), ["09", "0a"]);
    assert_eq!(sequence_lines(2, 9, 1, NumBase::Hex, 2, true), ["09", "0A"]);
    // 二进制 / 八进制
    assert_eq!(sequence_lines(3, 0, 2, NumBase::Bin, 4, false), ["0000", "0010", "0100"]);
    assert_eq!(sequence_lines(2, 8, 1, NumBase::Oct, 2, false), ["10", "11"]);
    // i64 饱和：极值 + 正步长停在 MAX，不回绕
    assert_eq!(
        sequence_lines(2, i64::MAX, 1, NumBase::Dec, 0, false),
        [i64::MAX.to_string(), i64::MAX.to_string()]
    );
    // i64 饱和：极小值 + 负步长停在 MIN
    assert_eq!(
        sequence_lines(2, i64::MIN, -1, NumBase::Dec, 0, false),
        [i64::MIN.to_string(), i64::MIN.to_string()]
    );
    // 补零宽度钳制：要求 100 位只给 32 位
    assert_eq!(
        sequence_lines(1, 7, 1, NumBase::Dec, 100, false),
        [format!("{:0>32}", "7")]
    );
    // 零行 = 空序列
    assert!(sequence_lines(0, 1, 1, NumBase::Dec, 0, false).is_empty());
}

#[test]
fn zero_width_block_is_persistent_insertion_column() {
    // 多行零宽块（r0<r1、c0==c1）不是空块：finish 保留、active 可查询
    let mut c = core_with("abcdef\ngh\nijklm");
    c.begin_block_select(CursorPos { line: 0, col: 2 });
    c.update_block_select(CursorPos { line: 2, col: 2 });
    assert!(c.finish_block_select(), "零宽插入列松开保留");
    assert_eq!(c.active_block(), Some((0, 2, 2, 2)));

    // 复制语义：各行片段为零串（不虚构内容）
    assert_eq!(c.block_copy_text(), Some("\n\n".to_owned()));

    // 插入 = 纯列前插入（零宽段无内容可删）；列越行尾的短行钳到行尾
    // 插入（行 1 "gh" 长 2，列 2 恰在行尾 → "ghZ"）
    assert!(c.insert_into_block("Z"));
    assert_eq!(c.doc.to_text(), "abZcdef\nghZ\nijZklm");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 3 }, "光标落首行插入文本之后");
    assert!(c.undo(), "零宽插入必须可撤销");
    assert_eq!(c.doc.to_text(), "abcdef\ngh\nijklm");

    // 序号寄生路径：sequence_lines 拼多行文本走 insert_into_block，
    // 块行数 == 文本行数时 1:1 填充（1 列宽块 = 替换每行第 2 字符）
    let seq = sequence_lines(3, 10, 5, NumBase::Dec, 2, false).join("\n");
    c.begin_block_select(CursorPos { line: 0, col: 1 });
    c.update_block_select(CursorPos { line: 2, col: 2 });
    assert!(c.finish_block_select());
    assert!(c.insert_into_block(&seq));
    assert_eq!(c.doc.to_text(), "a10cdef\ng15\ni20klm");
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "abcdef\ngh\nijklm");

    // 零宽删除 = 全空预检 no-op，不产快照
    let mut d = core_with("ab\ncd");
    d.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 1 },
        head: CursorPos { line: 1, col: 1 },
    });
    let snaps = d.undo_stack.len();
    assert!(!d.delete_block_content());
    assert_eq!(d.undo_stack.len(), snaps, "零宽删除不产快照");
    assert_eq!(d.doc.to_text(), "ab\ncd");

    // 单点（r0==r1 且 c0==c1）仍为空块：finish 自动清除（P87 口径保持）
    let mut e = core_with("abcd\n");
    e.begin_block_select(CursorPos { line: 1, col: 1 });
    assert!(!e.finish_block_select(), "单点空块松开自动清除");
    assert!(!e.has_block());
}

#[test]
fn block_select_lifecycle_and_geometry() {
    let mut c = core_with("abcdef\ngh\nijklm\n");
    assert!(!c.has_block());
    // 拖拽起点 = 空块
    c.begin_block_select(CursorPos { line: 0, col: 2 });
    assert!(!c.has_block(), "单击不应形成有效块");
    // 正向拖到 (2,4)：矩形 0..2 行 × 2..4 列
    c.update_block_select(CursorPos { line: 2, col: 4 });
    assert!(c.has_block());
    assert_eq!(c.active_block(), Some((0, 2, 2, 4)));
    // 反向拖拽归一化：head(1,1) → 行 0..1 × 列 1..2
    c.begin_block_select(CursorPos { line: 0, col: 2 });
    c.update_block_select(CursorPos { line: 1, col: 1 });
    assert_eq!(c.active_block(), Some((0, 1, 1, 2)));
    // 松开保留有效块；clear 后归零；空块 finish 自动清除
    assert!(c.finish_block_select());
    assert!(c.clear_block());
    assert!(!c.has_block());
    c.begin_block_select(CursorPos { line: 1, col: 1 });
    assert!(!c.finish_block_select(), "空块松开自动清除");
    assert!(!c.has_block());
}

#[test]
fn block_copy_delete_insert_oracles() {
    let doc = "abcdef\ngh\nij";
    // 复制：行0[1..3)="bc"、行1[1..3) 截到行尾="h"
    let mut c = core_with(doc);
    c.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 1 },
        head: CursorPos { line: 1, col: 3 },
    });
    assert_eq!(c.active_block(), Some((0, 1, 1, 3)));
    assert_eq!(c.block_copy_text(), Some("bc\nh".to_owned()));

    // 删除：行0 去 [1..3)="bc"；行1 长 2，c1 截到行尾 → 只去 "h"
    assert!(c.delete_block_content());
    assert_eq!(c.doc.to_text(), "adef\ng\nij");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 1 });
    // 全区间越行（短于 c0）→ no-op 不产快照
    let snaps = c.undo_stack.len();
    c.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 9 },
        head: CursorPos { line: 1, col: 9 },
    });
    assert!(!c.delete_block_content());
    assert_eq!(c.undo_stack.len(), snaps);

    // 插入：逐行把 [c0,c1) **替换**为「Z」（行1 长 2，c1 截到行尾）
    let mut e = core_with("abc\nXY\nW");
    e.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 1 },
        head: CursorPos { line: 1, col: 2 },
    });
    assert!(e.insert_into_block("Z"));
    assert_eq!(e.doc.to_text(), "aZc\nXZ\nW");
    assert_eq!(e.cursor, CursorPos { line: 0, col: 2 });
    assert!(e.undo());
    assert_eq!(e.doc.to_text(), "abc\nXY\nW", "撤销完整还原");

    // 书签不受影响（行数不变）
    let mut f = core_with("ab\ncd\n");
    f.toggle_bookmark(); // 行 0
    f.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 0 },
        head: CursorPos { line: 1, col: 1 },
    });
    assert!(f.delete_block_content());
    assert_eq!(f.bookmarked_lines(), vec![0], "恒等结构书签原位");
}

#[test]
fn block_insert_multiline_cycles_and_extends() {
    // v2 循环填充：文本 2 行填 3 行块 → 第 3 行循环回文本第 1 行；
    // 行数一致 → 逐行对应；撤销完整还原
    let mut c = core_with("111\n222\n333");
    c.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 0 },
        head: CursorPos { line: 2, col: 1 },
    });
    assert!(c.insert_into_block("A\nB"));
    assert_eq!(
        c.doc.to_text(),
        "A11\nB22\nA33",
        "循环填充：行 2 回用文本行 0"
    );
    assert!(c.undo());
    assert_eq!(c.doc.to_text(), "111\n222\n333");

    // 逐行对应：文本 3 行填 3 行块
    let mut d = core_with("111\n222\n333");
    d.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 0 },
        head: CursorPos { line: 2, col: 1 },
    });
    assert!(d.insert_into_block("A\nB\nC"));
    assert_eq!(d.doc.to_text(), "A11\nB22\nC33");

    // 块扩展：文本 3 行填 2 行块 → 余下行插到块末行下方，
    // 下方内容与书签整体下移
    let mut e = core_with("111\n222\n尾");
    e.toggle_bookmark(); // 行 0
    e.cursor = CursorPos { line: 2, col: 0 };
    e.toggle_bookmark(); // 行 2（块末行下方，扩展后应下移）
    e.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 0 },
        head: CursorPos { line: 1, col: 1 },
    });
    assert!(e.insert_into_block("A\nB\nC"));
    assert_eq!(e.doc.to_text(), "A11\nB22\nC\n尾", "余下行插到块末行下方");
    assert_eq!(e.bookmarked_lines(), vec![0, 3], "书签随扩展下移");
    assert!(e.undo());
    assert_eq!(e.doc.to_text(), "111\n222\n尾");
    assert_eq!(e.bookmarked_lines(), vec![0, 2], "撤销连书签一并还原");

    // 块末行无行尾时扩展：先补换行单元再插新行
    let mut g = core_with("111\n222");
    g.block_sel = Some(BlockSel {
        anchor: CursorPos { line: 0, col: 0 },
        head: CursorPos { line: 1, col: 1 },
    });
    assert!(g.insert_into_block("A\nB\nC"));
    assert_eq!(g.doc.to_text(), "A11\nB22\nC\n", "文档末尾补齐行尾");
}

#[test]
fn block_cleared_by_nav_undo_and_esc_op() {
    let mk = || {
        let mut c = core_with("abcd\n");
        c.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 1 },
            head: CursorPos { line: 0, col: 3 },
        });
        c
    };
    // 键盘移动 / 全选 / select_span（查找跳转）/ undo 各自清块
    let mut c = mk();
    c.apply_motion(Motion::Right, false);
    assert!(!c.has_block(), "键盘移动退出块态");
    let mut d = mk();
    d.select_all();
    assert!(!d.has_block());
    let mut e = mk();
    e.select_span(0, 0, 2);
    assert!(!e.has_block());
    let mut g = mk();
    g.insert_str("x"); // 普通插入前 apply_edit 已清（此处直测 core 口径）
    g.block_sel = mk().block_sel;
    assert!(g.undo());
    assert!(!g.has_block(), "撤销清块");
    // clear_block 返回「原本是否有块」（调用方据此决定是否触发刷新）
    let mut h = mk();
    assert!(h.clear_block());
    assert!(!h.clear_block(), "二次清除返回 false");
}

#[test]
fn cursor_offset_is_full_document_char_index() {
    // 第 69 轮状态栏「位置」：全文字符偏移（CRLF 的 \r 计 1）
    let mut c = core_with("ab\ncd\r\nef");
    c.cursor = CursorPos { line: 0, col: 1 };
    assert_eq!(c.cursor_offset(), 1);
    c.cursor = CursorPos { line: 1, col: 2 };
    // 行0 "ab\n"=3 + 行1 "cd" 前 2 = 5
    assert_eq!(c.cursor_offset(), 5);
    c.cursor = CursorPos { line: 2, col: 2 };
    // 3 + ("cd\r\n"=4) + 2 = 9
    assert_eq!(c.cursor_offset(), 9);
    // 文档尾 = 总字符数
    assert_eq!(c.cursor_offset(), c.doc.text_len());
}

// ---------- P121：选区块缩进/反缩进（Tab / Shift+Tab） ----------

#[test]
fn block_indent_adds_tab_to_touched_lines_and_shifts_selection() {
    let mut c = core_with("ab\ncd\nef");
    c.anchor = Some(CursorPos { line: 0, col: 1 });
    c.cursor = CursorPos { line: 1, col: 1 };
    assert!(c.indent_touched_lines(false));
    assert_eq!(c.doc.to_text(), "\tab\n\tcd\nef");
    // 列 0 起点保持列 0，列 >0 按所在行缩进量平移
    assert_eq!(c.anchor, Some(CursorPos { line: 0, col: 2 }));
    assert_eq!(c.cursor, CursorPos { line: 1, col: 2 });
    assert!(c.undo(), "块缩进必须可整体撤销");
    assert_eq!(c.doc.to_text(), "ab\ncd\nef");
}

#[test]
fn block_indent_covers_blank_lines_and_reverses() {
    let mut c = core_with("a\n\nb");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 2, col: 1 };
    assert!(c.indent_touched_lines(false));
    assert_eq!(c.doc.to_text(), "\ta\n\t\n\tb");
    assert!(c.indent_touched_lines(true));
    assert_eq!(c.doc.to_text(), "a\n\nb");
}

#[test]
fn block_indent_handles_crlf_and_keeps_tail_newline() {
    let mut c = core_with("a\r\nb\r\n");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 2, col: 0 }; // 末点在幻影行行首
    assert!(c.indent_touched_lines(false));
    // 幻影行不入块；按主导行尾 \r\n 重建，尾随换行保持
    assert_eq!(c.doc.to_text(), "\ta\r\n\tb\r\n");
}

#[test]
fn outdent_strips_tab_or_up_to_tab_stop_spaces() {
    let mut c = core_with("\tx\n    y\n   z\nw");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 3, col: 1 };
    assert!(c.indent_touched_lines(true));
    // 一个 \t 整体剥掉；4 空格剥光；3 空格不足一档也剥光；无空白不动
    assert_eq!(c.doc.to_text(), "x\ny\nz\nw");
    assert_eq!(c.cursor, CursorPos { line: 3, col: 1 }, "无增量行的列不动");
}

#[test]
fn outdent_without_selection_outdents_current_line() {
    let mut c = core_with("    x");
    c.cursor = CursorPos { line: 0, col: 2 }; // 光标在被剥空白的列内
    assert!(c.indent_touched_lines(true));
    assert_eq!(c.doc.to_text(), "x");
    assert_eq!(
        c.cursor,
        CursorPos { line: 0, col: 0 },
        "被剥空白内的列钳回 0"
    );
    // 无空白可收：幂等 no-op 不产快照
    assert!(!c.indent_touched_lines(true));
    assert_eq!(c.doc.to_text(), "x");
}

#[test]
fn indent_without_selection_inserts_tab_like_before() {
    let mut c = core_with("ab");
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.indent_touched_lines(false));
    assert_eq!(c.doc.to_text(), "a\tb");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
}

#[test]
fn outdent_on_phantom_line_is_noop() {
    let mut c = core_with("a\n");
    c.cursor = CursorPos { line: 1, col: 0 }; // 幻影行
    assert!(!c.indent_touched_lines(true));
    assert_eq!(c.doc.to_text(), "a\n");
}

    // ---------- P124：行操作扩展与词首大写 ----------

    #[test]
    fn reverse_lines_reverses_block_and_is_undoable() {
        let mut c = core_with("a\nb\nc");
        assert!(c.reverse_lines());
        assert_eq!(c.doc.to_text(), "c\nb\na");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a\nb\nc");
        // 回文块：幂等 no-op
        let mut p = core_with("x\ny\nx");
        assert!(!p.reverse_lines());
        assert_eq!(p.doc.to_text(), "x\ny\nx");
    }

    #[test]
    fn numeric_sort_extracts_leading_signed_int_and_parks_keyless_last() {
        let mut c = core_with("b 3\n-1 x\nno num\n+2 y\n10 z");
        assert!(c.sort_lines_numeric(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "-1 x\n+2 y\nb 3\n10 z\nno num");
        assert!(c.sort_lines_numeric(SortOrder::Descending));
        assert_eq!(c.doc.to_text(), "10 z\nb 3\n+2 y\n-1 x\nno num");
        // 子串中的数字也作数（主流编辑器同口径）：无需行首
        let mut d = core_with("id=42\nid=7");
        assert!(d.sort_lines_numeric(SortOrder::Ascending));
        assert_eq!(d.doc.to_text(), "id=7\nid=42");
    }

    #[test]
    fn length_sort_orders_by_char_count() {
        let mut c = core_with("ccc\na\ndd");
        assert!(c.sort_lines_length(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "a\ndd\nccc");
        assert!(c.sort_lines_length(SortOrder::Descending));
        assert_eq!(c.doc.to_text(), "ccc\ndd\na");
    }

    #[test]
    fn consecutive_dedupe_keeps_first_of_each_run() {
        let mut c = core_with("a\na\nb\na\na\na\nc");
        assert!(c.remove_consecutive_duplicate_lines());
        assert_eq!(c.doc.to_text(), "a\nb\na\nc");
        assert!(c.undo(), "可整体撤销");
        assert_eq!(c.doc.to_text(), "a\na\nb\na\na\na\nc");
        // 全无连续重复：no-op
        let mut d = core_with("x\ny");
        assert!(!d.remove_consecutive_duplicate_lines());
    }

    #[test]
    fn title_case_uppercases_word_starts_and_lowers_rest() {
        let mut c = core_with("hello WORLD foo_bar\n中文test");
        c.cursor = CursorPos { line: 0, col: 0 };
        c.select_all();
        assert!(c.convert_case(CaseKind::Title));
        assert_eq!(c.doc.to_text(), "Hello World Foo_Bar\n中文test", "下划线分段、CJK 视为词段");
        // 幂等：已词首大写 → no-op
        assert!(!c.convert_case(CaseKind::Title));
    }

    #[test]
    fn extract_leading_int_handles_signs_overflow_and_words() {
        assert_eq!(crate::editor::block::extract_leading_int("-12abc"), Some(-12));
        assert_eq!(crate::editor::block::extract_leading_int("+7"), Some(7));
        assert_eq!(crate::editor::block::extract_leading_int("abc-3"), Some(-3), "子串取数");
        assert_eq!(crate::editor::block::extract_leading_int("--5"), Some(-5), "第二个 - 与 5 成键");
        assert_eq!(crate::editor::block::extract_leading_int("no digits"), None);
        // 超长数字串：饱和钳制而非丢键
        assert_eq!(
            crate::editor::block::extract_leading_int(&"9".repeat(60)),
            Some(i128::MAX)
        );
        // 负向饱和
        assert_eq!(crate::editor::block::extract_leading_int(&format!("-{}", "9".repeat(60))), Some(i128::MIN));
    }
