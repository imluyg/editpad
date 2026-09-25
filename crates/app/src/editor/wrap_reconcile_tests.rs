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
    c.cursor = CursorPos {
        line: last,
        col: c.line_display_len(last),
    };
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

/// 第 183 轮 ⑤：软换行开态、视口宽 = `viewport_w` 的同一份夹具文档
/// （`converge` 是**与机制无关**的真值口径：逐行查一遍 = 绘制路径的收敛语义）。
fn wrapped_doc_at(viewport_w: f32) -> EditorCore {
    let mut c = wrapped_from(&fixture_text(), viewport_w);
    converge(&mut c);
    c
}

/// 同一份文档在 `viewport_w` 下开启软换行（不做额外对账）。
fn wrapped_from(text: &str, viewport_w: f32) -> EditorCore {
    let mut c = EditorCore::default();
    c.reset_document(editpad_core::Document::from_str(text));
    c.set_viewport_width(viewport_w);
    c.set_viewport_height(200.0);
    c.set_word_wrap(true);
    c
}

/// 9 行文档，每 3 行一条超宽行（折行开态下折多段）。
fn fixture_text() -> String {
    let mut text = String::new();
    for i in 1..=9 {
        if i % 3 == 0 {
            text.push_str(&"中".repeat(150));
        } else {
            text.push_str(&format!("line {i}"));
        }
        text.push('\n');
    }
    text
}

/// 视口宽变化 = 折行像素预算变化 → 整表重置。这条路径上没有编辑汇点来做
/// P154 那次全量对账，所以 `visual_rows_total()` 会**永久**停在「每行 1 段」，
/// 而它正是滚动夹紧与滚动条行程的上限来源。
#[test]
fn p270_viewport_width_change_makes_visual_total_trustworthy() {
    let mut c = wrapped_doc_at(400.0);
    let truth = wrapped_doc_at(300.0);

    let wide = c.visual_rows_total();
    let narrow_true = truth.visual_rows_total();
    assert!(
        narrow_true > wide,
        "夹具须对宽度敏感：同一文档从 400px 收到 300px 视觉行数必须增加（{wide} → {narrow_true}）"
    );

    // 只改视口宽（窗口横向拖拽一帧），期间没有任何编辑
    c.set_viewport_width(300.0);
    assert_eq!(
        c.visual_rows_total(),
        narrow_true,
        "变窄后的视觉行总数必须与「从一开始就是这个宽度」的同文档一致\
         （虚低 ⇒ 文档末尾滚不到、滚动条行程偏短）"
    );
}

/// 用户看得见的那一层：变窄之后把滚动推到底，落点必须仍在文档末尾。
#[test]
fn p270_document_tail_still_reachable_after_narrowing_viewport() {
    let mut c = wrapped_doc_at(400.0);
    let truth = wrapped_doc_at(300.0);

    c.set_viewport_width(300.0);
    c.scroll_top = f32::MAX;
    c.clamp_scroll();

    let want = (truth.visual_rows_total() as f32 - truth.viewport_h / truth.line_height()).max(0.0);
    assert!(
        (c.scroll_top - (want + 1.0)).abs() < 0.001,
        "变窄后滚到底必须落在真实上限 {want:.1}（+1.0 钳制余量）上，实际 {:.1}\
         （P154 同族：总数虚低 ⇒ clamp_scroll 把滚动位置打回上方）",
        c.scroll_top
    );
}

/// 同一条缺陷在「设置里打开软换行」这一格上：`enable` 也是整表重置，
/// 开关路径后面同样没有编辑汇点补对账。
#[test]
fn p270_wrap_toggle_makes_visual_total_trustworthy() {
    let truth = wrapped_doc_at(400.0);

    let mut c = EditorCore::default();
    c.reset_document(editpad_core::Document::from_str(&fixture_text()));
    c.set_viewport_width(400.0);
    c.set_viewport_height(200.0);
    c.set_word_wrap(true);

    assert_eq!(
        c.visual_rows_total(),
        truth.visual_rows_total(),
        "开启软换行后无需任何编辑，视觉行总数就必须是真值"
    );
}

/// ⑤ 的收敛是**分摊**的：一次 `clamp_scroll` 只推进行数上限那么多行（实测
/// debug profile 每 1000 行 ≈15 ms、6 万行一次全量 975 ms ⇒ 逐帧改宽绝不能一次
/// 做完）。未收完的窗口里不得往下钳（虚低总数 = 视口被打回上方、文档末尾滚不
/// 到），收完之后必须立刻恢复夹紧并落在真实上限上。
#[test]
fn p270_sweep_is_bounded_and_clamp_waits_for_it() {
    let step = EditorCore::default().wrap_reconcile_step();
    let mut text = String::new();
    for i in 0..(2 * step + 1) {
        text.push_str(&format!("{i:05}"));
        text.push_str(&"中".repeat(40));
        text.push('\n');
    }
    let mut c = wrapped_from(&text, 400.0);
    let mut t = wrapped_from(&text, 300.0);
    converge(&mut t);
    let truth = t.visual_rows_total();
    assert!(
        truth as usize > 2 * step + 1,
        "夹具须每行折多段：{truth} 段应大于 {} 行",
        2 * step + 1
    );
    let want = (truth as f32 - t.viewport_h / t.line_height()).max(0.0);

    // 一帧改宽 = 整表重置 + 第一步；再走一步 clamp 只到第二步 ⇒ 窗口仍未闭合
    c.set_viewport_width(300.0);
    c.scroll_top = 9_000_000.0;
    c.clamp_scroll();
    assert!(
        c.scroll_top > 8_000_000.0,
        "⑤：分摊对账未收完时不得按虚低总数往下钳，实际 {:.1}",
        c.scroll_top
    );

    // 第三步补齐 ⇒ 夹紧必须重新生效（守卫不得变成「永不夹紧」）
    c.clamp_scroll();
    assert!(
        (c.scroll_top - (want + 1.0)).abs() < 0.001,
        "对账收完后必须重新按真实上限 {want:.1}（+1.0 余量）钳制，实际 {:.1}",
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
    c.cursor = CursorPos {
        line: last,
        col: c.line_display_len(last),
    };
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
