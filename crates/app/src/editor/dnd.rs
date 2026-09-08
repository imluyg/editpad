//! P135：文档内拖拽移动/复制选区（路线图 B8，同类编辑器 Drag-and-Drop
//! 同类）。
//!
//! 状态机：选区内左键按下 = 候选（不清选区、不动光标）→ 移动超 4px
//! 阈值 = 拖拽（更新落点 + 贴缘自动推进，落点竖线指示）→ 释放执行
//! （Ctrl 按住 = 复制，否则移动）；候选态原地释放由控件层按普通点击
//! 兜底。编辑经 `EditOp::DropSelection` 走 apply_edit 管线（busy/只读
//! 守卫、置脏、自动保存、查找重扫全继承），核心执行为**单快照**下的
//! 「先删后插」两次 splice——一次撤销撤掉整步移动。

use super::core::{CursorPos, EditorCore};

/// 拖拽会话的瞬态（仅会话内、不入快照）。
pub(crate) struct DndState {
    /// 超过位移阈值、进入拖拽态
    pub(crate) started: bool,
    /// 按下点（控件局部像素），阈值判定基准
    pub(crate) press_x: f32,
    pub(crate) press_y: f32,
    /// 当前落点（拖拽中随鼠标更新；候选态 = 按下命中点）
    pub(crate) drop: CursorPos,
}

const DRAG_THRESHOLD_PX: f32 = 4.0;

impl EditorCore {
    /// 选区内左键按下：成立则记候选态并返回 true（控件层应拦截本次
    /// 按下——不清选区、不移动光标、不进入拖选）。列块/组字/只读态
    /// 一律不启动（v1 取舍：只读页拖拽整体禁用，含纯复制）。
    pub(crate) fn begin_dnd_press(&mut self, x: f32, y: f32) -> bool {
        // P147：记录按下坐标——曾恒存 (0,0)，4px 防误触阈值实际度量的是
        // 「到控件左上角的距离」，选区内几乎任意位置轻点微动 1px 即被判
        // 成拖拽（旧测试全用近原点坐标恰好绕过）
        let hit = self.hit_test(x, y);
        if self.begin_dnd_at(hit) {
            if let Some(d) = self.dnd.as_mut() {
                d.press_x = x;
                d.press_y = y;
            }
            true
        } else {
            false
        }
    }

    /// [`Self::begin_dnd_press`] 的命中点直入版（单测注入用）。
    pub(crate) fn begin_dnd_at(&mut self, hit: CursorPos) -> bool {
        if self.dnd.is_some()
            || self.block_sel.is_some()
            || self.preedit.is_some()
            || self.read_only
            // B10：多光标态不启动拖拽（设计 §4 #6）——普通点击折叠在
            // 控件层先于本函数裁决，能到这里即 multi 已清，此为双保险
            || self.has_multi()
        {
            return false;
        }
        let Some((s, e)) = self.selection_offsets() else {
            return false;
        };
        let hit_off = self
            .doc
            .line_to_char(hit.line)
            + hit.col.min(self.line_display_len(hit.line));
        if hit_off < s || hit_off > e {
            return false;
        }
        self.dnd = Some(DndState {
            started: false,
            press_x: 0.0,
            press_y: 0.0,
            drop: hit,
        });
        true
    }

    /// 拖拽推进：候选态超阈值则进入拖拽；拖拽中更新落点并贴缘自动
    /// 推进视口（与拖选同款）。返回 true = 状态有变，控件层应重绘。
    pub(crate) fn update_dnd(&mut self, x: f32, y: f32) -> bool {
        if self.dnd.is_none() {
            return false;
        }
        if !self.dnd.as_ref().unwrap().started {
            let (px, py) = {
                let d = self.dnd.as_ref().unwrap();
                (d.press_x, d.press_y)
            };
            let dx = x - px;
            let dy = y - py;
            if dx * dx + dy * dy < DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX {
                return false; // 阈值内仍是候选：等释放
            }
            self.dnd.as_mut().unwrap().started = true;
        }
        self.dnd.as_mut().unwrap().drop = self.hit_test(x, y);
        let (dx_cols, dy_lines) = self.edge_scroll_delta(x, y, self.viewport_w, self.viewport_h);
        if dx_cols != 0.0 {
            self.scroll_by_columns(dx_cols);
        }
        if dy_lines != 0.0 {
            self.scroll_by_lines(dy_lines);
        }
        true
    }

    /// 取消拖拽（Esc / 其他键按下）：丢弃候选态，选区原样保留。
    pub(crate) fn cancel_dnd(&mut self) {
        self.dnd = None;
    }

    /// 释放执行（经 apply_edit 管线调用）。`copy=false` 为移动：先删后
    /// 插（落点在选区内 = 原地踏步，取消）；`copy=true` 为复制：仅插入
    /// （落点与选区重叠合法，主流同口径）。光标落到插入文本尾，一次
    /// 撤销撤掉整步。书签随行结构变化重映射（被移动行上的书签不随行
    /// 迁移——主流同口径，撤销可整体还原）。
    pub(crate) fn finish_drop_selection(&mut self, drop: CursorPos, copy: bool) -> bool {
        self.dnd = None;
        let Some((start_off, end_off)) = self.selection_offsets() else {
            return false;
        };
        if start_off >= end_off {
            return false;
        }
        let drop_off = self.doc.line_to_char(drop.line)
            + drop.col.min(self.line_display_len(drop.line));
        if !copy && drop_off >= start_off && drop_off <= end_off {
            return false; // 移动落点在选区内 = 无变化
        }
        let text = self.doc.slice_text(start_off, end_off);
        let sel_len = end_off - start_off;
        let sel_start_line = self.doc.char_to_line(start_off);
        self.break_typing();
        self.snapshot(); // 单快照：删除+插入两步 splice 合为一次撤销
        let insert_off = if copy {
            drop_off
        } else {
            self.doc.remove_range(start_off, end_off);
            let removed_lines = text.matches('\n').count();
            self.remap_removed_rows(sel_start_line, removed_lines);
            if drop_off > end_off {
                drop_off - sel_len
            } else {
                drop_off
            }
        };
        self.doc.insert(insert_off, &text);
        let inserted_lines = text.matches('\n').count();
        if inserted_lines > 0 {
            let drop_line = self.doc.char_to_line(insert_off);
            self.remap_shift_below(drop_line, inserted_lines as isize);
        }
        let end_ins = insert_off + text.chars().count();
        let line = self.doc.char_to_line(end_ins.min(self.doc.text_len()));
        let col = end_ins - self.doc.line_to_char(line);
        self.cursor = CursorPos { line, col };
        self.anchor = None;
        self.invalidate_highlight_from(start_off.min(insert_off));
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::BlockSel;

    fn core_with(text: &str) -> EditorCore {
        let mut c = EditorCore::default();
        c.reset_document(editpad_core::Document::from_str(text));
        c
    }

    fn select(c: &mut EditorCore, l0: usize, c0: usize, l1: usize, c1: usize) {
        c.anchor = Some(CursorPos { line: l0, col: c0 });
        c.cursor = CursorPos { line: l1, col: c1 };
    }

    #[test]
    fn dnd_move_relocates_text_with_single_undo() {
        let mut c = core_with("abcd\nefgh\n");
        select(&mut c, 0, 1, 0, 3); // "bc"
        // 落点 = 行 1 列 2（"ef" 之后）；移动 → 先删后插
        assert!(c.finish_drop_selection(CursorPos { line: 1, col: 2 }, false));
        assert_eq!(c.doc.to_text(), "ad\nefbcgh\n");
        // 光标落到插入文本尾（行 1 列 4），选区清空
        assert_eq!(c.cursor, CursorPos { line: 1, col: 4 });
        assert!(c.anchor.is_none());
        // 单快照：一次撤销整体还原
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "abcd\nefgh\n");
    }

    #[test]
    fn dnd_copy_inserts_and_keeps_source() {
        let mut c = core_with("abcd\nefgh\n");
        select(&mut c, 0, 1, 0, 3);
        assert!(c.finish_drop_selection(CursorPos { line: 1, col: 2 }, true));
        assert_eq!(c.doc.to_text(), "abcd\nefbcgh\n", "复制只在落点插入（ef 之后）");
    }

    #[test]
    fn dnd_move_inside_selection_is_noop() {
        let mut c = core_with("abcd\nefgh\n");
        select(&mut c, 0, 1, 0, 3);
        // 落点在选区内（列 2）→ 取消不编辑
        assert!(!c.finish_drop_selection(CursorPos { line: 0, col: 2 }, false));
        assert_eq!(c.doc.to_text(), "abcd\nefgh\n");
        // 落点在选区边界（列 1 / 列 3）同样无效
        assert!(!c.finish_drop_selection(CursorPos { line: 0, col: 1 }, false));
        assert!(!c.finish_drop_selection(CursorPos { line: 0, col: 3 }, false));
    }

    #[test]
    fn dnd_press_guards_and_state() {
        let mut c = core_with("abcd\nefgh\n");
        // 无选区不启动
        assert!(!c.begin_dnd_at(CursorPos { line: 0, col: 1 }));
        select(&mut c, 0, 1, 0, 3);
        // 命中在选区外不启动
        assert!(!c.begin_dnd_at(CursorPos { line: 0, col: 0 }));
        assert!(!c.begin_dnd_at(CursorPos { line: 1, col: 0 }));
        // 命中在选区内（含边界）→ 候选态
        assert!(c.begin_dnd_at(CursorPos { line: 0, col: 2 }));
        assert!(c.dnd.is_some());
        assert!(!c.dnd.as_ref().unwrap().started, "初始为候选态");
        // 只读不启动
        c.cancel_dnd();
        c.read_only = true;
        assert!(!c.begin_dnd_at(CursorPos { line: 0, col: 2 }));
        c.read_only = false;
        // 列块态不启动
        c.block_sel = Some(BlockSel {
            anchor: CursorPos::default(),
            head: CursorPos::default(),
        });
        assert!(!c.begin_dnd_at(CursorPos { line: 0, col: 2 }));
    }

    #[test]
    fn dnd_update_requires_threshold_then_tracks_drop() {
        let mut c = core_with("abcd\nefgh\n");
        select(&mut c, 0, 1, 0, 3);
        assert!(c.begin_dnd_at(CursorPos { line: 0, col: 2 }));
        // 阈值内（<4px）仍是候选
        assert!(!c.update_dnd(2.0, 1.0));
        assert!(!c.dnd.as_ref().unwrap().started);
        // 超阈值进入拖拽
        assert!(c.update_dnd(10.0, 30.0));
        assert!(c.dnd.as_ref().unwrap().started);
        // 落点随鼠标更新（像素→hit_test 由列模型折算，只验证有更新）
        assert!(c.update_dnd(10.0, 55.0));
        // Esc 取消：会话丢弃、选区保留
        c.cancel_dnd();
        assert!(c.dnd.is_none());
        assert!(c.selection_offsets().is_some());
    }

    #[test]
    fn dnd_threshold_measures_from_press_point_not_widget_origin() {
        // P147 回归：press 坐标曾恒存 (0,0)——4px 阈值变成「到控件左上角
        // 的距离」，选区内轻点微动 1px 即被判成拖拽。选区钉在行尾，
        // 按下点远离原点，阈值必须从按下点起算。
        let mut c = core_with("abcd\nefgh\n");
        select(&mut c, 0, 2, 0, 4);
        // 按下点 (1000, 0)：hit_test 折算到行尾列 4，落在选区内 → 候选态
        assert!(c.begin_dnd_press(1000.0, 0.0));
        assert_eq!(
            c.dnd.as_ref().unwrap().press_x, 1000.0,
            "按下坐标必须如实记录"
        );
        // 距按下点 1px：阈值内仍是候选（旧实现按到原点距离 ≈1000px 已成拖拽）
        assert!(!c.update_dnd(1001.0, 0.0));
        assert!(!c.dnd.as_ref().unwrap().started);
        // 距按下点 5px：超阈值进入拖拽
        assert!(c.update_dnd(1005.0, 0.0));
        assert!(c.dnd.as_ref().unwrap().started);
    }
}
