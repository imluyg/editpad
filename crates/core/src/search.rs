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

/// 全部替换，返回 (新文本, 替换次数)。
pub fn replace_all(
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

    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut count = 0usize;
    while i <= t.len() - q.len() {
        if (0..q.len()).all(|k| char_eq(q[k], t[i + k], case_sensitive)) {
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
}
