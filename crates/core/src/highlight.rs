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
        })
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// 第 `line_idx` 行（含）之后的状态全部作废。
    pub fn invalidate_from(&mut self, line_idx: usize) {
        let keep = line_idx / STRIDE + 1;
        self.checkpoints.truncate(keep.max(1));
        self.line_cache.retain(|k, _| *k < line_idx);
    }

    /// 计算第 `line_idx` 行的着色片段。
    ///
    /// * `target_text`：该行的不含换行文本；
    /// * `text_of(i)`：跨越未缓存中间行时按需索取第 i 行文本。
    pub fn styled_line(
        &mut self,
        line_idx: usize,
        target_text: &str,
        text_of: &mut dyn FnMut(usize) -> String,
    ) -> Vec<StyledRun> {
        let ss = syntax_set();
        let highlighter = Highlighter::new(theme());

        // 补齐缺失的检查点（大跳转时一次性补齐沿途所有档位）
        while self.checkpoints.len() * STRIDE <= line_idx {
            let start = self.checkpoints.len() * STRIDE;
            let (mut parse, mut highlight) =
                self.checkpoints.last().cloned().expect("初始检查点恒存在");
            for i in start..start + STRIDE {
                advance(&mut parse, &mut highlight, &text_of(i), ss, &highlighter);
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
        let runs = hl.styled_line(0, "fn main() { let x = 1; }", &mut |_| String::new());
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
        let line0 = hl.styled_line(0, "/* 块注释开始", &mut |_| String::new());
        let line1 = hl.styled_line(1, "仍在注释 */ let x;", &mut |_| String::new());

        assert!(!line0.is_empty());
        assert_eq!(line0[0].color, line1[0].color, "第 1 行开头应延续注释配色");
        assert!(line1.len() >= 2, "注释结束后应有不同色的代码段，实际 {line1:?}");
    }

    #[test]
    fn invalidate_from_truncates_state() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let _ = hl.styled_line(300, "let a;", &mut |i| format!("let filler{i} = {i};"));
        assert!(!hl.line_cache.is_empty());

        hl.invalidate_from(150);
        assert!(hl.line_cache.iter().all(|(k, _)| *k < 150));

        let runs = hl.styled_line(160, "let b;", &mut |i| format!("let f{i} = {i};"));
        assert!(!runs.is_empty());
    }
}
