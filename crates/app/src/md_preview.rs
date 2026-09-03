use super::*;
// `column!` 宏在 glob 链与宏前奏之间有歧义（E0659 实测），需本文件
// 显式导入消歧（settings_ui.rs 同款先例；main.rs 枢纽组不 export column）。
use iced::widget::column;

/// 把解析出的块级元素排成只读预览列（滚动容器包裹）。
///
/// `font_size` = 当前正文字号：预览排版随 Ctrl+滚轮缩放（P36 口径：
/// 预览属文件内容渲染故跟随；工具栏等 UI 控件不跟随），各级基准值与
/// 旧硬编码一致（正文默认 16px 时逐项像素相等）。
/// `base` = 正文字形族（P34：预览属内容渲染，随设置切换）。
pub(crate) fn markdown_preview_element(
    source: &str,
    font_size: f32,
    base: Font,
) -> Element<'static, Message> {
    use iced::font::Weight;

    // 预览各级字号相对正文默认 16px 的既有比例
    let scaled = |px: f32| px * (font_size / 16.0);

    let blocks = editpad_core::markdown::parse_markdown(source);
    let mut col = column![].spacing(10).padding(14);
    for block in blocks {
        match block {
            editpad_core::markdown::MdBlock::Heading { level, spans } => {
                let px = match level {
                    1 => scaled(26.0),
                    2 => scaled(23.0),
                    3 => scaled(20.0),
                    4 => scaled(18.0),
                    5 => scaled(17.0),
                    _ => scaled(16.0),
                };
                col = col.push(md_spans_row(spans, px, true, base));
            }
            editpad_core::markdown::MdBlock::Paragraph { spans } => {
                col = col.push(md_spans_row(spans, scaled(15.0), false, base));
            }
            editpad_core::markdown::MdBlock::ListItem { spans } => {
                let mut line =
                    row![text("• ").size(scaled(15.0)).font(base)];
                for span in spans {
                    let font = md_font(base, span.is_bold(), span.is_italic());
                    let mut t =
                        text(span.text.clone()).size(scaled(15.0)).font(font);
                    if span.is_code() {
                        t = t.color([0.12, 0.36, 0.6]);
                    }
                    line = line.push(t);
                }
                col = col.push(line);
            }
            editpad_core::markdown::MdBlock::Quote { spans } => {
                let mut line = row![
                    text("▌ ")
                        .size(scaled(15.0))
                        .font(base)
                        .color([0.55, 0.55, 0.6])
                ];
                for span in spans {
                    let font = md_font(base, span.is_bold(), span.is_italic());
                    let t = text(span.text.clone())
                        .size(scaled(15.0))
                        .font(font)
                        .color([0.45, 0.45, 0.5]);
                    line = line.push(t);
                }
                col = col.push(line);
            }
            editpad_core::markdown::MdBlock::CodeBlock { lines } => {
                for line in lines {
                    col = col.push(
                        text(format!("▏ {line}"))
                            .size(scaled(14.0))
                            .font(Font {
                                weight: Weight::Normal,
                                ..base
                            })
                            .color([0.25, 0.35, 0.45]),
                    );
                }
            }
            editpad_core::markdown::MdBlock::Rule => {
                col = col.push(rule::horizontal(2));
            }
        }
    }
    scrollable(container(col).width(Fill)).height(Fill).into()
}

/// 按行内样式构造文本片段行。
pub(crate) fn md_spans_row(
    spans: Vec<editpad_core::markdown::MdSpan>,
    px: f32,
    bold: bool,
    base: Font,
) -> iced::widget::Row<'static, Message> {
    use iced::font::{Style as FontStyle, Weight};

    let mut row = row![].spacing(0);
    if spans.is_empty() {
        row = row.push(text(""));
        return row;
    }
    for span in spans {
        let weight = if bold || span.is_bold() {
            Weight::Bold
        } else {
            Weight::Normal
        };
        let style = if span.is_italic() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let font = Font {
            weight,
            style,
            ..base
        };
        let mut t = text(span.text.clone()).size(px).font(font);
        if span.is_code() {
            t = t.color([0.12, 0.36, 0.6]);
        }
        row = row.push(t);
    }
    row
}

pub(crate) fn md_font(base: Font, bold: bool, italic: bool) -> Font {
    Font {
        weight: if bold {
            iced::font::Weight::Bold
        } else {
            iced::font::Weight::Normal
        },
        style: if italic {
            iced::font::Style::Italic
        } else {
            iced::font::Style::Normal
        },
        ..base
    }
}
