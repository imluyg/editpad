use super::*;
// `column!` 宏在 glob 链与宏前奏之间有歧义（E0659 实测），需本文件
// 显式导入消歧（settings_ui.rs 同款先例；main.rs 枢纽组不 export column）。
use iced::widget::column;
use iced::widget::text::{Fragment, Rich, Span as TextSpan};
use std::path::Path;
// 链接点击的目标类型与正文链接共用一套（main.rs 枢纽组不含它，故显式导入；
// 见 `crate::editor::links`）。
use crate::editor::LinkTarget;

use editpad_core::markdown::{MdAlign, MdBlock, MdSpan};

/// 预览的配色：全部从主题 palette 派生（浅/深两套都能读，改主题不瞎）。
///
/// 改前三处颜色是写死的浅色主题字面量（`[0.12,0.36,0.6]` 等），深色主题下
/// 代码段与引用几乎看不见——与 `popup_card_style`（chrome.rs）同一口径重写。
pub(crate) struct MdInk {
    text: Color,
    code: Color,
    code_bg: Color,
    quote: Color,
    link: Color,
    box_border: Color,
}

impl MdInk {
    fn of(theme: Theme) -> Self {
        let p = theme.palette();
        Self {
            text: p.text,
            code: mix(p.text, p.primary, 0.6),
            code_bg: mix(p.background, p.text, 0.07),
            quote: mix(p.text, p.background, 0.3),
            link: p.primary,
            box_border: mix(p.background, p.text, 0.18),
        }
    }
}

/// 按系数 `t` 混两色（0＝全 a，1＝全 b）。
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

/// 把解析出的块级元素排成**会换行**的只读预览列（滚动容器包裹）。
///
/// O-6：收已解析的块表而非源文本——解析结果的缓存归调用方（见
/// `App::md_preview_cache`），本函数只负责每帧重建 widget 树（即时模式本就
/// 每帧重建）。
///
/// 排版改走 `Rich`（一段落＝一个富文本段）：改前每个块排成 `row![]`，块内
/// 每个样式段是**独立的 text 控件**——一段里只要出现行内标记，后面的段就拿
/// 不到宽度（实测 1280 宽视口下整列只按内容自然宽收缩、每块恒一行 19.5px，
/// 长段落直接横着跑出窗口）。Rich 让所有段在同一段落里流动并按词换行。
///
/// `font_size` = 当前正文字号：预览排版随 Ctrl+滚轮缩放（P36 口径：
/// 预览属文件内容渲染故跟随；工具栏等 UI 控件不跟随）。
/// `base` = 正文字形族（P34）；**代码段与代码块恒用等宽族**——比例字体的
/// 代码块失去对齐，这不是「随设置切换」该覆盖的范围。
/// `doc_dir` = 当前页所在目录，用于解析相对路径链接（`[图](./a.png)`）。
pub(crate) fn markdown_preview_element(
    blocks: &[MdBlock],
    font_size: f32,
    base: Font,
    theme: Theme,
    doc_dir: Option<&Path>,
) -> Element<'static, Message> {
    let ink = MdInk::of(theme);
    // 预览各级字号相对正文默认 16px 的既有比例
    let scaled = |px: f32| px * (font_size / 16.0);
    let prose = scaled(15.0);

    let mut col = column![].spacing(10).padding(14);
    for block in blocks {
        match block {
            MdBlock::Heading { level, spans } => {
                let px = match level {
                    1 => scaled(26.0),
                    2 => scaled(23.0),
                    3 => scaled(20.0),
                    4 => scaled(18.0),
                    5 => scaled(17.0),
                    _ => scaled(16.0),
                };
                col = col.push(md_paragraph(
                    to_text_spans(spans, base, &ink, true, None, doc_dir),
                    px,
                    Alignment::Start,
                ));
            }
            MdBlock::Paragraph { spans } => {
                col = col.push(md_paragraph(
                    to_text_spans(spans, base, &ink, false, None, doc_dir),
                    prose,
                    Alignment::Start,
                ));
            }
            MdBlock::ListItem {
                order,
                indent,
                task,
                spans,
            } => {
                let marker = match (task, order) {
                    (Some(true), _) => "☑ ",
                    (Some(false), _) => "☐ ",
                    (_, Some(n)) => &format!("{n}. "),
                    _ => "• ",
                };
                let mut line = vec![plain_span(Fragment::Owned(marker.to_owned()), base, None)];
                line.extend(to_text_spans(spans, base, &ink, false, None, doc_dir));
                let indent_px = (*indent as f32).min(6.0) * scaled(16.0);
                let para = md_paragraph(line, prose, Alignment::Start);
                // 顶层列表项**不套容器**：一个块＝一个布局节点，用例才数得清
                // 「一块一节点」这条契约（嵌套层级才需要内缩）。
                let child: Element<'static, Message> = if indent_px == 0.0 {
                    para.into()
                } else {
                    // 嵌套层级＝左右一起内缩（`Padding` 只吃 f32/[f32;2]）
                    container(para)
                        .padding(Padding::from([0.0, indent_px]))
                        .into()
                };
                col = col.push(child);
            }
            MdBlock::Quote { depth, spans } => {
                let mut line = vec![plain_span(
                    Fragment::Owned("▌ ".repeat(*depth)),
                    base,
                    Some(ink.quote),
                )];
                line.extend(to_text_spans(
                    spans,
                    base,
                    &ink,
                    false,
                    Some(ink.quote),
                    doc_dir,
                ));
                col = col.push(md_paragraph(line, prose, Alignment::Start));
            }
            MdBlock::CodeBlock { lines, .. } => {
                let mut body = column![].spacing(0);
                for line in lines {
                    body = body.push(
                        text(line.clone())
                            .size(scaled(14.0))
                            .font(Font::MONOSPACE)
                            .color(ink.text)
                            .width(Fill),
                    );
                }
                let bg = ink.code_bg;
                let border = ink.box_border;
                col = col.push(
                    container(body)
                        .padding([6, 10])
                        .width(Fill)
                        .style(move |_| container::Style {
                            background: Some(Background::Color(bg)),
                            border: Border {
                                color: border,
                                width: 1.0,
                                radius: Radius::from(4.0),
                            },
                            ..container::Style::default()
                        }),
                );
            }
            MdBlock::Table {
                align,
                header,
                rows,
            } => {
                let mut table = column![].spacing(2);
                table = table.push(md_table_row(
                    header,
                    align,
                    base,
                    &ink,
                    doc_dir,
                    scaled(15.0),
                    true,
                ));
                for row_cells in rows {
                    table = table.push(md_table_row(
                        row_cells,
                        align,
                        base,
                        &ink,
                        doc_dir,
                        scaled(15.0),
                        false,
                    ));
                }
                col = col.push(table);
            }
            MdBlock::Rule => {
                col = col.push(rule::horizontal(2));
            }
        }
    }
    scrollable(container(col).width(Fill)).height(Fill).into()
}

/// 一行表格：每格等宽，按分隔行的 `:---:` 对齐，表头整行带底色。
fn md_table_row(
    cells: &[Vec<MdSpan>],
    align: &[MdAlign],
    base: Font,
    ink: &MdInk,
    doc_dir: Option<&Path>,
    px: f32,
    header: bool,
) -> Element<'static, Message> {
    let mut r = row![].spacing(8);
    for (idx, cell) in cells.iter().enumerate() {
        let a = match align.get(idx).copied().unwrap_or(MdAlign::Left) {
            MdAlign::Left => Alignment::Start,
            MdAlign::Center => Alignment::Center,
            MdAlign::Right => Alignment::End,
        };
        r = r.push(
            md_paragraph(to_text_spans(cell, base, ink, header, None, doc_dir), px, a).width(Fill),
        );
    }
    if !header {
        return r.into();
    }
    let bg = ink.code_bg;
    container(r)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            ..container::Style::default()
        })
        .into()
}

/// 一段富文本（整段按词换行、块内样式连续流动）。
fn md_paragraph(
    spans: Vec<TextSpan<'static, LinkTarget>>,
    px: f32,
    align: Alignment,
) -> Rich<'static, LinkTarget, Message> {
    Rich::with_spans(spans)
        .size(px)
        .width(Fill)
        .align_x(align)
        .width(Fill)
        .on_link_click(Message::LinkClicked)
}

/// 一个不带样式的 span（列表符号、引用竖条这类渲染器自己加的前缀）。
fn plain_span<'a>(
    text: Fragment<'a>,
    font: Font,
    color: Option<Color>,
) -> TextSpan<'a, LinkTarget> {
    let mut span = TextSpan::new(text);
    span.font = Some(font);
    span.color = color;
    span
}

/// core 的行内段 → Rich 的 span：粗／斜／代码／删除线／链接／图片各落一格。
fn to_text_spans(
    spans: &[MdSpan],
    base: Font,
    ink: &MdInk,
    bold_all: bool,
    color: Option<Color>,
    doc_dir: Option<&Path>,
) -> Vec<TextSpan<'static, LinkTarget>> {
    use iced::font::{Style as FontStyle, Weight};

    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let is_code = span.is_code();
        let text = if span.is_image() {
            // 图片不加载外部资源：显示标记 + 替代文字（无替代文字时用文件名）
            let alt = if span.text.is_empty() {
                link_file_name(span.link.as_deref())
            } else {
                span.text.clone()
            };
            format!("▣ {alt}")
        } else {
            span.text.clone()
        };
        let mut ts = TextSpan::new(Fragment::Owned(text));
        ts.font = Some(Font {
            family: if is_code {
                Font::MONOSPACE.family
            } else {
                base.family
            },
            weight: if bold_all || span.is_bold() {
                Weight::Bold
            } else {
                Weight::Normal
            },
            style: if span.is_italic() {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..base
        });
        ts.color = Some(match (span.is_link(), color) {
            (true, _) => ink.link,
            (false, Some(c)) => c,
            (false, None) => {
                if is_code {
                    ink.code
                } else {
                    ink.text
                }
            }
        });
        ts.underline = span.is_link();
        ts.strikethrough = span.is_strike();
        // 只有真能打开的目标才挂 link：坏链不该长得像能点的东西
        ts.link = span
            .link
            .as_deref()
            .and_then(|dest| md_link_target(dest, doc_dir));
        out.push(ts);
    }
    out
}

/// 从链接目标取文件名（图片没有替代文字时的兜底显示）。
fn link_file_name(dest: Option<&str>) -> String {
    dest.and_then(|d| d.split(['#', '?']).next())
        .map(|p| {
            Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "图片".to_owned())
}

/// 把 Markdown 的链接目标解析成与正文链接**同一套**可点目标（P133 口径）。
///
/// * scheme 形态（http/https/ftp/file/mailto）→ 交系统打开；
/// * 其余按文件路径处理：相对路径以**当前文档所在目录**为基准，且必须存在
///   （与 `editor::links` 一致——存在性一票否决，避免把 `2026/09/02` 这类
///   日期当路径）；
/// * `#锚点` 与空目标 → `None`（预览没有滚动定位，不给可点外观）。
pub(crate) fn md_link_target(dest: &str, doc_dir: Option<&Path>) -> Option<LinkTarget> {
    let head = dest.split(['#', '?']).next().filter(|s| !s.is_empty())?;
    if head.starts_with("http://")
        || head.starts_with("https://")
        || head.starts_with("ftp://")
        || head.starts_with("file://")
        || head.starts_with("mailto:")
    {
        return Some(LinkTarget::Url(head.to_owned()));
    }
    let path = Path::new(head);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        doc_dir?.join(path)
    };
    if resolved.exists() {
        Some(LinkTarget::File {
            path: resolved,
            line: None,
        })
    } else {
        None
    }
}
