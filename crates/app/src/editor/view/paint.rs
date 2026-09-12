//! 绘制原语：预编辑串宽度实测、文本切片绘制、折行段绘制与其归属判定。
//!
//! （P160 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

/// P115：组字串实测宽（px）：与正文绘制同源的整串 shaping 末项（连字/
/// 混合宽度如实反映）；失败（字体未就绪）回退列模型保守宽（组字是
/// 瞬态，占位偏移误差可接受）。
pub(super) fn measure_preedit_w(font: Font, size: f32, preedit: &str) -> f32 {
    shape_row_xs(font, size, preedit)
        .and_then(|xs| xs.last().copied())
        .unwrap_or_else(|| display_cols(preedit) * size * 0.5625)
}
/// P115：正文列区间 `[lo, hi)` 的单片绘制（组字三段式复用；也承载原
/// 折行/关态两分支的整段/整行路径，pixel 批守护等价）。`seg_start` =
/// 段内像素基准列（关态 0）；`dx` = 段起点外的附加偏移（组字 C 段 =
/// preedit 占位后移）；无 runs 时整片一色，有 runs 时逐 run 裁色。
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_text_slice(
    renderer: &mut iced::Renderer,
    core: &super::super::core::EditorCore,
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
pub(super) struct ReflowLayout {
    pub(super) line: usize,
    pub(super) col_p: usize,
    pub(super) pel: usize,
    pub(super) s: String,
    pub(super) s_xs: Vec<f32>,
    pub(super) breaks: Vec<usize>,
    pub(super) k: isize,
    pub(super) v0: u32,
}
/// P115 续：组字重排段的绘制——合成串段 `[bs, be)` 内最多三块（前文/
/// 组字/后文）逐块上屏，x 全按合成串真实 xs 定位；前/后文块继承原行
/// runs 逐色（后文源列 = 合成索引 − pel），组字块用组字色。
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_composed_segment(
    renderer: &mut iced::Renderer,
    core: &super::super::core::EditorCore,
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
pub(super) fn reflow_seg_of(breaks: &[usize], idx: usize) -> Option<usize> {
    breaks
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &b)| b <= idx)
        .map(|(i, _)| i)
}
