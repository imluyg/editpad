//! 控件实现：`EditorView` 的布局/事件/绘制（P66 三层裁剪）与主题配色
//! （P68 自 editor.rs 拆出，纯移动零行为变更）。

use iced::advanced::{
    input_method,
    layout::{self, Layout},
    renderer::{self, Renderer as _},
    text::{self as core_text, Renderer as _},
    widget::Tree,
    Clipboard, Shell, Widget,
};
use iced::{alignment, border::Radius, mouse, window, Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme};

use super::core::{
    luminance, lighten, EditOp, EditorHandle, ImeCommit, SCROLL_LINES_PER_NOTCH,
};
use super::metrics::{
    char_cols, display_cols, measure_char_width, measure_ink_box, shape_row_xs,
};
use super::scrollbars::{
    HScrollbar, SCROLLBAR_EDGE_INSET, SCROLLBAR_THUMB_THICKNESS, SCROLLBAR_WIDTH,
    VScrollbar,
};
use super::wrap::segment_index as wrap_segment_index;
use super::wrap::pixel_breaks;
use super::{
    BOOKMARK_DOT, BOOKMARK_STRIP, GUTTER_FONT_SCALE, GUTTER_MIN, TEXT_LAYER_INSET,
};

impl super::core::EditorHandle {
    /// 构造可加入视图树的自绘控件。
    ///
    /// P34：字形族由调用方按设置传入——控件每帧重建，天然跟随运行期
    /// 切换，无需在 EditorCore 里维护同步状态（「下标 + 长期别名」教训
    /// 的反面教材：能靠每帧传参的状态就不要落库）。
    /// 默认语义 = [`BODY_FONT`]（P33 的 CJK 钉字仍生效）。
    pub fn view(&self, font: Font) -> Element<'_, crate::Message> {
        Element::new(EditorView { core: self.clone(), font, zoom_accum: 0.0 })
    }
}

// ---------- 字体一致性（P33） ----------
//
// 正文与 UI 的单一换装点：画布、行号栏、UI 控件全部引用 BODY_FONT，
// 字号全部从正文字号推导——P34（字体选择设置）落地时只需替换常量。

/// 正文与 UI 共用的字形族。
///
/// 现状 = `Font::MONOSPACE`，其 CJK 缺口由 [`apply_default_cjk_mono_pin`]
/// 在启动期把 `Family::Monospace` 的解析目标钉到系统里的 CJK 等宽字体来补齐
/// （方案 c：零体积治本，不捆绑字体文件）。
pub const BODY_FONT: Font = Font::MONOSPACE;

/// UI 控件字号的固定基准（px）。**P36 用户裁决：UI 不随正文字号缩放**——
/// Ctrl+滚轮（P48 落地）与设置弹窗的步进只调节文件内容，UI 控件保持
/// 固定尺寸。取 16px = iced 默认文本尺寸（`Settings::default_text_size`），
/// 与未缩放时的既有观感持平。
pub const UI_FONT_BASE_PX: f32 = 16.0;

/// Ctrl+滚轮缩放与设置面板步进共用的单步字号增量（px）。单一来源：
/// 两处入口的手感必须一致。
pub(crate) const FONT_ZOOM_STEP: f32 = 2.0;

/// UI 字号相对基准的微调系数（[`GUTTER_FONT_SCALE`] 先例；1.0 = 持平）。
pub const UI_FONT_SCALE: f32 = 1.0;

/// UI 控件统一字号（全项目唯一换算点）：`UI_FONT_BASE_PX × UI_FONT_SCALE`。
///
/// 勘误留痕：iced 0.14 默认文本尺寸实测为 **16px**
/// （`iced_core::settings::Settings::default_text_size`），与默认正文字号相同
/// ——第 25 轮「UI 14px vs 正文 16px」的记录有误，两层割裂实际只在字形族。
pub fn ui_font_px() -> f32 {
    UI_FONT_BASE_PX * UI_FONT_SCALE
}

/// CJK 等宽候选优先级表（P33 方案 c 钉字）：启动期从左到右扫描，
/// 第一个系统已安装的族名被设为 fontdb `Family::Monospace` 的解析目标。
///
/// - 更纱黑体 / Noto Mono CJK SC：社区推荐的中文等宽（用户自装时最优）；
/// - NSimSun（新宋体）/ MingLiU（细明体）：Windows 自带简/繁中文等宽；
/// - MS Gothic（ＭＳ ゴシック）/ Yu Gothic Mono：Windows 自带日文等宽；
/// - 全部未命中 → 不动（保持系统默认等宽解析，非 CJK 环境零行为变化）。
///
/// 刻意不含 SimSun（宋体）：其 ASCII 半宽非严格等宽，会破坏 P14 列映射。
pub const CJK_MONO_CANDIDATES: [&str; 6] = [
    "Sarasa Mono SC",
    "Noto Sans Mono CJK SC",
    "NSimSun",
    "MingLiU",
    "MS Gothic",
    "Yu Gothic Mono",
];

/// 族名归一：去空白 + 小写。候选表条目都是 ASCII 形态族名，
/// 与 fontdb 枚举出的本地化族名做同样宽松的比较即可覆盖大小写/空格变体。
/// P34 起公开：app 层的「配置字体 ↔ 系统清单」宽松匹配复用同一实现。
pub fn normalize_family(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 从已装字体族名集中挑出第一个命中的 CJK 等宽候选（纯函数便于测试）：
/// 候选表顺序即优先级；无命中返回 None（调用方保持现状不动）。
pub fn pick_cjk_mono_family(available: &[String]) -> Option<&'static str> {
    CJK_MONO_CANDIDATES.iter().find_map(|cand| {
        let want = normalize_family(cand);
        available
            .iter()
            .any(|family| normalize_family(family) == want)
            .then_some(*cand)
    })
}
// ---------- 控件实现 ----------

/// P48：Ctrl+滚轮缩放的单步判定（纯函数可单测）。
/// * `Lines`（滚轮格）：非零即一步，方向取符号（Windows 一格 y=±1）；
/// * `Pixels`（触控板）：增量已折算成行数累积，|累积| ≥ 1 行发一步并
///   **清零**（保留余量会触控板轻扫连发多步；清零 = 一步一格，与滚轮
///   手感一致）。反向增量先抵消同向累积。
///   返回 (新累积值, 步数符号；0 = 本帧不发)。
pub(crate) fn wheel_zoom_step(accum: f32, delta_lines: f32, is_pixels: bool) -> (f32, f32) {
    if is_pixels {
        let accum = accum + delta_lines;
        if accum.abs() >= 1.0 {
            (0.0, if accum > 0.0 { 1.0 } else { -1.0 })
        } else {
            (accum, 0.0)
        }
    } else if delta_lines != 0.0 {
        (accum, delta_lines.signum())
    } else {
        (accum, 0.0)
    }
}

/// P115：组字串实测宽（px）：与正文绘制同源的整串 shaping 末项（连字/
/// 混合宽度如实反映）；失败（字体未就绪）回退列模型保守宽（组字是
/// 瞬态，占位偏移误差可接受）。
fn measure_preedit_w(font: Font, size: f32, preedit: &str) -> f32 {
    shape_row_xs(font, size, preedit)
        .and_then(|xs| xs.last().copied())
        .unwrap_or_else(|| display_cols(preedit) * size * 0.5625)
}

/// P115：正文列区间 `[lo, hi)` 的单片绘制（组字三段式复用；也承载原
/// 折行/关态两分支的整段/整行路径，pixel 批守护等价）。`seg_start` =
/// 段内像素基准列（关态 0）；`dx` = 段起点外的附加偏移（组字 C 段 =
/// preedit 占位后移）；无 runs 时整片一色，有 runs 时逐 run 裁色。
#[allow(clippy::too_many_arguments)]
fn paint_text_slice(
    renderer: &mut iced::Renderer,
    core: &super::core::EditorCore,
    font: Font,
    x0: f32,
    y: f32,
    line: usize,
    seg_start: usize,
    text: &str,
    lo: usize,
    hi: usize,
    dx: f32,
    color: Color,
    runs: &[editpad_core::StyledRun],
    clip: Rectangle,
) {
    if lo >= hi {
        return;
    }
    let lh = core.line_height();
    let size = core.font_size();
    if runs.is_empty() {
        let segment: String = text.chars().skip(lo).take(hi - lo).collect();
        if segment.is_empty() {
            return;
        }
        renderer.fill_text(
            core_text::Text {
                content: segment,
                bounds: Size::new(f32::INFINITY, lh),
                size: Pixels(size),
                line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                font,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: core_text::Shaping::Advanced,
                wrapping: core_text::Wrapping::None,
            },
            Point::new(x0 + dx, y),
            color,
            clip,
        );
    } else {
        for run in runs {
            let s = run.start_col.max(lo);
            let e = run.end_col.min(hi);
            if e <= s {
                continue;
            }
            let segment: String = text.chars().skip(s).take(e - s).collect();
            if segment.is_empty() {
                continue;
            }
            let offset_px = core.px_of(line, text, s) - core.px_of(line, text, seg_start) + dx;
            let [r, g, b, a] = run.color;
            renderer.fill_text(
                core_text::Text {
                    content: segment,
                    bounds: Size::new(f32::INFINITY, lh),
                    size: Pixels(size),
                    line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                    font,
                    align_x: core_text::Alignment::Default,
                    align_y: alignment::Vertical::Top,
                    shaping: core_text::Shaping::Advanced,
                    wrapping: core_text::Wrapping::None,
                },
                Point::new(x0 + offset_px, y),
                Color::from_rgba8(
                    (r * 255.0).round() as u8,
                    (g * 255.0).round() as u8,
                    (b * 255.0).round() as u8,
                    a,
                ),
                clip,
            );
        }
    }
}

/// P115 续：组字行「插入重排」的预计算结果——合成串 S = 前文 + 组字 +
/// 后文；`s_xs`/`breaks` 按真实字形宽（shape 同源）与折行预算（含右缘
/// 一个汉字宽余量、P99 滚动条让位）断行；`k` = 该行视觉段数增量（后续
/// 逻辑行绘制整体下移 k 行）；`v0` = 该行原首段视觉行号。
struct ReflowLayout {
    line: usize,
    col_p: usize,
    pel: usize,
    s: String,
    s_xs: Vec<f32>,
    breaks: Vec<usize>,
    k: isize,
    v0: u32,
}

/// P115 续：组字重排段的绘制——合成串段 `[bs, be)` 内最多三块（前文/
/// 组字/后文）逐块上屏，x 全按合成串真实 xs 定位；前/后文块继承原行
/// runs 逐色（后文源列 = 合成索引 − pel），组字块用组字色。
#[allow(clippy::too_many_arguments)]
fn paint_composed_segment(
    renderer: &mut iced::Renderer,
    core: &super::core::EditorCore,
    font: Font,
    x0: f32,
    y: f32,
    s: &str,
    s_xs: &[f32],
    bs: usize,
    be: usize,
    col_p: usize,
    pel: usize,
    text_color: Color,
    preedit_color: Color,
    runs: &[editpad_core::StyledRun],
    clip: Rectangle,
) {
    if be <= bs {
        return;
    }
    let lh = core.line_height();
    let size = core.font_size();
    let mut paint = |lo: usize, hi: usize, color: Color| {
        let seg: String = s.chars().skip(lo).take(hi - lo).collect();
        if seg.is_empty() {
            return;
        }
        let bx = s_xs[bs.min(s_xs.len().saturating_sub(1))];
        let lo_x = s_xs[lo.min(s_xs.len().saturating_sub(1))];
        renderer.fill_text(
            core_text::Text {
                content: seg,
                bounds: Size::new(f32::INFINITY, lh),
                size: Pixels(size),
                line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                font,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: core_text::Shaping::Advanced,
                wrapping: core_text::Wrapping::None,
            },
            Point::new(x0 + (lo_x - bx), y),
            color,
            clip,
        );
    };
    // 前文块 [bs, col_p)
    if bs < col_p {
        let hi = be.min(col_p);
        if hi > bs {
            if runs.is_empty() {
                paint(bs, hi, text_color);
            } else {
                for run in runs {
                    let s0 = run.start_col.max(bs);
                    let e0 = run.end_col.min(hi);
                    if e0 > s0 {
                        paint(
                            s0,
                            e0,
                            Color::from_rgba8(
                                (run.color[0] * 255.0).round() as u8,
                                (run.color[1] * 255.0).round() as u8,
                                (run.color[2] * 255.0).round() as u8,
                                run.color[3],
                            ),
                        );
                    }
                }
            }
        }
    }
    // 组字块 [col_p, col_p + pel)
    let blo = bs.max(col_p);
    let bhi = be.min(col_p + pel);
    if bhi > blo {
        paint(blo, bhi, preedit_color);
    }
    // 后文块 [col_p + pel, ..)：源列 = 合成索引 − pel
    let blo = bs.max(col_p + pel);
    if be > blo {
        if runs.is_empty() {
            paint(blo, be, text_color);
        } else {
            for run in runs {
                let s0 = run.start_col.max(blo - pel);
                let e0 = run.end_col.min(be - pel);
                if e0 > s0 {
                    paint(
                        s0 + pel,
                        e0 + pel,
                        Color::from_rgba8(
                            (run.color[0] * 255.0).round() as u8,
                            (run.color[1] * 255.0).round() as u8,
                            (run.color[2] * 255.0).round() as u8,
                            run.color[3],
                        ),
                    );
                }
            }
        }
    }
}

/// 合成流断点表中包含索引 `idx` 的段序号（断点向量首项恒 0）。
fn reflow_seg_of(breaks: &[usize], idx: usize) -> Option<usize> {
    breaks
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &b)| b <= idx)
        .map(|(i, _)| i)
}

struct EditorView {
    core: EditorHandle,
    /// 本帧正文/行号栏使用的字形族（P34；默认 = [`BODY_FONT`]）。
    font: Font,
    /// P48：Ctrl+滚轮缩放的触控板累积器（Pixels 增量折算行数，满 1 行
    /// 发一步后清零；滚轮 Lines 增量不经过它）。
    zoom_accum: f32,
}

impl EditorView {
    /// P42：确保列宽与真实字形一致——首帧（或字体/字号变更后）用排版
    /// 段落实测 advance 并注入 EditorCore。按 (字体, 字号) 键去重：
    /// 同键不重测（无论成败，失败保持固定假设，不每帧空转）；
    /// P34 切字 / A±改字号（set_font_size 只做比例折算）换键重测校准。
    ///
    /// 调用点 = 控件 `layout`（每帧最先执行，早于任何 update 事件与
    /// draw），保证键盘/鼠标事件到达时 char_width 已是实测值。
    fn ensure_measured_char_width(&self) {
        let key = (self.font, self.core.borrow().font_size());
        {
            let core = self.core.borrow();
            if core.metric_key == Some(key) {
                return;
            }
        }
        // 先记键再量：即使本帧量度失败也不再重试（换字体/字号才换键）
        self.core.borrow_mut().metric_key = Some(key);
        if let Some(w) = measure_char_width(self.font, key.1) {
            self.core.borrow_mut().set_measured_char_width(w);
        }
        // P88/P89：同键顺带实测字形墨迹盒（上边距 + 墨迹高）——光标/
        // 选区纵向对齐基准（P89 起选区带按墨迹盒居中，需成对注入）。
        // 失败保持默认（顶 0 / 整行高 = 旧行为，恒安全）。
        if let Some((off, ink_h)) = measure_ink_box(self.font, key.1) {
            self.core.borrow_mut().set_ink_box(off, ink_h);
        }
    }

    /// 第 40 轮：可见行真实字形布局注入——对可见行做与正文绘制同源的
    /// shaping，把每个字符起点的真实像素 x 注入 EditorCore（先清后注，
    /// 无陈旧残留；行内容/字体/字号/滚动变化后下一帧自动对齐）。
    ///
    /// 成本：每帧对可见行（~50 行）各做一次段落 shaping（microsecond 级，
    /// 与既有绘制塑造同量级），换来的是一劳永逸的「光标/点击/选区与字形
    /// 零误差」——不再依赖任何静态列宽假设（主流编辑器的 DirectWrite
    /// glyph placement 同款思路）。
    fn refresh_row_layouts(&self) {
        let (font, size) = (self.font, self.core.borrow().font_size());
        let rows: Vec<(usize, Vec<f32>)> = {
            let core = self.core.borrow();
            let (first, last) = core.visible_range();
            let mut out = Vec::with_capacity((last - first + 1).min(512));
            for line in first..=last {
                let text = core.line_text(line);
                if let Some(xs) = shape_row_xs(font, size, &text) {
                    out.push((line, xs));
                }
            }
            out
        };
        let mut core = self.core.borrow_mut();
        core.clear_row_layouts();
        for (line, xs) in rows {
            core.set_row_layout(line, xs);
        }
    }
}

/// 浅色主题的固定配色（保持 v1 观感）；深色主题在 draw 时由 palette 派生。
const SELECTION_COLOR: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.25);
const CARET_COLOR: Color = Color::from_rgb8(0x11, 0x11, 0x11);
const GUTTER_BG: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
const GUTTER_TEXT: Color = Color::from_rgb8(0x99, 0x99, 0x99);
/// P115：组字串视觉与正文同色（用户点单：不再蓝字）——浅/深主题统一
/// 取 palette.text（见 EditorColors::resolve 两分支），下划线同色系
/// 0.6 透明度，与深色主题既有口径一致。
/// 书签圆点（第 60 轮）：琥珀色在浅灰行号栏与深色主题上都醒目，
/// 深浅主题共用一值（与选区/光标不同，它不承担「正文可读性」职能）。
const BOOKMARK_COLOR: Color = Color::from_rgb8(0xE0, 0x96, 0x2E);
/// 括号匹配下划线（第 61 轮）：浅色主题用与查找/预编辑同族的蓝，
/// 深色主题从前景派生（EditorColors::resolve）。
const BRACKET_LIGHT: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.85);

/// 一次 draw 用到的全部颜色（按当前主题解析）。
struct EditorColors {
    selection: Color,
    caret: Color,
    gutter_bg: Color,
    gutter_text: Color,
    preedit_text: Color,
    preedit_underline: Color,
    scrollbar_track: Color,
    scrollbar_thumb: Color,
    bookmark: Color,
    bracket: Color,
    /// 不可见字符标记（第 64 轮）：与选区同族的淡蓝（低透明度），
    /// 深浅主题都足够「隐」又不至于在白/黑底上消失。
    invisibles: Color,
}

impl EditorColors {
    /// 浅色：沿用固定值；深色：从 palette 派生
    /// （行号栏背景=背景提亮、行号/正文/光标/预编辑统一用 palette.text）。
    fn resolve(theme: &Theme) -> Self {
        let palette = theme.palette();
        // 前景比背景亮 → 视为深色主题（不依赖具体主题枚举，Custom 也适用）
        let dark = luminance(palette.text) > luminance(palette.background);
        if !dark {
            return Self {
                selection: SELECTION_COLOR,
                caret: CARET_COLOR,
                gutter_bg: GUTTER_BG,
                gutter_text: GUTTER_TEXT,
                // P115：组字串与正文同色（正文恒用 palette.text）
                preedit_text: palette.text,
                preedit_underline: Color { a: 0.6, ..palette.text },
                // 滚动条用前景色低透明度叠加，两种主题都自然成立
                scrollbar_track: Color::from_rgba8(0x00, 0x00, 0x00, 0.05),
                scrollbar_thumb: Color::from_rgba8(0x00, 0x00, 0x00, 0.30),
                bookmark: BOOKMARK_COLOR,
                bracket: BRACKET_LIGHT,
                // 与选区同族的淡蓝（更淡），像素对拍可复用蓝色判据
                invisibles: Color::from_rgba8(0x33, 0x66, 0xCC, 0.30),
            };
        }
        let text = palette.text;
        Self {
            selection: Color { a: 0.25, ..text },
            caret: text,
            gutter_bg: lighten(palette.background, 0.12),
            gutter_text: Color { a: 0.55, ..text },
            preedit_text: text,
            preedit_underline: Color { a: 0.6, ..text },
            scrollbar_track: Color { a: 0.06, ..palette.text },
            scrollbar_thumb: Color { a: 0.38, ..palette.text },
            bookmark: BOOKMARK_COLOR,
            bracket: Color { a: 0.85, ..text },
            invisibles: Color { a: 0.32, ..text },
        }
    }
}

impl Widget<crate::Message, Theme, iced::Renderer> for EditorView {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // P42：每帧最先校准列宽（实测 advance 注入，键相同即跳过）
        self.ensure_measured_char_width();
        // 第 40 轮：可见行真实字形布局注入——光标/点击/选区按字形位置定位。
        // 每帧重建（先清后注），无陈旧残留；行内容编辑后下一帧自动对齐。
        self.refresh_row_layouts();
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::INFINITE))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let core = self.core.borrow();
        // P34：本帧字形族来自构造入参（默认 = BODY_FONT）
        let body_font = self.font;
        let palette = theme.palette();
        let colors = EditorColors::resolve(theme);
        let lh = core.line_height();
        let char_w = core.char_width();
        // P13：横向滚动偏移——正文/选区/光标的文档系坐标统一扣减它，
        // 行号栏固定不动（与主流编辑器一致）
        let scroll_left = core.scroll_left;
        let gutter_w = core.gutter_width();

        // P59：滚动条测量提前（与绘制共用同一结果）。
        // 第 73 轮 ⑯：垂直行程按视觉行总数；软换行开态水平行程置 0
        // （hscroll 隐藏，needed 恒 false）
        let sb = VScrollbar::measure(
            core.scroll_content_lines(),
            core.viewport_h,
            lh,
            bounds.height,
            core.scroll_top,
        );
        let (hcontent_px, hview_px) = if core.wrap_enabled() {
            (0.0, (bounds.width - gutter_w).max(0.0))
        } else {
            // 关态 = 既有口径（列模型 ∪ 真实行宽）
            (
                core.content_width_px(),
                (bounds.width - gutter_w).max(0.0),
            )
        };
        let hsb = HScrollbar::measure(
            hcontent_px,
            hview_px,
            bounds.width,
            core.scroll_left,
        );

        // P99：软换行折行预算跟随垂直滚动条 needed——内容超出视口
        // （滚动条必然出现）时按滚动条可视带宽让位，折行文本在滑块
        // 左侧收尾，行尾字符不再被盖住/显得截断；放得下（无滚动条）
        // 时零预留全宽贴边（P95 口径保留）。稳定性双向自持：预算
        // 收缩只增视觉行数、「预留后仍 needed」与「取消后仍放得下」
        // 各自成立，不会逐帧翻转（见 core.rs set_wrap_sb_reserve
        // 注释）。翻转于本帧正文绘制前生效：预算变化经 WrapCache
        // 同步键整表重置，可见行在下方绘制循环内惰性重算（v1 已
        // 披露的收敛模型，滚动范围下一帧对齐）。
        let wrap_reserve = core.wrap_enabled() && sb.needed;
        drop(core);
        self.core.borrow_mut().set_wrap_sb_reserve(wrap_reserve);
        let core = self.core.borrow();

        // P66：三兄弟图层实现真裁剪。上游 Cached 文本分支用「声明的
        // 裁剪盒」冒充实际字形范围做 is_within 快路径（engine.rs）——层
        // 覆盖整控件时恒真、掩码被跳过，半可见行的字形会越界上屏。
        // 对策：quad 层用全尺寸 bounds（draw_quad 的层掩码可靠）；文本
        // 层四周内缩 TEXT_LAYER_INSET，迫使快路径判假、强制走掩码路径，
        // 越界字形被硬裁掉；光标/滚动条压顶层恢复全尺寸。层序 = 绘制序：
        // 选区(底) → 正文/行号 → 光标/预编辑下划线/滚动条(顶)。
        renderer.start_layer(bounds);

        // 背景与行号栏
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            palette.background,
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    width: gutter_w,
                    ..bounds
                },
                ..renderer::Quad::default()
            },
            colors.gutter_bg,
        );

        // 书签墨迹（第 60 轮）：左侧条带内的琥珀圆点，只为带书签的可见行
        // 画（is_bookmarked O(log n)/行）；在 A 层掩码内，半可见行的越界
        // 半圆被硬裁，与行号/正文同受控件边界约束
        // 第 73 轮 ⑯：软换行开态圆点锚定逻辑行**首段**的视觉行
        let (bk_first, bk_last) = core.visible_range();
        for line in bk_first..=bk_last {
            if !core.is_bookmarked(line) {
                continue;
            }
            let v = core.visual_row_of(line, 0);
            let y = bounds.y + (v as f32 - core.scroll_top) * lh;
            if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                continue;
            }
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: bounds.x + (BOOKMARK_STRIP - BOOKMARK_DOT) * 0.5,
                        y: y + (lh - BOOKMARK_DOT) * 0.5,
                        width: BOOKMARK_DOT,
                        height: BOOKMARK_DOT,
                    },
                    border: iced::Border {
                        radius: Radius::from(BOOKMARK_DOT * 0.5),
                        ..iced::Border::default()
                    },
                    ..renderer::Quad::default()
                },
                colors.bookmark,
            );
        }

        // 选区高亮：只画与视口相交的视觉行（双宽感知）。
        // 第 73 轮 ⑯：软换行开态逐视觉段画 [cs, ce) 子区间（跨段选区
        // 分段着色，段间空隙 = 折行边界天然不画）；关态走既有逻辑行路径
        if let Some((sel_start, sel_end)) = core.ordered_selection() {
            let last_line = core.doc.line_count().saturating_sub(1);
            for line in sel_start.line..=sel_end.line.min(last_line) {
                let text = core.line_text(line);
                let lens = text.chars().count();
                let start_col = if line == sel_start.line { sel_start.col } else { 0 };
                let end_col = if line == sel_end.line { sel_end.col } else { lens };
                if end_col <= start_col {
                    continue;
                }
                if core.wrap_enabled() {
                    let base = core.line_visual_base(line) as f32;
                    let breaks = core.segments_of_line(line, &text);
                    for (s, &seg_start) in breaks.iter().enumerate() {
                        let seg_end = breaks.get(s + 1).copied().unwrap_or(lens);
                        let cs = start_col.max(seg_start);
                        let ce = end_col.min(seg_end);
                        if ce <= cs {
                            continue;
                        }
                        let row = base + s as f32;
                        if row < core.scroll_top
                            || row > core.scroll_top + core.viewport_h / lh
                        {
                            continue;
                        }
                        // 段相对：续行从文本区左缘起排
                        let seg_base = core.px_of(line, &text, seg_start);
                        let x0 = core.px_of(line, &text, cs.min(lens)) - seg_base;
                        let x1 = core.px_of(line, &text, ce.min(lens)) - seg_base;
                        // P59：选区矩形与控件边界求交（quad 无任何裁剪）
                        // P88：y 下移字形墨迹上边距——行盒顶对齐会让选区
                        // 带顶悬在首行上方空带（用户截图「色带残影」）
                        // P89：改为按字形墨迹盒**居中**（带高仍 = 行盒高，
                        // 多行选区带带相接不断裂；P88 只顶对齐会让带底
                        // 悬出行盒下缘、单选字被顶在带顶不居中）
                        let Some(rect) = Rectangle {
                            x: bounds.x + gutter_w + x0 - scroll_left,
                            y: bounds.y + (row - core.scroll_top) * lh + core.decoration_inset(),
                            width: (x1 - x0).max(char_w),
                            height: lh,
                        }
                        .intersection(&bounds)
                        else {
                            continue;
                        };
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: rect,
                                ..renderer::Quad::default()
                            },
                            colors.selection,
                        );
                    }
                    continue;
                }
                let row = line as f32;
                if row < core.scroll_top || row > core.scroll_top + core.viewport_h / lh {
                    continue;
                }
                let x0 = core
                    .px_of(line, &text, start_col.min(lens));
                let x1 = core
                    .px_of(line, &text, end_col.min(lens));
                // P59：选区矩形与控件边界求交——部分可见行的高亮不再越界
                // （quad 无任何裁剪，越界部分会压标签条/状态栏）
                // P88/P89：与折行分支同款——y 从墨迹上边距改为按墨迹
                // 盒居中（带高仍 = 行盒高，单字选区字居中）
                let Some(rect) = Rectangle {
                    x: bounds.x + gutter_w + x0 - scroll_left,
                    y: bounds.y + (row - core.scroll_top) * lh + core.decoration_inset(),
                    width: (x1 - x0).max(char_w),
                    height: lh,
                }
                .intersection(&bounds)
                else {
                    continue;
                };
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        ..renderer::Quad::default()
                    },
                    colors.selection,
                );
            }
        }

        // 第 67 轮 ⑮：列块高亮——逐行画 [c0, c1) 段 quad，颜色与单选区
        // 同族（互斥状态不会同帧出现）。短行自动到行尾：x1 取该行实际
        // 列宽 min(c1) 的像素。P89：y 与选区同款按墨迹盒居中。
        if let Some((r0, r1, c0, c1)) = core.active_block() {
            for line in r0..=r1 {
                let y =
                    bounds.y + (line as f32 - core.scroll_top) * lh + core.decoration_inset();
                if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                    continue;
                }
                let text = core.line_text(line);
                let cols = text.chars().count();
                let x0 = core.px_of(line, &text, c0.min(cols));
                let x1 = core.px_of(line, &text, c1.min(cols).max(c0.min(cols)));
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + gutter_w + x0 - scroll_left,
                            y,
                            width: (x1 - x0).max(char_w * 0.4),
                            height: lh,
                        },
                        ..renderer::Quad::default()
                    },
                    colors.selection,
                );
            }
        }

        // 括号匹配高亮（第 61 轮）：光标邻接括号时，两侧括号各画一条
        // 2px 下划线（A 层 quad，随掩码裁剪；查询带光标键控缓存，
        // 命中帧零扫描）。括号恒 ASCII 单列，宽 = char_w。
        if let Some((boff, other)) = core.bracket_match() {
            for off in [boff, other] {
                let line = core.doc.char_to_line(off);
                let col = off - core.doc.line_to_char(line);
                let text = core.line_text(line);
                let x = core.px_of(line, &text, col);
                // 第 73 轮 ⑯：折行开态 y 经视觉行映射（括号可能落在非
                // 首段，画错行即画到别的逻辑行上——设计 §4.8），x 走
                // 段相对（续行左缘起排）
                let (v, seg_base) = if core.wrap_enabled() {
                    let lens = text.chars().count();
                    let breaks = core.segments_of_line(line, &text);
                    let seg = wrap_segment_index(&breaks, col.min(lens), lens);
                    let base = core.line_visual_base(line);
                    (base + seg as u32, core.px_of(line, &text, breaks[seg]))
                } else {
                    (line as u32, 0.0)
                };
                let y = bounds.y + (v as f32 - core.scroll_top) * lh;
                if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                    continue;
                }
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + gutter_w + (x - seg_base) - scroll_left,
                            y: y + lh - 2.0,
                            width: char_w.max(2.0),
                            height: 2.0,
                        },
                        ..renderer::Quad::default()
                    },
                    colors.bracket,
                );
            }
        }

        // A 层（quad）收口
        // 不可见字符覆盖标记（第 64 轮）：空格=字符格中央小点、制表符=
        // 格内短横、行尾=右端短竖标。纯 A 层 quad 叠加，不改文本布局与
        // 命中测试；x 优先取实测行布局 row_x（O(1)），该行布局未就绪则
        // 整行跳过（滞后一帧出现，可接受）。可见行外剔除与书签同款。
        // 第 73 轮 ⑯：软换行开态标记逐视觉段定位（字符所在段 = 视觉行）；
        // 行尾标只在逻辑行末段画（设计 §4.1）。关态整行单段，恒等退化。
        if core.show_whitespace || core.show_line_endings {
            let mark = colors.invisibles;
            // 空白标记绘制（闭包收纳 A 层 quad 分支，两态共用）
            let draw_ws_mark = |renderer: &mut iced::Renderer,
                                y: f32,
                                cx: f32,
                                ch: char,
                                adv: usize| {
                match ch {
                    ' ' => renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle {
                                x: cx + char_w * 0.5 - 1.0,
                                y: y + lh * 0.62,
                                width: 2.0,
                                height: 2.0,
                            },
                            ..renderer::Quad::default()
                        },
                        mark,
                    ),
                    '\t' => {
                        let w = (adv as f32 * char_w * 0.6).max(3.0);
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle {
                                    x: cx + char_w * 0.3,
                                    y: y + lh * 0.55,
                                    width: w,
                                    height: 1.5,
                                },
                                ..renderer::Quad::default()
                            },
                            mark,
                        )
                    }
                    _ => {}
                }
            };
            let (iv_first, iv_last) = core.visible_range();
            for line in iv_first..=iv_last {
                let text = core.line_text(line);
                let lens = text.chars().count();
                if core.wrap_enabled() {
                    let breaks = core.segments_of_line(line, &text);
                    let base = core.line_visual_base(line);
                    // 最后可见段（视口下缘内的段序上限，供空白标记截断）
                    let max_vis_seg = {
                        let mut m = 0usize;
                        for (s, _) in breaks.iter().enumerate() {
                            // ⚠️ `<` 必须与左操作数同行（换行会被解析器
                            // 当成 f32 的泛型参数开始）
                            if ((base + s as u32) as f32) < (core.scroll_top + core.viewport_h / lh + 1.0) {
                                m = s;
                            } else {
                                break;
                            }
                        }
                        m
                    };
                    if core.show_line_endings {
                        // 行尾短竖标只在逻辑行末段画
                        let last_seg = breaks.len() - 1;
                        if last_seg <= max_vis_seg {
                            let v = base + last_seg as u32;
                            let y = bounds.y + (v as f32 - core.scroll_top) * lh;
                            if y + lh > bounds.y && y < bounds.y + bounds.height {
                                let cols = core.line_display_len(line);
                                if let Some(x) = core.row_x(line, cols) {
                                    // 段相对：x 减末段起点像素（续行左缘）
                                    let x_rel =
                                        x - core.px_of(line, &text, breaks[last_seg]);
                                    renderer.fill_quad(
                                        renderer::Quad {
                                            bounds: Rectangle {
                                                x: bounds.x + gutter_w + x_rel
                                                    - scroll_left
                                                    + char_w * 0.25,
                                                y: y + lh * 0.25,
                                                width: 2.0,
                                                height: lh * 0.45,
                                            },
                                            ..renderer::Quad::default()
                                        },
                                        mark,
                                    );
                                }
                            }
                        }
                    }
                    if core.show_whitespace {
                        let mut col = 0usize;
                        for ch in text.chars() {
                            if ch == '\n' || ch == '\r' {
                                break;
                            }
                            let adv = char_cols(ch, col) as usize;
                            // 字符所在段 → 其视觉行（段序随 col 非降，
                            // 越过最后可见段的字符直接截断）
                            let seg = wrap_segment_index(&breaks, col, lens);
                            if seg > max_vis_seg {
                                break;
                            }
                            let y = bounds.y
                                + ((base + seg as u32) as f32 - core.scroll_top) * lh;
                            if y + lh > bounds.y && y < bounds.y + bounds.height {
                                if let Some(x) = core.row_x(line, col) {
                                    // 段相对：续行字符标记从段起点起排
                                    let cx = bounds.x + gutter_w
                                        + x
                                        - core.px_of(line, &text, breaks[seg])
                                        - scroll_left;
                                    draw_ws_mark(renderer, y, cx, ch, adv);
                                }
                            }
                            col += adv;
                        }
                    }
                    continue;
                }
                let y = bounds.y + (line as f32 - core.scroll_top) * lh;
                if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                    continue;
                }
                if core.show_whitespace {
                    let mut col = 0usize;
                    for ch in text.chars() {
                        if ch == '\n' || ch == '\r' {
                            break;
                        }
                        let adv = char_cols(ch, col) as usize;
                        if let Some(x) = core.row_x(line, col) {
                            let cx = bounds.x + gutter_w + x - scroll_left;
                            draw_ws_mark(renderer, y, cx, ch, adv);
                        }
                        col += adv;
                    }
                }
                if core.show_line_endings {
                    // 行尾短竖标：行内容右端再让出四分之一格
                    let cols = core.line_display_len(line);
                    if let Some(x) = core.row_x(line, cols) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle {
                                    x: bounds.x + gutter_w + x - scroll_left
                                        + char_w * 0.25,
                                    y: y + lh * 0.25,
                                    width: 2.0,
                                    height: lh * 0.45,
                                },
                                ..renderer::Quad::default()
                            },
                            mark,
                        );
                    }
                }
            }
        }
        renderer.end_layer();

        // B 层：文本专用，四周内缩 TEXT_LAYER_INSET——层边界严格小于
        // 传入的 clip 参数，上游 is_within 快路径必然判假、掩码强制生效，
        // 半可见行越出控件边界的字形被硬裁（P66 行号随滚动的根基）
        let text_layer = Rectangle {
            x: bounds.x + TEXT_LAYER_INSET,
            y: bounds.y + TEXT_LAYER_INSET,
            width: (bounds.width - 2.0 * TEXT_LAYER_INSET).max(0.0),
            height: (bounds.height - 2.0 * TEXT_LAYER_INSET).max(0.0),
        };
        renderer.start_layer(text_layer);

        // 文本与行号：只为可见行调用排版（虚拟化的核心）；启用高亮时按语法分色
        // 第 73 轮 ⑯：软换行开态改逐**视觉行**（折行段独立 shape，
        // P46 的 INFINITY bounds 对段依然成立，且段长天然 ≤ 视口，
        // 「超宽行 shaping 封顶」问题消失）；关态走既有逐逻辑行路径
        // （恒等退化，由既有像素批守护）
        // P115：组字串作为「虚拟插入文本」参与行绘制（前文+组字+后文
        // 三段式）——行中组字时后文被推开、光标跟到组字尾（用户复报
        // 组字与已有文字重叠/光标不前进）。display_right_edge：
        // 开态 = 折行边界（P114 让位后预算），关态 = 控件右缘。
        let preedit_text: Option<String> = core.preedit.clone().filter(|s| !s.is_empty());
        let display_right_edge = if core.wrap_enabled() {
            bounds.x + gutter_w + core.wrap_max_px() - core.scroll_left
        } else {
            bounds.x + bounds.width
        };
        let preedit_clip = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: (display_right_edge - bounds.x).max(0.0),
            height: bounds.height,
        };
        let text_x0 = bounds.x + gutter_w - scroll_left;
        // P115 续：组字行「插入重排」预计算（仅折行开态）——合成串
        // （前文+组字+后文）按真实字形宽与折行预算重断行，被挤出的
        // 后文**换行**到下段（修前在折行边界截断 → 行尾大片空白，用户
        // 复报）；k = 该行视觉段数增量，后续逻辑行绘制整体下移；shape
        // 失败回退下方旧三段式/空行逻辑（pre_slot 分支仍在）；关态
        // 不重排（右移+可滚动，既有口径）
        let reflow: Option<ReflowLayout> = if core.wrap_enabled() {
            preedit_text.as_deref().and_then(|p| {
                let rl = core.cursor.line;
                if rl >= core.doc.line_count() {
                    return None;
                }
                let rtext = core.line_text(rl);
                let rlens = rtext.chars().count();
                let col_p = core.cursor.col.min(rlens);
                let pel = p.chars().count();
                let s: String = rtext
                    .chars()
                    .take(col_p)
                    .chain(p.chars())
                    .chain(rtext.chars().skip(col_p))
                    .collect();
                if s.is_empty() {
                    return None;
                }
                let s_xs = shape_row_xs(body_font, core.font_size(), &s)?;
                if s_xs.len() < 2 {
                    return None;
                }
                let budget = (display_right_edge - text_x0).max(1.0);
                let breaks = pixel_breaks(&s_xs, budget, &s);
                let old_segs = core.segments_of_line(rl, &rtext).len() as isize;
                let k = (breaks.len() as isize - old_segs).max(0);
                Some(ReflowLayout {
                    line: rl,
                    col_p,
                    pel,
                    s,
                    s_xs,
                    breaks,
                    k,
                    v0: core.line_visual_base(rl),
                })
            })
        } else {
            None
        };
        if core.wrap_enabled() {
            let total = core.visual_rows_total();
            if total > 0 {
                let first_v = (core.scroll_top.floor() as i64).max(0) as u32;
                let rows_v = (core.viewport_h / lh).ceil() as u32 + 1;
                // 组字行重排多出的段数也纳入可见范围（后续行下移 k）
                let k_vis = reflow.as_ref().map_or(0, |r| r.k.max(0) as u32);
                let last_v = first_v
                    .saturating_add(rows_v)
                    .saturating_add(k_vis)
                    .min(total - 1);
                for v in first_v..=last_v {
                    let (line, seg, seg_start, seg_end) = core.locate_visual(v);
                    // 后续逻辑行整体下移：该行原首段之前的一切（含组字
                    // 行本身的旧表段）不动；组字行由首段全量重排绘制
                    let y_off = reflow
                        .as_ref()
                        .map_or(0.0f32, |r| if line > r.line { r.k as f32 } else { 0.0 });
                    let y = bounds.y + (v as f32 + y_off - core.scroll_top) * lh;
                    if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                        continue;
                    }
                    // 行号数字：仅逻辑行首段（设计 §4.1）。算法复刻关态
                    // （左缘 + Default 对齐，P66附 口径；数字仍右对齐于
                    // 行号栏右缘 − GUTTER_MIN）
                    if seg == 0 {
                        let num = (line + 1).to_string();
                        let num_w = num.chars().count() as f32 * char_w * GUTTER_FONT_SCALE;
                        let num_x = bounds.x + gutter_w - GUTTER_MIN - num_w;
                        renderer.fill_text(
                            core_text::Text {
                                content: num,
                                bounds: Size::new(num_w, lh),
                                size: Pixels(core.font_size() * GUTTER_FONT_SCALE),
                                line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                                font: body_font,
                                align_x: core_text::Alignment::Default,
                                align_y: alignment::Vertical::Top,
                                shaping: core_text::Shaping::Basic,
                                wrapping: core_text::Wrapping::None,
                            },
                            Point::new(num_x, y),
                            colors.gutter_text,
                            bounds,
                        );
                    }
                    // P115 续：组字行（含空行）由首段全量重排绘制——
                    // 段 0 覆盖全部新段（掩码裁视口外），其余旧表段跳过
                    if reflow.as_ref().is_some_and(|r| r.line == line) {
                        if seg == 0 {
                            let rtext = core.line_text(line);
                            let rruns = core.highlight_runs(line, &rtext);
                            if let Some(r) = &reflow {
                                for (bi, &bs) in r.breaks.iter().enumerate() {
                                    let be = r
                                        .breaks
                                        .get(bi + 1)
                                        .copied()
                                        .unwrap_or(r.s.chars().count());
                                    if be <= bs {
                                        continue;
                                    }
                                    let yv = bounds.y
                                        + (r.v0 as f32 + bi as f32 - core.scroll_top) * lh;
                                    if yv + lh <= bounds.y
                                        || yv >= bounds.y + bounds.height
                                    {
                                        continue;
                                    }
                                    paint_composed_segment(
                                        renderer, &core, body_font, text_x0, yv, &r.s,
                                        &r.s_xs, bs, be, r.col_p, r.pel, palette.text,
                                        colors.preedit_text, &rruns, bounds,
                                    );
                                }
                            }
                        }
                        continue;
                    }
                    let text = core.line_text(line);
                    let lens = text.chars().count();
                    // P115 勘误：插槽判断须在空行检查**之前**——空行
                    // （新文档/空白行输入）与行尾组字是老浮层实现本可
                    // 显示、三段式嵌入行绘制后会被整行跳过（用户复报
                    // 「组字直接没了」）；空行只有段 0 且无正文
                    let pre_slot: Option<(&str, usize, f32)> =
                        preedit_text.as_deref().and_then(|p| {
                            (core.cursor.line == line && core.cursor.col <= lens).then(|| {
                                (
                                    p,
                                    core.cursor.col,
                                    measure_preedit_w(body_font, core.font_size(), p),
                                )
                            })
                        });
                    if seg_start >= lens {
                        // 空行/幻影行：仅首段且无正文——组字画在段首，
                        // 可显示宽 = 整段预算（子层硬裁到折行边界）
                        if let Some((p, _, w)) = pre_slot {
                            let zone = Rectangle {
                                x: text_x0,
                                y: bounds.y,
                                width: (display_right_edge - text_x0).max(0.0),
                                height: bounds.height,
                            };
                            renderer.start_layer(zone);
                            if w > 0.0 {
                                renderer.fill_text(
                                    core_text::Text {
                                        content: p.to_owned(),
                                        bounds: Size::new(f32::INFINITY, lh),
                                        size: Pixels(core.font_size()),
                                        line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                                        font: body_font,
                                        align_x: core_text::Alignment::Default,
                                        align_y: alignment::Vertical::Top,
                                        shaping: core_text::Shaping::Advanced,
                                        wrapping: core_text::Wrapping::None,
                                    },
                                    Point::new(text_x0, y),
                                    colors.preedit_text,
                                    bounds,
                                );
                            }
                            renderer.end_layer();
                        }
                        continue;
                    }
                    let runs = core.highlight_runs(line, &text);
                    if let Some((p, col_p, w)) = pre_slot {
                        // 段内命中：col_p 在本段 [seg_start, seg_end)；行尾
                        // （末段 col==seg_end==lens）也在本段画——中间段尾
                        // col==seg_end 由下一段段首处理（避免双画）
                        let hit = seg_start <= col_p
                            && (col_p < seg_end || (col_p == seg_end && seg_end == lens));
                        if hit {
                            let rel = core.px_of(line, &text, col_p)
                                - core.px_of(line, &text, seg_start);
                            // 段尾可用 = 折行预算 − 段内起点（段末字符右缘
                            // ≤ 预算不贴满，P96；空行/段尾整宽按预算计）
                            let remain = (display_right_edge - text_x0 - rel).max(0.0);
                            let vis = w.min(remain);
                            paint_text_slice(
                                renderer, &core, body_font, text_x0, y, line, seg_start, &text,
                                seg_start, col_p, 0.0, palette.text, &runs, bounds,
                            );
                            // 组字 + 被挤出的后文放进「组字插入区」子层：
                            // fill_text 的 clip 参数受 iced 文本缓存「首次
                            // 绘制锁定」不可依赖（同内容先前以控件矩形缓存
                            // 后，此处的窄 clip 不生效）——改用层掩码硬裁，
                            // 右缘 = 折行边界（P115：后文被推到边界处截断，
                            // 不再画进滚动条槽位/控件右缘）
                            let zone = Rectangle {
                                x: text_x0 + rel,
                                y: bounds.y,
                                width: (display_right_edge - text_x0 - rel).max(0.0),
                                height: bounds.height,
                            };
                            renderer.start_layer(zone);
                            if vis > 0.0 {
                                renderer.fill_text(
                                    core_text::Text {
                                        content: p.to_owned(),
                                        bounds: Size::new(f32::INFINITY, lh),
                                        size: Pixels(core.font_size()),
                                        line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                                        font: body_font,
                                        align_x: core_text::Alignment::Default,
                                        align_y: alignment::Vertical::Top,
                                        shaping: core_text::Shaping::Advanced,
                                        wrapping: core_text::Wrapping::None,
                                    },
                                    Point::new(text_x0 + rel, y),
                                    colors.preedit_text,
                                    bounds,
                                );
                            }
                            paint_text_slice(
                                renderer, &core, body_font, text_x0, y, line, seg_start, &text,
                                col_p, seg_end, rel + vis, palette.text, &runs, bounds,
                            );
                            renderer.end_layer();
                            continue;
                        }
                    }
                    paint_text_slice(
                        renderer, &core, body_font, text_x0, y, line, seg_start, &text,
                        seg_start, seg_end, 0.0, palette.text, &runs, bounds,
                    );
                }
            }
        } else {
        let (first, last) = core.visible_range();
        for line in first..=last {
            let y = bounds.y + (line as f32 - core.scroll_top) * lh;
            // P66：只跳过完全在视口外的行；上下缘的半可见行照常绘制，
            // 越界部分由图层掩码裁掉（P59 时代的「不完整行跳过」退役——
            // 那是整行对齐的前提，也是行号钉死的共谋）
            if y + lh <= bounds.y || y >= bounds.y + bounds.height {
                continue;
            }

            // 行号数字：P66附 改「计算左缘 + Default 对齐」。上游把 Cached
            // 文本的损伤矩形存为 Rectangle::new(position, size)——Right 对齐
            // 时 position 是右缘、矩形向右展开，而字形实际向左展开 → 损伤区
            // 永远错位到字形右侧空白带，部分重绘时数字不被重绘（用户截图：
            // 滚动后行号滞后一帧/序号重复，内容却总是新鲜——Default 对齐的
            // 内容矩形方向正确）。数字是 ASCII 等宽（P42 实测 char_w，行号
            // 字号按 GUTTER_FONT_SCALE 线性折算），左缘可精确计算：
            // num_x + num_w = 行号栏右缘 − GUTTER_MIN，视觉仍是右对齐。
            let num = (line + 1).to_string();
            let num_w = num.chars().count() as f32 * char_w * GUTTER_FONT_SCALE;
            let num_x = bounds.x + gutter_w - GUTTER_MIN - num_w;
            renderer.fill_text(
                core_text::Text {
                    content: num,
                    bounds: Size::new(num_w, lh),
                    size: Pixels(core.font_size() * GUTTER_FONT_SCALE),
                    line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                    font: body_font,
                    align_x: core_text::Alignment::Default,
                    align_y: alignment::Vertical::Top,
                    shaping: core_text::Shaping::Basic,
                    wrapping: core_text::Wrapping::None,
                },
                Point::new(num_x, y),
                colors.gutter_text,
                bounds,
            );

            let text = core.line_text(line);
            let lens = text.chars().count();
            // P115 勘误：插槽判断先在空行检查前（空行组字须画，见开态
            // 同款注释——新文档/空白行输入是老浮层的常见场景）
            let pre_slot: Option<(&str, usize, f32)> =
                preedit_text.as_deref().and_then(|p| {
                    (core.cursor.line == line && core.cursor.col <= lens).then(|| {
                        (
                            p,
                            core.cursor.col,
                            measure_preedit_w(body_font, core.font_size(), p),
                        )
                    })
                });
            if text.is_empty() {
                // 空行：组字画在行首（col 必 0；clip = 控件右缘，关态
                // 无折行边界，超视口部分由掩码硬裁、可水平滚动）
                if let Some((p, _, w)) = pre_slot {
                    if w > 0.0 {
                        renderer.fill_text(
                            core_text::Text {
                                content: p.to_owned(),
                                bounds: Size::new(f32::INFINITY, lh),
                                size: Pixels(core.font_size()),
                                line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                                font: body_font,
                                align_x: core_text::Alignment::Default,
                                align_y: alignment::Vertical::Top,
                                shaping: core_text::Shaping::Advanced,
                                wrapping: core_text::Wrapping::None,
                            },
                            Point::new(text_x0, y),
                            colors.preedit_text,
                            preedit_clip,
                        );
                    }
                }
                continue;
            }
            let runs = core.highlight_runs(line, &text);
            // P115：本行含组字插入点 → 三段式（关态无折行约束，后文整体
            // 右移组字实测宽；超视口部分由 B 层掩码硬裁，可水平滚动查看）
            if let Some((p, col_p, w)) = pre_slot {
                let rel = core.px_of(line, &text, col_p);
                paint_text_slice(
                    renderer, &core, body_font, text_x0, y, line, 0, &text, 0, col_p, 0.0,
                    palette.text, &runs, bounds,
                );
                renderer.fill_text(
                    core_text::Text {
                        content: p.to_owned(),
                        bounds: Size::new(f32::INFINITY, lh),
                        size: Pixels(core.font_size()),
                        line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                        font: body_font,
                        align_x: core_text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: core_text::Shaping::Advanced,
                        wrapping: core_text::Wrapping::None,
                    },
                    Point::new(text_x0 + rel, y),
                    colors.preedit_text,
                    preedit_clip,
                );
                paint_text_slice(
                    renderer, &core, body_font, text_x0, y, line, 0, &text, col_p, lens,
                    rel + w, palette.text, &runs, preedit_clip,
                );
                continue;
            }
            paint_text_slice(
                renderer, &core, body_font, text_x0, y, line, 0, &text, 0, lens, 0.0,
                palette.text, &runs, bounds,
            );
        }
        }

        // 组字串文字已并入 B 层正文三段式绘制（P115）：前文 + 组字（正文
        // 同色，y = 行盒顶基准，clip 到折行边界）+ 后文右移组字可显示
        // 宽。下划线归 C 层压顶，宽度与组字**可显示宽**一致（P115：
        // 开态到段尾为止，与正文占位同口径）。

        // B 层（文本）收口
        renderer.end_layer();

        // C 层：压顶四边形——预编辑下划线、光标竖线、滚动条
        renderer.start_layer(bounds);

        // 输入法下划线：与组字占位同门控同口径——默认 = 原段口径（caret.x +
        // preedit_visual_w 可显示宽）；组字重排存在时按**合成流**定位：
        // 组字起点所在段内 x、宽 = 段内可显示部分、y = 该段行盒底
        if let Some(preedit) = preedit_text.as_deref() {
            let w = measure_preedit_w(body_font, core.font_size(), preedit);
            if let Some(r) = &reflow {
                let s0 = r.col_p;
                let s1 = (r.col_p + r.pel).min(r.s.chars().count());
                if let Some(bi) = reflow_seg_of(&r.breaks, s0) {
                    let seg_end = r
                        .breaks
                        .get(bi + 1)
                        .copied()
                        .unwrap_or(r.s.chars().count());
                    let x = text_x0 + (r.s_xs[s0] - r.s_xs[r.breaks[bi]]);
                    let vis = (r.s_xs[s1.min(seg_end)] - r.s_xs[s0]).max(0.0);
                    let y = bounds.y + (r.v0 as f32 + bi as f32 - core.scroll_top) * lh;
                    if vis > 0.0 && y + lh > bounds.y && y < bounds.y + bounds.height {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle {
                                    x,
                                    y: y + lh - 3.0,
                                    width: vis,
                                    height: 2.0,
                                },
                                ..renderer::Quad::default()
                            },
                            colors.preedit_underline,
                        );
                    }
                }
            } else {
                let caret = core.caret_rect_relative();
                let row_top_y = caret.y - core.ink_offset;
                let in_view =
                    caret.y + lh > 0.0 && caret.y < core.viewport_h;
                if in_view {
                    let vis = core.preedit_visual_w(core.cursor.col, w);
                    if vis > 0.0 {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle {
                                    x: bounds.x + caret.x,
                                    y: bounds.y + row_top_y + lh - 3.0,
                                    width: vis,
                                    height: 2.0,
                                },
                                ..renderer::Quad::default()
                            },
                            colors.preedit_underline,
                        );
                    }
                }
            }
        }

        // 光标竖线（静止期按闪烁相位隐现；活动窗口期内常显）。
        // P59：光标行不在可视范围（滚轮滚走）时不绘制，杜绝越界墨迹；
        // P66：半可见行的光标也绘制，与正文行同规则
        // P115：组字中光标画在组字**可显示尾**（后文右移起点）——
        // 与主流编辑器「光标随组字前进」观感一致；重排存在时按合成流
        // 组字尾所在段定位（可能随重排折到下一段）
        let caret = core.caret_rect_relative();
        let caret_in_view = caret.y + caret.height > 0.0 && caret.y < core.viewport_h;
        if core.caret_visible() && caret_in_view {
            let (cx, cy) = if let Some(r) = &reflow {
                let s1 = (r.col_p + r.pel).min(r.s.chars().count());
                match reflow_seg_of(&r.breaks, s1) {
                    Some(bj) => {
                        let x = text_x0 + (r.s_xs[s1] - r.s_xs[r.breaks[bj]]);
                        let y = bounds.y
                            + (r.v0 as f32 + bj as f32 - core.scroll_top) * lh
                            + core.ink_offset;
                        (x, y)
                    }
                    None => (bounds.x + caret.x + 0.0, bounds.y + caret.y),
                }
            } else {
                let pre_dx = preedit_text.as_deref().map_or(0.0f32, |p| {
                    let w = measure_preedit_w(body_font, core.font_size(), p);
                    core.preedit_visual_w(core.cursor.col, w)
                });
                (bounds.x + caret.x + pre_dx, bounds.y + caret.y)
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: cx,
                        y: cy,
                        width: caret.width,
                        height: caret.height,
                    },
                    ..renderer::Quad::default()
                },
                colors.caret,
            );
        }

        // 垂直滚动条：内容超出视口才绘制（覆盖在正文右缘之上）。
        // P53：按活动淡入淡出——闲置滑块自动隐藏，不再常驻遮挡行尾；
        // alpha≈0 时整条跳绘（含命中门控，隐藏即不可点）
        if sb.needed {
            let sb_alpha = core.scrollbar_visibility();
            if sb_alpha > 0.004 {
                let track_color = Color {
                    a: colors.scrollbar_track.a * sb_alpha,
                    ..colors.scrollbar_track
                };
                let thumb_color = Color {
                    a: colors.scrollbar_thumb.a * sb_alpha,
                    ..colors.scrollbar_thumb
                };
                let track_rect = Rectangle {
                    x: bounds.x + bounds.width - SCROLLBAR_EDGE_INSET - SCROLLBAR_WIDTH,
                    y: bounds.y + sb.track_y,
                    width: SCROLLBAR_WIDTH,
                    height: sb.track_h,
                };
                let mut track_quad = renderer::Quad::default();
                track_quad.bounds = track_rect;
                track_quad.border.radius = Radius::from(SCROLLBAR_WIDTH / 2.0);
                renderer.fill_quad(track_quad, track_color);

                let thumb = sb.thumb_rect(bounds.width);
                let mut thumb_quad = renderer::Quad::default();
                thumb_quad.bounds = Rectangle {
                    x: bounds.x + thumb.x,
                    y: bounds.y + thumb.y,
                    ..thumb
                };
                thumb_quad.border.radius = Radius::from(SCROLLBAR_WIDTH / 2.0);
                renderer.fill_quad(thumb_quad, thumb_color);
            }
        }

        // 水平滚动条（P13）：内容超宽才绘制（覆盖在正文下缘之上）。
        // P45：行程统一走「列模型 ∪ 真实行宽」口径，与滚动钳制一致。
        // P54：与竖直条共用活动戳，同款淡入淡出（横向滚动点亮，闲置淡出）。
        // P59：测量已提前到裁剪层之前。
        if hsb.needed {
            let sb_alpha = core.scrollbar_visibility();
            if sb_alpha > 0.004 {
                let track_color = Color {
                    a: colors.scrollbar_track.a * sb_alpha,
                    ..colors.scrollbar_track
                };
                let thumb_color = Color {
                    a: colors.scrollbar_thumb.a * sb_alpha,
                    ..colors.scrollbar_thumb
                };
                let track_rect = Rectangle {
                    x: bounds.x + hsb.track_x,
                    y: bounds.y + bounds.height - SCROLLBAR_EDGE_INSET - SCROLLBAR_THUMB_THICKNESS,
                    width: hsb.track_w,
                    height: SCROLLBAR_THUMB_THICKNESS,
                };
                let mut track_quad = renderer::Quad::default();
                track_quad.bounds = track_rect;
                track_quad.border.radius = Radius::from(SCROLLBAR_THUMB_THICKNESS / 2.0);
                renderer.fill_quad(track_quad, track_color);

                let thumb = hsb.thumb_rect(bounds.height);
                let mut thumb_quad = renderer::Quad::default();
                thumb_quad.bounds = Rectangle {
                    x: bounds.x + thumb.x,
                    y: bounds.y + thumb.y,
                    ..thumb
                };
                thumb_quad.border.radius = Radius::from(SCROLLBAR_THUMB_THICKNESS / 2.0);
                renderer.fill_quad(thumb_quad, thumb_color);
            }
        }

        // C 层（压顶 quad）收口
        renderer.end_layer();
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, crate::Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // 输入法常开：winit 在 Windows 上默认禁用 IME。
        // 每次事件都续约请求（运行时在下一帧 RedrawRequested 时消费），
        // 候选框始终跟随光标；预编辑串由本控件内联绘制，故不传给系统浮窗。
        {
            let core = self.core.borrow();
            let caret = core.caret_rect_relative();
            let ime: input_method::InputMethod = input_method::InputMethod::Enabled {
                cursor: Rectangle {
                    x: bounds.x + caret.x,
                    y: bounds.y + caret.y,
                    ..caret
                },
                purpose: input_method::Purpose::Normal,
                preedit: None,
            };
            shell.request_input_method(&ime);
        }

        match event {
            // 每帧把真实视口尺寸同步给核心（可见行数/滚动夹紧依赖它）
            iced::Event::Window(window::Event::RedrawRequested(_)) => {
                {
                    let mut core = self.core.borrow_mut();
                    core.set_viewport_height(bounds.height);
                    core.set_viewport_width(bounds.width);
                    // P12：可见区缺档超内联预算 → 通知应用层安排后台分批补建。
                    // 重复发布无害（应用层对同代在途任务幂等跳过）；铺建推进/
                    // 编辑换代后由下一帧重新评估，无需额外状态。
                    if core.needs_paving() {
                        shell.publish(crate::Message::HighlightPaveNeeded);
                    }
                }
            }

            // 跟踪修饰键（P13）：滚轮事件不携带修饰键，
            // Shift+滚轮横向滚动只能靠这里维护的最近状态
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                self.core.borrow_mut().mods = *mods;
            }

            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // P2 焦点裁决：点在编辑区内=接管焦点；点在区外（查找框/工具栏）
                // =交出焦点。必须在越界 early-return 之前完成，否则收不到区外点击。
                let inside = cursor.position_over(bounds).is_some();
                self.core.borrow_mut().pointer_focus(inside);
                let Some(pos) = cursor.position_over(bounds) else {
                    return;
                };

                // 滚动条优先于文本命中：落在交互区则进入拖拽/轨道跳转，
                // 不触发文本选区。垂直条优先判定，右下角归属垂直条。
                // P53：淡出隐藏中的竖直条不拦截点击（点击落到正文）——
                // 悬停会先点亮它（见 CursorMoved），点之前必然可见。
                {
                    let core = self.core.borrow();
                    let sb = VScrollbar::measure(
                        core.scroll_content_lines(),
                        core.viewport_h,
                        core.line_height(),
                        bounds.height,
                        core.scroll_top,
                    );
                    let (local_x, local_y) = (pos.x - bounds.x, pos.y - bounds.y);
                    if sb.hits(local_x, local_y, bounds.width)
                        && core.scrollbar_visibility() > 0.05
                    {
                        drop(core);
                        let mut core = self.core.borrow_mut();
                        core.touch_scrollbar_activity();
                        if local_y >= sb.thumb_y && local_y <= sb.thumb_y + sb.thumb_h {
                            core.scrollbar_grab = Some(local_y - sb.thumb_y);
                        } else {
                            core.scroll_top = sb.scroll_for_track_click(local_y);
                            core.clamp_scroll();
                            core.scrollbar_grab = Some(sb.thumb_h * 0.5);
                        }
                        core.dragging = false; // 绝不因此进入文本拖选
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }

                    // 水平滚动条（P13）：下缘窄带，交互语义与垂直条对称。
                    // P45：行程口径与钳制一致（列模型 ∪ 真实行宽）。
                    // P54：淡出隐藏中同样不拦截点击（与垂直条同款门控）。
                    let hsb = HScrollbar::measure(
                        core.hscroll_content_px(),
                        (bounds.width - core.gutter_width()).max(0.0),
                        bounds.width,
                        core.scroll_left,
                    );
                    if hsb.hits(local_x, local_y, bounds.height)
                        && core.scrollbar_visibility() > 0.05
                    {
                        drop(core);
                        let mut core = self.core.borrow_mut();
                        core.touch_scrollbar_activity();
                        if local_x >= hsb.thumb_x && local_x <= hsb.thumb_x + hsb.thumb_w {
                            core.hscrollbar_grab = Some(local_x - hsb.thumb_x);
                        } else {
                            core.scroll_left = hsb.scroll_for_track_click(local_x);
                            core.clamp_scroll_horizontal();
                            core.hscrollbar_grab = Some(hsb.thumb_w * 0.5);
                        }
                        core.dragging = false;
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }

                    // 未命中任何滚动条：走普通文本按下流程
                    drop(core);
                    {
                        let mut core = self.core.borrow_mut();
                        let hit = core.hit_test(pos.x - bounds.x, pos.y - bounds.y);
                        // 第 67 轮 ⑮：Alt+Shift+按下 = 列块拖拽建块
                        if core.mods.contains(iced_mods_alt_shift()) {
                            core.begin_block_select(hit);
                            shell.publish(crate::Message::EditorNavChanged);
                            shell.request_redraw();
                            shell.capture_event();
                            return;
                        }
                        // 普通点击退出块态（与单选区互斥）
                        core.clear_block();
                        core.dragging = true;
                        core.anchor = None;
                        core.cursor = hit;
                        core.break_typing(); // P37：点击落点打断组
                        core.clear_vertical_goal(); // 第 73 轮 ⑯：点击 = 非竖向操作
                    }
                    shell.publish(crate::Message::EditorNavChanged);
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(pos) = cursor.position_over(bounds) else {
                    return;
                };
                let mut core = self.core.borrow_mut();

                // P53：悬停进入竖直滚动条命中区 → 提前点亮（淡出隐藏时
                // 先可见再可点）；拖拽中随移动续期，条不中途消失。
                // P54：水平条同款（命中区在下缘窄带）。
                {
                    let sb = VScrollbar::measure(
                        core.scroll_content_lines(),
                        core.viewport_h,
                        core.line_height(),
                        bounds.height,
                        core.scroll_top,
                    );
                    let hsb = HScrollbar::measure(
                        core.hscroll_content_px(),
                        (bounds.width - core.gutter_width()).max(0.0),
                        bounds.width,
                        core.scroll_left,
                    );
                    let (local_x, local_y) = (pos.x - bounds.x, pos.y - bounds.y);
                    if sb.hits(local_x, local_y, bounds.width)
                        || hsb.hits(local_x, local_y, bounds.height)
                        || core.scrollbar_grab.is_some()
                        || core.hscrollbar_grab.is_some()
                    {
                        core.touch_scrollbar_activity();
                    }
                }

                // 垂直滚动条拖拽中：按抓取偏移反解 scroll_top
                if let Some(grab) = core.scrollbar_grab {
                    let sb = VScrollbar::measure(
                        core.scroll_content_lines(),
                        core.viewport_h,
                        core.line_height(),
                        bounds.height,
                        core.scroll_top,
                    );
                    core.scroll_top = sb.scroll_for_thumb_y(pos.y - bounds.y - grab);
                    core.clamp_scroll();
                    drop(core);
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }

                // 水平滚动条拖拽中（P13）：按抓取偏移反解 scroll_left。
                // P45：行程口径与钳制一致（列模型 ∪ 真实行宽）
                if let Some(grab) = core.hscrollbar_grab {
                    let hsb = HScrollbar::measure(
                        core.hscroll_content_px(),
                        (bounds.width - core.gutter_width()).max(0.0),
                        bounds.width,
                        core.scroll_left,
                    );
                    core.scroll_left = hsb.scroll_for_thumb_x(pos.x - bounds.x - grab);
                    core.clamp_scroll_horizontal();
                    drop(core);
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }

                // 第 67 轮 ⑮：列块拖拽中——更新对角（复用贴缘自动推进），
                // 与普通拖选互斥，走完即返回
                if core.block_dragging {
                    let (dx_cols, dy_lines) = core.edge_scroll_delta(
                        pos.x - bounds.x,
                        pos.y - bounds.y,
                        bounds.width,
                        bounds.height,
                    );
                    if dx_cols != 0.0 {
                        core.scroll_by_columns(dx_cols);
                    }
                    if dy_lines != 0.0 {
                        core.scroll_by_lines(dy_lines);
                    }
                    let hit = core.hit_test(pos.x - bounds.x, pos.y - bounds.y);
                    core.update_block_select(hit);
                    drop(core);
                    shell.publish(crate::Message::EditorNavChanged);
                    shell.request_redraw();
                    return;
                }

                if !core.is_dragging() {
                    return;
                }
                // 打磨：拖选贴近视口上下/左右缘时自动推进视口，
                // 选区可延伸到可见范围之外（事件驱动，随鼠标移动推进）
                let (dx_cols, dy_lines) = core.edge_scroll_delta(
                    pos.x - bounds.x,
                    pos.y - bounds.y,
                    bounds.width,
                    bounds.height,
                );
                if dx_cols != 0.0 {
                    core.scroll_by_columns(dx_cols);
                }
                if dy_lines != 0.0 {
                    core.scroll_by_lines(dy_lines);
                }
                let hit = core.hit_test(pos.x - bounds.x, pos.y - bounds.y);
                if core.cursor != hit {
                    // 拖选：锚点固定在按下时的位置（即移动前的光标）
                    if core.anchor.is_none() {
                        core.anchor = Some(core.cursor);
                    }
                    core.cursor = hit;
                    core.break_typing(); // P37：拖选延伸打断组
                    core.ensure_visible_pub();
                    drop(core);
                    shell.publish(crate::Message::EditorNavChanged);
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let mut core = self.core.borrow_mut();
                // 第 67 轮 ⑮：块拖拽收尾——空块自动清除，有效块保留
                core.finish_block_select();
                core.dragging = false;
                core.scrollbar_grab = None;
                core.hscrollbar_grab = None;
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if !cursor.is_over(bounds) {
                    return;
                }
                // P48：Ctrl+滚轮 = 正文字号缩放（P36 设计口径的落地：只动
                // 文件内容，UI 控件字号固定）。经 FontSizeDelta 走应用层的
                // clamp/当前页生效/落盘链路，与设置面板步进同一手感
                // （FONT_ZOOM_STEP）；触控板 Pixels 增量按行高折算累积，
                // 满一格发一步。事件捕获，不再参与滚动。
                if self.core.borrow().mods.control() {
                    let lh = self.core.borrow().line_height().max(1e-3);
                    let (lines, is_pixels) = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => (*y, false),
                        mouse::ScrollDelta::Pixels { y, .. } => (*y / lh, true),
                    };
                    let (accum, step) =
                        wheel_zoom_step(self.zoom_accum, lines, is_pixels);
                    self.zoom_accum = accum;
                    if step != 0.0 {
                        shell.publish(crate::Message::FontSizeDelta(
                            FONT_ZOOM_STEP * step.signum(),
                        ));
                    }
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }
                // P3：winit 0.30 Windows 上滚轮上推上报 LineDelta(y=+1)，其 changelog
                // 明确「positive Y means moving the content down」（视口向文档头走）；
                // scroll_by_lines 内部已是 scroll_top -= lines，这里再取负就会方向反转。
                // P13：Shift+滚轮改走横向（Windows 惯例，winit 不代做换轴，
                // 修饰键靠 KeyModifiersChanged 维护的最近状态）；触控板的
                // 原生横向分量（x≠0）也直接横滚。
                let (dx_cols, dy_lines, shift) = {
                    let core = self.core.borrow();
                    match delta {
                        mouse::ScrollDelta::Lines { x, y } => (
                            *x * SCROLL_LINES_PER_NOTCH,
                            *y * SCROLL_LINES_PER_NOTCH,
                            core.mods.shift(),
                        ),
                        mouse::ScrollDelta::Pixels { x, y } => (
                            *x / core.char_width().max(1e-3),
                            *y / core.line_height().max(1e-3),
                            core.mods.shift(),
                        ),
                    }
                };
                if shift {
                    self.core.borrow_mut().scroll_by_columns(dy_lines);
                } else {
                    let mut core = self.core.borrow_mut();
                    if dx_cols != 0.0 {
                        core.scroll_by_columns(dx_cols);
                    }
                    core.scroll_by_lines(dy_lines);
                }
                shell.request_redraw();
                shell.capture_event();
            }

            // 输入法：组字过程（预编辑串内联显示）与上屏。
            // P2：事件会被广播给所有控件，只有本控件持有焦点时才消费并捕获；
            // 否则原样放行，让真正持有焦点的控件（查找框等）处理。
            iced::Event::InputMethod(input_method::Event::Preedit(content, _range)) => {
                if self.core.borrow_mut().ime_preedit(content.clone()) {
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::InputMethod(input_method::Event::Commit(text)) => {
                match self.core.borrow_mut().ime_commit(text) {
                    ImeCommit::Ignored => {} // 焦点在别处：不捕获
                    ImeCommit::Consumed(inserted) => {
                        // 统一走应用层编辑入口：获得加载期守卫（active_load）
                        // 与置脏/撤销语义；不再直接 insert_str 绕过 apply_edit
                        if let Some(text) = inserted {
                            shell.publish(crate::Message::Edit(EditOp::InsertText(text)));
                        }
                        shell.request_redraw();
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if let Some(pos) = cursor.position_over(bounds) {
            let core = self.core.borrow();
            let sb = VScrollbar::measure(
                core.scroll_content_lines(),
                core.viewport_h,
                core.line_height(),
                bounds.height,
                core.scroll_top,
            );
            // 滚动条上：拖拽中给 Grabbing，悬停给 Grab；正文仍是文本光标
            if sb.hits(pos.x - bounds.x, pos.y - bounds.y, bounds.width) {
                return if core.scrollbar_grab.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Grab
                };
            }
            // 水平滚动条（P13）同款指针语义（行程口径见拖拽路径）
            let hsb = HScrollbar::measure(
                core.hscroll_content_px(),
                (bounds.width - core.gutter_width()).max(0.0),
                bounds.width,
                core.scroll_left,
            );
            if hsb.hits(pos.x - bounds.x, pos.y - bounds.y, bounds.height) {
                return if core.hscrollbar_grab.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Grab
                };
            }
        }
        if cursor.is_over(bounds) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }
}

/// Alt+Shift 修饰组合（第 67 轮 ⑮ 列块拖拽的触发判定）。
fn iced_mods_alt_shift() -> iced::keyboard::Modifiers {
    iced::keyboard::Modifiers::ALT | iced::keyboard::Modifiers::SHIFT
}


#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
