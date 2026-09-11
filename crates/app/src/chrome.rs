use super::*;
// P153：查找框控件的「未淡出态」直接复用既有按钮/复选框样式（保证淡出
// 开关两侧逐字等价）；二者定义在 settings_ui.rs，不经 main.rs 的 glob 转发。
use crate::settings_ui::{chrome_button_style, settings_checkbox_style};

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

/// P153：查找框淡出态背板透明度（用户点单「保留文字可读」，第三轮调至 0.4
/// ——「所有元素一起更淡」，故卡片/输入框/按钮/复选框/标签共用此单一系数）。
///
/// 半透明是**逐 quad 的 alpha 合成**（tiny-skia 后端直接按 alpha 混合）。
/// 卡片内**每个自带底色的控件都能单独盖住下层**：输入框用主题
/// `background.base` 实底色、按钮/复选框用 [`settings_colors`] 的
/// `control_bg`（浅色是纯白）——只压卡片背板，这些控件仍是不透明的白块
/// 浮在半透明卡片上（用户复报「输入框/替换框/按钮没有半透明」）。
/// 故淡出是**整框统一口径**：背板 + 输入框 + 按钮 + 复选框 + 纯文本标签的
/// 底色/文字/描边全部按同一系数压 alpha（见 [`find_input_style`] /
/// [`find_button_style`] / [`find_checkbox_style`] / [`find_label_color`]），
/// 被它盖住的正文才能真正透出来。
pub(crate) const FIND_CARD_DIM_ALPHA: f32 = 0.4;

/// P153：淡出系数（1.0 = 不淡出）。所有查找框内的控件样式都经它统一压
/// alpha，避免「卡片淡了、控件没淡」的口径分裂。
pub(crate) fn find_dim_factor(dimmed: bool) -> f32 {
    if dimmed {
        FIND_CARD_DIM_ALPHA
    } else {
        1.0
    }
}

/// P153：按 `factor` 压一个颜色的 alpha（factor ≥ 1 时原样返回）。
fn dampen(color: Color, factor: f32) -> Color {
    if factor >= 1.0 {
        color
    } else {
        Color {
            a: color.a * factor,
            ..color
        }
    }
}

/// P153：按 `factor` 压背景色的 alpha（`Background` 是枚举，非纯色原样返回）。
fn dampen_bg(bg: Background, factor: f32) -> Background {
    match bg {
        Background::Color(c) => Background::Color(dampen(c, factor)),
        other => other,
    }
}

/// P153：查找卡片里的**纯文本**标签（拖动条文案、命中计数、目录行、
/// 扫描进度）淡出用色：取主题正文字色按系数压 alpha。
///
/// 为什么单独给：`text` 控件无底色，用的是 iced 默认文字样式（主题
/// `palette.text`，alpha = 1）——上一版只压了背板与控件，这些标签仍是
/// 全不透明（用户复报「无匹配提示还是原来的，没跟着变淡」）。
/// `None` = 不淡出：调用方**不设色**（保持 iced 默认口径，逐字等价）。
pub(crate) fn find_label_color(theme: &Theme, factor: f32) -> Option<Color> {
    (factor < 1.0).then(|| dampen(theme.palette().text, factor))
}

/// P153：查找卡片淡出态样式——[`popup_card_style`] 的同一形状/描边，
/// 仅把背板背景色压到 [`FIND_CARD_DIM_ALPHA`]。
pub(crate) fn popup_card_dim_style(theme: &Theme) -> container::Style {
    let mut style = popup_card_style(theme);
    if let Some(Background::Color(color)) = style.background {
        style.background = Some(Background::Color(Color {
            a: FIND_CARD_DIM_ALPHA,
            ..color
        }));
    }
    style
}

/// P153：查找/替换输入框样式——未淡出（`factor >= 1`）= iced 默认样式
/// （与改前逐字等价：主题 `background.base` 底 + `strong` 描边、聚焦转
/// primary 描边）；淡出 = 同一套颜色整体压 alpha。
pub(crate) fn find_input_style(
    theme: &Theme,
    status: text_input::Status,
    factor: f32,
) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    if factor < 1.0 {
        style.background = dampen_bg(style.background, factor);
        style.border.color = dampen(style.border.color, factor);
        style.icon = dampen(style.icon, factor);
        style.placeholder = dampen(style.placeholder, factor);
        style.value = dampen(style.value, factor);
        style.selection = dampen(style.selection, factor);
    }
    style
}

/// P153：查找卡片按钮样式——未淡出 = 既有 [`chrome_button_style`]（逐字
/// 等价）；淡出 = 底色/描边/文字/悬停同口径压 alpha（禁用态次要色一并压）。
pub(crate) fn find_button_style(
    theme: &Theme,
    status: button::Status,
    factor: f32,
) -> button::Style {
    let mut style = chrome_button_style(theme, status);
    if factor < 1.0 {
        style.background = style.background.map(|bg| dampen_bg(bg, factor));
        style.text_color = dampen(style.text_color, factor);
        style.border.color = dampen(style.border.color, factor);
    }
    style
}

/// P153：查找卡片复选框样式——未淡出 = 既有 [`settings_checkbox_style`]；
/// 淡出 = 选中底（点缀色）/未选中底/描边/白勾/文字全部压 alpha（勾是
/// 纯白 `Color::WHITE`，不压就是淡出框里唯一的纯白亮点）。
pub(crate) fn find_checkbox_style(
    theme: &Theme,
    status: checkbox::Status,
    factor: f32,
) -> checkbox::Style {
    let mut style = settings_checkbox_style(theme, status);
    if factor < 1.0 {
        style.background = dampen_bg(style.background, factor);
        style.icon_color = dampen(style.icon_color, factor);
        style.border.color = dampen(style.border.color, factor);
        style.text_color = style.text_color.map(|c| dampen(c, factor));
    }
    style
}
