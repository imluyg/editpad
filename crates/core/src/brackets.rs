//! 括号匹配（第 61 轮）：纯逻辑的双向嵌套扫描，零 GUI 依赖。
//!
//! 口径：
//! * 括号对 = `()`、`[]`、`{}` 三对——`<>` 与比较/泛型/箭头歧义太大，
//!   不入表（主流轻量编辑器同口径）；
//! * 不做字符串/注释感知（无语法状态参与）：字符串里的括号同样参与
//!   配对，属已知取舍——换来的是与文档语言无关的稳定行为；
//! * 扫描步数有 [`MAX_BRACKET_SCAN_CHARS`] 硬上限：孤立括号（无匹配）
//!   的最坏 O(n) 逐字符扫描被封顶，超大文档上光标停顿不受惩罚；
//!   上限内流尽未配平同样返回 None。

/// 参与匹配的括号对（开, 闭）。
pub const BRACKET_PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// 单向扫描的字符步数上限（防御无匹配时的最坏 O(n)；按字符数计）。
pub const MAX_BRACKET_SCAN_CHARS: usize = 4_000_000;

/// 判定字符是否为括号。`Some((开括号, 闭括号, 是否开括号))`；非括号 → None。
pub fn bracket_kind(c: char) -> Option<(char, char, bool)> {
    for &(open, close) in &BRACKET_PAIRS {
        if c == open {
            return Some((open, close, true));
        }
        if c == close {
            return Some((open, close, false));
        }
    }
    None
}

/// 从开括号**之后**一个字符起扫描（调用方传入已排除开括号的剩余
/// 迭代器），返回配对闭括号在剩余序列中的**含端步数**——
/// `配对偏移 = 开括号偏移 + 返回值`（两括号紧邻 `"()"` 时剩余序列仅
/// 一个闭括号，返回 1）。嵌套计数只认同一括号对，其余字符一律跳过。
/// 步数超过 `cap` 或流尽未配平 → None。
pub fn scan_forward_capped(
    open: char,
    close: char,
    rest: impl Iterator<Item = char>,
    cap: usize,
) -> Option<usize> {
    let mut depth = 1usize;
    for (steps, c) in rest.take(cap).enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(steps + 1);
            }
        }
    }
    None
}

/// [`scan_forward_capped`] 的默认上限版（生产入口）。
pub fn scan_forward(open: char, close: char, rest: impl Iterator<Item = char>) -> Option<usize> {
    scan_forward_capped(open, close, rest, MAX_BRACKET_SCAN_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> impl Iterator<Item = char> + '_ {
        s.chars()
    }

    #[test]
    fn bracket_kind_classifies_pairs() {
        assert_eq!(bracket_kind('('), Some(('(', ')', true)));
        assert_eq!(bracket_kind(']'), Some(('[', ']', false)));
        assert_eq!(bracket_kind('}'), Some(('{', '}', false)));
        assert_eq!(bracket_kind('<'), None, "<> 不参与（歧义太大）");
        assert_eq!(bracket_kind('a'), None);
        assert_eq!(bracket_kind('中'), None);
    }

    #[test]
    fn forward_distance_counts_matched_char() {
        // "(a(b))"：首字符 '(' 的配对 = 末字符 ')'，含端距离 5
        let s = "(a(b))";
        assert_eq!(scan_forward('(', ')', chars(&s[1..])), Some(5));
        // 紧邻 "()"：rest 只剩 ")"，含端步数 1（配对偏移 = first+1）
        assert_eq!(scan_forward('(', ')', chars(")")), Some(1));
        // 嵌套 + 干扰字符（其他括号对不参与本对计数；"x[y]{}z}" 共
        // 8 字符，配对的正是末位 '}'）
        assert_eq!(scan_forward('{', '}', chars("x[y]{}z}")), Some(8));
        // 未配平
        assert_eq!(scan_forward('(', ')', chars("ab")), None);
        // 多配（闭比开多）：第一个归零处即配对
        assert_eq!(scan_forward('(', ')', chars(")))")), Some(1));
    }

    #[test]
    fn scan_cap_bounds_runaway() {
        // 上限内无配平 → None（而不是扫到流尽）：cap=4 时第 5 个字符
        // 本可配对，但被截断
        assert_eq!(scan_forward_capped('(', ')', chars("aaaaa)"), 5), None);
        assert_eq!(scan_forward_capped('(', ')', chars("aaaa)"), 5), Some(5));
    }
}
