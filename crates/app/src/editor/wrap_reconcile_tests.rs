//! P154：软换行开态下「行数变化 → 换行表整表重置」曾让滚动夹紧用到偏小的
//! 视觉行总数（重置后每行暂记 1 段），表现为末尾按回车后视口先被拉回上方、
//! 下一帧收敛后又跳回（用户复报：「行号闪一下」）。本组用例钉住编辑汇点的
//! 对账契约。

use super::*;

/// 把全部行查询一遍（= 绘制路径逐可见行收敛的语义），用于对照「真值」。
fn converge(c: &mut EditorCore) {
    let n = c.doc.line_count();
    for l in 0..n {
        let t = c.line_text(l);
        c.segments_of_line(l, &t);
    }
}

#[test]
fn p154_enter_at_wrapped_tail_keeps_scroll_on_true_total() {
    // 9 行文档，每 3 行一条超宽行（折行开态下折多段）
    let mut text = String::new();
    for i in 1..=9 {
        if i % 3 == 0 {
            text.push_str(&"中".repeat(150));
        } else {
            text.push_str(&format!("line {i}"));
        }
        text.push('\n');
    }
    let mut c = EditorCore::default();
    c.reset_document(editpad_core::Document::from_str(&text));
    c.set_viewport_width(400.0);
    c.set_viewport_height(200.0);
    c.set_word_wrap(true);
    converge(&mut c);
    // 滚到文档末尾
    c.scroll_top = (c.visual_rows_total() as f32 - c.viewport_h / c.line_height()).max(0.0);

    // 光标落最后一行行尾，按回车
    let last = c.doc.line_count() - 1;
    c.cursor = CursorPos { line: last, col: c.line_display_len(last) };
    c.insert_str("\n");

    let rows = c.viewport_h / c.line_height();
    let max_true = (c.visual_rows_total() as f32 - rows).max(0.0);
    assert!(
        (c.scroll_top - max_true).abs() < 0.01,
        "编辑后滚动必须落在**真实**总视觉行数的上限上：实际 {:.3}，真实上限 {max_true:.3}\
         （P154 回归：换行表重置后若未对账，总数按「每行 1 段」偏小，\
         视口会被拉回上方再跳回 → 行号闪一下）",
        c.scroll_top
    );

    // 再收敛一遍不得再改变滚动（否则说明编辑当下用的是陈旧上限）
    let after_edit = c.scroll_top;
    converge(&mut c);
    assert!(
        (c.scroll_top - after_edit).abs() < 0.01,
        "后续对账不得再挪动滚动：编辑后 {after_edit:.3} → 收敛后 {:.3}",
        c.scroll_top
    );
}

/// 对账后索引必须处于「可信」态（重复调用是 no-op），且视觉行映射自洽。
#[test]
fn p154_reconcile_is_idempotent_and_consistent() {
    let mut text = String::new();
    for i in 1..=9 {
        if i % 3 == 0 {
            text.push_str(&"中".repeat(150));
        } else {
            text.push_str(&format!("line {i}"));
        }
        text.push('\n');
    }
    let mut c = EditorCore::default();
    c.reset_document(editpad_core::Document::from_str(&text));
    c.set_viewport_width(400.0);
    c.set_viewport_height(200.0);
    c.set_word_wrap(true);
    converge(&mut c);

    let last = c.doc.line_count() - 1;
    c.cursor = CursorPos { line: last, col: c.line_display_len(last) };
    c.insert_str("\n");

    // 编辑汇点已对账：无需再收敛，总数即真值
    let total = c.visual_rows_total();
    let before = total;
    c.reconcile_wrap_index();
    assert_eq!(c.visual_rows_total(), before, "重复对账不得改变总数");
    assert!(
        total as usize >= c.doc.line_count(),
        "总视觉行数不得小于逻辑行数（{total} vs {}）",
        c.doc.line_count()
    );
    // 视觉行 → 逻辑行反解自洽：段首行的 line_visual_base 等于该视觉行号
    for v in 0..total {
        let (line, seg, _, _) = c.locate_visual(v);
        if seg == 0 {
            assert_eq!(
                c.line_visual_base(line),
                v,
                "视觉行 {v} 反解为 line {line} 段首，但 line_visual_base 不一致"
            );
        }
    }
}
