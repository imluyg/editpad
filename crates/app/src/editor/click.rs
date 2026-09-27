//! P307：编辑区连击选择 —— 双击选词 / 三击选行 / 按词与按行粒度拖选 /
//! Shift+点击扩展选区。
//!
//! 控件层只把「按下—移动—释放」三个鼠标事件转成对 core 的三个调用
//! （[`EditorCore::register_click`] → [`EditorCore::select_word_at`] /
//! [`EditorCore::select_line_at`] → [`EditorCore::apply_click_drag`]），
//! 连击计数、词/行边界与选区归一全留在这一层：无头测试可直接驱动同一入口，
//! 不必经过 iced 的事件流。

use editpad_core::search::is_word_char;

use super::core::{CursorPos, EditorCore};

/// 双击判定时间窗：与标签条就地重命名（`TAB_DOUBLE_CLICK_MS`）同值——
/// 同一个窗口里两套「双击」手感不该不一样。
pub(crate) const DOUBLE_CLICK_MS: u64 = 500;

/// 同一行内两次按下的列容差：相距不超过这么多列才算点在同一下、连击成立。
/// 8 ≈ 双击时手指的微移再加半个常用词宽；超过即视为换了目标，重新计数。
pub(crate) const CLICK_COL_SLOP: usize = 8;

/// 拖选粒度：由连击序号决定，左键释放时复位为 [`Self::Char`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragGran {
    /// 逐字符（普通按下）
    Char,
    /// 整词（双击后拖动）
    Word,
    /// 整行（三击后拖动）
    Line,
}

/// 字符分类：决定双击落在该字符上时向两边扩到哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if is_word_char(c) {
        CharClass::Word
    } else if c == ' ' || c == '\t' {
        CharClass::Space
    } else {
        CharClass::Punct
    }
}

/// `(line, col)` 的文档序比较（与 `selection_offsets` 同一口径：先行后列）。
fn before(a: &CursorPos, b: &CursorPos) -> bool {
    (a.line, a.col) <= (b.line, b.col)
}

impl EditorCore {
    /// 一次正文左键按下：记账并返回连击序号（1 = 单击，2 = 双击，3 = 三击）。
    ///
    /// 连击成立条件：同行、列相距在 [`CLICK_COL_SLOP`] 内、距上次按下不超
    /// [`DOUBLE_CLICK_MS`]。第四次起算重新从 1 开始（三击已是本层的最大
    /// 粒度，继续累加没有语义可挂）。
    pub(crate) fn register_click(&mut self, hit: CursorPos, now: std::time::Instant) -> u8 {
        let count = match self.click_last {
            Some((prev, line, col, at))
                if line == hit.line
                    && col.abs_diff(hit.col) <= CLICK_COL_SLOP
                    && prev < 3
                    && now.duration_since(at).as_millis() as u64 <= DOUBLE_CLICK_MS =>
            {
                prev + 1
            }
            _ => 1,
        };
        self.click_last = Some((count, hit.line, hit.col, now));
        count
    }

    /// 释放/取消时收尾：粒度回落逐字符、连击基准清空。
    /// `click_last` **不清**——双击是「按下—释放—按下」，计数链必须活过释放。
    pub(crate) fn end_click_drag(&mut self) {
        self.drag_gran = DragGran::Char;
        self.click_base = None;
    }

    /// `hit` 处双击应选中的范围 `(start, end)`（字符列，半开区间）。
    ///
    /// 口径与 [`Self::word_neighbor`]（词移动）一致：词字符连续段整体一段、
    /// 空白连续段整体一段、标点单字一段；CJK 连续段算一个词。
    /// 空行、以及点在最末行行尾之后没有字符可取时返回 None。
    pub(crate) fn word_range_at(&self, hit: CursorPos) -> Option<(CursorPos, CursorPos)> {
        let body = self.line_display_len(hit.line);
        if body == 0 {
            return None;
        }
        let col = hit.col.min(body);
        // 点于行尾（col == body）时向左取最后一个字符，与词移动同感
        let seed = if col == body { col - 1 } else { col };
        let start_off = self.doc.line_to_char(hit.line);
        let cls = char_class(self.char_at(start_off + seed)?);
        // 标点不扩段：双击单个标点只选它自己
        let (mut a, mut b) = (seed, seed + 1);
        if cls != CharClass::Punct {
            while a > 0 && char_class(self.char_at(start_off + a - 1)?) == cls {
                a -= 1;
            }
            while b < body && char_class(self.char_at(start_off + b)?) == cls {
                b += 1;
            }
        }
        Some((
            CursorPos {
                line: hit.line,
                col: a,
            },
            CursorPos {
                line: hit.line,
                col: b,
            },
        ))
    }

    /// 双击：选中 `hit` 所在词。返回 false = 该处无从选词（空行/无字符），
    /// 调用方按普通单击处理。
    pub(crate) fn select_word_at(&mut self, hit: CursorPos) -> bool {
        let Some((start, end)) = self.word_range_at(hit) else {
            return false;
        };
        self.apply_click_selection(hit, start, end, DragGran::Word);
        true
    }

    /// 三击：选中 `hit` 所在整行。选区含行末换行符（与「剪切整行」同口径），
    /// 末行没有换行符时止于行尾。
    pub(crate) fn select_line_at(&mut self, hit: CursorPos) -> bool {
        let count = self.doc.line_count();
        if count == 0 {
            return false;
        }
        let line = hit.line.min(count - 1);
        let start = CursorPos { line, col: 0 };
        let end = self.line_end_plus_break(line);
        self.apply_click_selection(hit, start, end, DragGran::Line);
        true
    }

    /// Shift+点击：选区另一端拉到 `hit`，锚点不动（无锚点时以当前光标为锚）。
    pub(crate) fn extend_selection_to(&mut self, hit: CursorPos) {
        self.break_typing();
        self.clear_block();
        self.collapse_multi();
        self.clear_vertical_goal();
        let anchor = self.anchor.unwrap_or(self.cursor);
        self.anchor = Some(anchor);
        self.cursor = hit;
        self.drag_gran = DragGran::Char;
        self.click_base = None;
        self.ensure_visible();
    }

    /// 连击后拖动：把悬停点吸附到整词/整行边界并重算选区。
    /// 返回 true = 选区变了（控件层据此重绘 + 发导航消息）。
    /// 粒度为 [`DragGran::Char`] 时恒 false，走既有逐字符拖选路径。
    pub(crate) fn apply_click_drag(&mut self, hover: CursorPos) -> bool {
        let gran = self.drag_gran;
        let Some(base) = self.click_base else {
            return false;
        };
        let (start, end) = match gran {
            DragGran::Char => return false,
            DragGran::Word => {
                let Some(base_range) = self.word_range_at(base) else {
                    return false;
                };
                // 悬停在空白/边界等无从取词处：按原样落点，不改变已有选择
                let hover_range = self.word_range_at(hover).unwrap_or((hover, hover));
                let (base_start, base_end) = base_range;
                if before(&base, &hover) {
                    let far = if before(&hover_range.1, &base_end) {
                        base_end
                    } else {
                        hover_range.1
                    };
                    (base_start, far)
                } else {
                    let far = if before(&hover_range.0, &base_start) {
                        hover_range.0
                    } else {
                        base_start
                    };
                    (far, base_end)
                }
            }
            DragGran::Line => {
                let count = self.doc.line_count();
                let la = base.line.min(count.saturating_sub(1));
                let lb = hover.line.min(count.saturating_sub(1));
                let (first, last) = if la <= lb { (la, lb) } else { (lb, la) };
                let start = CursorPos {
                    line: first,
                    col: 0,
                };
                let end = self.line_end_plus_break(last);
                (start, end)
            }
        };
        if self.anchor == Some(start) && self.cursor == end {
            return false;
        }
        self.anchor = Some(start);
        self.cursor = end;
        self.ensure_visible();
        true
    }

    /// 第 `line` 行「含行末换行符」的右端点：下一行行首，末行则行尾。
    fn line_end_plus_break(&self, line: usize) -> CursorPos {
        let count = self.doc.line_count();
        if line + 1 < count {
            CursorPos {
                line: line + 1,
                col: 0,
            }
        } else {
            let col = self.line_display_len(line);
            CursorPos { line, col }
        }
    }

    /// 连击选择的统一落点：清块/折叠多光标/打断打字组成组，然后落
    /// anchor..cursor 并进入拖选态（按下后不松手继续拖 = 按粒度扩展）。
    fn apply_click_selection(
        &mut self,
        hit: CursorPos,
        start: CursorPos,
        end: CursorPos,
        gran: DragGran,
    ) {
        self.break_typing();
        // ⚠️ 这里**不记**跳转历史（P312 复核纠正）：能走到连击这一拍，
        // 说明同一处刚发生过一次单击，而那一拍已把"点击前的位置"记进栈。
        // 再记一次会让 Alt+← 先小跳到点击处、按第二下才回到真正的出发点。
        self.clear_block(); // 块态与单选区互斥（第 67 轮口径）
        self.collapse_multi(); // 连击 = 重置为单光标（B10 主流口径）
        self.clear_vertical_goal(); // 点击类操作非竖向移动
        self.anchor = Some(start);
        self.cursor = end;
        self.dragging = true;
        self.drag_gran = gran;
        self.click_base = Some(hit);
        self.ensure_visible();
    }
}
