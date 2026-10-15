//! 壳层（chrome）中性样式：标签条页签、菜单栏按钮、菜单项与通用按钮。
//!
//! （P446 自 settings_ui.rs 纯移动拆出，零行为变更。）这批函数与设置弹窗
//! 共用同一份主题解析 `settings_colors`，而读者几乎全在 chrome／view 侧——
//! 改标签页或菜单栏的配色，原本要去设置弹窗的文件里翻一段无关的目录。

use super::*;
use crate::settings_ui::settings_colors;

/// P112：标签条页签「胶囊」容器样式——沿用
/// [`crate::settings_ui::chrome_nav_button_style`]
/// 在页签上的语义（活动页淡底描边、非活动透明、悬停淡染）。页签从
/// 单个按钮拆成「文字 + × 关闭」两个扁平按钮后，若底色由各自按钮画
/// 会露接缝（相邻 1px 描边叠成 2px 竖线、圆角对不上），故胶囊底统一
/// 由外层容器承担，内部按钮全部透明。活动态优先于悬停态。
pub(crate) fn tab_pill_style(theme: &Theme, active: bool, hovered: bool) -> container::Style {
    let sc = settings_colors(theme);
    // 活动态优先于悬停态；闲置 = 无底无边（None，与按钮样式同口径）
    let (background, border_color) = if active {
        (Some(Background::Color(sc.selected_bg)), sc.control_border)
    } else if hovered {
        (Some(Background::Color(sc.hover)), Color::TRANSPARENT)
    } else {
        (None, Color::TRANSPARENT)
    };
    container::Style {
        background,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

/// P112：页签「文字」按钮扁平样式——无底无框纯文字（与
/// [`menubar_text_style`] 同族），胶囊底色由外层容器绘制；禁用降为
/// 次要色。与 [`tab_close_style`] 一起构成页签胶囊的内部按钮。
pub(crate) fn tab_label_style(theme: &Theme, status: button::Status) -> button::Style {
    let sc = settings_colors(theme);
    button::Style {
        background: None,
        text_color: if matches!(status, button::Status::Disabled) {
            sc.desc
        } else {
            sc.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// P112：页签「×」关闭按钮样式——透明融入胶囊，悬停/按压加淡染圆角
/// 芯片（可点感）；禁用（busy 期间）降为次要色。固定页不渲染该按钮
/// （豁免口径与右键菜单「关闭」一致，update 层守卫双保险）。
pub(crate) fn tab_close_style(theme: &Theme, status: button::Status) -> button::Style {
    let sc = settings_colors(theme);
    button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered | button::Status::Pressed => sc.hover,
            _ => Color::TRANSPARENT,
        })),
        text_color: if matches!(status, button::Status::Disabled) {
            sc.desc
        } else {
            sc.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// P57：全局壳层中性按钮（工具栏/确认条/提示条/微按钮/设置弹窗控件
/// 共用）——控件底 + 1px 描边 + 正文字色，悬停淡染、按压描边转点缀色、
/// 禁用降为次要色。取代 iced 默认的实心 primary 蓝底白字（用户截图反馈：
/// 按钮一度像全选中的高亮条，菜单里尤其严重）。
pub(crate) fn chrome_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let sc = settings_colors(theme);
    let mut style = button::Style {
        background: Some(Background::Color(sc.control_bg)),
        text_color: sc.text,
        border: Border {
            color: sc.control_border,
            width: 1.0,
            radius: Radius::from(5.0),
        },
        shadow: Shadow::default(),
        snap: true,
    };
    match status {
        button::Status::Hovered => style.background = Some(Background::Color(sc.hover)),
        button::Status::Pressed => style.border.color = sc.accent,
        button::Status::Disabled => {
            style.text_color = sc.desc;
            style.border.color = sc.separator;
        }
        button::Status::Active => {}
    }
    style
}

/// 第 76 轮（用户点单）：顶部菜单栏按钮**纯文字扁平**样式——无背景、
/// 无边框、无凸起，看起来不像按钮；hover 不改底色（按钮感来自染底）。
/// 展开态用点缀色文字标示（第 69 轮 menu_bar_open_style 的淡底描边
/// 被取代）；禁用降为次要色（与 chrome_button_style 同一口径）。
pub(crate) fn menubar_text_style(
    theme: &Theme,
    status: button::Status,
    open: bool,
) -> button::Style {
    let sc = settings_colors(theme);
    button::Style {
        background: None,
        text_color: if open {
            sc.accent
        } else if matches!(status, button::Status::Disabled) {
            sc.desc
        } else {
            sc.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(0.0),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// P56：菜单项 / 下拉列表项样式（右键菜单、最近文件、字体候选共用）——
/// 常态透明融入卡片、正文字色，悬停淡染，禁用降为次要色。取代 iced
/// 默认实心蓝（菜单项一度像全选中的高亮条）。
pub(crate) fn chrome_menu_item_style(theme: &Theme, status: button::Status) -> button::Style {
    let sc = settings_colors(theme);
    button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered | button::Status::Pressed => sc.hover,
            _ => Color::TRANSPARENT,
        })),
        // P56：禁用降为次要色（与 chrome_button_style 同一口径）
        text_color: if matches!(status, button::Status::Disabled) {
            sc.desc
        } else {
            sc.text
        },
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: Radius::from(5.0),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}
