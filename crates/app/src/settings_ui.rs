use super::*;

// ---------- 设置弹窗分类导航（P47，侧栏分类风格） ----------

/// 设置弹窗左侧导航的分类页。`ALL` 的顺序 = 侧栏与搜索分组的展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsPage {
    /// 外观：主题、字号
    #[default]
    Appearance,
    /// 字体：正文字体族（含过滤候选列表）
    Font,
    /// 保存：即时保存与防抖
    Save,
    /// 会话与隐私：最近文件 / 快照 / 恢复 / 关窗行为
    Session,
    /// 快捷键：只读速查表（数据源 = HOTKEYS）
    Hotkeys,
    /// 关于：名称 / 版本 / 渲染后端 / 协议
    About,
}

impl SettingsPage {
    /// 侧栏顺序（= 搜索结果分组顺序）
    pub(crate) const ALL: [SettingsPage; 6] = [
        Self::Appearance,
        Self::Font,
        Self::Save,
        Self::Session,
        Self::Hotkeys,
        Self::About,
    ];

    /// 侧栏项 / 内容区分组小标题
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Appearance => "外观",
            Self::Font => "字体",
            Self::Save => "保存",
            Self::Session => "会话与隐私",
            Self::Hotkeys => "快捷键",
            Self::About => "关于",
        }
    }

    /// config 持久化键（P51）。与 core `SETTINGS_PAGES` 注册表的对应
    /// 关系由 `settings_nav_keys_match_core_registry` 测试钉住防漂移。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Appearance => editpad_core::SETTINGS_PAGE_APPEARANCE,
            Self::Font => editpad_core::SETTINGS_PAGE_FONT,
            Self::Save => editpad_core::SETTINGS_PAGE_SAVE,
            Self::Session => editpad_core::SETTINGS_PAGE_SESSION,
            Self::Hotkeys => editpad_core::SETTINGS_PAGE_HOTKEYS,
            Self::About => editpad_core::SETTINGS_PAGE_ABOUT,
        }
    }

    /// 从 config 键恢复分类页（P51）；未知值经 core 归一回默认页。
    pub(crate) fn from_key(key: &str) -> Self {
        match editpad_core::normalize_settings_page(key) {
            editpad_core::SETTINGS_PAGE_FONT => Self::Font,
            editpad_core::SETTINGS_PAGE_SAVE => Self::Save,
            editpad_core::SETTINGS_PAGE_SESSION => Self::Session,
            editpad_core::SETTINGS_PAGE_HOTKEYS => Self::Hotkeys,
            editpad_core::SETTINGS_PAGE_ABOUT => Self::About,
            _ => Self::Appearance,
        }
    }
}

/// 「正文字体」行的行键：控件匹配与字体挑选块挂载点共用同一常量，
/// 防止文案改动后两处漂移。
pub(crate) const FONT_ROW_KEY: &str = "正文字体";

/// 设置行的统一视图：静态元数据（SETTINGS_ROWS）+ 热键行动态展开，
/// 渲染与搜索共用同一清单（防「展示一套、过滤另一套」的数据漂移）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingsRow {
    pub(crate) page: SettingsPage,
    /// 控件匹配键（= title；热键行 = 组合键，全清单唯一）
    pub(crate) key: &'static str,
    pub(crate) title: &'static str,
    pub(crate) desc: &'static str,
}

/// 设置行静态元数据（P47）：标题 / 描述文案与所属页。热键行不在此列
/// （渲染时由 HOTKEYS 展开），关于页信息在此登记。
pub(crate) const SETTINGS_ROWS: &[SettingsRow] = &[
    SettingsRow {
        page: SettingsPage::Appearance,
        key: "主题",
        title: "主题",
        desc: "切换深色 / 浅色主题，立即生效并记住。",
    },
    SettingsRow {
        page: SettingsPage::Appearance,
        key: "字号",
        title: "字号",
        desc: "正文文字大小；编辑器内 Ctrl+滚轮 缩放，或在此步进调节。",
    },
    SettingsRow {
        page: SettingsPage::Font,
        key: FONT_ROW_KEY,
        title: FONT_ROW_KEY,
        desc: "界面与正文共用的字体族；建议选含中文字形的等宽字体，非等宽字体的列对齐会漂移。",
    },
    SettingsRow {
        page: SettingsPage::Save,
        key: "即时保存",
        title: "即时保存",
        desc: "停手后自动落盘，不必手动 Ctrl+S。",
    },
    SettingsRow {
        page: SettingsPage::Save,
        key: "防抖秒数",
        title: "防抖秒数",
        desc: "停手多少秒后执行自动保存。",
    },
    SettingsRow {
        page: SettingsPage::Session,
        key: "记住最近打开的文件",
        title: "记住最近打开的文件",
        desc: "在「最近打开」保留历史；关闭开关会一并清空存量记录。",
    },
    SettingsRow {
        page: SettingsPage::Session,
        key: "会话快照",
        title: "会话快照",
        desc: "关窗时自动保存未存内容，异常退出后可恢复。",
    },
    SettingsRow {
        page: SettingsPage::Session,
        key: "启动时恢复上次界面",
        title: "启动时恢复上次界面",
        desc: "启动时还原上次的标签页与内容。",
    },
    SettingsRow {
        page: SettingsPage::Session,
        key: "关窗行为",
        title: "关窗行为",
        desc: "「快照直退」不打断；「每次询问」先确认未保存内容。",
    },
    SettingsRow {
        page: SettingsPage::Session,
        key: "快照心跳间隔（秒）",
        title: "快照心跳间隔（秒）",
        desc: "后台周期保存快照的间隔。",
    },
    SettingsRow {
        page: SettingsPage::About,
        key: "名称",
        title: "名称",
        desc: "Editpad —— 极简记事本。",
    },
    SettingsRow {
        page: SettingsPage::About,
        key: "版本",
        title: "版本",
        desc: env!("CARGO_PKG_VERSION"),
    },
    SettingsRow {
        page: SettingsPage::About,
        key: "渲染后端",
        title: "渲染后端",
        desc: "tiny-skia 软渲染（P41 内存取舍：进程内存约为 GPU 路径的 1/12）。",
    },
    SettingsRow {
        page: SettingsPage::About,
        key: "开源协议",
        title: "开源协议",
        desc: "Apache-2.0",
    },
];

/// 全部设置行的统一清单：SETTINGS_ROWS + HOTKEYS 展开的热键行。
/// 顺序 = 分类内自上而下的展示顺序；搜索过滤与行渲染都从这里出发。
pub(crate) fn settings_rows() -> impl Iterator<Item = SettingsRow> {
    SETTINGS_ROWS.iter().copied().chain(HOTKEYS.iter().map(
        |(combo, desc)| SettingsRow {
            page: SettingsPage::Hotkeys,
            key: combo,
            title: combo,
            desc,
        },
    ))
}

/// 搜索命中判定（P47）：空白词 = 不过滤（全部命中）；否则对标题或描述
/// 做大小写不敏感的子串匹配（中文不受影响）。纯函数可单测。
pub(crate) fn settings_search_hit(query: &str, title: &str, desc: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || title.to_lowercase().contains(&needle)
        || desc.to_lowercase().contains(&needle)
}

/// 设置弹窗配色（P47）：浅色 = 参考用户提供的 设计稿观感的固定值
/// （米白底 + 橄榄绿点缀）；深色 = 从主题 palette 派生。深浅判别与
/// editor::EditorColors::resolve 同一口径（前景比背景亮 = 深色），
/// 不依赖具体主题枚举。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingsColors {
    /// 卡片背景（浅色 = 米白）
    pub(crate) card_bg: Color,
    /// 标题与正文
    pub(crate) text: Color,
    /// 描述等次要文字
    pub(crate) desc: Color,
    /// 行分隔线 / 分栏线
    pub(crate) separator: Color,
    /// 点缀色：复选框选中底、输入框聚焦描边
    pub(crate) accent: Color,
    /// 控件（按钮 / 输入框）底色
    pub(crate) control_bg: Color,
    /// 控件描边
    pub(crate) control_border: Color,
    /// 悬停底色（导航项 / 列表项 / 按钮）
    pub(crate) hover: Color,
    /// 选中导航项底色
    pub(crate) selected_bg: Color,
}

/// 按主题解析设置弹窗配色（浅色固定值 / 深色 palette 派生，见结构体注释）。
pub(crate) fn settings_colors(theme: &Theme) -> SettingsColors {
    let p = theme.palette();
    let dark = editor::luminance(p.text) > editor::luminance(p.background);
    if !dark {
        SettingsColors {
            card_bg: Color::from_rgb8(0xFA, 0xF8, 0xF0),
            text: Color::from_rgb8(0x35, 0x34, 0x2C),
            desc: Color::from_rgb8(0x92, 0x90, 0x84),
            separator: Color::from_rgb8(0xE8, 0xE4, 0xD7),
            accent: Color::from_rgb8(0x71, 0x9E, 0x4F),
            control_bg: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            control_border: Color::from_rgb8(0xD9, 0xD5, 0xC7),
            hover: Color::from_rgba8(0x00, 0x00, 0x00, 0.05),
            selected_bg: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        }
    } else {
        SettingsColors {
            card_bg: editor::lighten(p.background, 0.05),
            text: p.text,
            desc: Color { a: 0.55, ..p.text },
            separator: Color { a: 0.14, ..p.text },
            accent: p.primary,
            control_bg: editor::lighten(p.background, 0.10),
            control_border: Color { a: 0.30, ..p.text },
            hover: Color { a: 0.06, ..p.text },
            selected_bg: Color { a: 0.10, ..p.text },
        }
    }
}

/// P56/P57：导航项 / 页签样式（设置侧栏分类、标签条页签共用）——
/// 选中 = 底色 + 1px 描边（弱选中指示），未选中透明、悬停淡染。
pub(crate) fn chrome_nav_button_style(
    theme: &Theme,
    status: button::Status,
    selected: bool,
) -> button::Style {
    let sc = settings_colors(theme);
    let mut style = button::Style {
        background: None,
        text_color: sc.text,
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        snap: true,
    };
    if selected {
        style.background = Some(Background::Color(sc.selected_bg));
        style.border.color = sc.control_border;
    } else if matches!(
        status,
        button::Status::Hovered | button::Status::Pressed
    ) {
        style.background = Some(Background::Color(sc.hover));
    }
    style
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

/// P47：设置弹窗输入框（侧栏搜索 / 字体过滤）：控件底 + 描边，
/// 聚焦转点缀色。
pub(crate) fn settings_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let sc = settings_colors(theme);
    let border_color = match status {
        text_input::Status::Focused { .. } => sc.accent,
        text_input::Status::Disabled => sc.separator,
        _ => sc.control_border,
    };
    text_input::Style {
        background: Background::Color(sc.control_bg),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: Radius::from(5.0),
        },
        icon: sc.desc,
        placeholder: sc.desc,
        value: sc.text,
        selection: Color { a: 0.25, ..sc.accent },
    }
}

/// P47：设置行复选框——选中 = 点缀色底白勾，未选中 = 控件底 + 描边。
pub(crate) fn settings_checkbox_style(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let sc = settings_colors(theme);
    let is_checked = match status {
        checkbox::Status::Active { is_checked }
        | checkbox::Status::Hovered { is_checked }
        | checkbox::Status::Disabled { is_checked } => is_checked,
    };
    checkbox::Style {
        background: Background::Color(if is_checked {
            sc.accent
        } else {
            sc.control_bg
        }),
        icon_color: Color::WHITE,
        border: Border {
            color: if is_checked { sc.accent } else { sc.control_border },
            width: 1.0,
            radius: Radius::from(4.0),
        },
        text_color: Some(sc.text),
    }
}

/// P47：设置弹窗卡片样式——设置专属底色（浅色 = 米白）+ 1px 描边 +
/// 大圆角。无投影（P43 的 damage 取舍对一切浮层成立，见
/// `popup_card_style` 注释）。
pub(crate) fn settings_card_style(theme: &Theme) -> container::Style {
    let sc = settings_colors(theme);
    container::Style {
        background: Some(Background::Color(sc.card_bg)),
        border: Border {
            color: sc.control_border,
            width: 1.0,
            radius: Radius::from(8.0),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

/// 设置行之间的 1px 分隔线（行式布局的视觉骨架）。
pub(crate) fn settings_separator(sc: SettingsColors) -> Element<'static, Message> {
    rule::horizontal(1)
        .style(move |_| rule::Style {
            color: sc.separator,
            radius: Radius::from(0.0),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

/// 侧栏与内容区之间的 1px 纵向分栏线。
pub(crate) fn settings_divider(sc: SettingsColors) -> Element<'static, Message> {
    rule::vertical(1)
        .style(move |_| rule::Style {
            color: sc.separator,
            radius: Radius::from(0.0),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

/// 设置弹窗卡片的目标宽度（目标 ≈ 720px：侧栏 170 + 内容 ~530）。
pub(crate) const SETTINGS_CARD_W: f32 = 720.0;

/// P47：设置弹窗卡片尺寸适配——宽度 720 上限，小窗收缩到 vw−32（永不
/// 出窗）；返回的内容滚动区高度 ≤ vh−96（卡片总高 = 内容 + 标题行
/// ≈ vh−48，配合浮层 16px 边距上下各留 ~24px）。窗口尺寸未知（0）时
/// 用保守默认。纯函数可单测。
pub(crate) fn settings_card_size(vw: f32, vh: f32) -> (f32, f32) {
    let w = if vw >= 48.0 {
        SETTINGS_CARD_W.min(vw - 32.0)
    } else {
        SETTINGS_CARD_W
    };
    let h = if vh >= 144.0 { (vh - 96.0).min(680.0) } else { 560.0 };
    (w, h)
}

