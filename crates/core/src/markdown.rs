//! Markdown 解析（预览面板的语法层，core 纯逻辑）。
//!
//! 手写解析器（构建环境无法引入 pulldown-cmark）。v1 只有「标题/段落/列表/
//! 引用/围栏代码/三种行内标记」，且行内标记是**开关式**（未闭合的 `**` 会一路
//! 染到行尾），预览里真实 .md 文件大面积走样。v2 覆盖：
//!
//! * 块级：`#`~`######` 标题（含 ATX 闭栏 `# 标题 #`）、`---`/`***`/`_ _ _`
//!   分隔线、段落（连续非空行合并）、无序 `- * +` 与有序 `1.`/`1)` 列表
//!   （**保留序号、缩进层级、任务勾选框**）、`>` 引用（连续行合并、`>>` 深度）、
//!   ``` ``` ``` 与 `~~~` 围栏代码（记录 info 串的语言）、GFM 表格
//!   （含 `:---`/`:---:`/`---:` 对齐）
//! * 行内：`**粗**`/`__粗__`、`*斜*`/`_斜_`、`` `代码` ``（任意反引号成串）、
//!   `~~删除线~~`、`[文字](目标)` 链接、`![替代文字](地址)` 图片、
//!   `<https://…>` 自动链接、`\` 转义标点
//! * 行内标记**要求闭合**：找不到闭栏时按字面量输出（与 CommonMark 一致），
//!   不再把后半行整条染色
//!
//! 明确不支持（保持字面量）：HTML 标签、脚注、引用式链接 `[a][ref]`、
//! 列表内的续行段落、表格单元里的 `\|` 转义竖线。

/// 行内样式位标志。
pub const MD_BOLD: u8 = 1 << 0;
pub const MD_ITALIC: u8 = 1 << 1;
pub const MD_CODE: u8 = 1 << 2;
pub const MD_STRIKE: u8 = 1 << 3;
pub const MD_LINK: u8 = 1 << 4;
pub const MD_IMAGE: u8 = 1 << 5;

/// 一段同样式文本。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MdSpan {
    pub text: String,
    /// [`MD_BOLD`] 等按位或
    pub style: u8,
    /// 仅 [`MD_LINK`]／[`MD_IMAGE`] 有值：链接目标原文
    pub link: Option<String>,
}

impl MdSpan {
    #[cfg(test)]
    fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            style: 0,
            link: None,
        }
    }
    pub fn is_bold(&self) -> bool {
        self.style & MD_BOLD != 0
    }
    pub fn is_italic(&self) -> bool {
        self.style & MD_ITALIC != 0
    }
    pub fn is_code(&self) -> bool {
        self.style & MD_CODE != 0
    }
    pub fn is_strike(&self) -> bool {
        self.style & MD_STRIKE != 0
    }
    pub fn is_link(&self) -> bool {
        self.style & MD_LINK != 0
    }
    pub fn is_image(&self) -> bool {
        self.style & MD_IMAGE != 0
    }
}

/// 表格列的水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MdAlign {
    Left,
    Center,
    Right,
}

/// 块级元素。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MdBlock {
    Heading {
        level: usize,
        spans: Vec<MdSpan>,
    },
    Paragraph {
        spans: Vec<MdSpan>,
    },
    /// `lang` = 围栏 info 串首词（``` ```` ```rust → `Some("rust")`）。
    CodeBlock {
        lang: Option<String>,
        lines: Vec<String>,
    },
    /// `order` = 有序列表的序号（无序项为 `None`）；`indent` = 缩进层级
    /// （2 列或一个制表符为一层）；`task` = 任务勾选框状态（`Some(true)` 已勾）。
    ListItem {
        order: Option<usize>,
        indent: usize,
        task: Option<bool>,
        spans: Vec<MdSpan>,
    },
    Quote {
        depth: usize,
        spans: Vec<MdSpan>,
    },
    Table {
        align: Vec<MdAlign>,
        header: Vec<Vec<MdSpan>>,
        rows: Vec<Vec<Vec<MdSpan>>>,
    },
    Rule,
}

/// 文件头元数据块（front matter）**结尾栅栏之后那一行**的下标；不是 front matter 则 `None`。
///
/// 判据取各家渲染器的共同子集：起始栅栏必须顶格落在第一行，往后第一个配对的栅栏即收尾
/// （`---` 也接受 `...` 收尾，`+++` 只与 `+++` 配对）。等不到配对就整篇按普通 Markdown 走——
/// 以一个孤立 `---` 开头的文档里那是分隔线，不是元数据。
///
/// 这一区域在预览里整块隐去（不是降级成段落）：博客／静态站点生成的 `.md` 十有八九带它，
/// 留在那儿会把第一眼的好好一段变成 `title: xxx`；要看原文有左半正文页兜着。
fn front_matter_end(lines: &[&str]) -> Option<usize> {
    let opener = lines.first()?.trim();
    if opener != "---" && opener != "+++" {
        return None;
    }
    for (j, line) in lines.iter().enumerate().skip(1) {
        let closer = line.trim();
        if closer == opener || (opener == "---" && closer == "...") {
            return Some(j + 1);
        }
    }
    None
}

/// 解析整篇 Markdown。行尾兼容 `\r\n` / `\n` / 孤立 `\r` 与异体行界。
/// 开头若带 front matter（见 `front_matter_end`）则不渲染那一区域。
pub fn parse_markdown(text: &str) -> Vec<MdBlock> {
    let lines = split_lines(text);
    let mut blocks = Vec::new();
    let mut i = front_matter_end(&lines).unwrap_or(0);

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some((block, next)) = fence(&lines, i) {
            blocks.push(block);
            i = next;
            continue;
        }

        if let Some((level, content)) = heading(trimmed) {
            blocks.push(MdBlock::Heading {
                level,
                spans: inline(content),
            });
            i += 1;
            continue;
        }

        if is_rule(trimmed) {
            blocks.push(MdBlock::Rule);
            i += 1;
            continue;
        }

        if let Some((block, next)) = table(&lines, i) {
            blocks.push(block);
            i = next;
            continue;
        }

        if let Some((depth, content)) = quote(trimmed) {
            let mut merged = String::from(content);
            i += 1;
            // 同深度的连续引用行并为一段（跨行才有「引用是一整块」的观感）
            while i < lines.len() {
                match quote(lines[i].trim()) {
                    Some((same, next_content)) if same == depth => {
                        merged.push(' ');
                        merged.push_str(next_content);
                        i += 1;
                    }
                    _ => break,
                }
            }
            blocks.push(MdBlock::Quote {
                depth,
                spans: inline(&merged),
            });
            continue;
        }

        if let Some(item) = list_item(lines[i]) {
            blocks.push(MdBlock::ListItem {
                order: item.order,
                indent: item.indent,
                task: item.task,
                spans: inline(item.content),
            });
            i += 1;
            continue;
        }

        let mut paragraph = String::from(trimmed);
        let mut prev_raw = lines[i];
        i += 1;
        while i < lines.len() {
            let next = lines[i].trim();
            if next.is_empty()
                || fence(&lines, i).is_some()
                || heading(next).is_some()
                || is_rule(next)
                || is_table_start(&lines, i)
                || quote(next).is_some()
                || list_item(lines[i]).is_some()
            {
                break;
            }
            let (sep, drop_backslash) = line_break_kind(prev_raw);
            if drop_backslash {
                // 行尾那个 `\` 是换行标记本身，不进气泡
                paragraph.pop();
            }
            paragraph.push(sep);
            paragraph.push_str(next);
            prev_raw = lines[i];
            i += 1;
        }
        blocks.push(MdBlock::Paragraph {
            spans: inline(&paragraph),
        });
    }
    blocks
}

/// 切行：判据与 [`crate::document::split_lines_keep_ending`] **同一份**（读侧按全集，定案
/// R-11「读按全集」），差别只是这里丢掉行尾。
///
/// 文本以行界收尾时多补一个空末段——那是编辑器的行模型里真实存在的幻影行
/// （`empty_input_yields_no_blocks` 与预览行数都依赖这一格）。
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = crate::document::split_lines_keep_ending(text)
        .iter()
        .map(|seg| crate::document::trim_line_breaks(seg))
        .collect();
    if text
        .chars()
        .next_back()
        .is_some_and(crate::search::is_line_break)
    {
        lines.push("");
    }
    lines
}

/// 段落里两行正文之间的分隔符 ⇒（写进文本的分隔字符，是否吃掉行尾那个反斜杠）。
///
/// CommonMark 的两种硬换行写法：行尾 **≥2 个空格**，或行尾一个**落单的 `\`**。
/// 落单＝行尾反斜杠的个数是奇数——`foo\\` 里第二个 `\` 被第一个转义掉，那仍是软换行。
/// 硬换行写成 `\n` 交给渲染层折行；其余情况写空格，与浏览器折叠软换行的观感一致。
fn line_break_kind(line: &str) -> (char, bool) {
    if line.ends_with('\\') {
        let run = line.chars().rev().take_while(|&c| c == '\\').count();
        return if run % 2 == 1 {
            ('\n', true)
        } else {
            (' ', false)
        };
    }
    if line.ends_with("  ") {
        return ('\n', false);
    }
    (' ', false)
}

/// `# 标题` → Some((级别, 内容))；# 后必须有空格或就是整行。
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if rest.is_empty() {
        return Some((level, ""));
    }
    let first = rest.chars().next()?;
    if first != ' ' && first != '\t' {
        return None;
    }
    Some((level, strip_closing_hashes(rest.trim())))
}

/// ATX 闭栏：只有「空格之后的末尾 `#` 串」才算闭栏（`# C#` 的 `#` 是正文）。
fn strip_closing_hashes(content: &str) -> &str {
    let trimmed = content.trim_end();
    if !trimmed.ends_with('#') {
        return trimmed;
    }
    let hashes = trimmed.chars().rev().take_while(|&c| c == '#').count();
    // `#` 恒为一字节，按字节切安全
    let before = &trimmed[..trimmed.len() - hashes];
    if before.is_empty() || before.ends_with(' ') || before.ends_with('\t') {
        before.trim_end()
    } else {
        trimmed
    }
}

/// 分隔线：去掉空白后全部字符同种且属于 `{-, *, _}`、至少 3 个。
fn is_rule(line: &str) -> bool {
    let mut chars = line.trim().chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1usize;
    for c in chars {
        if c != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

/// 引用行：`>` 串 → Some((深度, 内容))。
fn quote(line: &str) -> Option<(usize, &str)> {
    let mut rest = line.trim_start();
    let mut depth = 0usize;
    while let Some(after) = rest.strip_prefix('>') {
        depth += 1;
        rest = after.trim_start_matches([' ', '\t']);
    }
    if depth == 0 {
        return None;
    }
    Some((depth, rest))
}

/// 列表项（收**原始行**：缩进层级只看行首空白）。
struct MdListItem<'a> {
    order: Option<usize>,
    indent: usize,
    task: Option<bool>,
    content: &'a str,
}

fn list_item(raw: &str) -> Option<MdListItem<'_>> {
    let mut cols = 0usize;
    let mut idx = 0usize;
    for c in raw.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4,
            _ => break,
        }
        idx += c.len_utf8();
    }
    let rest = &raw[idx..];
    let (order, after) = if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        (None, after)
    } else {
        // 有序：≤9 位数字 + `.` 或 `)`
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let byte_len: usize = rest[..digits].chars().map(char::len_utf8).sum();
        let head = &rest[..byte_len];
        let tail = &rest[byte_len..];
        let order = head.parse::<usize>().ok()?;
        let after = tail.strip_prefix(['.', ')'])?;
        (Some(order), after)
    };
    // 标记后必须跟空白，且不能是分隔线（`---` 已在块级先判掉）
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let content = after.trim_start_matches([' ', '\t']);
    let mut task = None;
    let mut body = content;
    if order.is_none() {
        for (mark, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(rest) = content.strip_prefix(mark) {
                task = Some(done);
                body = rest;
                break;
            }
        }
    }
    Some(MdListItem {
        order,
        indent: (cols / 2).min(6),
        task,
        content: body,
    })
}

/// 围栏代码块：```` ``` ```` 与 `~~~`，记录 info 串首词作语言；未闭栏也成块。
fn fence(lines: &[&str], start: usize) -> Option<(MdBlock, usize)> {
    let open = lines[start].trim_start();
    // ``` 与 ~~~ 都是围栏（两种 marker 都是三字节 ASCII，切片落在字符边界上）
    let backtick = open.strip_prefix("```");
    let tilde = open.strip_prefix("~~~");
    let (marker, info) = match (backtick, tilde) {
        (Some(info), _) => ('`', info),
        (None, Some(info)) => ('~', info),
        (None, None) => return None,
    };
    let lang = info.split_whitespace().next().map(str::to_owned);
    let mut body = Vec::new();
    let mut k = start + 1;
    while k < lines.len() {
        let candidate = lines[k].trim_start();
        // 闭栏：整行只有同种标记（带 info 的那行不算闭栏）
        if candidate.len() >= 3 && candidate.chars().all(|c| c == marker) {
            return Some((MdBlock::CodeBlock { lang, lines: body }, k + 1));
        }
        body.push(lines[k].to_owned());
        k += 1;
    }
    Some((MdBlock::CodeBlock { lang, lines: body }, k))
}

/// 表格第一行的判定（段落续行要用它断句）。
fn is_table_start(lines: &[&str], i: usize) -> bool {
    table(lines, i).is_some()
}

/// GFM 表格：表头行 + 分隔行（只含 `| : - 空格`）+ 数据行。
fn table(lines: &[&str], start: usize) -> Option<(MdBlock, usize)> {
    let head = lines[start].trim();
    if !head.contains('|') {
        return None;
    }
    let delim = lines.get(start + 1)?.trim();
    if delim.is_empty()
        || !delim.contains('-')
        || !delim.chars().all(|c| matches!(c, '|' | ':' | '-' | ' '))
    {
        return None;
    }
    let cells = split_row(delim);
    let mut align = Vec::new();
    for cell in &cells {
        align.push(parse_align(cell)?);
    }
    if align.is_empty() {
        return None;
    }
    let header = normalize_row(
        split_row(head).into_iter().map(inline).collect(),
        align.len(),
    );
    let mut rows = Vec::new();
    let mut k = start + 2;
    while k < lines.len() {
        let line = lines[k].trim();
        if line.is_empty() || !line.contains('|') {
            break;
        }
        let row: Vec<Vec<MdSpan>> = split_row(line).into_iter().map(inline).collect();
        rows.push(normalize_row(row, align.len()));
        k += 1;
    }
    Some((
        MdBlock::Table {
            align,
            header,
            rows,
        },
        k,
    ))
}

fn split_row(line: &str) -> Vec<&str> {
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|c| c.trim()).collect()
}

/// 单元格数补齐／裁到列数。
fn normalize_row(mut row: Vec<Vec<MdSpan>>, cols: usize) -> Vec<Vec<MdSpan>> {
    row.truncate(cols);
    while row.len() < cols {
        row.push(Vec::new());
    }
    row
}

/// `:---`/`:--:`/`---:`；不合法（无 `-`）返回 `None` → 整张表不成立。
fn parse_align(cell: &str) -> Option<MdAlign> {
    let t = cell.trim();
    if !t.contains('-') {
        return None;
    }
    let left = t.starts_with(':');
    let right = t.ends_with(':');
    let core = t.trim_matches(|c| c == ':' || c == '-' || c == ' ');
    if !core.is_empty() {
        return None;
    }
    Some(match (left, right) {
        (true, true) => MdAlign::Center,
        (false, true) => MdAlign::Right,
        _ => MdAlign::Left,
    })
}

/// 行内解析入口：递归展开标记后合并相邻同样式段。
fn inline(text: &str) -> Vec<MdSpan> {
    let mut spans: Vec<MdSpan> = Vec::new();
    parse_inline(text, 0, None, &mut spans);
    merge_spans(&mut spans);
    spans
}

/// 把相邻且样式／链接目标都相同的段并成一段。
fn merge_spans(spans: &mut Vec<MdSpan>) {
    let merged: Vec<MdSpan> =
        std::mem::take(spans)
            .into_iter()
            .fold(Vec::<MdSpan>::new(), |mut acc, span| {
                match acc.last_mut() {
                    Some(last) if last.style == span.style && last.link == span.link => {
                        last.text.push_str(&span.text)
                    }
                    _ => acc.push(span),
                }
                acc
            });
    *spans = merged;
}

/// HTML 实体名表：只收实际会掉进 Markdown 里的那些。CommonMark 的 full 表有两千多条，
/// 为编辑器装那份不值当——剩下的码位由数值型 `&#..;` 兜住。
fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        "thinsp" => '\u{2009}',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "middot" => '·',
        "bull" => '•',
        "dagger" => '†',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "rarr" => '→',
        "larr" => '←',
        "harr" => '↔',
        "uarr" => '↑',
        "darr" => '↓',
        "infin" => '∞',
        "ne" => '≠',
        "le" => '≤',
        "ge" => '≥',
        "sect" => '§',
        "para" => '¶',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "lambda" => 'λ',
        "theta" => 'θ',
        "mu" => 'μ',
        "pi" => 'π',
        "sigma" => 'σ',
        "omega" => 'ω',
        "Delta" => 'Δ',
        "Omega" => 'Ω',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        _ => return None,
    })
}

/// 串首若是一个合法实体引用 ⇒（解码后的文本，吃掉的字节数）。
///
/// 只认带 `;` 收尾的写法（CommonMark 早就不认无分号的老式），名字限 ASCII 且 ≤ 10 字符；
/// 数值型挡掉 0、代理区与超出 `char` 上界的码位。**解不动就原样留着**——坏数据被译成
/// 一个怪字符，比留着 `&#xD800;` 这种可见的原文更难查。
fn parse_entity(text: &str) -> Option<(String, usize)> {
    let body = text.strip_prefix('&')?;
    let semicolon = body.find(';')?;
    let name = &body[..semicolon];
    if name.is_empty() || name.len() > 10 || !name.is_ascii() {
        return None;
    }
    let decoded = match name.strip_prefix('#') {
        Some(rest) => {
            let (radix, digits) = match rest.as_bytes().first() {
                Some(b'x') | Some(b'X') => (16, &rest[1..]),
                _ => (10, rest),
            };
            if digits.is_empty()
                || !digits.bytes().all(|b| {
                    if radix == 16 {
                        (b as char).is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                return None;
            }
            let cp = u32::from_str_radix(digits, radix).ok()?;
            // `char::from_u32(0)` 在 Rust 里是**合法**的 `\0`（它只挡代理区与超界码位），
            // 而预览一段文字中间冒出一个 NUL／控制字符比留着 `&#0;` 原文糟得多。
            // 制表符放行——表格里用它对齐是真需求。
            if cp < 0x20 && cp != 0x09 {
                return None;
            }
            char::from_u32(cp)?
        }
        None => named_entity(name)?,
    };
    Some((decoded.to_string(), 1 + semicolon + 1))
}

/// 把串里每个合法实体换成对应字符，其余原样保留（链接目标走这条）。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match parse_entity(tail) {
            Some((decoded, consumed)) => {
                out.push_str(&decoded);
                rest = &tail[consumed..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 一层行内扫描：`base` 继承自外层标记（嵌套样式），`link` 是所在链接的目标。
fn parse_inline(text: &str, base: u8, link: Option<&str>, out: &mut Vec<MdSpan>) {
    let mut lit = String::new();
    macro_rules! flush {
        () => {
            if !lit.is_empty() {
                out.push(MdSpan {
                    text: std::mem::take(&mut lit),
                    style: base,
                    link: link.map(str::to_owned),
                });
            }
        };
    }

    let mut i = 0usize;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        let c_len = c.len_utf8();

        // 转义：`\` + 标点 ⇒ 字面量标点
        if c == '\\' {
            if let Some(next) = text[i + c_len..].chars().next() {
                if is_ascii_punct(next) {
                    lit.push(next);
                    i += c_len + next.len_utf8();
                    continue;
                }
            }
        }

        // HTML 实体：代码段分支自己切原文、不经过这里，所以 `` `&amp;` `` 仍是字面量
        if c == '&' {
            if let Some((decoded, consumed)) = parse_entity(&text[i..]) {
                lit.push_str(&decoded);
                i += consumed;
                continue;
            }
        }

        // 代码段：任意长度的反引号串成对，内容一律字面量
        if c == '`' {
            let run = run_len(&text[i..], '`');
            if let Some(close) = find_run(&text[i + run..], '`', run) {
                flush!();
                out.push(MdSpan {
                    text: trim_code_backticks(&text[i + run..i + run + close]).to_owned(),
                    style: base | MD_CODE,
                    link: link.map(str::to_owned),
                });
                i += run + close + run;
                continue;
            }
            lit.push_str(&text[i..i + run]);
            i += run;
            continue;
        }

        // 链接 / 图片：`[标签](目标)`、`![替代](地址)`
        if c == '[' || (c == '!' && text[i + 1..].starts_with('[')) {
            let is_image = c == '!';
            let label_at = i + if is_image { 2 } else { 1 };
            if let Some(close) = find_bracket(&text[label_at..]) {
                let label = &text[label_at..label_at + close];
                let after = &text[label_at + close + 1..];
                if let Some(dest_end) = after.strip_prefix('(').and_then(find_paren) {
                    // dest_end 是**剥掉 `(` 之后**串内的偏移
                    let raw_dest = &after[1..1 + dest_end];
                    if let Some(dest) = link_dest(raw_dest) {
                        flush!();
                        let style = base | MD_LINK | if is_image { MD_IMAGE } else { 0 };
                        parse_inline(label, style, Some(dest.as_str()), out);
                        i = label_at + close + 2 + dest_end + 1;
                        continue;
                    }
                }
            }
        }

        // 自动链接 `<https://…>`（HTML 标签按字面量，不吞）
        if c == '<' {
            if let Some(end) = text[i + 1..].find('>') {
                let inner = &text[i + 1..i + 1 + end];
                if is_urlish(inner) {
                    flush!();
                    out.push(MdSpan {
                        text: inner.to_owned(),
                        style: base | MD_LINK,
                        link: Some(inner.to_owned()),
                    });
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        // 强调：`*`/`_`（粗／斜）、`~`（删除线），**必须找到闭栏**
        if matches!(c, '*' | '_' | '~') {
            let run = run_len(&text[i..], c);
            let opts: &[(u8, usize)] = if c == '~' {
                &[(MD_STRIKE, 2)]
            } else if run >= 3 {
                &[(MD_BOLD | MD_ITALIC, 3), (MD_BOLD, 2), (MD_ITALIC, 1)]
            } else if run == 2 {
                &[(MD_BOLD, 2), (MD_ITALIC, 1)]
            } else {
                &[(MD_ITALIC, 1)]
            };
            let mut opened = false;
            for &(style, len) in opts {
                if run < len {
                    continue;
                }
                // 开栏后紧跟空白 ⇒ 不算开栏（`** **` 是字面量）
                if text[i + len..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
                {
                    continue;
                }
                let Some(close) = find_run(&text[i + len..], c, len) else {
                    continue;
                };
                if close == 0 {
                    continue; // 空内容不成段
                }
                // 闭栏前紧跟空白 ⇒ 不算闭栏（`**粗 **`）
                let abs_close = i + len + close;
                if text[..abs_close]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
                {
                    continue;
                }
                flush!();
                parse_inline(&text[i + len..abs_close], base | style, link, out);
                i = abs_close + len;
                opened = true;
                break;
            }
            if opened {
                continue;
            }
            lit.push_str(&text[i..i + run]);
            i += run;
            continue;
        }

        lit.push(c);
        i += c_len;
    }
    flush!();
}

/// 从串首数连续同字符的长度。
fn run_len(s: &str, c: char) -> usize {
    s.chars().take_while(|&x| x == c).count()
}

/// 在 `s` 中找第一个长度 ≥ `min` 的 `c` 串，返回其**起始字节偏移**。
fn find_run(s: &str, c: char, min: usize) -> Option<usize> {
    let mut offset = 0usize;
    for ch in s.chars() {
        if ch == c {
            let run = run_len(&s[offset..], c);
            if run >= min {
                return Some(offset);
            }
            offset += run * c.len_utf8();
        } else {
            offset += ch.len_utf8();
        }
    }
    None
}

/// `[` 之后找配对的 `]`（按嵌套深度），返回其字节偏移。
fn find_bracket(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut offset = 0usize;
    for ch in s.chars() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
        offset += ch.len_utf8();
    }
    None
}

/// `(` 已剥掉之后，找配对的 `)`（允许一层嵌套），返回其字节偏移。
fn find_paren(s: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut offset = 0usize;
    for ch in s.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
        offset += ch.len_utf8();
    }
    None
}

/// `(https://a "标题")` → `Some("https://a")`；空目标 ⇒ `None`（按字面量）。
///
/// 目标里的实体要解开：`?a=1&amp;b=2` 写成 URL 时是真 `&`，不解开点出去的是错地址。
fn link_dest(raw: &str) -> Option<String> {
    let t = raw.trim();
    let t = t
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(t);
    let head = t.split_whitespace().next().unwrap_or("");
    if head.is_empty() {
        return None;
    }
    Some(decode_entities(head))
}

/// 自动链接判据：已知 scheme + 无空白。
fn is_urlish(s: &str) -> bool {
    !s.chars().any(char::is_whitespace)
        && (s.starts_with("http://")
            || s.starts_with("https://")
            || s.starts_with("ftp://")
            || s.starts_with("mailto:")
            || s.starts_with("file://"))
}

fn is_ascii_punct(c: char) -> bool {
    matches!(
        c,
        '!' | '"'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '-'
            | '.'
            | '/'
            | ':'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '_'
            | '`'
            | '{'
            | '|'
            | '}'
            | '~'
    )
}

/// 代码段内容：CommonMark 规定首尾各一个空白时先剥掉（`` ` x ` `` → `x`）。
fn trim_code_backticks(body: &str) -> &str {
    if body.len() > 2 && body.starts_with(' ') && body.ends_with(' ') {
        &body[1..body.len() - 1]
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(spans: &[MdSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    fn code(text: &str) -> MdSpan {
        MdSpan {
            text: text.into(),
            style: MD_CODE,
            link: None,
        }
    }

    fn styled(text: &str, style: u8) -> MdSpan {
        MdSpan {
            text: text.into(),
            style,
            link: None,
        }
    }

    #[test]
    fn headings_levels_and_content() {
        let blocks = parse_markdown("# 一级\n##二级无空格不当标题\n### 三级 内容");
        // 「##后无空格」不是标题，按普通段落处理 → 共 3 块
        assert_eq!(blocks.len(), 3);
        match &blocks[0] {
            MdBlock::Heading { level, spans } => {
                assert_eq!(*level, 1);
                assert_eq!(spans_text(spans), "一级");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&blocks[1], MdBlock::Paragraph { .. }));
        match &blocks[2] {
            MdBlock::Heading { level, .. } => assert_eq!(*level, 3),
            other => panic!("{other:?}"),
        }
    }

    /// ATX 闭栏（`# 标题 #`）与「正文里的 #」要分得开。
    #[test]
    fn heading_closing_hashes_are_stripped_only_after_space() {
        let blocks = parse_markdown("## 章节 ## \n### C# 语言\n# #");
        let texts: Vec<String> = blocks
            .iter()
            .map(|b| match b {
                MdBlock::Heading { spans, .. } => spans_text(spans),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, vec!["章节", "C# 语言", ""]);
    }

    #[test]
    fn paragraphs_merge_consecutive_lines_and_stop_at_blank() {
        let blocks = parse_markdown("第一行\n第二行\n\n新段落");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            MdBlock::Paragraph { spans } => {
                assert_eq!(spans_text(spans), "第一行 第二行");
            }
            other => panic!("{other:?}"),
        }
        match &blocks[1] {
            MdBlock::Paragraph { spans } => assert_eq!(spans_text(spans), "新段落"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn code_fence_preserves_inner_lines_without_inline_parsing() {
        let text = "前文\n```\n**不是粗体**\nlet a = 1;\n```\n后文";
        let blocks = parse_markdown(text);
        assert_eq!(blocks.len(), 3);
        match &blocks[1] {
            MdBlock::CodeBlock { lang, lines } => {
                assert_eq!(lang, &None);
                assert_eq!(
                    lines,
                    &vec!["**不是粗体**".to_owned(), "let a = 1;".to_owned()]
                );
            }
            other => panic!("{other:?}"),
        }
        // 未闭合围栏也整体作为代码块
        let blocks = parse_markdown("```\nonly");
        assert!(matches!(blocks[0], MdBlock::CodeBlock { .. }));
    }

    /// 围栏 info 串的语言被记下来（预览可据此排版），`~~~` 同款可用。
    #[test]
    fn fence_records_language_and_accepts_tildes() {
        let blocks = parse_markdown("```rust\nfn main() {}\n```");
        match &blocks[0] {
            MdBlock::CodeBlock { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(lines, &vec!["fn main() {}".to_owned()]);
            }
            other => panic!("{other:?}"),
        }
        let blocks = parse_markdown("~~~\n裸波浪线\n~~~");
        match &blocks[0] {
            MdBlock::CodeBlock { lang, lines } => {
                assert_eq!(lang, &None);
                assert_eq!(lines, &vec!["裸波浪线".to_owned()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lists_quotes_and_rules() {
        let blocks = parse_markdown("- 甲\n* 乙\n1. 丙\n> 引用一句\n---");
        assert_eq!(blocks.len(), 5);
        assert!(matches!(&blocks[0], MdBlock::ListItem { .. }));
        assert!(matches!(&blocks[1], MdBlock::ListItem { .. }));
        match &blocks[2] {
            MdBlock::ListItem { order, .. } => assert_eq!(*order, Some(1)),
            other => panic!("{other:?}"),
        }
        match &blocks[3] {
            MdBlock::Quote { depth, spans } => {
                assert_eq!(*depth, 1);
                assert_eq!(spans_text(spans), "引用一句");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(blocks[4], MdBlock::Rule);
    }

    /// 有序序号／缩进层级／任务勾选框——预览要按这三格排版。
    #[test]
    fn list_items_keep_number_indent_and_task_checkbox() {
        let blocks =
            parse_markdown("3. 第三项\n    - 子项（缩进两层）\n- [x] 已完成\n- [ ] 未完成\n");
        assert_eq!(blocks.len(), 4, "{blocks:?}");
        match &blocks[0] {
            MdBlock::ListItem {
                order,
                indent,
                task,
                ..
            } => {
                assert_eq!(*order, Some(3));
                assert_eq!(*indent, 0);
                assert_eq!(*task, None);
            }
            other => panic!("{other:?}"),
        }
        match &blocks[1] {
            MdBlock::ListItem { order, indent, .. } => {
                assert_eq!(*order, None);
                assert_eq!(*indent, 2, "四个空格＝两层");
            }
            other => panic!("{other:?}"),
        }
        match (&blocks[2], &blocks[3]) {
            (
                MdBlock::ListItem {
                    task: Some(true), ..
                },
                MdBlock::ListItem {
                    task: Some(false), ..
                },
            ) => {}
            other => panic!("{other:?}"),
        }
        // 「3.14 是圆周率」不是列表项（数字后没有空格）
        let blocks = parse_markdown("3.14 是圆周率");
        assert!(matches!(&blocks[0], MdBlock::Paragraph { .. }));
    }

    /// 连续引用行并成一块；`>>` 记深度。
    #[test]
    fn quotes_merge_consecutive_lines_and_track_depth() {
        let blocks = parse_markdown("> 第一句\n> 第二句\n>> 更深的一句");
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        match &blocks[0] {
            MdBlock::Quote { depth, spans } => {
                assert_eq!(*depth, 1);
                assert_eq!(spans_text(spans), "第一句 第二句");
            }
            other => panic!("{other:?}"),
        }
        match &blocks[1] {
            MdBlock::Quote { depth, spans } => {
                assert_eq!(*depth, 2);
                assert_eq!(spans_text(spans), "更深的一句");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn gfm_table_with_alignment_and_padded_rows() {
        let blocks = parse_markdown("| 名称 | 数量 |\n|:-----|-----:|\n| 甲 | 3 |\n| 乙\n");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        match &blocks[0] {
            MdBlock::Table {
                align,
                header,
                rows,
            } => {
                assert_eq!(align, &vec![MdAlign::Left, MdAlign::Right]);
                assert_eq!(header.len(), 2);
                assert_eq!(spans_text(&header[0]), "名称");
                assert_eq!(rows.len(), 2);
                assert_eq!(spans_text(&rows[0][1]), "3");
                // 缺列的行补空，不越界
                assert_eq!(rows[1].len(), 2);
                assert!(rows[1][1].is_empty());
            }
            other => panic!("{other:?}"),
        }
        // 没有分隔行的竖线文本仍是段落
        let blocks = parse_markdown("| 甲 | 乙 |\n不是表格");
        assert!(
            matches!(&blocks[0], MdBlock::Paragraph { .. }),
            "{blocks:?}"
        );
    }

    /// P376（池 ⑬）：行内代码段里的 `*` 是**字面量**，不参与样式开关。
    #[test]
    fn code_span_keeps_asterisks_literal() {
        assert_eq!(inline("`a**b**c`"), vec![code("a**b**c")], "代码段整段原样");
        assert_eq!(
            inline("`2 * 3`"),
            vec![code("2 * 3")],
            "代码段里的单个 `*` 也不许被吃掉"
        );

        // 反面对照：代码段**外**的标记照常开关样式
        assert_eq!(
            inline("`x` *y* **z**"),
            vec![
                code("x"),
                MdSpan::plain(" "),
                styled("y", MD_ITALIC),
                MdSpan::plain(" "),
                styled("z", MD_BOLD),
            ],
            "代码段外仍是既定语义"
        );
    }

    /// 双反引号成串：内含单反引号。
    #[test]
    fn code_span_accepts_backtick_runs() {
        assert_eq!(inline("``a`b``"), vec![code("a`b")]);
        assert_eq!(inline("`` x ``"), vec![code("x")], "首尾各一空格被剥掉");
        // 找不到同长闭串时按字面量，不吃掉后文
        assert_eq!(inline("``未闭合"), vec![MdSpan::plain("``未闭合")]);
    }

    #[test]
    fn inline_styles_toggle_and_merge() {
        let spans = inline("**粗**斜*体*普通`code`");
        assert_eq!(
            spans,
            vec![
                styled("粗", MD_BOLD),
                MdSpan::plain("斜"),
                styled("体", MD_ITALIC),
                MdSpan::plain("普通"),
                code("code"),
            ]
        );
    }

    /// 下划线粗斜体、删除线、嵌套：真实 .md 里最常见的四种。
    #[test]
    fn underscore_strike_and_nested_emphasis() {
        assert_eq!(
            inline("__粗__ _斜_ ~~删~~"),
            vec![
                styled("粗", MD_BOLD),
                MdSpan::plain(" "),
                styled("斜", MD_ITALIC),
                MdSpan::plain(" "),
                styled("删", MD_STRIKE),
            ]
        );
        assert_eq!(
            inline("**粗 *both* 粗**"),
            vec![
                styled("粗 ", MD_BOLD),
                styled("both", MD_BOLD | MD_ITALIC),
                styled(" 粗", MD_BOLD),
            ]
        );
        assert_eq!(
            inline("***三合一***"),
            vec![styled("三合一", MD_BOLD | MD_ITALIC)]
        );
    }

    /// v1 的开关式语义（未闭合一路染到行尾）换成「要求闭合」：
    /// 少一个闭栏时整段按字面量，而不是把后半行染色。
    #[test]
    fn unclosed_marker_stays_literal() {
        assert_eq!(inline("**粗但没闭栏"), vec![MdSpan::plain("**粗但没闭栏")]);
        assert_eq!(inline("尾随星号*"), vec![MdSpan::plain("尾随星号*")]);
        // 空内容不成段
        assert_eq!(inline("** **"), vec![MdSpan::plain("** **")]);
    }

    #[test]
    fn backslash_escapes_keep_punctuation_literal() {
        assert_eq!(inline("\\*不是斜体\\*"), vec![MdSpan::plain("*不是斜体*")]);
        assert_eq!(
            inline("a\\b"),
            vec![MdSpan::plain("a\\b")],
            "非标点不脱转义符"
        );
    }

    #[test]
    fn links_images_and_autolinks_carry_target() {
        assert_eq!(
            inline("看 [文档](https://e.io/a) 吧"),
            vec![
                MdSpan::plain("看 "),
                MdSpan {
                    text: "文档".into(),
                    style: MD_LINK,
                    link: Some("https://e.io/a".into()),
                },
                MdSpan::plain(" 吧"),
            ]
        );
        // 标签内的强调继承链接目标
        assert_eq!(
            inline("[**粗链**](http://x)"),
            vec![MdSpan {
                text: "粗链".into(),
                style: MD_BOLD | MD_LINK,
                link: Some("http://x".into()),
            }]
        );
        assert_eq!(
            inline("![图](C:/a/b.png)"),
            vec![MdSpan {
                text: "图".into(),
                style: MD_IMAGE | MD_LINK,
                link: Some("C:/a/b.png".into()),
            }]
        );
        assert_eq!(
            inline("<https://auto.io>"),
            vec![MdSpan {
                text: "https://auto.io".into(),
                style: MD_LINK,
                link: Some("https://auto.io".into()),
            }]
        );
        // 空目标／带标题的写法／未闭合都不成链（按字面量）
        assert_eq!(inline("[空]( )"), vec![MdSpan::plain("[空]( )")]);
        assert_eq!(
            inline("[带标题](https://e.io \"题外话\")"),
            vec![MdSpan {
                text: "带标题".into(),
                style: MD_LINK,
                link: Some("https://e.io".into()),
            }]
        );
        assert_eq!(
            inline("[没闭括号](https://e.io"),
            vec![MdSpan::plain("[没闭括号](https://e.io")]
        );
    }

    /// HTML 标签不在 v2 范围内：按字面量原样，不许吞掉内容。
    #[test]
    fn html_tags_stay_literal() {
        assert_eq!(
            inline("<br>与<div class=\"x\">"),
            vec![MdSpan::plain("<br>与<div class=\"x\">")]
        );
    }

    #[test]
    fn crlf_and_lone_cr_split_like_common_editors() {
        // \r 与 \n 都是行界；相邻普通行合并为同一段落
        let blocks = parse_markdown("# T\r\n正文A\r正文B\n");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], MdBlock::Heading { level: 1, .. }));
        match &blocks[1] {
            MdBlock::Paragraph { spans } => {
                assert_eq!(spans_text(spans), "正文A 正文B");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(parse_markdown("").is_empty());
        assert!(parse_markdown("\n\r\n  \n").is_empty());
    }

    /// 预览分行必须与编辑器的行模型同源（定案 R-11「读按全集」）：用异体行界分隔的正文，
    /// 预览该与同样内容用 `\n` 分隔时逐块相等。改前整块被当成一行＝预览说"一段"、
    /// 编辑器里却是三行。
    #[test]
    fn alternative_line_breaks_split_the_preview_like_the_editor() {
        let lf = parse_markdown("正文A\n正文B\n正文C\n");
        let alt = parse_markdown("正文A\u{2028}正文B\u{000C}正文C\u{0085}");
        assert_eq!(alt, lf, "三种异体行界都要断行");
    }

    /// 两份切行器并成一份之后，与上游行模型逐格对账（行界集是**上游实现**）。
    #[test]
    fn both_splitters_share_one_line_break_rule() {
        for text in [
            "a\nb\r\nc\rd\u{000B}e\u{000C}f\u{0085}g\u{2028}h\u{2029}i",
            "a\u{2028}\n b\r\n\u{000B}",
            "solo\n",
        ] {
            let kept = crate::document::split_lines_keep_ending(text);
            let mut expect: Vec<&str> = kept
                .iter()
                .map(|seg| crate::document::trim_line_breaks(seg))
                .collect();
            if text
                .chars()
                .next_back()
                .is_some_and(crate::search::is_line_break)
            {
                expect.push(""); // 幻影末行：与编辑器的行模型同口径
            }
            assert_eq!(
                split_lines(text),
                expect,
                "markdown 那份必须只多一个幻影末行，判据同一份：{text:?}"
            );
            assert_eq!(
                crate::document::Document::from_str(text).line_count(),
                split_lines(text).len(),
                "读侧切行的行数必须等于编辑器行模型的行数：{text:?}"
            );
        }
    }

    /// 一篇「什么都有一点」的真实 README 片段：块型序列必须逐格对上
    /// （预览的观感取决于块的顺序与类型，而不是某一块单独对不对）。
    #[test]
    fn mixed_document_block_shapes_in_order() {
        let src = "# 标题\n\n段落 **粗** 收尾\n\n- 一\n- [x] 二\n1. 三\n\n> 引用\n\n\
                   | 甲 | 乙 |\n|---|---|\n| 1 | 2 |\n\n```py\nprint(1)\n```\n\n---\n";
        let kinds: Vec<&str> = parse_markdown(src)
            .iter()
            .map(|b| match b {
                MdBlock::Heading { .. } => "heading",
                MdBlock::Paragraph { .. } => "paragraph",
                MdBlock::CodeBlock { .. } => "code",
                MdBlock::ListItem { .. } => "item",
                MdBlock::Quote { .. } => "quote",
                MdBlock::Table { .. } => "table",
                MdBlock::Rule => "rule",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "heading",
                "paragraph",
                "item",
                "item",
                "item",
                "quote",
                "table",
                "code",
                "rule"
            ],
            "{src:?}"
        );
    }

    /// 带元数据头的 `.md`（博客／静态站点生成的十有八九如此）：预览里那一段必须
    /// **整块消失**，剩下的块要与「源文件根本没有这段头」时逐格相同。
    #[test]
    fn front_matter_is_skipped_like_renderers_do() {
        let body = "# 标题\n正文";
        for src in [
            format!("---\ntitle: 你好\ntags: [a, b]\n---\n{body}"),
            format!("---\ntitle: 你好\n...\n{body}"),
            format!("+++\ntitle = \"你好\"\n+++\n{body}"),
            format!("---\n---\n{body}"),
        ] {
            assert_eq!(
                parse_markdown(&src),
                parse_markdown(body),
                "front matter 不该留下任何块：{src:?}"
            );
        }
    }

    /// 判据本身：起始栅栏顶格在第一行、且等得到配对，两者缺一都不算 front matter。
    #[test]
    fn front_matter_needs_its_opener_on_the_first_line() {
        assert_eq!(
            front_matter_end(&split_lines("---\ntitle: x\n---\n正文")),
            Some(3),
            "栅栏行是第 0、2 行 ⇒ 从第 3 行起才是正文"
        );
        assert_eq!(
            front_matter_end(&split_lines("\n---\ntitle: x\n---\n正文")),
            None,
            "首行是空行 ⇒ 不是文件头的元数据"
        );
        assert_eq!(
            front_matter_end(&split_lines("---\ntitle: x\n+++\n正文")),
            None,
            "`---` 开栏只与 `---`／`...` 配对，`+++` 不算它的收尾"
        );
        assert_eq!(
            front_matter_end(&split_lines("---\n正文")),
            None,
            "等不到配对 ⇒ 交回普通解析，那个 `---` 是分隔线"
        );
    }

    /// 反向的下界：没有配对的开篇栅栏仍是分隔线，栅栏成对但不在首行也不许吞正文。
    #[test]
    fn a_lone_or_later_fence_is_still_a_rule() {
        let kinds = |src: &str| -> Vec<&'static str> {
            parse_markdown(src)
                .iter()
                .map(|b| match b {
                    MdBlock::Heading { .. } => "heading",
                    MdBlock::Paragraph { .. } => "paragraph",
                    MdBlock::CodeBlock { .. } => "code",
                    MdBlock::ListItem { .. } => "item",
                    MdBlock::Quote { .. } => "quote",
                    MdBlock::Table { .. } => "table",
                    MdBlock::Rule => "rule",
                })
                .collect()
        };
        assert_eq!(
            kinds("---\n正文"),
            vec!["rule", "paragraph"],
            "孤立的 --- 之后正文照旧渲染"
        );
        assert_eq!(
            kinds("# H\n\n---\n\n正文"),
            vec!["heading", "rule", "paragraph"],
            "栅栏不在第一行 ⇒ 与 front matter 无关"
        );
    }

    /// 实体解码：`&amp;` 这类导出工具留下的转义，预览里该显示成它本来的字符。
    #[test]
    fn named_and_numeric_entities_are_decoded() {
        let blocks = parse_markdown("A &amp; B &lt;x&gt; 空格&nbsp;末 &#65;&#x4e2d;&#9;");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("期望一个段落：{blocks:?}");
        };
        assert_eq!(spans_text(spans), "A & B <x> 空格\u{00A0}末 A中\u{0009}");
    }

    /// 不解的那一半：不认识的名字、没分号、坏码位、被转义压住的 `&` —— 判据是
    /// **原样留着**，把 `&#xD800;` 译成一个怪字符比留着原文难查得多。
    #[test]
    fn undecodable_entities_stay_literal() {
        for src in [
            "&notreal;",
            "&amp",
            "&#;",
            "&#xZZ;",
            "&#0;",
            "&#xD800;",
            "&#x110000;",
            "\\&amp;",
        ] {
            let blocks = parse_markdown(src);
            let MdBlock::Paragraph { spans } = &blocks[0] else {
                panic!("期望一个段落：{src:?}");
            };
            assert_eq!(
                spans_text(spans),
                src.replace("\\&", "&"),
                "{src:?} 该原样显示"
            );
        }
    }

    /// 代码段里不译（CommonMark：代码内容原样），链接目标要译——不解开点出去的是错 URL。
    #[test]
    fn code_keeps_entities_but_link_targets_decode() {
        let blocks = parse_markdown("`&amp;` 与 [&amp;](https://e.io/a?x=1&amp;y=2)");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("期望一个段落：{blocks:?}");
        };
        assert_eq!(spans_text(spans), "&amp; 与 &");
        let link = spans
            .iter()
            .find_map(|s| s.link.clone())
            .unwrap_or_default();
        assert_eq!(link, "https://e.io/a?x=1&y=2", "目标里的 &amp; 要解开");
    }

    /// 硬换行的两种写法都要在段落的文本里留一个 `\n`（渲染层据此折行），
    /// 单个尾空格与被转义的双反斜杠仍算软换行——不区分这两格就会把「换行」
    /// 变成「每个回车都断行」。
    #[test]
    fn hard_line_breaks_survive_into_the_span_text() {
        let para = |src: &str| -> String {
            match &parse_markdown(src)[0] {
                MdBlock::Paragraph { spans } => spans_text(spans),
                other => panic!("期望一个段落：{other:?}"),
            }
        };
        assert_eq!(para("第一行  \n第二行"), "第一行\n第二行");
        assert_eq!(para("第一行\\\n第二行"), "第一行\n第二行");
        assert_eq!(para("甲  \n乙  \n丙"), "甲\n乙\n丙", "连着三行都硬换行");
        assert_eq!(
            para("第一行 \n第二行"),
            "第一行 第二行",
            "单个尾空格不算硬换行"
        );
        assert_eq!(
            para("第一行\\\\\n第二行"),
            "第一行\\ 第二行",
            "双反斜杠＝转义的反斜杠＋软换行"
        );
        assert_eq!(para("末行  \n"), "末行", "段尾的硬换行标记不产生多余的换行");
    }
}
