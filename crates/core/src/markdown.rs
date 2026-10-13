//! Markdown 解析（预览面板的语法层，core 纯逻辑）。
//!
//! 手写解析器（构建环境无法引入 pulldown-cmark）。v1 只有「标题/段落/列表/
//! 引用/围栏代码/三种行内标记」，且行内标记是**开关式**（未闭合的 `**` 会一路
//! 染到行尾），预览里真实 .md 文件大面积走样。v2 覆盖：
//!
//! * 块级：`#`~`######` 标题（含 ATX 闭栏 `# 标题 #`）、`---`/`***`/`_ _ _`
//!   分隔线、段落（连续非空行合并）、无序 `- * +` 与有序 `1.`/`1)` 列表
//!   （序号按 GitHub 口径**逐项重号**、缩进层级、任务勾选框，并吃**续行**）、`>` 引用（连续行合并、`>>` 深度、
//!   **内部按块级再解析**：`> - 甲` 里是一个列表项，**懒续行**＝紧跟的正文行不写 `>` 也留在引用里）、
//!   ``` ``` ``` 与 `~~~` 围栏代码（记录 info 串的语言；闭栏长度 ≥ 开栏长度）、
//!   缩进代码块（空行后 ≥4 列或制表符）、整行 HTML 注释（隐去）、GFM 表格
//!   （含 `:---`/`:---:`/`---:` 对齐、`\|` 转义竖线算单元格里的字符）
//! * 行内：`**粗**`/`__粗__`、`*斜*`/`_斜_`、`` `代码` ``（任意反引号成串）、
//!   `~~删除线~~`、`[文字](目标)` 链接、`![替代文字](地址)` 图片、
//!   `<https://…>` 自动链接、**裸 URL**（`https://…` 与 `www.…`，GFM autolink literal）、
//!   `\` 转义标点
//! * 行内标记**要求闭合**：找不到闭栏时按字面量输出（与 CommonMark 一致），
//!   不再把后半行整条染色；配对时**跳过当不了闭栏的位置**继续往后找
//!   （`**粗 ** 与 后**` 配到最后那一对，与 CommonMark 同口径）；
//!   开栏与闭栏都按 CommonMark 的**贴边**（flanking）判据——紧跟全角标点的 `**` 开不了栏、
//!   词内的 `_` 不开样式。这批判据的期望值拿 GitHub `/markdown`（cmark-gfm）逐条对拍过，
//!   并拿本仓 40 行真实日志做过整语料差分（强调数 31 比 31）
//! * **编辑面**判据（预览不碰，由应用层落进文档）：[`list_enter_action`] 接住列表项回车、
//!   [`wrap_action`] 包／解行内标记（粗体／斜体／行内代码／删除线／链接一台机器）。
//!   两者的期望值同样拿 GitHub `/markdown` 对拍过——斜体取 `*…*` 而不是 `_…_`，
//!   因为词内的 `_` 开不了栏，中文句子里 `是_重点_的` 在 GitHub 上就是字面量。
//!
//! 明确不支持（保持字面量）：HTML 标签（`<br>` 除外）。

// ── 成本尺子（只在测试里存在）─────────────────────────────────────────────
// 盯的是**为答一个是非题而做的实解析**：块级判据（`starts_new_block`）会拿产出块的
// 那一份解析当布尔问，这类浪费不改变块表输出，因此**没有任何用例能红出来**。
// 本仓口径＝成本契约用计数盯，不用毫秒断言（忙机器假红、温热假绿，见
// `crates/app/src/md_preview.rs` 里 `MD_BUILD_COUNT` 的同族说明）。
#[cfg(test)]
thread_local! {
    /// 表格**单元格**被走行内解析的次数。判据（`is_table_start`）本不该解析内容。
    pub(crate) static MD_TABLE_CELL_PARSES: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    /// 围栏代码块正文被逐行抄进 `Vec<String>` 的次数。未闭栏时这一份是整篇长度。
    pub(crate) static MD_FENCE_BODY_PUSHES: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
    /// 脚注标签的**逐条比对**次数（查定义表、查首次引用顺序，每比一对标签记一笔）。
    /// 这一把尺子量的就是工作量本身：标签表是 `Vec` 时它随「引用数 × 定义数」涨。
    pub(crate) static MD_NOTE_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_cell_parse() {
    MD_TABLE_CELL_PARSES.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn note_fence_body() {
    MD_FENCE_BODY_PUSHES.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn note_scan() {
    MD_NOTE_SCANS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(test))]
#[inline]
fn note_cell_parse() {}

#[cfg(not(test))]
#[inline]
fn note_fence_body() {}

#[cfg(not(test))]
#[inline]
fn note_scan() {}

/// 行内样式位标志。
pub const MD_BOLD: u8 = 1 << 0;
pub const MD_ITALIC: u8 = 1 << 1;
pub const MD_CODE: u8 = 1 << 2;
pub const MD_STRIKE: u8 = 1 << 3;
pub const MD_LINK: u8 = 1 << 4;
pub const MD_IMAGE: u8 = 1 << 5;
/// 脚注引用：`text` 先放标签，编号在块级收尾时统一换算成上标数字。
pub const MD_FOOTNOTE: u8 = 1 << 6;

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
    pub fn is_footnote(&self) -> bool {
        self.style & MD_FOOTNOTE != 0
    }
}

/// 一条脚注：编号＋正文（正文已走行内解析）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MdFootnote {
    pub number: usize,
    pub spans: Vec<MdSpan>,
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
    /// 引用块：`depth` ＝行首连续 `>` 的层数；`blocks` ＝**去栏之后**按同一份块级解析走出来的
    /// 内部块——`> - 甲` 里面是一个真的 ListItem，不是"- 甲"这两个字（改前整块只并成一行正文，
    /// 引用里的列表／标题／围栏都把记号原样画出来）。
    Quote {
        depth: usize,
        blocks: Vec<MdBlock>,
    },
    Table {
        align: Vec<MdAlign>,
        header: Vec<Vec<MdSpan>>,
        rows: Vec<Vec<Vec<MdSpan>>>,
    },
    Rule,
    /// 文末注脚区：整篇**最多一块**，永远排在最后（脚注定义行本身不出正文）。
    Footnotes {
        items: Vec<MdFootnote>,
    },
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
///
/// 三步：切行 ⇒ 收定义（引用式链接＋脚注，两者都要看整篇）⇒ `parse_lines` 走块级
/// ⇒ 收尾发脚注编号。引用块内部走的是**同一份** `parse_lines`，所以 `> - 甲` 里
/// 是一个真的列表项而不是"- 甲"这两个字。
pub fn parse_markdown(text: &str) -> Vec<MdBlock> {
    let lines = split_lines(text);
    // 引用式链接的定义先收一遍整篇：定义行可以写在用到它的那句**后面**（前向引用）
    let (defs, is_def) = collect_link_defs(&lines);
    let doc_begin = front_matter_end(&lines).unwrap_or(0);
    let mut blocks = parse_lines(lines.as_slice(), &defs, &is_def, doc_begin);
    renumber_ordered_lists(&mut blocks);
    assign_footnotes(&mut blocks, &defs);
    blocks
}

/// 块级主循环。`doc_begin` ＝第一篇正文的起始行（front matter 之后），缩进代码那一支
/// 用它判断"上面没有正文行可依赖"。
fn parse_lines(lines: &[&str], defs: &LinkDefs, is_def: &[bool], doc_begin: usize) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut i = doc_begin;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // 定义行不产生块：它的去处是查表，不是画在预览里
        if is_def[i] {
            i += 1;
            continue;
        }

        // 缩进代码排在围栏**之前**：4 格以上的 ``` 是代码内容，不是开栏（CommonMark 只允许 3 格）
        if let Some((block, next)) = indented_code(lines, i, doc_begin) {
            blocks.push(block);
            i = next;
            continue;
        }

        // HTML 注释整块隐去（与浏览器同一观感）
        if let Some(next) = html_comment(lines, i) {
            i = next;
            continue;
        }

        if let Some((block, next)) = fence(lines, i) {
            blocks.push(block);
            i = next;
            continue;
        }

        if let Some((level, content)) = heading(trimmed) {
            blocks.push(MdBlock::Heading {
                level,
                spans: inline_with(content, defs),
            });
            i += 1;
            continue;
        }

        if is_rule(trimmed) {
            blocks.push(MdBlock::Rule);
            i += 1;
            continue;
        }

        if let Some((block, next)) = table(lines, i, defs) {
            blocks.push(block);
            i = next;
            continue;
        }

        if let Some((depth, content)) = quote(trimmed) {
            // 同深度的连续引用行收在一起**去栏**后交给同一份 parse_lines：引用里的列表、
            // 标题、代码围栏、表格都是真块。`>>` 的深度在这一格收成 depth，
            // 去栏去掉了所有 `>` ⇒ 内部不会再套出第二层 Quote。
            let mut inner: Vec<&str> = vec![content];
            i += 1;
            while i < lines.len() {
                match quote(lines[i].trim()) {
                    Some((same, next_content)) if same == depth => {
                        inner.push(next_content);
                        i += 1;
                    }
                    _ => break,
                }
            }
            // 懒续行：紧跟在 `>` 行后面的普通正文行**仍属于这个引用**（GitHub 把它放进同一个
            // `<blockquote>`），只在它不是"另起一块"的开头时收进来。改前它掉到引用外面另起一段
            // ＝写引用时第二行忘了 `>` 就会让引用只剩一行。
            while i < lines.len() {
                let t = lines[i].trim();
                if t.is_empty() || starts_new_block(lines, i, is_def) {
                    break;
                }
                inner.push(t);
                i += 1;
            }
            let no_def = vec![false; inner.len()];
            blocks.push(MdBlock::Quote {
                depth,
                blocks: parse_lines(&inner, defs, &no_def, 0),
            });
            continue;
        }

        if let Some(item) = list_item(lines[i]) {
            // 一个列表项可能占好几行：正文写完直接回车换行（缩进够 or CommonMark 的
            // lazy continuation 不缩进），或空一行再接第二段。改前这些续行统统掉回
            // 段落分支——项目符号后面接的话被顶到列表外面另起一段，列表看着断了。
            // 项首行行尾的空白是硬换行标记（见 `line_break_kind`），不是正文——与段落
            // 分支同一口径，先摘掉再拼接
            let mut body = String::from(item.content.trim_end());
            let mut prev_raw = lines[i];
            let mut blanks = 0usize;
            let mut k = i + 1;
            while k < lines.len() {
                let raw = lines[k];
                let t = raw.trim();
                if t.is_empty() {
                    blanks += 1;
                    k += 1;
                    continue;
                }
                // 另起一块的开头（标题/围栏/分隔线/表格/引用/下一个列表项/定义行）不吃；
                // 空过行之后只有缩进够到正文列的那一行还归本项
                let belongs = !starts_new_block(lines, k, is_def)
                    && (blanks == 0 || indent_cols(raw) >= item.content_col);
                if !belongs {
                    break;
                }
                let (sep, drop_backslash) = line_break_kind(prev_raw);
                if drop_backslash {
                    // 行尾那个 `\` 是换行标记本身，不进正文文本
                    body.pop();
                }
                if !body.is_empty() {
                    // 空过行＝另起一段：用 `\n` 断在项内，比并成一行更像原来的写法
                    body.push(if blanks > 0 { '\n' } else { sep });
                }
                body.push_str(t);
                blanks = 0;
                prev_raw = raw;
                k += 1;
            }
            blocks.push(MdBlock::ListItem {
                order: item.order,
                indent: item.indent,
                task: item.task,
                spans: inline_with(&body, defs),
            });
            i = k;
            continue;
        }

        let mut paragraph = String::from(trimmed);
        let mut prev_raw = lines[i];
        i += 1;
        let mut setext: Option<usize> = None;
        while i < lines.len() {
            let next = lines[i].trim();
            // 下划线优先于分隔线：`正文\n---` 是二级标题，不是「段落＋横线」
            if let Some(level) = setext_level(next) {
                if !paragraph.contains('\n') {
                    setext = Some(level);
                    i += 1;
                    break;
                }
            }
            if next.is_empty() || starts_new_block(lines, i, is_def) {
                break;
            }
            let (sep, drop_backslash) = line_break_kind(prev_raw);
            if drop_backslash {
                // 行尾那个 `\` 是换行标记本身，不进正文文本
                paragraph.pop();
            }
            paragraph.push(sep);
            paragraph.push_str(next);
            prev_raw = lines[i];
            i += 1;
        }
        let spans = inline_with(&paragraph, defs);
        blocks.push(match setext {
            Some(level) => MdBlock::Heading { level, spans },
            None => MdBlock::Paragraph { spans },
        });
    }
    blocks
}

/// 有序项**重号**。口径拿 GitHub `/markdown` 现量对拍过：`5. 五 / 7. 七 / 空行 / 9. 九 / 2. 二`
/// 在 GitHub 是**一个** `<ol start="5">`（浏览器渲染 5,6,7,8），也就是——除了第一项写下的号，
/// 后面的号一律不采信、逐项 +1，**空行也不另起一个列表**。改前我们原样回声，
/// 于是预览里会冒出「1. 1. 9.」这种作者改完条目忘了改号的数字。
///
/// 层与层各起各的号；中间插入一个无序项 ⇒ 那层的连续号断掉（GitHub 也是两个列表）；
/// 回到上一层时接着那层已发到的号继续走。
fn renumber_ordered_lists(blocks: &mut [MdBlock]) {
    // (缩进层, 下一个该发的号)
    let mut levels: Vec<(usize, usize)> = Vec::new();
    for block in blocks.iter_mut() {
        match block {
            MdBlock::ListItem {
                order: Some(n),
                indent,
                ..
            } => {
                levels.retain(|(d, _)| *d <= *indent);
                let next = match levels.iter_mut().find(|(d, _)| *d == *indent) {
                    Some((_, next)) => {
                        let v = *next;
                        *next = v + 1;
                        v
                    }
                    None => {
                        levels.push((*indent, *n + 1));
                        *n
                    }
                };
                *n = next;
            }
            // 无序项／别的块都把这层的号断掉
            MdBlock::ListItem { order: None, .. } => levels.clear(),
            MdBlock::Quote { blocks: inner, .. } => {
                renumber_ordered_lists(inner);
                levels.clear();
            }
            _ => levels.clear(),
        }
    }
}

/// 数字 → 上标字符（`¹²³…`）：预览没有「上标」这种排版属性，用 Unicode 上标数字既不用
/// 多带一份状态，也不会把行距拉开。认不到的字符原样留着。
/// 渲染层注脚区的行首编号共用这一份（正文标记与注脚行的字形必须同一个来源）。
pub fn superscript_digits(n: usize) -> String {
    n.to_string()
        .chars()
        .map(|d| match d {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            other => other,
        })
        .collect()
}

/// 逐块过一遍**所有**行内段（脚注收尾要按全篇顺序看，一处漏了就少一个编号）。
fn each_span_list(blocks: &mut [MdBlock], f: &mut impl FnMut(&mut Vec<MdSpan>)) {
    for block in blocks.iter_mut() {
        match block {
            MdBlock::Heading { spans, .. } | MdBlock::Paragraph { spans } => f(spans),
            // 引用块内部也是块：脚注编号要一起走进去
            MdBlock::Quote { blocks, .. } => each_span_list(blocks, f),
            MdBlock::ListItem { spans, .. } => f(spans),
            MdBlock::Table { header, rows, .. } => {
                for cell in header.iter_mut().chain(rows.iter_mut().flatten()) {
                    f(cell);
                }
            }
            MdBlock::Footnotes { items } => {
                for item in items.iter_mut() {
                    f(&mut item.spans);
                }
            }
            // 代码块与分隔线里没有行内标记
            MdBlock::CodeBlock { .. } | MdBlock::Rule => {}
        }
    }
}

/// 脚注收尾：①按**首次引用**的出现顺序发号，把正文里那截标签换成上标数字；②注脚区挂在
/// 最后一块——被引用的按编号排，**从没被引用的定义也排在后面**（用户写下的正文不该消失）。
/// 一张定义表都没有 ⇒ 不产生块。
fn assign_footnotes(blocks: &mut Vec<MdBlock>, defs: &LinkDefs) {
    if defs.notes.is_empty() {
        return;
    }
    let mut order: Vec<String> = Vec::new();
    each_span_list(blocks, &mut |spans: &mut Vec<MdSpan>| {
        for span in spans.iter_mut() {
            if span.style & MD_FOOTNOTE == 0 {
                continue;
            }
            let label = normalize_label(&span.text);
            let idx = match order.iter().position(|l| {
                note_scan();
                *l == label
            }) {
                Some(idx) => idx,
                None => {
                    order.push(label);
                    order.len() - 1
                }
            };
            span.text = superscript_digits(idx + 1);
        }
    });
    let mut items: Vec<MdFootnote> = Vec::new();
    let mut emitted = vec![false; defs.notes.len()];
    for label in order.iter() {
        if let Some(at) = defs.notes.iter().position(|(l, _)| {
            note_scan();
            l == label
        }) {
            emitted[at] = true;
            items.push(MdFootnote {
                number: items.len() + 1,
                spans: inline_with(&defs.notes[at].1, defs),
            });
        }
    }
    for (at, (_, body)) in defs.notes.iter().enumerate() {
        if !emitted[at] {
            items.push(MdFootnote {
                number: items.len() + 1,
                spans: inline_with(body, defs),
            });
        }
    }
    if !items.is_empty() {
        blocks.push(MdBlock::Footnotes { items });
    }
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

/// 段落下划线式的标题（setext）：整行都是 `=` ⇒ 一级，整行都是 `-` ⇒ 二级。
///
/// 只在**正在攒的那段正文没被硬换行打断**时才成立（CommonMark 明确排除含硬换行的
/// setext 标题），也只在上面真有一段正文时才查——空行之后的 `---` 仍是分隔线。
fn setext_level(line: &str) -> Option<usize> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
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
    /// 正文起始**列**（行首缩进 + 标记宽度 + 标记后的空白）：续行的归属判据用它。
    content_col: usize,
}

/// 行首空白的列宽（制表符按 4 列，与 [`list_item`] 的缩进层级同一口径）。
fn indent_cols(raw: &str) -> usize {
    let mut cols = 0usize;
    for c in raw.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4,
            _ => break,
        }
    }
    cols
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
    // 标记本身的列宽（无序 1 列；有序＝数字位数 + `.`/`)`），续行归属判据要用
    let (order, after, marker_cols) = if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        (None, after, 1usize)
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
        (Some(order), after, digits + 1)
    };
    // 标记后必须跟空白，且不能是分隔线（`---` 已在块级先判掉）
    if !after.starts_with([' ', '\t']) {
        return None;
    }
    let ws_cols = indent_cols(after);
    let content = after.trim_start_matches([' ', '\t']);
    let mut task = None;
    let mut body = content;
    // GFM 的任务勾选框两种列表都有（改前只认无序项，`1. [x]` 那格把框当正文原样画出来）
    for (mark, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
        if let Some(rest) = content.strip_prefix(mark) {
            task = Some(done);
            body = rest;
            break;
        }
    }
    Some(MdListItem {
        order,
        indent: (cols / 2).min(6),
        task,
        content: body,
        content_col: cols + marker_cols + ws_cols,
    })
}

/// 列表／引用**回车续写**的两种结果（编辑面，预览解析器不用它）。
///
/// 只判**当前这一行**（不看上下文），因为按回车时唯一确定的就是光标所在行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdListEnter {
    /// 接着开下一项：`prefix` ＝缩进＋标记＋空格（有序已进位、任务框给空框）。
    /// `from_col` ＝正文起始列；光标还在标记之前时不接管（那一下就是普通回车）。
    Next { prefix: String, from_col: usize },
    /// 正文是空的 ⇒ **退出这一层**：删掉行内 `[marker_start, marker_end)` 那截标记。
    /// 起点不是恒 0——引用块里的空列表项只该摘掉列表那截、留着栏
    /// （见 [`quote_list_enter_action`]），一层一层退才按得下手。
    Exit {
        marker_start: usize,
        marker_end: usize,
    },
}

/// 列表项回车续写的判据（编辑面）。`marker_end` ＝正文起始的**字符列**，
/// 因标记区全是 ASCII，它同时是字节偏移。
///
/// * `- 甲` → `Next("- ")`；`3. 甲` → `Next("4. ")`（保持零位数宽）；
/// * `- [x] 甲` → `Next("- [ ] ")`（勾带过来就变成"新项也已完成"，是错的）；
/// * `- `（正文空）→ `Exit{2}`：回车退出这个列表，而不是无限接空项。
pub fn list_enter_action(line: &str) -> Option<MdListEnter> {
    let head_len: usize = line
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(char::len_utf8)
        .sum();
    let head = &line[..head_len];
    let rest = &line[head_len..];
    // 标记：无序 1 字符；有序＝数字 + `.`/`)`
    let (marker_text, marker_bytes) = match rest.chars().next() {
        Some(c @ ('-' | '*' | '+')) => (c.to_string(), 1usize),
        Some(d) if d.is_ascii_digit() => {
            let digits: usize = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .map(char::len_utf8)
                .sum();
            let sep = rest[digits..]
                .chars()
                .next()
                .filter(|c| matches!(c, '.' | ')'))?;
            let n: usize = rest[..digits].parse().ok()?;
            // 零位宽度保持：`08.` 的下一项是 `09.`，不是 `9.`
            (
                format!("{:0>width$}{sep}", n + 1, width = digits),
                digits + 1,
            )
        }
        _ => return None,
    };
    // 标记后必须紧跟空白（`-abc`、`1.ab` 都不是列表）
    let after = &rest[marker_bytes..];
    let spaces: usize = after
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(char::len_utf8)
        .sum();
    if spaces == 0 {
        return None;
    }
    let body = &after[spaces..];
    // 任务框算标记的一部分（有序项同样有，与 `list_item` 的勾选框判据同一口径）：
    // 新项给一个**空框**——把勾带过来就变成「新项也已完成」，那是错的
    let (box_len, box_text, body_text) = match ["[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|m| body.strip_prefix(m))
    {
        Some(tail) => {
            let ws = tail.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
            (3 + ws, "[ ]", &tail[ws..])
        }
        None => (0, "", body),
    };
    let marker_end = head_len + marker_bytes + spaces + box_len;
    if body_text.trim().is_empty() {
        return Some(MdListEnter::Exit {
            marker_start: 0,
            marker_end,
        });
    }
    let prefix = if box_text.is_empty() {
        format!("{head}{marker_text} ")
    } else {
        format!("{head}{marker_text} {box_text} ")
    };
    Some(MdListEnter::Next {
        prefix,
        from_col: marker_end,
    })
}

/// 引用块里的回车续写（编辑面）：行首那几道 `>` 栏跟着接下去，栏里的列表一起接。
///
/// 引用判据与预览解析器**共用一份**（`quote`）——`>甲` 这种栏后不带空格的写法
/// 解析器认，编辑面若不认，同一行就在两处各有一个答案。
///
/// * `> 甲` → `Next("> ")`；`> - 甲` → `Next("> - ")`（`d77caa1` 之后引用块内部
///   本来就按块级解析，栏里的列表是真列表项）；`>> 甲` → `Next(">> ")`；
/// * 栏后没有正文（`>`／`> `）⇒ `Exit` 摘掉那截栏 ⇒ 回车退出引用；
/// * 栏里剩一个空列表项（`> - `）⇒ `Exit` **只删列表那截、留着栏**：一层一层退，
///   按一下键跳掉两层太陡；
/// * 栏后是别的块级开头（`> # 标题`）⇒ 照样续栏——续的是「引用」这一层，与栏里
///   装的是什么块无关（标题本身不续行，那是这一行之外的事）；
/// * 非引用行原样交回 [`list_enter_action`]，普通列表的行为一字不变。
///
/// 续出去的栏一律本仓的正规形（每层一道 `>`、末尾一个空格，行首缩进原样带过来）；
/// 而 `marker_start`／`marker_end`／`from_col` 说的是**当前这一行**的字符列，
/// 所以按原样写的那截（`>甲`、`>  `）用它自己的长度。
pub fn quote_list_enter_action(line: &str) -> Option<MdListEnter> {
    // 不是引用行 ⇒ 原样交回普通列表判据（应用层只调这一个函数，这条退路
    // 就是「普通列表的行为一字不变」的落点）
    let Some((depth, _)) = quote(line) else {
        return list_enter_action(line);
    };
    // 行首缩进（与 list_enter_action 同一口径：`  > 甲` 也是引用）
    let head_len: usize = line
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(char::len_utf8)
        .sum();
    // 栏那截按**原样**数到头：`>  ` 后面那两个空格也属于标记，摘栏时要一起摘掉
    let mut bar_end = head_len;
    for _ in 0..depth {
        bar_end += 1; // `>` 本身（quote() 已确认它在这儿，且恒为 ASCII）
        bar_end += line[bar_end..]
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .map(char::len_utf8)
            .sum::<usize>();
    }
    let written = line[..bar_end].chars().count();
    // 续出去的栏＝每层一道 `>` ＋一个空格（`>>` 而不是 `> >`：与用户自己写的那形一致）
    let bars = format!("{}{} ", &line[..head_len], ">".repeat(depth));
    let body = &line[bar_end..];
    match list_enter_action(body) {
        Some(MdListEnter::Next { prefix, from_col }) => Some(MdListEnter::Next {
            prefix: format!("{bars}{prefix}"),
            from_col: written + from_col,
        }),
        Some(MdListEnter::Exit { marker_end, .. }) => Some(MdListEnter::Exit {
            marker_start: written,
            marker_end: written + marker_end,
        }),
        None => {
            if body.trim().is_empty() {
                Some(MdListEnter::Exit {
                    marker_start: 0,
                    marker_end: written,
                })
            } else {
                Some(MdListEnter::Next {
                    prefix: bars,
                    from_col: written,
                })
            }
        }
    }
}

/// 行内标记的种类（**编辑面**：包选区与解包共用一台机器，预览解析器不用它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdInlineMarker {
    /// `**正文**`
    Bold,
    /// `*正文*`。**不是** `_正文_`：本仓与 GitHub 对拍过的贴边判据里词内的 `_`
    /// 开不了栏（见 `emphasis_flanking_matches_the_github_reference` 钉的 `_词界_斜`），
    /// 而中文句子里被选中的词两侧恰恰紧邻汉字——用 `_` 会让「按了键却没变斜」成常态。
    Italic,
    /// `` `正文` ``
    Code,
    /// `~~正文~~`
    Strike,
    /// `[正文](目标)`——非对称的一对，包完把光标放进目标槽等着填 URL。
    Link,
}

impl MdInlineMarker {
    /// 这一对的开栏串与闭栏串（替换文本直接拿它拼）。
    pub fn pair(self) -> (&'static str, &'static str) {
        match self {
            Self::Bold => ("**", "**"),
            Self::Italic => ("*", "*"),
            Self::Code => ("`", "`"),
            Self::Strike => ("~~", "~~"),
            Self::Link => ("[", "]()"),
        }
    }
}

/// [`wrap_action`] 的产出＝要落下的那一处替换。
///
/// 替换的**范围**＝「选区再向前吃 [`MdWrapEdit::head_cut`] 个字符、向后吃
/// [`MdWrapEdit::tail_cut`] 个字符」——解包时那两截记号本来就长在选区外面。
/// **偏移一律按字符计**，且相对 [`MdWrapEdit::text`] 的起点：判据只吃字符串本身，
/// 字符↔列↔字节的换算留在编辑面，CJK 宽度这类坑就不至于两边各踩一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdWrapEdit {
    /// 换进上面那个范围的文本。
    pub text: String,
    /// 范围向选区**前**多吃的字符数（＝连选区外的开栏记号一起删）。
    pub head_cut: usize,
    /// 范围向选区**后**多吃的字符数。
    pub tail_cut: usize,
    /// 替换后的新选区（起点，长度）；`None` ⇒ 不保留选区，只落光标。
    pub select: Option<(usize, usize)>,
    /// 替换后的光标位置（字符偏移）。
    pub caret: usize,
}

/// [`wrap_action`] 的 `before`／`after` 至少给多少个字符才判得出解包。
///
/// 最长的记号串是链接闭栏 `]()`（3 字符），给 4 留一格余量。编辑面按这个数取窗，
/// **不物化整行**——单行 50 MB 的日志取整行串是本仓禁项（P298 立下的口径）。
pub const WRAP_CONTEXT_CHARS: usize = 4;

/// 包／解行内标记的判据（编辑面）。`sel` ＝当前选中的文本，`None` ＝没有选区；
/// `before`／`after` ＝选区紧邻的前／后文本，各给 [`WRAP_CONTEXT_CHARS`] 个字符就够
/// （给长了也无妨——判据只看两端各记号串那么长的一截）。四条落点，按判据顺序：
///
/// ① 选区**整尾**就是这一对（连记号一起选中了 `**甲**`）⇒ 剥掉选区首尾那两截；
/// ② 记号长在选区**外面**（`**甲**` 里只选中 `甲`，或光标停在 `****` 中间）⇒ 把替换
///    范围扩到那两截记号上删掉，正文照常选中。**这一条才是「再按一次就取消」的开关**：
///    第一次包完，选区里剩的是正文，只判选区自身永远解不掉；
/// ③ 没有正文（无选区或空选区）⇒ 插一对空记号，光标落在该写正文的那一格；
/// ④ 其余 ⇒ 首尾各加一截记号，选区两端的空格／制表符提到**标记外**——连选常把行首
///    缩进或词后空白一起带上，那些空白不该被关进粗体里。
///
/// 选区里本来就含同一记号（`*甲*` 再按一次斜体）⇒ 照样再加一层，**不做智能升降级**：
/// 要分清「这一层是不是已有的一半」得把整行重解析，代价与错判面都不划算。跨行的选区
/// 照包，因此**不带**「多行不生效」的提醒分支——段落里连续行本来就并成一份文本，
/// `**甲\n乙**` 确实是一段粗体（钉在 `bold_span_may_cross_lines_in_a_paragraph`）。
pub fn wrap_action(sel: Option<&str>, before: &str, after: &str, m: MdInlineMarker) -> MdWrapEdit {
    let (open, close) = m.pair();
    let open_chars = open.chars().count();
    let close_chars = close.chars().count();
    let body = sel.unwrap_or("");
    let body_chars = body.chars().count();
    // ① 选区自己就是包好的一对。先剥头再剥尾，所以 `**`（只有一截开栏）不会把头尾
    // 算成同一串字符；剥完正文可以为空 ⇒ 选中的本来就是 `****` 那对空标记，整对删掉。
    if let Some(inner) = sel
        .and_then(|s| s.strip_prefix(open))
        .and_then(|rest| rest.strip_suffix(close))
    {
        let n = inner.chars().count();
        return MdWrapEdit {
            text: inner.to_owned(),
            head_cut: 0,
            tail_cut: 0,
            select: (n > 0).then_some((0, n)),
            caret: n,
        };
    }
    // ② 记号紧贴选区两侧 ⇒ 把它们吃进替换范围删掉，正文原样留着并继续选中。
    if before.ends_with(open) && after.starts_with(close) {
        return MdWrapEdit {
            text: body.to_owned(),
            head_cut: open_chars,
            tail_cut: close_chars,
            select: (body_chars > 0).then_some((0, body_chars)),
            caret: body_chars,
        };
    }
    // ③ 空正文 ⇒ 只放一对空记号，光标停在中间那一格。
    if body_chars == 0 {
        return MdWrapEdit {
            text: format!("{open}{close}"),
            head_cut: 0,
            tail_cut: 0,
            select: None,
            caret: open_chars,
        };
    }
    // ④ 两端空白提到标记外。还是先摘头再摘尾（同 ① 的道理），于是纯空白的选区
    // 只会得到空正文；那时不提了——把 `   ` 拆成标记外的空白与标记内的空正文
    // 没有意义，原样包起来反倒看得出发生了什么。
    let lead_len: usize = body
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(char::len_utf8)
        .sum();
    let after_lead = &body[lead_len..];
    let trail_len: usize = after_lead
        .chars()
        .rev()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .map(char::len_utf8)
        .sum();
    let (lead, inner, trail) = if after_lead.len() == trail_len {
        ("", body, "")
    } else {
        (
            &body[..lead_len],
            &after_lead[..after_lead.len() - trail_len],
            &body[body.len() - trail_len..],
        )
    };
    let body_off = lead.chars().count() + open_chars;
    let inner_chars = inner.chars().count();
    let text = format!("{lead}{open}{inner}{close}{trail}");
    let (select, caret) = if m == MdInlineMarker::Link {
        // 正文已经就位 ⇒ 光标进 `]()` 的两括号中间等着填目标，不再保留选区
        (None, body_off + inner_chars + close_chars - 1)
    } else {
        (Some((body_off, inner_chars)), body_off + inner_chars)
    };
    MdWrapEdit {
        text,
        head_cut: 0,
        tail_cut: 0,
        select,
        caret,
    }
}

/// 缩进代码块（4 列／制表符）：把行首那 4 列吃掉，剩下的原样留着（含相对缩进）。
fn strip_first_four_columns(line: &str) -> String {
    let mut rest = line;
    let mut cols = 0usize;
    while cols < 4 {
        match rest.chars().next() {
            Some(' ') => {
                rest = &rest[1..];
                cols += 1;
            }
            Some('\t') => {
                rest = &rest[1..];
                cols += 4;
            }
            _ => break,
        }
    }
    rest.to_owned()
}

/// 缩进代码块：行首空白 ≥4 列的那一行起头，一路吃到「缩进不够的正文行」为止，
/// 中间的空行留着（那是代码里的空行），末尾连续空行丢掉。
///
/// 带上「上一行必须是空行（或就是文件头）」这一格判据：紧跟正文的缩进行是段落的
/// 懒续行，紧跟列表项的缩进行是子项／续行——少了这一格就会把 `- 甲` 下面的
/// `    - 子项` 变成代码，把上一轮刚接上的列表再拆坏。
fn indented_code(lines: &[&str], start: usize, doc_begin: usize) -> Option<(MdBlock, usize)> {
    let starts_clean = start == doc_begin || lines[start - 1].trim().is_empty();
    if !starts_clean || indent_cols(lines[start]) < 4 {
        return None;
    }
    let mut body = Vec::new();
    let mut blanks = 0usize;
    let mut k = start;
    while k < lines.len() {
        let line = lines[k];
        if line.trim().is_empty() {
            blanks += 1;
            k += 1;
            continue;
        }
        if indent_cols(line) < 4 {
            break;
        }
        for _ in 0..blanks {
            body.push(String::new());
        }
        blanks = 0;
        body.push(strip_first_four_columns(line));
        k += 1;
    }
    Some((
        MdBlock::CodeBlock {
            lang: None,
            lines: body,
        },
        k,
    ))
}

/// 围栏代码块：```` ``` ```` 与 `~~~`，记录 info 串首词作语言；未闭栏也成块。
///
/// 闭栏要**不短于开栏**且整行除空白外只有同种标记——` ```` ` 里放 ` ``` ` 是教
/// Markdown 的文档最常见的写法，改前内层那三根就收了栏，剩下的当成正文染色。
fn fence(lines: &[&str], start: usize) -> Option<(MdBlock, usize)> {
    let open = lines[start].trim_start();
    // ``` 与 ~~~ 都是围栏（两种 marker 都是三字节 ASCII，切片落在字符边界上）
    let backtick = open.strip_prefix("```");
    let tilde = open.strip_prefix("~~~");
    let marker = match (backtick, tilde) {
        (Some(_), _) => '`',
        (None, Some(_)) => '~',
        (None, None) => return None,
    };
    let open_len = open.chars().take_while(|c| *c == marker).count();
    let info = &open[open_len..];
    let lang = info.split_whitespace().next().map(str::to_owned);
    let mut body = Vec::new();
    let mut k = start + 1;
    while k < lines.len() {
        let candidate = lines[k].trim();
        let run = candidate.chars().take_while(|c| *c == marker).count();
        // 闭栏：长度 ≥ 开栏，且这一行除空白外没有别的字符（带 info 的那行是内容）
        if run >= open_len && candidate.chars().all(|c| c == marker) {
            return Some((MdBlock::CodeBlock { lang, lines: body }, k + 1));
        }
        body.push({
            note_fence_body();
            lines[k].to_owned()
        });
        k += 1;
    }
    Some((MdBlock::CodeBlock { lang, lines: body }, k))
}

/// 表格第一行的判定（段落续行要用它断句）。
///
/// 判据只看**形状**（[`table_shape`]）。改前它写成 `table(lines, i, defs).is_some()`，
/// 为了回答「这一行是不是新块」把整张表**每一格都做了行内解析**再丢掉：段落紧贴表格
/// 的文档里，一张 1204 格的表因此被解析两遍（现量＝格解析 2408 次 vs 1204 次）。
fn is_table_start(lines: &[&str], i: usize) -> bool {
    table_shape(lines, i).is_some()
}

/// 这一行是不是 HTML 注释的起头（段落的续行判据要用它断句）。
fn is_comment_start(line: &str) -> bool {
    line.trim().starts_with("<!--")
}

/// HTML 注释 ⇒ 预览里**整块隐去**（浏览器也是这么处理的）：README 里那一排徽章／锚点
/// 注释原先一行行显示出来，看着像排版坏了。
///
/// 只吃**从行首起**的注释：单行闭合（`<!-- … -->`）或跨行吃到 `-->`。行中间的注释
/// 不动——那是正文的一部分，删掉等于吃字。等不到收尾就交回普通解析（原样看得见，
/// 比静默吞掉后半篇正文好查）。
fn html_comment(lines: &[&str], start: usize) -> Option<usize> {
    let body = lines[start].trim().strip_prefix("<!--")?;
    if body.contains("-->") {
        return Some(start + 1);
    }
    let mut k = start + 1;
    while k < lines.len() {
        if lines[k].contains("-->") {
            return Some(k + 1);
        }
        k += 1;
    }
    None
}

/// 这一行**看起来**是不是围栏开栏（`starts_new_block` 的判据专用）。
///
/// 改前那一问答的是 `fence(lines, i).is_some()`，而 `fence()` 无论闭没闭栏都返回
/// `Some`——于是为了知道"是不是新块"，它把从这一行到收尾（未闭栏＝到文档末尾）的
/// **整段正文逐行抄进一个新的 `Vec<String>` 再丢掉**。段落紧贴一段没闭栏的代码那种
/// 写法（贴日志、贴报错最常见）现量＝抄行 720 vs 360，正好整篇两遍。
/// 布尔判据只需要行首，这一份就只看行首。
fn looks_like_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// 这一行是不是**另起一块**的开头：段落的续行与列表项的续行共用这一份判据
/// （两处各写一遍就会各漏一格，比如列表项吃了 `>` 引用行）。
fn starts_new_block(lines: &[&str], i: usize, is_def: &[bool]) -> bool {
    let t = lines[i].trim();
    looks_like_fence(lines[i])
        || heading(t).is_some()
        || is_rule(t)
        || is_table_start(lines, i)
        || quote(t).is_some()
        || list_item(lines[i]).is_some()
        || is_comment_start(lines[i])
        || is_def[i]
}

/// 表格的**形状**：`(各列对齐方式, 表格之后下一行的下标)`。不成表 ⇒ `None`。
///
/// 一份两用：[`table`] 用它定位要填内容的区间，[`is_table_start`] 用它答布尔。
/// 这一份**不做行内解析**，所以"这一行是不是新块"那一问的代价与表的格数无关。
fn table_shape(lines: &[&str], start: usize) -> Option<(Vec<MdAlign>, usize)> {
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
    let mut k = start + 2;
    while k < lines.len() {
        let line = lines[k].trim();
        if line.is_empty() || !line.contains('|') {
            break;
        }
        k += 1;
    }
    Some((align, k))
}

/// GFM 表格：表头行 + 分隔行（只含 `| : - 空格`）+ 数据行。
///
/// 形状判据在 [`table_shape`]，这里只把区间内的格子填上内容。
fn table(lines: &[&str], start: usize, defs: &LinkDefs) -> Option<(MdBlock, usize)> {
    let (align, end) = table_shape(lines, start)?;
    let cols = align.len();
    let header = normalize_row(
        split_row(lines[start].trim())
            .into_iter()
            .map(|cell| {
                note_cell_parse();
                inline_with(cell, defs)
            })
            .collect(),
        cols,
    );
    let mut rows = Vec::new();
    for line in &lines[start + 2..end] {
        let row: Vec<Vec<MdSpan>> = split_row(line.trim())
            .into_iter()
            .map(|cell| {
                note_cell_parse();
                inline_with(cell, defs)
            })
            .collect();
        rows.push(normalize_row(row, cols));
    }
    Some((
        MdBlock::Table {
            align,
            header,
            rows,
        },
        end,
    ))
}

/// 切表格一行：只切在**没被转义**的竖线上。
///
/// `\|` 是单元格里的一个竖线字符（GFM 口径）。改前它照样分列，多出来那一列被
/// `normalize_row` 按列数裁掉＝用户写的字静默消失。反斜杠按**奇数串**才算转义，
/// 所以 `\\|` 是「转义的反斜杠＋真分列」。
fn split_row(line: &str) -> Vec<&str> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut start = 0usize;
    let mut slashes = 0usize;
    for (idx, c) in t.char_indices() {
        match c {
            '\\' => slashes += 1,
            '|' if slashes % 2 == 1 => slashes = 0,
            '|' => {
                cells.push(t[start..idx].trim());
                start = idx + 1;
                slashes = 0;
            }
            _ => slashes = 0,
        }
    }
    cells.push(t[start..].trim());
    cells
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

/// 引用式链接的定义表＋脚注定义表：`[标签]: 目标` 与 `[^标签]: 正文`。
#[derive(Default)]
struct LinkDefs {
    links: std::collections::HashMap<String, String>,
    /// `[^标签]: 正文`，按**定义出现顺序**存；同名只首个生效。
    notes: Vec<(String, String)>,
}

impl LinkDefs {
    fn get(&self, label: &str) -> Option<&str> {
        self.links.get(label).map(String::as_str)
    }
    /// **首个定义生效**（CommonMark 同口径）：同名的后面那些只当重复，不覆盖。
    fn insert(&mut self, label: String, dest: String) {
        self.links.entry(label).or_insert(dest);
    }
    fn note(&mut self, label: String, body: String) {
        if !self.notes.iter().any(|(l, _)| {
            note_scan();
            *l == label
        }) {
            self.notes.push((label, body));
        }
    }
    fn has_note(&self, label: &str) -> bool {
        self.notes.iter().any(|(l, _)| {
            note_scan();
            l == label
        })
    }
}

/// 标签规范化：去首尾空白、内部连续空白并成一格、大小写折叠。
/// 引用式链接的标签匹配**只认这三条**（`[Foo  Bar]` 与 `[foo bar]` 是同一个）。
fn normalize_label(s: &str) -> String {
    let mut out = String::new();
    let mut prev_ws = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            prev_ws = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// `[标签]: 目标 "可选标题"` ⇒ Some((规范化标签, 目标))。只吃**一行写完**的写法：
/// 目标可带 `<…>` 包裹，标题（引号或括号那段）直接忽略——预览没有 tooltip 这份状态。
/// 目标为空 ⇒ 不是定义（那行仍是正文）。
fn link_def(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let close = find_bracket(rest)?;
    let label = &rest[..close];
    if label.trim().is_empty() {
        return None;
    }
    // `[^标签]` 是脚注不是引用式链接：让它进链接表的话，注脚正文会被当成 URL，
    // 正文里那个 `[^1]` 就渲成一个下划线断链、注脚整篇消失
    // 护栏：`[^标签]` 是脚注不是引用式链接。**收集顺序已经保证**脚注定义先被
    // `footnote_def` 收走（变异探针现量：摘掉这一格 39 条 core 用例全绿），这里再挡一次
    // 是因为哪天两次判定调了个位，注脚正文就会变成一条 URL、`[^1]` 变成断链。
    if label.starts_with('^') {
        return None;
    }
    let after = rest[close + 1..].trim_start().strip_prefix(':')?;
    let after = after.trim_start();
    let dest = if let Some(inner) = after.strip_prefix('<') {
        let end = inner.find('>')?;
        &inner[..end]
    } else {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        &after[..end]
    };
    if dest.is_empty() {
        return None;
    }
    Some((normalize_label(label), decode_entities(dest)))
}

/// `[^标签]: 正文` ⇒ Some((规范化标签, 正文))。正文一律**一行写完**（缩进续行的多段注脚
/// 先不做）；正文为空 ⇒ 不是定义（那行仍是正文）。标签与引用同一套规范化（空白并一格＋
/// 大小写折叠），所以 `[^Note]` 与 `[^note]: …` 配得上。
fn footnote_def(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("[^")?;
    let close = find_bracket(rest)?;
    let label = &rest[..close];
    if label.trim().is_empty() {
        return None;
    }
    let body = rest[close + 1..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    if body.is_empty() {
        return None;
    }
    Some((normalize_label(label), body.to_owned()))
}

/// 整篇扫一遍收定义，同时标出**该被吃掉**的行（代码围栏内、缩进 4 格以上的都不算）。
/// 返回的表在正文解析之前就有了 ⇒ `[ref]` 写在定义行前面也能解析（前向引用）。
fn collect_link_defs(lines: &[&str]) -> (LinkDefs, Vec<bool>) {
    let mut defs = LinkDefs::default();
    let mut consumed = vec![false; lines.len()];
    let mut in_fence = false;
    for (idx, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // 4 格以上缩进（或制表符）＝缩进代码块，不是定义行
        let indent = raw.len() - raw.trim_start().len();
        if raw.starts_with('\t') || indent >= 4 {
            continue;
        }
        if let Some((label, body)) = footnote_def(t) {
            defs.note(label, body);
            consumed[idx] = true;
            continue;
        }
        if let Some((label, dest)) = link_def(t) {
            defs.insert(label, dest);
            consumed[idx] = true;
        }
    }
    (defs, consumed)
}

/// 生产入口用 `inline_with`（要带定义表）；这份薄壳只给**已有的一批行内单测**用，
/// 它们测的是标记本身、根本没有引用式链接，空表等价于「那条路不存在」。
#[cfg(test)]
fn inline(text: &str) -> Vec<MdSpan> {
    inline_with(text, &LinkDefs::default())
}

/// 块级行内入口：递归展开标记后合并相邻同样式段（`defs`＝引用式链接的定义表）。
fn inline_with(text: &str, defs: &LinkDefs) -> Vec<MdSpan> {
    let mut spans: Vec<MdSpan> = Vec::new();
    parse_inline(text, 0, None, &mut spans, defs);
    merge_spans(&mut spans);
    spans
}

/// 一层行内扫描：`base` 继承自外层标记（嵌套样式），`link` 是所在链接的目标。
fn parse_inline(text: &str, base: u8, link: Option<&str>, out: &mut Vec<MdSpan>, defs: &LinkDefs) {
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

        // 裸链接（GFM 的 autolink literal）：粘贴进来的 URL 过去只是普通文字，而别的
        // 写法都能点——同一篇里"能不能点"取决于有没有那对尖括号，这说不通。
        // 已经在链接里（`link.is_some()`）不再套一层：链接文字里的 URL 不是嵌套链接。
        if link.is_none() && url_boundary_ok(text[..i].chars().next_back()) {
            if let Some((len, dest)) = bare_url(&text[i..]) {
                flush!();
                out.push(MdSpan {
                    text: text[i..i + len].to_owned(),
                    style: base | MD_LINK,
                    link: Some(dest),
                });
                i += len;
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

        // 链接 / 图片：`[标签](目标)`、`![替代](地址)`，以及引用式
        // `[标签][ref]`／`[标签][]`／`[标签]`（目标查定义表）
        if c == '[' || (c == '!' && text[i + 1..].starts_with('[')) {
            let is_image = c == '!';
            let label_at = i + if is_image { 2 } else { 1 };
            if let Some(close) = find_bracket(&text[label_at..]) {
                let label = &text[label_at..label_at + close];
                // 脚注引用 `[^标签]`：查到定义才接管，查不到就原样留着（与"不给断链"同口径）
                if !is_image {
                    if let Some(raw) = label.strip_prefix('^') {
                        if defs.has_note(&normalize_label(raw)) {
                            flush!();
                            out.push(MdSpan {
                                // 先放标签：编号要按全篇**首次引用**顺序发，收尾时统一换算
                                text: raw.trim().to_owned(),
                                style: base | MD_FOOTNOTE,
                                link: None,
                            });
                            i = label_at + close + 1;
                            continue;
                        }
                    }
                }
                let after = &text[label_at + close + 1..];
                if let Some(dest_end) = after.strip_prefix('(').and_then(find_paren) {
                    // dest_end 是**剥掉 `(` 之后**串内的偏移
                    let raw_dest = &after[1..1 + dest_end];
                    if let Some(dest) = link_dest(raw_dest) {
                        flush!();
                        let style = base | MD_LINK | if is_image { MD_IMAGE } else { 0 };
                        parse_inline(label, style, Some(dest.as_str()), out, defs);
                        i = label_at + close + 2 + dest_end + 1;
                        continue;
                    }
                }
                if !label.trim().is_empty() {
                    if let Some((dest, extra)) = reference_dest(after, label, defs) {
                        flush!();
                        let style = base | MD_LINK | if is_image { MD_IMAGE } else { 0 };
                        parse_inline(label, style, Some(dest.as_str()), out, defs);
                        i = label_at + close + 1 + extra;
                        continue;
                    }
                }
            }
        }

        // 自动链接 `<https://…>`（HTML 标签按字面量，不吞），但 `<br>` 这一个是换行
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
                // 从 Word／Confluence／各种导出工具里最常漏出来的一个标签：
                // 它不是样式，是**换行**，按字面量显示等于把用户的意思吃掉
                let body = inner.trim();
                if !body.is_empty()
                    && body
                        .trim_end_matches(|c: char| c == '/' || c.is_whitespace())
                        .eq_ignore_ascii_case("br")
                {
                    lit.push('\n');
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
                // 贴边（flanking）判据：紧跟标点的那一串不能开栏——中文正文里
                // 「是**“重点”**的部分」这种写法按 GitHub 的渲染就是字面量，
                // 而我们旧的"只要后面不是空格就开栏"会把后面一大段染成粗体
                let prev_open = text[..i].chars().next_back();
                let next_open = text[i + len..].chars().next();
                if !can_open_emphasis(c, prev_open, next_open) {
                    continue;
                }
                // 往后找**第一个能当闭栏的**同种串：改前只取第一个，于是 `**粗 ** 与 后**`
                // 里那串前面跟着空格的 `**` 当不了闭栏，就直接放弃整对——哪怕后面明明还有
                // 一对能配上（CommonMark 的配对是跳过不合格的位置继续找）
                let mut search = i + len;
                while let Some((rel, rl)) = next_emphasis_run(&text[search..], c, len) {
                    let abs_close = search + rel;
                    search = abs_close + rl;
                    if abs_close == i + len {
                        continue; // 空内容不成段
                    }
                    // 候选闭栏必须**自己也能当闭栏**（贴边判据），不合格就跳过去继续往后找
                    let prev_close = text[..abs_close].chars().next_back();
                    let next_close = text[abs_close + rl..].chars().next();
                    if !can_close_emphasis(c, prev_close, next_close) {
                        continue;
                    }
                    flush!();
                    parse_inline(&text[i + len..abs_close], base | style, link, out, defs);
                    i = abs_close + len;
                    opened = true;
                    break;
                }
                if opened {
                    break;
                }
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

/// 强调记号的**贴边**（flanking）判据要用到的"标点"：ASCII 标点 + 中文里真会踩到的
/// 全角／CJK 标点。标准库只给 `is_ascii_punctuation`（没有 Unicode 类别表），这里手写一份
/// 子集——少列一种，那一处的 `**，` 就仍被当开栏。
/// 对拍基准＝GitHub 自己的 `/markdown`（cmark-gfm）：`是**“重点”**的部分`、
/// `foo_bar_baz`、`_词界_斜`、`正文收窄**，不是放开**` 在 GitHub 上**全是字面量**，
/// 而 `定案的**收窄**，不是放开` 里那对 `**` 前后都是汉字 ⇒ GitHub 给出 `<strong>收窄</strong>`。
fn is_emphasis_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '，' | '。'
                | '、'
                | '；'
                | '：'
                | '？'
                | '！'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '〈'
                | '〉'
                | '「'
                | '」'
                | '『'
                | '』'
                | '\u{201C}'
                | '\u{201D}'
                | '\u{2018}'
                | '\u{2019}'
                | '…'
                | '—'
                | '～'
                | '·'
                | '\u{3000}'
        )
}

/// 行首／行尾按 CommonMark 的规定算空白。
fn flank_ws(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => c.is_whitespace(),
    }
}
fn flank_punct(c: Option<char>) -> bool {
    c.is_some_and(is_emphasis_punct)
}

/// 记号串能否**开栏**：`*`／`~` 只要左贴边；`_` 还要求不右贴边（词内 `_` 不开样式，
/// 所以 `foo_bar_baz` 与 `_词界_斜` 保持字面量——这两格都拿 GitHub 的输出对拍过）。
fn can_open_emphasis(c: char, prev: Option<char>, next: Option<char>) -> bool {
    let left = !flank_ws(next) && (!flank_punct(next) || flank_ws(prev) || flank_punct(prev));
    if !left {
        return false;
    }
    if c == '_' {
        let right = !flank_ws(prev) && (!flank_punct(prev) || flank_ws(next) || flank_punct(next));
        !right
    } else {
        true
    }
}

/// 记号串能否**闭栏**（镜像判据；`_` 同样禁词内）。
fn can_close_emphasis(c: char, prev: Option<char>, next: Option<char>) -> bool {
    let right = !flank_ws(prev) && (!flank_punct(prev) || flank_ws(next) || flank_punct(next));
    if !right {
        return false;
    }
    if c == '_' {
        let left = !flank_ws(next) && (!flank_punct(next) || flank_ws(prev) || flank_punct(prev));
        !left
    } else {
        true
    }
}

/// 从串首数连续同字符的长度。
fn run_len(s: &str, c: char) -> usize {
    s.chars().take_while(|&x| x == c).count()
}

/// 强调用的"下一个候选闭栏"：返回（串内字节偏移，这一串的长度）。
/// 与 [`find_run`] 的差别就是要**带出串长**，好让调用方跳过不合格的候选继续往后找。
fn next_emphasis_run(s: &str, c: char, min: usize) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    while offset < s.len() {
        let ch = s[offset..].chars().next()?;
        if ch == c {
            let run = run_len(&s[offset..], c);
            if run >= min {
                return Some((offset, run));
            }
            offset += run * ch.len_utf8();
        } else {
            offset += ch.len_utf8();
        }
    }
    None
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

/// 引用式链接的目标：`[标签][ref]`、`[标签][]`（省略 ref＝标签自己就是引用）、
/// `[标签]`（shortcut）。返回（目标，**标签右括号之后**还要再吃掉多少字节）。
/// 查不到定义 ⇒ `None`——那对括号按字面量留着，**不猜**（写了 `[x]` 但没定义，
/// 用户看得见原样，比给个断链好）。
fn reference_dest(after: &str, label: &str, defs: &LinkDefs) -> Option<(String, usize)> {
    let (cited, extra) = match after.strip_prefix('[') {
        Some(rest) => {
            let close = find_bracket(rest)?;
            let cited = &rest[..close];
            // `[标签][]` 那种空 ref：拿标签去查
            if cited.trim().is_empty() {
                (label, close + 2)
            } else {
                (cited, close + 2)
            }
        }
        None => (label, 0usize),
    };
    Some((defs.get(&normalize_label(cited))?.to_owned(), extra))
}

/// 裸 URL 的左边界：前面是字母数字或 URL 里常见的连接符 ⇒ 那是更大的一个词，不是链接起点
/// （`mywww.example.com` 不许切出 `www.example.com`）。中文这类非 ASCII 字符算边界——
/// 「见https://x.com/a」里那个链接该认出来，中文用户粘贴时常常不带空格。
fn url_boundary_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            !(c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '.' | '_' | '-' | '/' | '+' | ':' | ';' | '?' | '&' | '%' | '#' | '@'
                ))
        }
    }
}

/// 裸 URL 的字面量（GFM autolink literal 的子集）：`http(s)://…` 与 `www.….xx…`。
/// 返回（显示文本的字节长，链接目标）。三条口径：
///
/// * 本体只走 **ASCII 可打印**字符——「见 https://x.com/a，谢谢」那个 `，` 必须收尾，
///   中文正文里粘 URL 的人不会替你把标点转义；
/// * 行尾句读（`.` `,` `;` `:` `!` `?` `'` `"`）不算进链接；右括号按**配对**算，
///   所以「(https://x.com/a)」里最后一个 `)` 是外面的括号，而 `…/a_(b)` 的那个留着；
/// * `www.` 没有 scheme，交出去的目标补 `https://`（点开的必须是能打开的地址），
///   但**显示文本保持原样**。
fn bare_url(s: &str) -> Option<(usize, String)> {
    let has_scheme = s.starts_with("http://") || s.starts_with("https://");
    if !has_scheme && !s.starts_with("www.") {
        return None;
    }
    let mut end = 0usize;
    let mut parens = 0usize;
    while end < s.len() {
        let c = s[end..].chars().next()?;
        if !c.is_ascii()
            || c.is_whitespace()
            || matches!(c, '<' | '>' | '"' | '\'' | '`' | '[' | ']' | '{' | '}')
        {
            break;
        }
        match c {
            '(' => parens += 1,
            // 没有配对的左括号 ⇒ 这根右括号不属于链接本体
            ')' if parens == 0 => break,
            ')' => parens -= 1,
            _ => {}
        }
        end += c.len_utf8();
    }
    // 本体到这里全是 ASCII（非 ASCII 已 break），按字节回退安全
    while end > 0
        && matches!(
            s.as_bytes()[end - 1],
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'"'
        )
    {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let literal = &s[..end];
    let host = if has_scheme {
        literal.split_once("://")?.1
    } else {
        &literal[4..]
    };
    if host.is_empty() {
        return None;
    }
    if !has_scheme && !host.split('/').next().unwrap_or("").contains('.') {
        return None; // `www.x` 不是域名
    }
    let dest = if has_scheme {
        literal.to_owned()
    } else {
        format!("https://{literal}")
    };
    // 目标里的实体要解开（与 `[文字](url)` 同一口径）：`?a=1&amp;b=2` 写成 URL 时是真 `&`
    Some((end, decode_entities(&dest)))
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

    /// 引用块的读数＝（深度，`子块型:正文` 逐条）——测试既盯形状也盯正文。
    fn quote_reading(block: &MdBlock) -> (usize, Vec<(&'static str, String)>) {
        let MdBlock::Quote { depth, blocks } = block else {
            panic!("期望引用块：{block:?}");
        };
        (
            *depth,
            blocks
                .iter()
                .map(|b| match b {
                    MdBlock::Paragraph { spans } => ("paragraph", spans_text(spans)),
                    MdBlock::Heading { spans, .. } => ("heading", spans_text(spans)),
                    MdBlock::ListItem { spans, .. } => ("item", spans_text(spans)),
                    MdBlock::Quote { .. } => ("quote", String::new()),
                    MdBlock::CodeBlock { .. } => ("code", String::new()),
                    MdBlock::Table { .. } => ("table", String::new()),
                    MdBlock::Rule => ("rule", String::new()),
                    MdBlock::Footnotes { .. } => ("notes", String::new()),
                })
                .collect(),
        )
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
        assert_eq!(
            quote_reading(&blocks[3]),
            (1, vec![("paragraph", "引用一句".to_owned())]),
            "{blocks:?}"
        );
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

        // 有序任务框（GFM 两种列表都有）：改前 `[x]` 留在正文里原样画出来
        let ordered = parse_markdown("1. [x] 做完\n2. [ ] 没做\n");
        assert_eq!(ordered.len(), 2, "{ordered:?}");
        for (want, idx) in [(Some(true), 0usize), (Some(false), 1usize)] {
            match &ordered[idx] {
                MdBlock::ListItem {
                    order, task, spans, ..
                } => {
                    assert_eq!(*task, want, "第 {idx} 项的勾选框状态：{ordered:?}");
                    assert!(order.is_some(), "序号仍在");
                    assert_eq!(
                        spans.iter().map(|s| s.text.as_str()).collect::<String>(),
                        if idx == 0 { "做完" } else { "没做" },
                        "框算标记，不留在正文里"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// 列表项的**续行**留在项里：缩进接着写的、不缩进接着写的（CommonMark 的 lazy
    /// continuation）、空一行再接的第二段，统统并回同一个项目符号；而真正另起一块的
    /// 开头（下一个项／标题／引用／围栏／分隔线／定义行）仍把项收住。
    /// 改前这些续行全掉回段落分支＝符号后面接的话跑出列表，看着像排版坏了。
    #[test]
    fn list_item_continuation_lines_stay_in_the_item() {
        let items = |src: &str| -> Vec<(usize, String)> {
            parse_markdown(src)
                .iter()
                .filter_map(|b| match b {
                    MdBlock::ListItem { indent, spans, .. } => {
                        Some((*indent, spans.iter().map(|s| s.text.as_str()).collect()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            items("- 第一项说明\n  接着写的续行\n- 第二项\n"),
            vec![
                (0, "第一项说明 接着写的续行".to_owned()),
                (0, "第二项".to_owned())
            ],
            "缩进续行并回上一项"
        );
        assert_eq!(
            items("- 第一项\n不缩进也接着写\n"),
            vec![(0, "第一项 不缩进也接着写".to_owned())],
            "lazy continuation：不缩进也归本项"
        );
        assert_eq!(
            items("- 甲  \n  乙\n"),
            vec![(0, "甲\n乙".to_owned())],
            "行尾两个空格＝项内的硬换行，不是两段"
        );
        assert_eq!(
            items("- 第一段\n\n  第二段\n\n下一段\n"),
            vec![(0, "第一段\n第二段".to_owned())],
            "空行后缩进的第二段仍在本项内；顶格那行才是新的一块"
        );
        assert_eq!(
            items("1. 甲\n   续行\n2. 乙\n"),
            vec![(0, "甲 续行".to_owned()), (0, "乙".to_owned())],
            "有序项的续行判据按标记宽度（`1. `＝3 列）"
        );
        assert_eq!(
            items("- 甲\n  - 乙\n"),
            vec![(0, "甲".to_owned()), (1, "乙".to_owned())],
            "嵌套子项是另一项，不是续行"
        );
        assert_eq!(
            items("- [ ] 未完成\n  再补一句\n"),
            vec![(0, "未完成 再补一句".to_owned())],
            "任务项的续行不吃掉勾选框"
        );

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
                    MdBlock::Footnotes { .. } => "notes",
                })
                .collect()
        };
        assert_eq!(
            kinds("- 甲\n  # 标题\n"),
            vec!["item", "heading"],
            "缩进够到正文列的 `#` 仍是标题，不当续行"
        );
        assert_eq!(
            kinds("- 甲\n  > 引用\n"),
            vec!["item", "quote"],
            "引用行不并进列表项"
        );
        assert_eq!(
            kinds("- 甲\n  ```\n  代码\n  ```\n"),
            vec!["item", "code"],
            "项内的围栏自成一块"
        );
        assert_eq!(
            kinds("- 甲\n\n- 乙\n"),
            vec!["item", "item"],
            "松散列表：空行之后的同级项仍是独立项"
        );
    }

    /// 连续引用行并成一块；`>>` 记深度。
    #[test]
    fn quotes_merge_consecutive_lines_and_track_depth() {
        let blocks = parse_markdown("> 第一句\n> 第二句\n>> 更深的一句");
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(
            quote_reading(&blocks[0]),
            (1, vec![("paragraph", "第一句 第二句".to_owned())]),
            "同深度连续行仍并成一段"
        );
        assert_eq!(
            quote_reading(&blocks[1]),
            (2, vec![("paragraph", "更深的一句".to_owned())]),
            "`>>` 的深度记在一格上，内部不再套第二层"
        );
    }

    /// 引用块内部**按块级解析**：`> - 甲` 里是一个真的列表项，`> ## 小节` 里是一个标题。
    /// 改前整块只并成一行正文，`- ` 与 `##` 原样画在预览里＝引用里的列表和标题全走样。
    #[test]
    fn quote_bodies_parse_as_blocks() {
        assert_eq!(
            quote_reading(&parse_markdown("> - 甲\n> - 乙\n")[0]),
            (
                1,
                vec![("item", "甲".to_owned()), ("item", "乙".to_owned())]
            ),
            "引用里的两行列表＝两个 ListItem"
        );
        assert_eq!(
            quote_reading(&parse_markdown("> ## 小节\n> 正文\n")[0]),
            (
                1,
                vec![
                    ("heading", "小节".to_owned()),
                    ("paragraph", "正文".to_owned())
                ]
            ),
            "标题升成标题，正文另起一段"
        );
        // 任务框／有序号在引用里也认
        assert_eq!(
            quote_reading(&parse_markdown("> 1. 头一项\n")[0]).1[0].0,
            "item",
            "引用里的有序项"
        );
        // 引用里的围栏是代码，不是带 ``` 的正文
        assert_eq!(
            quote_reading(&parse_markdown("> ```\n> 代码\n> ```\n")[0]).1,
            vec![("code", String::new())],
            "引用里的围栏自成代码块"
        );
        // 行内标记照旧在内部块里解
        let bold = parse_markdown("> 这是 **粗** 的一句\n");
        assert_eq!(
            quote_reading(&bold[0]),
            (1, vec![("paragraph", "这是 粗 的一句".to_owned())]),
            "{bold:?}"
        );
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

    /// 缩进代码块（4 格／制表符）：空行之后的缩进行是代码。改前它被当正文走行内解析
    /// ⇒ 里面的 `*x*` 被染色、行首缩进全丢，README 里那张命令输出看着像排版坏了。
    #[test]
    fn indented_blocks_after_a_blank_line_become_code() {
        let blocks = parse_markdown("段落一句\n\n    这是缩进代码\n      多缩两格留着\n\n下一段\n");
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        let MdBlock::CodeBlock { lang, lines } = &blocks[1] else {
            panic!("中间那块该是缩进代码：{blocks:?}");
        };
        assert_eq!(lang, &None, "缩进代码没有语言标注");
        assert_eq!(
            lines,
            &vec!["这是缩进代码".to_owned(), "  多缩两格留着".to_owned()],
            "只吃掉 4 列，相对缩进留着"
        );
        assert!(matches!(&blocks[2], MdBlock::Paragraph { .. }));

        let tabbed = parse_markdown("正文\n\n\t制表符起头\n\t第二行\n");
        let MdBlock::CodeBlock { lines, .. } = &tabbed[1] else {
            panic!("{tabbed:?}");
        };
        assert_eq!(
            lines,
            &vec!["制表符起头".to_owned(), "第二行".to_owned()],
            "一个制表符＝4 列"
        );

        // 反向的那几格：上一行不是空行 ⇒ 不是缩进代码
        assert!(
            matches!(
                &parse_markdown("正文\n    接着写")[0],
                MdBlock::Paragraph { .. }
            ),
            "紧跟正文的缩进行是懒续行"
        );
        let nested = parse_markdown("- 甲\n    - 子项\n");
        assert!(
            nested.iter().all(|b| matches!(b, MdBlock::ListItem { .. })),
            "紧跟列表项的缩进行还是子项：{nested:?}"
        );

        // 文件头第一件事就是缩进代码（上面没有行可依赖）
        let head = parse_markdown("    开头就是代码\n");
        let MdBlock::CodeBlock { lines, .. } = &head[0] else {
            panic!("{head:?}");
        };
        assert_eq!(lines, &vec!["开头就是代码".to_owned()]);
    }

    /// 闭栏长度要 ≥ 开栏长度：` ```` ` 里放 ` ``` ` 是教 Markdown 的文档最常见的写法，
    /// 改前内层那三根就收了栏 ⇒ 一块裂成两块，剩下的一半当正文染色。
    #[test]
    fn longer_fence_keeps_shorter_fences_inside() {
        let blocks = parse_markdown("````markdown\n```rust\nfn a() {}\n```\n````\n");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let MdBlock::CodeBlock { lang, lines } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(lang.as_deref(), Some("markdown"));
        assert_eq!(
            lines,
            &vec![
                "```rust".to_owned(),
                "fn a() {}".to_owned(),
                "```".to_owned()
            ],
            "内层的 ``` 是内容"
        );
        // 同长的三反引号照常收栏
        let plain = parse_markdown("```\na\n```\n");
        assert_eq!(plain.len(), 1, "{plain:?}");
        // 带 info 的那行不是闭栏
        let with_info = parse_markdown("```\na\n```b\n```\n");
        let MdBlock::CodeBlock { lines, .. } = &with_info[0] else {
            panic!("{with_info:?}");
        };
        assert_eq!(
            lines,
            &vec!["a".to_owned(), "```b".to_owned()],
            "```b 有正文，是代码内容不是闭栏"
        );
    }

    /// HTML 注释隐去：整行 `<!-- … -->`（含跨行的那种）在预览里不占位置。改前它照旧
    /// 是一段正文——README 顶部那一排徽章／锚点注释全裸露着。
    #[test]
    fn html_comments_take_no_room() {
        assert_eq!(
            parse_markdown("正文\n\n<!-- 说明 -->\n\n更多\n").len(),
            2,
            "单行注释不留块"
        );
        assert_eq!(
            parse_markdown("甲\n\n<!--\n隐藏的行\n-->\n\n乙\n").len(),
            2,
            "跨行注释整段吞掉"
        );
        // 夹在两行正文中间：把那段切断，自己不出现
        let mid = parse_markdown("上\n<!-- 注 -->\n下\n");
        assert_eq!(mid.len(), 2, "{mid:?}");

        // 行**中间**的注释不动：那是正文的一部分，删掉等于吃字
        let inline = parse_markdown("前 <!-- 中 --> 后\n");
        let MdBlock::Paragraph { spans } = &inline[0] else {
            panic!("{inline:?}");
        };
        assert_eq!(spans_text(spans), "前 <!-- 中 --> 后");

        // 等不到收尾 ⇒ 交回普通解析（原样看得见，比静默吞掉后半篇好查）
        let open = parse_markdown("<!-- 没关\n");
        let MdBlock::Paragraph { spans } = &open[0] else {
            panic!("{open:?}");
        };
        assert_eq!(spans_text(spans), "<!-- 没关");
    }

    /// 引用块的**懒续行**：紧跟 `>` 行的普通正文行仍属于这个引用——GitHub `/markdown` 对
    /// `> 一段引用\n继续挂在引用里` 现量输出的是同一个 `<blockquote>` 里的一个 `<p>`。
    /// 改前第二行掉到引用外面另起一段＝写引用时忘了 `>` 就"只剩一行"。
    #[test]
    fn lazy_lines_after_a_quote_stay_inside_it() {
        assert_eq!(
            quote_reading(&parse_markdown("> 一段引用\n继续挂在引用里\n")[0]),
            (1, vec![("paragraph", "一段引用 继续挂在引用里".to_owned())]),
            "第二行不写 `>` 也归这个引用"
        );
        // 空行就断（那是两块，不是一段的懒续行）
        let two = parse_markdown("> 甲\n\n乙\n");
        assert_eq!(two.len(), 2, "{two:?}");
        assert!(matches!(&two[1], MdBlock::Paragraph { .. }));
        // 另起一块的行不吞进来
        assert_eq!(parse_markdown("> 甲\n# 标题\n").len(), 2, "标题不吞进引用");
        assert_eq!(
            parse_markdown("> 甲\n- 项\n").len(),
            2,
            "列表不吞进引用：{:#?}",
            parse_markdown("> 甲\n- 项\n")
        );
        // 引用里的列表项同样吃懒续行（走的是内部那一层列表判据）
        assert_eq!(
            quote_reading(&parse_markdown("> - 甲\n接着写\n")[0]).1,
            vec![("item", "甲 接着写".to_owned())],
            "引用里的项也能接懒续行"
        );
    }

    /// 有序项**重号**（对拍基准＝GitHub `/markdown`：`5. 五 / 7. 七 / 空行 / 9. 九 / 2. 二`
    /// 出一个 `<ol start="5">`＝渲染成 5,6,7,8）。改前原样回声，预览里会冒「1. 1. 9.」。
    #[test]
    fn ordered_items_renumber_like_github() {
        let nums = |src: &str| -> Vec<Option<usize>> {
            parse_markdown(src)
                .iter()
                .filter_map(|b| match b {
                    MdBlock::ListItem { order, .. } => Some(*order),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            nums("1. 一\n1. 一\n9. 九\n"),
            vec![Some(1), Some(2), Some(3)],
            "除了第一项，后面写几都不采信"
        );
        assert_eq!(
            nums("5. 五\n7. 七\n\n9. 九\n"),
            vec![Some(5), Some(6), Some(7)],
            "空行不另起一个列表（GitHub 的 <ol start=\"5\"> 同此）"
        );
        assert_eq!(
            nums("3. 三\n4. 四\n"),
            vec![Some(3), Some(4)],
            "本来就对得上时不变"
        );
        assert_eq!(
            nums("1. 外\n   1. 内\n   1. 内二\n"),
            vec![Some(1), Some(1), Some(2)],
            "嵌套层各起各的号"
        );
        assert_eq!(
            nums("1. 外\n   1. 内\n2. 外二\n"),
            vec![Some(1), Some(1), Some(2)],
            "回到上一层接着那层已发到的号走"
        );
        assert_eq!(
            nums("1. 一\n- 无序插一脚\n2. 二\n"),
            vec![Some(1), None, Some(2)],
            "无序项把那层的连续号断掉"
        );
        // 引用块内部是另一份号（自己从 1 起）
        let inside = parse_markdown("> 1. 一\n> 1. 二\n");
        let MdBlock::Quote { blocks: inner, .. } = &inside[0] else {
            panic!("{inside:?}");
        };
        let got: Vec<Option<usize>> = inner
            .iter()
            .filter_map(|b| match b {
                MdBlock::ListItem { order, .. } => Some(*order),
                _ => None,
            })
            .collect();
        assert_eq!(got, vec![Some(1), Some(2)], "{inside:?}");
    }

    /// 表格单元里的 `\|` 是一个竖线字符，不是分列记号。改前它照样分列，多出来那一列被
    /// `normalize_row` 按列数裁掉＝用户写的字**静默消失**。
    /// ⚠️ 这条只有 core 作证：预览的块节点数／高度都数不出"某一格少了半句字"，
    /// 已登记成眼验项（带 `\|` 的真实表格开一次真窗口）。
    #[test]
    fn escaped_pipes_stay_inside_their_cell() {
        let blocks = parse_markdown("| a | b |\n|---|---|\n| x \\| y | 2 |\n");
        let MdBlock::Table { header, rows, .. } = &blocks[0] else {
            panic!("期望一张表：{blocks:?}");
        };
        assert_eq!(header.len(), 2);
        assert_eq!(rows[0].len(), 2, "竖线不该多分出一列把正文裁掉：{rows:?}");
        assert_eq!(spans_text(&rows[0][0]), "x | y", "转义的竖线留在单元格里");
        assert_eq!(spans_text(&rows[0][1]), "2");

        // 对照：`\\|`＝转义的反斜杠＋真分列（偶数串反斜杠不挡竖线）
        let two = parse_markdown("| a | b |\n|---|---|\n| x \\\\| y |\n");
        let MdBlock::Table { rows, .. } = &two[0] else {
            panic!("{two:?}");
        };
        assert_eq!(rows[0].len(), 2, "{two:?}");
        assert_eq!(spans_text(&rows[0][0]), "x \\");
        assert_eq!(spans_text(&rows[0][1]), "y");
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
        // 未闭合的 `[文字](` 按字面量留着，而里面那串裸 URL **照旧**认成链接。
        // 这条判据改过：原本断的是整串原样，那是「还没有裸 URL 识别」时的产物；
        // GFM 的 autolink literal 在正文里扫 URL，不因前面有个失败的 `[` 就放过。
        assert_eq!(
            inline("[没闭括号](https://e.io"),
            vec![
                MdSpan::plain("[没闭括号]("),
                MdSpan {
                    text: "https://e.io".into(),
                    style: MD_LINK,
                    link: Some("https://e.io".into()),
                },
            ],
            "方括号那截仍是字面量，URL 那截是链接"
        );
    }

    /// 裸 URL 也是链接（GFM 的 autolink literal）。改前粘贴进来的 `https://…` 只是普通文字，
    /// 同一段里 `[文字](url)` 与 `<url>` 都能点——能不能点取决于写法，这说不通。
    #[test]
    fn bare_urls_become_clickable_links() {
        let links = |src: &str| -> Vec<(String, String)> {
            inline_with(src, &LinkDefs::default())
                .iter()
                .filter(|s| s.is_link())
                .map(|s| (s.text.clone(), s.link.clone().unwrap_or_default()))
                .collect()
        };
        assert_eq!(
            links("看 https://example.com/a?b=1 吧"),
            vec![(
                "https://example.com/a?b=1".to_owned(),
                "https://example.com/a?b=1".to_owned()
            )]
        );
        assert_eq!(
            links("访问 www.example.com/x"),
            vec![(
                "www.example.com/x".to_owned(),
                "https://www.example.com/x".to_owned()
            )],
            "显示文本原样，交出去的目标补 https://"
        );
        assert_eq!(
            links("见https://x.com/a，谢谢"),
            vec![("https://x.com/a".to_owned(), "https://x.com/a".to_owned())],
            "中文标点直接收尾（非 ASCII 不进 URL 本体）"
        );
        assert_eq!(
            links("见 https://x.com/a. 下一句"),
            vec![("https://x.com/a".to_owned(), "https://x.com/a".to_owned())],
            "行尾的英文句号不算进链接"
        );
        assert_eq!(
            links("(https://x.com/a_(b))"),
            vec![(
                "https://x.com/a_(b)".to_owned(),
                "https://x.com/a_(b)".to_owned()
            ),],
            "右括号按配对算"
        );
        assert_eq!(
            links("打开 https://x.com/?q=1&amp;r=2 看"),
            vec![(
                "https://x.com/?q=1&amp;r=2".to_owned(),
                "https://x.com/?q=1&r=2".to_owned()
            )],
            "显示留原样、目标解实体（与 `[文字](url)` 同口径）"
        );
        // URL 里的 `_` 不许被当成斜体记号
        let under = inline_with("看 https://x.com/a_b_c 吧", &LinkDefs::default());
        assert!(under.iter().all(|s| s.style & MD_ITALIC == 0), "{under:?}");
        assert_eq!(spans_text(&under), "看 https://x.com/a_b_c 吧");

        // 反向的那些
        assert!(
            links("mywww.example.com").is_empty(),
            "前面是字母数字 ⇒ 不是链接起点"
        );
        assert!(links("`https://x.com/a`").is_empty(), "代码段里是内容");
        assert_eq!(
            links("[看 https://x.com](https://y.com)"),
            vec![("看 https://x.com".to_owned(), "https://y.com".to_owned())],
            "链接文字里的裸 URL 不许再套一层链接"
        );
        assert!(links("https://").is_empty(), "空 host 不是链接");
        assert!(links("www.x").is_empty(), "`www.` 后面没有点号不算域名");
    }

    /// 闭栏位置不合格就**跳过它继续往后找**，而不是放弃整对。GitHub `/markdown` 对同一串
    /// `**粗 ** 与 后**` 现量出 `<strong>粗 ** 与 后</strong>`＝这个行为是参照渲染器的行为；
    /// 改前只取第一个同种串，一看不合格就放弃整对，还会把后半截配成一个**假斜体段**。
    #[test]
    fn emphasis_skips_unusable_closers_and_keeps_looking() {
        assert_eq!(
            inline("**粗 ** 与 后**"),
            vec![styled("粗 ** 与 后", MD_BOLD)],
            "中间那串闭不了栏的 `**` 留在正文里，配对配到最后那一对"
        );
        // 反向对照：真的没有可用闭栏 ⇒ 照旧原样（防"继续找"变成乱配）
        assert_eq!(
            inline("**只有开栏没有闭栏"),
            vec![MdSpan::plain("**只有开栏没有闭栏")]
        );
    }

    /// 贴边（flanking）判据。**期望值不是我推的，是 GitHub `/markdown`（cmark-gfm）对同一串的输出**
    /// （2026-10-13 现量：六串一次送审，另拿本仓 40 行真实日志做整语料对拍＝强调数 31 比 31、
    /// 字面量残留 1 比 1、逐条文本相同）：
    ///
    /// * `是**“重点”**的部分` ⇒ GitHub 出**字面量**（前面是汉字、后面是全角引号 ⇒ 开不了栏）；
    /// * `定案的**收窄**，不是放开` ⇒ GitHub 出 `<strong>收窄</strong>`；
    /// * `foo_bar_baz`、`_词界_斜` ⇒ GitHub 出字面量（词内 `_` 不开样式）；
    /// * `正文收窄**，不是放开**` ⇒ 字面量。
    #[test]
    fn emphasis_flanking_matches_the_github_reference() {
        assert_eq!(
            inline("是**“重点”**的部分"),
            vec![MdSpan::plain("是**“重点”**的部分")],
            "GitHub 也不染这处粗体：改前我们把它一路染到行尾"
        );
        assert_eq!(
            inline("定案的**收窄**，不是放开"),
            vec![
                MdSpan::plain("定案的"),
                styled("收窄", MD_BOLD),
                MdSpan::plain("，不是放开"),
            ],
            "闭栏后面紧跟全角逗号照样成立（GitHub 出 <strong>收窄</strong>）"
        );
        assert_eq!(inline("foo_bar_baz"), vec![MdSpan::plain("foo_bar_baz")]);
        assert_eq!(
            inline("no_connect_this"),
            vec![MdSpan::plain("no_connect_this")]
        );
        assert_eq!(
            inline("_词界_斜"),
            vec![MdSpan::plain("_词界_斜")],
            "词内 `_` 禁，GitHub 同"
        );
        assert_eq!(
            inline("_词界_ 斜"),
            vec![styled("词界", MD_ITALIC), MdSpan::plain(" 斜")],
            "对照：闭栏后面是空格就成斜体"
        );
        assert_eq!(
            inline("正文收窄**，不是放开**"),
            vec![MdSpan::plain("正文收窄**，不是放开**")]
        );
        // 行首/行尾按空白算，串首尾的一对照常成立
        assert_eq!(inline("**粗**"), vec![styled("粗", MD_BOLD)]);
        assert_eq!(
            inline("*斜*体"),
            vec![styled("斜", MD_ITALIC), MdSpan::plain("体")]
        );
    }

    /// HTML 标签**除了 `<br>`** 都按字面量（本解析器不渲 HTML）。`<br>` 单独立出来
    /// 是因为它是「换行」而不是一种样式，见 `br_tags_become_line_breaks`。
    #[test]
    fn html_tags_stay_literal() {
        assert_eq!(
            inline("<div class=\"x\">与</span>"),
            vec![MdSpan::plain("<div class=\"x\">与</span>")]
        );
    }

    /// 引用式链接：目标写在 `[标签]: 目标` 那行里，正文里用 `[文字][标签]`／`[标签][]`／
    /// `[标签]` 三种写法查它。改前这三样全部原样显示，定义行还多占一段正文。
    #[test]
    fn reference_links_resolve_through_the_definition_table() {
        fn links_of(src: &str) -> Vec<(String, String)> {
            parse_markdown(src)
                .iter()
                .flat_map(|b| match b {
                    MdBlock::Paragraph { spans } | MdBlock::Heading { spans, .. } => spans.clone(),
                    _ => Vec::new(),
                })
                .filter(|s| s.is_link())
                .map(|s| (s.text.clone(), s.link.unwrap_or_default()))
                .collect()
        }
        let u = |s: &str| s.to_owned();
        // 三种写法：完整 [文字][ref]、省略 [ref][]、shortcut [ref]
        assert_eq!(
            links_of("见 [文档][ref]。"),
            Vec::<(String, String)>::new(),
            "没定义就**不是**链接：宁可显示原样，不给断链"
        );
        assert_eq!(
            links_of("见 [文档][ref]。\n\n[ref]: https://e.io/a"),
            vec![(u("文档"), u("https://e.io/a"))]
        );
        assert_eq!(
            links_of("[ref][]\n\n[ref]: https://e.io/b"),
            vec![(u("ref"), u("https://e.io/b"))],
            "`[ref][]`＝标签自己就是引用"
        );
        assert_eq!(
            links_of("[ref]\n\n[ref]: https://e.io/c"),
            vec![(u("ref"), u("https://e.io/c"))],
            "shortcut 写法"
        );
        assert_eq!(
            links_of("[Foo   Bar]\n\n[foo bar]: https://e.io/d"),
            vec![(u("Foo   Bar"), u("https://e.io/d"))],
            "标签匹配＝去首尾＋内部空白并一格＋大小写折叠，显示文本原样留着"
        );
        assert_eq!(
            links_of("[x][r]\n\n[r]: <https://e.io/e> \"标题\""),
            vec![(u("x"), u("https://e.io/e"))],
            "`<…>` 包裹的目标与尾随标题都吃掉"
        );
        assert_eq!(
            links_of("[x]: https://e.io/1\n[x]: https://e.io/2\n\n[x]"),
            vec![(u("x"), u("https://e.io/1"))],
            "同名定义**首个生效**"
        );
        assert_eq!(
            links_of("[a] [b]\n\n[a]: https://e.io/1\n[b]: https://e.io/2"),
            vec![(u("a"), u("https://e.io/1")), (u("b"), u("https://e.io/2"))],
            "中间有空格＝两个 shortcut，不是一个 `[a][b]`"
        );
        assert_eq!(
            links_of("![图][img]\n\n[img]: ./a.png"),
            vec![(u("图"), u("./a.png"))],
            "引用式的图片走同一张表"
        );
    }

    /// 定义行本身：**不产生块**。它是查表用的，不是画在预览里的。
    #[test]
    fn definition_lines_are_not_rendered() {
        assert_eq!(
            parse_markdown("见 [文档][ref]。\n\n[ref]: https://e.io/a").len(),
            1,
            "定义行不该留下第二个块"
        );
        // 定义行夹在正文里：把上面那段切断，自己不出现
        let mid = parse_markdown("甲\n[ref]: https://e.io\n\n[ref]");
        assert_eq!(mid.len(), 2, "段落被定义行切断后各自成块：{mid:?}");
        assert!(
            matches!(&mid[0], MdBlock::Paragraph { spans } if spans_text(spans) == "甲"),
            "{mid:?}"
        );
        // 代码围栏里的 `[x]: url` 是代码内容，不进表
        let in_code = parse_markdown("```\n[x]: https://e.io\n```\n\n[x]");
        assert!(
            matches!(&in_code[0], MdBlock::CodeBlock { lines, .. } if lines == &vec!["[x]: https://e.io".to_owned()]),
            "{in_code:?}"
        );
        assert!(
            links_of_none(&in_code),
            "围栏里那行没被收集 ⇒ 下面的 `[x]` 仍是字面量：{in_code:?}"
        );
        // 缩进 4 格＝缩进代码，不是定义
        assert!(
            links_of_none(&parse_markdown("    [x]: https://e.io\n\n[x]")),
            "缩进行不当定义行收：{:#?}",
            parse_markdown("    [x]: https://e.io\n\n[x]")
        );
        // 空目标不算定义 ⇒ 那行照旧是正文
        assert_eq!(
            parse_markdown("[x]: \n\n[x]").len(),
            2,
            "目标为空 ⇒ 那行不是定义，仍当段落"
        );
    }

    /// 一段里没有任何链接 ⇒ true（用来断「这条根本不该进表」）。
    fn links_of_none(blocks: &[MdBlock]) -> bool {
        !blocks.iter().any(|b| match b {
            MdBlock::Paragraph { spans } | MdBlock::Heading { spans, .. } => {
                spans.iter().any(|s| s.is_link())
            }
            _ => false,
        })
    }

    /// 脚注：正文里 `[^标签]` 落一个上标编号，定义行**不进正文**，注脚区挂在最后一块。
    /// 改前那条定义行被**引用式链接的收集器**吃掉（标签＝`^1`），于是 `[^1]` 渲成一条
    /// 下划线断链、注脚正文整篇消失——比"不支持"更糟。
    #[test]
    fn footnotes_become_superscripts_and_a_notes_block() {
        let blocks = parse_markdown("看这句[^1]收尾。\n\n[^1]: 这是注脚正文。\n");
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("第一块该是段落：{blocks:?}");
        };
        assert!(
            spans.iter().any(|s| s.is_footnote()),
            "该有一个脚注标记：{spans:?}"
        );
        assert_eq!(spans_text(spans), "看这句¹收尾。", "编号用上标数字");
        let MdBlock::Footnotes { items } = &blocks[1] else {
            panic!("最后一块该是注脚区：{blocks:?}");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 1);
        assert_eq!(spans_text(&items[0].spans), "这是注脚正文。");

        // 注脚正文自己走行内解析（链接那套照用）
        let linked = parse_markdown("[^1]\n\n[^1]: 见 [文档](https://e.io)\n");
        let MdBlock::Footnotes { items } = &linked[linked.len() - 1] else {
            panic!("{linked:?}");
        };
        assert!(
            items[0]
                .spans
                .iter()
                .any(|s| s.is_link() && s.link.as_deref() == Some("https://e.io")),
            "注脚正文里的链接该照解：{:?}",
            items[0].spans
        );
    }

    /// 编号按**首次引用**的顺序发（不是定义顺序），而从没被引用的定义排在注脚区末尾——
    /// 用户写下的注脚正文不该因为忘了引用就消失。
    #[test]
    fn footnote_numbers_follow_first_reference_and_keep_orphans() {
        let blocks = parse_markdown("乙[^b] 甲[^a]\n\n[^a]: 注 A\n[^b]: 注 B\n[^c]: 没人引用\n");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(spans_text(spans), "乙¹ 甲²", "先引用的拿到 1");
        let MdBlock::Footnotes { items } = &blocks[blocks.len() - 1] else {
            panic!("{blocks:?}");
        };
        let notes: Vec<(usize, String)> = items
            .iter()
            .map(|i| (i.number, spans_text(&i.spans)))
            .collect();
        assert_eq!(
            notes,
            vec![
                (1, "注 B".to_owned()),
                (2, "注 A".to_owned()),
                (3, "没人引用".to_owned())
            ],
            "{blocks:?}"
        );
        assert_eq!(blocks.len(), 2, "三条定义行都不该留下正文块：{blocks:?}");
    }

    /// `link_def` 自己的契约：`[^标签]: 目标` **不进**链接定义表。变异探针现量过＝
    /// 收集顺序已经保证这一点，所以这格护栏平时够不着——它是给"哪天把两次判定调个位"
    /// 那一天买的保险，测试把它钉住，不许静默退化成"注脚正文当 URL、`[^1]` 当断链"。
    #[test]
    fn caret_labels_never_enter_the_link_definition_table() {
        assert!(link_def("[^1]: https://e.io").is_none());
        assert!(
            link_def("[ref]: https://e.io").is_some(),
            "对照：普通标签照收"
        );
        let blocks = parse_markdown("[^1]: https://e.io\n\n[^1]\n");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(spans.iter().any(|s| s.is_footnote()), "{spans:?}");
        assert!(
            !spans.iter().any(|s| s.is_link()),
            "脚注标记不许长成一条链接：{spans:?}"
        );
    }

    /// 反向的那些：查不到定义就原样留着（不给假引用）；围栏里的定义行是代码内容；
    /// 空正文不算定义。这一格与引用式链接「查不到定义⇒不断链」是同一条口径。
    #[test]
    fn unknown_footnote_markers_stay_literal() {
        let blocks = parse_markdown("这里[^x]没定义\n");
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let MdBlock::Paragraph { spans } = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(spans_text(spans), "这里[^x]没定义");
        assert!(!spans.iter().any(|s| s.is_footnote()));

        let coded = parse_markdown("```\n[^1]: 在代码里\n```\n\n[^1]\n");
        assert!(
            matches!(&coded[0], MdBlock::CodeBlock { lines, .. } if lines == &vec!["[^1]: 在代码里".to_owned()]),
            "{coded:?}"
        );
        assert!(
            !coded.iter().any(|b| matches!(b, MdBlock::Footnotes { .. })),
            "围栏里那行没进定义表 ⇒ 不产生注脚区：{coded:?}"
        );

        let empty = parse_markdown("[^1]: \n\n正文\n");
        assert!(
            !empty.iter().any(|b| matches!(b, MdBlock::Footnotes { .. })),
            "正文为空不算定义：{empty:?}"
        );
    }

    /// `<br>` 的几种写法 ⇒ 换行；代码段里仍是字面量。导出工具（Word／Confluence／
    /// 各种富文本转 Markdown）留下的 `<br>` 按字面量显示，等于把用户写下的
    /// 「这里换行」吃掉。
    #[test]
    fn br_tags_become_line_breaks() {
        for src in ["上<br>下", "上<br/>下", "上<br />下", "上<BR>下"] {
            assert_eq!(inline(src), vec![MdSpan::plain("上\n下")], "{src:?}");
        }
        assert_eq!(
            inline("`上<br>下`"),
            vec![code("上<br>下")],
            "代码段里 `<br>` 是内容不是标记"
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
                MdBlock::Footnotes { .. } => "notes",
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
                    MdBlock::Footnotes { .. } => "notes",
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

    /// 下划线式标题（setext）：`标题` 下面压一行 `=` 或 `-` 就是标题，不是
    /// 「一段正文加一条横线」——后者是改前的读法，看上去像排版坏了。
    #[test]
    fn setext_underlines_upgrade_the_paragraph_to_a_heading() {
        assert_eq!(
            parse_markdown("标题\n==="),
            vec![MdBlock::Heading {
                level: 1,
                spans: vec![MdSpan::plain("标题")],
            }],
            "下划线本身不该留在文本里"
        );
        assert_eq!(
            parse_markdown("甲\n乙\n====="),
            vec![MdBlock::Heading {
                level: 1,
                spans: vec![MdSpan::plain("甲 乙")],
            }],
            "整段正文一起升上去"
        );
        let two = parse_markdown("小节\n---\n正文");
        assert!(
            matches!(two[0], MdBlock::Heading { level: 2, .. }),
            "一行 `-` 是二级标题下划线：{two:?}"
        );
        assert_eq!(two.len(), 2, "下划线被吃掉，不该多出块：{two:?}");
    }

    /// 反向的那些：上面没有正文、上面那段被硬换行打断、带空格的 `- - -`——
    /// 都不许升成标题。（CommonMark 明确排除含硬换行的 setext 标题。）
    #[test]
    fn setext_needs_a_clean_paragraph_above_it() {
        let shapes = |src: &str| -> Vec<String> {
            parse_markdown(src)
                .iter()
                .map(|b| match b {
                    MdBlock::Heading { level, .. } => format!("heading{level}"),
                    MdBlock::Paragraph { .. } => "paragraph".to_owned(),
                    MdBlock::CodeBlock { .. } => "code".to_owned(),
                    MdBlock::ListItem { .. } => "item".to_owned(),
                    MdBlock::Quote { .. } => "quote".to_owned(),
                    MdBlock::Table { .. } => "table".to_owned(),
                    MdBlock::Rule => "rule".to_owned(),
                    MdBlock::Footnotes { .. } => "notes".to_owned(),
                })
                .collect()
        };
        assert_eq!(
            shapes("===\n正文"),
            vec!["paragraph"],
            "上面没正文 ⇒ `===` 只是文字"
        );
        assert_eq!(
            shapes("# H\n==="),
            vec!["heading1", "paragraph"],
            "`#` 标题不是段落，下划线跟它无关"
        );
        assert_eq!(
            shapes("甲  \n乙\n---"),
            vec!["paragraph", "rule"],
            "段里有硬换行就不算 setext"
        );
        assert_eq!(
            shapes("- 项\n---"),
            vec!["item", "rule"],
            "列表项后面的 --- 仍是分隔线"
        );
        assert_eq!(
            shapes("正文\n- - -"),
            vec!["paragraph", "rule"],
            "带空格的 --- 不是下划线"
        );
    }

    /// 列表项**回车续写**的判据：接下一项（有序进位、任务框给空框）、空项退出列表、
    /// 非列表行交回普通回车。编辑面（`enter_markdown`）唯一动手的就是这张表。
    #[test]
    fn enter_action_continues_the_list_or_exits_it() {
        let next = |line: &str| match list_enter_action(line) {
            Some(MdListEnter::Next { prefix, .. }) => prefix,
            other => panic!("期望 Next：{other:?}"),
        };
        let exit = |line: &str| match list_enter_action(line) {
            Some(MdListEnter::Exit {
                marker_start,
                marker_end,
            }) => {
                assert_eq!(marker_start, 0, "普通列表的标记就是从行首起");
                marker_end
            }
            other => panic!("期望 Exit：{other:?}"),
        };
        assert_eq!(next("- 甲"), "- ");
        assert_eq!(next("* 甲"), "* ");
        assert_eq!(next("+ 甲"), "+ ");
        assert_eq!(next("  - 甲"), "  - ", "缩进跟着走");
        assert_eq!(next("\t- 甲"), "\t- ");
        assert_eq!(next("3. 甲"), "4. ");
        assert_eq!(next("3) 甲"), "4) ", "`)` 分隔符同样认");
        assert_eq!(next("08. 甲"), "09. ", "零位宽度保持");
        assert_eq!(next("9. 甲"), "10. ");
        assert_eq!(next("- [x] 甲"), "- [ ] ", "已勾的项接出来是**空框**");
        assert_eq!(next("  * [X] 甲"), "  * [ ] ");
        assert_eq!(
            next("1. [x] 甲"),
            "2. [ ] ",
            "有序任务框：这条期望在本文件里改过两次判据——先按「只有无序项有框」写成 `2. `，\n\
             后来解析器（`list_item`）认了 GFM 的有序任务列表，这条连**输入形状**一起订正，\n\
             两层口径不许再各走各的"
        );

        assert_eq!(exit("- "), 2, "空项：删掉行首这 2 个字符");
        assert_eq!(exit("-   "), 4, "标记后的空白一并删");
        assert_eq!(exit("  -  "), 5, "缩进也算标记那截");
        assert_eq!(exit("2. "), 3);
        assert_eq!(exit("- [ ]"), 5, "只剩一个空框也算空项");

        for line in ["正文", "-abc", "1.ab", "---", "*** 粗", "", "  ", "# 标题"] {
            assert!(list_enter_action(line).is_none(), "{line:?} 不该被当列表项");
        }
    }

    /// 引用块里的回车：栏跟着续、栏里的列表一起续；空栏退引用，空列表项只摘列表那截。
    /// 引用判据拿的是解析器那一份（`quote`），所以 `>甲` 这种不带空格的也算引用。
    #[test]
    fn quote_enter_continues_the_bar_and_the_list_inside_it() {
        let next = |line: &str| match quote_list_enter_action(line) {
            Some(MdListEnter::Next { prefix, .. }) => prefix,
            other => panic!("期望 Next：{other:?}"),
        };
        let exit = |line: &str| match quote_list_enter_action(line) {
            Some(MdListEnter::Exit {
                marker_start,
                marker_end,
            }) => (marker_start, marker_end),
            other => panic!("期望 Exit：{other:?}"),
        };

        assert_eq!(next("> 甲"), "> ");
        assert_eq!(next(">> 甲"), ">> ", "两道栏各带一份");
        assert_eq!(
            next("> - 甲"),
            "> - ",
            "引用里的列表项一起接（d77caa1 之后栏内本来就按块级解析）"
        );
        assert_eq!(next("> 3. 甲"), "> 4. ", "有序号照样进位");
        assert_eq!(next("> - [x] 甲"), "> - [ ] ", "任务框接出来是空框");
        assert_eq!(next("  > 甲"), "  > ", "行首缩进带过来");
        assert_eq!(next(">甲"), "> ", "栏后不带空格也算引用，与解析器同一判据");
        assert_eq!(
            next("> # 标题"),
            "> ",
            "续的是「引用」这一层，与栏里装的是什么块无关"
        );
        assert_eq!(
            next(">[x] 甲"),
            "> ",
            "脱离列表标记的任务框不算列表项，只续栏"
        );

        // 空栏 ⇒ 摘掉原样那截，回车退出引用
        assert_eq!(exit(">"), (0, 1));
        assert_eq!(exit("> "), (0, 2));
        assert_eq!(exit(">   "), (0, 4), "栏后多余空白也算标记那截");
        assert_eq!(exit(">> "), (0, 3));
        assert_eq!(exit("  > "), (0, 4), "行首缩进一起摘");
        // 栏里剩一个空列表项 ⇒ 只摘列表那截、栏留着（一层一层退）
        assert_eq!(exit("> - "), (2, 4));
        assert_eq!(exit(">  3. "), (3, 6));

        // 非引用行＝原样交回普通列表判据（应用层只调这一个函数，退路在这钉住）
        assert_eq!(next("- 甲"), "- ");
        assert_eq!(exit("- "), (0, 2));
        assert!(quote_list_enter_action("正文").is_none());
        assert!(quote_list_enter_action("").is_none());
    }

    // ---------- 第 315 轮 C1：包／解行内标记（编辑面判据） ----------

    /// 按 [`MdWrapEdit`] 的偏移取回新选区罩住的那截正文。
    fn selected_body(e: &MdWrapEdit) -> String {
        let (off, len) = e.select.expect("这一格应当保留选区");
        e.text.chars().skip(off).take(len).collect()
    }

    #[test]
    fn wrap_without_selection_inserts_an_empty_pair() {
        for (m, want, caret) in [
            (MdInlineMarker::Bold, "****", 2),
            (MdInlineMarker::Italic, "**", 1),
            (MdInlineMarker::Code, "``", 1),
            (MdInlineMarker::Strike, "~~~~", 2),
            (MdInlineMarker::Link, "[]()", 1),
        ] {
            let e = wrap_action(None, "正文 ", " 结尾", m);
            assert_eq!(e.text, want, "{m:?} 按下去先落一对空记号");
            assert_eq!(e.select, None, "没有正文可选");
            assert_eq!((e.head_cut, e.tail_cut), (0, 0), "没邻接记号可删");
            assert_eq!(e.caret, caret, "{m:?} 光标要落在两记号中间");
        }
    }

    #[test]
    fn wrapping_then_pressing_again_unwraps_it() {
        for (m, open, close) in [
            (MdInlineMarker::Bold, "**", "**"),
            (MdInlineMarker::Italic, "*", "*"),
            (MdInlineMarker::Code, "`", "`"),
            (MdInlineMarker::Strike, "~~", "~~"),
        ] {
            // 第一下：包起来，选区收缩到正文（记号落在选区外）
            let wrapped = wrap_action(Some("重点"), "", "", m);
            assert_eq!(wrapped.text, format!("{open}重点{close}"));
            assert_eq!(selected_body(&wrapped), "重点", "包完选区要还罩住正文");
            // 第二下：记号紧贴选区两侧 ⇒ 判的是邻接上下文，只看选区自身永远解不掉
            let off = wrap_action(Some("重点"), &format!("是{open}"), &format!("{close}的"), m);
            assert_eq!(off.text, "重点", "{m:?} 再按一次要剥回原样");
            assert_eq!(
                (off.head_cut, off.tail_cut),
                (open.chars().count(), close.chars().count()),
                "{m:?} 那两截记号在选区外面，得靠邻接删"
            );
            assert_eq!(selected_body(&off), "重点", "解完仍罩住正文");
        }
    }

    #[test]
    fn a_pair_selected_whole_is_stripped_and_an_empty_one_deleted() {
        let e = wrap_action(Some("**甲**"), "", "", MdInlineMarker::Bold);
        assert_eq!(e.text, "甲");
        assert_eq!((e.head_cut, e.tail_cut), (0, 0), "剥的是选区自身，不动邻接");
        assert_eq!(selected_body(&e), "甲");

        let empty = wrap_action(Some("****"), "", "", MdInlineMarker::Bold);
        assert_eq!(empty.text, "", "选中一对空记号 ⇒ 整对删掉");
        assert_eq!(empty.select, None);

        // 只有一截开栏：先剥头再剥尾，不会把头尾算成同一串字符 ⇒ 照常再加一层
        let half = wrap_action(Some("**甲"), "", "", MdInlineMarker::Bold);
        assert_eq!(half.text, "****甲**");
    }

    #[test]
    fn edge_whitespace_stays_outside_the_markers() {
        let e = wrap_action(Some("  甲  "), "", "", MdInlineMarker::Bold);
        assert_eq!(e.text, "  **甲**  ", "连选带上的空白不该被关进粗体里");
        assert_eq!(selected_body(&e), "甲");
        assert_eq!(e.caret, "  **甲".chars().count());

        // 纯空白的选区：不提空白（一提就只剩空正文），原样包起来反倒看得出发生了什么
        let ws = wrap_action(Some("   "), "", "", MdInlineMarker::Bold);
        assert_eq!(ws.text, "**   **");
        assert_eq!(selected_body(&ws), "   ");
    }

    #[test]
    fn link_wrap_puts_the_caret_where_the_target_goes() {
        let bare = wrap_action(None, "", "", MdInlineMarker::Link);
        assert_eq!(bare.text, "[]()");
        assert_eq!(bare.caret, 1, "先让光标落在括号中间写正文");

        let e = wrap_action(Some("官网"), "", "", MdInlineMarker::Link);
        assert_eq!(e.text, "[官网]()");
        assert_eq!(e.select, None, "正文已就位，不再留着选区");
        assert_eq!(e.caret, "[官网](".chars().count(), "光标进目标槽等着填 URL");

        let off = wrap_action(Some("官网"), "[", "]()", MdInlineMarker::Link);
        assert_eq!(off.text, "官网");
        assert_eq!(
            (off.head_cut, off.tail_cut),
            (1, 3),
            "解包吃掉 `[` 与 `]()`"
        );
    }

    /// 跨行的一段粗体在预览里确实成立 ⇒ 包裹判据**不需要**「多行不生效」的提醒分支。
    /// 对拍基准＝GitHub：同一份文本出 `<strong>甲<br>乙</strong>`。
    #[test]
    fn bold_span_may_cross_lines_in_a_paragraph() {
        assert_eq!(
            parse_markdown("正文 **甲\n乙** 结尾"),
            vec![MdBlock::Paragraph {
                spans: vec![
                    MdSpan::plain("正文 "),
                    styled("甲 乙", MD_BOLD),
                    MdSpan::plain(" 结尾"),
                ]
            }],
            "段落把连续行并成一份文本，粗体跨得过去"
        );
    }

    /// 表格的成本契约：**每一格最多被行内解析一次**。
    ///
    /// 病在判据那一侧——段落续行要用 `is_table_start` 断句，改前它就是
    /// `table(...).is_some()`，于是表格紧贴段落的那一种写法（中间不空行）会把
    /// 整张表解析两遍：一遍为答布尔，一遍为产出。大文档里这一份落在**每次按键**上。
    #[test]
    fn table_cells_are_parsed_exactly_once() {
        let (rows, cols) = (40usize, 4usize);
        fn row(cols: usize, cell: impl Fn(usize) -> String) -> String {
            let mut s = String::new();
            for c in 0..cols {
                s.push_str("| ");
                s.push_str(&cell(c));
                s.push(' ');
            }
            s.push_str("|\n");
            s
        }
        // 段落**紧贴**表格（没有空行）⇒ 段落续行判据会为表格首行问一次 is_table_start
        let mut src = String::from("# 表格\n\n正文第一段最后一行\n");
        src.push_str(&row(cols, |c| format!("列{c}")));
        src.push_str(&row(cols, |_| "-".repeat(8)));
        for r in 0..rows {
            src.push_str(&row(cols, |c| format!("第{r}行第{c}格")));
        }

        MD_TABLE_CELL_PARSES.with(|c| c.set(0));
        let blocks = parse_markdown(&src);
        let parsed = MD_TABLE_CELL_PARSES.with(|c| c.get());

        // 夹具自证：那张表真的成了表（表头＋数据行的每一格都在块里）
        let cells: usize = blocks
            .iter()
            .map(|b| match b {
                MdBlock::Table { header, rows, .. } => {
                    header.len() + rows.iter().map(|r| r.len()).sum::<usize>()
                }
                _ => 0,
            })
            .sum();
        assert_eq!(
            cells,
            (rows + 1) * cols,
            "夹具里的表格没被解析成表格：这条断言就成了空断言"
        );
        assert_eq!(
            cells, parsed,
            "格被行内解析了 {parsed} 次，而表里只有 {cells} 格＝判据那一问把整表又解了一遍"
        );
    }

    /// 围栏正文**只抄一遍**：续行判据那一问不该把整段代码再抄一份然后丢掉。
    ///
    /// `fence()` 未闭栏时会一路吃到文档末尾，改前 `starts_new_block` 就靠它答布尔，
    /// 所以段落紧贴未闭栏的写法（贴日志／贴报错最常见）整篇被抄两遍。
    #[test]
    fn fence_bodies_are_copied_exactly_once() {
        let mut src = String::from("# 未闭栏\n\n正文第一段最后一行\n```text\n");
        // 末行**不带**换行：带了就多一个幻影末行，块里行数就不是 30 了（夹具自证那格
        // 会先把这件事报出来，而不是让下面的等式糊里糊涂成立）
        for i in 0..30 {
            if i > 0 {
                src.push('\n');
            }
            src.push_str(&format!("第{i}行被围栏包着的正文"));
        }

        MD_FENCE_BODY_PUSHES.with(|c| c.set(0));
        let blocks = parse_markdown(&src);
        let copied = MD_FENCE_BODY_PUSHES.with(|c| c.get());

        // 夹具自证：未闭栏确实成了一整块代码（不成块，这条断言就是空断言）
        let lines_in_blocks: usize = blocks
            .iter()
            .map(|b| match b {
                MdBlock::CodeBlock { lines, .. } => lines.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(
            lines_in_blocks, 30,
            "夹具里的未闭栏没吃下 30 行正文：这条断言就成了空断言"
        );
        assert_eq!(
            lines_in_blocks, copied,
            "围栏正文被抄了 {copied} 行，块里只有 {lines_in_blocks} 行＝判据那一问抄了一遍丢掉"
        );
    }
}
