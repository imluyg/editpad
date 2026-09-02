//! 纵横滚动条的几何与行程换算（P68 自 editor.rs 拆出，纯移动零行为变更）。

use iced::Rectangle;

// ---------- 垂直滚动条 ----------
//
// 覆盖式（overlay）设计：不改变文本排版与命中测试的坐标体系，
// 只在控件右缘绘制并在鼠标事件里优先拦截。内容不超出视口时整个
// 滚动条不存在（用户要求：没超就不需要）。

/// 滑块/轨道厚度。
pub(crate) const SCROLLBAR_WIDTH: f32 = 10.0;
/// 滚动条距控件右缘的间隙。
pub(crate) const SCROLLBAR_EDGE_INSET: f32 = 3.0;
/// 轨道距控件上下缘的内缩。
const SCROLLBAR_TRACK_PAD: f32 = 2.0;
/// 超长文档下滑块的最小高度（否则 50MB 文档的滑块只剩几个像素抓不住）。
pub(crate) const THUMB_MIN_H: f32 = 32.0;
/// 命中区总宽（比可视宽度略宽，好点中）。
pub(crate) const SCROLLBAR_ZONE_W: f32 = SCROLLBAR_WIDTH + SCROLLBAR_EDGE_INSET * 2.0;
/// 垂直滚动条占用的**可视**带宽 = 滑块厚 + 右缘间隙（不含命中区扩大，
/// 命中区多出的 EDGE_INSET 在控件右缘之外，不影响文本排版）。
/// P99：软换行折行预算在滚动条需要出现时按它让位——折行文本在滑块
/// 左侧收尾，行尾字符不再被盖住/显得截断（P95 零预留的贴满右缘
/// 是「被盖住」的直接成因之一，用户复点单后按需预留）。
pub(crate) const VERTICAL_SCROLLBAR_RESERVE: f32 =
    SCROLLBAR_WIDTH + SCROLLBAR_EDGE_INSET;

/// 垂直滚动条几何。全部为**相对控件**的像素坐标；由
/// [`VScrollbar::measure`] 从当前状态推导——窗口缩放、字号调整、
/// 文档变化都会在下一帧自然反映，无需额外同步。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VScrollbar {
    /// 内容超出视口才为 true
    pub(crate) needed: bool,
    pub(crate) track_y: f32,
    pub(crate) track_h: f32,
    pub(crate) thumb_y: f32,
    pub(crate) thumb_h: f32,
    /// 可滚动行程（行）。口径与 `clamp_scroll` 的 max+1 对齐：
    /// 允许滚到末行后留一行空白，与滚轮行为一致。
    pub(crate) range_lines: f32,
}

impl VScrollbar {
    pub(crate) fn measure(
        content_lines: usize,
        viewport_h: f32,
        line_h: f32,
        widget_h: f32,
        scroll_top: f32,
    ) -> Self {
        let content_h = content_lines.max(1) as f32 * line_h;
        let needed = line_h > 0.0 && viewport_h > 0.0 && content_h > viewport_h;
        let track_y = SCROLLBAR_TRACK_PAD;
        let track_h = (widget_h - SCROLLBAR_TRACK_PAD * 2.0).max(0.0);
        if !needed || track_h <= 0.0 {
            return Self {
                needed: false,
                track_y,
                track_h,
                thumb_y: track_y,
                thumb_h: 0.0,
                range_lines: 0.0,
            };
        }
        let rows_in_view = viewport_h / line_h;
        let range_lines = (content_lines as f32 - rows_in_view + 1.0).max(1.0);
        let thumb_h = (track_h * viewport_h / content_h).clamp(THUMB_MIN_H, track_h);
        let travel = (track_h - thumb_h).max(0.0);
        let ratio = (scroll_top / range_lines).clamp(0.0, 1.0);
        Self {
            needed: true,
            track_y,
            track_h,
            thumb_y: track_y + ratio * travel,
            thumb_h,
            range_lines,
        }
    }

    /// 滑块顶部目标 y → scroll_top（已夹紧到行程内）。
    pub(crate) fn scroll_for_thumb_y(&self, thumb_top_y: f32) -> f32 {
        let travel = (self.track_h - self.thumb_h).max(1e-3);
        let ratio = ((thumb_top_y - self.track_y) / travel).clamp(0.0, 1.0);
        ratio * self.range_lines
    }

    /// 点击轨道：把滑块中心对准点击处（连续按住可继续拖拽）。
    pub(crate) fn scroll_for_track_click(&self, click_y: f32) -> f32 {
        self.scroll_for_thumb_y(click_y - self.thumb_h * 0.5)
    }

    /// 控件局部坐标是否落在滚动条交互区。
    pub(crate) fn hits(&self, local_x: f32, local_y: f32, widget_w: f32) -> bool {
        self.needed
            && local_x >= widget_w - SCROLLBAR_ZONE_W
            && local_y >= self.track_y
            && local_y <= self.track_y + self.track_h
    }

    /// 滑块矩形（x/y 相对控件左上角），供绘制与拖拽命中。
    pub(crate) fn thumb_rect(&self, widget_w: f32) -> Rectangle {
        Rectangle {
            x: widget_w - SCROLLBAR_EDGE_INSET - SCROLLBAR_WIDTH,
            y: self.thumb_y,
            width: SCROLLBAR_WIDTH,
            height: self.thumb_h,
        }
    }
}

/// 超宽内容下水平滑块的最小宽度（与垂直侧同理由：必须抓得住）。
pub(crate) const THUMB_MIN_W: f32 = 32.0;
/// 滑块/轨道的可视厚度（垂直条的厚度常量复用于水平条的高度）。
/// P58：水平条 10→6px——用户截图反馈覆盖末行文字过于明显；
/// 淡出（P54）+ 降厚双管齐下。垂直条宽度（SCROLLBAR_WIDTH）不变。
pub(crate) const SCROLLBAR_THUMB_THICKNESS: f32 = 6.0;
/// 水平条命中区高度（下缘窄带，比可视厚度略高好点中）。
pub(crate) const SCROLLBAR_ZONE_H: f32 = SCROLLBAR_THUMB_THICKNESS + SCROLLBAR_EDGE_INSET * 2.0;

/// 水平滚动条几何（P13）。坐标体系与 [`VScrollbar`] 对称：
/// 全部为**相对控件**的像素坐标，贴控件下缘；轨道横贯整个控件宽度，
/// 行程比例按「正文区可视宽」计算（行号栏不参与横向滚动）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HScrollbar {
    /// 内容超出视口宽才为 true
    pub(crate) needed: bool,
    pub(crate) track_x: f32,
    pub(crate) track_w: f32,
    pub(crate) thumb_x: f32,
    pub(crate) thumb_w: f32,
    /// 可横向滚动的行程（像素）。
    pub(crate) range_px: f32,
}

impl HScrollbar {
    /// * `content_px`：最宽行的像素宽度（高水位 × 列宽）；
    /// * `view_px`：正文区可视宽度（视口宽 − 行号栏）。
    pub(crate) fn measure(content_px: f32, view_px: f32, widget_w: f32, scroll_left: f32) -> Self {
        let needed = view_px > 0.0 && content_px > view_px;
        let track_x = SCROLLBAR_TRACK_PAD;
        let track_w = (widget_w - SCROLLBAR_TRACK_PAD * 2.0).max(0.0);
        if !needed || track_w <= 0.0 {
            return Self {
                needed: false,
                track_x,
                track_w,
                thumb_x: track_x,
                thumb_w: 0.0,
                range_px: 0.0,
            };
        }
        let range_px = (content_px - view_px).max(1.0);
        let thumb_w = (track_w * view_px / content_px).clamp(THUMB_MIN_W, track_w);
        let travel = (track_w - thumb_w).max(0.0);
        let ratio = (scroll_left / range_px).clamp(0.0, 1.0);
        Self {
            needed: true,
            track_x,
            track_w,
            thumb_x: track_x + ratio * travel,
            thumb_w,
            range_px,
        }
    }

    /// 滑块左缘目标 x → scroll_left（已夹紧到行程内）。
    pub(crate) fn scroll_for_thumb_x(&self, thumb_left_x: f32) -> f32 {
        let travel = (self.track_w - self.thumb_w).max(1e-3);
        let ratio = ((thumb_left_x - self.track_x) / travel).clamp(0.0, 1.0);
        ratio * self.range_px
    }

    /// 点击轨道：把滑块中心对准点击处（连续按住可继续拖拽）。
    pub(crate) fn scroll_for_track_click(&self, click_x: f32) -> f32 {
        self.scroll_for_thumb_x(click_x - self.thumb_w * 0.5)
    }

    /// 控件局部坐标是否落在滚动条交互区（下缘窄带）。
    pub(crate) fn hits(&self, local_x: f32, local_y: f32, widget_h: f32) -> bool {
        self.needed
            && local_y >= widget_h - SCROLLBAR_ZONE_H
            && local_x >= self.track_x
            && local_x <= self.track_x + self.track_w
    }

    /// 滑块矩形（相对控件），供绘制与拖拽命中。
    pub(crate) fn thumb_rect(&self, widget_h: f32) -> Rectangle {
        Rectangle {
            x: self.thumb_x,
            y: widget_h - SCROLLBAR_EDGE_INSET - SCROLLBAR_THUMB_THICKNESS,
            width: self.thumb_w,
            height: SCROLLBAR_THUMB_THICKNESS,
        }
    }
}

