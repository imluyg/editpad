//! B10 多光标一期（设计 docs/multi-cursor-design.md）：附加光标集的
//! 增删/排序去重/折叠与行内步进。编辑同步（InsertText/Backspace/
//! Delete 多点应用）属 Phase 2；本期附加光标为纯导航态。

use super::*;

impl EditorCore {
    /// 是否存在附加光标（空集 = 单光标恒等退化，全部新路径短路）。
    pub(crate) fn has_multi(&self) -> bool {
        !self.extra_cursors.is_empty()
    }

    /// 折叠为单光标。返回是否原本存在附加光标（调用方据此刷新）。
    pub(crate) fn collapse_multi(&mut self) -> bool {
        let had = self.has_multi();
        self.extra_cursors.clear();
        had
    }

    /// 维持「文档位序升序、同位去重（优先保留带选区者）」的不变量。
    /// 主光标位置不参与去重——附加光标落在主光标位即无意义，新增时拒。
    fn sort_dedup_cursors(&mut self) {
        self.extra_cursors
            .sort_by_key(|e| (e.cursor.line, e.cursor.col, e.anchor.is_none()));
        self.extra_cursors.dedup_by_key(|e| (e.cursor.line, e.cursor.col));
    }

    /// Alt+Click：点击处加/移除附加光标。守卫（设计 §3.3/§4）：折行
    /// 开态 / 列块态 / 组字态拒绝；点击主光标位 = 无意义 no-op；命中
    /// 既有附加光标（同行列）= 移除；封顶拒新增。成功后打断组字组。
    /// 返回集合是否发生变化（调用方据此刷新）。
    pub(crate) fn toggle_extra_cursor(&mut self, at: CursorPos) -> bool {
        if self.wrap.borrow().enabled || self.block_sel.is_some() || self.preedit.is_some() {
            return false;
        }
        if at == self.cursor {
            return false;
        }
        if let Some(i) = self
            .extra_cursors
            .iter()
            .position(|e| e.cursor == at)
        {
            self.extra_cursors.remove(i);
            self.break_typing();
            return true;
        }
        if self.extra_cursors.len() >= MAX_EXTRA_CURSORS {
            return false;
        }
        self.extra_cursors.push(ExtraCursor {
            cursor: at,
            anchor: None,
        });
        self.sort_dedup_cursors();
        self.break_typing();
        true
    }

    /// 多光标态行内水平步进（apply_motion 仅 Left/Right 且不扩展时
    /// 进入）：附加光标逐个走一步，触行界者折叠该光标（设计 §3.3）；
    /// 主光标走既有 move_local 语义（含跨行）。全部走完后若附加集空
    /// 则自然回归单光标路径。
    pub(crate) fn multi_step_horizontal(&mut self, right: bool) {
        let mut i = 0;
        while i < self.extra_cursors.len() {
            let len = self.line_display_len(self.extra_cursors[i].cursor.line);
            let col = self.extra_cursors[i].cursor.col;
            let step_ok = if right {
                col < len
            } else {
                col > 0
            };
            if step_ok {
                self.extra_cursors[i].cursor.col = if right { col + 1 } else { col - 1 };
                i += 1;
            } else {
                self.extra_cursors.remove(i); // 越界折叠该光标
            }
        }
    }

    /// 多光标全集（主光标 + 附加）按文档位序——绘制层逐光标消费。
    pub(crate) fn all_cursors(&self) -> Vec<CursorPos> {
        let mut all: Vec<CursorPos> = self
            .extra_cursors
            .iter()
            .map(|e| e.cursor)
            .chain(std::iter::once(self.cursor))
            .collect();
        all.sort_by_key(|p| (p.line, p.col));
        all.dedup();
        all
    }

    /// 任意位置的光标矩形（主光标 `caret_rect_relative` 提参重入；
    /// 折行开态多光标被拒，此路径实际只服务关态——分支保留以复用）。
    pub(crate) fn caret_rect_at(&self, pos: CursorPos) -> Rectangle {
        let text = self.line_text(pos.line);
        let col = pos.col.min(text.chars().count());
        // P45：滞后感知（同 ensure_visible_horizontal）——布局新鲜走真实
        // 字形位置，滞后回退列模型实时计算；IME 候选框定位同样受益
        let x_px = self.px_of(pos.line, &text, col);
        let (v, seg_start) = if self.wrap.borrow().enabled {
            let mut w = self.wrap.borrow_mut();
            w.ensure_synced(
                self.doc.line_count(),
                self.wrap_max_cols(),
                self.wrap_max_px(),
            );
            let lens = text.chars().count();
            let real_xs = self.trusted_xs(pos.line, &text);
            let breaks = w.segments_of(pos.line, &text, real_xs);
            let seg = segment_index(&breaks, col, lens);
            let v = w.index.prefix_rows(pos.line) + seg as u32;
            (v, breaks[seg])
        } else {
            (pos.line as u32, 0)
        };
        Rectangle {
            x: self.gutter_width() + (x_px - self.px_of(pos.line, &text, seg_start))
                - self.scroll_left,
            // P88：字形墨迹在行盒内下浮 ink_offset——光标矩形下移到
            // 墨迹顶对齐，高度同步收窄到墨迹盒（行盒顶对齐会让光标
            // 顶部悬在首行上方空带、底部压进下行间隙，见
            // measure_ink_offset 注释；IME 候选框定位同样受益）
            y: (v as f32 - self.scroll_top) * self.line_height() + self.ink_offset,
            width: CARET_WIDTH,
            height: (self.line_height() - 2.0 * self.ink_offset).max(CARET_WIDTH * 2.0),
        }
    }
}
