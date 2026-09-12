use super::*;

impl EditorCore {
    pub fn select_all(&mut self) {
        self.break_typing(); // P37：选区变更打断组
        self.goal_px = None; // 第 73 轮 ⑯：选区变更 = 非竖向操作
        self.clear_block(); // 第 67 轮：块态与单选区互斥
        self.collapse_multi(); // B10：全选重置为单光标（白名单外动作）
        let last = self.doc.line_count().saturating_sub(1);
        self.anchor = Some(CursorPos::default());
        self.cursor = CursorPos {
            line: last,
            col: self.line_display_len(last),
        };
        self.ensure_visible();
    }

    /// 当前选区文本（无选区或零宽返回 None）。
    pub fn selected_text(&self) -> Option<String> {
        let (start_off, end_off) = self.selection_offsets()?;
        if start_off >= end_off {
            return None;
        }
        Some(self.doc.slice_text(start_off, end_off))
    }

    /// 选区的字符偏移区间（有序）。
    pub(crate) fn selection_offsets(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (a, b) = if (anchor.line, anchor.col) <= (self.cursor.line, self.cursor.col) {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        let start = self.doc.line_to_char(a.line) + a.col.min(self.line_display_len(a.line));
        let end = self.doc.line_to_char(b.line) + b.col.min(self.line_display_len(b.line));
        Some((start.min(end), end.max(start)))
    }

    /// 光标邻接括号与其配对位置（渲染高亮与跳转共用的查询，带缓存）。
    ///
    /// 邻接口径：光标**右侧**字符是括号优先（光标停在括号前），否则看
    /// **左侧**字符（光标停在括号后）。返回 `(括号偏移, 配对偏移)`——
    /// 全文字符偏移口径（CRLF 的 `\r` 计 1 字符）。无邻接括号 / 扫描
    /// 超上限 / 未配平 → None。
    pub fn bracket_match(&self) -> Option<(usize, usize)> {
        let key = self.cursor;
        if let Some(hit) = self.bracket_cache.borrow().as_ref() {
            if hit.0 == key {
                return hit.1;
            }
        }
        let result = self.bracket_match_uncached();
        *self.bracket_cache.borrow_mut() = Some((key, result));
        result
    }

    /// [`Self::bracket_match`] 的无缓存实现。
    pub(crate) fn bracket_match_uncached(&self) -> Option<(usize, usize)> {
        let off = self.doc.line_to_char(self.cursor.line)
            + self.cursor.col.min(self.line_display_len(self.cursor.line));
        // 右侧字符优先，其次左侧；char_at 越界返回 None 天然覆盖文档尾
        let probe = self
            .char_at(off)
            .and_then(bracket_kind)
            .map(|k| (off, k))
            .or_else(|| {
                (off > 0)
                    .then(|| self.char_at(off - 1))
                    .flatten()
                    .and_then(bracket_kind)
                    .map(|k| (off - 1, k))
            });
        let (boff, (open, close, is_open)) = probe?;
        let other = if is_open {
            boff + scan_forward(open, close, self.doc.chars_from(boff + 1))?
        } else {
            boff - self.scan_backward_chunks(boff, open, close)?
        };
        Some((boff, other))
    }

    /// 反向括号扫描（第 61 轮）：`boff` 处是**闭**括号，向文档头找配对
    /// 开括号，返回**含端字符距离**（`配对偏移 = boff - 返回值`）。
    ///
    /// 实现注记：ropey 1.6 的字符迭代器不支持反向（Chars 非
    /// DoubleEndedIterator），故按 64K 字符分块取切片、块内逆序计数——
    /// 缓冲有界（块大小 × 1），不产生全文 String；总步数受
    /// [`MAX_BRACKET_SCAN_CHARS`] 封顶（与正向同口径）。
    pub(crate) fn scan_backward_chunks(
        &self,
        boff: usize,
        open: char,
        close: char,
    ) -> Option<usize> {
        const CHUNK: usize = 65536;
        let mut consumed = 0usize;
        let mut depth = 1usize;
        let mut buf: Vec<char> = Vec::with_capacity(CHUNK.min(boff));
        let mut end = boff;
        loop {
            let start = end.saturating_sub(CHUNK);
            buf.clear();
            buf.extend(self.doc.slice_text(start, end).chars());
            for &c in buf.iter().rev() {
                consumed += 1;
                if consumed > MAX_BRACKET_SCAN_CHARS {
                    return None;
                }
                if c == close {
                    depth += 1;
                } else if c == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(consumed);
                    }
                }
            }
            if start == 0 {
                return None;
            }
            end = start;
        }
    }

    /// 跳到配对括号的另一侧。光标停在括号前 → 落到配对括号后；停在
    /// 括号后 → 落到配对括号前（保持「相对括号同侧」的手感，来回按即
    /// 在两侧往返）。无邻接括号/无匹配返回 false 不动。纯光标移动：
    /// 不产生快照，按 P37 口径打断打字组。
    pub fn jump_to_matching_bracket(&mut self) -> bool {
        let Some((boff, other)) = self.bracket_match() else {
            return false;
        };
        // B10：跳转重置为单光标（白名单外动作，与 select_span 同口径）
        self.collapse_multi();
        let cursor_off = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        let target = if cursor_off <= boff { other + 1 } else { other };
        let target = target.min(self.doc.text_len());
        let line = self.doc.char_to_line(target);
        let col = target - self.doc.line_to_char(line);
        self.break_typing();
        self.anchor = None;
        self.cursor = CursorPos { line, col };
        self.ensure_visible();
        true
    }

    /// 应用光标移动；`extend` 为 true 时保持锚点形成选区。
    pub fn apply_motion(&mut self, motion: Motion, extend: bool) {
        // B10：多光标态仅行内 Left/Right（不扩展）保持存活——附加光标
        // 逐个步进（触行界折叠该光标），主光标走既有完整语义；其余动
        // 作（竖向/Home/End/词导航…）折叠后走单光标既有路径。
        if self.has_multi() {
            if !extend {
                match motion {
                    Motion::Left => {
                        self.break_typing();
                        self.clear_vertical_goal();
                        if let Some(left) = self.step_main_horizontal(false) {
                            self.cursor = left;
                        }
                        self.multi_step_horizontal(false);
                        self.ensure_visible();
                        return;
                    }
                    Motion::Right => {
                        self.break_typing();
                        self.clear_vertical_goal();
                        if let Some(right) = self.step_main_horizontal(true) {
                            self.cursor = right;
                        }
                        self.multi_step_horizontal(true);
                        self.ensure_visible();
                        return;
                    }
                    _ => {}
                }
            }
            self.collapse_multi();
        }
        self.break_typing(); // P37：光标移动打断组
        self.clear_block(); // 第 67 轮：键盘移动退出块态（块内编辑走专属分支）
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        if !extend {
            self.anchor = None;
        }
        self.move_local(motion);
        self.ensure_visible();
    }

    /// B10：主光标行内一步（与附加光标同一「行界即折叠」口径——行首
    /// Left / 行尾 Right 返回 None，由调用方决定折叠或保持）。独立于
    /// move_local 的跨行语义：多光标态所有光标统一行内步进口径。
    fn step_main_horizontal(&mut self, right: bool) -> Option<CursorPos> {
        let len = self.line_display_len(self.cursor.line);
        let col = self.cursor.col;
        if right {
            if col < len {
                Some(CursorPos { line: self.cursor.line, col: col + 1 })
            } else {
                None
            }
        } else if col > 0 {
            Some(CursorPos { line: self.cursor.line, col: col - 1 })
        } else {
            None
        }
    }

    pub(crate) fn move_local(&mut self, motion: Motion) {
        let page_rows = ((self.viewport_h / self.line_height()) as usize).max(1);
        let last_line = self.doc.line_count().saturating_sub(1);

        // 第 73 轮 ⑯：软换行开态，Up/Down/PageUp/PageDown 按视觉行行走
        // （goal-column 口径）；其余动作为非竖向操作，先清 goal
        if self.wrap.borrow().enabled {
            match motion {
                Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                    let Some((line, col)) = self.vertical_target(motion, page_rows) else {
                        return;
                    };
                    self.cursor = CursorPos {
                        line,
                        col: col.min(self.line_display_len(line)),
                    };
                    return;
                }
                _ => self.goal_px = None,
            }
        }

        match motion {
            Motion::Left => {
                self.goal_px = None;
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                } else if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.line_display_len(self.cursor.line);
                }
            }
            Motion::Right => {
                self.goal_px = None;
                if self.cursor.col < self.line_display_len(self.cursor.line) {
                    self.cursor.col += 1;
                } else if self.cursor.line < last_line {
                    self.cursor.line += 1;
                    self.cursor.col = 0;
                }
            }
            Motion::Up => {
                if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.cursor.col.min(self.line_display_len(self.cursor.line));
                }
            }
            Motion::Down => {
                if self.cursor.line < last_line {
                    self.cursor.line += 1;
                    self.cursor.col = self.cursor.col.min(self.line_display_len(self.cursor.line));
                }
            }
            Motion::Home => {
                self.goal_px = None;
                // P134（路线图 C8）：软换行开态到**视觉行**首（主流
                // wrapping-aware 口径）；关态恒等退化为逻辑行首
                if self.wrap_enabled() {
                    let v = self.visual_row_of(self.cursor.line, self.cursor.col);
                    let (line, _, seg_start, _) = self.locate_visual(v);
                    self.cursor.line = line;
                    self.cursor.col = seg_start;
                } else {
                    self.cursor.col = 0
                }
            }
            Motion::End => {
                self.goal_px = None;
                // P134（C8）：开态到视觉行尾（末段 = 逻辑行尾，段末列已
                // 按 line_text 剥 EOL 口径，与 line_display_len 一致）
                if self.wrap_enabled() {
                    let v = self.visual_row_of(self.cursor.line, self.cursor.col);
                    let (line, _, _, seg_end) = self.locate_visual(v);
                    self.cursor.line = line;
                    self.cursor.col = seg_end;
                } else {
                    self.cursor.col = self.line_display_len(self.cursor.line)
                }
            }
            // P134（C8）：逻辑行首/尾（Alt+Home/End）——开态穿越折行段
            // 直达逻辑行边界，关态与 Home/End 恒等
            Motion::LogicalHome => {
                self.goal_px = None;
                self.cursor.col = 0
            }
            Motion::LogicalEnd => {
                self.goal_px = None;
                self.cursor.col = self.line_display_len(self.cursor.line)
            }
            Motion::PageUp => {
                self.cursor.line = self.cursor.line.saturating_sub(page_rows);
                self.cursor.col = self.cursor.col.min(self.line_display_len(self.cursor.line));
            }
            Motion::PageDown => {
                self.cursor.line = (self.cursor.line + page_rows).min(last_line);
                self.cursor.col = self.cursor.col.min(self.line_display_len(self.cursor.line));
            }
            Motion::DocStart => {
                self.goal_px = None;
                self.cursor = CursorPos::default()
            }
            Motion::DocEnd => {
                self.goal_px = None;
                self.cursor = CursorPos {
                    line: last_line,
                    col: self.line_display_len(last_line),
                };
            }
            // P122：词级导航（行首空白再往左落列 0，下一拍才跨行）
            Motion::WordLeft | Motion::WordRight => {
                self.goal_px = None;
                if let Some((line, col)) = self.word_neighbor(motion == Motion::WordLeft) {
                    self.cursor = CursorPos { line, col };
                }
            }
        }
    }

    /// P122：从光标出发的词级移动目标（Ctrl+←/→ 与删词共用同一口径）。
    ///
    /// - 词字符 = Unicode 字母数字或 `_`（CJK 连续段视为一个词）；
    /// - 行内先跳过紧邻的空白（空格/Tab），再按「词字符连续段整体一步、
    ///   单个标点一步」前进/后退——与主流编辑器的词移动手感一致；
    /// - 行首再往左 = 上一行行尾，行尾再往右 = 下一行行首（跨行不截断）；
    ///   缩进内再往左落到列 0（下一拍才跨行）；
    /// - 到文档边缘返回 None。返回 `(line, col)`，col 为字符列（与
    ///   `CursorPos` 同口径）。
    pub(crate) fn word_neighbor(&self, left: bool) -> Option<(usize, usize)> {
        fn is_word_char(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        let count = self.doc.line_count();
        let (line, col) = (self.cursor.line, self.cursor.col);
        let body_len = self.line_display_len(line);
        let start = self.doc.line_to_char(line);
        if left {
            if col == 0 {
                return if line == 0 {
                    None
                } else {
                    Some((line - 1, self.line_display_len(line - 1)))
                };
            }
            let mut o = start + col;
            while o > start {
                match self.char_at(o - 1) {
                    Some(' ') | Some('\t') => o -= 1,
                    _ => break,
                }
            }
            if o == start {
                return Some((line, 0)); // 行内只剩空白：落列 0
            }
            if self.char_at(o - 1).is_some_and(is_word_char) {
                while o > start && self.char_at(o - 1).is_some_and(is_word_char) {
                    o -= 1;
                }
            } else {
                o -= 1; // 单个标点一步
            }
            Some((line, o - start))
        } else {
            if col >= body_len {
                return if line + 1 < count {
                    Some((line + 1, 0))
                } else {
                    None
                };
            }
            let end = start + body_len;
            let mut o = start + col;
            while o < end {
                match self.char_at(o) {
                    Some(' ') | Some('\t') => o += 1,
                    _ => break,
                }
            }
            if o == end {
                return if line + 1 < count {
                    Some((line + 1, 0))
                } else {
                    Some((line, body_len))
                };
            }
            if self.char_at(o).is_some_and(is_word_char) {
                while o < end && self.char_at(o).is_some_and(is_word_char) {
                    o += 1;
                }
            } else {
                o += 1;
            }
            Some((line, o - start))
        }
    }

    /// 跳转到第 `line_1based` 行行首（1 起）。
    pub fn jump_to_line(&mut self, line_1based: usize) {
        self.break_typing(); // P37：跳转打断组
        self.goal_px = None; // 第 73 轮 ⑯：跳转 = 非竖向操作
        let target = (line_1based.saturating_sub(1)).min(self.doc.line_count().saturating_sub(1));
        self.anchor = None;
        self.cursor = CursorPos {
            line: target,
            col: 0,
        };
        self.ensure_visible();
    }

    /// 会话恢复的无副作用定位入口（P30）：把光标与滚动放回上次退出时的位置。
    ///
    /// 与编辑/移动入口的区别：不产生撤销快照、不触发置脏、不唤醒光标闪烁；
    /// 行列越界夹紧到文档范围，滚动值先消毒（非有限归零）再按行程钳制。
    /// 视口尺寸在首次布局前未知——此处只做行数域的宽松钳制，
    /// 像素级精钳制由布局后的 [`Self::set_viewport_height`] 收口。
    pub fn restore_view(&mut self, line: usize, col: usize, scroll_top: f32, scroll_left: f32) {
        self.break_typing(); // P37：定位打断组（会话恢复/最近文件定位路径）
        self.goal_px = None; // 第 73 轮 ⑯：恢复定位 = 非竖向操作
        let last = self.doc.line_count().saturating_sub(1);
        let line = line.min(last);
        // 列按显示口径夹紧（行尾 \r 不计，与光标移动语义一致）
        let col = col.min(self.line_display_len(line));
        self.anchor = None;
        self.cursor = CursorPos { line, col };
        self.scroll_top = if scroll_top.is_finite() {
            scroll_top
        } else {
            0.0
        };
        self.scroll_left = if scroll_left.is_finite() {
            scroll_left
        } else {
            0.0
        };
        self.clamp_scroll();
        self.clamp_scroll_horizontal();
    }

    pub fn scroll_by_lines(&mut self, lines: f32) {
        self.scroll_top = (self.scroll_top - lines).max(0.0);
        self.clamp_scroll();
        // P53：滚动即活动——竖直滚动条点亮（淡出中的条立即回全显）
        self.touch_scrollbar_activity();
    }

    /// 拖选时视口边缘的自动滚动量（打磨项）：
    /// 返回 `(列增量, 行增量)`——语义与 `scroll_by_columns` /
    /// `scroll_by_lines` 的入参一致（行增量为负 = 视口向文档尾推进）。
    ///
    /// 指针进入控件四缘的窄带即产生滚动，越深入越快（线性放大）；
    /// 位于安全区返回 (0, 0)。事件驱动方案：仅鼠标移动时推进，
    /// 无定时器参与（已知取舍：指针完全静止时不滚动）。
    pub fn edge_scroll_delta(
        &self,
        local_x: f32,
        local_y: f32,
        widget_w: f32,
        widget_h: f32,
    ) -> (f32, f32) {
        const ZONE: f32 = 32.0;

        // 强度 ∈ (0,1]：越贴近边缘滚得越快
        fn strength(dist: f32, zone: f32) -> f32 {
            1.0 + ((zone - dist) / zone).max(0.0) * 3.0
        }

        let mut dy = 0.0f32;
        if local_y < ZONE {
            dy = strength(local_y, ZONE) * SCROLL_LINES_PER_NOTCH;
        } else if local_y > widget_h - ZONE {
            dy = -strength(widget_h - local_y, ZONE) * SCROLL_LINES_PER_NOTCH;
        }

        let mut dx = 0.0f32;
        if local_x < ZONE {
            dx = -strength(local_x, ZONE);
        } else if local_x > widget_w - ZONE {
            dx = strength(widget_w - local_x, ZONE);
        }

        (dx, dy)
    }

    /// 横向滚动 `cols` 列（正数向右看更后的内容，P13）。
    /// Shift+滚轮、触控板横向分量、水平滚动条共用本入口。
    pub fn scroll_by_columns(&mut self, cols: f32) {
        let target = self.scroll_left + cols * self.char_width();
        // 巨量滚动时中间量可能溢出为 ±inf：按方向饱和，交给钳制收口
        self.scroll_left = if target.is_finite() {
            target
        } else if cols > 0.0 {
            f32::MAX
        } else {
            f32::MIN
        };
        self.clamp_scroll_horizontal();
        // P54：横向滚动同样点亮滚动条（两条共用一个活动戳，见 draw）
        self.touch_scrollbar_activity();
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_top = self.scroll_top.max(0.0);
        // 第 73 轮 ⑯：行程按视觉行总数（软换行开态 = 段计数前缀，
        // 关态 = 逻辑行数，恒等退化）
        let rows_total = self.scroll_content_lines() as f32;
        let max = (rows_total - self.viewport_h / self.line_height()).max(0.0);
        // P66：恢复小数滚动位置。P59 的整行对齐（round）是「tiny-skia 对
        // Cached 文本无真裁剪」年代的权宜——半可见行会画出控件边界且部分
        // 重绘清不掉，只能让每行要么完整要么不可见；副作用是滚动永远整行
        // 瞬跳、行号栏钉死在固定槽位（用户反馈「行号不跟随内容移动」）。
        // 现绘制侧改用 start_layer 图层掩码在光栅期硬裁剪一切图元（见
        // draw），半可见行可以安全上屏，滚动回到像素级平滑。
        self.scroll_top = self.scroll_top.min(max + 1.0).max(0.0);
    }

    /// 正文区可视宽度（像素）= 视口宽 − 行号栏宽。
    pub(crate) fn text_viewport_w(&self) -> f32 {
        (self.viewport_w - self.gutter_width()).max(0.0)
    }

    /// 水平行程钳制：左缘不出负，右缘不超出最宽行。
    ///
    /// P45：缩短类编辑/撤销重做后置 [`Self::max_cols_stale`]，这里按
    /// 冷却窗（可重现的限流，避免 50MB 文档每次删除都 O(n) 全扫）惰性
    /// 全量重算——`max_line_cols` 只升不降的旧取舍会让删除超宽行后
    /// 水平滚动条永不消失。重算结果仍取「列模型 ∪ 真实行宽」。
    pub fn clamp_scroll_horizontal(&mut self) {
        // 第 73 轮 ⑯：软换行开态锁水平滚动为 0（设计 §0：hscroll 隐藏、
        // scroll_left 锁 0——折行形态不存在横向行程）
        if self.wrap.borrow().enabled {
            self.scroll_left = 0.0;
            return;
        }
        if self.max_cols_stale {
            let now = std::time::Instant::now();
            if self
                .max_cols_checked
                .is_none_or(|t| now.duration_since(t) >= RECOMPUTE_MAX_COLS_COOLDOWN)
            {
                self.max_cols_checked = Some(now);
                self.recompute_max_line_cols();
                self.max_cols_stale = false;
            }
        }
        // 第 40 轮：真实行宽高水位并取最大——列模型对分数/超宽字形可能
        // 欠估，不补则超宽行（emoji/全宽符号）滚不到头、光标被钳出视口
        let content_w = self.content_width_px();
        let max = (content_w - self.text_viewport_w()).max(0.0);
        if self.scroll_left.is_finite() {
            self.scroll_left = self.scroll_left.clamp(0.0, max);
        } else {
            self.scroll_left = 0.0;
        }
    }

    /// 水平方向的光标可见性：光标像素位置越出左右缘即平移视口。
    pub(crate) fn ensure_visible_horizontal(&mut self) {
        let text = self.line_text(self.cursor.line);
        let col = self.cursor.col.min(text.chars().count());
        // P45：滞后感知——布局新鲜走真实字形位置，滞后回退列模型
        // （实时文本），保证超宽打字立即滚动、不吞字
        let cx = self.px_of(self.cursor.line, &text, col);
        let view_w = self.text_viewport_w();
        if view_w <= 0.0 {
            return;
        }
        if cx < self.scroll_left {
            self.scroll_left = cx;
        } else if cx > self.scroll_left + view_w - CARET_WIDTH {
            // 光标竖线本身也要完整可见
            self.scroll_left = cx - view_w + CARET_WIDTH;
        }
        self.clamp_scroll_horizontal();
    }

    pub(crate) fn ensure_visible(&mut self) {
        self.poke_caret();
        let first = self.scroll_top;
        // P66：可见行数按浮点口径（viewport_h / lh）——半可见行现在照常
        // 绘制，光标收敛目标不再依赖「底缘留白 ≤ 一行」的旧假设；小数
        // scroll_top 下 last 为小数，光标行（整数）越过它即触发平移，
        // 收敛结果与整行对齐时代一致且随平滑滚动连续
        let rows_visible = (self.viewport_h / self.line_height()).max(1.0);
        // 第 73 轮 ⑯：软换行开态按视觉行收敛（光标所在段 = 视觉行）
        if self.wrap.borrow().enabled {
            let cur_v = self.visual_row_of(self.cursor.line, self.cursor.col) as f32;
            let last = self.scroll_top + rows_visible - 1.0;
            if cur_v < first {
                self.scroll_top = cur_v;
            } else if cur_v > last {
                self.scroll_top = cur_v - rows_visible + 1.0;
            }
            self.clamp_scroll();
            // P53：视口确实随光标移动才点亮滚动条（行内打字不无谓点亮）
            if (self.scroll_top - first).abs() > f32::EPSILON {
                self.touch_scrollbar_activity();
            }
            return; // 开态无水平行程（scroll_left 恒 0），横向收敛跳过
        }
        let last = self.scroll_top + rows_visible - 1.0;
        let line = self.cursor.line as f32;
        if line < first {
            self.scroll_top = line;
        } else if line > last {
            self.scroll_top = line - rows_visible + 1.0;
        }
        self.clamp_scroll();
        // P53：视口确实随光标移动才点亮滚动条（行内打字不无谓点亮）
        if (self.scroll_top - first).abs() > f32::EPSILON {
            self.touch_scrollbar_activity();
        }
        self.ensure_visible_horizontal();
    }

    /// 全量重算最大显示列数（整体换文档时调用；O(n)，加载路径本来 O(n)）。
    pub(crate) fn recompute_max_line_cols(&mut self) {
        let count = self.doc.line_count();
        let mut max = 0usize;
        for line in 0..count {
            let cols = display_cols(self.line_text(line).as_str()) as usize;
            if cols > max {
                max = cols;
            }
        }
        self.max_line_cols = max;
    }

    /// 把 `[first_line, last_line]` 内各行宽度并入高水位（编辑后调用）。
    pub(crate) fn raise_max_line_cols(&mut self, lines: std::ops::RangeInclusive<usize>) {
        for line in lines {
            let cols = display_cols(self.line_text(line).as_str()) as usize;
            if cols > self.max_line_cols {
                self.max_line_cols = cols;
            }
        }
    }

    /// 当前最大显示列数（测试诊断用；水平滚动条以高水位 × 列宽定行程）。
    #[cfg(test)]
    pub fn max_line_display_cols(&self) -> usize {
        self.max_line_cols
    }

    /// 可见的行号闭区间 [first, last]（已夹紧到文档范围）。
    /// 第 73 轮 ⑯：软换行开态反解为「覆盖可见视觉行的逻辑行区间」。
    pub fn visible_range(&self) -> (usize, usize) {
        let count = self.doc.line_count();
        if self.wrap.borrow().enabled {
            if count == 0 {
                return (0, 0);
            }
            let total = self.visual_rows_total();
            let first_raw = (self.scroll_top.floor() as i64).max(0) as u32;
            let first = first_raw.min(total.saturating_sub(1));
            let rows = (self.viewport_h / self.line_height()).ceil() as u32 + 1;
            let last = first.saturating_add(rows).min(total.saturating_sub(1));
            return (self.locate_visual(first).0, self.locate_visual(last).0);
        }
        let first = (self.scroll_top.floor() as i64).max(0) as usize;
        let rows = (self.viewport_h / self.line_height()).ceil() as usize + 1;
        let last = (first + rows).min(count.saturating_sub(1));
        (first, last)
    }

    /// 第 `line` 行的可见长度（不含换行符）。
    pub fn line_display_len(&self, line: usize) -> usize {
        if line >= self.doc.line_count() {
            return 0;
        }
        self.doc
            .line_str(line)
            .trim_end_matches(['\n', '\r'])
            .chars()
            .count()
    }

    /// 软换行开关（设置项 word_wrap 下发）。
    pub fn wrap_enabled(&self) -> bool {
        self.wrap.borrow().enabled
    }

    /// 设置软换行开关（应用层 Settings 下发，fresh_tab 幂等）。
    /// 开启瞬间：清列块（设计 §4.10 互斥）、锁水平滚动为 0、整表重置；
    /// 关闭瞬间：清竖向目标列（goal 只在开态有意义）。
    pub fn set_word_wrap(&mut self, enabled: bool) {
        if enabled == self.wrap.borrow().enabled {
            return;
        }
        if enabled {
            self.clear_block();
            // B10：折行开态多光标互斥（设计 §4 #7）——附加光标随块一并折叠
            self.collapse_multi();
            self.scroll_left = 0.0;
            let (lines, cols) = (self.doc.line_count(), self.wrap_max_cols());
            let px = self.wrap_max_px();
            self.wrap.borrow_mut().enable(lines, cols, px);
        } else {
            self.wrap.borrow_mut().disable();
            self.goal_px = None;
        }
        self.clamp_scroll();
    }

    /// P99：设定折行预算是否预留垂直滚动条可视带宽。判定 = 垂直滚动条
    /// needed（内容超出视口），由控件层 draw 每帧按当前行程测量注入。
    ///
    /// 稳定性（无逐帧翻转）：折行段宽预算收缩只会**增加**（不会减少）
    /// 视觉行数——判定「需要滚动条」时预留，预留后行数更多、滚动条仍
    /// needed，状态自持；判定「放得下」时取消预留，取消后行数更少、
    /// 依然放得下，同样自持。两个方向各自自洽，仅内容/视口跨越边界
    /// 时切换一次。
    ///
    /// 预算变化经 [`Self::wrap_max_px`] 反映到 WrapCache 同步键 → 整表
    /// 重置 → 可见行经 draw 惰性收敛（v1 已披露的收敛模型），切换帧
    /// 内渲染以新预算排布，滚动范围下一帧对齐。
    pub fn set_wrap_sb_reserve(&mut self, reserve: bool) {
        self.wrap_sb_reserve = reserve;
    }

    /// 软换行可用显示列预算 = 折行预算 ÷ 列宽，≥1。
    /// P96 起仅作**列模型回退**用（真实字形布局未注入时，见
    /// [`Self::wrap_line_breaks`]）；有真实布局时按
    /// [`Self::wrap_max_px`] 像素断行。
    pub(crate) fn wrap_max_cols(&self) -> usize {
        let cw = self.char_width().max(0.1);
        let w = self.wrap_budget_px();
        (w / cw).floor().max(1.0) as usize
    }

    /// 软换行段宽像素预算 = 折行预算（来源见 [`Self::wrap_budget_px`]）。
    pub(crate) fn wrap_max_px(&self) -> f32 {
        self.wrap_budget_px()
    }

    /// 折行预算（像素，列模型与像素断行的单一来源）。
    ///
    /// P95：开态**不预留水平滚动条槽位**——hscroll 在软换行下恒隐藏
    /// （HScrollbar needed=false），预留的 SCROLLBAR_ZONE_W+2 会让折行
    /// 文本在窗口右缘留下一片可见空白（贴边留白观感）。
    ///
    /// P99：**垂直**滚动条需要出现（内容超出视口）时按
    /// [`VERTICAL_SCROLLBAR_RESERVE`] 让位——折行文本在滑块左侧收尾，
    /// 行尾字符不再被盖住/显得截断；内容放得下（无滚动条）时零预留
    /// 全宽贴边（P95 口径保留）。判定与稳定性见
    /// [`Self::set_wrap_sb_reserve`]。
    ///
    /// P115/P116：**行尾与文本区右缘恒留一个汉字宽**（= 默认字号全宽，
    /// [`RIGHT_EDGE_HAN_GAP`] 固定 16px）——英文半宽字符行尾不再贴右缘/
    /// 滚动条，组字挤出后文时右缘有余量缓冲；大字号下余量不随之膨胀
    /// （用户复报「放大后段变短、右侧空白大」）。折行断行（P96）与组字
    /// 重排（P115）共用本预算，口径单一。
    pub(crate) fn wrap_budget_px(&self) -> f32 {
        let full = (self.text_viewport_w() - RIGHT_EDGE_HAN_GAP).max(4.0);
        if self.wrap_sb_reserve {
            (full - VERTICAL_SCROLLBAR_RESERVE).max(4.0)
        } else {
            full
        }
    }

    /// 总视觉行数（开关关 = 逻辑行数；开 = Fenwick 段数前缀总和）。
    pub fn visual_rows_total(&self) -> u32 {
        if !self.wrap.borrow().enabled {
            return self.doc.line_count() as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(
            self.doc.line_count(),
            self.wrap_max_cols(),
            self.wrap_max_px(),
        );
        w.index.total()
    }

    /// P154：把整表软换行索引对账到当前文档（全量逐行 `segments_of_line`）。
    ///
    /// **为什么必须在本帧做**：行数变化（按回车/粘贴多行/删行）会让换行表
    /// 整表重置为「每行 1 段」，`visual_rows_total()` 因此在收敛前系统性
    /// 偏小；而 `ensure_visible` 的滚动夹紧与滚动条行程都在**绘制之前**取
    /// 该值算上限 → 视口被错误拉回上方，下一帧收敛后又跳回（用户复报：
    /// 末尾按回车「行号闪一下」，实测 scroll 24.9 → 2.9 → 25.9）。
    /// 由编辑汇点 [`Self::invalidate_highlight_from`] 在帧末调用；此时本帧
    /// 的夹紧已经发生（用的是旧上限，无害——夹紧是幂等钳制），对账后下一
    /// 帧一切读数都可信。
    ///
    /// 成本：逐行一次 `segments_of_line`。同代 memo 命中直接返回（不重算
    /// 断点、不 touch BIT），仅编辑点邻域重算；未命中的行才做真实字形断行。
    /// 只在「重置后尚未对账」时执行（`needs_reconcile`），稳态零开销。
    pub(crate) fn reconcile_wrap_index(&mut self) {
        if !self.wrap.borrow().enabled || !self.wrap.borrow().needs_reconcile() {
            return;
        }
        let lines = self.doc.line_count();
        for line in 0..lines {
            let text = self.line_text(line);
            self.segments_of_line(line, &text);
        }
        self.wrap.borrow_mut().mark_reconciled();
    }

    /// 可信的真实字形布局：与正文**逐字符对齐**（字符数 + 1）**且字号与
    /// 当前一致**才返回。所有 [`WrapCache::segments_of`] 调用点必须经此
    /// 取 xs——换文档/换行内容的帧里 row_layouts 尚存上一帧布局，残缺 xs
    /// 会让像素断行在截断前缀上漏判溢出（boot 后 CJK 长行整行不折、滚动
    /// 不自愈）；P116：缩放帧旧字号 xs 长度对齐但字宽过期，同样不得
    /// 用于断行（否者 break 按旧字宽计算 → 缩小留白/放大超右缘）。
    /// B10：cursors.rs 的 caret_rect_at 重入也消费——pub(crate)。
    pub(crate) fn trusted_xs(&self, line: usize, body: &str) -> Option<&[f32]> {
        let n = body.chars().count();
        let size_ok = (self.row_layouts_font_size - self.font_size).abs() < 0.01;
        self.row_layouts
            .get(&line)
            .map(|v| v.as_slice())
            .filter(|xs| xs.len() == n + 1 && size_ok)
    }

    /// (line, col) 的视觉行号（开关关 = line）。
    pub fn visual_row_of(&self, line: usize, col: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return line as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(
            self.doc.line_count(),
            self.wrap_max_cols(),
            self.wrap_max_px(),
        );
        let lines = self.doc.line_count();
        let line = line.min(lines);
        let prefix = w.index.prefix_rows(line);
        if line >= lines {
            return prefix;
        }
        let body = self.line_text(line);
        let real_xs = self.trusted_xs(line, &body);
        let breaks = w.segments_of(line, &body, real_xs);
        let seg = segment_index(&breaks, col, body.chars().count());
        prefix + seg as u32
    }

    /// 逻辑行 `line` 的视觉段数（测试诊断用）。
    #[cfg(test)]
    pub fn line_visual_segments(&self, line: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return 1;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(
            self.doc.line_count(),
            self.wrap_max_cols(),
            self.wrap_max_px(),
        );
        let body = self.line_text(line);
        let real_xs = self.trusted_xs(line, &body);
        w.segments_of(line, &body, real_xs).len() as u32
    }

    /// 逻辑行 `line` 的段首列向量（调用方常已持有正文，免二次取串；
    /// 内部按需重算并差值更新 BIT）。
    ///
    /// xs 可信度见 [`Self::trusted_xs`]（历史：本处曾内联同款过滤，
    /// locate_visual 等其余调用点漏防——boot 后 CJK 长行整行不折）。
    pub(crate) fn segments_of_line(&self, line: usize, body: &str) -> Rc<Vec<usize>> {
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(
            self.doc.line_count(),
            self.wrap_max_cols(),
            self.wrap_max_px(),
        );
        let real_xs = self.trusted_xs(line, body);
        w.segments_of(line, body, real_xs)
    }

    /// 视觉行反解：`(逻辑行, 段序, 段首列, 段末列)`。段末列 = 下一段
    /// 段首或行尾（含）。`visual` 越界钳到末行末段。
    /// 关态恒等退化：返回 `(visual, 0, 0, 行长)`。
    pub fn locate_visual(&self, visual: u32) -> (usize, usize, usize, usize) {
        let lines = self.doc.line_count();
        if !self.wrap.borrow().enabled {
            let line = (visual as usize).min(lines.saturating_sub(1));
            return (line, 0, 0, self.line_display_len(line));
        }
        if lines == 0 {
            return (0, 0, 0, 0);
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(lines, self.wrap_max_cols(), self.wrap_max_px());
        let total = w.index.total();
        let v = visual.min(total.saturating_sub(1));
        // 二分：最大 line 使 prefix_rows(line) ≤ v（prefix_rows = 该行
        // 首个视觉行号；行号 = 之前所有行的段数和——O(log n) 一次）
        let (mut lo, mut hi) = (0usize, lines);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if w.index.prefix_rows(mid) <= v {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let line = lo.saturating_sub(1);
        let body = self.line_text(line);
        let lens = body.chars().count();
        let real_xs = self.trusted_xs(line, &body);
        let breaks = w.segments_of(line, &body, real_xs);
        let off = (v.saturating_sub(w.index.prefix_rows(line))) as usize;
        let seg = off.min(breaks.len().saturating_sub(1));
        let seg_start = breaks[seg];
        let seg_end = breaks.get(seg + 1).copied().unwrap_or(lens);
        (line, seg, seg_start, seg_end)
    }

    /// 逻辑行 `line` 的首视觉行号（书签/行号/选区锚定用；O(log n)）。
    pub fn line_visual_base(&self, line: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return line as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(
            self.doc.line_count(),
            self.wrap_max_cols(),
            self.wrap_max_px(),
        );
        w.index.prefix_rows(line.min(self.doc.line_count()))
    }

    /// 滚动条/行程口径的「内容行数」（开态 = 视觉行数；关态 = 逻辑行数）。
    pub(crate) fn scroll_content_lines(&self) -> usize {
        if self.wrap.borrow().enabled {
            self.visual_rows_total() as usize
        } else {
            self.doc.line_count()
        }
    }

    /// 水平行程内容像素宽：软换行开态恒 0（hscroll 隐藏、needed=false）。
    pub(crate) fn hscroll_content_px(&self) -> f32 {
        if self.wrap.borrow().enabled {
            0.0
        } else {
            self.content_width_px()
        }
    }

    /// 清除竖向移动目标列（P37 打断口：任何非竖向操作调用）。
    pub(crate) fn clear_vertical_goal(&mut self) {
        self.goal_px = None;
    }

    /// 竖向移动的目标解析：目标视觉行内按 goal 像素列反解字符列。
    /// 返回 (line, col, visual)。视觉行越界（首行上/末行下）返回 None。
    pub(crate) fn vertical_target(
        &mut self,
        motion: Motion,
        page_rows: usize,
    ) -> Option<(usize, usize)> {
        let total = self.visual_rows_total();
        let cur_v = self.visual_row_of(self.cursor.line, self.cursor.col);
        let step: i64 = match motion {
            Motion::Up => -1,
            Motion::Down => 1,
            Motion::PageUp => -(page_rows.max(1) as i64),
            Motion::PageDown => page_rows.max(1) as i64,
            _ => return None,
        };
        let target_v = (cur_v as i64 + step).clamp(0, total as i64 - 1) as u32;
        if target_v == cur_v {
            return None;
        }
        // goal-column：竖向序列首步记录当前光标 x，序列内沿用
        if self.goal_px.is_none() {
            let text = self.line_text(self.cursor.line);
            let col = self.cursor.col.min(text.chars().count());
            self.goal_px = Some(self.px_of(self.cursor.line, &text, col));
        }
        let goal = self.goal_px.unwrap_or(0.0);
        let (line, _seg, s0, s1) = self.locate_visual(target_v);
        let text = self.line_text(line);
        let lens = text.chars().count();
        let (s0, s1) = (s0.min(lens), s1.min(lens));
        // 段相对 goal：续行从文本区左缘起排——goaI 换算为「段起点之后
        // 的像素偏移」再按字符中点反解（与 hit_test 同口径）
        let seg_base_px = self.px_of(line, &text, s0);
        let goal_seg = goal - seg_base_px;
        let xs_fresh: Option<&Vec<f32>> = self
            .row_layouts
            .get(&line)
            .filter(|xs| xs.len().saturating_sub(1) >= lens);
        let mut col = if goal_seg <= 0.0 {
            s0 // goal 在段起点左侧 → 段首
        } else if let Some(xs) = xs_fresh {
            let mut c = s1; // 未命中（goal 越过段尾）→ 段尾
            for k in s0..s1 {
                let mid = (xs[k] + xs[k + 1]) * 0.5 - seg_base_px;
                if mid > goal_seg {
                    c = k;
                    break;
                }
            }
            c
        } else {
            // 列模型回退（无布局注入/布局滞后；绝对列累计——Tab 宽度
            // 依赖绝对列——再换算段相对）
            let mut acc = 0.0f32; // 绝对列
            let mut base = 0.0f32; // s0 之前的列数
            let mut c = s1;
            for (k, ch) in text.chars().enumerate() {
                if k >= s1 {
                    break;
                }
                let w = char_cols(ch, acc as usize);
                if k < s0 {
                    base += w;
                } else {
                    let mid_rel = (acc + w * 0.5) * self.char_width() - base * self.char_width();
                    if mid_rel > goal_seg {
                        c = k;
                        break;
                    }
                }
                acc += w;
            }
            c
        };
        // 越段钳制：goaI 超出段尾 → 停在段末字符格（不跳到下一视觉段，
        // 光标视觉行保持目标行；末段（s1 == lens）允许到行尾）
        if col >= s1 && s1 < lens {
            col = s1.saturating_sub(1);
        }
        Some((line, col))
    }

    /// 命中测试：控件内坐标 -> 光标位置（CJK 双宽感知）。
    /// P13：x 需先减去水平滚动偏移——点击坐标在视口系，字符列在文档系。
    pub fn hit_test(&self, x: f32, y: f32) -> CursorPos {
        let gutter = self.gutter_width();
        let char_w = self.char_width();
        // 第 73 轮 ⑯：折行开态——y 先反解视觉行，再映射 (逻辑行, 段)，
        // x 命中落在该段字符区间内（段起点与绘制同源 px_of）
        if self.wrap.borrow().enabled {
            let total = self.visual_rows_total();
            if total == 0 {
                return CursorPos::default();
            }
            let v_f = self.scroll_top + (y.max(0.0) / self.line_height());
            let v = ((v_f.floor() as i64).clamp(0, total as i64 - 1)) as u32;
            let (line, _seg, s0, s1) = self.locate_visual(v);
            let text = self.line_text(line);
            let lens = text.chars().count();
            let (s0, s1) = (s0.min(lens), s1.min(lens));
            // 段相对 x：续行从文本区**左缘**起排（P94 左缘模型）——点击
            // 偏移（x−gutter）即段内偏移，**不得**再减 px_of(seg_start)：
            // 那会把注入布局的真实 x（如 xs[34]=952px）当段起点，所有
            // 续行点击全部落回段首（P96 像素测试当场暴露的潜伏 bug，
            // P94 测试全走列模型回退路径所以未被抓到）
            let rel = (x - gutter + self.scroll_left).max(0.0);
            let col = self.hit_col_in_range(line, &text, s0, s1, rel);
            return CursorPos { line, col };
        }
        let line_f = self.scroll_top + (y.max(0.0) / self.line_height());
        let line = ((line_f.floor() as i64).clamp(0, self.doc.line_count() as i64 - 1)) as usize;
        let text = self.line_text(line);
        let rel = (x - gutter + self.scroll_left).max(0.0);

        // P45：真实字形布局命中（与绘制同源，字符边界 = 真实 x 中点，
        // 与列模型的「左半/右半」选位语义一致）；布局滞后（注入的是
        // 上一帧旧文本，长度覆盖不到当前文本）整体回退列模型实时计算
        if let Some(xs) = self.row_layouts.get(&line) {
            if xs.len().saturating_sub(1) >= text.chars().count() {
                let mut col = xs.len() - 1;
                for (k, pair) in xs.windows(2).enumerate() {
                    let mid = (pair[0] + pair[1]) * 0.5;
                    if rel < mid {
                        col = k;
                        break;
                    }
                    col = k + 1;
                }
                return CursorPos { line, col };
            }
        }

        let mut col = text.chars().count();
        let mut acc = 0.0f32; // 累计显示列（含 Tab 制表位推进）
        for (i, ch) in text.chars().enumerate() {
            let w = char_cols(ch, acc as usize);
            // 落在字符左半边选前位，右半边选后位
            if rel < (acc + w * 0.5) * char_w {
                col = i;
                break;
            }
            acc += w;
        }
        CursorPos { line, col }
    }

    /// 折行开态：在段字符区间 `[s0, s1)` 内按**段相对** x（已扣 gutter/
    /// 水平偏移与段起点像素）反解字符列。真实布局新鲜走字形中点；
    /// 滞后回退列模型（段内累计从零起步）。
    pub(crate) fn hit_col_in_range(
        &self,
        line: usize,
        text: &str,
        s0: usize,
        s1: usize,
        rel: f32,
    ) -> usize {
        let char_w = self.char_width();
        let lens = text.chars().count();
        if s1 <= s0 {
            return s0;
        }
        if let Some(xs) = self.row_layouts.get(&line) {
            if xs.len().saturating_sub(1) >= lens {
                let base = xs[s0];
                for k in s0..s1 {
                    let mid = (xs[k] + xs[k + 1]) * 0.5 - base;
                    if rel < mid {
                        return k;
                    }
                }
                return s1;
            }
        }
        let mut acc = 0.0f32; // 绝对列累计（Tab 宽度依赖绝对列，不能段内重算）
        let mut base = 0.0f32; // s0 之前的列数（段起点）
        for (i, ch) in text.chars().enumerate() {
            if i >= s1 {
                break;
            }
            let w = char_cols(ch, acc as usize);
            if i < s0 {
                base += w;
            } else {
                let mid_rel = (acc + w * 0.5) * char_w - base * char_w;
                if rel < mid_rel {
                    return i;
                }
            }
            acc += w;
        }
        s1
    }

    /// 第 `line` 行的不含换行文本。
    pub(crate) fn line_text(&self, line: usize) -> String {
        if line >= self.doc.line_count() {
            return String::new();
        }
        self.doc
            .line_str(line)
            .trim_end_matches(['\n', '\r'])
            .to_owned()
    }

    /// 行号栏宽度。
    ///
    /// P59：系数 0.7→1.0——行号以正文字号×1.0 渲染，数字宽 = char_w，
    /// 旧系数让右对齐的行号盒比数字窄 ~30%，右对齐向左溢出画出控件左缘
    /// （iced 文本裁剪对 Cached 文本不生效，无法兜底）。盒宽 ≥ 数字宽后
    /// 溢出消失；GUTTER_MIN 继续充当行号与正文的间距。
    /// 第 60 轮：左侧加 BOOKMARK_STRIP 书签条带（圆点槽位），行号右缘
    /// 相对条带右缘的位置不变——所有消费方都经本函数取宽，无硬编码。
    pub fn gutter_width(&self) -> f32 {
        let digits = self.doc.line_count().to_string().len().max(3);
        BOOKMARK_STRIP + GUTTER_MIN + digits as f32 * self.char_width()
    }

    /// 心跳：翻转闪烁相位（由应用层节拍驱动）。P53：按真实流逝间隔门控
    /// ——淡出动画期间心跳临时加密到 33ms 快拍，相位翻转仍保持
    /// ~CARET_BLINK_MS 节奏，光标闪烁不受动画驱动影响。
    pub fn tick_blink(&mut self) {
        self.tick_blink_at(std::time::Instant::now());
    }

    /// [`Self::tick_blink`] 的可注入时钟版（单测用）。
    pub(crate) fn tick_blink_at(&mut self, now: std::time::Instant) {
        let blink_due = self
            .last_blink_at
            .is_none_or(|t| now.duration_since(t).as_millis() >= CARET_BLINK_MS as u128);
        if blink_due {
            if self
                .last_activity
                .is_none_or(|t| now.duration_since(t).as_millis() >= CARET_ACTIVE_MS)
            {
                self.blink_on = !self.blink_on;
            }
            self.last_blink_at = Some(now);
        }
    }

    /// 光标活动（移动/输入/点击）：立即常显并重置活动时刻。
    pub fn poke_caret(&mut self) {
        self.blink_on = true;
        self.last_activity = Some(std::time::Instant::now());
    }

    /// 此刻是否应绘制光标：活动窗口期内常显，静止期按相位隐现。
    /// P167：组字进行中恒隐藏——组字串自带插入点指示（下划线 + 组字
    /// 光标），应用光标叠画在组字尾且随闪烁相位忽隐忽现，主流编辑器
    /// （记事本/VSCode/Word）组字期均不显示文本光标（用户对照截图
    /// 复报「光标不对」）。IME 定位（request_input_method 的 caret
    /// rect）不经本闸，候选窗跟随不受影响。
    pub fn caret_visible(&self) -> bool {
        if self.preedit.as_deref().is_some_and(|p| !p.is_empty()) {
            return false;
        }
        if let Some(t) = self.last_activity {
            if t.elapsed().as_millis() < CARET_ACTIVE_MS {
                return true;
            }
        }
        self.blink_on
    }

    /// 滚动/滚动条交互活动：重置竖直滚动条显示计时（淡出中的条立即回全显）。
    pub fn touch_scrollbar_activity(&mut self) {
        self.sb_activity = Some(std::time::Instant::now());
    }

    /// 当前竖直滚动条绘制 alpha（0 = 隐藏，1 = 全显）。
    pub fn scrollbar_visibility(&self) -> f32 {
        self.scrollbar_visibility_at(std::time::Instant::now())
    }

    /// [`Self::scrollbar_visibility`] 的可注入时钟版（单测用）。
    pub(crate) fn scrollbar_visibility_at(&self, now: std::time::Instant) -> f32 {
        match self.sb_activity {
            None => 0.0,
            Some(t) => scrollbar_alpha(now.duration_since(t).as_millis() as u32),
        }
    }

    /// 淡出动画是否进行中（应用层 CaretTick 常驻链据此切换 33ms 快拍）。
    pub fn scrollbar_fading(&self) -> bool {
        self.scrollbar_fading_at(std::time::Instant::now())
    }

    pub(crate) fn scrollbar_fading_at(&self, now: std::time::Instant) -> bool {
        match self.sb_activity {
            None => false,
            Some(t) => {
                let e = now.duration_since(t).as_millis() as u32;
                e > SCROLLBAR_IDLE_MS && e < SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS
            }
        }
    }

    /// P115：组字串在光标处的**可显示宽**（px）——软换行开态钳到
    /// 「折行预算 − 段内起点」（段末字符右缘 ≤ 预算不贴满，P96；空行/
    /// 段尾按整段预算计——修前用段末字符右缘作段尾，空行段 [0,0)
    /// 剩余被算成 0 → 空行/行尾组字整条消失）；关态 = 原宽。
    /// `w` = 组字串实测宽（调用方量度）。供组字占位（后文右移/C 段
    /// 起点）与光标/下划线偏移共用同一口径。
    pub(crate) fn preedit_visual_w(&self, col: usize, w: f32) -> f32 {
        if !self.wrap.borrow().enabled {
            return w;
        }
        let line = self.cursor.line;
        let text = self.line_text(line);
        let lens = text.chars().count();
        let col = col.min(lens);
        let breaks = self.segments_of_line(line, &text);
        let seg = segment_index(&breaks, col, lens);
        let seg_start = breaks[seg];
        let rel = self.px_of(line, &text, col) - self.px_of(line, &text, seg_start);
        let remain = (self.wrap_max_px() - rel).max(0.0);
        w.min(remain)
    }

    /// 相对控件的光标矩形（供输入法定位候选框，双宽感知）。
    /// P13：x 含水平滚动偏移的抵扣——返回值是视口系坐标。
    /// 第 73 轮 ⑯：软换行开态 y 走视觉行映射（光标所在段），x 走**段
    /// 相对**定位（续行从文本区左缘起排，主流折行口径）。
    /// B10：多光标绘制经 [`Self::caret_rect_at`] 提参重入，本函数保持
    /// 主光标语义不变。
    pub fn caret_rect_relative(&self) -> Rectangle {
        self.caret_rect_at(self.cursor)
    }

    pub fn set_viewport_height(&mut self, h: f32) {
        self.viewport_h = h.max(self.line_height());
        self.clamp_scroll();
        // P53：几何变化（窗口缩放/字号调整）后滚动条重新点亮
        self.touch_scrollbar_activity();
    }

    /// 同步视口宽度（P13；RedrawRequested 时与高度一起更新）。
    pub fn set_viewport_width(&mut self, w: f32) {
        self.viewport_w = w.max(0.0);
        self.clamp_scroll_horizontal();
    }

    /// 视口高度（像素）。应用层暂未消费，供测试与后续里程碑（如状态栏显示）使用。
    #[allow(dead_code)]
    pub fn viewport_height(&self) -> f32 {
        self.viewport_h
    }

    /// 控件层专用的可见性保证入口。
    pub(crate) fn ensure_visible_pub(&mut self) {
        self.ensure_visible();
    }

    /// 鼠标按下决定焦点归属：落点在编辑区内 → 接管焦点；
    /// 落点在区外（查找框、工具栏……） → 交出焦点并清掉残留组字串。
    pub fn pointer_focus(&mut self, inside: bool) {
        self.focused = inside;
        if !inside {
            // 焦点离开时组字中断，内联预编辑串不能悬在正文里
            self.preedit = None;
            // P37：失焦打断打字组（去查找框搜一圈再回来，输入不该并组）
            self.break_typing();
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 归一化选区（起点<=终点）；无锚点返回 None。
    pub fn ordered_selection(&self) -> Option<(CursorPos, CursorPos)> {
        let anchor = self.anchor?;
        if (anchor.line, anchor.col) <= (self.cursor.line, self.cursor.col) {
            Some((anchor, self.cursor))
        } else {
            Some((self.cursor, anchor))
        }
    }
}
