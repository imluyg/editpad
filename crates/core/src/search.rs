//! 纯文本查找与替换。
//!
//! M1 采用 O(n·m) 字符级扫描，几十 MB 量级完全够用；
//! M2 随 rope 化升级为增量匹配。
//!
//! 大小写语义：仅对 ASCII 做大小写折叠（`a == A`），非 ASCII 字符
//! （如中文）本身没有大小写概念，始终精确比较——这样所有位置
//! 计数都不会受 Unicode 折叠改变字符数的影响。

/// 一个匹配的位置：行号、行内字符列（均从 0 计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchPos {
    pub line: usize,
    pub col: usize,
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
        a.to_ascii_lowercase() == b.to_ascii_lowercase()
    }
}

fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    ascii_case_eq(a, b, case_sensitive)
}

/// 找出全部匹配（按文档顺序）。查询串为空时返回空表。
pub fn find_all(text: &str, query: &str, case_sensitive: bool) -> Vec<MatchPos> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let q: Vec<char> = query.chars().collect();
    for (line_idx, line) in text.split('\n').enumerate() {
        scan_line(line, &q, case_sensitive, line_idx, &mut out);
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

    let mut line = String::new();
    let mut line_idx = 0usize;
    for chunk in doc.chunks() {
        let mut rest = chunk;
        // 块边界可能落在任意位置：'\n' 前的残段累积进当前行缓冲，
        // 遇到完整 '\n' 才结算一行——保证与 split('\n') 逐字节等价
        while let Some(pos) = rest.find('\n') {
            line.push_str(&rest[..pos]);
            scan_line(&line, &q, case_sensitive, line_idx, &mut out);
            line.clear();
            line_idx += 1;
            rest = &rest[pos + 1..];
        }
        line.push_str(rest);
    }
    scan_line(&line, &q, case_sensitive, line_idx, &mut out);
    out
}

/// 单行窗口扫描：在 `line` 的字符序列上滑动长度 `q.len()` 的窗口逐一比较。
fn scan_line(line: &str, q: &[char], case_sensitive: bool, line_idx: usize, out: &mut Vec<MatchPos>) {
    let lc: Vec<char> = line.chars().collect();
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
        });
    }
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
            vec![MatchPos{line:0,col:0}, MatchPos{line:0,col:6}, MatchPos{line:1,col:1}]
        );
        // 不区分大小写：第0行多出 col 3（"AB"）
        assert_eq!(
            find_all(text, "ab", false),
            vec![
                MatchPos{line:0,col:0},
                MatchPos{line:0,col:3},
                MatchPos{line:0,col:6},
                MatchPos{line:1,col:1},
            ]
        );
    }

    #[test]
    fn find_chinese_and_empty_query() {
        let text = "你好，世界\n你好\n";
        assert_eq!(
            find_all(text, "你好", true),
            vec![MatchPos{line:0,col:0}, MatchPos{line:1,col:0}]
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
            for query in ["a", "aa", "foo", "中", "🚀x", "\r", "\r\n", "zz"] {
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
            for query in ["a", "ab", "c\n", "中", "xx", "a\r"] {
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
            vec![MatchPos { line: 0, col: 100_000 }]
        );
        // 空查询约定：返回空表
        assert!(find_all_document(&doc, "", true).is_empty());
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
}
