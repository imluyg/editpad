//! 迷你 Markdown 解析（P22 第三批，core 纯逻辑）。
//!
//! 手写解析器（构建环境无法引入 pulldown-cmark），覆盖预览面板 v1
//! 所需的最小集：
//!
//! * 标题 `#`~`######`、水平分隔线 `---`/`***`/`___`
//! * 段落（连续非空行合并）
//! * 无序/有序列表项 `- item` / `* item` / `+ item` / `1. item`
//! * 引用行 `> …`
//! * 围栏代码块 ```…```（内部不做任何行内标记解析）
//! * 行内样式：`**粗体**`、`*斜体*`、`` `代码` ``（开关式，未闭合的
//!   标记作用于剩余文本——与简易编辑器惯例一致）
//!
//! 不支持（明确超出 v1）：表格、图片、链接目标展开、嵌套列表缩进。

/// 行内样式位标志。
pub const MD_BOLD: u8 = 1 << 0;
pub const MD_ITALIC: u8 = 1 << 1;
pub const MD_CODE: u8 = 1 << 2;

/// 一段同样式文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSpan {
    pub text: String,
    /// [`MD_BOLD`] 等按位或
    pub style: u8,
}

impl MdSpan {
    pub fn is_bold(&self) -> bool {
        self.style & MD_BOLD != 0
    }
    pub fn is_italic(&self) -> bool {
        self.style & MD_ITALIC != 0
    }
    pub fn is_code(&self) -> bool {
        self.style & MD_CODE != 0
    }
}

/// 块级元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdBlock {
    Heading { level: usize, spans: Vec<MdSpan> },
    Paragraph { spans: Vec<MdSpan> },
    CodeBlock { lines: Vec<String> },
    ListItem { spans: Vec<MdSpan> },
    Quote { spans: Vec<MdSpan> },
    Rule,
}

/// 解析整篇 Markdown。行尾兼容 `\r\n` / `\n` / 孤立 `\r`。
pub fn parse_markdown(text: &str) -> Vec<MdBlock> {
    let lines = split_lines(text);
    let mut blocks = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // 围栏代码块：开栏后吞到闭栏或文末（未闭合也算代码块）
        if trimmed.starts_with("```") {
            let mut code_lines = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code_lines.push(lines[i].to_owned());
                i += 1;
            }
            if i < lines.len() {
                i += 1; // 吃掉闭栏
            }
            blocks.push(MdBlock::CodeBlock { lines: code_lines });
            continue;
        }

        // 标题
        if let Some((level, content)) = heading(trimmed) {
            blocks.push(MdBlock::Heading {
                level,
                spans: inline(content),
            });
            i += 1;
            continue;
        }

        // 水平分隔线
        if is_rule(trimmed) {
            blocks.push(MdBlock::Rule);
            i += 1;
            continue;
        }

        // 引用行
        if let Some(content) = trimmed.strip_prefix('>') {
            let content = content.strip_prefix(' ').unwrap_or(content);
            blocks.push(MdBlock::Quote {
                spans: inline(content.trim()),
            });
            i += 1;
            continue;
        }

        // 列表项
        if let Some(content) = list_item(trimmed) {
            blocks.push(MdBlock::ListItem {
                spans: inline(content),
            });
            i += 1;
            continue;
        }

        // 段落：连续「普通行」以空格拼接
        let mut paragraph = String::from(trimmed);
        i += 1;
        while i < lines.len() {
            let next = lines[i].trim();
            if next.is_empty()
                || next.starts_with("```")
                || heading(next).is_some()
                || is_rule(next)
                || next.starts_with('>')
                || list_item(next).is_some()
            {
                break;
            }
            paragraph.push(' ');
            paragraph.push_str(next);
            i += 1;
        }
        blocks.push(MdBlock::Paragraph {
            spans: inline(&paragraph),
        });
    }
    blocks
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&text[start..i]);
                start = i + 1;
            }
            b'\r' => {
                lines.push(&text[start..i]);
                // \r\n 整体消费
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    lines.push(&text[start..]);
    lines
}

/// `# 标题` → Some((级别, 内容))；# 后必须有空格或就是整行。
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if rest.is_empty() {
        Some((level, ""))
    } else if rest.starts_with(' ') {
        Some((level, rest[1..].trim()))
    } else {
        None
    }
}

/// 分隔线：全部字符属于 {-, *, _} 且至少 3 个同种标记。
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 3 {
        return false;
    }
    let first = t.chars().next().unwrap();
    matches!(first, '-' | '*' | '_') && t.chars().all(|c| c == first)
}

/// 列表项内容：`- `/`* `/`+ `/`数字. ` 之后的部分。
fn list_item(line: &str) -> Option<&str> {
    for mark in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(mark) {
            return Some(rest);
        }
    }
    // 有序列表：数字 + '.' + 空格
    let dot = line.find(". ")?;
    let num = &line[..dot];
    if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
        return Some(&line[dot + 2..]);
    }
    None
}

/// 行内标记解析：**粗体** / *斜体* / `代码`（开关式）。
fn inline(text: &str) -> Vec<MdSpan> {
    let mut spans: Vec<MdSpan> = Vec::new();
    let mut current = String::new();
    let mut style = 0u8;

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0usize;

    macro_rules! flush {
        () => {
            if !current.is_empty() {
                spans.push(MdSpan {
                    text: std::mem::take(&mut current),
                    style,
                });
            }
        };
    }

    while i < n {
        let c = chars[i];
        if c == '`' {
            flush!();
            style ^= MD_CODE;
            i += 1;
            continue;
        }
        if c == '*' && i + 1 < n && chars[i + 1] == '*' {
            flush!();
            style ^= MD_BOLD;
            i += 2;
            continue;
        }
        if c == '*' {
            flush!();
            style ^= MD_ITALIC;
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    flush!();

    // 合并相邻同样式段
    let mut merged: Vec<MdSpan> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if last.style == span.style => last.text.push_str(&span.text),
            _ => merged.push(span),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(spans: &[MdSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
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
            MdBlock::CodeBlock { lines } => {
                assert_eq!(lines, &vec!["**不是粗体**".to_owned(), "let a = 1;".to_owned()]);
            }
            other => panic!("{other:?}"),
        }
        // 未闭合围栏也整体作为代码块
        let blocks = parse_markdown("```\nonly");
        assert!(matches!(blocks[0], MdBlock::CodeBlock { .. }));
    }

    #[test]
    fn lists_quotes_and_rules() {
        let blocks = parse_markdown("- 甲\n* 乙\n1. 丙\n> 引用一句\n---");
        assert_eq!(blocks.len(), 5);
        assert!(matches!(&blocks[0], MdBlock::ListItem { .. }));
        assert!(matches!(&blocks[1], MdBlock::ListItem { .. }));
        assert!(matches!(&blocks[2], MdBlock::ListItem { .. }));
        match &blocks[3] {
            MdBlock::Quote { spans } => assert_eq!(spans_text(spans), "引用一句"),
            other => panic!("{other:?}"),
        }
        assert_eq!(blocks[4], MdBlock::Rule);
    }

    #[test]
    fn inline_styles_toggle_and_merge() {
        // 开关式语义：标记成对出现包裹内容；未闭合作用于剩余文本
        let spans = inline("**粗**斜*体*普通`code`");
        assert_eq!(
            spans,
            vec![
                MdSpan { text: "粗".into(), style: MD_BOLD },
                MdSpan { text: "斜".into(), style: 0 },
                MdSpan { text: "体".into(), style: MD_ITALIC },
                MdSpan { text: "普通".into(), style: 0 },
                MdSpan { text: "code".into(), style: MD_CODE },
            ]
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
}
