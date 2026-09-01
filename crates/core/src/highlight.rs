//! 逐行懒高亮 —— 「逐行高亮状态机」思想的检查点版本。
//!
//! syntect 的解析本质是顺序的；编辑器却需要**视口随机访问**。
//! 做法：每隔 [`STRIDE`] 行保存一个 `(ParseState, HighlightState)` 检查点，
//! 取第 N 行时从最近检查点最多前进 STRIDE 行；沿途的行文本由调用方
//! 通过闭包从 rope 文档即时供给（本模块不持有文本）。
//!
//! 失效策略：第 L 行发生编辑 ⇒ 从 L 起（含）所有行状态作废——
//! 检查点截断到 L/STRIDE+1 个，逐行缓存丢弃键 >= L 的条目。

use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

/// 检查点间距（行）。
pub const STRIDE: usize = 128;
/// 逐行状态缓存上限，超出即整体清空。
const LINE_CACHE_CAP: usize = 8192;
/// 浅色主题，配合编辑器的浅色背景。
const THEME_NAME: &str = "InspiredGitHub";

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

/// 行文本不含换行符，因此使用 nonewlines 变体的语法定义。
fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_nonewlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get(THEME_NAME)
            .cloned()
            .unwrap_or_else(|| themes.themes.values().next().cloned().expect("内置主题非空"))
    })
}

/// 一段同色文本：行内字符列区间 `[start_col, end_col)` 与前景色（RGBA 0..1）。
#[derive(Debug, Clone, PartialEq)]
pub struct StyledRun {
    pub start_col: usize,
    pub end_col: usize,
    pub color: [f32; 4],
}

type State = (ParseState, HighlightState);

/// 可选语言下的懒高亮器。
#[derive(Clone)]
pub struct LazyHighlighter {
    syntax_name: String,
    /// checkpoints[k] = 解析完第 `k*STRIDE - 1` 行后的状态；`[0]` 为初始态。
    checkpoints: Vec<State>,
    /// 行后状态缓存：key = 已解析完的行号。
    line_cache: HashMap<usize, State>,
    /// 补建时被垫付空行的起始行号（P23）：文档当时在此结束，档位内
    /// 之后的部分是垫付的。任何失效发生时，含垫付的检查点一并截掉，
    /// 杜绝「跳到文末 → 文档增长 → 从脏检查点续算」的错色。
    phantom_from: Option<usize>,
}

impl LazyHighlighter {
    /// 按文件扩展名构造；找不到对应语法则返回 None（调用方走纯文本路径）。
    pub fn new(extension: &str) -> Option<Self> {
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        let syntax = syntax_set().find_syntax_by_extension(&ext)?;
        let highlighter = Highlighter::new(theme());
        Some(Self {
            syntax_name: syntax.name.clone(),
            checkpoints: vec![(
                ParseState::new(syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
            )],
            line_cache: HashMap::new(),
            phantom_from: None,
        })
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// 第 `line_idx` 行（含）之后的状态全部作废。
    ///
    /// 除按行截断外，还把「补建时垫付过空行」的尾部检查点一并截掉
    /// （P23）：那些状态缺了文档后来长出来的真实行，续算会错色。
    /// 垫付只可能发生在最后一次补建的末档，重建成本 ≤ 一个档位。
    pub fn invalidate_from(&mut self, line_idx: usize) {
        let mut keep = line_idx / STRIDE + 1;
        if let Some(p) = self.phantom_from {
            // 检查点 k 覆盖 [k*STRIDE, (k+1)*STRIDE)；含垫付行的档位全部不要
            keep = keep.min((p / STRIDE).max(1));
        }
        self.checkpoints.truncate(keep.max(1));
        if let Some(p) = self.phantom_from.take() {
            // 行缓存里落在垫付区之后的条目同样不可信
            self.line_cache.retain(|k, _| *k < line_idx && *k < p);
        } else {
            self.line_cache.retain(|k, _| *k < line_idx);
        }
    }

    /// 计算第 `line_idx` 行的着色片段。
    ///
    /// * `total_lines`：当前文档总行数（调用方传 `Document::line_count()`）；
    /// * `target_text`：该行的不含换行文本；
    /// * `text_of(i)`：跨越未缓存中间行时按需索取第 i 行文本
    ///   （只需对 `i < total_lines` 有效）。
    ///
    /// 补建检查点时按整档位推进，但文档末尾未必对齐档位——越过
    /// `total_lines` 的部分以空行垫付（状态机语义等价于「文档在此后
    /// 没有内容」；文档增长时 `invalidate_from` 会先截掉这些垫付检查点，
    /// 不会污染新行的配色）。此前实现会拿越界行号去问调用方，触发
    /// ropey 的越界 panic（P23：>128 行的高亮文件跳到文末必崩）。
    pub fn styled_line(
        &mut self,
        line_idx: usize,
        target_text: &str,
        total_lines: usize,
        text_of: &mut dyn FnMut(usize) -> String,
    ) -> Vec<StyledRun> {
        let ss = syntax_set();
        let highlighter = Highlighter::new(theme());

        // 补齐缺失的检查点（大跳转时一次性补齐沿途所有档位）
        while self.checkpoints.len() * STRIDE <= line_idx {
            let start = self.checkpoints.len() * STRIDE;
            let (mut parse, mut highlight) =
                self.checkpoints.last().cloned().expect("初始检查点恒存在");
            let real_end = (start + STRIDE).min(total_lines);
            for i in start..real_end {
                advance(&mut parse, &mut highlight, &text_of(i), ss, &highlighter);
            }
            // 档位内越过文末的部分垫空行，保持「检查点 k = 第 k*STRIDE-1 行
            // 后状态」的不变量（供 invalidate_from 的整除算术使用）
            for _ in real_end..start + STRIDE {
                advance(&mut parse, &mut highlight, "", ss, &highlighter);
            }
            if real_end < start + STRIDE {
                // 记录垫付起点：下次失效时含垫付的档位会被整体截掉
                self.phantom_from = Some(real_end);
            }
            self.checkpoints.push((parse, highlight));
        }

        // 起点：上一行的后状态有缓存则零步直达；否则从最近检查点推进
        let sequential_hit = line_idx > 0 && self.line_cache.contains_key(&(line_idx - 1));
        let (mut parse, mut highlight) = if sequential_hit {
            self.line_cache[&(line_idx - 1)].clone()
        } else {
            let ck = line_idx / STRIDE;
            self.checkpoints[ck].clone()
        };
        let walk_from = if sequential_hit {
            line_idx
        } else {
            (line_idx / STRIDE) * STRIDE
        };
        for i in walk_from..line_idx {
            advance(&mut parse, &mut highlight, &text_of(i), ss, &highlighter);
        }

        // 解析目标行：HighlightIterator 直接产出 (Style, 文本片段)
        let ops = parse.parse_line(target_text, ss).unwrap_or_default();
        let regions =
            HighlightIterator::new(&mut highlight, &ops[..], target_text, &highlighter);

        let mut runs: Vec<StyledRun> = Vec::new();
        let mut char_pos = 0usize;
        for (style, segment) in regions {
            let start_col = char_pos;
            char_pos += segment.chars().count();
            if end_reached(start_col, char_pos) {
                continue;
            }
            let color = [
                style.foreground.r as f32 / 255.0,
                style.foreground.g as f32 / 255.0,
                style.foreground.b as f32 / 255.0,
                style.foreground.a as f32 / 255.0,
            ];
            match runs.last_mut() {
                Some(last) if last.color == color && last.end_col == start_col => {
                    last.end_col = char_pos;
                }
                _ => runs.push(StyledRun {
                    start_col,
                    end_col: char_pos,
                    color,
                }),
            }
        }

        if self.line_cache.len() >= LINE_CACHE_CAP {
            self.line_cache.clear();
        }
        self.line_cache.insert(line_idx, (parse, highlight));

        runs
    }
}

fn end_reached(start: usize, end: usize) -> bool {
    end <= start
}

/// 推进一行状态：解析并驱动高亮状态（丢弃着色结果）。
fn advance(
    parse: &mut ParseState,
    highlight: &mut HighlightState,
    line: &str,
    ss: &SyntaxSet,
    highlighter: &Highlighter,
) {
    let ops = parse.parse_line(line, ss).unwrap_or_default();
    for _ in HighlightIterator::new(highlight, &ops[..], line, highlighter) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_extension_yields_none() {
        assert!(LazyHighlighter::new("不存在的扩展名").is_none());
        assert!(LazyHighlighter::new("rs").is_some());
    }

    #[test]
    fn rust_line_is_split_into_multiple_colors() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let runs = hl.styled_line(0, "fn main() { let x = 1; }", usize::MAX, &mut |_| String::new());
        assert!(runs.len() >= 3, "关键字与标识符应分色，实际 {runs:?}");
        assert_eq!(runs.first().unwrap().start_col, 0);
        for pair in runs.windows(2) {
            assert_eq!(pair[0].end_col, pair[1].start_col);
        }
        assert_eq!(runs.last().unwrap().end_col, "fn main() { let x = 1; }".chars().count());
    }

    #[test]
    fn comment_state_carries_to_next_line() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let line0 = hl.styled_line(0, "/* 块注释开始", usize::MAX, &mut |_| String::new());
        let mut served: Vec<usize> = Vec::new();
        let line1 = hl.styled_line(1, "仍在注释 */ let x;", usize::MAX, &mut |i| {
            served.push(i);
            String::new()
        });

        assert!(!line0.is_empty());
        assert_eq!(line0[0].color, line1[0].color, "第 1 行开头应延续注释配色");
        assert!(line1.len() >= 2, "注释结束后应有不同色的代码段，实际 {line1:?}");
        assert!(served.is_empty(), "第 0 行已有缓存，不应回索取中间行文本");
    }

    #[test]
    fn invalidate_from_truncates_state() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let _ = hl.styled_line(300, "let a;", usize::MAX, &mut |i| format!("let filler{i} = {i};"));
        assert!(!hl.line_cache.is_empty());

        hl.invalidate_from(150);
        assert!(hl.line_cache.iter().all(|(k, _)| *k < 150));

        let runs =
            hl.styled_line(160, "let b;", usize::MAX, &mut |i| format!("let f{i} = {i};"));
        assert!(!runs.is_empty());
    }

    // ---------- P23：文末不对齐档位时补建不得越界 ----------

    #[test]
    fn jump_to_last_line_of_unaligned_document_does_not_panic() {
        // 行数刻意取非 STRIDE 整倍数（300 = 2×128 + 44）：旧实现补建
        // 第 2 个档位时会拿 256..384 的行号去问文本，越界 panic。
        for total in [129usize, 255, 300, 1000] {
            let last = total - 1;
            let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
            let mut asked_max = 0usize;
            let runs = hl.styled_line(
                last,
                "let tail = 1;",
                total,
                &mut |i| {
                    asked_max = asked_max.max(i);
                    format!("let filler{i} = {i};")
                },
            );
            assert!(
                !runs.is_empty(),
                "total={total} 文末行应有正常配色"
            );
            assert!(
                asked_max < total,
                "total={total} 闭包只该被问有效行号，最大被问到 {asked_max}"
            );
        }
    }

    #[test]
    fn padded_tail_checkpoints_do_not_leak_into_grown_document() {
        // 先在小文档上跳到末行（产生垫付空行的尾部检查点），
        // 再模拟文档增长：invalidate_from 截断后新行配色仍正确延续
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let _ = hl.styled_line(200, "let a = 1;", 201, &mut |i| format!("let f{i} = {i};"));

        // 文档长到 400 行，用户在第 350 行编辑 → 从 350 起失效
        hl.invalidate_from(350);
        let runs = hl.styled_line(360, "/* 尾部注释", 401, &mut |i| {
            if i == 359 {
                "/* 头部注释".to_owned()
            } else {
                format!("let g{i} = {i};")
            }
        });
        assert_eq!(
            runs.first().unwrap().color,
            hl_comment_color(&mut hl, 361),
            "增长后注释配色必须跨行延续（垫付检查点不得残留）"
        );
    }

    /// 辅助：单独解析下一行取首段颜色，验证状态连续性。
    fn hl_comment_color(hl: &mut LazyHighlighter, line: usize) -> [f32; 4] {
        let runs = hl.styled_line(line, "仍在注释 */", usize::MAX, &mut |_| String::new());
        runs.first().unwrap().color
    }
}
