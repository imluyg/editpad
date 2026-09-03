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
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use syntect::highlighting::{
    HighlightIterator, HighlightState, Highlighter, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxDefinition, SyntaxSet};

use crate::syntaxes::{LOG, TOML};

/// 检查点间距（行）。
pub const STRIDE: usize = 128;
/// 逐行状态缓存上限，超出即整体清空。
const LINE_CACHE_CAP: usize = 8192;
/// 浅色主题，配合编辑器的浅色背景。
const THEME_NAME: &str = "InspiredGitHub";

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();
/// 高亮器代次的全局单调计数（P12）。
///
/// 代次必须跨实例单调：`set_language`/重新打开文件会整体换掉高亮器，
/// 若每个实例都从 0 起算，旧实例在途的后台补建结果（代次 0）会被
/// 新实例（也是代次 0）误认成自己的。全局计数器保证不重号。
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// 行文本不含换行符，因此使用 nonewlines 变体的语法定义。
fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(|| {
        // P22 第一批：默认包 + 内嵌迷你语法（Log/TOML）合并初始化，
        // 仍走本 OnceLock 懒加载路径，不破坏检查点机制与启动开销模型
        let mut builder = SyntaxSet::load_defaults_nonewlines().into_builder();
        for (yaml, fallback) in [(LOG, "Editpad Log"), (TOML, "Editpad TOML")] {
            match SyntaxDefinition::load_from_str(yaml, false, Some(fallback)) {
                Ok(def) => builder.add(def),
                // 内嵌语法写错只损失该格式配色，绝不拖垮编辑器
                Err(error) => eprintln!("内嵌语法 {fallback} 加载失败: {error}"),
            }
        }
        builder.build()
    })
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
pub struct LazyHighlighter {    syntax_name: String,
    /// checkpoints[k] = 解析完第 `k*STRIDE - 1` 行后的状态；`[0]` 为初始态。
    checkpoints: Vec<State>,
    /// 行后状态缓存：key = 已解析完的行号。
    line_cache: HashMap<usize, State>,
    /// P61 渐进上色：可视区**近似**行后状态缓存（key = 行号）。
    /// 与精确 `line_cache` 严格隔离——近似状态绝不被精确路径消费，
    /// 精确铺建到达后调用方自然改用精确结果，近似条目沦为死键
    /// （容量超限整体清空）。任何失效（编辑）整体清空。
    approx_line_cache: HashMap<usize, State>,
    /// 补建时被垫付空行的起始行号（P23）：文档当时在此结束，档位内
    /// 之后的部分是垫付的。任何失效发生时，含垫付的检查点一并截掉，
    /// 杜绝「跳到文末 → 文档增长 → 从脏检查点续算」的错色。
    phantom_from: Option<usize>,
    /// 高亮器代次（P12）：任何失效（编辑）都会换新号。后台分批补建
    /// 的在途结果按它过滤——代次不符即整体丢弃，杜绝拿旧文档状态
    /// 覆盖新文档。全局单调计数保证跨实例也不重号。
    generation: u64,
}

// ParseState/HighlightState 在 fancy-regex 后端下均为纯数据字段
// （Vec/Option/String/ScopeStack），可安全跨线程搬运——后台补建依赖
// 此性质；此处编译期钉住，后端更换时第一时间暴露。
#[cfg(test)]
#[allow(dead_code)]
fn _assert_highlighter_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<LazyHighlighter>();
}

impl std::fmt::Debug for LazyHighlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 内部状态体积可观且无调试价值，只输出摘要
        f.debug_struct("LazyHighlighter")
            .field("syntax_name", &self.syntax_name)
            .field("checkpoints", &self.checkpoints.len())
            .field("cached_lines", &self.line_cache.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl LazyHighlighter {
    /// 按文件扩展名构造；找不到对应语法则返回 None（调用方走纯文本路径）。
    pub fn new(extension: &str) -> Option<Self> {
        let ext = extension.trim_start_matches('.').to_ascii_lowercase();
        let syntax = syntax_set().find_syntax_by_extension(&ext)?;
        Some(Self::from_syntax(syntax.clone()))
    }

    /// 按语法名构造（P22）：别名层与嗅探层的落点，
    /// 名字必须来自 [`syntax_for_extension`] / [`sniff_language`] / 内置索引。
    pub fn new_by_name(name: &str) -> Option<Self> {
        let syntax = syntax_set().find_syntax_by_name(name)?;
        Some(Self::from_syntax(syntax.clone()))
    }

    fn from_syntax(syntax: syntect::parsing::SyntaxReference) -> Self {
        let highlighter = Highlighter::new(theme());
        Self {
            syntax_name: syntax.name.clone(),
            checkpoints: vec![(
                ParseState::new(&syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
            )],
            line_cache: HashMap::new(),
            approx_line_cache: HashMap::new(),
            phantom_from: None,
            generation: next_generation(),
        }
    }

    pub fn syntax_name(&self) -> &str {
        &self.syntax_name
    }

    /// 当前代次（P12）：后台补建结果按它过滤。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 已保存的检查点数（含初始态 [0]）。
    pub fn checkpoints_len(&self) -> usize {
        self.checkpoints.len()
    }

    /// 覆盖第 `line_idx` 行还缺几个检查点档位（0 = 已就绪）。
    pub fn strides_missing(&self, line_idx: usize) -> usize {
        (line_idx / STRIDE + 1).saturating_sub(self.checkpoints.len())
    }

    /// 是否还有「真实行组成的完整档位」可建（P12）：后台补建是否还有活干。
    ///
    /// 文档尾部不足一档的残余永远不算——那部分留给同步路径按需处理
    /// （≤1 档，成本与日常随机访问相同），因此本值为 false 即代表
    /// 后台分批可以收工。
    pub fn needs_background_pave(&self, total_lines: usize) -> bool {
        let start = self.checkpoints.len() * STRIDE;
        start + STRIDE <= total_lines
    }

    /// 第 `line_idx` 行（含）之后的状态全部作废。
    ///
    /// 除按行截断外，还把「补建时垫付过空行」的尾部检查点一并截掉
    /// （P23）：那些状态缺了文档后来长出来的真实行，续算会错色。
    /// 垫付只可能发生在最后一次补建的末档，重建成本 ≤ 一个档位。
    pub fn invalidate_from(&mut self, line_idx: usize) {
        // P12：编辑即换代——在途的后台补建结果回来后对不上号，整体丢弃
        self.generation = next_generation();
        // P61：近似状态全部作废（基于旧文档内容，且与精确路径隔离的
        // 独立缓存——整体清空最简单，可视区会在后续帧重新近似）
        self.approx_line_cache.clear();
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

    /// 内联补建预算：调用线程（UI）单次最多现算的缺失档位数。
    ///
    /// 一个档位 = STRIDE 行 ≈ 10ms 量级（实测 75µs/行），是既有热路径
    /// 的可接受上限；超过即说明用户做了大跳转，必须降级 + 转后台分批。
    pub const MAX_INLINE_STRIDES: usize = 1;

    /// 与 [`styled_line`](Self::styled_line) 相同，但补建预算受限（P12）。
    ///
    /// 缺失档位数超过 `max_inline_strides` 时**不做任何补建**，返回
    /// `None`——调用方应以无色渲染该行并安排后台分批补建，而不是在
    /// UI 线程上一次性冻结数十秒。
    pub fn styled_line_limited(
        &mut self,
        line_idx: usize,
        target_text: &str,
        total_lines: usize,
        max_inline_strides: usize,
        text_of: &mut dyn FnMut(usize) -> String,
    ) -> Option<Vec<StyledRun>> {
        if self.strides_missing(line_idx) > max_inline_strides {
            return None;
        }
        Some(self.styled_line(line_idx, target_text, total_lines, text_of))
    }

    /// P61 渐进上色：可视区**近似**上色——从 `anchor` 行的全新语法状态
    /// 出发推进到 `line_idx`（沿途行状态进 `approx_line_cache`，连续滚动
    /// 时逐行 O(1) 延续），解析目标行并缓存终态。
    ///
    /// 与精确路径的关系：**不建检查点、不读写 `line_cache`**——近似状态
    /// 存独立缓存，绝不会污染精确路径（[`Self::styled_line`] 的
    /// sequential_hit 只看 line_cache）。取舍：多行构造（跨行注释/
    /// 字符串）跨 `anchor` 的前几行可能错色；后台精确铺建到达该行后，
    /// 调用方优先取精确结果自然替换。
    ///
    /// * `anchor`：近似状态起点（调用方传可视区首行）；`line_idx <
    ///   anchor` 时按 `line_idx == anchor` 处理（全新状态直解目标行）。
    /// * 成本：连续滚动每新行一次 parse_line（≈75µs 量级）；跳转后首帧
    ///   最多一个视口的行推进（几十行 × 75µs，远小于 UI 冻结阈值）。
    pub fn styled_line_approx(
        &mut self,
        line_idx: usize,
        anchor: usize,
        target_text: &str,
        total_lines: usize,
        text_of: &mut dyn FnMut(usize) -> String,
    ) -> Vec<StyledRun> {
        let ss = syntax_set();
        let highlighter = Highlighter::new(theme());
        let syntax = syntax_set()
            .find_syntax_by_name(&self.syntax_name)
            .expect("语法名来自构造期，必然存在");

        // 起点：上一行的近似终态（连续滚动逐行延续）；否则从 anchor 的
        // 全新语法状态推进 anchor..line_idx（行文本按需索取）
        let (mut parse, mut highlight) = if line_idx > 0 {
            match self.approx_line_cache.get(&(line_idx - 1)) {
                Some(state) => state.clone(),
                None => {
                    let mut state = (
                        ParseState::new(syntax),
                        HighlightState::new(&highlighter, ScopeStack::new()),
                    );
                    let from = anchor.min(line_idx);
                    for i in from..line_idx {
                        if i >= total_lines {
                            break;
                        }
                        advance(&mut state.0, &mut state.1, &text_of(i), ss, &highlighter);
                    }
                    state
                }
            }
        } else {
            (
                ParseState::new(syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
            )
        };

        // 解析目标行：与精确路径同一产出管线
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

        // 终态入近似缓存（下一行延续）；容量超限整体清空（同 line_cache）
        if self.approx_line_cache.len() >= LINE_CACHE_CAP {
            self.approx_line_cache.clear();
        }
        self.approx_line_cache.insert(line_idx, (parse, highlight));

        runs
    }

    /// 从当前检查点继续向后补建，一次最多 `max_strides` 个档位（P12）。
    ///
    /// 只用**真实存在**的行、只建**完整**的档位——起点越过文档末尾或
    /// 剩余不足一个整档位时停止（返回已建数量），因此本方法永远不会
    /// 产生垫付空行、不触碰 [`phantom_from`](Self::phantom_from) 语义；
    /// 文档尾部不足一档的部分留给同步路径按需处理（≤1 档，成本与
    /// 日常随机访问相同）。
    ///
    /// 这是后台分批补建的推进原语：每批调用一次，批间可检查取消标志、
    /// 上报进度。不改变代次——补建不是失效。
    pub fn advance_checkpoints(
        &mut self,
        max_strides: usize,
        total_lines: usize,
        text_of: &mut dyn FnMut(usize) -> String,
    ) -> usize {
        let ss = syntax_set();
        let highlighter = Highlighter::new(theme());
        let mut built = 0usize;
        while built < max_strides {
            let start = self.checkpoints.len() * STRIDE;
            if start + STRIDE > total_lines {
                break; // 剩余不足一个完整档位：交给同步路径
            }
            let (mut parse, mut highlight) =
                self.checkpoints.last().cloned().expect("初始检查点恒存在");
            for i in start..start + STRIDE {
                advance(&mut parse, &mut highlight, &text_of(i), ss, &highlighter);
            }
            self.checkpoints.push((parse, highlight));
            built += 1;
        }
        built
    }
}

fn end_reached(start: usize, end: usize) -> bool {
    end <= start
}

// ---------- 语言解析：扩展名别名 + 无扩展名嗅探（P22 第一批） ----------

/// 内置扩展索引未覆盖时的自维护别名表：扩展名 → 语法名。
///
/// 顺序无关（全表精确匹配）；ini/cfg/conf 借道 TOML 语法只是
/// 「节名+键值+字符串」的近似着色（`;` 注释不识别），已知取舍。
const ALIAS_TABLE: &[(&str, &str)] = &[
    ("log", "Editpad Log"),
    ("markdown", "Markdown"),
    ("md", "Markdown"),
    ("toml", "Editpad TOML"),
    ("ini", "Editpad TOML"),
    ("cfg", "Editpad TOML"),
    ("conf", "Editpad TOML"),
    ("mk", "Makefile"),
    ("yml", "YAML"),
    ("dockerfile", "Dockerfile"),
    ("txt", "Plain Text"),
];

/// 扩展名 → 语法名：先查 syntect 内建扩展索引，未命中再查别名表。
pub fn syntax_for_extension(ext: &str) -> Option<String> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if let Some(syntax) = syntax_set().find_syntax_by_extension(&ext) {
        return Some(syntax.name.clone());
    }
    ALIAS_TABLE
        .iter()
        .find(|(alias, _)| *alias == ext)
        .map(|(_, name)| name.to_string())
}

/// shebang 解释器名 → 语法名候选（按序取第一个内置存在的）。
/// 名字以 syntect 默认包实测为准（Bash 语法的实名是
/// 「Bourne Again Shell (bash)」，另有 Shell-Unix-Generic 兜底）。
fn shebang_candidates(base: &str) -> &'static [&'static str] {
    match base {
        "bash" | "sh" | "zsh" | "dash" => &["Bourne Again Shell (bash)", "Shell-Unix-Generic"],
        "python" | "python3" | "python2" => &["Python"],
        "perl" => &["Perl"],
        "ruby" => &["Ruby"],
        "node" | "nodejs" => &["JavaScript"],
        _ => &[],
    }
}

/// 无扩展名 / 扩展名无解时按内容嗅探语法（P22 第一批）：
/// shebang、`<?xml`、JSON 前缀启发、YAML 文档分隔符、
/// Dockerfile/Makefile 约定文件名。只读样本前 4KB。
pub fn sniff_language(sample: &str, file_name: Option<&str>) -> Option<String> {
    // 约定文件名优先于内容（大小写不敏感）
    if let Some(name) = file_name {
        match name.to_ascii_lowercase().as_str() {
            "dockerfile" => return Some("Dockerfile".to_owned()),
            "makefile" | "gnumakefile" | "bsdmakefile" => return Some("Makefile".to_owned()),
            _ => {}
        }
    }

    let head = sample.get(..4096).unwrap_or(sample);
    let trimmed = head.trim_start();

    // shebang：取解释器路径末段（剥 env、剥 .exe）
    if let Some(rest) = trimmed.strip_prefix("#!") {
        let mut tokens = rest.split_whitespace();
        let mut interp = tokens.next().unwrap_or("").to_ascii_lowercase();
        if interp == "env" || interp.ends_with("/env") || interp.ends_with("\\env") {
            interp = tokens.next().unwrap_or_default().to_ascii_lowercase();
        }
        let base = interp.rsplit(['/', '\\']).next().unwrap_or("");
        let base = base.strip_suffix(".exe").unwrap_or(base);
        for candidate in shebang_candidates(base) {
            if let Some(syntax) = syntax_set().find_syntax_by_name(candidate) {
                return Some(syntax.name.clone());
            }
        }
        return None;
    }

    // XML 声明
    if trimmed.starts_with("<?xml") {
        return Some("XML".to_owned());
    }

    // YAML 文档分隔符开头
    if let Some(rest) = trimmed.strip_prefix("---") {
        let after = rest.chars().next();
        if after.is_none() || after == Some('\n') || after == Some('\r') {
            return Some("YAML".to_owned());
        }
    }

    // JSON 前缀启发：首非空白是 { 或 [ 且样本同时含引号与冒号
    if let Some(first) = trimmed.chars().next() {
        if (first == '{' || first == '[')
            && head.contains('"')
            && head.contains(':')
        {
            return Some("JSON".to_owned());
        }
    }

    None
}

/// 统一入口：给定路径（可空）与解码后文本样本，返回应使用的语法名。
/// 顺序：约定文件名/内容嗅探仅在扩展名无解时介入；扩展名命中即返回。
pub fn resolve_language(path: Option<&Path>, sample: &str) -> Option<String> {
    if let Some(path) = path {
        if let Some(ext) = path.extension().and_then(std::ffi::OsStr::to_str) {
            if let Some(name) = syntax_for_extension(ext) {
                return Some(name);
            }
        }
    }
    let file_name = path.and_then(Path::file_name).and_then(std::ffi::OsStr::to_str);
    sniff_language(sample, file_name)
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

    // ---------- P12：有限补建 + 后台分批 ----------

    #[test]
    fn strides_missing_drives_limited_degrade_contract() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let filler = &mut |i: usize| format!("let v{i} = {i};");

        // 初始只有检查点 [0]：第 100 行落在 [0,128) 内已覆盖 → 缺 0 档
        assert_eq!(hl.strides_missing(100), 0);
        // 第 200 行需要检查点 [1] → 缺 1 档，恰在预算内：内联现算成功
        assert_eq!(hl.strides_missing(200), 1);
        let runs = hl
            .styled_line_limited(200, "let x = 1;", 600, LazyHighlighter::MAX_INLINE_STRIDES, filler)
            .expect("缺 1 档属于预算内，必须内联现算");
        assert!(!runs.is_empty());

        // 第 500 行需 4 个档位、现有 2 个 → 缺 2 超预算 → 拒绝内联
        assert_eq!(hl.strides_missing(500), 2);
        assert!(
            hl.styled_line_limited(500, "let y = 2;", 600, LazyHighlighter::MAX_INLINE_STRIDES, filler)
                .is_none(),
            "超预算必须返回 None（降级），不得在调用线程大段补建"
        );
    }

    #[test]
    fn advance_checkpoints_builds_full_strides_and_stops_at_document_edge() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        // 300 行：初始检查点已覆盖 [0,128)，完整档位只剩 [128,256) 一个；
        // 起点 256 的档位越界（256+128=384 > 300）不得建
        let total = 300usize;
        let built = hl.advance_checkpoints(10, total, &mut |i| format!("let f{i} = {i};"));
        assert_eq!(built, 1, "只应建完整档位，实际建了 {built}");
        assert!(
            !hl.needs_background_pave(total),
            "残余不足一档时应判定后台无活可干"
        );
        assert!(hl.phantom_from.is_none(), "后台推进绝不产生垫付空行");

        // 再推进无活可干
        assert_eq!(hl.advance_checkpoints(10, total, &mut |i| format!("{i}")), 0);

        // 残余档位在预算内直接可取（末行走同步路径 ≤1 档），且不 panic
        let runs = hl
            .styled_line_limited(
                total - 1,
                "let tail = 1;",
                total,
                LazyHighlighter::MAX_INLINE_STRIDES,
                &mut |i| format!("let f{i} = {i};"),
            )
            .expect("残余不足预算时末行不应再要求降级");
        assert!(!runs.is_empty());
    }

    #[test]
    fn styled_line_result_identical_whether_paved_inline_or_in_background() {
        // 同一目标行的配色：同步全量补建 vs 后台分批铺满后取用，必须一致
        let total = 700usize;
        let text_of = |i: usize| format!("fn f{i}(x: f64) -> f64 {{ x /* 注释 */ }}");

        let mut sync_hl = LazyHighlighter::new("rs").unwrap();
        let sync_runs = sync_hl.styled_line(total - 1, "let a = 1;", total, &mut |i| text_of(i));

        let mut bg_hl = LazyHighlighter::new("rs").unwrap();
        while bg_hl.advance_checkpoints(2, total, &mut |i| text_of(i)) > 0 {}
        let bg_runs = bg_hl.styled_line(total - 1, "let a = 1;", total, &mut |i| text_of(i));

        assert_eq!(sync_runs, bg_runs, "后台分批铺建的检查点必须产出与同步路径相同的配色");
    }

    #[test]
    fn generation_bumps_on_invalidate_only_and_is_globally_monotonic() {
        let mut hl = LazyHighlighter::new("rs").unwrap();
        let g0 = hl.generation();
        let _ = hl.styled_line(10, "let a;", usize::MAX, &mut |i| format!("{i}"));
        let built = hl.advance_checkpoints(1, usize::MAX, &mut |i| format!("{i}"));
        assert_eq!(built, 1);
        assert_eq!(hl.generation(), g0, "读取/补建都不是失效，不得换代");

        hl.invalidate_from(5);
        assert!(hl.generation() > g0, "编辑失效必须换代（在途结果据此丢弃）");

        // 全局计数器：新实例不与旧实例重号（换语言/换文档场景的正确性前提）
        let fresh = LazyHighlighter::new("rs").unwrap();
        assert!(fresh.generation() > hl.generation(), "代次跨实例单调递增");
    }

    // ---------- P22 第一批：别名层 + 内嵌语法 + 嗅探 ----------

    #[test]
    fn builtin_extensions_win_before_alias_table() {
        // 内建索引命中：直接给规范语法名
        for (ext, name) in [("rs", "Rust"), ("py", "Python"), ("json", "JSON"), ("yml", "YAML")] {
            assert_eq!(syntax_for_extension(ext).as_deref(), Some(name), "{ext}");
        }
        // 别名表兜底
        for (ext, name) in [
            ("log", "Editpad Log"),
            ("toml", "Editpad TOML"),
            ("md", "Markdown"),
            ("markdown", "Markdown"),
            ("ini", "Editpad TOML"),
            ("cfg", "Editpad TOML"),
            ("conf", "Editpad TOML"),
            ("mk", "Makefile"),
            ("txt", "Plain Text"),
        ] {
            assert_eq!(syntax_for_extension(ext).as_deref(), Some(name), "{ext}");
        }
        // 完全未知 → None（交由嗅探）
        assert_eq!(syntax_for_extension("xyzzy"), None);
        // 大小写与前置点归一
        assert_eq!(syntax_for_extension(".RS").as_deref(), Some("Rust"));
    }

    #[test]
    fn embedded_log_and_toml_syntaxes_are_registered_and_color_levels() {
        let set = syntax_set();
        assert!(set.find_syntax_by_name("Editpad Log").is_some());
        assert!(set.find_syntax_by_name("Editpad TOML").is_some());

        // Log：ERROR 行与 INFO 行的首段颜色必须可区分（级别分色的验收口径）
        let mut log_hl = LazyHighlighter::new_by_name("Editpad Log").expect("Log 语法存在");
        let error_runs = log_hl.styled_line(0, "[2026-08-24 10:00:00] ERROR boom", 1, &mut |_| String::new());
        let info_runs = log_hl.styled_line(1, "[2026-08-24 10:00:01] INFO fine", 1, &mut |_| String::new());
        assert!(!error_runs.is_empty() && !info_runs.is_empty());
        // 级别分色的验收口径：两行全部着色段的颜色序列必须不同
        // （ERROR 行含 invalid 红色系，INFO 行只有 string/numeric 系）
        let err_colors: Vec<_> = error_runs.iter().map(|r| r.color).collect();
        let info_colors: Vec<_> = info_runs.iter().map(|r| r.color).collect();
        assert_ne!(
            err_colors, info_colors,
            "ERROR 与 INFO 行应呈现不同配色（级别词分色）"
        );

        // TOML：注释/节名/字符串至少产出多段配色
        let mut toml_hl = LazyHighlighter::new_by_name("Editpad TOML").expect("TOML 语法存在");
        let runs = toml_hl.styled_line(
            0,
            "# 注释\n[section]\nkey = \"value\"",
            3,
            &mut |_| String::new(),
        );
        assert!(runs.len() >= 2, "TOML 应产出分段配色");
    }

    #[test]
    fn sniffing_covers_shebang_xml_json_yaml_and_convention_names() {
        // shebang 各解释器（名字以默认包实名为准）
        for (sample, expected) in [
            ("#!/bin/bash\nset -e", "Bourne Again Shell (bash)"),
            ("#! /usr/bin/env python3\nprint(1)", "Python"),
            ("#!/usr/bin/perl -w\n", "Perl"),
            ("#!/usr/bin/ruby\n", "Ruby"),
            ("#!C:\\tools\\node.exe\n", "JavaScript"),
        ] {
            assert_eq!(
                sniff_language(sample, None).as_deref(),
                Some(expected),
                "{sample}"
            );
        }
        // 未识别的 shebang → None
        assert_eq!(sniff_language("#!/opt/weird/run", None), None);

        // XML / JSON / YAML
        assert_eq!(
            sniff_language("<?xml version=\"1.0\"?>\n<root/>", None).as_deref(),
            Some("XML")
        );
        assert_eq!(
            sniff_language("{\"a\": [1, 2], \"b\": \"中文\"}", None).as_deref(),
            Some("JSON")
        );
        assert_eq!(
            sniff_language("[{\"k\": 1}]", None).as_deref(),
            Some("JSON")
        );
        assert_eq!(sniff_language("---\ntitle: x\n", None).as_deref(), Some("YAML"));

        // 约定文件名优先且大小写不敏感
        assert_eq!(
            sniff_language("", Some("DockerFile")).as_deref(),
            Some("Dockerfile")
        );
        assert_eq!(
            sniff_language("", Some("Makefile")).as_deref(),
            Some("Makefile")
        );

        // 普通无扩展名内容 → None
        assert_eq!(sniff_language("just some words\n", None), None);
    }

    #[test]
    fn resolve_language_prefers_extension_then_sniff_fallback() {
        use std::path::Path;

        // 扩展名命中即返回，不再嗅探（.log 内容是 JSON 也按 Log 处理——扩展名可信）
        let p = Path::new("logs/app.log");
        assert_eq!(
            resolve_language(Some(p), "{\"level\": 1}").as_deref(),
            Some("Editpad Log")
        );

        // 无扩展名 + shebang → 嗅探接管
        let p = Path::new("build");
        assert_eq!(
            resolve_language(Some(p), "#!/bin/sh\necho hi").as_deref(),
            Some("Bourne Again Shell (bash)")
        );

        // 有扩展名但完全未知 → 嗅探仍有机会（约定文件名场景之外的内容嗅探）
        let p = Path::new("artifact.xyzzy");
        assert_eq!(resolve_language(Some(p), "plain"), None);
    }

    // ---------- P61 渐进上色（可视区近似上色） ----------

    #[test]
    fn styled_line_approx_colors_without_touching_checkpoints() {
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let total = 2000usize; // 远超内联预算（跳到文中未铺建区的等价物）
        let text_of = &mut |i: usize| format!("let v{i} = {i};");
        assert!(
            hl.strides_missing(1500) > LazyHighlighter::MAX_INLINE_STRIDES,
            "前置：目标行必须超出内联预算"
        );
        let before = hl.checkpoints_len();

        // 近似上色：可视区（anchor = 首个可见行）内的行
        let runs = hl.styled_line_approx(
            1500,
            1490,
            "let v1500 = 1500;",
            total,
            text_of,
        );
        assert!(!runs.is_empty(), "近似上色必须产出着色片段");
        assert_eq!(
            hl.checkpoints_len(),
            before,
            "近似路径不得创建检查点（与精确铺建严格隔离）"
        );

        // 连续下一行：经近似缓存逐行延续，仍产出片段
        let runs2 = hl.styled_line_approx(
            1501,
            1490,
            "let v1501 = 1501;",
            total,
            text_of,
        );
        assert!(!runs2.is_empty());
    }

    #[test]
    fn approx_does_not_poison_exact_path() {
        // 关键契约：近似状态存独立缓存，绝不污染精确路径——
        // 先近似、后精确的结果必须与「从未近似」的对照高亮器完全一致
        let doc: Vec<String> = (0..600)
            .map(|i| format!("let v{i} = {i}; // 注释 {i}"))
            .collect();
        let total = doc.len();
        let text_of = &mut |i: usize| doc[i].clone();

        let mut approxed = LazyHighlighter::new("rs").expect("rust 语法存在");
        for line in [300usize, 301, 302] {
            approxed.styled_line_approx(line, 300, &doc[line], total, text_of);
        }

        // 精确路径：预算内的行（100）+ 跨近似区的行（310，精确补建会
        // 途经近似过的 300..=302——必须走检查点状态而非近似缓存）
        let runs_100 = approxed.styled_line(100, &doc[100], total, text_of);
        let runs_310 = approxed.styled_line(310, &doc[310], total, text_of);

        let mut control = LazyHighlighter::new("rs").expect("rust 语法存在");
        let ctrl_100 = control.styled_line(100, &doc[100], total, text_of);
        let ctrl_310 = control.styled_line(310, &doc[310], total, text_of);

        assert_eq!(runs_100, ctrl_100, "预算内精确结果不受近似影响");
        assert_eq!(runs_310, ctrl_310, "跨近似区的精确结果不得被污染");
    }

    #[test]
    fn approx_state_continues_across_rows() {
        // 跨行块注释：第二行的近似上色应延续第一行的注释态
        // （若无状态延续，第二行按全新状态解析会得到普通代码色）
        let doc_lines = ["let a = 1;".to_owned(),
            "/* 跨行注释开始".to_owned(),
            "仍然是注释".to_owned(),
            "let b = 2;".to_owned()];
        let total = doc_lines.len();
        let text_of = &mut |i: usize| doc_lines[i].clone();
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        let r1 = hl.styled_line_approx(1, 1, &doc_lines[1], total, text_of);
        let r2 = hl.styled_line_approx(2, 1, &doc_lines[2], total, text_of);
        assert!(!r1.is_empty() && !r2.is_empty());
        assert_eq!(
            r1.first().unwrap().color,
            r2.first().unwrap().color,
            "跨行注释态必须经近似缓存延续"
        );
    }

    #[test]
    fn invalidate_clears_approx_and_exact_still_correct() {
        let doc: Vec<String> = (0..300)
            .map(|i| format!("// c{i}\n"))
            .chain(std::iter::once("let tail = 0;".to_owned()))
            .collect();
        let total = doc.len();
        let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
        hl.styled_line_approx(250, 250, &doc[250], total, &mut |i| doc[i].clone());

        // 编辑：第 10 行起失效 → 近似缓存整体清空
        hl.invalidate_from(10);

        // 失效后近似重新可用，且精确路径与「从未近似」的对照完全一致
        let mut edited = |i: usize| {
            if i == 300 {
                "let tail = 1;".to_owned()
            } else {
                doc[i].clone()
            }
        };
        let runs = hl.styled_line_approx(250, 250, &edited(250), total, &mut edited);
        assert!(!runs.is_empty());

        let mut control = LazyHighlighter::new("rs").expect("rust 语法存在");
        let expected = control.styled_line(250, &edited(250), total, &mut edited);
        let actual = hl.styled_line(250, &edited(250), total, &mut edited);
        assert_eq!(actual, expected, "失效后精确路径不受近似影响");
    }
}
