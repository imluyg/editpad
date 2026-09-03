use super::*;

/// 右键菜单卡片的估宽（px）：最宽项「关闭其他标签页(N)」≈ 8 汉字 ×16px
/// + 内边距。仅用于贴边钳制，与实际 Shrink 宽度的少量偏差可接受。
pub(crate) const CTX_MENU_W: f32 = 200.0;
/// 右键菜单卡片的估高（px）：标题行 + 6 个菜单项 + 分隔线 + 内边距。
/// 窗口高度未知或极小（≤48px）时作为保守值：钳制与滚动共用。
pub(crate) const CTX_MENU_H: f32 = 280.0;

// ---------- 第 70 轮：菜单栏槽位几何 ----------

/// 菜单栏按钮槽宽与左缘（与 view.rs 菜单栏布局耦合：五按钮同宽 2 字
/// 文本 + padding [3,12] + 1px 边框 + spacing 2；布局改动需同步）。
pub(crate) const MENU_SLOT_W: f32 = 54.0;
pub(crate) const MENU_BAR_LEFT: f32 = 6.0;
/// 菜单栏条带高度（浮层背板据此判定「点击落在菜单栏上=切换菜单」）。
pub(crate) const MENU_BAR_H: f32 = 36.0;

/// 悬停 x → 所属菜单槽位序号（0..=4，纯函数可单测）。
pub(crate) fn menubar_slot_idx(x: f32) -> usize {
    (((x - MENU_BAR_LEFT) / MENU_SLOT_W).floor().max(0.0) as usize).min(4)
}

/// P43：右键菜单卡片高度适配——窗口高度已知且足够时最高占
/// `vh − 16`（上下各留 8px 边距），内容超高时卡片内部滚动；
/// 窗口高度未知/过小（首帧或极小窗）回退常量估高（仍保证 ≤ 全高，
/// 永不盖满界面）。纯函数可单测。
pub(crate) fn ctx_menu_card_h(vh: f32) -> f32 {
    if vh > 48.0 {
        (vh - 16.0).min(CTX_MENU_H)
    } else {
        CTX_MENU_H
    }
}

/// P39：浮层锚点贴边钳制——菜单整体保持在窗口内（右缘翻左/下缘翻上
/// 的效果 = 把锚点往回拉）。窗口尺寸未知（宽或高为 0）时该轴不钳制；
/// 结果恒非负。纯函数可单测。
pub(crate) fn clamp_menu_anchor(
    pos: (f32, f32),
    viewport: (f32, f32),
    menu_w: f32,
    menu_h: f32,
) -> (f32, f32) {
    let (mut x, mut y) = pos;
    let (vw, vh) = viewport;
    if vw > 0.0 && menu_w.is_finite() && menu_w > 0.0 {
        x = x.min((vw - menu_w).max(0.0));
    }
    if vh > 0.0 && menu_h.is_finite() && menu_h > 0.0 {
        y = y.min((vh - menu_h).max(0.0));
    }
    (x.max(0.0), y.max(0.0))
}

/// P39/P40：浮层卡片样式——主题背景 + 1px 描边（**无投影**），深浅主题
/// 通用（从 palette 派生，与编辑器 EditorColors::resolve 同一口径）。
///
/// 无投影是有意取舍（P43 追加）：tiny-skia 后端部分重绘的 damage 区只覆盖
/// quad.bounds + 1px，而 shadow 模糊带外扩 blur_radius（4px）且在 alpha 合成
/// 下残留叠加——鼠标移动反复触发 hover 重绘时阴影逐帧变浓，表现为
/// 「弹窗随鼠标移动越来越暗」。1px 描边 + 圆角已足以区分浮层与背景。
pub(crate) fn popup_card_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Background::Color(palette.background)),
        border: Border {
            color: Color { a: 0.35, ..palette.text },
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}
