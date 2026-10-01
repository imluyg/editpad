//! B10 多光标一期（设计 docs/multi-cursor-design.md）：附加光标集的
//! 增删/排序去重/折叠与行内步进。Phase 2：同步编辑（InsertText/
//! Backspace/Delete 从后往前逐点应用、单快照）与添加下一匹配（Ctrl+M）。
//! 二期首批（roadmap §9 第 7 行）：选区按行拆分＝每行正文末尾各一条光标。

use super::*;

/// B10 Phase 2：同步编辑的操作种类（设计 §3.2 三操作）。
pub(crate) enum MultiEditKind<'a> {
    /// 在各光标处插入同一段文本（有选区 = 替换选区）
    Insert(&'a str),
    /// 退格：有选区删选区，无选区删光标前一字符
    Backspace,
    /// 前向删除：有选区删选区，无选区删光标后一字符
    Delete,
}

/// 同步编辑的点级计划：一段字符偏移区间（删除 [s,e) 后在 s 处插入
/// payload）与其来源光标。
enum PointEdit {
    Main,
    Extra(usize),
}

impl EditorCore {
    /// 是否存在附加光标（空集 = 单光标恒等退化，全部新路径短路）。
    pub(crate) fn has_multi(&self) -> bool {
        !self.extra_cursors.is_empty()
    }

    /// 折叠为单光标。返回是否原本存在附加光标（调用方据此刷新）。
    pub(crate) fn collapse_multi(&mut self) -> bool {
        let had = self.has_multi();
        self.extra_cursors.clear();
        // P328：集合没了，那份"逐点组尾"也就没有归属对象——不清的话，日后重新
        // 造出的集合若恰好对上旧偏移串，会把不相干的两次输入并成一组。
        // 单光标那份组状态**故意不动**（折叠后的回退路径要继续用它，改它就是改行为）。
        self.typing_run_multi = None;
        had
    }

    /// 维持「文档位序升序、同位去重（优先保留带选区者）」的不变量。
    /// 主光标位置不参与去重——附加光标落在主光标位即无意义，新增时拒。
    fn sort_dedup_cursors(&mut self) {
        self.extra_cursors
            .sort_by_key(|e| (e.cursor.line, e.cursor.col, e.anchor.is_none()));
        self.extra_cursors
            .dedup_by_key(|e| (e.cursor.line, e.cursor.col));
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
        if let Some(i) = self.extra_cursors.iter().position(|e| e.cursor == at) {
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
            let step_ok = if right { col < len } else { col > 0 };
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
            // measure_ink_box 注释；IME 候选框定位同样受益）
            y: (v as f32 - self.scroll_top) * self.line_height() + self.ink_offset,
            width: CARET_WIDTH,
            height: (self.line_height() - 2.0 * self.ink_offset).max(CARET_WIDTH * 2.0),
        }
    }

    /// 光标的非空选区字符偏移区间（无选区/零宽 = None）。主光标与附加
    /// 光标共用：附加光标的 anchor 语义与主 anchor 相同（anchor..cursor）。
    fn cursor_selection_offsets(
        &self,
        anchor: Option<CursorPos>,
        cursor: CursorPos,
    ) -> Option<(usize, usize)> {
        let a = anchor?;
        let (lo, hi) = if (a.line, a.col) <= (cursor.line, cursor.col) {
            (a, cursor)
        } else {
            (cursor, a)
        };
        let s = self.doc.line_to_char(lo.line) + lo.col.min(self.line_display_len(lo.line));
        let e = self.doc.line_to_char(hi.line) + hi.col.min(self.line_display_len(hi.line));
        (s < e).then_some((s, e))
    }

    /// 编辑区间是否跨行（区间内字符不在同一逻辑行）。多光标同步编辑
    /// 一期只做行内编辑——跨行者整批折叠回退（设计 §3.2 约束）。
    fn range_crosses_lines(&self, s: usize, e: usize) -> bool {
        s < e && self.doc.char_to_line(s) != self.doc.char_to_line(e - 1)
    }

    /// B10 Phase 2 同步编辑（设计 §3.2）：收集多光标全集（主 + 附加）
    /// 的编辑区间，按全文字符偏移**从后往前**逐点应用——后面的偏移不
    /// 受前面 splice 影响；**单快照一次撤销**（一期不做多光标打字成组，
    /// 已知取舍见设计 §3.2 第 3 点）。跨行区间（选区跨行 / 触行界的
    /// 退格与删除）或区间重叠超出一期约束：折叠全部光标返回 None，由
    /// 调用方改走普通单光标路径。
    ///
    /// 返回 `None` = 本路径不适用（调用方须用既有单光标函数兜底）；
    /// `Some(changed)` = 已完成（changed 决定置脏/自动保存，与
    /// backspace/delete_forward 的返回值同口径）。
    pub(crate) fn multi_edit(&mut self, kind: MultiEditKind<'_>) -> Option<bool> {
        if !self.has_multi() {
            return None;
        }
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        let payload = match &kind {
            MultiEditKind::Insert(text) => {
                let t = self.doc.line_ending().normalize(text);
                if t.is_empty() {
                    return Some(false); // 空文本 no-op，不产快照
                }
                t
            }
            _ => String::new(),
        };
        // 覆写模式只服务单光标打字语义（P125），多光标插入一律纯插入。
        let mut edits: Vec<(usize, usize, PointEdit)> = Vec::new();
        // 主光标
        let main_range = self
            .cursor_selection_offsets(self.anchor, self.cursor)
            .or_else(|| match kind {
                MultiEditKind::Insert(_) => Some((self.cursor_offset(), self.cursor_offset())),
                _ => None,
            });
        match kind {
            MultiEditKind::Insert(_) => {
                if let Some((s, e)) = main_range {
                    if self.range_crosses_lines(s, e) {
                        self.collapse_multi();
                        return None;
                    }
                    edits.push((s, e, PointEdit::Main));
                }
            }
            MultiEditKind::Backspace | MultiEditKind::Delete => {
                let back = matches!(kind, MultiEditKind::Backspace);
                if let Some((s, e)) = main_range {
                    if self.range_crosses_lines(s, e) {
                        self.collapse_multi();
                        return None;
                    }
                    edits.push((s, e, PointEdit::Main));
                } else {
                    let off = self.cursor_offset();
                    let col = self.cursor.col;
                    if back {
                        if col > 0 {
                            edits.push((off - 1, off, PointEdit::Main));
                        } else if self.cursor == CursorPos::default() {
                            // 文档原点退格：该光标 no-op，其余光标照常
                        } else {
                            // 行首退格 = 跨行并行，一期不做
                            self.collapse_multi();
                            return None;
                        }
                    } else if col < self.line_display_len(self.cursor.line) {
                        edits.push((off, off + 1, PointEdit::Main));
                    } else if off >= self.doc.text_len() {
                        // 文档末尾删除：该光标 no-op，其余光标照常
                    } else {
                        // 行尾删除 = 吞换行单元并下一行，一期不做
                        self.collapse_multi();
                        return None;
                    }
                }
            }
        }
        // 附加光标（anchor 语义与主光标一致；点编辑区间构造同上）
        for (i, ec) in self.extra_cursors.iter().enumerate() {
            let off = self.doc.line_to_char(ec.cursor.line) + ec.cursor.col;
            let sel = self.cursor_selection_offsets(ec.anchor, ec.cursor);
            match kind {
                MultiEditKind::Insert(_) => {
                    let (s, e) = sel.unwrap_or((off, off));
                    if self.range_crosses_lines(s, e) {
                        self.collapse_multi();
                        return None;
                    }
                    edits.push((s, e, PointEdit::Extra(i)));
                }
                MultiEditKind::Backspace => {
                    if let Some((s, e)) = sel {
                        if self.range_crosses_lines(s, e) {
                            self.collapse_multi();
                            return None;
                        }
                        edits.push((s, e, PointEdit::Extra(i)));
                    } else if ec.cursor.col > 0 {
                        edits.push((off - 1, off, PointEdit::Extra(i)));
                    } else {
                        // 附加光标行首退格 = 跨行，一期约束 → 整批回退
                        self.collapse_multi();
                        return None;
                    }
                }
                MultiEditKind::Delete => {
                    if let Some((s, e)) = sel {
                        if self.range_crosses_lines(s, e) {
                            self.collapse_multi();
                            return None;
                        }
                        edits.push((s, e, PointEdit::Extra(i)));
                    } else if ec.cursor.col < self.line_display_len(ec.cursor.line) {
                        edits.push((off, off + 1, PointEdit::Extra(i)));
                    } else if off < self.doc.text_len() {
                        // 附加光标行尾删除 = 吞换行单元，一期约束 → 回退
                        self.collapse_multi();
                        return None;
                    }
                    // 文档末尾（off == text_len）：no-op，不参与
                }
            }
        }
        // 区间两两不干扰是「从后往前独立 splice」的前提（同位光标已在
        // 增删时去重，但选区可能包住其他光标）。两类干扰：
        // ① 严格交叠（含零宽点落在区间内部）；② 同起点但一宽一零——
        // 插入点与删除区间共享起点的退化情形语义不明，整批折叠回退。
        // 同起点同宽（双零宽插入同点）放行：落点修正按应用次序处理。
        edits.sort_by_key(|(s, e, _)| (*s, *e));
        for w in edits.windows(2) {
            let interior = w[0].1 > w[1].0;
            let same_start_mixed = w[0].0 == w[1].0 && w[0].1 != w[1].0;
            if interior || same_start_mixed {
                self.collapse_multi();
                return None;
            }
        }
        if payload.is_empty() && edits.iter().all(|(s, e, _)| s == e) {
            return Some(false); // 全体 no-op（无实编辑），不产快照
        }
        // P328 多光标成组打字（关掉设计 §3.2 第 3 点那条「首版不做，待按手感点单」的
        // 取舍）。合格条件与单光标 `insert_str` 逐条对齐：载荷是**单个非换行字符**、
        // **全部**点编辑都是零宽（没有谁的选区被吃掉）、且每个点的起点恰好等于上一字符
        // 在该点插入后的结束偏移。第三条是关键——中途改过集合（Alt+点击／Ctrl+M／拆行）、
        // 动过光标、撤销/重做都经 `break_typing` 把这份组状态清掉了。删除类载荷为空 ⇒
        // 恒开新组，与单光标 backspace/delete 不并入组同口径。
        let single_char = payload.chars().count() == 1
            && !matches!(payload.chars().next(), Some('\n') | Some('\r'));
        let all_zero_width = edits.iter().all(|(s, e, _)| s == e);
        let starts: Vec<usize> = edits.iter().map(|(s, _, _)| *s).collect(); // 此刻 edits 已按 (s,e) 升序
        let merges =
            single_char && all_zero_width && self.typing_run_multi.as_ref() == Some(&starts);
        if !merges {
            self.snapshot(); // 开新组：单快照＝一次撤销撤掉整步同步编辑
        }
        // 成组期间单光标那份组状态恒为 None（两条链不许互相顶班；不合并时 snapshot 已清，
        // 合并路径不经快照 ⇒ 这里显式钉住）
        self.typing_run = None;
        // 从后往前应用（按 (start,end) 降序）；应用后回填各光标落点
        edits.sort_by_key(|(s, e, _)| std::cmp::Reverse((*s, *e)));
        let min_start = edits.last().map(|(s, _, _)| *s).unwrap_or(0);
        for (s, e, _) in &edits {
            if payload.is_empty() {
                self.doc.remove_range(*s, *e);
            } else {
                if s < e {
                    self.doc.remove_range(*s, *e);
                }
                self.doc.insert(*s, &payload);
                // 跨行插入平移下方书签（insert_str 同口径；行内插入
                // delta=0 天然 no-op）。从后往前应用，逐点平移可叠加。
                let (new_lines, _) = measure_insertion(&payload);
                if new_lines > 0 {
                    let at_line = self.doc.char_to_line(*s);
                    self.remap_shift_below(at_line, new_lines as isize);
                }
            }
        }
        // 落点回填：先应用的（高偏移）编辑落点会被后应用的（低偏移）
        // 编辑净长度变化平移——修正到最终文档坐标系。起点严格更小者
        // 取其 delta；**同起点**者：数组序在前的先应用、被后应用的插
        // 入顶到上方，故本编辑的落点要加上「排在后面（尚未应用）的
        // 同起点编辑」数量 × 单点净增。（delta = 插入净增 / 删除净减；
        // O(n²)、n ≤ 光标数封顶 1000 可接受。）
        let payload_len = payload.chars().count() as i64;
        let delta_of = |(s, e): (usize, usize)| -> i64 {
            if payload.is_empty() {
                -((e - s) as i64)
            } else {
                payload_len - (e - s) as i64
            }
        };
        let mut landed: Vec<(PointEdit, CursorPos)> = Vec::with_capacity(edits.len());
        let mut ends: Vec<usize> = Vec::with_capacity(edits.len());
        let mut changed = false;
        for (i, &(s, .., ref owner)) in edits.iter().enumerate() {
            changed = true;
            let final_s = (s as i64
                + edits[i + 1..].iter().filter(|ej| ej.0 == s).count() as i64 * payload_len
                + edits
                    .iter()
                    .filter(|ej| ej.0 < s)
                    .map(|ej| delta_of((ej.0, ej.1)))
                    .sum::<i64>())
            .max(0) as usize;
            let end_off = if payload.is_empty() {
                final_s
            } else {
                final_s + payload.chars().count()
            };
            let line = self.doc.char_to_line(end_off);
            let col = end_off - self.doc.line_to_char(line);
            ends.push(end_off); // P328：组尾按**最终文档坐标**记，下一字符才比得上
            landed.push((
                match owner {
                    PointEdit::Main => PointEdit::Main,
                    PointEdit::Extra(i) => PointEdit::Extra(*i),
                },
                CursorPos { line, col },
            ));
        }
        for (owner, pos) in landed {
            match owner {
                PointEdit::Main => {
                    self.cursor = pos;
                    self.anchor = None;
                }
                PointEdit::Extra(i) => {
                    if let Some(ec) = self.extra_cursors.get_mut(i) {
                        ec.cursor = pos;
                        ec.anchor = None;
                    }
                }
            }
        }
        self.sort_dedup_cursors(); // 应用后可能并位，恢复位序不变量
                                   // P328：合格的一组把组尾留给下一字符比对（升序，逐点）；多字符／换行／
                                   // 有人带选区被吃／删除类一律置 None ⇒ 下一击开新组。
        ends.sort_unstable();
        self.typing_run_multi = if single_char && all_zero_width {
            Some(ends)
        } else {
            None
        };
        self.typing_run = None; // 成组期间单光标那条组恒为 None（两条链不互相顶班）
        self.invalidate_highlight_from(min_start);
        self.ensure_visible(); // 只跟随主光标（设计 §4 #1）
        Some(changed)
    }
    /// 主光标处的词范围（行内字符列区间，词口径与 word_neighbor 一致：
    /// 字母数字或 `_`）。光标在词内向两侧扩展；在词尾向左扩展；不在
    /// 词上返回 None。
    fn word_range_at_cursor(&self) -> Option<(usize, usize)> {
        fn is_word_char(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        let chars: Vec<char> = self.line_text(self.cursor.line).chars().collect();
        let col = self.cursor.col.min(chars.len());
        let on_word = chars.get(col).copied();
        let before_word = (col > 0).then(|| chars[col - 1]);
        if on_word.is_some_and(is_word_char) {
            let mut s = col;
            while s > 0 && is_word_char(chars[s - 1]) {
                s -= 1;
            }
            let mut e = col;
            while e < chars.len() && is_word_char(chars[e]) {
                e += 1;
            }
            Some((s, e))
        } else if before_word.is_some_and(is_word_char) {
            let mut s = col;
            while s > 0 && is_word_char(chars[s - 1]) {
                s -= 1;
            }
            Some((s, col))
        } else {
            None
        }
    }

    /// B10 Phase 2：添加下一匹配（Ctrl+M，设计 §3.3）。检索文本 = 主
    /// 光标选区（有则用），否则主光标处词；从主光标选区末尾向后线性
    /// 扫描第一个**未占用**实例，扫到文档尾回绕开头（当前选区自身因
    /// 占用判定天然跳过）；命中 → 新增附加光标并让其实例带选区
    /// （anchor=实例起点），纵向滚动收敛让新实例进入视口。不改文档、
    /// 不置脏；`Err` = 拒绝原因（由调用方按界面语言取文案，见
    /// [`crate::editor::EditErr::text`]）。
    pub(crate) fn add_next_match(&mut self) -> Result<bool, EditErr> {
        if self.wrap.borrow().enabled || self.block_sel.is_some() || self.preedit.is_some() {
            return Ok(false);
        }
        let (needle, scan_from) = if let Some(text) = self.selected_text() {
            let end = self.selection_offsets().map(|(_, e)| e).unwrap_or(0);
            (text, end)
        } else {
            let (ws, we) = self
                .word_range_at_cursor()
                .ok_or(EditErr::CursorNotOnWord)?;
            let line_off = self.doc.line_to_char(self.cursor.line);
            let needle: String = self
                .line_text(self.cursor.line)
                .chars()
                .skip(ws)
                .take(we - ws)
                .collect();
            (needle, line_off + we)
        };
        if self.extra_cursors.len() >= MAX_EXTRA_CURSORS {
            return Err(EditErr::ExtraCursorCap(MAX_EXTRA_CURSORS));
        }
        // 占用区间 = 各光标的选区/落点 + 主光标**当前实例**（选区本身或
        // 光标处词）——环形回绕扫到起点时不得把当前实例再加进来；命中
        // 实例与之相交即跳过（零宽点只在实例**内部**才算占用——贴边
        // 不算，主流同口径）
        let mut occupied: Vec<(usize, usize)> = Vec::new();
        if let Some((s, e)) = self.cursor_selection_offsets(self.anchor, self.cursor) {
            occupied.push((s, e));
        } else if let Some((ws, we)) = self.word_range_at_cursor() {
            let line_off = self.doc.line_to_char(self.cursor.line);
            occupied.push((line_off + ws, line_off + we));
        } else {
            occupied.push((self.cursor_offset(), self.cursor_offset()));
        }
        for ec in &self.extra_cursors {
            occupied.push(
                self.cursor_selection_offsets(ec.anchor, ec.cursor)
                    .unwrap_or((
                        self.doc.line_to_char(ec.cursor.line) + ec.cursor.col,
                        self.doc.line_to_char(ec.cursor.line) + ec.cursor.col,
                    )),
            );
        }
        let is_taken = |ms: usize, me: usize| {
            occupied.iter().any(|&(os, oe)| {
                if os == oe {
                    os > ms && os < me
                } else {
                    ms < oe && os < me
                }
            })
        };
        let hay = self.doc.slice_text(0, self.doc.text_len());
        // 环形扫描：先 [scan_from, 文档尾)，再回绕 [0, scan_from)。
        // str::match_indices 为原生快速搜索，全文一遍 O(n)（设计 §4 #14
        // 的 4M 步封顶按「比照括号匹配」口径由线性原生搜索天然满足）。
        // 单位口径：hay 是 String（字节索引），而 scan_from/占用表是
        // 字符偏移——起点先经 char_to_byte 换算，命中 [字节, 字节+needle
        // 字节数] 再经 byte_to_char 换回字符口径，才与占用表和 ropey
        // 行列 API 对接。曾把字符偏移直接当字节下标切 String：CJK 文档
        // 上 byte index not a char boundary panic 或定位错乱（P145 回归）。
        let scan_from_b = self.doc.char_to_byte(scan_from);
        let mut hit: Option<(usize, usize)> = None;
        for (base, range) in [(scan_from_b, &hay[scan_from_b..]), (0, &hay[..scan_from_b])] {
            for (off, _) in range.match_indices(&needle) {
                let ms = self.doc.byte_to_char(base + off);
                let me = self.doc.byte_to_char(base + off + needle.len());
                if !is_taken(ms, me) {
                    hit = Some((ms, me));
                    break;
                }
            }
            if hit.is_some() {
                break;
            }
        }
        let Some((ms, me)) = hit else {
            return Err(EditErr::NoMoreMatch);
        };
        let sl = self.doc.char_to_line(ms);
        let sc = ms - self.doc.line_to_char(sl);
        let el = self.doc.char_to_line(me);
        let ec = me - self.doc.line_to_char(el);
        self.extra_cursors.push(ExtraCursor {
            cursor: CursorPos { line: el, col: ec },
            anchor: Some(CursorPos { line: sl, col: sc }),
        });
        self.sort_dedup_cursors();
        self.break_typing();
        // 纵向收敛让新实例进入视口（ensure_visible 只锚主光标——设计
        // §4 #1——故这里对新增实例单独做一次行级收敛；折行态已拒，
        // 逻辑行即视觉行）。水平不追：词选区通常落在可视宽度内。
        let rows_visible = (self.viewport_h / self.line_height()).max(1.0);
        let row = el as f32;
        if row < self.scroll_top {
            self.scroll_top = row;
        } else if row > self.scroll_top + rows_visible - 1.0 {
            self.scroll_top = row - rows_visible + 1.0;
        }
        self.clamp_scroll();
        Ok(true)
    }

    /// B10 二期（roadmap §9 第 7 行「拆行多选」）：把选区按行拆开——选区触及的
    /// 每一条逻辑行在**正文末尾**各落一条光标，随后打字／退格即逐点同步（走既有
    /// [`Self::multi_edit`] 管线）。不改文档、不置脏、不产快照。
    ///
    /// 守卫与 [`Self::add_next_match`] 同一口径：结构性互斥（折行开态／列块态／
    /// 组字态）静默 `Ok(false)`；面向用户的理由给 `Err`（无选区／超封顶——被拒的
    /// 尝试**全有或全无**，连用户已有的集合都不动，所以清理只能放在守卫之后）。
    /// 只触及一行也是 `Ok(false)` 不改动：这条命令唯一的承诺是「产生多个光标」，
    /// 单行时产生不出第二个，把用户选区吃掉是净损失。
    pub(crate) fn split_selection_by_lines(&mut self) -> Result<bool, EditErr> {
        if self.wrap.borrow().enabled || self.block_sel.is_some() || self.preedit.is_some() {
            return Ok(false);
        }
        if self.anchor.is_none() {
            return Err(EditErr::NoSelection);
        }
        // 行区间口径复用 touched_lines()：它已经把「选区末点落在行首 ⇒ 该行不算
        // 触及」与尾随换行的幻影末行两条规则付过学费并钉了用例，这里再算一份
        // 就是本仓第 7 次口径分叉。
        let (first, last) = self.touched_lines();
        if last <= first {
            return Ok(false); // 含"锚点与光标同位"的零宽选区
        }
        let count = last - first + 1;
        if count - 1 > MAX_EXTRA_CURSORS {
            return Err(EditErr::ExtraCursorCap(MAX_EXTRA_CURSORS));
        }
        // 主光标留在它自己那一行（区间内原样、区间外夹紧）：`ensure_visible` 只锚
        // 主光标（设计 §4 #1），换行做主光标会让画面跳到用户没看的地方。
        let main_line = self.cursor.line.clamp(first, last);
        // 落点一律只问 line_display_len（＝ Document::line_body_len_chars，O(1)
        // 且不物化整行）。逐行取串在本仓主夹具（单行 50 MB 日志）上＝把整份文件
        // 抄 N 遍——P298 那条账已经把这种写法否掉了。
        let mut extras: Vec<ExtraCursor> = Vec::with_capacity(count - 1);
        for line in first..=last {
            if line == main_line {
                continue;
            }
            let col = self.line_display_len(line);
            extras.push(ExtraCursor {
                cursor: CursorPos { line, col },
                anchor: None,
            });
        }
        // 守卫之后才动状态：整体换新（不是并入）——旧集合留下来会与新集合的
        // 位置撞在一起，而 sort_dedup_cursors 只在附加集内部去重、**从不与主光标
        // 比**，附加光标压在主光标位＝同一行插两份（multi_edit 的干扰判定对"同起点
        // 两个零宽插入"是放行的），屏幕上还看不出来。
        self.extra_cursors = extras;
        let main_col = self.line_display_len(main_line);
        self.cursor = CursorPos {
            line: main_line,
            col: main_col,
        };
        // 主选区必须落掉：留着跨行选区，下一次插入会撞 multi_edit 的
        // range_crosses_lines ⇒ 整批折叠回退单光标 ⇒ 功能看着是坏的。
        self.anchor = None;
        self.goal_px = None; // 主光标被挪到行末 ⇒ 竖向 goal 作废（同行内水平步进）
        self.sort_dedup_cursors();
        self.break_typing();
        self.ensure_visible();
        Ok(true)
    }
}
