//! 纯文本查找与替换。
//!
//! M1 采用 O(n·m) 字符级扫描，几十 MB 量级完全够用；
//! M2 随 rope 化升级为增量匹配。
//!
//! 大小写语义：仅对 ASCII 做大小写折叠（`a == A`），非 ASCII 字符
//! （如中文）本身没有大小写概念，始终精确比较——这样所有位置
//! 计数都不会受 Unicode 折叠改变字符数的影响。

/// 一个匹配的位置：行号、行内字符列（均从 0 计）。
///
/// **API 变更（P26）**：新增 `len_chars` 字段——命中的「选区显示字符跨度」，
/// 与编辑器 `select_span` / `line_display_len` 同一坐标口径：
/// 行尾 `\r` 不计、每跨一行计 1。单行命中它恒等于查询字符数；
/// 跨行命中在 CRLF 文档上小于原始字符数（`\r\n` 整体只计 1），
/// 因此可以直接喂给 `select_span` 还原选区，不会多走一格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchPos {
    pub line: usize,
    pub col: usize,
    /// 命中的选区显示跨度（见结构体文档），单位与光标列一致。
    pub len_chars: usize,
}

use crate::document::Document;

/// 单字符相等判断：`case_sensitive=false` 时仅对 ASCII 折叠大小写。
///
/// P15 起作为全项目唯一的大小写折叠实现——app 层「选中文本是否等于查询」
/// 也复用这里，避免两处手写、改语义必漏一边。
pub fn ascii_case_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    ascii_case_eq(a, b, case_sensitive)
}

/// 找出全部匹配（按文档顺序）。查询串为空时返回空表。
///
/// P26：查询含 `\n` 时走 [`MultiLineScanner`] 跨行归一分支——
/// 文本侧 `\r\n` / 孤立 `\r` / `\n` 都算一个换行单元与查询的 `\n`
/// 判等；不含 `\n` 的查询维持原有单行路径，行为零变化。
pub fn find_all(text: &str, query: &str, case_sensitive: bool) -> Vec<MatchPos> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let q: Vec<char> = query.chars().collect();
    if q.contains(&'\n') {
        scan_multiline_str(text, &q, case_sensitive, &mut out);
        return out;
    }
    // 行字符缓冲跨行复用：50MB 级文档约 50 万行，逐行新建 Vec<char>
    // 会产生等量小堆分配（第 39 轮实测占查找耗时可观份额）
    let mut lc: Vec<char> = Vec::new();
    for (line_idx, line) in text.split('\n').enumerate() {
        scan_line(line, &q, case_sensitive, line_idx, &mut lc, &mut out);
    }
    out
}

/// 在 [`Document`]（rope）上直接查找，语义与 [`find_all`] 完全一致（P10）。
///
/// 与 `find_all(&doc.to_text(), ..)` 相比省掉整份全文 String：
/// 按存储块零拷贝迭代、手工按 `\n` 分段，峰值内存只多一个「当前行」缓冲
/// （复用分配，长度 = 最长行）。列语义逐字符对齐 `split('\n')`——
/// 行尾 `\r` 保留在行内（与 find_all 一致），孤立 `\r` 不当行界。
pub fn find_all_document(doc: &Document, query: &str, case_sensitive: bool) -> Vec<MatchPos> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let q: Vec<char> = query.chars().collect();

    // P26：跨行查询与 find_all 共用同一个扫描器（对拍不发散的结构保证），
    // rope 按存储块流式喂入，窗口缓冲 O(查询长度)
    if q.contains(&'\n') {
        scan_multiline_chunks(doc, &q, case_sensitive, &mut out);
        return out;
    }

    let mut line = String::new();
    let mut line_idx = 0usize;
    let mut lc: Vec<char> = Vec::new(); // 同 find_all：跨行复用字符缓冲
    for chunk in doc.chunks() {
        let mut rest = chunk;
        // 块边界可能落在任意位置：'\n' 前的残段累积进当前行缓冲，
        // 遇到完整 '\n' 才结算一行——保证与 split('\n') 逐字节等价
        while let Some(pos) = rest.find('\n') {
            line.push_str(&rest[..pos]);
            scan_line(&line, &q, case_sensitive, line_idx, &mut lc, &mut out);
            line.clear();
            line_idx += 1;
            rest = &rest[pos + 1..];
        }
        line.push_str(rest);
    }
    scan_line(&line, &q, case_sensitive, line_idx, &mut lc, &mut out);
    out
}

/// 单行窗口扫描：在 `line` 的字符序列上滑动长度 `q.len()` 的窗口逐一比较。
///
/// `lc` 为跨行复用的字符缓冲（clear+extend 重用分配；第 39 轮优化），
/// 语义与每行新建 `Vec<char>` 完全一致。
fn scan_line(
    line: &str,
    q: &[char],
    case_sensitive: bool,
    line_idx: usize,
    lc: &mut Vec<char>,
    out: &mut Vec<MatchPos>,
) {
    lc.clear();
    lc.extend(line.chars());
    if lc.len() < q.len() {
        return;
    }
    let first = q[0];
    'window: for start in 0..=(lc.len() - q.len()) {
        // 首字符快速过滤：绝大多数位置在此被跳过，省掉内层循环开销
        if !char_eq(first, lc[start], case_sensitive) {
            continue;
        }
        for (offset, &qc) in q.iter().enumerate().skip(1) {
            if !char_eq(qc, lc[start + offset], case_sensitive) {
                continue 'window;
            }
        }
        out.push(MatchPos {
            line: line_idx,
            col: start,
            // 单行命中无换行跨越，显示跨度 == 查询字符数
            len_chars: q.len(),
        });
    }
}

// ---------- P26：跨行查询的归一化扫描 ----------

/// 文本流归一后的扫描 token：普通字符，或一个换行单元。
///
/// 文本侧 `\r\n`、孤立 `\r`、`\n` 三种行尾一律折叠成 [`ScanToken::Newline`]
/// （与 ropey/编辑器的行界口径一致）；查询侧只做字面映射——字面 `\n`
/// 映射为 [`ScanToken::Newline`]（因此能命中任何形态的文本换行），
/// 字面 `\r` 保持普通字符（文本侧不再产出 `\r` token，故含 `\r`
/// 的查询在本分支永不命中——相比旧实现「含 `\n` 恒零命中」无回归，
/// 纯增量能力）。
#[derive(Clone, Copy, PartialEq, Debug)]
enum ScanToken {
    Char(char),
    Newline,
}

/// 查询字符 -> 扫描 token 的字面映射（见 [`ScanToken`] 文档）。
fn query_token(c: char) -> ScanToken {
    if c == '\n' {
        ScanToken::Newline
    } else {
        ScanToken::Char(c)
    }
}

/// token 判等：普通字符走 ASCII 折叠；查询的换行单元恰好对应文本侧
/// 归一换行单元。查询字面 `\r` 对任何文本 token 都失配。
fn token_matches(qt: ScanToken, tt: ScanToken, case_sensitive: bool) -> bool {
    match (qt, tt) {
        (ScanToken::Char(a), ScanToken::Char(b)) => char_eq(a, b, case_sensitive),
        (ScanToken::Newline, ScanToken::Newline) => true,
        _ => false,
    }
}

/// 窗口槽位：token + 产生它时的 (line, col) 显示坐标。
///
/// 坐标推进规则与编辑器一致：普通字符列 +1；换行单元本身记在当前
/// 行末列，随后行号 +1、列归零。孤立 `\r` 也算行界（ropey 默认
/// `unicode_lines` 特性行界含 `\r`，编辑器行号同源），跨行命中在
/// 此类文档上的 (line, col) 同样可被 `select_span` 正确还原。
struct WindowSlot {
    token: ScanToken,
    line: usize,
    col: usize,
}

/// 跨行滑动窗口扫描器——[`find_all`] 与 [`find_all_document`] 共用，
/// 结构上保证两条路径逐字节对拍不发散。
///
/// * 文本按任意片段推送（`push_str` / `finish`），块边界可以落在
///   包括 `\r\n` 中间在内的任意位置：`\r` 悬置到下一片段裁决；
/// * 窗口缓冲 O(查询长度)，峰值内存与文档大小无关；
/// * 行列坐标随推送增量维护，无需回看已消费文本；
/// * 命中判定沿用单行路径的两条契约：仅 ASCII 折叠、重叠窗口逐一
///   上报（每个可能补全命中的 token 都触发一次完整比较）。
struct MultiLineScanner {
    q: Vec<ScanToken>,
    case_sensitive: bool,
    buf: std::collections::VecDeque<WindowSlot>,
    cur_line: usize,
    cur_col: usize,
    /// 上一推送片段以 `\r` 结尾且尚未裁决是否 CRLF
    pending_cr: bool,
}

impl MultiLineScanner {
    fn new(q: &[char], case_sensitive: bool) -> Self {
        Self {
            q: q.iter().map(|&c| query_token(c)).collect(),
            case_sensitive,
            buf: std::collections::VecDeque::with_capacity(q.len().min(4096)),
            cur_line: 0,
            cur_col: 0,
            pending_cr: false,
        }
    }

    /// 推送一段任意长度的文本片段。
    ///
    /// 结构与 [`crate::document::EolCounter`] 同构：peekable 单遍扫描，
    /// `\r` 在片段末尾时悬置到下一片段裁决，孤立 `\r` 当场定案为换行。
    fn push_str(&mut self, text: &str, out: &mut Vec<MatchPos>) {
        let mut chars = text.chars().peekable();
        // 先裁决上一片段悬置的 `\r`（片段边界切在 `\r\n` 中间的情况）
        if self.pending_cr {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    self.pending_cr = false;
                    self.push_token(ScanToken::Newline, out);
                }
                Some(_) => {
                    // 孤立 `\r` 定案；当前字符留给下方常规循环处理
                    self.pending_cr = false;
                    self.push_token(ScanToken::Newline, out);
                }
                None => return, // 空片段：继续悬置
            }
        }
        while let Some(c) = chars.next() {
            match c {
                '\r' => match chars.peek() {
                    Some('\n') => {
                        chars.next();
                        self.push_token(ScanToken::Newline, out);
                    }
                    Some(_) => self.push_token(ScanToken::Newline, out), // 孤立 `\r`
                    None => self.pending_cr = true, // 片段末尾：悬置待下片段裁决
                },
                '\n' => self.push_token(ScanToken::Newline, out),
                other => self.push_token(ScanToken::Char(other), out),
            }
        }
    }

    /// 文本推送完毕：悬置的 `\r` 以孤立换行定案并做最后一次窗口检查。
    fn finish(&mut self, out: &mut Vec<MatchPos>) {
        if self.pending_cr {
            self.pending_cr = false;
            self.push_token(ScanToken::Newline, out);
        }
    }

    /// 吞入一个 token：维护坐标与 O(查询长度) 窗口，必要时上报命中。
    ///
    /// 命中跨度 = 窗口内 token 数——正是「选区显示跨度」口径：
    /// 普通字符计 1、每次跨行计 1（`\r\n` 整体计 1）。
    fn push_token(&mut self, token: ScanToken, out: &mut Vec<MatchPos>) {
        let qlen = self.q.len();
        // 尾 token 快速过滤：新进 token 必须等于查询尾 token 才可能补全命中
        let may_complete = token_matches(self.q[qlen - 1], token, self.case_sensitive);

        let slot = WindowSlot {
            token,
            line: self.cur_line,
            col: self.cur_col,
        };
        if slot.token == ScanToken::Newline {
            self.cur_line += 1;
            self.cur_col = 0;
        } else {
            self.cur_col += 1;
        }

        if self.buf.len() == qlen {
            self.buf.pop_front();
        }
        self.buf.push_back(slot);

        if may_complete && self.buf.len() == qlen && self.window_matches() {
            let front = &self.buf[0];
            out.push(MatchPos {
                line: front.line,
                col: front.col,
                len_chars: qlen,
            });
        }
    }

    fn window_matches(&self) -> bool {
        self.buf
            .iter()
            .zip(self.q.iter())
            .all(|(slot, &qt)| token_matches(qt, slot.token, self.case_sensitive))
    }
}

/// [`find_all`] 的跨行入口：&str 一次喂入。
fn scan_multiline_str(text: &str, q: &[char], case_sensitive: bool, out: &mut Vec<MatchPos>) {
    let mut scanner = MultiLineScanner::new(q, case_sensitive);
    scanner.push_str(text, out);
    scanner.finish(out);
}

/// [`find_all_document`] 的跨行入口：rope 存储块零拷贝流式喂入。
fn scan_multiline_chunks(
    doc: &Document,
    q: &[char],
    case_sensitive: bool,
    out: &mut Vec<MatchPos>,
) {
    let mut scanner = MultiLineScanner::new(q, case_sensitive);
    for chunk in doc.chunks() {
        scanner.push_str(chunk, out);
    }
    scanner.finish(out);
}

/// 光标 (line, col) 处（含该位置命中）之后的第一个匹配下标；
/// 没有更靠后的则回卷到第一个——对应编辑器「向下查找」的环绕语义。
pub fn next_from(matches: &[MatchPos], line: usize, col: usize) -> Option<usize> {
    matches
        .iter()
        .position(|m| m.line > line || (m.line == line && m.col >= col))
        .or(if matches.is_empty() { None } else { Some(0) })
}

/// 光标之前的最后一个匹配下标；没有则回卷到最后一个。
pub fn prev_from(matches: &[MatchPos], line: usize, col: usize) -> Option<usize> {
    matches
        .iter()
        .rposition(|m| m.line < line || (m.line == line && m.col < col))
        .or(if matches.is_empty() { None } else { Some(matches.len() - 1) })
}

/// 单字节大小写折叠：仅 ASCII 受影响，与 [`ascii_case_eq`] 同一口径
/// （≥0x80 的 UTF-8 字节原样保留，多字节字符永远精确比较）。
#[inline]
fn fold_byte(b: u8, case_sensitive: bool) -> u8 {
    if case_sensitive {
        b
    } else {
        b.to_ascii_lowercase()
    }
}

/// 在 `hay[from..]` 中找第一个「折叠后等于 fq」的字节窗口，返回命中起点；
/// 没有完整窗口则返回 None。
///
/// 字节级匹配的正确性依据 UTF-8 自同步性：任何字符编码的首字节要么是
/// ASCII（<0x80）要么是前导字节（≥0xC0），续字节一律落在 0x80..0xC0——
/// 所以查询首字节只可能等值于某个字符边界上的字节，逐字节推进不会把
/// 多字节字符切进命中窗口。
fn find_next(hay: &[u8], fq: &[u8], case_sensitive: bool, from: usize) -> Option<usize> {
    let first = fq[0];
    let len = fq.len();
    let mut i = from;
    while i + len <= hay.len() {
        // 首字节快速过滤：绝大多数位置在此被跳过
        if fold_byte(hay[i], case_sensitive) != first {
            i += 1;
            continue;
        }
        if hay[i..i + len]
            .iter()
            .zip(fq)
            .all(|(&h, &q)| fold_byte(h, case_sensitive) == q)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 全部替换，返回 (新文本, 替换次数)。
///
/// P11 重写：直接在 UTF-8 字节上扫描推进，不再把全文收集成
/// `Vec<char>`（每字符 4 字节，50MB 文档的替换峰值曾达原文 ×5），
/// 内存峰值降为「输入 + 输出」两份。折叠语义与旧实现完全一致：
/// 仅 ASCII 折叠、最左优先、命中后整体跳过查询长度（不重叠）、
/// 空查询是 no-op。
pub fn replace_all(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if query.is_empty() {
        return (text.to_owned(), 0);
    }
    let fq: Vec<u8> = query.bytes().map(|b| fold_byte(b, case_sensitive)).collect();
    let mut out = String::with_capacity(text.len());
    let mut count = 0usize;
    let mut pos = 0usize; // 已消费的原文边界 = 下一个搜索起点
    while let Some(hit) = find_next(text.as_bytes(), &fq, case_sensitive, pos) {
        out.push_str(&text[pos..hit]);
        out.push_str(replacement);
        pos = hit + fq.len();
        count += 1;
    }
    out.push_str(&text[pos..]);
    (out, count)
}

/// 在 [`Document`]（rope）上直接全部替换（P11/P19 协同项）。
///
/// 与 `replace_all(&doc.to_text(), ..)` 相比省掉整份全文 String 拷贝：
/// 按存储块零拷贝迭代、流式写入输出串，峰值内存 ≈ 输出文本自身 +
/// 一个小于查询长度的跨块残段缓冲。匹配语义与 [`replace_all`] 完全
/// 一致；空查询返回 (全文, 0)，保持 no-op 约定。
pub fn replace_all_document(
    doc: &Document,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if query.is_empty() {
        let mut whole = String::new();
        for chunk in doc.chunks() {
            whole.push_str(chunk);
        }
        return (whole, 0);
    }
    let fq: Vec<u8> = query.bytes().map(|b| fold_byte(b, case_sensitive)).collect();
    let mut out = String::new();
    // 跨块残段：块边界可能落在任意位置，末尾不足一个查询长度的尾巴
    // 先攒着，与下一块拼接后再扫（复用分配，峰值 ≈ 存储块 + 查询长度）
    let mut carry = String::new();
    let mut count = 0usize;
    for chunk in doc.chunks() {
        carry.push_str(chunk);
        count += drain_matches(&mut carry, &mut out, &fq, replacement, case_sensitive, false);
    }
    count += drain_matches(&mut carry, &mut out, &fq, replacement, case_sensitive, true);
    (out, count)
}

/// 把 `carry` 里能确定的扫描结果冲进 `out`（流式替换的内核）。
///
/// * `final_pass=false`：块迭代进行中——末尾不足一个查询长度的残段
///   可能与下一块拼出跨块命中，保留在 `carry` 里等待后续数据；
/// * `final_pass=true`：数据到齐，冲刷全部剩余内容并清空 `carry`。
///
/// 返回本次确认的替换次数。ropey 的块边界必落在字符边界上，但本函数
/// 的游标按字节推进，切分点需回退对齐到字符边界再分家。
fn drain_matches(
    carry: &mut String,
    out: &mut String,
    fq: &[u8],
    replacement: &str,
    case_sensitive: bool,
    final_pass: bool,
) -> usize {
    let mut count = 0usize;
    let mut consumed = 0usize; // carry 中已确认处理完的边界（恒为字符边界）
    while let Some(hit) = find_next(carry.as_bytes(), fq, case_sensitive, consumed) {
        out.push_str(&carry[consumed..hit]);
        out.push_str(replacement);
        consumed = hit + fq.len();
        count += 1;
    }
    if final_pass {
        out.push_str(&carry[consumed..]);
        carry.clear();
        return count;
    }
    // 末尾不足一个查询长度的残段留给下一轮；其余已确认无命中的原文直接冲走
    let mut keep_from = consumed.max(carry.len().saturating_sub(fq.len() - 1));
    while keep_from > consumed && !carry.is_char_boundary(keep_from) {
        keep_from -= 1;
    }
    if keep_from > consumed {
        out.push_str(&carry[consumed..keep_from]);
    }
    // 就地丢弃已确认前缀（尾部 memmove，不重新分配）
    carry.replace_range(..keep_from, "");
    count
}

// ---------- P70 正则查找/替换 ----------

/// 编译正则（P70）：fancy-regex（回溯引擎，支持环视/反向引用等
/// 主流编辑器级能力；病态模式可能慢——app 侧有后台扫描+防抖兜底）。
/// 大小写不敏感经 `(?i)` 前缀注入。错误信息原样返回（含模式错误
/// 位置），调用方直接展示。
pub fn compile_regex(pattern: &str, case_sensitive: bool) -> Result<fancy_regex::Regex, String> {
    let wrapped = if case_sensitive {
        pattern.to_owned()
    } else {
        format!("(?i){pattern}")
    };
    fancy_regex::Regex::new(&wrapped).map_err(|e| e.to_string())
}

/// P70：正则查找——对全文单遍扫描（`text` 为 `doc.to_text()` 全文，
/// 调用方负责在后台线程产生）。命中跨度换算为
/// [`MatchPos`]（line/col/len_chars），行尾 `\r\n` 计 1 字符（与 P26
/// 选区跨度口径一致，跨行命中可直接喂 `select_span`）。
pub fn find_all_regex(
    text: &str,
    pattern: &str,
    case_sensitive: bool,
) -> Result<Vec<MatchPos>, String> {
    let re = compile_regex(pattern, case_sensitive)?;
    let mut spans = Vec::new();
    for m in re.find_iter(text) {
        let m = m.map_err(|e| e.to_string())?;
        spans.push((m.start(), m.end()));
    }
    Ok(spans_to_matchpos(text, &spans))
}

/// P70：正则替换——`replacement` 用 fancy-regex 语法（`$1`/`${1}` 组
/// 引用，`$$` 为字面 `$；与字面模式的 `\n` 转义不同语法，UI 需提示）。
/// 返回 `(新文本, 替换次数)`。
pub fn replace_all_regex(
    text: &str,
    pattern: &str,
    replacement: &str,
    case_sensitive: bool,
) -> Result<(String, usize), String> {
    let re = compile_regex(pattern, case_sensitive)?;
    // 计数走错误传播：find_iter 的运行期错误不得静默吞掉（否则计数
    // 为 0 时会误报「无替换」），也不得与替换趟的结果口径不一致。
    let mut count = 0usize;
    for m in re.find_iter(text) {
        m.map_err(|e| e.to_string())?;
        count += 1;
    }
    if count == 0 {
        return Ok((text.to_owned(), 0));
    }
    // 不可用 replace_all：其内部 unwrap 会把运行期错误（如回溯超限）
    // 直接 panic（UI 线程调用即整应用崩溃），必须走 try_replacen 上抛。
    let out = re
        .try_replacen(text, 0, replacement)
        .map_err(|e| e.to_string())?
        .into_owned();
    Ok((out, count))
}

/// 字节跨度序列 → [`MatchPos`]：单遍游标推进（避免每命中一次
/// `text[..start].chars().count()` 的 O(命中×文档) 复杂度）。
/// `\r\n` 计 1 字符；命中内容内的换行推进行号、列号回到行首计数。
fn spans_to_matchpos(text: &str, spans: &[(usize, usize)]) -> Vec<MatchPos> {
    let mut out = Vec::with_capacity(spans.len());
    let mut byte_pos = 0usize;
    let mut char_pos = 0usize;
    let mut line = 0usize;
    let mut line_start_char = 0usize;

    let advance_to = |text: &str, byte_pos: &mut usize, char_pos: &mut usize,
                          line: &mut usize, line_start_char: &mut usize, target: usize| {
        while *byte_pos < target {
            let c = text[*byte_pos..].chars().next().unwrap_or('\0');
            *byte_pos += c.len_utf8();
            if c == '\r' && text[*byte_pos..].starts_with('\n') {
                *byte_pos += 1;
                *char_pos += 1;
                *line += 1;
                *line_start_char = *char_pos;
                continue;
            }
            *char_pos += 1;
            if c == '\n' {
                *line += 1;
                *line_start_char = *char_pos;
            }
        }
    };

    for &(start, end) in spans {
        advance_to(text, &mut byte_pos, &mut char_pos, &mut line, &mut line_start_char, start);
        let col = char_pos - line_start_char;
        // 命中跨度：同口径计数（\r\n 计 1）
        let mut len_chars = 0usize;
        let mut b = start;
        while b < end {
            let c = text[b..].chars().next().unwrap_or('\0');
            b += c.len_utf8();
            if c == '\r' && text[b..].starts_with('\n') {
                b += 1;
            }
            len_chars += 1;
        }
        out.push(MatchPos { line, col, len_chars });
        advance_to(text, &mut byte_pos, &mut char_pos, &mut line, &mut line_start_char, end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- P15：唯一大小写折叠实现 ----------

    #[test]
    fn ascii_case_eq_folds_only_ascii() {
        // ASCII 折叠生效
        assert!(ascii_case_eq('a', 'A', false));
        assert!(ascii_case_eq('Z', 'z', false));
        assert!(!ascii_case_eq('a', 'B', false));
        // 区分大小写时退化为精确比较
        assert!(!ascii_case_eq('a', 'A', true));
        assert!(ascii_case_eq('a', 'a', true));

        // 非 ASCII 永不折叠（土耳其 İ、希腊 Σ、德语 ß 都不算同一字符）
        assert!(!ascii_case_eq('\u{130}', 'i', false)); // İ vs i
        assert!(!ascii_case_eq('\u{3A3}', '\u{3C2}', false)); // Σ vs ς
        assert!(!ascii_case_eq('ß', 's', false));
        // 中文无大小写概念，始终精确
        assert!(ascii_case_eq('中', '中', false));
        assert!(!ascii_case_eq('中', '文', false));
    }

    #[test]
    fn find_all_basic_and_case_folding() {
        let text = "ab AB ab\nbaba\n";
        // 区分大小写："ab" → 第0行 col 0、col 6；第1行 "baba" 中 col 1
        assert_eq!(
            find_all(text, "ab", true),
            vec![
                MatchPos { line: 0, col: 0, len_chars: 2 },
                MatchPos { line: 0, col: 6, len_chars: 2 },
                MatchPos { line: 1, col: 1, len_chars: 2 },
            ]
        );
        // 不区分大小写：第0行多出 col 3（"AB"）
        assert_eq!(
            find_all(text, "ab", false),
            vec![
                MatchPos { line: 0, col: 0, len_chars: 2 },
                MatchPos { line: 0, col: 3, len_chars: 2 },
                MatchPos { line: 0, col: 6, len_chars: 2 },
                MatchPos { line: 1, col: 1, len_chars: 2 },
            ]
        );
    }

    #[test]
    fn find_chinese_and_empty_query() {
        let text = "你好，世界\n你好\n";
        assert_eq!(
            find_all(text, "你好", true),
            vec![
                MatchPos { line: 0, col: 0, len_chars: 2 },
                MatchPos { line: 1, col: 0, len_chars: 2 },
            ]
        );
        assert!(find_all(text, "", true).is_empty());
        assert!(find_all(text, "不存在", true).is_empty());
    }

    #[test]
    fn next_prev_wrap_around_cursor() {
        let ms = find_all("aXaXaX", "a", true);
        // 光标在 col 1 → 下一个是 col 2；上一个回卷到最后
        assert_eq!(next_from(&ms, 0, 1), Some(1));
        assert_eq!(prev_from(&ms, 0, 1), Some(0));
        // 光标越过最后一个 → 向下回卷到第一个
        assert_eq!(next_from(&ms, 0, 5), Some(0));
        // 光标在最前 → 向上回卷到最后一个
        assert_eq!(prev_from(&ms, 0, 0), Some(2));
        assert_eq!(next_from(&[], 0, 0), None);
    }

    #[test]
    fn replace_all_counts_and_preserves_rest() {
        let (out, n) = replace_all("aa bb aa", "AA", "c", false);
        assert_eq!(out, "c bb c");
        assert_eq!(n, 2);

        let (out, n) = replace_all("中文内容", "内容", "text", true);
        assert_eq!(out, "中文text");
        assert_eq!(n, 1);

        let (out, n) = replace_all("abc", "abcd", "x", true);
        assert_eq!(out, "abc");
        assert_eq!(n, 0);

        // 空查询是 no-op
        let (out, n) = replace_all("abc", "", "x", true);
        assert_eq!(out, "abc");
        assert_eq!(n, 0);
    }

    // ---------- P10：rope 直查与 find_all 逐字节对拍 ----------

    /// 确定性伪随机串（小字母表 + 换行 + CRLF + 多字节字符），
    /// 保证两条路径在跨块边界、行尾形态各异的输入上完全一致。
    fn pseudo_random_text(seed: u64, len: usize) -> String {
        const ALPHABET: [char; 8] = ['a', 'b', 'c', '\n', '\r', '中', '🚀', 'x'];
        let mut state = seed | 1;
        let mut out = String::with_capacity(len * 4);
        for _ in 0..len {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            out.push(ALPHABET[(state >> 33) as usize % ALPHABET.len()]);
        }
        out
    }

    #[test]
    fn find_all_document_matches_find_all_exactly() {
        let fixtures = [
            "",                                  // 空文档
            "single line no newline",            // 单行无换行
            "foo\nbar foo\n",                    // 常规多行（尾换行）
            "a\r\nb\r\nc",                       // CRLF：\r 留在行内，列语义同 split('\n')
            "x\r\ny\nz\rw",                      // 混合 + 孤立 \r（不作行界）
            "中文中文\n🚀🚀中\n",                 // 多字节字符列号
            "\tindent\ttab\t\n",                 // Tab 原样计数
            "aaaa aa\naa",                       // 重叠命中窗口
        ];
        for text in fixtures {
            let doc = Document::from_str(text);
            // P26：查询清单补入跨行查询（含首尾换行、纯换行、含 \r 的永不命中例）
            for query in ["a", "aa", "foo", "中", "🚀x", "\r", "\r\n", "zz", "\n", "\nfoo", "foo\nbar", "a\nb", "\n\n", "a\r\nb"] {
                for cs in [true, false] {
                    assert_eq!(
                        find_all_document(&doc, query, cs),
                        find_all(text, query, cs),
                        "不一致: text={text:?} query={query:?} cs={cs}"
                    );
                }
            }
        }
    }

    #[test]
    fn find_all_document_matches_find_all_on_random_texts() {
        for seed in [1u64, 0xDEAD_BEEF, 12345] {
            let text = pseudo_random_text(seed, 20_000);
            let doc = Document::from_str(&text);
            // P26：随机文本对拍补跨行查询——字母表自带 \n 与孤立 \r，
            // 命中会落在各种行尾形态与块相位上
            for query in ["a", "ab", "c\n", "中", "xx", "a\r", "\n", "a\nb", "\na", "a\na"] {
                assert_eq!(
                    find_all_document(&doc, query, false),
                    find_all(&text, query, false),
                    "seed={seed} query={query:?}"
                );
            }
        }
    }

    #[test]
    fn find_all_document_handles_long_lines_and_empty_query() {
        // 超长单行（> ropey 默认块大小若干倍），覆盖块边界残段累积逻辑
        let long = format!("{}needle{}\n", "x".repeat(100_000), "y".repeat(50_000));
        let doc = Document::from_str(&long);
        assert_eq!(
            find_all_document(&doc, "needle", true),
            vec![MatchPos { line: 0, col: 100_000, len_chars: 6 }]
        );
        // 空查询约定：返回空表
        assert!(find_all_document(&doc, "", true).is_empty());
    }

    #[test]
    fn find_all_reuses_line_char_buffer_across_lines() {
        // 回归第 39 轮缓冲复用优化：跨行长短交替 + 空行 + 多字节行，
        // 若复用缓冲残留上一行状态（clear 缺失/顺序错乱）会当场断言失败；
        // 两条路径共享同一 scan_line，钉住精确语义即可双向覆盖
        let text = "the quick brown fox jumps over the lazy dog\n短行 the\n\nthe end 🚀\nno match here\nthe";
        let want = vec![
            MatchPos { line: 0, col: 0, len_chars: 3 },
            MatchPos { line: 0, col: 31, len_chars: 3 },
            MatchPos { line: 1, col: 3, len_chars: 3 },
            MatchPos { line: 3, col: 0, len_chars: 3 },
            MatchPos { line: 5, col: 0, len_chars: 3 },
        ];
        assert_eq!(find_all(text, "the", true), want);
        let doc = Document::from_str(text);
        assert_eq!(find_all_document(&doc, "the", true), want);
    }

    // ---------- P26：跨行查询 ----------

    #[test]
    fn multiline_query_hits_on_every_eol_style() {
        // 同一查询在三种行尾形态的文档上都应命中同一位置：
        // 查询的字面 \n 与文本侧 \n / \r\n / 孤立 \r 全部判等
        for text in ["x a\nb y", "x a\r\nb y", "x a\rb y"] {
            let hits = find_all(text, "a\nb", true);
            assert_eq!(
                hits,
                vec![MatchPos { line: 0, col: 2, len_chars: 3 }],
                "text={text:?}"
            );
            let doc = Document::from_str(text);
            assert_eq!(find_all_document(&doc, "a\nb", true), hits, "rope 路径同结果");
        }
    }

    #[test]
    fn multiline_match_positions_follow_crossing_semantics() {
        // 命中起点行号按换行单元推进：跨过孤立 \r 也算一行（与 ropey
        // 默认 unicode_lines 行界一致，select_span 可直接还原选区）
        let text = "one\ntwo\rthree\n";
        assert_eq!(Document::from_str(text).line_count(), 4, "\\r 应计作行界");
        let hits = find_all(text, "two\nthree", true);
        assert_eq!(hits, vec![MatchPos { line: 1, col: 0, len_chars: 9 }]);
        let doc = Document::from_str(text);
        assert_eq!(find_all_document(&doc, "two\nthree", true), hits);
    }

    #[test]
    fn multiline_len_chars_is_display_span() {
        // CRLF 文档上原始字符跨度是 4（a,\r,\n,b），显示跨度只记 3：
        // 行尾 \r 不计、每跨一行计 1。用行列换算验证「选区还原」：
        // 起点 + 显示跨度走线得到的切片恰为匹配文本本身
        let doc = Document::from_str("xx a\r\nb yy");
        let hit = find_all_document(&doc, "a\nb", true);
        assert_eq!(hit, vec![MatchPos { line: 0, col: 3, len_chars: 3 }]);

        // 模拟 select_span 的走线：显示跨度 3 = 行内 1 字符 + 1 次跨行 + 行内 1 字符
        let start = doc.line_to_char(hit[0].line) + hit[0].col;
        let end = doc.line_to_char(hit[0].line + 1) + 1;
        assert_eq!(doc.slice_text(start, end), "a\r\nb");
    }

    #[test]
    fn multiline_query_starting_with_newline_covers_line_break() {
        // 以 \n 开头的查询：命中点挂在上一行行末列，选区恰好覆盖换行符
        let hits = find_all("x\nb", "\nb", true);
        assert_eq!(hits, vec![MatchPos { line: 0, col: 1, len_chars: 2 }]);
        // 查询就是裸 \n：文档里每个行界各得一个命中
        assert_eq!(
            find_all("a\nb\n", "\n", true),
            vec![
                MatchPos { line: 0, col: 1, len_chars: 1 },
                MatchPos { line: 1, col: 1, len_chars: 1 },
            ]
        );
    }

    #[test]
    fn multiline_query_with_literal_cr_never_matches() {
        // 含字面 \r 且含 \n 的查询走归一分支：文本侧不再产出 \r token，
        // 故永不命中（相比旧实现「含 \n 恒零命中」无回归，纯增量）
        assert!(find_all("a\r\nb", "a\r\nb", true).is_empty());
        assert!(find_all_document(&Document::from_str("a\r\nb"), "a\r\nb", true).is_empty());

        // 对照：不含 \n 的查询仍走旧单行路径——行内字面 \r 照常命中，
        // 单行路径行为零变化
        assert_eq!(
            find_all("x\ry", "x\ry", true),
            vec![MatchPos { line: 0, col: 0, len_chars: 3 }]
        );
    }

    #[test]
    fn multiline_overlapping_matches_are_reported_per_position() {
        // 重叠契约与单行路径一致：每个可能补全命中的窗口终点都独立判定
        let hits = find_all("a\na\na", "a\na", true);
        assert_eq!(
            hits,
            vec![
                MatchPos { line: 0, col: 0, len_chars: 3 },
                MatchPos { line: 1, col: 0, len_chars: 3 },
            ]
        );
        assert_eq!(find_all_document(&Document::from_str("a\na\na"), "a\na", true), hits);
    }

    #[test]
    fn multiline_edges_return_no_matches_without_panicking() {
        // 查询长于文本 / 文本没有任何换行：零命中
        assert!(find_all("ab", "a\nb", true).is_empty());
        assert!(find_all("abc", "\n", true).is_empty());
        assert!(find_all_document(&Document::from_str("abc"), "a\nb", true).is_empty());
        // 大小写折叠照常作用于普通字符（\n 自身无大小写概念）
        assert_eq!(
            find_all("A\r\nB", "a\nb", false),
            vec![MatchPos { line: 0, col: 0, len_chars: 3 }]
        );
    }

    #[test]
    fn scanner_fragment_boundaries_agree_with_single_push() {
        // 片段边界穷举对拍：任意切分位置的多次推送必须与一次整段推送、
        // 以及 find_all 参照实现三者一致。重点覆盖边界落在 \r\n 中间、
        // 片段以孤立 \r 结尾等悬置裁决路径（ropey 块边界虽保证不切 CRLF，
        // 本契约让扫描器不依赖该善意）。
        let texts = [
            "a\r\nb\rc\n\rd",
            "x\r\n\r\ny",
            "end\r",
            "pre\r\r\npost",
            "\r\n\r\n\r",
            "中\n\r🚀\r\n文",
        ];
        let queries: Vec<Vec<char>> = ["a\nb", "\n", "b\nc", "a\na", "中\n"]
            .iter()
            .map(|q| q.chars().collect())
            .collect();
        for text in texts {
            for q in &queries {
                // 参照一：一次整段推送
                let mut whole_out = Vec::new();
                let mut whole = MultiLineScanner::new(q, false);
                whole.push_str(text, &mut whole_out);
                whole.finish(&mut whole_out);

                // 参照二：逐字符碎片化推送（极端块相位）
                let mut frag_out = Vec::new();
                let mut frag = MultiLineScanner::new(q, false);
                for c in text.chars() {
                    let mut buf = [0u8; 4];
                    frag.push_str(c.encode_utf8(&mut buf), &mut frag_out);
                }
                frag.finish(&mut frag_out);

                // 地面真值：find_all 参照路径
                let truth = find_all(text, &q.iter().collect::<String>(), false);

                assert_eq!(whole_out, truth, "text={text:?} q={:?} 整段推送偏离参照", String::from_iter(q.iter()));
                assert_eq!(frag_out, truth, "text={text:?} q={:?} 碎片化推送偏离参照", String::from_iter(q.iter()));
            }
        }
    }

    // ---------- P11：字节级替换与 rope 流式替换 ----------

    /// 旧实现的等价参照：P11 之前的 Vec<char> 逐字符算法，用于对拍。
    /// （4 字节/char，50MB 文档峰值 ≈ 原文 ×5——正是本次重写要消掉的。）
    fn replace_all_vecchar_reference(
        text: &str,
        query: &str,
        replacement: &str,
        case_sensitive: bool,
    ) -> (String, usize) {
        if query.is_empty() {
            return (text.to_owned(), 0);
        }
        let t: Vec<char> = text.chars().collect();
        let q: Vec<char> = query.chars().collect();
        if q.len() > t.len() {
            return (text.to_owned(), 0);
        }
        let mut out = String::new();
        let mut i = 0usize;
        let mut count = 0usize;
        while i <= t.len() - q.len() {
            if (0..q.len()).all(|k| ascii_case_eq(q[k], t[i + k], case_sensitive)) {
                out.push_str(replacement);
                i += q.len();
                count += 1;
            } else {
                out.push(t[i]);
                i += 1;
            }
        }
        out.extend(t[i.min(t.len())..].iter());
        (out, count)
    }

    #[test]
    fn replace_all_matches_vecchar_reference_on_fixtures() {
        // 覆盖：多字节字符、4 字节 emoji、CRLF/孤立 CR、Tab、大小写折叠、
        // 查询比原文长、空替换、替换文本含查询本身、命中首尾相邻
        let fixtures = [
            "",
            "aaa",
            "aaaa",
            "ab AB ab\nbaba\n",
            "中文ABC中文abc",
            "🚀🚀x🚀",
            "a\r\nb\r\nc\r\n",
            "x\r\ny\nz\rw",
            "\ttab\tsep\t\n",
            "abcabcabc",
            "needleatstartneedleatend",
        ];
        for text in fixtures {
            for query in ["a", "aa", "ab", "中", "中文", "🚀", "\r", "\r\n", "needle", "zz"] {
                for replacement in ["", "X", "XY长", "ab"] {
                    for cs in [true, false] {
                        assert_eq!(
                            replace_all(text, query, replacement, cs),
                            replace_all_vecchar_reference(text, query, replacement, cs),
                            "不一致: text={text:?} query={query:?} repl={replacement:?} cs={cs}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn replace_all_greedy_leftmost_semantics_are_pinned() {
        // 最左优先 + 命中后整体跳过查询长度（不重叠）：
        // "aaaa" 替换 "aa" → 两处；"aaa" → 一处 + 尾部残余
        assert_eq!(replace_all("aaaa", "aa", "b", true), ("bb".to_owned(), 2));
        assert_eq!(replace_all("aaa", "aa", "b", true), ("ba".to_owned(), 1));

        // 多字节字符不被切坏：emoji 与中文夹着的命中原样保留其余字符
        assert_eq!(replace_all("🚀中🚀", "中", "", true), ("🚀🚀".to_owned(), 1));
        assert_eq!(
            replace_all("中文内容", "内容", "text", true),
            ("中文text".to_owned(), 1)
        );

        // 不区分大小写时 ASCII 折叠、非 ASCII 精确：İ(U+0130) 与 ı(U+0131)
        // 都不折叠为 i，只有 'i' 和 'I' 两处命中
        assert_eq!(replace_all("İiIı", "i", "X", false), ("İXXı".to_owned(), 2));
    }

    #[test]
    fn replace_all_document_matches_replace_all_exactly() {
        let fixtures = [
            "",
            "single line no newline",
            "foo bar foo\nbar foo\n",
            "a\r\nb\r\nc",
            "x\r\ny\nz\rw",
            "中文中文\n🚀🚀中\n",
            "aaaa aa\naa",
        ];
        for text in fixtures {
            let doc = Document::from_str(text);
            for query in ["a", "aa", "foo", "中", "🚀", "\r\n", "zz"] {
                for cs in [true, false] {
                    assert_eq!(
                        replace_all_document(&doc, query, "X", cs),
                        replace_all(text, query, "X", cs),
                        "不一致: text={text:?} query={query:?} cs={cs}"
                    );
                }
            }
        }
        // 空查询 no-op：返回全文与 0 次
        let doc = Document::from_str("hello\nworld");
        assert_eq!(replace_all_document(&doc, "", "X", true), (doc.to_text(), 0));
    }

    #[test]
    fn replace_all_document_matches_on_random_texts() {
        for seed in [2u64, 0xFEED_FACE, 98765] {
            let text = pseudo_random_text(seed, 20_000);
            let doc = Document::from_str(&text);
            for query in ["a", "ab", "c\n", "中", "xx", "a\r", "🚀"] {
                let (streamed, n_streamed) = replace_all_document(&doc, query, "<R>", false);
                let (plain, n_plain) = replace_all(&text, query, "<R>", false);
                assert_eq!(streamed, plain, "seed={seed} query={query:?}");
                assert_eq!(n_streamed, n_plain);
            }
        }
    }

    #[test]
    fn replace_all_document_hits_across_chunk_boundaries() {
        // 单行远超 ropey 存储块：把命中串放到一串不同偏移上，
        // 无论块边界落在命中的哪个位置（前缀/中间/后缀跨块）都必须命中
        let needle = "needle";
        let total = 60_000usize;
        let mut offset = 0usize;
        while offset + needle.len() <= total - 100 {
            let mut text = String::with_capacity(total);
            text.push_str(&"x".repeat(offset));
            text.push_str(needle);
            text.push_str(&"y".repeat(total - offset - needle.len()));
            let doc = Document::from_str(&text);
            let (out, n) = replace_all_document(&doc, needle, "NEEDLE", true);
            assert_eq!(n, 1, "offset={offset} 应恰有一处命中");
            let mut expected = String::with_capacity(total);
            expected.push_str(&"x".repeat(offset));
            expected.push_str("NEEDLE");
            expected.push_str(&"y".repeat(total - offset - needle.len()));
            assert_eq!(out, expected, "offset={offset} 替换结果不一致");
            offset += 511; // 步长取奇数，覆盖相对块边界的全部相位
        }
    }

    #[test]
    fn replace_all_document_query_longer_than_chunk_still_works() {
        // 查询比单个存储块还长的极端情况：残段缓冲必须能攒到完整窗口
        let query = "Q".repeat(9_000);
        let mut text = "a".repeat(20_000);
        text.push_str(&query);
        text.push_str(&"b".repeat(20_000));
        let doc = Document::from_str(&text);
        let (out, n) = replace_all_document(&doc, &query, "HIT", true);
        assert_eq!(n, 1);
        assert_eq!(out, format!("{}HIT{}", "a".repeat(20_000), "b".repeat(20_000)));
    }

    // ---------- P70 正则查找/替换 ----------

    #[test]
    fn regex_find_maps_line_col_and_multiline_span() {
        // CRLF 文档：命中含跨行（\r\n 计 1 字符，P26 选区跨度口径）
        let text = "foo 1\r\nbar 22\r\nfoo 333";
        let hits = find_all_regex(text, r"\d+", false).expect("合法模式");
        assert_eq!(
            hits,
            vec![
                MatchPos { line: 0, col: 4, len_chars: 1 },
                MatchPos { line: 1, col: 4, len_chars: 2 },
                MatchPos { line: 2, col: 4, len_chars: 3 },
            ]
        );

        // 跨行命中：\r\n 计 1，行号推进、列号回行首
        // （'1' + '\r\n'单元 + 'bar' = 5 字符）
        let hits = find_all_regex(text, r"1\r\nbar", false).expect("合法模式");
        assert_eq!(
            hits,
            vec![MatchPos { line: 0, col: 4, len_chars: 5 }]
        );
    }

    #[test]
    fn regex_case_flag_and_invalid_pattern() {
        let text = "Foo foo FOO";
        assert_eq!(find_all_regex(text, "foo", false).unwrap().len(), 3);
        assert_eq!(find_all_regex(text, "foo", true).unwrap().len(), 1);

        // 非法模式 → Err 带引擎错误信息（调用方直接展示）
        let err = find_all_regex(text, "(unclosed", true).unwrap_err();
        assert!(!err.is_empty());

        assert!(compile_regex("(?i)ok", true).is_ok());
    }

    #[test]
    fn regex_replace_expands_capture_groups() {
        // $1 组引用：日期重排
        let text = "2026-08-25 2026-01-02";
        let (out, n) =
            replace_all_regex(text, r"(\d{4})-(\d{2})-(\d{2})", "$3/$2/$1", true).unwrap();
        assert_eq!(out, "25/08/2026 02/01/2026");
        assert_eq!(n, 2);

        // 零命中：原文返回、次数 0
        let (out, n) = replace_all_regex(text, "zzz", "x", true).unwrap();
        assert_eq!(n, 0);
        assert_eq!(out, text);

        // 非法模式 → Err
        assert!(replace_all_regex(text, "[", "x", true).is_err());
    }

    #[test]
    fn regex_replace_propagates_runtime_errors_instead_of_panic() {
        // fancy-regex 存在编译期拦不住的运行期错误（回溯超限，默认上限
        // 100 万步）。替换路径必须上抛 Err 而非 panic——旧实现走
        // replace_all（内部 unwrap），在 UI 线程调用即整应用崩溃。
        // 触发构造取自 fancy-regex 官方超限用例的模式加长文本：歧义
        // 分割（a/b/ab）无法被引擎专项优化消除，120 字节即确定性触限
        // （实测约 17ms，耗时被封顶）。
        let text = "ab".repeat(60);
        let err = replace_all_regex(&text, "(a|b|ab)*(?>c)", "x", true).unwrap_err();
        assert!(!err.is_empty());
        // 查找路径同一口径：运行期错误同样上抛，不静默吞
        assert!(find_all_regex(&text, "(a|b|ab)*(?>c)", true).is_err());
    }

    #[test]
    fn regex_multiline_mode_documented_via_inline_flags() {
        // ^ $ 默认只锚文本首尾；逐行锚定用 (?m) 内联标志（UI 提示口径）
        let text = "a1\nb2\na3";
        assert_eq!(find_all_regex(text, "^a.", false).unwrap().len(), 1);
        assert_eq!(find_all_regex(text, "(?m)^a.", false).unwrap().len(), 2);
    }
}
