use super::*;

impl EditorCore {
    /// 光标/选区触及的行范围（含首尾）。选区末点落在某行行首（col 0）时
    /// 该行不算触及——视觉上选区没有盖到它的任何字符。
    pub(crate) fn touched_lines(&self) -> (usize, usize) {
        match self.ordered_selection() {
            Some((start, end)) => {
                let last =
                    if end.col == 0 && end.line > start.line { end.line - 1 } else { end.line };
                (start.line, last)
            }
            None => (self.cursor.line, self.cursor.line),
        }
    }

    /// 删除光标/选区触及的所有整行（含各自行尾）。返回是否发生删除。
    ///
    /// 边界口径：
    /// - 整个文档只剩一个空行时无事可做；
    /// - 光标停在文末空行（尾随换行产生的幻影行）时退化为吃掉其前面的
    ///   换行单元（`\r\n` 整体），与主流编辑器的 Ctrl+L 手感一致；
    /// - 删除文档末尾若干行后若还剩前文，末尾换行随块一并回收。
    pub fn delete_current_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let start = self.doc.line_to_char(a);
        let end =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let (start, end, removed_rows) = if start == end {
            if a == 0 {
                return false; // 唯一内容为空：无可删
            }
            // 幻影末行：改为移除前一行行尾的换行单元（\r\n 整体或 \n）。
            // start ≥ 1 恒成立（前面至少有一个换行才轮得到空行）；start == 1
            // 时前面只有这 1 字节、必非 CRLF——须先判 start ≥ 2 再回看 2 字节，
            // 否则文档恰为一个换行符且光标在幻影行时 usize 下溢崩溃。
            let crlf = start >= 2 && self.doc.slice_text(start - 2, start) == "\r\n";
            let s = start - usize::from(crlf) - 1;
            (s, start, 1)
        } else {
            (start, end, b - a + 1)
        };
        self.snapshot();
        self.doc.remove_range(start, end);
        // 第 60 轮：书签随行消失/上移（幻影分支恒移除 1 行——空壳随其
        // 换行单元一起消失；常规分支移除触及块 b-a+1 行）
        self.remap_removed_rows(a, removed_rows);
        let at = start.min(self.doc.text_len());
        let line = self.doc.char_to_line(at);
        let col = at - self.doc.line_to_char(line);
        self.cursor = CursorPos { line, col };
        self.anchor = None;
        self.invalidate_highlight_from(start);
        // 行结构整体变化，列高水位可能过估——交惰性收敛（P45/P13 同口径）
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    /// 在触及块的正下方复制一份相同内容，光标落到副本首行。返回是否复制。
    pub fn duplicate_current_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let start = self.doc.line_to_char(a);
        let end =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let text = self.doc.slice_text(start, end);
        if text.is_empty() {
            return false; // 空文档 / 幻影末行无内容可复制
        }
        self.snapshot();
        let nl = self.doc.line_ending().newline();
        // 尾部块本身不带行尾时先补一个主导行尾再插到文档末，
        // 保证副本独立成行而不是拼在原块后面。
        // P80 勘误（第 60 轮书签测试揪出的既有缺陷）：判定依据应为
        // 「块文本是否已带行尾单元」，而非 end < len——文档以换行收尾时
        // 末真实行的块文本自带行尾但 end == len，旧条件误补行尾，
        // 副本前凭空多出一个空行。
        let ins = if text.ends_with('\n') || text.ends_with('\r') {
            text
        } else {
            format!("{nl}{text}")
        };
        self.doc.insert(end, &ins);
        self.invalidate_highlight_from(end);
        self.max_cols_stale = true;
        // 第 60 轮：块副本插入 b-a+1 行，原块之后的书签整体下推；
        // 副本本身不带书签（主流编辑器同口径——书签标的是旧行）
        self.remap_shift_below(b, (b - a + 1) as isize);
        self.anchor = None;
        // 插入点恒为第 b+1 行行首（块内已有行尾 / 已补行尾）
        self.cursor = CursorPos { line: b + 1, col: 0 };
        self.ensure_visible();
        true
    }

    /// 上移/下移光标触及的行块（与相邻行整体换位）。已在边界时不动并返回
    /// false（不产生快照）。多行选区整块旋转、内部相对次序保持。
    pub fn move_current_lines(&mut self, up: bool) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let (first, last) = if up {
            if a == 0 {
                return false;
            }
            (a - 1, b)
        } else {
            if b + 1 >= count {
                return false;
            }
            (a, b + 1)
        };
        // 光标列尽量保持（钳到原行长度；换位后行长可能不同，仅取近似）
        let keep_col = self.cursor.col.min(self.doc.line_len_chars(self.cursor.line));
        let rs = self.doc.line_to_char(first);
        let re =
            if last + 1 < count { self.doc.line_to_char(last + 1) } else { self.doc.text_len() };
        // 区域内各行内容按方向旋转一行后以主导行尾重建；原区域以行尾
        // 结尾则重建串同样收尾（文档末行无行尾的形态保持）。
        // 混合行尾经此归一到主导行尾——与 P9「编辑不产生混合行尾」同哲学。
        // 第 59 轮起经共用助手剥行尾（口径：`\r\n`/`\n`/孤立 `\r` 皆换行）。
        let mut lines: Vec<String> =
            (first..=last).map(|i| self.line_body_without_eol(i)).collect();
        if up {
            lines.rotate_left(1);
        } else {
            lines.rotate_right(1);
        }
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if re < self.doc.text_len() {
            rebuilt.push_str(nl);
        }
        self.snapshot();
        self.doc.remove_range(rs, re);
        self.doc.insert(rs, &rebuilt);
        self.invalidate_highlight_from(rs);
        self.max_cols_stale = true;
        // 第 60 轮：块内轮转的书签跟随——上移时相邻上行换入块尾、块内
        // 各上移一位；下移对称（相邻下行换入块头、块内各下移一位）
        let (enter_from, enter_to, inner) =
            if up { (a - 1, b, -1i64) } else { (b + 1, a, 1i64) };
        self.bookmarks = self
            .bookmarks
            .iter()
            .map(|&l| {
                if l == enter_from {
                    enter_to
                } else if l >= a && l <= b {
                    (l as i64 + inner) as usize
                } else {
                    l
                }
            })
            .collect();
        self.anchor = None;
        self.cursor = CursorPos { line: if up { a - 1 } else { b + 1 }, col: keep_col };
        self.ensure_visible();
        true
    }

    /// 大小写转换：有选区只转选区字符，无选区转整个文档。返回是否改动。
    ///
    /// - Unicode 全量映射（`str::to_uppercase/to_lowercase`）：`ß→SS`、
    ///   `ﬁ→FI` 一类映射会改变字符数——区域整体重写，光标/选区按新偏移
    ///   重建（有选区时转换结果保持选中，便于连续操作）；
    /// - 换行符不在大小写映射表里：行数与主导行尾元数据恒不变；
    /// - 幂等保护：内容无可转换字符（纯数字/CJK/空白）时不动、不产快照。
    pub fn convert_case(&mut self, kind: CaseKind) -> bool {
        let (start, end) = match self.selection_offsets() {
            Some((s, e)) if s < e => (s, e),
            _ => (0, self.doc.text_len()),
        };
        if start >= end {
            return false;
        }
        let src = self.doc.slice_text(start, end);
        let out = match kind {
            CaseKind::Upper => src.to_uppercase(),
            CaseKind::Lower => src.to_lowercase(),
        };
        if out == src {
            return false; // 无可转换内容：no-op 不产生撤销快照
        }
        let had_selection = self.anchor.is_some();
        // 行号在换行结构不变的前提下保持；列钳到行长防越界
        let keep = self.cursor;
        self.snapshot();
        self.doc.remove_range(start, end);
        self.doc.insert(start, &out);
        self.invalidate_highlight_from(start);
        // ß→SS 一类长度变化让列高水位可能过估——交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        if had_selection {
            // 新文本整体重新选中：起止偏移换算回行列（end 可等于 text_len）
            let end_off = start + out.chars().count();
            let sl = self.doc.char_to_line(start);
            let sc = start - self.doc.line_to_char(sl);
            let el = self.doc.char_to_line(end_off.min(self.doc.text_len()));
            let ec = end_off.min(self.doc.text_len()) - self.doc.line_to_char(el);
            self.anchor = Some(CursorPos { line: sl, col: sc });
            self.cursor = CursorPos { line: el, col: ec };
        } else {
            self.anchor = None;
            self.cursor = CursorPos {
                col: keep.col.min(self.line_display_len(keep.line)),
                ..keep
            };
        }
        self.ensure_visible();
        true
    }

    /// 去除行首/行尾空白：有选区只清触及行，无选区清全文档。返回是否改动。
    ///
    /// - 空白口径 = `char::is_whitespace`（Unicode White_Space：半角/全角
    ///   空格、Tab、NBSP 等都算），行尾判定先剥换行单元再 trim；
    /// - 与 P73 行操作同哲学：触及块按主导行尾重建，不产生混合行尾；
    ///   文档末行无行尾的形态保持；
    /// - 无任何行需要清理时不动、不产快照（幂等 no-op）。
    pub fn trim_touched_lines(&mut self, mode: TrimMode) -> bool {
        let count = self.doc.line_count();
        let (a, b) = if self.anchor.is_some() {
            self.touched_lines()
        } else {
            (0, count.saturating_sub(1))
        };
        let rs = self.doc.line_to_char(a);
        let re =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let mut changed = false;
        let mut lines: Vec<String> = Vec::with_capacity(b - a + 1);
        for i in a..=b {
            // 第 59 轮起经共用助手剥行尾（ropey 口径：`\r\n`/`\n`/孤立
            // `\r` 皆换行单元，与 P73 原实现一致）
            let s = self.line_body_without_eol(i);
            let t = match mode {
                TrimMode::Leading => s.trim_start(),
                TrimMode::Trailing => s.trim_end(),
                TrimMode::Both => s.trim(),
            };
            // trim 只删前后缀，字节数相同即内容未变
            changed |= t.len() != s.len();
            lines.push(t.to_owned());
        }
        if !changed {
            return false;
        }
        // 光标行钳进触及范围、列钳到清理后的行长（行可能变短）
        let keep_line = self.cursor.line.clamp(a, b);
        let keep_col = self.cursor.col;
        self.snapshot();
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if re < self.doc.text_len() {
            rebuilt.push_str(nl); // 区域不是文档末尾：补回块尾换行
        }
        self.doc.remove_range(rs, re);
        self.doc.insert(rs, &rebuilt);
        self.invalidate_highlight_from(rs);
        self.max_cols_stale = true;
        self.anchor = None;
        self.cursor =
            CursorPos { line: keep_line, col: keep_col.min(self.line_display_len(keep_line)) };
        self.ensure_visible();
        true
    }

    /// 取第 `i` 行正文：剥掉行尾换行单元。ropey 换行口径 = `\r\n`、`\n`、
    /// **孤立 `\r`** 三者皆是换行（第 59 轮实测钉死），故末尾的 `\r` 同样
    /// 属换行单元而非内容，必须剥除。
    pub(crate) fn line_body_without_eol(&self, i: usize) -> String {
        let mut s = self.doc.line_str(i);
        if s.ends_with("\r\n") {
            s.truncate(s.len() - 2);
        } else if s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    pub(crate) fn collect_line_block(&self) -> LineBlock {
        let count = self.doc.line_count();
        let len = self.doc.text_len();
        let (a, b) =
            if self.anchor.is_some() { self.touched_lines() } else { (0, count.saturating_sub(1)) };
        let start = self.doc.line_to_char(a);
        let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { len };
        // 幻影末行检测：块到达文档末尾且原文以换行单元收尾（`\n` 或孤立
        // `\r`，ropey 口径两者皆换行）→ 最后一行是空壳。仅在块内还有
        // 更前面的行时才排除（a == b 时无从回退，交由调用方的「不足两行」
        // no-op 兜底，避免下溢）。
        // 第 64 轮勘误（🟠 既有缺陷）：补 b == count-1 守卫——幻影只可能
        // 是文档最后一行；此前凡块区域顶到文档尾就把 b 行当幻影剔除，
        // 「选中含末真实行、未含末尾换行后的空壳」的局部选区会把该行
        // 从重建内容里丢掉（排序/去重/注释切换共用此骨架，均受影响）。
        let phantom_tail = end == len
            && len > start
            && b == count - 1
            && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        // 块尾换行补回判定与幻影排除解耦（第 64 轮勘误第二半）：只要块
        // 区域以换行单元收尾（含「选中真实末行、其行尾即文档末尾换行」），
        // 行数不变式重建就必须补回，否则丢尾随换行。
        let nl_tail = end == len
            && len > start
            && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        let last = if phantom_tail && b > a { b - 1 } else { b };
        let lines: Vec<String> = (a..=last).map(|i| self.line_body_without_eol(i)).collect();
        LineBlock {
            a,
            b,
            start,
            end,
            push_nl: end < len || nl_tail,
            lines,
        }
    }

    /// 用新内容替换收集时的块区域（排序/去重共用的写回路径）：按主导行尾
    /// 重建不产生混合行尾；块尾/文档末尾换行形态保持；光标行列钳回块内，
    /// 选区清空。调用前必须已确认内容确有变化（no-op 不产快照）。
    ///
    /// `map[rel]` = 块内原第 rel 行的新块内位置（None = 内容消失）——
    /// 先快照（携带重排前的书签集）再按映射搬迁书签，最后重写文档字节。
    pub(crate) fn apply_line_block(&mut self, blk: &LineBlock, lines: &[String], map: &[Option<usize>]) {
        let keep_line = self.cursor.line.clamp(blk.a, blk.b);
        let keep_col = self.cursor.col;
        self.snapshot();
        // 第 60 轮：书签跟随行内容搬到新位置
        self.remap_block_mapping(blk.a, map);
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if blk.push_nl {
            rebuilt.push_str(nl);
        }
        self.doc.remove_range(blk.start, blk.end);
        self.doc.insert(blk.start, &rebuilt);
        self.invalidate_highlight_from(blk.start);
        // 行结构整体变化 + 行长可能增减——列高水位交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        self.anchor = None;
        // 第 66 轮勘误：keep_line 还要收敛到**新行数域**——块被压缩
        // （删空行/合并）后旧光标行可能超出新行数（随机对拍当场抓住：
        // 全删空文档只剩 1 行而 keep_line 仍是旧的大行号）
        let keep_line = keep_line.min(self.doc.line_count() - 1);
        self.cursor =
            CursorPos { line: keep_line, col: keep_col.min(self.line_display_len(keep_line)) };
        self.ensure_visible();
    }

    /// 行排序：有选区只排触及块，无选区排全文档。返回是否改动。
    ///
    /// - 比较口径 = UTF-8 字节序（码点序），大小写敏感、稳定排序；
    /// - 空白行是真实内容，正常参与排序（只有幻影末行除外，见
    ///   [`Self::collect_line_block`]）；
    /// - 已有序 / 块内不足两行时不动、不产快照（幂等 no-op）；
    /// - 书签跟随行内容走：按稳定排序的排列反演映射搬迁（第 60 轮）。
    pub fn sort_lines(&mut self, order: SortOrder) -> bool {
        let blk = self.collect_line_block();
        if blk.lines.len() <= 1 {
            return false; // 0/1 行无从排序
        }
        let n = blk.lines.len();
        // perm[new_rel] = old_rel（稳定排序）；反演成旧→新映射供书签搬迁
        let mut perm: Vec<usize> = (0..n).collect();
        match order {
            SortOrder::Ascending => perm.sort_by(|&i, &j| blk.lines[i].cmp(&blk.lines[j])),
            SortOrder::Descending => perm.sort_by(|&i, &j| blk.lines[j].cmp(&blk.lines[i])),
        }
        let mut map = vec![None; n];
        for (new_rel, &old_rel) in perm.iter().enumerate() {
            map[old_rel] = Some(new_rel);
        }
        let sorted: Vec<String> = perm.iter().map(|&i| blk.lines[i].clone()).collect();
        if sorted == blk.lines {
            return false; // 已有序：幂等 no-op 不产生撤销快照
        }
        self.apply_line_block(&blk, &sorted, &map);
        true
    }

    /// 去除重复行：保留首次出现、其余行相对次序不变。返回是否改动。
    ///
    /// - 有选区只清触及块，无选区清全文档；逐字节整行比对（含空白差异）；
    /// - 无重复 / 块内不足两行时不动、不产快照（幂等 no-op）；
    /// - 书签跟随：保留行的书签搬到新位置，重复项的书签随行丢弃。
    pub fn remove_duplicate_lines(&mut self) -> bool {
        let blk = self.collect_line_block();
        if blk.lines.len() <= 1 {
            return false;
        }
        let n = blk.lines.len();
        let mut seen: HashSet<&str> = HashSet::with_capacity(n);
        // 旧块内相对位置 → 新块内位置；None = 该行是重复项，内容消失
        let mut map: Vec<Option<usize>> = vec![None; n];
        let mut kept: Vec<String> = Vec::with_capacity(n);
        for (rel, line) in blk.lines.iter().enumerate() {
            if seen.insert(line.as_str()) {
                map[rel] = Some(kept.len());
                kept.push(line.clone());
            }
        }
        if kept.len() == n {
            return false; // 本就无重复：幂等 no-op 不产生撤销快照
        }
        self.apply_line_block(&blk, &kept, &map);
        true
    }

    /// Tab↔空格互转：有选区只转触及块，无选区转全文档。返回是否改动。
    ///
    /// - 制表位宽度 = 渲染层 `TAB_STOP_COLS`，转换与绘制对齐严格一致
    ///   （宽字符占 2 显示列也计入制表位推进，`中\tX` 的 Tab 落到列 4）；
    /// - 无变化时不动、不产快照（幂等 no-op）；行数不变 → 恒等映射，
    ///   书签原位保留。
    pub fn convert_tabs_spaces(&mut self, kind: TabSpaceKind) -> bool {
        let blk = self.collect_line_block();
        let mut changed = false;
        let lines: Vec<String> = blk
            .lines
            .iter()
            .map(|s| {
                let t = match kind {
                    TabSpaceKind::LeadingTabsToSpaces => expand_tabs_in(s, true),
                    TabSpaceKind::AllTabsToSpaces => expand_tabs_in(s, false),
                    TabSpaceKind::LeadingSpacesToTabs => entab_leading_ws(s),
                };
                // 长度可能碰巧相等（列 3 处的 1 个 Tab ↔ 1 空格），必须按值比对
                changed |= t != *s;
                t
            })
            .collect();
        if !changed {
            return false;
        }
        let map: Vec<Option<usize>> = (0..lines.len()).map(Some).collect();
        self.apply_line_block(&blk, &lines, &map);
        true
    }

    /// 合并行：触及块合成一行；无选区 = 当前行并入下一行。返回是否改动。
    ///
    /// - 连接口径：各行 `trim()` 后以**单个空格**连接，空白行直接消失；
    ///   全空块合并为一个空行（行数缩减本身就是改动）；
    /// - 书签：首行内容前缀幸存 → 块内书签全部收敛到合并后的首行；
    ///   其余行的书签随行并入丢弃（撤销可整体找回）；
    /// - 无选区且已在末行 / 块内不足两行 → no-op 不产快照。
    pub fn merge_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = if self.anchor.is_some() {
            self.touched_lines()
        } else {
            let a = self.cursor.line.min(count.saturating_sub(1));
            if a + 1 >= count {
                return false; // 末行没有「下一行」可并
            }
            (a, a + 1)
        };
        let start = self.doc.line_to_char(a);
        let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        // 区域尾字节是否换行单元（\n / 孤立 \r）：决定重建时是否补回
        // 块尾换行——选中真实末行（其行尾即文档末尾换行）时同样成立，
        // 不能只看「块不在文档末尾」
        let len = self.doc.text_len();
        let nl_tail =
            end == len && end > start
                && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        // 幻影末行排除：仅当 b 本身就是文档最后一行（ropey 在尾随换行后
        // 多出的空壳行）且区域确以换行收尾。b 是唯一真实行的场景无从回
        // 退，直接按无可并内容处理。
        let last = if nl_tail && b == count - 1 {
            if b > a {
                b - 1
            } else {
                return false;
            }
        } else {
            b
        };
        if last <= a {
            return false; // 触及块实际不足两行
        }
        let bodies: Vec<String> =
            (a..=last).map(|i| self.line_body_without_eol(i)).collect();
        let mut joined = String::new();
        for part in &bodies {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(t);
        }
        let blk = LineBlock { a, b, start, end, push_nl: end < len || nl_tail, lines: bodies };
        let n = last - a + 1;
        let map: Vec<Option<usize>> =
            (0..n).map(|rel| if rel == 0 { Some(0) } else { None }).collect();
        self.apply_line_block(&blk, &[joined], &map);
        // 块由 n 行并为 1 行：块尾之下的书签整体上移 n-1 行
        self.shift_bookmarks_below(blk.b, -(n as isize - 1));
        true
    }

    /// 拆分行。返回是否改动。
    ///
    /// - 无选区：在光标处断行（回车等价的可重映射动作）；
    /// - 有选区：把选中文本独立成行——选区前后各插一个换行，前后残文
    ///   各留原行（「摘出片段」语义）；
    /// - 单次快照、主导行尾重建（选区内混合行尾归一）、书签按行结构
    ///   下推/保留：首行（残文前缀所在行）书签原位，其余被拆行的书签
    ///   并入块尾残文行；
    /// - 断在空行正中（前后皆空）→ 内容不变，no-op 不产快照。
    pub fn split_line(&mut self) -> bool {
        let (s, e) = match self.anchor {
            None => (self.cursor, self.cursor),
            Some(_) => self.ordered_selection().expect("有锚必有有序选区"),
        };
        let ls = s.line;
        let le = e.line;
        // 三段重建：pre | 选中文本（内部换行归一为主流行尾再切开）| post
        // （行尾符号本身由 apply_line_block 按主导行尾统一补回，此处不碰）
        let off_s = self.doc.line_to_char(ls) + s.col.min(self.line_display_len(ls));
        let off_e = self.doc.line_to_char(le) + e.col.min(self.line_display_len(le));
        let pre = {
            let body = self.line_body_without_eol(ls);
            let cut = s.col.min(body.chars().count());
            body.chars().take(cut).collect::<String>()
        };
        let post = {
            let body = self.line_body_without_eol(le);
            let cut = e.col.min(body.chars().count());
            body.chars().skip(cut).collect::<String>()
        };
        let middle_raw = if off_e > off_s {
            self.doc.slice_text(off_s, off_e)
        } else {
            String::new()
        };
        // 选区文本内的行尾统一到主流行尾（P26 口径的逆用：\r\n/\r → \n）
        let middle_normalized = {
            let eol = self.doc.line_ending();
            eol.normalize(&middle_raw)
        };
        let middle: Vec<String> = if middle_normalized.is_empty() {
            Vec::new()
        } else {
            middle_normalized
                .trim_end_matches('\n')
                .split('\n')
                .map(|p| p.to_owned())
                .collect()
        };
        // 区域尾形态：与 merge 同一通判——选中真实末行时其行尾即文档
        // 末尾换行，重建必须补回；无尾随换行的文档则不加
        let count = self.doc.line_count();
        let start_off = self.doc.line_to_char(ls);
        let end_off =
            if le + 1 < count { self.doc.line_to_char(le + 1) } else { self.doc.text_len() };
        let len = self.doc.text_len();
        let nl_tail =
            end_off == len && end_off > start_off
                && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        let mut new_lines = Vec::with_capacity(middle.len() + 2);
        new_lines.push(pre);
        new_lines.extend(middle.iter().cloned());
        new_lines.push(post);
        // no-op 守卫：新旧逐行一致（如空行正中断行）
        let old_lines: Vec<String> = (ls..=le).map(|i| self.line_body_without_eol(i)).collect();
        if new_lines == old_lines {
            return false;
        }
        // 书签映射：rel0 = pre 行原位；rel k>0 的行内容主体搬到新位置 k
        // （k ≤ middle.len() 时），越界（单行摘出场景）并入块尾 post 行
        let n_new = new_lines.len();
        let span = le - ls;
        let mut map: Vec<Option<usize>> = Vec::with_capacity(span + 1);
        for rel in 0..=span {
            map.push(if rel == 0 {
                Some(0)
            } else if rel <= middle.len() {
                Some(rel)
            } else {
                Some(n_new - 1)
            });
        }
        let blk = LineBlock {
            a: ls,
            b: le,
            start: start_off,
            end: end_off,
            push_nl: end_off < len || nl_tail,
            lines: old_lines,
        };
        self.apply_line_block(&blk, &new_lines, &map);
        // 块行数增量 = 新行数 - 原行数：块尾之下的书签整体平移
        self.shift_bookmarks_below(
            le,
            new_lines.len() as isize - blk.lines.len() as isize,
        );
        // 光标落到断出的新内容行首（无选区 = 后半段行首；选区 = 摘出行首）
        let target = (ls + 1).min(self.doc.line_count().saturating_sub(1));
        self.cursor = CursorPos { line: target, col: 0 };
        self.ensure_visible();
        true
    }

    /// 删除空行/空白行：有选区只清触及块，无选区清全文档。返回是否改动。
    ///
    /// - 幻影末行不参与（尾随换行的有无形态保持，P77 教训口径）；
    /// - 书签随幸存行搬到压缩后的新位置，被删行的书签丢弃（可撤销找回）；
    /// - 无可删行 → no-op 不产快照。
    pub fn delete_empty_lines(&mut self, kind: BlankKind) -> bool {
        let mut blk = self.collect_line_block();
        let mut kept: Vec<String> = Vec::with_capacity(blk.lines.len());
        let mut map: Vec<Option<usize>> = Vec::with_capacity(blk.lines.len());
        for line in &blk.lines {
            let blank = match kind {
                BlankKind::Empty => line.is_empty(),
                BlankKind::Whitespace => line.trim().is_empty(),
            };
            if blank {
                map.push(None);
            } else {
                map.push(Some(kept.len()));
                kept.push(line.clone());
            }
        }
        if kept.len() == blk.lines.len() {
            return false; // 无可删行：幂等 no-op 不产生撤销快照
        }
        let delta = kept.len() as isize - blk.lines.len() as isize;
        // 全删空的特殊形态学：块内一行不剩时，重建串为空，push_nl 的
        // 直觉（「补回块尾换行」）不再适用——此时它决定的是**前后内容的
        // 衔接符**：
        // * 块在文档头或顶到文档尾：外侧本无另一段内容（或其行尾已在
        //   块外），多插一个换行就是凭空多出的空行 → 不补；
        // * 块夹在两段内容中间：左右行各自的换行一个在块前、一个被并进
        //   了删除区，必须补一个换行否则相邻内容粘连成一行。
        if kept.is_empty() {
            blk.push_nl = blk.start > 0 && blk.end < self.doc.text_len();
        }
        self.apply_line_block(&blk, &kept, &map);
        // 净删了若干行：块尾之下的书签整体上移
        self.shift_bookmarks_below(blk.b, delta);
        true
    }

    /// 是否有非空列块选区。
    pub fn has_block(&self) -> bool {
        self.block_sel.is_some_and(|b| !b.is_empty())
    }

    /// 清除列块选区。返回是否原本存在块（调用方据此决定是否刷新）。
    pub fn clear_block(&mut self) -> bool {
        self.block_dragging = false;
        self.block_sel.take().is_some()
    }

    /// Alt+Shift 按下：以命中点为锚角开始块选拖拽。
    /// 第 73 轮 ⑯：软换行开态直接拒绝（设计 §4.10 互斥——列块依赖整行
    /// 矩形列几何，折行形态下无意义；开启软换行本身也会清掉既有块）。
    pub(crate) fn begin_block_select(&mut self, at: CursorPos) {
        if self.wrap.borrow().enabled {
            return;
        }
        self.block_dragging = true;
        self.block_sel = Some(BlockSel { anchor: at, head: at });
        // 块态与单选区互斥
        self.anchor = None;
        self.break_typing();
    }

    /// 拖拽中：更新对角（渲染实时跟随；无效中间态允许存在）。
    pub(crate) fn update_block_select(&mut self, to: CursorPos) {
        if let Some(b) = &mut self.block_sel {
            b.head = to;
        }
    }

    /// 松开鼠标：结束拖拽。返回是否留下有效块（空块自动清除——单击
    /// Alt+Shift 不应残留任何状态）。
    pub(crate) fn finish_block_select(&mut self) -> bool {
        self.block_dragging = false;
        match self.block_sel {
            Some(b) if !b.is_empty() => true,
            _ => {
                self.block_sel = None;
                false
            }
        }
    }

    /// 归一化块矩形 (r0, r1, c0, c1)；每行的实际右界会被该行长度截断
    /// （短行自动到行尾——主流编辑器同口径），由消费方各自处理。
    pub(crate) fn active_block(&self) -> Option<(usize, usize, usize, usize)> {
        let b = self.block_sel?;
        let (r0, r1, c0, c1) = b.normalized();
        (!b.is_empty()).then_some((r0, r1, c0, c1))
    }

    /// 复制块内容：各行截取片段以 `\n` 连接（不含行尾符；短行取到行尾）。
    /// 无块 → None。
    pub fn block_copy_text(&self) -> Option<String> {
        let (r0, r1, c0, c1) = self.active_block()?;
        let mut parts: Vec<String> = Vec::with_capacity(r1 - r0 + 1);
        for line in r0..=r1 {
            let start = self.doc.line_to_char(line);
            let len = self.line_display_len(line);
            let a = start + c0.min(len);
            let b = start + c1.min(len);
            if b > a {
                parts.push(self.doc.slice_text(a, b));
            } else {
                parts.push(String::new());
            }
        }
        Some(parts.join("\n"))
    }

    /// 删除块内容：逐行移除 [c0, c1) 片段（短行无内容则跳过该行），
    /// 行数不变 → 书签原位。产快照；成功后清块、光标落块左上角。
    /// 返回是否改动。
    pub fn delete_block_content(&mut self) -> bool {
        let Some((r0, r1, c0, c1)) = self.active_block() else {
            return false;
        };
        // 预检是否有任一行确有可删内容，全空则 no-op 不产快照
        let spans: Vec<(usize, usize, usize)> = (r0..=r1)
            .filter_map(|line| {
                let start = self.doc.line_to_char(line);
                let len = self.line_display_len(line);
                let a = start + c0.min(len);
                let b = start + c1.min(len);
                (b > a).then_some((a, b, len))
            })
            .collect();
        if spans.is_empty() {
            return false;
        }
        self.snapshot();
        // 从后往前删，前面的偏移不受影响
        for (a, b, _) in spans.iter().rev() {
            self.doc.remove_range(*a, *b);
        }
        self.invalidate_highlight_from(self.doc.line_to_char(r0));
        self.max_cols_stale = true;
        self.block_sel = None;
        self.cursor = CursorPos { line: r0, col: c0.min(self.line_display_len(r0)) };
        self.ensure_visible();
        true
    }

    /// 向块内插入文本（v2）：
    /// * 单行文本 → 逐行填入同一内容（v1 口径）；
    /// * 多行文本 → **循环填充**，块内第 i 行取文本第 `i % 文本行数` 行
    ///   （行数一致时逐行对应，不足时循环续用，主流列块粘贴手感）；
    /// * 文本行数多于块行数 → **块扩展**，余下行作为新行插到块末行
    ///   下方（下方内容与书签整体下移；新行以主导行尾收尾）。
    ///
    /// 行尾一律归一到主导行尾再切行；尾部孤立换行视为分隔符而非填充行。
    /// 产快照；成功后清块、光标落首行插入文本之后。返回是否改动。
    pub fn insert_into_block(&mut self, text: &str) -> bool {
        let Some((r0, r1, c0, c1)) = self.active_block() else {
            return false;
        };
        let eol = self.doc.line_ending();
        let normalized = eol.normalize(text);
        let mut lines: Vec<&str> = normalized.split(eol.newline()).collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.is_empty() || lines.iter().all(|l| l.is_empty()) {
            return false;
        }
        self.snapshot();
        let rows = r1 - r0 + 1;
        // 逐行删旧块内容 [c0, c1) 再填入（从后往前，前面的偏移不受影响）
        for offset in (0..rows).rev() {
            let line = r0 + offset;
            let fill = lines[offset % lines.len()];
            let start = self.doc.line_to_char(line);
            let len = self.line_display_len(line);
            let a = start + c0.min(len);
            let b = start + c1.min(len);
            if b > a {
                self.doc.remove_range(a, b);
            }
            self.doc.insert(a, fill);
        }
        // 块扩展：文本行数多于块行数 → 余下行插到块末行下方。
        // 块末行本身无行尾时先补一个换行单元把它与新增行隔开
        if lines.len() > rows {
            let extra = &lines[rows..];
            let eol_str = eol.newline();
            let bare_last = {
                let last = self.doc.line_str(r1);
                !(last.ends_with('\n') || last.ends_with('\r'))
            };
            let mut insert = String::new();
            if bare_last {
                insert.push_str(eol_str);
            }
            insert.push_str(&extra.join(eol_str));
            insert.push_str(eol_str);
            let at = self.doc.line_to_char((r1 + 1).min(self.doc.line_count()));
            self.doc.insert(at, &insert);
            self.shift_bookmarks_below(r1, extra.len() as isize);
        }
        self.invalidate_highlight_from(self.doc.line_to_char(r0));
        self.max_cols_stale = true;
        self.block_sel = None;
        let new_col = c0 + lines[0].chars().count();
        self.cursor = CursorPos { line: r0, col: new_col.min(self.line_display_len(r0)) };
        self.ensure_visible();
        true
    }

    /// 行注释前缀查表（第 64 轮）：按当前语法名（syntect 名，大小写
    /// 不敏感匹配）取行注释前缀。未知语法默认 `//`。块注释不入表
    /// （`/* */` 需要成对处理，超出本动作的「整行」范畴）。
    pub(crate) fn comment_prefix_for(&self) -> &'static str {
        let lang = self.highlight_syntax_name().unwrap_or_default();
        let lang = lang.to_ascii_lowercase();
        match lang.as_str() {
            "python" | "ruby" | "shellscript" | "bash" | "yaml" | "toml" | "ini"
            | "properties" | "r" | "perl" => "#",
            "sql" | "lua" => "--",
            "batch file" | "bat" | "dosbatch" => "::",
            _ => "//", // rust/c/cpp/java/js/ts/go/json/css/php/未知……
        }
    }

    /// 行注释切换（Ctrl+Q）。有选区只作用触及块，无选区全文。
    /// 返回是否改动。
    ///
    /// - 判定：块内**全部非空行的缩进后都已带本前缀** → 去（每行剥一层：
    ///   前缀 + 紧随的一个空格，若有）；否则 → 加（在每行首个非空白字符
    ///   前插 `前缀 + 空格`；纯空行跳过不参与）；
    /// - 行数不变 → 恒等映射书签原位；无变化 no-op 不产快照；
    /// - 前缀只认「缩进后紧跟」——字符串里出现的 `//` 不受影响（不做
    ///   语法感知是既有取舍，与括号匹配同口径）。
    pub fn toggle_line_comment(&mut self) -> bool {
        let prefix = self.comment_prefix_for();
        let blk = self.collect_line_block();
        let is_commented = |s: &str| -> bool {
            let t = s.trim_start();
            !t.is_empty() && t.starts_with(prefix)
        };
        let commentable: Vec<bool> = blk.lines.iter().map(|s| !s.trim().is_empty()).collect();
        // 全部非空行已注释 → 去一层；否则加一层
        let unwrap_mode = blk
            .lines
            .iter()
            .zip(commentable.iter())
            .all(|(s, &c)| !c || is_commented(s));
        let mut changed = false;
        let lines: Vec<String> = blk
            .lines
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if !commentable[i] {
                    return s.clone();
                }
                let out = if unwrap_mode {
                    // 剥：缩进保留，去前缀 + 至多一个紧随空格
                    let t = s.trim_start();
                    let ind = &s[..s.len() - t.len()];
                    let rest = &t[prefix.len()..];
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{ind}{rest}")
                } else {
                    // 加：插到首个非空白字符前
                    let t = s.trim_start();
                    let ind = &s[..s.len() - t.len()];
                    format!("{ind}{prefix} {t}")
                };
                changed |= out != *s;
                out
            })
            .collect();
        if !changed {
            return false;
        }
        let map: Vec<Option<usize>> = (0..lines.len()).map(Some).collect();
        self.apply_line_block(&blk, &lines, &map);
        true
    }

    /// 当前行是否带书签（渲染层逐可见行查询，O(log n)）。
    pub fn is_bookmarked(&self, line: usize) -> bool {
        self.bookmarks.contains(&line)
    }

    /// 全部书签行（升序）。测试与应用层诊断消费。
    pub fn bookmarked_lines(&self) -> Vec<usize> {
        self.bookmarks.iter().copied().collect()
    }

    /// 当前行书签开关。返回切换后的状态（true = 现在带书签）。
    ///
    /// 每次实际翻转都先入撤销栈（快照携带翻转前的书签集，见
    /// [`Snapshot::bookmarks`]）——书签是可回滚状态，代价是开关会打断
    /// 打字组并清空重做栈，与一切状态变更操作同待遇。
    pub fn toggle_bookmark(&mut self) -> bool {
        self.snapshot();
        let line = self.cursor.line;
        if !self.bookmarks.remove(&line) {
            self.bookmarks.insert(line);
            return true;
        }
        false
    }

    /// 清除全部书签。返回是否有清除动作（空集合为幂等 no-op，不入栈）。
    pub fn clear_bookmarks(&mut self) -> bool {
        if self.bookmarks.is_empty() {
            return false;
        }
        self.snapshot();
        self.bookmarks.clear();
        true
    }

    /// 下一个/上一个书签的行号（[`Self::next_bookmark`] 的纯查询半边）：
    /// 光标之后（前）最近者；没有更近的就回绕到第一个（最后一个）。
    pub(crate) fn next_bookmark_line(&self, forward: bool) -> Option<usize> {
        let cur = self.cursor.line;
        if forward {
            self.bookmarks
                .range((std::ops::Bound::Excluded(cur), std::ops::Bound::Unbounded))
                .next()
                .or_else(|| self.bookmarks.iter().next())
                .copied()
        } else {
            self.bookmarks
                .range((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(cur)))
                .next_back()
                .or_else(|| self.bookmarks.iter().next_back())
                .copied()
        }
    }

    /// 跳到下一个（`forward`）/上一个书签。唯一书签恰为当前行时原地不动
    /// 并返回 false；无任何书签同理。纯光标移动：不产生快照，但按 P37
    /// 口径打断打字组。
    pub fn next_bookmark(&mut self, forward: bool) -> bool {
        let Some(target) = self.next_bookmark_line(forward) else {
            return false;
        };
        if target == self.cursor.line {
            return false;
        }
        self.break_typing();
        self.anchor = None;
        self.cursor = CursorPos { line: target, col: 0 };
        self.ensure_visible();
        true
    }

    /// 复制全部标记行：按升序取各行正文（剥换行单元）以主导行尾连接，
    /// 每行末尾都带行尾——粘贴到他处保持整行语义。无书签返回 None。
    /// 只读操作：不改文档、不置脏、不产快照。
    pub fn copy_bookmarked_lines(&self) -> Option<String> {
        if self.bookmarks.is_empty() {
            return None;
        }
        let nl = self.doc.line_ending().newline();
        let mut out = String::new();
        for &l in &self.bookmarks {
            if l >= self.doc.line_count() {
                continue; // 防御：悬空行号不参与（正常路径不可达）
            }
            out.push_str(&self.line_body_without_eol(l));
            out.push_str(nl);
        }
        (!out.is_empty()).then_some(out)
    }

    /// 删除全部标记行（含各自行尾）。返回是否发生删除。
    ///
    /// * 字节区间按原文档一次算齐、自底向上逐段移除——高地址段先行，
    ///   低地址偏移全程有效；
    /// * 连续标记行合并成一段，最小化 remove 次数；
    /// * 幻影末行（文档以换行收尾时空出的末行）单独被标记时退化为吃掉
    ///   它前面的换行单元，与 [`Self::delete_current_lines`] 的幻影分支
    ///   同口径；
    /// * 成功后书签集合自然清空（删的就是全部书签行）；撤销经快照把
    ///   文本与书签一并找回。
    pub fn remove_bookmarked_lines(&mut self) -> bool {
        if self.bookmarks.is_empty() {
            return false;
        }
        let count = self.doc.line_count();
        let len = self.doc.text_len();
        let marked = self.bookmarked_lines();

        // 连续行合并成段 [r0..=r1]
        let mut runs: Vec<(usize, usize)> = Vec::with_capacity(marked.len());
        let mut r0 = marked[0];
        let mut prev = marked[0];
        for &l in &marked[1..] {
            if l == prev + 1 {
                prev = l;
            } else {
                runs.push((r0, prev));
                r0 = l;
                prev = l;
            }
        }
        runs.push((r0, prev));

        // 段 → 字节区间
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(runs.len());
        let mut first_deleted_at = usize::MAX;
        for (a, b) in runs {
            let start = self.doc.line_to_char(a);
            let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { len };
            if start < end {
                spans.push((start, end));
            } else if start > 0 {
                // 幻影末行空壳单标：改为移除其前面的换行单元（\r\n 整体）。
                // 同 delete_current_lines：start == 1 时必非 CRLF，先判 ≥ 2
                // 再回看，防文档恰为一个换行符时下溢。
                let crlf = start >= 2 && self.doc.slice_text(start - 2, start) == "\r\n";
                spans.push((start - usize::from(crlf) - 1, start));
            }
            // start == end == 0：空文档唯一空行被标，无可删（跳过）
            first_deleted_at = first_deleted_at.min(spans.last().map_or(usize::MAX, |r| r.0));
        }
        if spans.is_empty() {
            return false;
        }

        self.snapshot();
        spans.sort_unstable_by_key(|x| std::cmp::Reverse(x.0)); // 自底向上
        for (s, e) in spans {
            self.doc.remove_range(s, e);
        }
        let at = first_deleted_at.min(self.doc.text_len());
        let line = self.doc.char_to_line(at);
        let col = at - self.doc.line_to_char(line);
        self.cursor = CursorPos { line, col };
        self.anchor = None;
        self.invalidate_highlight_from(at);
        // 行结构整体变化——列高水位交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        self.bookmarks.clear();
        self.ensure_visible();
        true
    }

    /// 变化点之后的书签整体平移 `delta`（行号 > `from_exclusive` 参与；
    /// 平移出文档范围（≤0）的书签丢弃——防御，正常路径不可达）。
    pub(crate) fn remap_shift_below(&mut self, from_exclusive: usize, delta: isize) {
        if delta == 0 || self.bookmarks.is_empty() {
            return;
        }
        self.bookmarks = self
            .bookmarks
            .iter()
            .filter_map(|&l| {
                let moved =
                    if l > from_exclusive { l as isize + delta } else { l as isize };
                usize::try_from(moved).ok()
            })
            .collect();
    }

    /// 删除 `[first, first+count)` 行后的重映射：区间内书签随行消失，
    /// 其后整体上移 count（[`Self::delete_current_lines`] /
    /// [`Self::remove_bookmarked_lines`] 幻影分支共用）。
    pub(crate) fn remap_removed_rows(&mut self, first: usize, count: usize) {
        let gone_end = first + count;
        self.bookmarks = self
            .bookmarks
            .iter()
            .filter_map(|&l| match l {
                _ if l >= gone_end => Some(l - count),
                _ if l >= first => None,
                _ => Some(l),
            })
            .collect();
    }

    /// 两行并成一行的重映射（回退删除换行单元 / Delete 吞掉换行单元）：
    /// 被吞行与幸存行的书签取**并集**落在幸存行上（任一来源有标记即视
    /// 为合并结果有标记），其后行号整体 −1。
    pub(crate) fn remap_merge_pair(&mut self, survivor: usize, gone: usize) {
        let had_gone = self.bookmarks.remove(&gone);
        if had_gone {
            self.bookmarks.insert(survivor);
        }
        self.remap_shift_below(gone, -1);
    }

    /// 跨行区域被替换（选区删除/替换、跨行选区粘贴）的重映射：
    /// * 起点行前缀幸存（`start_keeps` = 选区起点列 > 0）时保留自身书签，
    ///   整行被吃掉则丢弃；
    /// * 终点行内容必有幸存部分（终点列 > 0 剩后缀；= 0 则整行并入结果
    ///   行）→ 其书签恒**并入**结果行；
    /// * 中间整行内容消失 → 书签丢弃；
    /// * 其后行按消失行数 `vanished` 上移。
    pub(crate) fn remap_replaced_span(
        &mut self,
        start_line: usize,
        start_keeps: bool,
        end_line: usize,
        vanished: usize,
    ) {
        // 结果行书签 = (起点行自身 ∧ 前缀幸存) ∨ 终点行并入——两项独立
        // 成立即保留，起点行整行被吃不得连坐终点行并进来的书签
        let had_start = self.bookmarks.contains(&start_line) && start_keeps;
        let had_end = self.bookmarks.remove(&end_line);
        if had_start || had_end {
            self.bookmarks.insert(start_line);
        } else {
            self.bookmarks.remove(&start_line);
        }
        let interior: Vec<usize> =
            self.bookmarks.range(start_line + 1..end_line).copied().collect();
        for l in interior {
            self.bookmarks.remove(&l);
        }
        self.remap_shift_below(end_line, -(vanished as isize));
    }

    /// 块级重排（排序/去重）的按映射搬迁：`map[rel]` = 块内原第 rel 行的
    /// 新位置；`None` = 该行内容消失（去重的重复项），书签丢弃。映射表
    /// 不覆盖的行（如块外的幻影末行）原位保留——排序/去重不改变块前行数
    /// 与尾随换行的有无，幻影行号恒不变。
    pub(crate) fn remap_block_mapping(&mut self, start_line: usize, map: &[Option<usize>]) {
        if self.bookmarks.is_empty() {
            return;
        }
        let mut out = BTreeSet::new();
        for &l in &self.bookmarks {
            let new_line = match l.checked_sub(start_line).and_then(|rel| map.get(rel)) {
                Some(Some(new_rel)) => start_line + new_rel,
                Some(None) => continue,
                None => l,
            };
            out.insert(new_line);
        }
        self.bookmarks = out;
    }

    /// 第 62 轮：块外书签平移。合并/拆分/删空行经 [`Self::apply_line_block`]
    /// 改变行数时，块内映射管不到的「块尾之下」行号必须按块的行数增量
    /// 整体平移（排序/去重行数不变故无此需求）。`boundary` = 块的最后一
    /// 个原始行号，`delta` 为负即上方净删了行。须在 apply 之后调用
    /// （apply 内部已先行快照，撤销可整体回滚）。
    pub(crate) fn shift_bookmarks_below(&mut self, boundary: usize, delta: isize) {
        if delta == 0 || self.bookmarks.is_empty() {
            return;
        }
        self.bookmarks = self
            .bookmarks
            .iter()
            .map(|&l| {
                if l > boundary {
                    (l as isize + delta).max(0) as usize
                } else {
                    l
                }
            })
            .collect();
    }
}
