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
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree};
use iced::advanced::{Clipboard, Shell};
use iced::Event;

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
/// `block_cache` = **按块**复用的上一帧树（见下面的复用说明）。
pub(crate) fn markdown_preview_element(
    blocks: &[MdBlock],
    font_size: f32,
    base: Font,
    theme: Theme,
    doc_dir: Option<&Path>,
    block_cache: &MdBlockCacheSlot,
) -> Element<'static, Message> {
    let dark = matches!(theme, Theme::Dark);
    let ink = MdInk::of(theme);
    // 整次建树共享一份着色额度（见 `MdColorBudget`）
    let color_budget = MdColorBudget(std::cell::Cell::new(MD_COLOR_LINE_BUDGET));
    note_build();
    // 预览各级字号相对正文默认 16px 的既有比例
    let scaled = |px: f32| px * (font_size / 16.0);

    // ── 按块复用（P412）────────────────────────────────────────────────────
    // 外层整棵树的缓存（`App::md_preview_tree`）只管"内容没变的帧"；换版那一帧
    // 以前总是整棵重建，实测成本随**块数**走（README×10＝1421 块 133 ms；
    // 1500 个小代码块 347 ms）。这里再叠一层按块缓存：块的内容指纹没变就直接
    // 复用上一帧那一棵（`MdCached` 包住，widget 实例跨帧存活 ⇒ 里面的 `Rich`
    // 排版缓存也活着），只有指纹变了的区间才重建。
    // 前后各裁一段公共区：在文末敲一个字 ⇒ 只有那一个块重建；插入一个新块会让
    // 下标整体右移，但**从尾部数过来的公共后缀**仍然对得上，所以也不会全重建。
    let fps: Vec<u64> = blocks.iter().map(md_block_fingerprint).collect();
    let mut next: Vec<(u64, MdTree)> = Vec::with_capacity(blocks.len());
    {
        let old = block_cache.0.borrow();
        let mut lo = 0usize;
        while lo < fps.len() && lo < old.len() && old[lo].0 == fps[lo] {
            lo += 1;
        }
        let mut hi = 0usize;
        while hi + lo < fps.len()
            && hi + lo < old.len()
            && old[old.len() - 1 - hi].0 == fps[fps.len() - 1 - hi]
        {
            hi += 1;
        }
        next.extend(old[..lo].iter().map(|(f, t)| (*f, t.clone())));
        for i in lo..fps.len() - hi {
            let tree: MdTree = Rc::new(std::cell::RefCell::new(Some(build_block(
                &blocks[i],
                &scaled,
                base,
                &ink,
                dark,
                doc_dir,
                &color_budget,
            ))));
            next.push((fps[i], tree));
        }
        next.extend(old[old.len() - hi..].iter().map(|(f, t)| (*f, t.clone())));
    }
    *block_cache.0.borrow_mut() = next.clone();

    let mut col = column![].spacing(10).padding(14);
    for (_, tree) in next {
        col = col.push(Element::new(MdCached::new(tree)));
    }
    scrollable(container(col).width(Fill)).height(Fill).into()
}

/// 一个块 → 一棵独立的 widget 树（按块缓存的最小单位，见
/// [`markdown_preview_element`] 里的复用说明）。
fn build_block(
    block: &MdBlock,
    scaled: &dyn Fn(f32) -> f32,
    base: Font,
    ink: &MdInk,
    dark: bool,
    doc_dir: Option<&Path>,
    color_budget: &MdColorBudget,
) -> Element<'static, Message> {
    note_block_build();
    let prose = scaled(15.0);
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
            md_paragraph(
                to_text_spans(spans, base, ink, true, None, doc_dir),
                px,
                Alignment::Start,
            )
            .into()
        }
        MdBlock::Paragraph { spans } => md_paragraph(
            to_text_spans(spans, base, ink, false, None, doc_dir),
            prose,
            Alignment::Start,
        )
        .into(),
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
            line.extend(to_text_spans(spans, base, ink, false, None, doc_dir));
            let indent_px = (*indent as f32).min(6.0) * scaled(16.0);
            let para = md_paragraph(line, prose, Alignment::Start);
            // 顶层列表项**不套容器**：一个块＝一个布局节点，用例才数得清
            // 「一块一节点」这条契约（嵌套层级才需要内缩）。
            if indent_px == 0.0 {
                para.into()
            } else {
                // 嵌套层级＝左右一起内缩（`Padding` 只吃 f32/[f32;2]）
                container(para)
                    .padding(Padding::from([0.0, indent_px]))
                    .into()
            }
        }
        MdBlock::Quote { depth, spans } => {
            // 深度条挨着排（"▌▌ 文本"），中间不空格——空一格读起来像两段引用
            let mut line = vec![plain_span(
                Fragment::Owned(format!("{} ", "▌".repeat(*depth))),
                base,
                Some(ink.quote),
            )];
            line.extend(to_text_spans(
                spans,
                base,
                ink,
                false,
                Some(ink.quote),
                doc_dir,
            ));
            md_paragraph(line, prose, Alignment::Start).into()
        }
        MdBlock::CodeBlock { lines, lang } => {
            let body = Rich::with_spans(code_block_spans(
                lines,
                lang.as_deref(),
                base,
                dark,
                color_budget,
            ))
            .size(scaled(14.0))
            .width(Fill)
            // 代码不该把整词推到下一行：长 token 按字形断开
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
            let bg = ink.code_bg;
            let border = ink.box_border;
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
                })
                .into()
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
                ink,
                doc_dir,
                scaled(15.0),
                true,
            ));
            for row_cells in rows {
                table = table.push(md_table_row(
                    row_cells,
                    align,
                    base,
                    ink,
                    doc_dir,
                    scaled(15.0),
                    false,
                ));
            }
            table.into()
        }
        MdBlock::Rule => rule::horizontal(2).into(),
    }
}

/// 块的**内容指纹**：同一篇里内容没变的块复用上一帧那棵树。
fn md_block_fingerprint(block: &MdBlock) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    block.hash(&mut h);
    h.finish()
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

/// 预览缓存的依赖指纹（[`MdCached`] 的比对键）。
///
/// `seq` 是**必要**的一维，不能只靠块表指针：旧 Rc 一被释放，分配器很可能把同一
/// 地址给下一份等长 Vec ⇒ 光看指针会把"新文档"认成"没变"，预览停在陈内容。
pub(crate) fn md_preview_key(
    seq: u64,
    blocks_ptr: usize,
    font_size: f32,
    dark: bool,
    family: Option<&'static str>,
    dir: Option<&Path>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seq.hash(&mut h);
    blocks_ptr.hash(&mut h);
    font_size.to_bits().hash(&mut h);
    dark.hash(&mut h);
    family.hash(&mut h);
    dir.map(|p| p.display().to_string()).hash(&mut h);
    h.finish()
}

/// 预览换版的节流窗口（毫秒）：距上一次换版不足这么久就**继续用旧树**。
///
/// 为什么还要这一道：`Rich` 的排版缓存活在 widget 实例上，P408 的复用把"没改动的帧"
/// 降到约 1 ms，但**内容一变仍要整棵重建**（实测 353 KB 那档约 600 ms）⇒ 左边打字、
/// 右边预览就是每键一卡顿。节流后一次连打最多每窗口换版一次。
///
/// ⚠️ **停手后的最后一版靠谁补**：没为它新订定时链（那要新增 Message），换版靠既有
/// 拍子再跑一次 `view()`＝光标闪烁拍（`editor::CARET_BLINK_MS`＝530 ms，聚焦且在编辑
/// 态时在订）或快照心跳拍（默认 10 s）⇒ 落地时延 ≈ 窗口＋一拍（约 0.3～0.8 s）。
/// 若停手后立刻失焦（闪烁拍被撤销、心跳又关着），预览会停在上一版直到下一条消息
/// 进来——这一条无头测不到（订阅时序），登记为眼验项。300 ms ≈ "还跟手"的上限。
pub(crate) const MD_PREVIEW_SWAP_MS: u64 = 300;

#[cfg(test)]
thread_local! {
    /// 测试里把窗口调 0（立刻换）或极大（永不换），让节流能被**确定性地**盯住——
    /// 不靠 sleep（本仓口径：耗时读数在忙机器假红、温热假绿）。
    pub(crate) static MD_SWAP_WINDOW_MS: std::cell::Cell<u64> =
        const { std::cell::Cell::new(MD_PREVIEW_SWAP_MS) };
}

/// 现在是否允许换预览版。`last`＝上一次换版的时刻（`None`＝还没换过 ⇒ 允许）。
pub(crate) fn md_preview_can_swap(last: Option<std::time::Instant>) -> bool {
    #[cfg(test)]
    let window = MD_SWAP_WINDOW_MS.with(|c| c.get());
    #[cfg(not(test))]
    let window = MD_PREVIEW_SWAP_MS;
    match last {
        None => true,
        Some(t) => window == 0 || t.elapsed() >= std::time::Duration::from_millis(window),
    }
}

/// 取本帧该用的预览树：键没变⇒复用；键变了但还在节流窗口里⇒**继续用旧树**；
/// 否则整棵重建并记下换版时刻。
///
/// 旧树是**自足**的（`markdown_preview_element` 把每个 span 的文本克隆成 owned
/// Fragment），所以即使旧块表已经被释放，旧树仍能把上一版内容完整画出来——
/// 节流期间预览落后 ≤ 一个窗口，而不是画错或 panic。
pub(crate) fn md_preview_tree_for(
    holder: &MdTreeSlot,
    swap_at: &std::cell::Cell<Option<std::time::Instant>>,
    want: u64,
    build: impl FnOnce() -> Element<'static, Message>,
) -> MdTree {
    let mut held = holder.0.borrow_mut();
    match held.as_mut() {
        Some((k, slot)) if *k == want => slot.clone(),
        Some((_, slot)) if !md_preview_can_swap(swap_at.get()) => slot.clone(),
        _ => {
            let built = Rc::new(std::cell::RefCell::new(Some(build())));
            *held = Some((want, built.clone()));
            swap_at.set(Some(std::time::Instant::now()));
            built
        }
    }
}

// 预览树真正被构建的次数——成本契约要**看得见**，不能靠量耗时断言（忙机器假红、
// 温热假绿，本仓已两次因此把成本契约改成计数／同一性）。用 `thread_local` 而非
// 全局原子：测试线程各跑各的，全局计数会被并发用例串味。
#[cfg(test)]
thread_local! {
    pub(crate) static MD_BUILD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// **按块**建树的次数（P412 的尺子：改了尾部一个块，就不该把整篇都重建）。
    pub(crate) static MD_BLOCK_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_build() {
    MD_BUILD_COUNT.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn note_block_build() {
    MD_BLOCK_BUILDS.with(|c| c.set(c.get() + 1));
}

#[cfg(not(test))]
#[inline]
fn note_build() {}

#[cfg(not(test))]
#[inline]
fn note_block_build() {}

/// 按块缓存的那一排树：`(内容指纹, 树)`，下标＝块序。
///
/// 包一层同样是为了 `Debug`（`Editpad` derive(Debug)，而 `Element` 没有）。
#[derive(Clone)]
pub(crate) struct MdBlockCacheSlot(pub std::cell::RefCell<Vec<(u64, MdTree)>>);

impl Default for MdBlockCacheSlot {
    fn default() -> Self {
        Self(std::cell::RefCell::new(Vec::new()))
    }
}

impl std::fmt::Debug for MdBlockCacheSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MdBlockCacheSlot({} 块)", self.0.borrow().len())
    }
}

/// 把整棵预览树**缓存**在 widget 的 Tree 状态里，依赖不变就复用上一次的实例。
///
/// 为什么需要：即时模式每帧重建 widget，而 `Rich` 把排好的段落缓存**在 widget
/// 自己身上**——重建即丢失，于是预览开态的每一帧都把整篇重新折字。实测
/// （release／1280×800／只算布局）：预览**关**态 1.1 ms/帧（与文档大小无关，
/// 正文面本来就虚拟化），**开**态 59 ms（35 KB／143 块）→ 180 ms（105 KB／427 块）
/// → 600 ms（353 KB／1421 块）。空闲心跳每秒约 5 帧 ⇒ 大文档光"开着预览不动手"
/// 就吃掉每秒约 3 秒 CPU。
///
/// 做法抄上游 `iced_widget::lazy`（本仓没开 `lazy` 特性——它要拉 `ouroboros`，
/// 与"Cargo.lock 不新增包"的纪律冲突，故把这套委托自带一份；上游代码 Apache-2.0）。
/// 缓存的 `Element` 留在 `tree.state` 里 ⇒ 内部的排版缓存跨帧存活；依赖（块表
/// 指针＋字号＋字形族＋主题＋链接基准目录）一变就整棵重建。
///
/// ⚠️ 没实现 `overlay`（默认 `None`）：预览树里只有 Rich／container／text／
/// scrollable／rule，滚动条走 `draw`＋`update`，不是 overlay 组件。
/// 跨帧存活的预览树：`Rc` 里放**同一份**已构建 Element。
pub(crate) type MdTree = Rc<std::cell::RefCell<Option<Element<'static, Message>>>>;

/// 应用状态里那一格：依赖指纹＋树。
///
/// 包一层是因为 `Editpad` 整块 `derive(Debug)`，而 iced 的 `Element` 没有 `Debug`。
#[derive(Clone, Default)]
pub(crate) struct MdTreeSlot(pub std::cell::RefCell<Option<(u64, MdTree)>>);

impl std::fmt::Debug for MdTreeSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MdTreeSlot({})", self.0.borrow().is_some())
    }
}

/// 把**已经建好**的预览树原样交给 iced，不再逐帧重建。
///
/// 病（实测，release／1280×800／只算布局）：预览**关**态 1.1 ms/帧（正文面本来就
/// 虚拟化，与文档大小无关），**开**态 59 → 180 → 600 ms 对应 35 KB／105 KB／353 KB。
/// 原因是即时模式每帧重建 widget，而 `Rich` 把排好的段落缓存**在 widget 自己身上**
/// ⇒ 重建即丢，每一帧都把整篇重新折字；空闲心跳每秒约 5 帧 ⇒ 353 KB 文档光"开着
/// 预览不动手"就吃掉每秒约 3 秒 CPU。
///
/// 做法与上游 `iced_widget::lazy` 同形（本仓没开 `lazy` 特性——它要拉 `ouroboros`，
/// 与"Cargo.lock 不新增包"的纪律冲突，故自带一份委托；上游代码 Apache-2.0）。
/// ⚠️ 关键差别：缓存**握在应用状态里**（`App::md_preview_tree`），不放在 widget 的
/// `Tree::state`——本轮先按上游那样放 Tree 状态，实测第一帧就被建了两次：`Tree::new`
/// 之前父层还会问 `children()`／`size()`，而父层是否把 `diff` 传导到本子节点并不由本
/// widget 保证（`ViewTree` 夹具里根本复现不出来）。握在状态里 ⇒ 只有"依赖变了"才会
/// 重建，与 iced 内部 diff 时序无关。
///
/// ⚠️ 没实现 `overlay`（默认 `None`）：预览树里只有 Rich／container／text／
/// scrollable／rule，滚动条走 `draw`＋`update`，不是 overlay 组件。
pub(crate) struct MdCached {
    inner: MdTree,
}

impl MdCached {
    pub(crate) fn new(inner: MdTree) -> Self {
        Self { inner }
    }

    fn with_element<T>(&self, f: impl FnOnce(&Element<'static, Message>) -> T) -> T {
        let held = self.inner.borrow();
        f(held.as_ref().expect("交给本 widget 的树必须已建好"))
    }

    fn with_element_mut<T>(&self, f: impl FnOnce(&mut Element<'static, Message>) -> T) -> T {
        let mut held = self.inner.borrow_mut();
        f(held.as_mut().expect("交给本 widget 的树必须已建好"))
    }
}

impl widget::Widget<Message, Theme, iced::Renderer> for MdCached {
    fn children(&self) -> Vec<Tree> {
        self.with_element(|element| vec![Tree::new(element.as_widget())])
    }

    fn diff(&self, tree: &mut Tree) {
        self.with_element(|element| tree.diff_children(std::slice::from_ref(&element.as_widget())));
    }

    fn size(&self) -> iced::Size<iced::Length> {
        self.with_element(|element| element.as_widget().size())
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.with_element_mut(|element| {
            element
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        })
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.with_element_mut(|element| {
            element
                .as_widget_mut()
                .operate(&mut tree.children[0], layout, renderer, operation)
        })
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.with_element_mut(|element| {
            element.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            )
        })
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.with_element(|element| {
            element.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        })
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.with_element(|element| {
            element.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            )
        })
    }
}

// ── 代码块着色 ────────────────────────────────────────────────────────────────

/// 一次建树里**还能着色多少行**的额度（1500 行 × 近似路径 ≈75 µs ⇒ 着色部分
/// 约 110 ms 封顶）。额度在 `markdown_preview_element` 里随建树新建，不在树上共享。
pub(crate) const MD_COLOR_LINE_BUDGET: usize = 1500;

pub(crate) struct MdColorBudget(pub std::cell::Cell<usize>);

/// 默认＝满额度（`Default` 不是 0：0 额度意味着"整篇不上色"，那是特例，
/// 要显式写 `MdColorBudget(Cell::new(0))`，测试里才这么用）。
impl Default for MdColorBudget {
    fn default() -> Self {
        Self(std::cell::Cell::new(MD_COLOR_LINE_BUDGET))
    }
}

impl MdColorBudget {
    /// 领走最多 `want` 行的着色额度，返回实际允许的行数（0＝整块素色）。
    pub(crate) fn take(&self, want: usize) -> usize {
        let left = self.0.get();
        let got = want.min(left);
        self.0.set(left - got);
        got
    }
}

/// 代码块＝整块一个富文本段，并试着按围栏 info 串上色。
///
/// 为什么值得做：代码块是 .md 里最常要"看清楚"的内容，此前预览里它是一行行素色
/// 文本，与正文无异。`core::highlight::Highlighter` 早就给正文编辑器产出逐行
/// `StyledRun`（颜色已按主题解算），预览复用零新依赖。
///
/// 认法：先按扩展名（`` ```py ````／`` ```rs ````），再按语法名首字母大写
/// （`` ```Python ````）；都不中 ⇒ **整块素色**（不嗅探——语言是作者写明的，
/// 猜错比不猜更误导）。
///
/// 成本（实测，release）：3000 行的 ```rs 块走精确 `styled_line` 建树要 **596 ms**，
/// 同一块认不出语言（整块素色）只要 **2.4 ms** ⇒ 换版那一帧几乎全花在逐行 syntect
/// 解析上。两道闸：① 走 `styled_line_approx`（P61 的渐进上色，**不建检查点**、
/// 连续逐行 ≈75 µs，比精确路径快 2.7 倍；取舍＝跨 anchor 的前几行可能错色，
/// 而正文编辑器里那份仍是精确上色）；② 一次建树一个**总行数额度**
/// （[`MD_COLOR_LINE_BUDGET`]），额度用尽后的块整块素色 ⇒ 着色成本封顶。
pub(crate) fn code_block_spans(
    lines: &[String],
    lang: Option<&str>,
    base: Font,
    dark: bool,
    budget: &MdColorBudget,
) -> Vec<TextSpan<'static, LinkTarget>> {
    let ink = MdInk::of(if dark { Theme::Dark } else { Theme::Light });
    // 先领额度：额度为 0 时连高亮器都不必构造
    let colored_allowed = budget.take(lines.len());

    let mut hl = lang.and_then(|raw| {
        let t = raw.trim();
        if t.is_empty() {
            return None;
        }
        let named = {
            let mut n = t.to_owned();
            if let Some(head) = n.get_mut(0..1) {
                head.make_ascii_uppercase();
            }
            n
        };
        editpad_core::LazyHighlighter::new(t)
            .or_else(|| editpad_core::LazyHighlighter::new_by_name(&named))
    });
    if let Some(h) = hl.as_mut() {
        h.set_dark_mode(dark);
    }
    let plain = [ink.text.r, ink.text.g, ink.text.b, 1.0];
    let mut text_of = |idx: usize| lines.get(idx).cloned().unwrap_or_default();

    let mut spans: Vec<TextSpan<'static, LinkTarget>> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        // 逐字上色：run 覆盖到的字用 run 的颜色，没覆盖到的（含整块认不出语法时）
        // 用正文字色。**按列对齐**而不是直接铺 run——run 之间有缝隙时不丢字，
        // 也不会把同一个字重画两遍。
        let mut colored: Vec<[f32; 4]> = vec![plain; n];
        // 只有额度内的行上色；超出的行留在 `plain` 上（整块后半素色）
        if let Some(h) = hl.as_mut().filter(|_| idx < colored_allowed) {
            for r in h.styled_line_approx(idx, 0, line, lines.len(), &mut text_of) {
                if r.start_col >= n {
                    continue;
                }
                let to = r.end_col.min(n);
                for c in colored.iter_mut().take(to).skip(r.start_col) {
                    *c = r.color;
                }
            }
        }
        // 连续同色合并成一段（少几个 span，样式不变）
        let mut pieces: Vec<(String, [f32; 4])> = Vec::new();
        for (i, ch) in chars.iter().enumerate() {
            let c = colored[i];
            match pieces.last_mut() {
                Some((buf, last)) if *last == c => buf.push(*ch),
                _ => pieces.push((ch.to_string(), c)),
            }
        }
        let last_idx = pieces.len().saturating_sub(1);
        for (pi, (piece, c)) in pieces.into_iter().enumerate() {
            let content = if pi == last_idx {
                format!("{piece}\n") // 行尾换行：整块仍是一个段落节点
            } else {
                piece
            };
            let mut ts = TextSpan::new(Fragment::Owned(content));
            ts.font = Some(Font {
                family: Font::MONOSPACE.family,
                ..base
            });
            ts.color = Some(Color {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            });
            spans.push(ts);
        }
        if n == 0 {
            // 空行没有字符就没有段，但它必须占一行高
            let mut ts = TextSpan::new(Fragment::Owned("\n".to_owned()));
            ts.font = Some(Font {
                family: Font::MONOSPACE.family,
                ..base
            });
            ts.color = Some(ink.text);
            spans.push(ts);
        }
    }
    spans
}
