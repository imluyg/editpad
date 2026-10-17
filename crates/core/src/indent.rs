//! 回车那一档的缩进判据（纯函数，零文档/零 GUI 依赖）。
//!
//! 住在 core 的理由与 [`crate::pairing`]、[`crate::markdown::quote_list_enter_action`]
//! 相同：这一格决定**往正文里写哪几个空白字符**，只有钉得住的用例能证明它不写错；
//! 而它需要的全部事实（光标左边、光标右边、当前行的行首空白、这页的语法名、一档多宽）
//! 都能在外面量好再递进来。
//!
//! 三条口径先写在这儿，各自都有用例：
//!
//! 1. **只作用在登记过的语言上**（`INDENT_RULES` 是封闭名单，它是本模块私有项，
//!    所以这里不用文档链接——链了会撞 `rustdoc::private_intra_doc_links`）。
//!    名单外的语法、
//!    未命名的语法、纯文本页一律 `Plain`＝今天的行为，一个字都不多写。理由与 B11
//!    （括号自动配对默认关）同源：**它写的是用户没按下的字符**——宁可少做，
//!    不许在散文里凭空多出四格空白。
//! 2. **一档多宽由调用方给**（[`IndentUnit`]）。syntect 的 `SyntaxDefinition` 里
//!    根本没有 `tab_size`/`indent_size`（现量：整个 syntect `src/` grep 这两个名字＝0
//!    命中），所以"按语言取宽度"这条路不存在，宽度只能从文档自己统计。
//! 3. **不做字符串/注释感知**，与 [`crate::brackets`]、[`crate::pairing`] 同一口径。
//!    代价：`print("s = {` 这种**行内没闭合**的字符串会把下一行推深一档。
//!    要做那一半得吃高亮的 scope 栈（见 docs/days 里 C4 那三闸），不在这一档。
//!    但判据的**形状**不是取舍：加档只认「光标左边最后一个非空白字符是开括号」，
//!    所以 `if (x) {|` 与 `if (x) { |` 两个落点给同一档，而 `foo(bar)` 结尾不给。

/// 名单外的语言拿不到「一档多宽」时的兜底宽度（格）＝与渲染层制表位同值。
///
/// 这一格不是"猜个漂亮数字"：app 侧 `strip_one_indent_unit` 多年来按
/// 「一个制表符，否则至多 4 个空格」剥，兜底取 4 才不至于让加/退两边各说各话。
pub const FALLBACK_INDENT_COLS: usize = 4;

/// 一档的长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentUnit {
    /// 一个制表符（文档/当前行用制表符缩进时）。
    Tab,
    /// `n` 个空格（`n >= 1`，由 [`IndentUnit::spaces`] 保证）。
    Spaces(usize),
}

impl IndentUnit {
    /// 造一个空格档，`n` 至少 1——0 格的一档＝按了键什么都不发生，那种"一档"没意义。
    pub fn spaces(n: usize) -> Self {
        Self::Spaces(n.max(1))
    }

    /// 这一档写出来是什么文本。
    pub fn text(&self) -> String {
        match self {
            IndentUnit::Tab => "\t".to_owned(),
            IndentUnit::Spaces(n) => " ".repeat(*n),
        }
    }

    /// 剥一层时至多剥**多少个空格**（制表符那一支恒剥一个 `\t`，用不上这个数）。
    fn space_cols(&self) -> usize {
        match self {
            IndentUnit::Tab => FALLBACK_INDENT_COLS,
            IndentUnit::Spaces(n) => *n,
        }
    }
}

/// 一次回车的全部已知事实（借用版，判据用它）。
///
/// ⚠️ `line_prefix` 是**截到光标**的那一截，不是整行：加档判据问的是
/// "我按回车时左边是什么"，`foo({|})` 与 `foo({})` 光标在末尾是两个不同落点。
/// `line_tail` 是光标到行尾。两者拼起来＝整行（不含行尾符）。
pub struct EnterProbe<'a> {
    /// 这页的语法名（syntect 真实名）。`None`＝没判出语言＝纯文本路径。
    pub syntax: Option<&'a str>,
    pub line_prefix: &'a str,
    pub line_tail: &'a str,
    /// 当前行的行首空白（与旧 `enter()` 同一取法：只数 `' '` 与 `'\t'`）。
    pub indent: &'a str,
    pub unit: IndentUnit,
}

/// 同一份事实的**持有版**：app 侧一行文本本来就是自己的 `String`，切三截之后
/// 三截必须活得不比它短，所以这里用 owned 字段；判据要借用时再 [`EnterFacts::probe`]。
///
/// 为什么切片这个动作也住在 core：`caret_col` 是**字符**列（ropey 口径），
/// 中文行按字节切会切歪；而行首空白的取法（`min(空白长, 光标列)`）本身就是判据的一部分。
/// 让 app 各写一遍，就是本仓已经被咬过两次的那一族"两份判据各说各话"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterFacts {
    pub syntax: Option<String>,
    pub line_prefix: String,
    pub line_tail: String,
    pub indent: String,
    pub unit: IndentUnit,
}

impl EnterFacts {
    /// 从整行文本与光标列算出全部事实。列超界钳到行尾（不 panic）。
    pub fn from_line(syntax: Option<&str>, line: &str, caret_col: usize, unit: IndentUnit) -> Self {
        let chars: Vec<char> = line.chars().collect();
        let at = caret_col.min(chars.len());
        let indent_len = chars
            .iter()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count()
            .min(at);
        Self {
            syntax: syntax.map(str::to_owned),
            line_prefix: chars[..at].iter().collect(),
            line_tail: chars[at..].iter().collect(),
            indent: chars[..indent_len].iter().collect(),
            unit,
        }
    }

    pub fn probe(&self) -> EnterProbe<'_> {
        EnterProbe {
            syntax: self.syntax.as_deref(),
            line_prefix: &self.line_prefix,
            line_tail: &self.line_tail,
            indent: &self.indent,
            unit: self.unit,
        }
    }
}

/// 判出来的那一档。测试与调用方都看这个**名字**，不看字符串——
/// 只断"插进去的文本长什么样"能把走错分支也一并糊过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterAction {
    /// 与今天逐字一致：新行抄当前行的行首空白。
    Plain,
    /// 新行＝当前缩进 + 一档。
    Indent,
    /// 光标后只剩闭花括号 ⇒ 新行＝当前缩进 − 一档（那个 `}` 自己落到新行）。
    OutdentTail,
    /// 光标两边最近的非空白字符是 `{` 与 `}`（夹不夹空白同一形）⇒ 展开成三行，
    /// 光标停在中间那行。
    ExpandPair,
}

/// 落地方案：要插入的文本，以及光标应落在**下一行**的第几个字符列。
///
/// ⚠️ 这里刻意不用"从插入文本末尾往回数几格"那种写法：插入串是我按 `\n` 造的，
/// 而 `insert_str` 会把行尾归一成文档的主导风格（CRLF 文档里一个换行是两个字符），
/// 数回来的列就会错位。"下一行的第几列"与行尾怎么归一**无关**，
/// 而四种档位里光标都落在起点行的下一行（`ExpandPair` 多出来的那第三行在光标之后）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterPlan {
    pub insert: String,
    pub caret_col_next_line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadRule {
    /// 只认开括号结尾（`{`／`[`／`(`）。
    CurlyOnly,
    /// 开括号 ＋ Python 的块头（关键字起头、行尾 `:`）。
    Python,
    /// 开括号 ＋ Lua 的块头（`then`／`do`／`function`／`repeat`／`else` 整词收尾）。
    Lua,
}

/// 封闭名单：只有登记过的语法会被写进额外空白。语法名**必须**是 syntect 的真实名
/// （`p457_indent_rules_only_name_existing_syntaxes` 逐条对着语法集查——档1 量过这一族
/// 的病根：名字写错编译器一声不响，症状只是"那门语言没这功能"）。
///
/// 名单外的刻意不进，是因为它们的"行尾形状"推不出缩进：Makefile 靠 `\` 续行、
/// TOML/INI 是键值、SQL 一行一句、Markdown 的 `#` 另有自己的续写判据，
/// 给它们加档就是写错。
const INDENT_RULES: &[(&str, HeadRule)] = &[
    ("C", HeadRule::CurlyOnly),
    ("C++", HeadRule::CurlyOnly),
    ("C#", HeadRule::CurlyOnly),
    ("Objective-C", HeadRule::CurlyOnly),
    ("Java", HeadRule::CurlyOnly),
    ("JavaScript", HeadRule::CurlyOnly),
    ("JSON", HeadRule::CurlyOnly),
    ("CSS", HeadRule::CurlyOnly),
    ("D", HeadRule::CurlyOnly),
    ("Go", HeadRule::CurlyOnly),
    ("PHP", HeadRule::CurlyOnly),
    ("Rust", HeadRule::CurlyOnly),
    ("Ruby", HeadRule::CurlyOnly),
    // YAML 只在流式写法（行尾 `{`／`[`）上加档；`key:` 结尾**刻意不加**——块映射的
    // 缩进是用户手写的，加一档就写出错文件。墓碑见
    // `p457_enter_after_yaml_key_colon_is_a_deliberate_no_op`。
    ("YAML", HeadRule::CurlyOnly),
    ("Python", HeadRule::Python),
    ("Lua", HeadRule::Lua),
];

fn rule_for(syntax: Option<&str>) -> Option<HeadRule> {
    let name = syntax?;
    INDENT_RULES
        .iter()
        .find(|(needle, _)| needle.eq_ignore_ascii_case(name))
        .map(|(_, rule)| *rule)
}

/// 离光标最近的那个**非空白**字符（往行首方向看）。
///
/// ⚠️ 不是"最后一个字符若不是空白就作数"——`if (x) { |` 这种光标前留了一格空白的
/// 落点很常见，写错的话它整个不加成（p457 那轮红过一次）。P459 把这条口径统一到
/// 三档判据：光标左边的一切判据都走这里，不许一边跳过空白、一边按相邻字符读。
fn last_non_blank(s: &str) -> Option<char> {
    s.chars().rev().find(|c| !c.is_whitespace())
}

/// 行尾最后一个**非空白**字符是开括号？
fn ends_with_opener(prefix: &str) -> bool {
    matches!(last_non_blank(prefix), Some('{') | Some('[') | Some('('))
}

/// 光标后是否**只剩**闭花括号（可以连着好几个、中间夹空白）——②的整条形判据。
/// 只认 `}`：`)`／`]` 结尾退档会在实参列表里猜错（`foo(a, |)` 不该把整行提走一档）。
///
/// P459：这一条自写成就容忍空白，②却还额外要求"紧贴的那个字符是 `}`"，等于把
/// 自己允许的形状挡在门外——那个守卫已删，`closers_only_tail` 单独作数。
/// 反过来它也是①的右半边（`{ |}` 与 `{|}` 同一形），所以两处共用一把尺。
fn closers_only_tail(tail: &str) -> bool {
    let mut saw_closer = false;
    for c in tail.chars() {
        match c {
            '}' => saw_closer = true,
            c if c.is_whitespace() => {}
            _ => return false,
        }
    }
    saw_closer
}

/// 行首缩进剥掉一档。形状与 app 侧 `strip_one_indent_unit` 一致（先剥一个制表符，
/// 否则至多剥一档的空格），区别只在那个"至多"现在跟着文档的步长走——
/// 两边不共用一个数的话，"加上去退得回来"就不成立（`p457_indent_and_strip_round_trip`）。
///
/// 字节切片安全：`indent` 按定义只含 `' '` 与 `'\t'`（与旧 `enter()` 同一取法）。
pub fn strip_one_indent(indent: &str, unit: IndentUnit) -> String {
    if let Some(rest) = indent.strip_prefix('\t') {
        return rest.to_owned();
    }
    let spaces = indent.len() - indent.trim_start_matches(' ').len();
    indent[spaces.min(unit.space_cols())..].to_owned()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 行尾的那个词（Lua 的 `then`/`do` 必须整词匹配：`local todo` 以 `do` 收尾但不是块头）。
fn trailing_word(text: &str) -> &str {
    text.rsplit(|c: char| !is_word_char(c)).next().unwrap_or("")
}

/// Python 的块头：关键字起头、行尾是 `:`。
///
/// 取值照内置包 `source.python` 的 `increaseIndentPattern`（现量：
/// `^\s*(class|((\basync\s+)?(def|for|with))|elif|else|except|finally|if|try|while)\b.*:\s*$`），
/// 这里写成关键词表而不是正则：本档**不调用** metadata 的正则——syntect 那些串
/// 首次使用才编译、编不过就 panic（雷区名单见 `p455_default_metadata_is_loaded_...`）。
fn python_header(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    if !trimmed.ends_with(':') {
        return false;
    }
    const KEYWORDS: &[&str] = &[
        "def", "class", "if", "elif", "else", "for", "while", "with", "try", "except", "finally",
        "async", "match", "case",
    ];
    let head = trimmed.trim_start();
    // `starts_with` 而不是按字节切：`head[..kw.len()]` 在中文行首会切在多字节中间 panic。
    KEYWORDS.iter().any(|kw| {
        head.starts_with(kw)
            && head[kw.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_word_char(c))
    })
}

/// Lua 的块头：以这些词**整词**收尾。
fn lua_header(prefix: &str) -> bool {
    const WORDS: &[&str] = &["then", "do", "function", "repeat", "else"];
    let trimmed = prefix.trim_end();
    !trimmed.is_empty() && WORDS.iter().any(|w| trailing_word(trimmed) == *w)
}

/// 判这一次回车该给哪一档。
pub fn enter_action(p: &EnterProbe) -> EnterAction {
    // 名单外（含纯文本页、Markdown 页）一个字都不多写。Markdown 那一支本来有自己的
    // `enter_markdown()`，这里的守卫是**第二道**：万一接线顺序被改动，Markdown 行
    // 也不会被花括号判据推着走。
    let Some(rule) = rule_for(p.syntax) else {
        return EnterAction::Plain;
    };
    // ① 光标两边最近的非空白字符是 `{` 与 `}` ⇒ 展开三行（先判这条：它同时满足②那条形）
    if last_non_blank(p.line_prefix) == Some('{') && closers_only_tail(p.line_tail) {
        return EnterAction::ExpandPair;
    }
    // ② 光标后只剩闭花括号，且这一行确实有内容、有缩进可退 ⇒ 那一档退回去
    if closers_only_tail(p.line_tail) && !p.line_prefix.trim().is_empty() && !p.indent.is_empty() {
        return EnterAction::OutdentTail;
    }
    // ③ 加档
    if ends_with_opener(p.line_prefix) {
        return EnterAction::Indent;
    }
    match rule {
        HeadRule::Python if python_header(p.line_prefix) => EnterAction::Indent,
        HeadRule::Lua if lua_header(p.line_prefix) => EnterAction::Indent,
        _ => EnterAction::Plain,
    }
}

/// 判据 + 落地文本＝一次算清（调用方只管 `insert_str` 与按列设光标）。
pub fn enter_plan(p: &EnterProbe) -> EnterPlan {
    let next_line_indent = |plan_next: String| plan_next.chars().count();
    match enter_action(p) {
        EnterAction::Plain => EnterPlan {
            insert: format!("\n{}", p.indent),
            caret_col_next_line: next_line_indent(p.indent.to_owned()),
        },
        EnterAction::Indent => {
            let grown = format!("{}{}", p.indent, p.unit.text());
            let caret_col_next_line = next_line_indent(grown.clone());
            EnterPlan {
                insert: format!("\n{grown}"),
                caret_col_next_line,
            }
        }
        EnterAction::OutdentTail => {
            let stripped = strip_one_indent(p.indent, p.unit);
            let caret_col_next_line = next_line_indent(stripped.clone());
            EnterPlan {
                insert: format!("\n{stripped}"),
                caret_col_next_line,
            }
        }
        EnterAction::ExpandPair => {
            // `{|}` → `{`⏎`  |`⏎`}`：中间那行加一档、光标留在那儿，闭括号落到第三行行首。
            // ⚠️ `{` 与光标之间那截空白**原样留在第一行**——吞掉它等于删掉用户自己打下的
            // 字符，与"补对只认本功能自己补的"（B11）同一条克制。
            let middle = format!("{}{}", p.indent, p.unit.text());
            let caret_col_next_line = next_line_indent(middle.clone());
            EnterPlan {
                insert: format!("\n{middle}\n{}", p.indent),
                caret_col_next_line,
            }
        }
    }
}

/// 一次回车至多扫这么多行的行首空白（[`dominant_indent`]）。
///
/// 刻意**不做缓存、不做增量维护**：这份统计只在按键那一刻算，代价是 ≤ 这么多行的
/// 前缀字符比较（微秒级）。要缓存就得管失效，而本仓跨帧缓存那一族账（P442/P443、
/// `row_layout_memo`）都贵在"什么时候作废"上——为一档缩进宽度开那个面不划算。
pub const INDENT_SAMPLE_LINES: usize = 2000;

/// 一个"最外层缩进"超过这么多格就当**没证据**。
///
/// 理由：取最小正数是取"最外面那一档"，而一个文件里第一层就缩进 12 格几乎不可能是
/// 一档宽度，那是某个深层缩进行恰好是唯一带缩进的行的形状。宁回落兜底。
pub const MAX_INDENT_STEP: usize = 8;

/// 从若干行的行首空白归纳一档多宽（空格文件）。拿不到任何证据 ⇒ `None`。
///
/// 泛型到 `AsRef<str>`：调用方手里可能是 `String`（app 逐行取行首空白时不可能借到
/// 文档的生命周期），判据不该为此逼它先攒一个 `Vec<&str>`。
///
/// 规则：只看**纯空格起头**的行。含制表符的行首不参与统计——tab 文件的"一档"就是一个
/// tab，本来就不需要宽度这个数；而 `  \tx` 这种混排起头说明这一行给不出"一档"的宽度。
/// 但同文件里另一条干净的行照样算证据（见用例里那一格现量）。
pub fn dominant_indent<I, S>(lines: I) -> Option<IndentUnit>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut min_spaces: Option<usize> = None;
    for line in lines.into_iter().take(INDENT_SAMPLE_LINES) {
        let line = line.as_ref();
        let spaces = line.chars().take_while(|&c| c == ' ').count();
        if spaces == 0 {
            continue;
        }
        if let Some(c) = line.chars().nth(spaces) {
            if c == '\t' {
                continue; // 混排起头：这一行给不出"一档"的宽度
            }
        }
        min_spaces = Some(match min_spaces {
            Some(m) => m.min(spaces),
            None => spaces,
        });
    }
    min_spaces
        .filter(|n| *n <= MAX_INDENT_STEP)
        .map(IndentUnit::spaces)
}

/// 这一次该用哪一档：**先看这一行自己的风格，再看文档的主导步长，最后兜底**。
///
/// "随当前行风格"是用户点单的那条：行首是制表符就加制表符（跟 Tab 键与
/// `strip_one_indent_unit` 的制表符优先规则同源），是空格就加文档自己的步长——
/// 硬给人家 2 格缩进的 YAML/Python 文件插 4 格，或插一个制表符，都是往正文里
/// 写坏文件（Python 3 直接拒绝 tab/空格混用，报 IndentationError）。
pub fn indent_unit_for<I, S>(current_indent: &str, doc_lines: I) -> IndentUnit
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if current_indent.starts_with('\t') {
        return IndentUnit::Tab;
    }
    dominant_indent(doc_lines).unwrap_or(IndentUnit::Spaces(FALLBACK_INDENT_COLS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(syntax: Option<&str>, prefix: &str, tail: &str, indent: &str) -> EnterFacts {
        EnterFacts {
            syntax: syntax.map(str::to_owned),
            line_prefix: prefix.to_owned(),
            line_tail: tail.to_owned(),
            indent: indent.to_owned(),
            unit: IndentUnit::Spaces(4),
        }
    }

    fn action(syntax: Option<&str>, prefix: &str, tail: &str, indent: &str) -> EnterAction {
        enter_action(&facts(syntax, prefix, tail, indent).probe())
    }

    #[test]
    fn p457_open_brace_at_caret_adds_one_level() {
        for syntax in ["Java", "C++", "Python", "Lua", "JSON", "YAML", "Rust"] {
            let p = facts(Some(syntax), "    if (x) {", "", "    ");
            assert_eq!(enter_action(&p.probe()), EnterAction::Indent, "{syntax}");
            let plan = enter_plan(&p.probe());
            assert_eq!(plan.insert, "\n        ", "{syntax} 插入的文本");
            assert_eq!(plan.caret_col_next_line, 8, "{syntax} 光标停在新一档的末尾");
        }
        // 行尾带空格（`{ ` 那形）也是同一档：判据取最后一个**非空白**字符
        assert_eq!(
            action(Some("Java"), "        if (x) {  ", "", "        "),
            EnterAction::Indent
        );
        // 方括号与圆括号（实参列表换行）同样加一档
        assert_eq!(
            action(Some("Java"), "call(", "", "    "),
            EnterAction::Indent
        );
        assert_eq!(action(Some("JSON"), "[", "", ""), EnterAction::Indent);
    }

    #[test]
    fn p457_plain_line_copies_the_indent_unchanged() {
        let p = facts(Some("Java"), "    int a = 1;", "", "    ");
        assert_eq!(enter_action(&p.probe()), EnterAction::Plain);
        assert_eq!(enter_plan(&p.probe()).insert, "\n    ");
    }

    #[test]
    fn p457_python_header_adds_one_level_but_not_an_expression() {
        for head in [
            "def f():",
            "class C:",
            "    if x:",
            "    else:",
            "        for i in range(3):",
            "async def g():",
            "    try:",
            "    match x:",
        ] {
            let indent_len = head.len() - head.trim_start().len();
            assert_eq!(
                action(Some("Python"), head, "", &head[..indent_len]),
                EnterAction::Indent,
                "{head}"
            );
        }
        // 反过来：以 `:` 收尾但**不是块头**的写法一律不动。
        for head in [
            "    d = {'a': }",
            "    x: int",
            "    dictionary:",
            "    if_x:",
            "    labeled:",
            "    return",
        ] {
            assert_eq!(
                action(Some("Python"), head, "", "    "),
                EnterAction::Plain,
                "{head} 不该加档"
            );
        }
        // 词边界这一格是真会误伤的：`if_x:` 以关键字 `if` 起头但那是个变量名。
        assert_eq!(
            action(Some("Python"), "    if_x:", "", "    "),
            EnterAction::Plain
        );
        // `def:` 这种"关键字后紧跟冒号"（本身是语法错误的行）**会给一档**——这不是宽松，
        // 是与内置包那条 `\bdef\b.*:\s*$` 逐字同判（现量过它的 increaseIndentPattern
        // 就是这个形状）。写在这儿是为了哪天有人想"顺手收紧"时先看见这条依据。
        assert_eq!(
            action(Some("Python"), "    def:", "", "    "),
            EnterAction::Indent
        );
        // 中文行首不许 panic（按字节切就会切歪）
        assert_eq!(
            action(Some("Python"), "    注释:", "", "    "),
            EnterAction::Plain
        );
    }

    #[test]
    fn p457_lua_headers_add_a_level_and_todo_does_not() {
        for head in [
            "if x then",
            "for i = 1, 10 do",
            "local function",
            "repeat",
            "else",
            "local t = {",
        ] {
            assert_eq!(
                action(Some("Lua"), head, "", ""),
                EnterAction::Indent,
                "{head}"
            );
        }
        // 整词判据的意义：`local todo` 以 `do` 收尾，但它不是块头
        for head in ["local todo", "my_func()", "local x = 1", "echoed"] {
            assert_eq!(
                action(Some("Lua"), head, "", "    "),
                EnterAction::Plain,
                "{head}"
            );
        }
    }

    #[test]
    fn p457_enter_after_yaml_key_colon_is_a_deliberate_no_op() {
        // 墓碑：块映射的缩进是用户手写的，给 `key:` 加一档等于写坏文件
        // （Python 那一支的 `:` 有块头关键字兜着，YAML 什么都没有）。
        for head in ["server:", "  items:", "port: 8080"] {
            assert_eq!(
                action(Some("YAML"), head, "", "  "),
                EnterAction::Plain,
                "{head} 要给 YAML 加档，先解释块映射怎么办"
            );
        }
        // 而流式写法（行尾 `{`／`[`）照加——那一档是结构给的，不是猜的
        assert_eq!(action(Some("YAML"), "map: {", "", ""), EnterAction::Indent);
    }

    #[test]
    fn p457_caret_between_braces_expands_three_lines() {
        let p = facts(Some("Java"), "    if (x) {", "}", "    ");
        assert_eq!(enter_action(&p.probe()), EnterAction::ExpandPair);
        let plan = enter_plan(&p.probe());
        assert_eq!(plan.insert, "\n        \n    ");
        // 光标停在中间那行的缩进之后（列＝8），第三行的 `}` 在它后面
        assert_eq!(plan.caret_col_next_line, 8);
    }

    #[test]
    fn p457_closer_only_tail_outdents_one_level() {
        // `foo();}` 光标在 `}` 前 ⇒ 那一行剩下的部分落到退一档的位置
        assert_eq!(
            action(Some("Java"), "        foo();", "}", "        "),
            EnterAction::OutdentTail
        );
        let p = facts(Some("Java"), "        foo();", "}", "        ");
        assert_eq!(enter_plan(&p.probe()).insert, "\n    ");
        // 连着两个闭括号也只退一档（光标只切在第一格）
        assert_eq!(
            action(Some("Java"), "        a();", "}}", "        "),
            EnterAction::OutdentTail
        );
        // 反面对照：圆括号那形**不退档**。它走"加档"那一支（光标左边是 `(`），
        // 产出 `foo(⏎····)`——不退，也不展开成三行（`ExpandPair` 只认花括号，
        // 实参列表中间那行本来就该由用户自己决定要不要留）。
        assert_eq!(
            action(Some("Java"), "    foo(", ")", "    "),
            EnterAction::Indent
        );
        // 而光标左边不是开括号、右边也还有正文时，什么都不该改：
        assert_eq!(
            action(Some("Java"), "    x = f(a", ") + 1", "    "),
            EnterAction::Plain
        );
        // 光标后除了闭括号还有正文 ⇒ 不是这一形
        assert_eq!(
            action(Some("Java"), "    a();", "} b", "    "),
            EnterAction::Plain
        );
        // 行首没缩进可退 ⇒ 不许产出负缩进
        assert_eq!(action(Some("Java"), "a();", "}", ""), EnterAction::Plain);
    }

    #[test]
    fn p459_expand_pair_reads_the_nearest_non_blank_not_the_adjacent_char() {
        // 同一个手势不该因为 `{` 与光标之间有一个空格就换一种结果。
        // 改前现量：后两格走 OutdentTail ⇒ `}` 掉到**第 0 列**、`{` 那行还留着尾随空格，
        // 而紧贴那一形（第一格）多年都是展开三行。判据的口径在③早就写成"取最后一个
        // 非空白字符"（`ends_with_opener` 的注释自陈"写错的话它就整个不加成"），
        // ①却还按相邻字符读＝同一把尺只管一边。
        for (prefix, tail) in [
            ("    fn f() {", "}"),
            ("    fn f() { ", "}"),
            ("    fn f() {   ", "}"),
        ] {
            let p = facts(Some("Rust"), prefix, tail, "    ");
            assert_eq!(
                enter_action(&p.probe()),
                EnterAction::ExpandPair,
                "{prefix}"
            );
            let plan = enter_plan(&p.probe());
            assert_eq!(plan.insert, "\n        \n    ", "{prefix} 展开成三行");
            assert_eq!(plan.caret_col_next_line, 8, "{prefix} 光标停在中间那行");
        }
        // 光标右边隔着空白也一样认（②的 closers_only_tail 本来就容忍空白）
        let p = facts(Some("Rust"), "    fn f() {", " }", "    ");
        assert_eq!(enter_action(&p.probe()), EnterAction::ExpandPair);
    }

    #[test]
    fn p459_outdent_tail_also_reads_through_the_whitespace() {
        // ②的 `closers_only_tail` 一开始就写成"可以连着好几个、中间夹空白"，但守卫里
        // 那句 `first == Some('}')` 把空白那一形直接挡回 Plain——两处口径打架。
        // 现量（改前）：`        foo(); |  }` 给 Plain（那个 `}` 留在原地、不退档），
        // 而把空白挪到光标左边（`        foo();   |}`）就给 OutdentTail。
        for tail in ["}", " }", "  }", "} }"] {
            let p = facts(Some("Java"), "        foo();", tail, "        ");
            assert_eq!(
                enter_action(&p.probe()),
                EnterAction::OutdentTail,
                "光标后只剩闭括号（夹不夹空白）＝同一形：{tail:?}"
            );
            assert_eq!(
                enter_plan(&p.probe()).insert,
                "\n    ",
                "退的就是那一档：{tail:?}"
            );
        }
        // 反面对照不许跟着放宽：右边还有正文／行首没缩进可退／左边是别的开括号
        assert_eq!(
            action(Some("Java"), "        foo();", "} b", "        "),
            EnterAction::Plain
        );
        assert_eq!(action(Some("Java"), "a();", "  }", ""), EnterAction::Plain);
        assert_eq!(
            action(Some("Java"), "    foo(", "  )", "    "),
            EnterAction::Indent,
            "圆括号那形既不展开也不退档，仍是加档"
        );
    }

    #[test]
    fn p457_unregistered_syntaxes_never_get_written_whitespace() {
        // 名单外的一切＝今天的行为。这条是"默认关之外还有一道封闭名单"的证据。
        for syntax in [
            Some("Markdown"),
            Some("reStructuredText"),
            Some("Makefile"),
            Some("Editpad TOML"),
            Some("SQL"),
            Some("HTML"),
            Some("Some Language Nobody Registered"),
            None,
        ] {
            let p = facts(syntax, "    if (x) {", "", "    ");
            assert_eq!(enter_action(&p.probe()), EnterAction::Plain, "{syntax:?}");
            assert_eq!(
                enter_plan(&p.probe()).insert,
                "\n    ",
                "{syntax:?} 必须逐字等于旧 enter()"
            );
        }
    }

    #[test]
    fn p457_indent_and_strip_round_trip_per_unit() {
        // 「加上去退得回来」是属性，不是愿望。
        // ⚠️ 只对**风格相配**的起头缩进验：一档的长度由调用方按当前行的风格推导
        // （行首是制表符⇒Tab 档、是空格⇒Spaces 档，见 C2 的 `dominant_indent`），
        // 所以"空格起头 ＋ Tab 档"那种混合起头在生产里拿不到。硬拿它来验属性，
        // 量到的是 `strip` 的制表符优先规则，而不是这条属性本身（本轮红过一次，
        // 形状＝起于 `"  "` 加一个 tab、再退时把两个空格一起吃掉）。
        let cases: &[(IndentUnit, &[&str])] = &[
            (IndentUnit::Tab, &["", "\t", "\t\t"]),
            (IndentUnit::Spaces(2), &["", "  ", "    ", "        "]),
            (IndentUnit::Spaces(4), &["", "    ", "        "]),
        ];
        for (unit, starts) in cases {
            for &start in starts.iter() {
                let grown = format!("{}{}", start, unit.text());
                assert_eq!(
                    strip_one_indent(&grown, *unit),
                    start,
                    "{unit:?} 起于 {start:?}：加一档再退一档必须回到原样"
                );
            }
        }
        // 空格档退不出负数；不足一档一并收掉（与 app 侧同一形状）
        assert_eq!(strip_one_indent("", IndentUnit::Spaces(4)), "");
        assert_eq!(strip_one_indent("  ", IndentUnit::Spaces(4)), "");
        // 制表符优先（与 `strip_one_indent_unit` 的顺序一致）
        assert_eq!(
            strip_one_indent("\t  x", IndentUnit::Spaces(2)),
            "  x",
            "先剥一个 tab"
        );
    }

    #[test]
    fn p457_indent_rules_only_name_existing_syntaxes() {
        // 档1 那一族病根的复读：表里写一个不存在的语法名，编译器一声不响，
        // 症状只是"那门语言没有这个功能"。逐条对着语法集查（走公开面
        // `available_syntax_names`，不为测试把 `syntax_set` 放开）。
        let names = crate::highlight::available_syntax_names();
        for (name, _) in INDENT_RULES {
            assert!(
                names.iter().any(|n| n == name),
                "INDENT_RULES 里的 {name} 不是真实语法名"
            );
        }
    }

    #[test]
    fn p457_from_line_cuts_on_char_cols_not_bytes() {
        // 中文行：列是**字符**列，按字节切会 panic 或切歪。
        // （`    注释 {` 的 `{` 在第 8 列：4 格空白＋两个汉字各算一列。）
        let f = EnterFacts::from_line(Some("Java"), "    注释 {", 8, IndentUnit::Spaces(4));
        assert_eq!(f.line_prefix, "    注释 {");
        assert_eq!(f.line_tail, "");
        assert_eq!(f.indent, "    ");
        assert_eq!(enter_action(&f.probe()), EnterAction::Indent);
        // 差一列的对照：光标停在 `{` 之前 ⇒ 判据看不到开括号
        let f0 = EnterFacts::from_line(Some("Java"), "    注释 {", 7, IndentUnit::Spaces(4));
        assert_eq!(f0.line_prefix, "    注释 ");
        assert_eq!(enter_action(&f0.probe()), EnterAction::Plain);
        // 光标落在行首空白之内 ⇒ indent 取到 min(空白长, 光标列)，不许越到光标右边
        let f2 = EnterFacts::from_line(Some("Java"), "        a", 2, IndentUnit::Spaces(4));
        assert_eq!(f2.indent, "  ", "光标前两格空白，不是整行的八格");
        // 列超界钳到行尾，不 panic
        let f3 = EnterFacts::from_line(Some("Java"), "ab", 99, IndentUnit::Spaces(4));
        assert_eq!(f3.line_prefix, "ab");
        assert_eq!(f3.line_tail, "");
    }

    #[test]
    fn p457_plain_plan_is_byte_identical_to_the_old_enter() {
        // 「设置关着＝行为一字不变」的 core 半边：Plain 支的产出必须与 `enter()`
        // 多年来的算法逐字相同（`\n` ＋ 当前行行首空白，空白长度按 min 钳到光标列）。
        for (line, col) in [
            ("    foo();", 10),
            ("        if (x) { ", 9),
            ("", 0),
            ("\t x", 3),
        ] {
            let f = EnterFacts::from_line(Some("Java"), line, col, IndentUnit::Spaces(4));
            if enter_action(&f.probe()) != EnterAction::Plain {
                continue; // 非 Plain 的落点由上面那些用例各自盯
            }
            let indent_len = line
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .count()
                .min(col);
            let expected: String = std::iter::once('\n')
                .chain(line.chars().take(indent_len))
                .collect();
            assert_eq!(enter_plan(&f.probe()).insert, expected, "{line:?} @ {col}");
        }
    }

    #[test]
    fn p457_indent_unit_never_describes_a_zero_width_level() {
        assert_eq!(IndentUnit::spaces(0), IndentUnit::Spaces(1));
        assert_eq!(IndentUnit::spaces(2).text(), "  ");
        assert_eq!(IndentUnit::Tab.text(), "\t");
        // 兜底那一档＝渲染层制表位宽度（改动它会破坏与 `strip_one_indent_unit` 的口径一致）
        assert_eq!(
            strip_one_indent("       ", IndentUnit::Tab),
            "   ",
            "Tab 档剥空格时用 {FALLBACK_INDENT_COLS}"
        );
    }

    #[test]
    fn p458_dominant_indent_reads_the_outermost_step() {
        // 4 格一档的文件：最外层那一档就是 4
        let four = ["def a():", "    x = 1", "        y = 2", "    z = 3"];
        assert_eq!(
            dominant_indent(four.iter().copied()),
            Some(IndentUnit::Spaces(4))
        );
        // 2 格一档（YAML 的常见形状）
        let two = ["a:", "  b:", "    c: 1"];
        assert_eq!(
            dominant_indent(two.iter().copied()),
            Some(IndentUnit::Spaces(2))
        );
        // 全文没有一行带缩进 ⇒ 没证据，交回调用方兜底
        assert_eq!(dominant_indent(["foo", "bar"].iter().copied()), None);
        // 唯一带缩进的行是 12 格起头 ⇒ 那不是"一档"，是深层嵌套恰好露了一手
        assert_eq!(
            dominant_indent(["if x:", "            y = 1"].iter().copied()),
            None,
            "超过 {MAX_INDENT_STEP} 格的最小值必须当没证据"
        );
        // 制表符文件不参与空格统计（它的"一档"是一个 tab，不需要宽度这个数）
        assert_eq!(dominant_indent(["\tx", "\t\ty"].iter().copied()), None);
        // 混排起头（两个空格再一个 tab）**自己**不算证据：它的 2 格不是"一档"。
        assert_eq!(dominant_indent(["  \tx"].iter().copied()), None);
        // 但同一份文件里另一条干净的 4 格起头照样算——因为别处有一行不自洽，
        // 就否认这里有步长，等于把"没证据"扩大成"惩罚有证据的那一行"。
        assert_eq!(
            dominant_indent(["  \tx", "    y"].iter().copied()),
            Some(IndentUnit::Spaces(4))
        );
        // 采样上界真的有效：把一行 2 格的缩进推到第 INDENT_SAMPLE_LINES+1 行 ⇒ 扫不到
        let mut lines: Vec<String> = (0..INDENT_SAMPLE_LINES + 4)
            .map(|i| format!("l{i}"))
            .collect();
        lines[INDENT_SAMPLE_LINES] = "  deep".to_owned();
        assert_eq!(dominant_indent(lines.iter().map(String::as_str)), None);
    }

    #[test]
    fn p458_unit_comes_from_the_line_first_then_the_document() {
        // ① 当前行自己用制表符 ⇒ Tab（与 Tab 键、`strip_one_indent_unit` 的制表符优先同源）
        assert_eq!(
            indent_unit_for("\t\t", ["    x"].iter().copied()),
            IndentUnit::Tab
        );
        // ② 当前行是空格 ⇒ 文档主导步长
        assert_eq!(
            indent_unit_for("    ", ["a:", "  b:", "    c"].iter().copied()),
            IndentUnit::spaces(2)
        );
        // ③ 当前行没缩进、文档也没证据 ⇒ 兜底 4 格
        assert_eq!(
            indent_unit_for("", ["foo", "bar"].iter().copied()),
            IndentUnit::Spaces(FALLBACK_INDENT_COLS)
        );
        // ⚠️ 这一格记下那条属性为什么只在相配的风格上成立：空格起头的行配上 Tab 档
        // 就是混合缩进文件，加/退两边会各说各话（第 331 轮那族"红在隔壁"的形状）。
        // 生产里拿不到这个组合——`indent_unit_for` 先认当前行的风格。
        let mixed = strip_one_indent(&format!("  {}", IndentUnit::Tab.text()), IndentUnit::Tab);
        assert_ne!(
            mixed, "  ",
            "空格起头 ＋ Tab 档＝加上去退不回来，所以不允许出现"
        );
    }
}
