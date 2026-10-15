//! 括号自动配对判据（路线图 B11）：纯逻辑，零 GUI 依赖。
//!
//! 这一档只回答"这一个字符该补对、该跳过、还是照常插"，**不碰文档**——
//! 落笔与撤销成组那一段在 app 层（`EditorCore::insert_pair`）。判据握在
//! core 换来的是：整张取舍表能不开窗口逐臂钉死。
//!
//! 口径：
//! * 配对表 = `()`、`[]`、`{}` 三对，与 [`crate::brackets::BRACKET_PAIRS`]
//!   **各自一份**而不是共用：那一侧是"跳转"的字符围，把 `"` `'` 拉进去会
//!   顺手改掉 <kbd>Ctrl+Shift+M</kbd> 的语义。两张表的关系由用例
//!   `auto_pair_table_does_not_leak_into_bracket_scan` 钉住；
//! * `<>` 不入表——[`crate::brackets`] 头部已因与比较/泛型/箭头歧义排除它，
//!   两个功能对同一字符表各说一套比缺一半更糟；
//! * 引号与反引号 v1 一律不配：中文正文里的 `'`、Markdown 里的 `` ` ``
//!   是两处已知伤害面（补进去的就是错字符）；
//! * **不做语法/字符串/注释感知**，与 [`crate::brackets`] 同款刻意取舍：
//!   换来与文档语言无关的稳定行为；
//! * 开括号只在右侧邻位"干净"时补对——右侧是词字符（字母/数字/下划线/CJK）
//!   时不补。这条判据复用 [`crate::search::is_word_char`] 而不是自写一份，
//!   所以中文正文里打 `(` 不会被塞一个看不见的 `)`。

use crate::search::is_word_char;

/// 参与自动配对的括号对（开, 闭）。
pub const AUTO_PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// 开括号对应的闭括号；不是开括号 → None。
pub fn closer_for(open: char) -> Option<char> {
    AUTO_PAIRS.iter().find(|(o, _)| *o == open).map(|&(_, c)| c)
}

/// 闭括号对应的开括号；不是闭括号 → None。
pub fn opener_for(close: char) -> Option<char> {
    AUTO_PAIRS
        .iter()
        .find(|(_, c)| *c == close)
        .map(|&(o, _)| o)
}

/// 一次打字的全部已知事实（调用方现量后填，本模块不碰文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairProbe {
    /// 用户按下的那个字符。
    pub typed: char,
    /// 光标左侧的字符；文档原点 → None。
    pub before: Option<char>,
    /// 光标右侧的字符；文档末尾 → None。
    pub after: Option<char>,
    /// 本功能在光标处补上的那个**闭括号**字符；没有 → None。
    ///
    /// 记字符而不是记偏移：在 `(|)` 里继续打字会把闭括号一路右移，绝对偏移
    /// 每次插入都得跟着搬，漏搬一次就指错地方。字符加"每次现量右侧邻居"
    /// 重验，天生没有陈旧这一类。
    pub auto_closer: Option<char>,
    /// 有选区（打字要先吃掉它，不是纯插入）。
    pub has_selection: bool,
    /// 覆写模式（`Insert` 键置起）。
    pub overwrite: bool,
    /// IME 组字中。
    pub composing: bool,
    /// 列块（矩形）选区态。
    pub in_block: bool,
    /// 存在附加光标（多光标同步编辑那一链）。
    pub has_extra_cursors: bool,
}

/// 这一击该怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAction {
    /// 补成对：插入开括号与闭括号，光标留在两者之间。
    InsertPair,
    /// 不吃字符、只把光标右移一格，越过本功能自己补的那个闭括号。
    SkipOver,
    /// 与引入本功能前逐字节相同。
    Plain,
}

/// 五种"这一律不配"的情形（[`PairAction::Plain`] 的唯一来源）。
///
/// 每一条都有自己的来由，不是笼统的"怕出事"：
/// 有选区＝打字是替换；覆写态＝补对的第二次插入会吃掉右边那个字符；
/// 组字中＝上屏文本不是逐字符按键；列块态＝逐行替换整列；
/// 多光标＝那条链的成组判据要求载荷恰为一个字符，双字符载荷会把每一步
/// 打成碎撤销（`cursors.rs` 的 `single_char`）。
fn suppressed(p: &PairProbe) -> bool {
    p.has_selection || p.overwrite || p.composing || p.in_block || p.has_extra_cursors
}

/// 右侧邻位是否允许补对：末尾、空白、行界、标点都算干净，词字符不算。
fn right_is_clean(after: Option<char>) -> bool {
    !after.is_some_and(is_word_char)
}

/// 判据：这一击补对、越过、还是照常插入。
pub fn pair_action(p: &PairProbe) -> PairAction {
    if suppressed(p) {
        return PairAction::Plain;
    }
    // 跳过排在补对之前：`(|)` 上按 `)` 是"越过"，不是"在括号里再插一个"。
    if p.auto_closer == Some(p.typed) && p.after == Some(p.typed) && opener_for(p.typed).is_some() {
        return PairAction::SkipOver;
    }
    if closer_for(p.typed).is_some() && right_is_clean(p.after) {
        return PairAction::InsertPair;
    }
    PairAction::Plain
}

/// 退格要不要把整对一起删掉（true＝整对）。
///
/// 判据的两道闸都在防同一件事——**不许吃掉用户手打出来的括号**：
/// 只有 `auto_closer` 认领过的那个闭括号才算我们补的，且它必须**确实**还在
/// 光标右侧（内容可能已被别处改动）；左侧那格必须是它的配对开括号，否则
/// `(x|)` 退格会把 `x` 一起删掉。
///
/// `typed` 不参与本判据（调用方按退格键时填什么都行）。
pub fn pair_deletes_whole_pair(p: &PairProbe) -> bool {
    if p.has_selection || p.composing || p.in_block || p.has_extra_cursors {
        return false;
    }
    let Some(close) = p.auto_closer else {
        return false;
    };
    if p.after != Some(close) {
        return false;
    }
    p.before == opener_for(close)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brackets::BRACKET_PAIRS;

    /// 一个"什么都不拦"的最小探针：各臂在它上面只加自己要钉的那一格。
    /// 只在测试里用（生产侧的探针由 app 层从文档现量填），所以不进主模块，
    /// 否则是一条死代码。
    fn bare(typed: char) -> PairProbe {
        PairProbe {
            typed,
            before: None,
            after: None,
            auto_closer: None,
            has_selection: false,
            overwrite: false,
            composing: false,
            in_block: false,
            has_extra_cursors: false,
        }
    }

    /// 右侧邻位决定补不补（D4）：末尾/空白/行界/标点都补，词字符不补。
    #[test]
    fn pair_action_requires_a_clean_right_neighbour() {
        for after in [None, Some(' '), Some('\n'), Some(','), Some(')')] {
            let p = PairProbe { after, ..bare('(') };
            assert_eq!(
                pair_action(&p),
                PairAction::InsertPair,
                "右侧 {after:?} 该补对"
            );
        }
        // 词字符挡住：中文正文里打括号不该凭空多出一个 `)`。
        for after in [Some('a'), Some('7'), Some('_'), Some('中')] {
            let p = PairProbe { after, ..bare('(') };
            assert_eq!(
                pair_action(&p),
                PairAction::Plain,
                "右侧 {after:?} 是词字符，不许补"
            );
        }
    }

    /// 五种抑制各钉一格：摘掉哪一条，只有对应那一格红。
    #[test]
    fn pairing_is_suppressed_by_selection_overtype_composition_block_and_multicursor() {
        // 一臂两问：同一抑制既要压住"补对"，也要压住"跳过"——
        // 列块态打 `)` 是逐行替换整列，不是把光标右移一格。
        macro_rules! arm {
            ($flag:ident, $label:expr) => {{
                let name = $label;
                let mut pair = bare('(');
                pair.$flag = true;
                assert_eq!(pair_action(&pair), PairAction::Plain, "{name}：不许补对");
                let mut skip = PairProbe {
                    auto_closer: Some(')'),
                    after: Some(')'),
                    ..bare(')')
                };
                skip.$flag = true;
                assert_eq!(pair_action(&skip), PairAction::Plain, "{name}：不许跳过");
            }};
        }
        arm!(has_selection, "有选区");
        arm!(overwrite, "覆写态");
        arm!(composing, "组字中");
        arm!(in_block, "列块态");
        arm!(has_extra_cursors, "多光标");
        // 反面控制：一个抑制都不加时，补对与跳过都成立（挡住"判据恒 Plain"这种假绿）。
        assert_eq!(
            pair_action(&bare('(')),
            PairAction::InsertPair,
            "前提：不加抑制时补对必须成立"
        );
        let clean = PairProbe {
            auto_closer: Some(')'),
            after: Some(')'),
            ..bare(')')
        };
        assert_eq!(
            pair_action(&clean),
            PairAction::SkipOver,
            "前提：不加抑制时跳过必须成立"
        );
    }

    /// 只认自己补的那一个（D5）：手打的 `()` 上按 `)` 仍是普通插入。
    #[test]
    fn typing_a_closer_skips_only_the_auto_closer_we_inserted() {
        let auto = PairProbe {
            auto_closer: Some(')'),
            after: Some(')'),
            ..bare(')')
        };
        assert_eq!(pair_action(&auto), PairAction::SkipOver);

        // 没有认领标记＝用户手打的闭括号，跳过它会让人没法把 `)` 盖上去。
        let hand = PairProbe {
            after: Some(')'),
            ..bare(')')
        };
        assert_eq!(pair_action(&hand), PairAction::Plain);

        // 标记还在、右侧字符已经不是它了（内容被别处改动）⇒ 重验必须挡住。
        let stale = PairProbe {
            auto_closer: Some(')'),
            after: Some('x'),
            ..bare(')')
        };
        assert_eq!(pair_action(&stale), PairAction::Plain);

        // 三种括号各自认领自己那一个：`{` 补的是 `}`，标记不会串台。
        let brace = PairProbe {
            auto_closer: Some('}'),
            after: Some('}'),
            ..bare('}')
        };
        assert_eq!(pair_action(&brace), PairAction::SkipOver);
        let wrong_mark = PairProbe {
            auto_closer: Some(')'),
            after: Some('}'),
            ..bare('}')
        };
        assert_eq!(pair_action(&wrong_mark), PairAction::Plain);
    }

    /// 整对删只认自动补的，且左侧必须是它的配对开括号（D6）。
    #[test]
    fn backspace_deletes_the_whole_pair_only_for_an_auto_pair() {
        let fresh = PairProbe {
            before: Some('('),
            after: Some(')'),
            auto_closer: Some(')'),
            ..bare('\u{8}')
        };
        assert!(pair_deletes_whole_pair(&fresh), "(|) 上退格删整对");

        // 手打的 `()`：没有认领标记 ⇒ 只删左边那一个。
        let hand = PairProbe {
            before: Some('('),
            after: Some(')'),
            ..bare('\u{8}')
        };
        assert!(
            !pair_deletes_whole_pair(&hand),
            "用户手打的右括号不许被退格顺带吃掉"
        );

        // `(x|)`：左侧不是开括号 ⇒ 只删 `x`。
        let content = PairProbe {
            before: Some('x'),
            after: Some(')'),
            auto_closer: Some(')'),
            ..bare('\u{8}')
        };
        assert!(!pair_deletes_whole_pair(&content));

        // 闭括号已被删掉/移走 ⇒ 标记陈旧也不作案。
        let gone = PairProbe {
            before: Some('('),
            after: None,
            auto_closer: Some(')'),
            ..bare('\u{8}')
        };
        assert!(!pair_deletes_whole_pair(&gone));

        // 列块与多光标下退格另有语义 ⇒ 一律不整对删。
        let block = PairProbe {
            in_block: true,
            ..fresh
        };
        assert!(!pair_deletes_whole_pair(&block));
        let multi = PairProbe {
            has_extra_cursors: true,
            ..fresh
        };
        assert!(!pair_deletes_whole_pair(&multi));
    }

    /// 两张表互不串台（守住跳转那一侧的既有断言）。
    #[test]
    fn auto_pair_table_does_not_leak_into_bracket_scan() {
        // v1 里两张表恰好同形，但**是两个常量**：加宽任何一张都得先过这一格。
        assert_eq!(AUTO_PAIRS.len(), BRACKET_PAIRS.len());
        for (open, close) in AUTO_PAIRS {
            assert_eq!(closer_for(open), Some(close), "开括号 {open} 的配对");
            assert_eq!(opener_for(close), Some(open), "闭括号 {close} 的配对");
        }
        // 歧义字符与引号两类都不在表里。
        for c in ['<', '>', '"', '\'', '`', '（', '「'] {
            assert_eq!(closer_for(c), None, "{c} 不参与自动配对");
            assert_eq!(opener_for(c), None, "{c} 不参与自动配对");
        }
        // 反向：自动配对的字符**必须**是跳转那一侧认得的括号，否则会出现
        // "补了个对却按 Ctrl+Shift+M 跳不过去"的怪状态。
        for (open, close) in AUTO_PAIRS {
            assert!(
                crate::brackets::bracket_kind(open).is_some()
                    && crate::brackets::bracket_kind(close).is_some(),
                "自动配对表里的 {open}{close} 必须也在跳转表内"
            );
        }
    }
}
