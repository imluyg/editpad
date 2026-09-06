use super::*;

impl EditorCore {
    /// 用新文档整体替换（全部替换时用），保留撤销链以便反悔。
    pub fn replace_whole_document(&mut self, doc: Document) {
        self.snapshot();
        self.doc = doc;
        self.cursor = CursorPos::default();
        self.anchor = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        // 第 60 轮：全部替换后行号与旧内容的对应关系不可信，书签整体作废
        // （撤销仍可经快照找回——书签已随 snapshot() 入栈）
        self.bookmarks.clear();
        self.recompute_max_line_cols();
        // 第 61 轮：经唯一汇点失效——顺带清括号匹配缓存（光标复位 (0,0)，
        // 全部替换后不得吐旧文档的陈旧配对）
        self.invalidate_highlight_from(0);
    }

    /// P37：打断当前打字组。凡不经过 [`Self::snapshot`] 的状态变更
    /// （光标移动、选区变更、撤销/重做、焦点离开、点击落点）都必须调用，
    /// 保证组内只有真正连续的单字符输入。
    pub(crate) fn break_typing(&mut self) {
        self.typing_run = None;
    }

    /// 在光标处插入文本（先吃掉当前选区）。支持多行文本。
    /// P9：入文前把 `\r\n` / `\n` / 孤立 `\r` 统一归一为文档主导行尾，
    /// 回车（插 `\n`）、输入法上屏、剪贴板粘贴共用本入口，不再产生混合行尾。
    /// P37 打字成组：仅当「上一操作就是本组内的一次合格插入、本次也是
    /// 单个非换行字符、无选区、插入点恰好接在上次结束处」才并入当前组
    /// （跳过快照，一次撤销撤掉整段连续输入）；换行、粘贴（多字符）、
    /// 选区替换一律开新组。
    pub fn insert_str(&mut self, text: &str) {
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        let text = self.doc.line_ending().normalize(text);
        let single = {
            let mut it = text.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let at = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        let merges = match (self.typing_run, single) {
            (Some(run_end), Some(c)) => {
                c != '\n' && c != '\r' && self.selection_offsets().is_none() && at == run_end
            }
            _ => false,
        };
        // 本次插入是否合格为「组内一员」——决定下一字符能否继续并入
        let eligible = matches!(single, Some(c) if c != '\n' && c != '\r')
            && self.selection_offsets().is_none();
        if !merges {
            self.snapshot();
        }
        self.typing_run = None; // 插入成功且合格后在本函数末尾重立

        // P125 覆写模式：单字符、无选区、行内有字符时先吃掉光标处字符
        // 再走常规插入（净长度不变 → 打字成组/光标推进/书签映射全兼容）；
        // 行尾与换行字符不参与（在行尾照常追加）。多字符（粘贴/IME 上屏）
        // 恒为插入，与主流编辑器一致。
        let overwrite_char = if self.overwrite {
            match single {
                Some(c) if c != '\n' && c != '\r' && self.selection_offsets().is_none() => {
                    let body_len = self.line_display_len(self.cursor.line);
                    if self.cursor.col < body_len {
                        self.char_at(at)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        if let Some(rc) = overwrite_char {
            self.doc.remove_range(at, at + rc.len_utf8());
        }


        // 第 60 轮：跨行选区将被替换——先记录 (起点行, 起点列>0, 终点行,
        // 消失行数) 供书签再映射（口径与 touched_lines 一致：末点在行首
        // 时该行不算触及，其内容整体并入结果行）
        let replaced_span: Option<(usize, bool, usize, usize)> =
            self.ordered_selection().and_then(|(s, e)| {
                (e.line > s.line).then(|| {
                    let vanished = if e.col == 0 {
                        e.line - 1 - s.line
                    } else {
                        e.line - s.line
                    };
                    (s.line, s.col > 0, e.line, vanished)
                })
            });
        let start_offset = match self.selection_offsets() {
            Some((start, end)) => {
                if end > start {
                    self.doc.remove_range(start, end);
                }
                let line = self.doc.char_to_line(start);
                let col = start - self.doc.line_to_char(line);
                self.cursor = CursorPos { line, col };
                start
            }
            None => self.doc.line_to_char(self.cursor.line) + self.cursor.col,
        };
        self.anchor = None;
        let first_line = self.cursor.line;

        self.doc.insert(start_offset, &text);
        self.invalidate_highlight_from(start_offset);

        // 推进光标到插入文本的末尾（EOL 单元感知，CRLF 算一次换行）
        let (new_lines, tail_cols) = measure_insertion(&text);
        if new_lines > 0 {
            self.cursor.line += new_lines;
            self.cursor.col = tail_cols;
        } else {
            self.cursor.col += tail_cols;
        }
        // P13：受影响行（插入跨行时含沿途各行）宽度只上调高水位
        self.raise_max_line_cols(first_line..=self.cursor.line);
        // 第 60 轮：书签再映射——被替换的跨行选区按精化规则搬迁（起点行
        // 前缀幸存才保留自身书签；终点行内容必有幸存 → 书签恒并入结果
        // 行；中间整行丢弃）；插入的换行再把变化点之后的行号下推
        if let Some((sa, keeps, eb, van)) = replaced_span {
            self.remap_replaced_span(sa, keeps, eb, van);
        }
        self.remap_shift_below(first_line, new_lines as isize);
        // P37：合格单字符插入把组延伸到新的结束偏移；换行/粘贴/选区替换
        // 保持 None——下一字符开新组
        self.typing_run = if eligible {
            Some(start_offset + 1)
        } else {
            None
        };
        self.ensure_visible();
    }

    /// 用给定文本替换当前选区；无选区时退化为插入。
    pub fn replace_selection(&mut self, text: &str) {
        self.insert_str(text);
    }

    /// P121 智能缩进回车：插入「换行 + 当前行行首空白」。
    ///
    /// 继承口径 = 行首到首个非空白字符（仅空格/Tab 计入），再与光标列取
    /// 较小者——光标停在行首空白内时只带到光标处，不把其后的空白拖到
    /// 新行。整体文本走 insert_str 统一入口：EOL 归一为主导行尾（P9）、
    /// 换行开新撤销组（P37）、跨行选区替换的书签再映射全部继承。
    pub fn enter(&mut self) {
        let body = self.line_body_without_eol(self.cursor.line);
        let indent_len = body
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count()
            .min(self.cursor.col);
        let mut text = String::with_capacity(indent_len + 2);
        text.push('\n');
        text.extend(body.chars().take(indent_len));
        self.insert_str(&text);
    }

    /// P122 无选区 Ctrl+C/X 的整行取词：当前行原文（含本行行尾单元，
    /// 幻影末行返回空串）。复制/剪切共用，不改文档、不置脏。
    pub fn current_line_copy_text(&self) -> String {
        self.doc.line_str(self.cursor.line).to_string()
    }

    /// P122 删词：Ctrl+Backspace 删到词首 / Ctrl+Delete 删到词尾。
    ///
    /// - 有选区：退化为普通退格/删除（删选区，主流口径）；
    /// - 词边界复用 [`Self::word_neighbor`]（与 Ctrl+←/→ 同源）；
    /// - 删除经 delete_selection 统一管线（快照、跨行书签再映射、
    ///   光标落点全部继承），删除前先打断打字组（P37）。
    pub fn delete_word(&mut self, left: bool) -> bool {
        if self.selection_offsets().is_some() {
            return if left {
                self.backspace()
            } else {
                self.delete_forward()
            };
        }
        let Some((line, col)) = self.word_neighbor(left) else {
            return false; // 文档边缘无从删
        };
        if line == self.cursor.line && col == self.cursor.col {
            return false;
        }
        self.break_typing(); // P37：删词打断打字组
        self.anchor = Some(CursorPos { line, col });
        self.delete_selection()
    }
    /// P128 选区文本工具：Base64/URL 编解码与 MD5/SHA-256 摘要。
    ///
    /// - 仅作用于非空选区（无选区 → Err 提示）；
    /// - 摘要输出小写十六进制并替换选区；解码失败 → Err 不动文档；
    /// - 结果与原文相同时幂等 no-op（Ok(false) 不产快照）；
    /// - 转换后选区保持覆盖新文本（与 convert_case 同款偏移重建）。
    pub fn apply_tool(&mut self, kind: ToolKind) -> Result<bool, String> {
        let (start, end) = match self.selection_offsets() {
            Some((s, e)) if s < e => (s, e),
            _ => return Err("请先选中要处理的文本".to_owned()),
        };
        let src = self.doc.slice_text(start, end);
        let out = match kind {
            ToolKind::ToolBase64Encode => {
                editpad_core::toolkit::base64_encode(src.as_bytes())
            }
            ToolKind::ToolBase64Decode => {
                let bytes = editpad_core::toolkit::base64_decode(&src).ok_or_else(|| {
                    "Base64 解码失败：选区不是有效的 Base64 文本".to_owned()
                })?;
                String::from_utf8(bytes)
                    .map_err(|_| "Base64 解码结果不是有效的 UTF-8 文本".to_owned())?
            }
            ToolKind::ToolUrlEncode => editpad_core::toolkit::url_encode(&src),
            ToolKind::ToolUrlDecode => editpad_core::toolkit::url_decode(&src)
                .ok_or_else(|| "URL 解码失败：选区含有无效的百分号转义".to_owned())?,
            ToolKind::ToolMd5 => editpad_core::toolkit::md5_hex(src.as_bytes()),
            ToolKind::ToolSha256 => editpad_core::toolkit::sha256_hex(src.as_bytes()),
        };
        if out == src {
            return Ok(false); // 幂等 no-op
        }
        let had_selection = self.anchor.is_some();
        self.snapshot();
        self.doc.remove_range(start, end);
        self.doc.insert(start, &out);
        self.invalidate_highlight_from(start);
        // 摘要/编码可能大幅改变长度——列高水位交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        if had_selection {
            let end_off = start + out.chars().count();
            let sl = self.doc.char_to_line(start);
            let sc = start - self.doc.line_to_char(sl);
            let el = self.doc.char_to_line(end_off.min(self.doc.text_len()));
            let ec = end_off.min(self.doc.text_len()) - self.doc.line_to_char(el);
            self.anchor = Some(CursorPos { line: sl, col: sc });
            self.cursor = CursorPos { line: el, col: ec };
        } else {
            self.anchor = None;
        }
        self.ensure_visible();
        Ok(true)
    }

    /// 把选区起点放到 `(line, col)` 并向右延伸 `len_chars` 个字符形成新选区。
    pub fn select_span(&mut self, line: usize, col: usize, len_chars: usize) {
        self.break_typing(); // P37：选区变更打断组（查找跳转/替换当前都经此）
        self.clear_block(); // 第 67 轮：块态与单选区互斥
        let start = CursorPos { line, col };
        let mut cur = start;
        let mut remain = len_chars;
        while remain > 0 {
            let line_len = self.line_display_len(cur.line);
            let room = line_len.saturating_sub(cur.col);
            if room >= remain {
                cur.col += remain;
                remain = 0;
            } else if cur.line + 1 < self.doc.line_count() {
                remain -= room + 1; // 吃掉换行符
                cur.line += 1;
                cur.col = 0;
            } else {
                cur.col = line_len;
                break;
            }
        }
        self.anchor = Some(start);
        self.cursor = cur;
        self.ensure_visible();
    }

    /// 全文字符偏移处的字符；越界返回 None
    /// （ropey 切片越界会 panic，探测相邻字符前必须先夹紧）。
    pub(crate) fn char_at(&self, offset: usize) -> Option<char> {
        if offset >= self.doc.text_len() {
            return None;
        }
        self.doc.slice_text(offset, offset + 1).chars().next()
    }

    /// 光标偏移处是否为完整的 CRLF 换行单元（`\r\n` 相邻成对）。
    pub(crate) fn is_crlf_at(&self, offset: usize) -> bool {
        self.char_at(offset) == Some('\r') && self.char_at(offset + 1) == Some('\n')
    }

    /// 退格。返回是否真的删除了内容——文档原点且无选区时是静默 no-op，
    /// 应用层须以返回值判定置脏/自动保存，不得把空操作当编辑。
    pub fn backspace(&mut self) -> bool {
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        if self.delete_selection() {
            return true;
        }
        if self.cursor == CursorPos::default() {
            return false;
        }
        self.snapshot();
        // 第 60 轮：col==0 且非首行 = 将删掉换行单元并上一行（书签取并集）
        let merges_up = self.cursor.col == 0 && self.cursor.line > 0;
        self.move_local(Motion::Left);
        let start = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        // P9：跨行回退落在 CRLF 上时把 `\r\n` 当一个换行单元整体移除。
        // 旧行为只删一半字符：第一下视觉无反应，第二下才真正并行的两行。
        let end = start + 1 + usize::from(self.is_crlf_at(start));
        self.doc.remove_range(start, end);
        if merges_up {
            // 幸存行 = 当前行（move_local 已回退到上一行），被吞行 = 下一行
            self.remap_merge_pair(self.cursor.line, self.cursor.line + 1);
        }
        self.invalidate_highlight_from(start);
        // P13：并行后的新行可能更宽（也可能只是收缩——高水位不回退）
        self.raise_max_line_cols(self.cursor.line..=self.cursor.line);
        self.ensure_visible();
        true
    }

    /// 前向删除。返回是否真的删除了内容——文档末尾且无选区时是静默
    /// no-op，应用层须以返回值判定置脏/自动保存，不得把空操作当编辑。
    pub fn delete_forward(&mut self) -> bool {
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        if self.delete_selection() {
            return true;
        }
        let offset = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        if offset >= self.doc.text_len() {
            return false;
        }
        // 第 60 轮：光标已在行尾（显示口径）= 将删掉换行单元并下一行进来
        let merges_down = self.cursor.col >= self.line_display_len(self.cursor.line);
        self.snapshot();
        // P9：行尾 Delete 同样按 EOL 单元处理，一下删掉整个 `\r\n`
        let end = offset + 1 + usize::from(self.is_crlf_at(offset));
        self.doc.remove_range(offset, end);
        if merges_down {
            // 幸存行 = 当前行，被吞行 = 下一行（文档末尾的孤立换行也适用：
            // 幻影行消失，并集与平移都是无害 no-op）
            self.remap_merge_pair(self.cursor.line, self.cursor.line + 1);
        }
        self.invalidate_highlight_from(offset);
        // P13：下一行并入当前行，合并结果可能更宽
        let merged = self.cursor.line;
        self.raise_max_line_cols(merged..=merged);
        self.ensure_visible();
        true
    }

    /// 下发不可见字符标记开关（设置保存/建页时调用）。
    pub fn set_invisibles(&mut self, whitespace: bool, line_endings: bool) {
        self.show_whitespace = whitespace;
        self.show_line_endings = line_endings;
    }

    /// P132：下发缩进参考线开关（设置保存/建页时调用，仅影响绘制）。
    pub fn set_indent_guides(&mut self, on: bool) {
        self.indent_guides = on;
    }

    /// P132：下发右缘标尺列（0 = 关，设置保存/建页时调用，仅影响绘制）。
    pub fn set_edge_column(&mut self, col: u32) {
        self.edge_column = col;
    }

    /// P133：下发相对路径链接的解析基准（本页文件所在目录）。
    /// tab.path 变更的所有位点（装载/另存/改名/关闭）调用。
    pub fn set_base_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.base_dir = dir;
    }

    /// P133：在 (line, col) 处探测链接（列 = 行内字符索引，hit_test 同
    /// 口径）。路径存在性判定含 base_dir 回退（原样不存在时试「本页文件
    /// 所在目录 + 路径」，构建日志里的相对路径主要靠它）。返回
    /// `(c0, c1, target)`。
    pub(crate) fn resolve_link_at(
        &self,
        line: usize,
        col: usize,
    ) -> Option<(usize, usize, super::LinkTarget)> {
        let text = self.line_text(line);
        let base = self.base_dir.clone();
        let resolve = move |p: &str| -> Option<std::path::PathBuf> {
            let path = std::path::Path::new(p);
            if path.exists() {
                return Some(path.to_path_buf());
            }
            base.as_deref().and_then(|b| {
                let joined = b.join(p);
                joined.exists().then_some(joined)
            })
        };
        super::link_at(&text, col, &resolve)
    }

    /// 光标的全文字符偏移（第 69 轮状态栏「位置」用，1 起显示由调用方
    /// +1）。列即行内字符索引，无宽字符折算——与查找偏移同口径。
    pub fn cursor_offset(&self) -> usize {
        self.doc.line_to_char(self.cursor.line) + self.cursor.col
    }

    /// 当前选区的显示字符数（None = 无选区/零宽）。口径与查找命中的
    /// `len_chars` 一致：行尾 `\r\n` 计 1、每跨一行计 1——可直接与
    /// 「字符数」相加对账。
    ///
    /// 跨行计数是 O(选区行数)，而状态栏每帧都会查询：经偏移对键控缓存
    /// （sel_span_cache，同一失效汇点），锚点/光标不动时命中帧零开销。
    pub fn selection_display_len(&self) -> Option<usize> {
        let (s, e) = self.ordered_selection()?;
        let key = (
            self.doc.line_to_char(s.line) + s.col,
            self.doc.line_to_char(e.line) + e.col,
        );
        if let Some(hit) = self.sel_span_cache.borrow().as_ref() {
            if hit.0 == key {
                return hit.1;
            }
        }
        let span: Option<usize> = if s.line == e.line {
            Some(e.col.saturating_sub(s.col))
        } else if s.line < self.doc.line_count() && e.line < self.doc.line_count() {
            // 首行余部 + 其换行占 1；中间各行 disp+1；末行前缀 e.col
            let mut n =
                self.line_display_len(s.line) - s.col.min(self.line_display_len(s.line)) + 1;
            for l in (s.line + 1)..e.line {
                n += self.line_display_len(l) + 1;
            }
            Some(n + e.col)
        } else {
            None
        };
        *self.sel_span_cache.borrow_mut() = Some((key, span));
        span
    }

    /// F5：在光标处插入当前本地日期时间（记事本同款）。
    /// 格式 `YYYY-MM-DD HH:MM`（24 小时制）；本地时区不可得时降级 UTC
    /// 并在状态栏提示（time 的 now_local 在极端环境可能 Err）。
    /// 返回插入的时间戳文本（应用层状态栏反馈用）。
    pub fn insert_date_time(&mut self) -> String {
        let stamp = local_datetime_stamp();
        self.insert_str(&stamp);
        stamp
    }

    /// 有选区时删除之（含快照）；返回是否发生了删除。零宽选区仅清除标记。
    pub(crate) fn delete_selection(&mut self) -> bool {
        if self.selected_text().is_some() {
            // 第 60 轮：跨行选区删除 = 多行并一行——先记 (起点行, 起点列>0,
            // 终点行, 消失行数)，删除后按精化规则再映射书签（同 insert_str）
            let span = self.ordered_selection().and_then(|(s, e)| {
                (e.line > s.line).then(|| {
                    let vanished = if e.col == 0 {
                        e.line - 1 - s.line
                    } else {
                        e.line - s.line
                    };
                    (s.line, s.col > 0, e.line, vanished)
                })
            });
            self.snapshot();
            if let Some((start, end)) = self.selection_offsets() {
                self.doc.remove_range(start, end);
                let line = self.doc.char_to_line(start);
                let col = start - self.doc.line_to_char(line);
                self.cursor = CursorPos { line, col };
                self.invalidate_highlight_from(start);
                // P13：跨行删除后首尾两行拼成一行的宽度可能变化
                let joined = self.cursor.line;
                self.raise_max_line_cols(joined..=joined);
                if let Some((sa, keeps, eb, van)) = span {
                    self.remap_replaced_span(sa, keeps, eb, van);
                }
            }
            self.anchor = None;
            self.ensure_visible();
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// 输入法预编辑事件：仅持有焦点时消费（存串并返回 true 供控件捕获），
    /// 否则放行给真正持有焦点的控件（如查找框）。
    pub fn ime_preedit(&mut self, content: String) -> bool {
        if !self.focused {
            return false;
        }
        self.preedit = if content.is_empty() {
            None
        } else {
            Some(content)
        };
        true
    }

    /// 输入法上屏事件：仅持有焦点时消费。文本的真正插入由应用层
    /// [`Message::Edit`] 统一分发（获得加载期守卫与撤销/置脏语义）。
    pub fn ime_commit(&mut self, text: &str) -> ImeCommit {
        if !self.focused {
            return ImeCommit::Ignored;
        }
        self.preedit = None;
        ImeCommit::Consumed((!text.is_empty()).then(|| text.to_owned()))
    }
}
