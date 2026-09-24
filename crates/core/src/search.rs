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

/// 整词判定的词字符：Unicode 字母/数字 + 下划线（`\w` 的宽松近似；
/// CJK 连续段落同属一个词，与主流编辑器的整词口径一致）。
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    ascii_case_eq(a, b, case_sensitive)
}

// ---------- 行界口径（查找 ↔ ropey 对齐） ----------

/// ropey `unicode_lines` 行界全集的判定：`c` 构成的行界共几字节
/// （CRLF 配对算一个单元计 2 字节），非行界返回 0。
///
/// 这是查找模块与编辑器（ropey 默认 `unicode_lines` 特性）行号坐标的
/// 唯一对齐点：`\n`、`\r\n`、孤立 `\r`、VT、FF、NEL、LS、PS 一律算
/// 行界。旧实现只认 `\n`，在含孤立 `\r` 的文档上（加载器明确支持的
/// 经典 Mac CR 文件）所有命中的行号都会漂移、选区定位错位。
fn line_break_byte_len(c: char, next: Option<char>) -> usize {
    let is_break = matches!(
        c,
        '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    );
    if !is_break {
        return 0;
    }
    let mut len = c.len_utf8();
    if c == '\r' && next == Some('\n') {
        len += '\n'.len_utf8();
    }
    len
}

/// 按 ropey 行界全集把 `text` 切成行并逐行回调（行内容不含行界）。
///
/// 公开形态不变（app 层在用）。需要在命中数封顶时早退的调用方走
/// `for_each_line_until`——切行逻辑仅此一份，两者不可能口径漂移。
pub fn for_each_line(text: &str, mut f: impl FnMut(usize, &str)) {
    let _ = for_each_line_until(text, |line_idx, line| {
        f(line_idx, line);
        std::ops::ControlFlow::Continue(())
    });
}

/// [`for_each_line`] 的可早退版：回调返回 [`std::ops::ControlFlow::Break`]
/// 时立刻停止切行与后续字符扫描（O-13「命中取满即收口」的基础设施）。
fn for_each_line_until(
    text: &str,
    mut f: impl FnMut(usize, &str) -> std::ops::ControlFlow<()>,
) -> std::ops::ControlFlow<()> {
    let mut line_start = 0usize;
    let mut line_idx = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().copied().map(|(_, nc)| nc);
        let blen = line_break_byte_len(c, next);
        if blen == 0 {
            continue;
        }
        f(line_idx, &text[line_start..i])?;
        line_idx += 1;
        line_start = i + blen;
        if c == '\r' && next == Some('\n') {
            // 消费 CRLF 配对的 \n。**按字符身份判，不按宽度**：旧判据写成
            // `blen == 2`，而 NEL(U+0085) 本身就是 2 字节 → 「NEL 后紧跟一个
            // 字符」时那个字符被无条件吃掉，NEL+\n 并成一行（行内容里还留着
            // 裸 \n），此后所有行号左移一位（P217）。
            chars.next();
        }
    }
    f(line_idx, &text[line_start..])
}

/// 在片段内找第一个「完整落在片段内」的行界，返回 (起始字节偏移, 字节数)。
///
/// 片段末尾悬置的 `\r` 不在此返回——它可能是跨块 `\r\n` 的前半，
/// 由调用方按 [`MultiLineScanner::pending_cr`] 同款逻辑裁决。
fn next_line_break(s: &str) -> Option<(usize, usize)> {
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().copied().map(|(_, nc)| nc);
        let blen = line_break_byte_len(c, next);
        if blen == 0 {
            continue;
        }
        if c == '\r' && next.is_none() {
            return None; // 片段末尾的 \r：跨块归并悬置
        }
        return Some((i, blen));
    }
    None
}

/// 找出全部匹配（按文档顺序）。查询串为空时返回空表。
///
/// P26：查询含 `\n` 时走 `MultiLineScanner` 跨行归一分支——
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
    // O-11：查询备好一次，逐行只碰字节（旧版此处是跨行复用的 Vec<char>）
    let lq = LiteralQuery::new(query, case_sensitive);
    for_each_line(text, |line_idx, line| {
        scan_line(line, &lq, line_idx, &mut out, usize::MAX);
    });
    out
}

/// 字面查找的**封顶**版：取满 `limit` 个命中即刻早退（O-13）。
///
/// 结果恒等于 `find_all(text, query, case_sensitive)` 的前 `limit` 个——
/// 旧做法「全收进 Vec 再 truncate」在 64MB 文件搜 `the` 要先收约 100 万
/// 个 `MatchPos`（24B/个）再丢弃，且剩余文本继续白扫。
///
/// 仅覆盖**不含换行的字面查询**：跨行查询走 [`find_all`] 的
/// [`MultiLineScanner`] 分支（该内核不接封顶参数，保持全量后截断），
/// 结果仍与 `find_all(..).take(limit)` 相等。`limit == 0` 直接空表。
pub(crate) fn find_all_limited(
    text: &str,
    query: &str,
    case_sensitive: bool,
    limit: usize,
) -> Vec<MatchPos> {
    let mut out = Vec::new();
    if query.is_empty() || limit == 0 {
        return out;
    }
    let q: Vec<char> = query.chars().collect();
    if q.contains(&'\n') {
        let mut all = find_all(text, query, case_sensitive);
        all.truncate(limit);
        return all;
    }
    let lq = LiteralQuery::new(query, case_sensitive);
    let _ = for_each_line_until(text, |line_idx, line| {
        scan_line(line, &lq, line_idx, &mut out, limit);
        if out.len() >= limit {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    });
    out
}

/// 在 [`Document`]（rope）上直接查找，语义与 [`find_all`] 完全一致（P10）。
///
/// 与 `find_all(&doc.to_text(), ..)` 相比省掉整份全文 String：
/// 按存储块零拷贝迭代、手工按行界全集分段，峰值内存只多一个「当前行」
/// 缓冲（复用分配，长度 = 最长行）。行界口径与 ropey 一致（含孤立
/// `\r` 等），与编辑器行号同源；`\r\n` 跨块悬置裁决保证逐字节等价。
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
    let lq = LiteralQuery::new(query, case_sensitive); // O-11：备好一次
    // 上块以 \r 结尾：下块若以 \n 开头则并入同一 CRLF 单元（不另起一行）
    let mut pending_cr = false;
    for chunk in doc.chunks() {
        let mut rest = chunk;
        if pending_cr {
            pending_cr = false;
            if let Some(stripped) = rest.strip_prefix('\n') {
                rest = stripped; // \r\n 跨块：\n 已随上块的 \r 一并结算
            }
        }
        // 块边界可能落在任意位置：行界前的残段累积进当前行缓冲，
        // 遇到完整行界才结算一行——保证与全文单遍切分逐字节等价
        while let Some((pos, blen)) = next_line_break(rest) {
            line.push_str(&rest[..pos]);
            scan_line(&line, &lq, line_idx, &mut out, usize::MAX);
            line.clear();
            line_idx += 1;
            rest = &rest[pos + blen..];
        }
        if rest.ends_with('\r') {
            // 悬置 \r 必是行界（孤立或 CRLF 均然）：行内容到此结算，
            // 是否吞掉下块开头的 \n 交 pending_cr 裁决
            line.push_str(&rest[..rest.len() - '\r'.len_utf8()]);
            scan_line(&line, &lq, line_idx, &mut out, usize::MAX);
            line.clear();
            line_idx += 1;
            pending_cr = true;
        } else {
            line.push_str(rest);
        }
    }
    scan_line(&line, &lq, line_idx, &mut out, usize::MAX);
    out
}

/// 整词边界判定的**唯一实现**（[`filter_whole_word`] 与其 `&str` 孪生
/// [`crate::find_in_files::filter_whole_word_text`] 共用，O-12 起结构上
/// 不可能再漂移）：命中起点前一字符与终点后一字符均非词字符才保留。
///
/// 三条边界口径由本函数钉住，两侧传入的 `content` 必须是**行内容字符**：
/// * `hit.col == 0` → 行首即边界；
/// * `end == content.len()` → 行尾即边界；
/// * `end > content.len()` → 跨行命中（`len_chars` 含行界单元，行内边界
///   语义不成立），一律保留。
pub(crate) fn whole_word_bounds_ok(content: &[char], hit: MatchPos) -> bool {
    let end = hit.col + hit.len_chars;
    if end > content.len() {
        return true; // 跨行命中：保留
    }
    let before_ok = hit.col == 0 || !is_word_char(content[hit.col - 1]);
    let after_ok = end == content.len() || !is_word_char(content[end]);
    before_ok && after_ok
}

#[cfg(test)]
thread_local! {
    /// 测试钩子（O-12 成本形状契约）：[`filter_whole_word`] **物化行内容缓冲的
    /// 次数**。耗时断言在本仓库不被接受（忙机器上既可能假绿也可能假红），而
    /// 「同一行只物化一次」本质是**次数**命题，与机器性能无关。
    /// 生产构建整字段不参与编译（与 app 层 `EditorCore::line_text_calls` 同形状）。
    static WHOLE_WORD_LINE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 物化第 `line` 行的「行内容字符」到 `buf`（行尾换行单元按原口径截断）。
///
/// 零拷贝读法：`line_to_char` + `chars_from` 走 rope 叶片迭代，
/// `take(line_len_chars)` 把范围钉回本行——不产生整行 `String`。
/// `take_while` 的判据与历史实现逐字相同（只截 `\n`/`\r`），故行内容为
/// VT/FF/NEL/LS/PS 结尾时这些字符**留在** `content` 里；两者都在
/// [`whole_word_bounds_ok`] 里得出同一结论（这些字符非词字符，
/// 「窥到它」与「视作行尾」判定一致），口径与孪生函数无分叉。
fn fill_line_chars(doc: &Document, line: usize, buf: &mut Vec<char>) {
    buf.clear();
    buf.extend(
        doc
            .chars_from(doc.line_to_char(line))
            .take(doc.line_len_chars(line))
            .take_while(|&c| c != '\n' && c != '\r'),
    );
    #[cfg(test)]
    WHOLE_WORD_LINE_BUILDS.with(|c| c.set(c.get() + 1));
}

/// 整词过滤：只保留命中起点前一字符与终点后一字符**均非词字符**的
/// 命中（行首/文档首与行尾/文档尾视为边界）。
///
/// 跨行命中的 `len_chars` 含行界单元，行内边界语义不成立，一律保留；
/// 正则模式不适用整词（边界语义由正则自身表达），调用方自行判定。
///
/// **成本口径（O-12）**：行内容缓冲**只在行号变化时物化一次**（缓冲复用），
/// 故成本是 O(命中所在行数 × 行长) 而非旧的 O(命中数 × 行长)——旧实现每个
/// 命中都 `line_str().chars().collect::<Vec<char>>()` 把整行物化两遍，
/// 20MB 单行里搜 `name` 即上万次整行拷贝。命中不要求有序：行号一变就重建，
/// 乱序输入结果与有序输入同口径（只是退化回逐命中成本）。
pub fn filter_whole_word(doc: &Document, hits: Vec<MatchPos>) -> Vec<MatchPos> {
    let mut out = Vec::with_capacity(hits.len());
    let mut cur_line = usize::MAX;
    let mut content: Vec<char> = Vec::new();
    for hit in hits {
        if hit.line != cur_line {
            cur_line = hit.line;
            fill_line_chars(doc, hit.line, &mut content);
        }
        if whole_word_bounds_ok(&content, hit) {
            out.push(hit);
        }
    }
    out
}

/// 单行窗口扫描：在 `line` 的字符序列上滑动长度 `q.len()` 的窗口逐一比较。
///
/// 字面查询的预处理产物（O-11）：一次备好，逐行复用。
///
/// `folded` 是**按大小写口径折叠后的查询字节**，直接喂 [`find_next`]；
/// `chars` 是查询字符数（= 命中的 `len_chars`，单行命中无换行跨越，显示跨度
/// 就等于它）。旧实现逐行把整行 `extend` 进 `Vec<char>`，一次查找等于把全文
/// 按 4 字节/字符搬一遍；本结构让扫描全程只碰字节。
struct LiteralQuery {
    /// 折叠后的查询字节（大小写敏感时即原始字节）
    folded: Vec<u8>,
    /// 查询字符数
    chars: usize,
    /// 大小写口径（透传给 [`find_next`] 的首字节过滤与逐字节比较）
    case_sensitive: bool,
}

impl LiteralQuery {
    fn new(query: &str, case_sensitive: bool) -> Self {
        Self {
            folded: query.as_bytes().iter().map(|&b| fold_byte(b, case_sensitive)).collect(),
            chars: query.chars().count(),
            case_sensitive,
        }
    }
}

/// 单行字面扫描：在 UTF-8 **字节**上滑动窗口，只在真正命中时才折算字符列号。
///
/// 正确性依据同 [`find_next`]：UTF-8 自同步性保证命中起点必落在字符边界
/// （续字节一律在 `0x80..0xC0`，等不上任何 ASCII 或前导字节的查询首字节），
/// 所以字节级匹配与逐字符匹配的命中集合完全相同。字符列号靠**游标推进**：
/// 命中按字节序递增，每次只把「上次结算点到本次命中点」的缺口折成字符数
/// （非续字节计数），故一行的折算总量 = 该行字节数，与命中数无关。
///
/// 命中后起点前进 **1 字节**而非查询长度：保留重叠命中语义（旧实现的
/// `for start in 0..=(len-q.len())` 天然重叠，`"aaa"` 搜 `"aa"` 得两处）。
///
/// `limit` 是 `out` 的**容量上限**（O-13）：写满即返回，故 `out` 长度恒
/// 不超过 limit（不是「先写满再截断」）。不限的调用方传 `usize::MAX`。
fn scan_line(
    line: &str,
    lq: &LiteralQuery,
    line_idx: usize,
    out: &mut Vec<MatchPos>,
    limit: usize,
) {
    let hay = line.as_bytes();
    let mut from = 0usize;
    let mut settled = 0usize; // 已折进 col 的字节前缀
    let mut col = 0usize;
    while let Some(i) = find_next(hay, &lq.folded, lq.case_sensitive, from) {
        for &b in &hay[settled..i] {
            // 续字节 (10xx_xxxx) 不起始新字符 ⇒ 非续字节数即字符数
            col += (b & 0xC0 != 0x80) as usize;
        }
        settled = i;
        if out.len() >= limit {
            return;
        }
        out.push(MatchPos { line: line_idx, col, len_chars: lq.chars });
        from = i + 1;
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
                // 行界全集（除 \r\n 外的单字符行界）：与编辑器行号同源
                '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                    self.push_token(ScanToken::Newline, out)
                }
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

/// 整词全部替换：与 [`replace_all`] 同语义，但命中起点前一字符与终点
/// 后一字符**均非词字符**才替换（行首/文档首与行尾/文档尾视为边界）。
/// 仅字面查询适用；正则模式的整词由模式自身表达。
///
/// 与流式 [`replace_all_document`] 不同，本函数工作在全文 `&str` 上——
/// 边界判定需要窥视命中两侧字符，调用方负责文档规模上限（与正则
/// 替换同款防冻结口径）。
pub fn replace_all_word(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if query.is_empty() {
        return (text.to_owned(), 0);
    }
    let fq: Vec<u8> = query.bytes().map(|b| fold_byte(b, case_sensitive)).collect();
    // 整词边界规则的行内口径由 [`whole_word_bounds_ok`] 钉住：**跨行命中一律
    // 保留**（行内边界语义不成立）。字面查询是否跨行只取决于查询自身含不含
    // 行界字符，故在此提前判定——P222 之前这里无条件按「命中两侧字符」判界，
    // 于是含换行的查询出现「条上显示 N 处命中、按全部替换报 0 处」的单向分叉。
    let crosses_line = query
        .chars()
        .any(|c| line_break_byte_len(c, None) > 0);
    let mut out = String::with_capacity(text.len());
    let mut count = 0usize;
    let mut pos = 0usize; // 已确认写出的原文边界
    let mut i = 0usize; // 下一个搜索起点
    while let Some(hit) = find_next(text.as_bytes(), &fq, case_sensitive, i) {
        // UTF-8 自同步性保证 hit 与 hit+len 均为字符边界（find_next 契约）
        let before_ok = hit == 0
            || !text[..hit].chars().next_back().is_some_and(is_word_char);
        let after_ok = !text[hit + fq.len()..]
            .chars()
            .next()
            .is_some_and(is_word_char);
        if crosses_line || (before_ok && after_ok) {
            out.push_str(&text[pos..hit]);
            out.push_str(replacement);
            pos = hit + fq.len();
            count += 1;
            i = pos;
        } else {
            // 边界失格：只前进一个字符继续扫，保持与 find_all 的
            // 非重叠命中序列可对拍
            i = hit + 1;
        }
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
///
/// 行尾归一（P26「替换当前」同口径下沉至此）：查询与替换文本里的换行
/// 一律先改写为文档主导行尾再匹配/写入——CRLF 文档上跨行查询 `a\nb`
/// 才找得到，替换文本里的 `\n` 也不会在结果里制造混合行尾。
pub fn replace_all_document(
    doc: &Document,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if query.is_empty() {
        let mut whole = String::with_capacity(doc.text_len_bytes());
        for chunk in doc.chunks() {
            whole.push_str(chunk);
        }
        return (whole, 0);
    }
    let eol = doc.line_ending();
    let query = eol.normalize(query);
    let replacement = eol.normalize(replacement);
    let fq: Vec<u8> = query.bytes().map(|b| fold_byte(b, case_sensitive)).collect();
    // 输出容量提示（O-15b）：原状 `String::new()` 从 0 翻倍长到全文大小，
    // 50MB 即 20+ 次 realloc、等量 memcpy，峰值还多一倍。
    // ⚠️ 只在**替换不短于查询**时预容量——那时输出长度 ≥ 输入长度，按文档
    // 字节数预置恰好是下界；替换更短（尤其整篇删空）时输出可能远小于输入，
    // 照抄全文预容量会凭空多占一个全文大小的缓冲，比翻倍增长更糟 → 交给
    // String 自身的增长策略。`with_capacity(0)` 不分配，等价于 new()。
    let cap_hint = if replacement.len() >= query.len() {
        doc.text_len_bytes()
    } else {
        0
    };
    let mut out = String::with_capacity(cap_hint);
    // 跨块残段：块边界可能落在任意位置，末尾不足一个查询长度的尾巴
    // 先攒着，与下一块拼接后再扫（复用分配，峰值 ≈ 存储块 + 查询长度）
    let mut carry = String::new();
    let mut count = 0usize;
    for chunk in doc.chunks() {
        carry.push_str(chunk);
        count += drain_matches(&mut carry, &mut out, &fq, &replacement, case_sensitive, false);
    }
    count += drain_matches(&mut carry, &mut out, &fq, &replacement, case_sensitive, true);
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
    find_all_regex_compiled(text, &re)
}

/// P146：已编译正则的扫描半程（[`find_all_regex`] 拆分）——
/// FIF 对至多 2 万文件逐个匹配，正则曾每文件重新编译一次（O(文件数)
/// 次编译放大），现在编译一次跨文件复用。
pub fn find_all_regex_compiled(
    text: &str,
    re: &fancy_regex::Regex,
) -> Result<Vec<MatchPos>, String> {
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

/// 对字节偏移 `byte_start` 处的当前正则命中做单次替换展开（支持
/// `$1`/`${1}` 组引用），专为零宽命中设计——常规命中可直接对命中文本
/// `Regex::replace`，零宽命中拿不到命中文本，须在原文上按位置重取
/// 捕获组。`text` 须包含该命中（零宽命中必为单行，所在行即可），
/// `byte_start` 为命中起点在 `text` 内的字节偏移。
pub fn expand_regex_at(
    text: &str,
    byte_start: usize,
    pattern: &str,
    replacement: &str,
    case_sensitive: bool,
) -> Result<String, String> {
    let re = compile_regex(pattern, case_sensitive)?;
    let caps = re
        .captures_from_pos(text, byte_start)
        .map_err(|e| e.to_string())?
        .ok_or("无匹配")?;
    let m0 = caps.get(0).ok_or("无匹配")?;
    if m0.start() != byte_start {
        // 行窗口内首个匹配不在命中位置（多行命中被窗口截断等异常形态）：
        // 显式报错而非静默错位展开
        return Err(format!("命中位置漂移（期望 {byte_start}，实际 {}）", m0.start()));
    }
    let mut out = String::new();
    caps.expand(replacement, &mut out);
    Ok(out)
}

/// 字节跨度序列 → [`MatchPos`]：单遍游标推进（避免每命中一次
/// `text[..start].chars().count()` 的 O(命中×文档) 复杂度）。
/// 行界按 ropey `unicode_lines` 全集（`\r\n` 计 1 字符）；命中内容内
/// 的换行推进行号、列号回到行首计数。
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
            if line_break_byte_len(c, None) > 0 {
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
    fn find_line_numbers_align_with_ropey_unicode_lines() {
        // 行界口径与 ropey `unicode_lines` 全集对齐：孤立 \r 也是行界。
        // 旧实现只认 \n，经典 Mac CR 文件上所有命中行号漂移、选区错位。
        // "a\rb" 在编辑器里是两行（ropey 行界），b 在第 1 行第 0 列。
        let text = "a\rb";
        assert_eq!(
            find_all(text, "b", true),
            vec![MatchPos { line: 1, col: 0, len_chars: 1 }]
        );
        // 单字符行界 VT/FF/NEL/LS/PS 同口径
        for br in ['\u{000B}', '\u{000C}', '\u{0085}', '\u{2028}', '\u{2029}'] {
            let text = format!("a{br}b");
            assert_eq!(
                find_all(&text, "b", true),
                vec![MatchPos { line: 1, col: 0, len_chars: 1 }],
                "行界字符 U+{:04X}", br as u32
            );
        }
        // rope 路径与 str 路径同口径
        let doc = Document::from_str("a\rb");
        assert_eq!(
            find_all_document(&doc, "b", true),
            find_all("a\rb", "b", true),
        );
        // 正则路径同口径
        assert_eq!(
            find_all_regex("a\rb", "b", true).unwrap(),
            vec![MatchPos { line: 1, col: 0, len_chars: 1 }]
        );
        // 跨行查询沿用 P26 归一口径：查询 \n 命中孤立 \r 的行界
        assert_eq!(
            find_all("a\rb", "a\nb", true),
            vec![MatchPos { line: 0, col: 0, len_chars: 3 }]
        );
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

    /// P217 回归：行界字符**后面紧跟另一个行界字符**时必须切成两行。
    ///
    /// 旧切行用 `blen == 2` 当「这是 CRLF 成对」的判据，而 NEL(U+0085) 本身就是
    /// 2 字节，于是 NEL 后面那个字符被无条件 `chars.next()` 吃掉：`"a\u{85}\nb"`
    /// 切成两行（第二行是 `"\nb"`），此后所有行号左移一位。用户可见后果：查找
    /// 全部/在文件中查找报的行号与编辑器不一致，点上去选中的也不是那一处。
    ///
    /// ⚠️ 这条**测不出**在既有对拍里不是偶然：`find_all_char_ref` 那份 oracle
    /// 与本函数共用 `for_each_line`，切错时两者一起错，「新码 == 旧码」恒成立。
    /// 能暴露它的是与 **rope 路径**（`find_all_document`，行号来自 ropey）同结论。
    #[test]
    fn consecutive_line_breaks_split_into_separate_lines() {
        const BREAKS: [char; 7] = [
            '\n', '\r', '\u{000B}', '\u{000C}', '\u{0085}', '\u{2028}', '\u{2029}',
        ];
        for &a in &BREAKS {
            for &b in &BREAKS {
                let text = format!("x{a}{b}y");
                // 只有 \r\n 是一整个行界；其余任意两两组合都是两个行界 → 三行
                let paired = a == '\r' && b == '\n';
                let mut lines: Vec<(usize, String)> = Vec::new();
                for_each_line(&text, |i, l| lines.push((i, l.to_owned())));
                assert_eq!(lines.len(), if paired { 2 } else { 3 }, "{text:?} 切行数错");
                assert!(
                    lines.iter().all(|(_, l)| !l.contains(BREAKS)),
                    "{text:?}: 行内容里不得残留行界字符：{lines:?}"
                );
                let y = lines.iter().position(|(_, l)| l == "y").expect("应有 y 行");
                assert_eq!(y, if paired { 1 } else { 2 }, "{text:?}: y 的行号错");
                assert_eq!(
                    find_all(&text, "y", true),
                    find_all_document(&Document::from_str(&text), "y", true),
                    "{text:?}: 与 rope 路径口径分叉"
                );
            }
        }
    }

    #[test]
    fn find_all_document_matches_find_all_exactly() {
        let fixtures = [
            "",                                  // 空文档
            "single line no newline",            // 单行无换行
            "foo\nbar foo\n",                    // 常规多行（尾换行）
            "a\r\nb\r\nc",                       // CRLF：\r\n 是一个行界单元
            "x\r\ny\nz\rw",                      // 混合 + 孤立 \r（孤立 \r 也是行界）
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
    fn find_all_pins_line_and_multibyte_columns() {
        // 钉住精确坐标：跨行长短交替 + 空行 + 多字节行（O-11 之前这条回归的是
        // 「跨行复用 Vec<char> 是否残留上一行状态」；内核改字节扫描后缓冲已不
        // 存在，但坐标断言本身仍是想要的不变量——列号按**字符**而非字节）
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

    /// O-11 参照实现：字面查找的**旧逐字符算法**原样留档（P11 / 第 39 轮的
    /// 形状），只当差分对拍的 oracle。
    ///
    /// 为什么非要留一份旧代码：重构后 `find_all` 与 `find_all_document` 共用
    /// **同一个**新内核，它俩互相相等已经不构成证据（同一段代码自己等于自己）。
    /// 真正的论点是「字节窗口匹配 ≡ 逐字符窗口匹配」，那只靠 UTF-8 自同步性
    /// 推演不够，必须有独立参照。跨行查询走另一个内核（本次未触碰），直接委托
    /// 生产实现跳过对拍。
    fn find_all_char_ref(text: &str, query: &str, case_sensitive: bool) -> Vec<MatchPos> {
        let mut out = Vec::new();
        if query.is_empty() {
            return out;
        }
        let q: Vec<char> = query.chars().collect();
        if q.contains(&'\n') {
            return find_all(text, query, case_sensitive);
        }
        let mut lc: Vec<char> = Vec::new();
        for_each_line(text, |line_idx, line| {
            lc.clear();
            lc.extend(line.chars());
            if lc.len() < q.len() {
                return;
            }
            let first = q[0];
            'window: for start in 0..=(lc.len() - q.len()) {
                if !char_eq(first, lc[start], case_sensitive) {
                    continue;
                }
                for (offset, &qc) in q.iter().enumerate().skip(1) {
                    if !char_eq(qc, lc[start + offset], case_sensitive) {
                        continue 'window;
                    }
                }
                out.push(MatchPos { line: line_idx, col: start, len_chars: q.len() });
            }
        });
        out
    }

    /// O-11 主护栏：字节扫描与旧的逐字符扫描**逐点相等**（行号/列号/跨度），
    /// 并顺带交叉钉住 O-13 的「封顶 == 全量前缀」在新内核上仍成立。
    #[test]
    fn find_all_byte_scan_equals_char_scan_on_random_texts() {
        let queries = [
            "a", "aa", "中", "🚀", "ab", "a中", "\r", "x", " ", "行", "the", "THe",
        ];
        for seed in [101u64, 202, 30303, 0xC0FFEE] {
            let text = pseudo_random_text(seed, 20_000);
            let doc = Document::from_str(&text);
            // 加载度自检：随机样本字母表是 a b c \n \r 中 🚀 x，若哪天生成器
            // 变了导致这些查询全部空对空，本用例就成了假绿——先钉住它真的忙。
            let load = find_all(&text, "a", true).len();
            assert!(load > 1000, "seed={seed} 样本里 'a' 命中仅 {load}，对拍已失去判别力");
            assert!(
                text.contains('中') && text.contains('🚀') && text.contains('\r'),
                "seed={seed} 样本必须含多字节与 \\r 行界"
            );
            for q in queries {
                for cs in [true, false] {
                    let got = find_all(&text, q, cs);
                    assert_eq!(
                        got,
                        find_all_char_ref(&text, q, cs),
                        "字节扫描与逐字符参照不等: seed={seed} query={q:?} cs={cs}"
                    );
                    assert_eq!(
                        find_all_document(&doc, q, cs),
                        got,
                        "rope 路径须与全文路径同结果: seed={seed} query={q:?} cs={cs}"
                    );
                    for limit in [0usize, 1, 7, 500] {
                        let want: Vec<MatchPos> = got.iter().take(limit).cloned().collect();
                        assert_eq!(
                            find_all_limited(&text, q, cs, limit),
                            want,
                            "封顶版不再是全量前缀: seed={seed} query={q:?} limit={limit}"
                        );
                    }
                }
            }
        }
    }

    /// O-11 的行界口径定例：`pseudo_random_text` 的字母表里没有 VT/FF/NEL/LS/PS
    /// 这四个「也算行界」的字符，而字面查找按行切分（`for_each_line`）——查询
    /// 若含这些字符，命中集合取决于**切行口径**，正是新旧实现最容易分叉的地方。
    /// 逐字符参照与字节扫描必须在每一种形态上给出同一张表。
    #[test]
    fn find_all_byte_scan_agrees_with_char_scan_on_exotic_line_breaks() {
        let fixtures = [
            "a\u{b}a\u{c}a",                 // VT / FF 也是行界
            "\u{85}ab\u{85}cd",              // NEL
            "行\u{2028}前\u{2029}后",         // LS / PS
            "x\r\ny\rz\nw",                  // CRLF / 孤立 CR / LF 混排
            "\u{1F600}\u{b}\u{1F600}",        // 4 字节字符夹一个行界
            "a\u{b}",                        // 行界收尾
        ];
        let queries = [
            "a", "\u{b}", "\u{c}", "\u{85}", "\u{2028}", "\u{2029}", "\u{1F600}", "ab",
            "x\r", "\r", "行", "前", "后",
            // 定例 4（x\r\ny\rz\nw）只有拉丁短串可命中——「x\r」按下面的行界口径
            // 永不命中，故必须补 x / y / cd，否则该定例是空对空的假绿
            "x", "y", "cd", "z",
        ];
        for text in fixtures {
            let doc = Document::from_str(text);
            let mut any_hit = 0usize;
            for q in queries {
                for cs in [true, false] {
                    let got = find_all(text, q, cs);
                    assert_eq!(
                        got,
                        find_all_char_ref(text, q, cs),
                        "行界定例不等: text={text:?} query={q:?} cs={cs}"
                    );
                    assert_eq!(find_all_document(&doc, q, cs), got, "rope 路径 text={text:?} query={q:?}");
                    any_hit += got.len();
                }
            }
            assert!(any_hit > 0, "定例 {text:?} 一条都没命中，对拍失去判别力");
        }
        // 显式钉住一条**既有口径**（本次重构原样保留，不是新决策）：VT/FF/NEL/
        // LS/PS 属于行界全集 ⇒ 它们不进入行内容 ⇒ 查询里含这些字符时**永不命中**，
        // 即便文本里确有该字符。与「查询含 \r 也永不命中」同族。改这条要先过
        // N-09 那张分叉矩阵（半修行界口径会把「找不到」变成「找得到换不掉」）。
        assert!(find_all("a\u{b}b", "\u{b}", true).is_empty(), "VT 是行界：不得作为行内命中");
        assert!(find_all("行\u{2028}前", "\u{2028}", true).is_empty(), "LS 是行界：同上");
        assert_eq!(
            find_all("a\u{b}b", "a", true),
            vec![MatchPos { line: 0, col: 0, len_chars: 1 }],
            "VT 前的一段就是一条独立行"
        );
        assert_eq!(
            find_all("a\u{b}b", "b", true),
            vec![MatchPos { line: 1, col: 0, len_chars: 1 }],
            "VT 之后的内容算下一行"
        );
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

        // 对照：行界统一后 \r 只作为行界存在（与 ropey 同源），单行窗口
        // 内不会再出现字面 \r——含 \r 不含 \n 的查询恒不命中；
        // 跨行匹配一律经 \n 查询走归一分支（孤立 \r 折叠判等）
        assert!(find_all("x\ry", "x\ry", true).is_empty());
        assert_eq!(
            find_all("x\ry", "x\ny", true),
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
            // Document 路径的既定语义含行尾归一：对拍基准用同口径归一后
            // 的查询（EOL 无关查询归一为恒等，等价性不受影响）
            let eol = doc.line_ending();
            for query in ["a", "aa", "foo", "中", "🚀", "\r\n", "zz"] {
                for cs in [true, false] {
                    assert_eq!(
                        replace_all_document(&doc, query, "X", cs),
                        replace_all(text, &eol.normalize(query), "X", cs),
                        "不一致: text={text:?} query={query:?} cs={cs}"
                    );
                }
            }
        }
        // 空查询 no-op：返回全文与 0 次
        let doc = Document::from_str("hello\nworld");
        assert_eq!(replace_all_document(&doc, "", "X", true), (doc.to_text(), 0));
    }

    /// O-15b：输出**容量提示**不得改变结果，且两个分支都要走到。
    ///
    /// 既有对拍用例的替换串一律不短于查询（`"X"` / `"<R>"`），只走「预容量」
    /// 那一支；「替换更短甚至删空 → 不预容量」这一支此前从未与 `replace_all`
    /// 对拍过。本条把两个方向一起钉住（结果必须与朴素实现逐字节相同）。
    #[test]
    fn replace_all_document_capacity_branches_match_plain_replace() {
        let fixtures = ["", "aaaa aa\naa", "中文中文\n🚀🚀中\n", "x\r\ny\nz\rw"];
        for text in fixtures {
            let doc = Document::from_str(text);
            let eol = doc.line_ending();
            for (query, repl) in [
                ("a", "aaaaaa"), // 变长 → 预容量支
                ("aa", "X"),     // 变短 → 不预容量支
                ("中文", ""),    // 整篇删空 → 不预容量支（预置会白占全文大小）
                ("\r\n", "|"),   // 行界相关 + 变短
            ] {
                for cs in [true, false] {
                    assert_eq!(
                        replace_all_document(&doc, query, repl, cs),
                        replace_all(text, &eol.normalize(query), repl, cs),
                        "不一致: text={text:?} query={query:?} repl={repl:?} cs={cs}"
                    );
                }
            }
        }
    }

    #[test]
    fn replace_all_document_matches_on_random_texts() {
        for seed in [2u64, 0xFEED_FACE, 98765] {
            let text = pseudo_random_text(seed, 20_000);
            let doc = Document::from_str(&text);
            for query in ["a", "ab", "c\n", "中", "xx", "a\r", "🚀"] {
                let (streamed, n_streamed) = replace_all_document(&doc, query, "<R>", false);
                let (plain, n_plain) =
                    replace_all(&text, &doc.line_ending().normalize(query), "<R>", false);
                assert_eq!(streamed, plain, "seed={seed} query={query:?}");
                assert_eq!(n_streamed, n_plain);
            }
        }
    }

    #[test]
    fn replace_all_document_normalizes_query_and_replacement_eol() {
        // 行尾归一（P26「替换当前」同口径下沉）：CRLF 文档上跨行查询
        // 找得到（旧实现纯字节匹配恒零命中——查找侧归一、替换侧不归一
        // 的「找得到换不掉」）；替换文本里的 \n 改写为主导行尾，
        // 不再在结果里制造混合行尾。
        let doc = Document::from_str("a\r\nX\r\nb\r\nX\r\nb");
        let (out, n) = replace_all_document(&doc, "X\nb", "Y\nZ", true);
        assert_eq!(n, 2);
        assert_eq!(out, "a\r\nY\r\nZ\r\nY\r\nZ");
        // 反向：LF 文档上 \r\n 形式的查询同样命中 LF 换行单元
        let doc = Document::from_str("a\nX\nb");
        let (out, n) = replace_all_document(&doc, "X\r\nb", "Y", true);
        assert_eq!(n, 1);
        assert_eq!(out, "a\nY");
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
    fn filter_whole_word_checks_both_boundaries() {
        // 词字符包围的命中剔除；行首/行尾/标点边界保留；下划线算词字符
        let doc = Document::from_str("cat concat scat cat_\ncat");
        let hits = find_all_document(&doc, "cat", true);
        assert_eq!(hits.len(), 5);
        let words = filter_whole_word(&doc, hits);
        // 独立的 cat：行 0 首位与行 1；cat_ 的 cat 后随下划线（词字符）剔除
        assert_eq!(
            words,
            vec![
                MatchPos { line: 0, col: 0, len_chars: 3 },
                MatchPos { line: 1, col: 0, len_chars: 3 },
            ]
        );
        // 行尾命中：后边界 = 行尾，保留
        let doc = Document::from_str("x cat");
        assert_eq!(
            filter_whole_word(&doc, find_all_document(&doc, "cat", true)),
            vec![MatchPos { line: 0, col: 2, len_chars: 3 }]
        );
    }

    /// **改前实现原样留档**（O-12 等价性对拍的基准）：每个命中都把整行物化
    /// 成 `Vec<char>`，成本 O(命中数 × 行长)。语义即旧契约，一条不改。
    fn filter_whole_word_naive(doc: &Document, hits: Vec<MatchPos>) -> Vec<MatchPos> {
        hits.into_iter()
            .filter(|hit| {
                let content: Vec<char> = doc
                    .line_str(hit.line)
                    .chars()
                    .take_while(|&c| c != '\n' && c != '\r')
                    .collect();
                let end = hit.col + hit.len_chars;
                if end > content.len() {
                    return true; // 跨行命中：保留
                }
                let before_ok = hit.col == 0 || !is_word_char(content[hit.col - 1]);
                let after_ok = end == content.len() || !is_word_char(content[end]);
                before_ok && after_ok
            })
            .collect()
    }

    /// O-12 的语义钉：穷尽边界表上，「按行游标复用的新实现」「改前的朴素
    /// 实现」「`&str` 孪生 `filter_whole_word_text`」三方逐点相等。
    #[test]
    fn filter_whole_word_agrees_with_naive_reference_and_str_twin() {
        use crate::find_in_files::filter_whole_word_text;
        // 每条形如 (文本, 查询)：行界形态 × 邻字符形态 的笛卡尔覆盖
        let cases: &[(&str, &str)] = &[
            // 行首 / 行尾 / 行中间
            ("cat", "cat"),
            ("cat dog", "cat"),
            ("dog cat", "cat"),
            ("dog cat dog", "cat"),
            ("concat catalog", "cat"),
            ("cat concat scat cat_\ncat", "cat"),
            // 行尾形态：LF / CRLF / 孤立 CR / 无行尾 / 末行为空
            ("cat\r\ncat dog\r\n", "cat"),
            ("cat\rcat dog\r", "cat"),
            ("cat\ncat\n", "cat"),
            // VT/FF/NEL/LS/PS 结尾：Document 版把该行界字符留在 content 里，
            // &str 孪生的 content 不含它——两种形状必须得出同一结论
            ("cat\u{000B}xcat", "cat"),
            ("cat\u{000C}cat", "cat"),
            ("cat\u{0085}cat", "cat"),
            ("cat\u{2028}cat", "cat"),
            ("cat\u{2029}cat", "cat"),
            // 邻字符类别：标点 / 空白 / 下划线 / CJK（词字符）/ emoji（非词字符）
            ("(cat)-cat.", "cat"),
            ("_cat cat_", "cat"),
            ("中cat cat中", "cat"),
            ("\u{1F680}cat cat\u{1F680}", "cat"),
            ("  cat  ", "cat"),
            // 大小写折叠命中本体（边界判定与大小写无关）
            ("Cat CAT cat", "cat"),
            // 重叠命中窗口
            ("aaaa aa\naa", "aa"),
            // 跨行命中（查询含 \n → len_chars 含行界单元，end 越过行内容）
            ("cat\ncat", "t\nc"),
            ("cat\r\ndog", "t\nd"),
            ("cat\rcat", "t\nc"),
            // 空行 / 空文档
            ("\n\ncat", "cat"),
            ("", "cat"),
        ];
        for (text, query) in cases {
            let doc = Document::from_str(text);
            for cs in [true, false] {
                let hits = find_all_document(&doc, query, cs);
                assert_eq!(hits, find_all(text, query, cs), "命中表 {text:?}");
                let fast = filter_whole_word(&doc, hits.clone());
                assert_eq!(
                    fast,
                    filter_whole_word_naive(&doc, hits.clone()),
                    "新实现偏离改前语义: text={text:?} query={query:?} cs={cs}"
                );
                assert_eq!(
                    fast,
                    filter_whole_word_text(text, hits),
                    "与 &str 孪生口径分叉: text={text:?} query={query:?} cs={cs}"
                );
            }
        }
    }

    #[test]
    fn filter_whole_word_keeps_cross_line_hits_and_ignores_hit_order() {
        // 跨行命中（end > 行内容长度）一律保留：真实命中（查询含 \n）…
        let doc = Document::from_str("cat\ncat");
        let hits = find_all_document(&doc, "t\nc", true);
        assert_eq!(hits, vec![MatchPos { line: 0, col: 2, len_chars: 3 }]);
        assert_eq!(filter_whole_word(&doc, hits.clone()), hits);
        // …与手工构造的越界命中同口径（end 恰等于行内容是「行尾」，不是越界）
        assert_eq!(
            filter_whole_word(
                &doc,
                vec![
                    MatchPos { line: 0, col: 1, len_chars: 3 }, // end=4 > 3：跨行
                    MatchPos { line: 0, col: 0, len_chars: 3 }, // end=3 = 行尾：保留
                    MatchPos { line: 0, col: 0, len_chars: 2 }, // end=2 < 3：后邻 't' 为词字符 → 剔除
                ]
            ),
            vec![
                MatchPos { line: 0, col: 1, len_chars: 3 },
                MatchPos { line: 0, col: 0, len_chars: 3 },
            ],
            "越界命中保留、行尾命中保留、词内命中剔除"
        );

        // 乱序命中表：行号一变就重建缓冲，结果与有序输入同集合（口径不依赖顺序）
        let doc = Document::from_str("cat concat\ncat\ndog cat");
        let hits = find_all_document(&doc, "cat", true);
        let kept_asc = filter_whole_word(&doc, hits.clone());
        let mut rev = hits.clone();
        rev.reverse();
        let mut kept_rev = filter_whole_word(&doc, rev);
        kept_rev.sort_by_key(|h| (h.line, h.col));
        assert_eq!(kept_asc, vec![
            MatchPos { line: 0, col: 0, len_chars: 3 },
            MatchPos { line: 1, col: 0, len_chars: 3 },
            MatchPos { line: 2, col: 4, len_chars: 3 },
        ]);
        assert_eq!(kept_asc, kept_rev, "乱序输入不得改变过滤结果");
    }

    #[test]
    fn filter_whole_word_materializes_each_hit_line_once() {
        // 成本形状契约（非耗时）：命中数 ≫ 命中行数时，行内容缓冲只按
        // **行**物化。改前每个命中物化整行两遍（整行 String + Vec<char>），
        // 本用例的计数会是 202 而非 2——断言与机器性能无关。
        let text = format!("{}cat dog\ncat\n", "cat ".repeat(200));
        let doc = Document::from_str(&text);
        let hits = find_all_document(&doc, "cat", true);
        assert_eq!(hits.len(), 202, "前提：201 个命中挤在同一个长行上");

        WHOLE_WORD_LINE_BUILDS.with(|c| c.set(0));
        let kept = filter_whole_word(&doc, hits.clone());
        let builds = WHOLE_WORD_LINE_BUILDS.with(|c| c.get());

        assert_eq!(builds, 2, "命中只分布在 2 行 → 只物化 2 次（改前为 202 次）");
        assert_eq!(kept.len(), 202, "全部命中各自独立成词，都应保留");
        assert_eq!(kept, filter_whole_word_naive(&doc, hits), "省钱不改语义");
    }

    #[test]
    fn replace_all_word_respects_boundaries() {
        // 词内命中不替换（concat 内部的 cat 前邻 n，非词边界）；
        // 文首独立词替换
        let (out, n) = replace_all_word("cat concat scat", "cat", "dog", true);
        assert_eq!((out.as_str(), n), ("dog concat scat", 1));
        // 标点/行尾边界保留命中
        let (out, n) = replace_all_word("(cat), cat", "cat", "dog", true);
        assert_eq!((out.as_str(), n), ("(dog), dog", 2));
        // 与 find_all + filter_whole_word 的命中序列对拍（不重叠语义一致）
        for text in ["aa a aa", "aaa", "aaba", "cat concat cat cat"] {
            let doc = Document::from_str(text);
            let hits = filter_whole_word(&doc, find_all_document(&doc, "cat", true));
            let expected = hits.len();
            let (_, n) = replace_all_word(text, "cat", "X", true);
            assert_eq!(n, expected, "text={text:?}");
        }
    }

    /// P222：整词替换的数量必须等于「整词过滤器保留的命中里，贪心取出的
    /// 非重叠子集」——条上看到几处（能同时换的）就换几处。用小字母表
    /// {a, ., 空格, \n} 穷举文本（长度 0..=4）× 查询（长度 1..=2）跑一遍：
    /// 当初正是这条没测，1306 例单向分叉（全部含换行的查询：显示 N 处命中、
    /// 替换 0 处）才能活到审计那天。
    ///
    /// 为什么要先做「非重叠」折算而不是直接比命中数：`find_all` 有意保留
    /// 重叠命中（起点前进 1，O-11 钉过的契约），而一次替换会吃掉后一个重叠
    /// 命中的立足点——`"..."` 查 `".."` 保留 2 处却只能换 1 处，这是正确的。
    /// 既有 `replace_all_word_respects_boundaries` 的对拍只喂单行、不重叠的
    /// 查询，两类岔路都测不到。
    #[test]
    fn whole_word_replace_all_matches_the_filter_on_every_small_case() {
        const ALPHA: [char; 4] = ['a', '.', ' ', '\n'];
        let enumerate = |len: usize| -> Vec<String> {
            (0..4u32.pow(len as u32))
                .map(|code| {
                    let mut rest = code;
                    let mut s = String::new();
                    for _ in 0..len {
                        s.push(ALPHA[(rest % 4) as usize]);
                        rest /= 4;
                    }
                    s
                })
                .collect()
        };
        let texts: Vec<String> = (0..=4).flat_map(enumerate).collect();
        let queries: Vec<String> = (1..=2).flat_map(enumerate).collect();
        let mut loaded = 0usize;
        for text in &texts {
            let doc = Document::from_str(text);
            for query in &queries {
                let kept = filter_whole_word(&doc, find_all(text, query, true));
                // 贪心折算成非重叠子集（本字母表全为 1 字节字符，故
                // line_to_char + col 就是字符偏移）
                let mut expected = 0usize;
                let mut free_from = 0usize;
                for h in &kept {
                    let start = doc.line_to_char(h.line) + h.col;
                    if start >= free_from {
                        expected += 1;
                        free_from = start + h.len_chars;
                    }
                }
                let (out, n) = replace_all_word(text, query, "X", true);
                assert_eq!(
                    n,
                    expected,
                    "文本 {text:?} 查询 {query:?}：过滤器保留 {} 处（非重叠 {expected} 处），替换了 {n} 处",
                    kept.len()
                );
                if n > 0 {
                    // 数量对了还得真的换了东西（防空转）
                    assert!(
                        out.contains('X'),
                        "文本 {text:?} 查询 {query:?}：n={n} 却没写入替换串"
                    );
                    loaded += 1;
                }
            }
        }
        assert!(
            loaded > 500,
            "自检：这批组合里必须有足够多真的发生替换，否则整条用例是空转（实际 {loaded}）"
        );
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
    fn expand_regex_at_expands_zero_width_match_in_place() {
        // 零宽命中按位置展开：$0 引用整段命中、空匹配也能展开
        assert_eq!(expand_regex_at("abc", 1, "b*", "[${0}]", true).unwrap(), "[b]");
        assert_eq!(expand_regex_at("abc", 1, "x*", "Y", true).unwrap(), "Y");
        // 组引用与常规命中同样可用
        assert_eq!(expand_regex_at("a1b", 0, r"a(\d)", "<$1>", true).unwrap(), "<1>");
        // 行窗口内首个匹配不在命中位置：显式报错而非静默错位展开
        assert!(expand_regex_at("abc", 1, "c", "Y", true).is_err());
    }

    #[test]
    fn regex_multiline_mode_documented_via_inline_flags() {
        // ^ $ 默认只锚文本首尾；逐行锚定用 (?m) 内联标志（UI 提示口径）
        let text = "a1\nb2\na3";
        assert_eq!(find_all_regex(text, "^a.", false).unwrap().len(), 1);
        assert_eq!(find_all_regex(text, "(?m)^a.", false).unwrap().len(), 2);
    }
}
