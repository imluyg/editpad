//! P311 单测：智能 Home（在「缩进后第一个非空白」与「真行首」两档之间切换）。
//!
//! 这是**行为变更**（改前 Home 恒到列 0 / 视觉段首），所以档位边界逐条钉：
//! 无缩进行与纯空白行必须与改前逐字同形 —— 日志行大多不缩进，那一档不该有任何
//! 变化；而"按 Home 往右跑"是这类改动最容易踩出来的怪味，单独钉一条。

use super::tests::*;
use super::*;

fn col(c: &EditorCore) -> usize {
    c.cursor.col
}

#[test]
fn home_cycles_between_indent_and_line_start() {
    let mut c = core_with("    code here");
    c.cursor = CursorPos { line: 0, col: 10 };
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 4, "内容区 → 缩进后");
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0, "缩进后 → 真行首");
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 4, "行首再按 → 回缩进后（两档循环）");
}

#[test]
fn home_from_inside_the_indent_goes_to_line_start() {
    // 落在缩进区中间（列 2）：与主流口径一致 ⇒ 直接到行首，不去缩进后
    let mut c = core_with("    code here");
    c.cursor = CursorPos { line: 0, col: 2 };
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0);
}

#[test]
fn home_on_unindented_line_is_unchanged() {
    let mut c = core_with("message from host 10.0.0.1");
    c.cursor = CursorPos { line: 0, col: 15 };
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0, "无缩进行 = 改前行为");
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0, "已在行首再按不许动");
}

#[test]
fn home_on_whitespace_only_line_does_not_move_right() {
    // 整行皆空白 ⇒ 没有"缩进后"这一档。若拿行宽当落点，按 Home 会往**右**跑
    let mut c = core_with("   \nnext");
    c.cursor = CursorPos { line: 0, col: 2 };
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0, "空白行按 Home 不许落到行尾");
    let mut c = core_with("");
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0, "空行同样原地不动");
}

#[test]
fn tab_indent_counts_as_indent() {
    let mut c = core_with("\t\tsame()");
    c.cursor = CursorPos { line: 0, col: 6 };
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 2, "制表位也算缩进（列按字符口径）");
    c.apply_motion(Motion::Home, false);
    assert_eq!(col(&c), 0);
}

#[test]
fn logical_home_still_lands_at_column_zero() {
    // Alt+Home（P134 的"直达逻辑行首"）不许被智能档位改掉
    let mut c = core_with("    code here");
    c.cursor = CursorPos { line: 0, col: 10 };
    c.apply_motion(Motion::LogicalHome, false);
    assert_eq!(col(&c), 0);
}

#[test]
fn shift_home_extends_to_the_same_smart_target() {
    let mut c = core_with("    code here");
    c.cursor = CursorPos { line: 0, col: 10 };
    c.apply_motion(Motion::Home, true);
    assert_eq!(col(&c), 4, "Shift+Home 同样走智能档位");
    assert_eq!(
        c.anchor,
        Some(CursorPos { line: 0, col: 10 }),
        "扩展选区时锚点留在按下处"
    );
    assert_eq!(c.selected_text().as_deref(), Some("code h"));
}
