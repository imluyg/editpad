use super::*;
use iced::widget::column;

// ---------- 设置弹窗分类导航（P47，侧栏分类风格） ----------

/// 设置弹窗左侧导航的分类页。`ALL` 的顺序 = 侧栏与搜索分组的展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsPage {
    /// 外观：主题、不可见字符标记
    #[default]
    Appearance,
    /// 字体：正文字体族（含过滤候选列表）+ 字号（第 68 轮移入）
    Font,
    /// 保存：自动写盘开关与延迟
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
    /// 持久化键。第 64 轮撤销分类页记忆后仅剩测试消费（注册表对拍），
    /// 故限定测试编译。
    #[cfg(test)]
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
    // 第 64 轮用户点单：设置弹窗不再持久化/恢复上次浏览的分类页——
    // 每次打开都落在第一分类（外观），原 from_key 恢复入口随之移除。
}

/// 「正文字体」行的行键：控件匹配与字体挑选块挂载点共用同一常量，
/// 防止文案改动后两处漂移。
pub(crate) const FONT_ROW_KEY: &str = "正文字体";

/// 「编辑后自动写盘」行的行键（P63 改名，原「即时保存」）：控件匹配
/// 与元数据声明共用同一常量，防止文案改动后两处漂移。
pub(crate) const AUTOSAVE_ROW_KEY: &str = "编辑后自动写盘";

/// 设置行的统一视图：静态元数据（SETTINGS_ROWS）+ 热键行动态展开，
/// 渲染与搜索共用同一清单（防「展示一套、过滤另一套」的数据漂移）。
/// P62：字段改 String——热键行的描述是当前生效组合（随重映射变化，
/// 非 'static）。
#[derive(Debug, Clone)]
pub(crate) struct SettingsRow {
    pub(crate) page: SettingsPage,
    /// 控件匹配键（= title；热键行 = 动作 id，全清单唯一）
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) desc: String,
}

/// 设置行静态元数据（P47）：标题 / 描述文案与所属页。热键行不在此列
/// （P62 起热键页为动态行，由 view::rows_for 构建），关于页信息在此登记。
pub(crate) struct StaticRow {
    page: SettingsPage,
    key: &'static str,
    title: &'static str,
    desc: &'static str,
}

/// 设置行静态元数据（P47）：标题 / 描述文案与所属页。
pub(crate) const SETTINGS_ROWS: &[StaticRow] = &[
    StaticRow {
        page: SettingsPage::Appearance,
        key: "主题",
        title: "主题",
        desc: "切换深色 / 浅色主题，立即生效并记住。",
    },
    StaticRow {
        page: SettingsPage::Appearance,
        key: "显示空白字符",
        title: "显示空白字符",
        desc: "在空格与制表符位置画淡色标记（不改文档内容）。",
    },
    StaticRow {
        page: SettingsPage::Appearance,
        key: "显示行尾符",
        title: "显示行尾符",
        desc: "在每行末尾画一个短标，标出换行位置。",
    },
    StaticRow {
        page: SettingsPage::Font,
        key: FONT_ROW_KEY,
        title: FONT_ROW_KEY,
        desc: "界面与正文共用的字体族；建议选含中文字形的等宽字体，非等宽字体的列对齐会漂移。",
    },
    // 第 68 轮用户点单：字号从外观移入字体分类（族与大小同页调）
    StaticRow {
        page: SettingsPage::Font,
        key: "字号",
        title: "字号",
        desc: "正文文字大小；编辑器内 Ctrl+滚轮 缩放，或在此步进调节。",
    },
    StaticRow {
        page: SettingsPage::Save,
        key: AUTOSAVE_ROW_KEY,
        title: AUTOSAVE_ROW_KEY,
        desc: "默认关闭（主流编辑器口径）：修改留在窗口内，Ctrl+S 才写原文件；崩溃防护由会话快照兜底。开启后若文件被外部改动会先拒写并提示。",
    },
    StaticRow {
        page: SettingsPage::Save,
        key: "自动写盘延迟（秒）",
        title: "自动写盘延迟（秒）",
        desc: "停手多少秒后执行自动写盘；仅在上面的开关开启时生效。",
    },
    StaticRow {
        page: SettingsPage::Save,
        key: "保存时备份",
        title: "保存时备份",
        desc: "覆盖已有文件前把磁盘旧版复制一份：「覆盖式」= 同目录 name.bak；「时间戳历史」= name.bak.d 目录内逐次留存；超过 64MB 的文件自动跳过，备份失败不阻断保存。",
    },
    StaticRow {
        page: SettingsPage::Session,
        key: "记住最近打开的文件",
        title: "记住最近打开的文件",
        desc: "在「最近打开」保留历史；关闭开关会一并清空存量记录。",
    },
    StaticRow {
        page: SettingsPage::Session,
        key: "会话快照",
        title: "会话快照",
        desc: "关窗时自动保存未存内容，异常退出后可恢复。",
    },
    StaticRow {
        page: SettingsPage::Session,
        key: "启动时恢复上次界面",
        title: "启动时恢复上次界面",
        desc: "启动时还原上次的标签页与内容。",
    },
    StaticRow {
        page: SettingsPage::Session,
        key: "关窗行为",
        title: "关窗行为",
        desc: "「快照直退」不打断；「每次询问」先确认未保存内容。",
    },
    StaticRow {
        page: SettingsPage::Session,
        key: "快照心跳间隔（秒）",
        title: "快照心跳间隔（秒）",
        desc: "后台周期保存快照的间隔。",
    },
    StaticRow {
        page: SettingsPage::About,
        key: "名称",
        title: "名称",
        desc: "Editpad —— 轻量文本编辑器。",
    },
    StaticRow {
        page: SettingsPage::About,
        key: "版本",
        title: "版本",
        desc: env!("CARGO_PKG_VERSION"),
    },
    StaticRow {
        page: SettingsPage::About,
        key: "渲染后端",
        title: "渲染后端",
        desc: "tiny-skia 软渲染（P41 内存取舍：进程内存约为 GPU 路径的 1/12）。",
    },
    StaticRow {
        page: SettingsPage::About,
        key: "开源协议",
        title: "开源协议",
        desc: "Apache-2.0",
    },
];

/// 静态设置行清单（P62 起**不含热键行**——热键页为动态行，由
/// view::rows_for 按动作注册表与当前重映射构建）。顺序 = 分类内
/// 自上而下的展示顺序；搜索过滤与行渲染都从这里出发。
pub(crate) fn settings_rows() -> impl Iterator<Item = SettingsRow> {
    SETTINGS_ROWS.iter().map(|r| SettingsRow {
        page: r.page,
        key: r.key.to_owned(),
        title: r.title.to_owned(),
        desc: r.desc.to_owned(),
    })
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

/// P112：标签条页签「胶囊」容器样式——沿用 [`chrome_nav_button_style`]
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
pub(crate) fn chrome_menu_item_style(theme: &Theme, status: button::Status) -> button::Style {    let sc = settings_colors(theme);
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

// ---------- 设置面板渲染（第 82 轮 Phase 2a：自 view.rs 迁入） ----------

impl Editpad {
    /// P47 设置弹窗面板：两栏布局——顶部标题行（+ 右上角
    /// 关闭），下方左侧「搜索 + 分类导航」、右侧「标题 + 灰描述 + 右对齐
    /// 控件」的行式列表（行间 1px 分隔线），内容区内部滚动。设置项与
    /// P27 完全一致（消息不改，只重排展示），改动即写回的语义不变。
    ///
    /// `content_h`：内容滚动区高度（来自 `settings_card_size`，随窗口钳制）。
    fn settings_panel(&self, content_h: f32) -> Element<'_, Message> {
        // P33/P36：UI 与正文同族，字号固定不随正文缩放；P34：族随设置
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let sc = settings_colors(&self.theme());

        // 标题行：「设置」+ 右上角 ×（关闭按钮放在标题栏右侧）
        let header = container(
            row![
                text("设置").size(uipx * 1.25).font(uifont),
                container(
                    button(text("×").size(uipx).font(uifont))
                        .padding([2, 9])
                        .style(chrome_button_style)
                        .on_press(Message::SettingsToggled),
                )
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Right),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([10, 14])
        .width(Fill);

        // 显式统一生命周期：分隔线是 'static 元素，收进本地生命周期的列
        let header_sep: Element<'_, Message> = settings_separator(sc);
        let divider: Element<'_, Message> = settings_divider(sc);

        column![
            header,
            header_sep,
            row![
                self.settings_sidebar(),
                divider,
                self.settings_content(content_h),
            ]
            .align_y(iced::alignment::Vertical::Top),
        ]
        .into()
    }

    /// 左侧栏（P47）：搜索框 + 「选项」小标 + 分类导航。选中项 = 底色
    /// 加 1px 描边高亮（同款）；搜索进行中不高亮（内容区已是
    /// 跨分类的命中结果）。
    fn settings_sidebar(&self) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let sc = settings_colors(&self.theme());
        let searching = !self.settings_search.trim().is_empty();

        let mut nav = column![].spacing(2);
        for page in SettingsPage::ALL {
            let selected = !searching && self.settings_page == page;
            nav = nav.push(
                button(
                    container(text(page.title()).size(uipx).font(uifont))
                        .width(Fill)
                        .align_x(iced::alignment::Horizontal::Left),
                )
                .width(Fill)
                .padding([5, 10])
                .style(move |theme, status| {
                    chrome_nav_button_style(theme, status, selected)
                })
                .on_press(Message::SettingsPageSelected(page)),
            );
        }

        column![
            text_input("搜索设置…", &self.settings_search)
                .size(uipx)
                .font(uifont)
                .on_input(Message::SettingsSearchChanged)
                .style(settings_input_style)
                .width(Fill),
            text("选项").size(uipx * 0.85).font(uifont).color(sc.desc),
            nav,
        ]
        .spacing(10)
        .padding(Padding { top: 12.0, right: 10.0, bottom: 12.0, left: 12.0 })
        .width(170)
        .into()
    }

    /// 右侧内容区（P47）：无搜索词 = 当前分类页（页首标题 + 行列表）；
    /// 有搜索词 = 跨分类的命中行，按分类分组展示（同款），
    /// 无命中给出提示。整列由外层 scrollable 钳高滚动。
    fn settings_content(&self, content_h: f32) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let sc = settings_colors(&self.theme());
        let query = self.settings_search.trim();
        let current_page = self.settings_page;

        let mut list = column![].spacing(0);
        if query.is_empty() {
            // 分类浏览：页首标题 + 该页全部行
            list = list.push(
                container(
                    text(current_page.title())
                        .size(uipx * 1.25)
                        .font(uifont),
                )
                .padding(Padding { top: 4.0, right: 4.0, bottom: 8.0, left: 4.0 }),
            );
            // P62：热键页头部带「全部恢复默认」与说明
            if current_page == SettingsPage::Hotkeys {
                list = list.push(
                    container(
                        row![
                            button(text("全部恢复默认").size(uipx).font(uifont))
                                .padding([3, 12])
                                .style(chrome_button_style)
                                .on_press_maybe(
                                    (!self.busy).then_some(Message::HotkeysResetAll)
                                ),
                            text("修改后立即生效并写入 config.toml")
                                .size(uipx * 0.85)
                                .font(uifont)
                                .color(sc.desc),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                .padding(Padding { top: 0.0, right: 4.0, bottom: 6.0, left: 4.0 }),
                );
            }
            for r in self.rows_for(current_page) {
                list = list.push(self.settings_row_widget(r));
            }
        } else {
            // 搜索：按分类顺序分组展示命中行
            let mut total_hits = 0;
            for page in SettingsPage::ALL {
                let hits: Vec<SettingsRow> = self
                    .rows_for(page)
                    .into_iter()
                    .filter(|r| settings_search_hit(query, &r.title, &r.desc))
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                total_hits += hits.len();
                list = list.push(
                    container(
                        text(page.title())
                            .size(uipx * 0.9)
                            .font(uifont)
                            .color(sc.desc),
                    )
                    .padding(Padding { top: 8.0, right: 4.0, bottom: 2.0, left: 4.0 }),
                );
                for r in hits {
                    list = list.push(self.settings_row_widget(r));
                }
            }
            if total_hits == 0 {
                list = list.push(
                    container(
                        text(format!("没有匹配「{query}」的设置"))
                            .size(uipx)
                            .font(uifont)
                            .color(sc.desc),
                    )
                    .padding([12, 4]),
                );
            }
        }
        scrollable(
            container(list)
                .padding(Padding { top: 4.0, right: 16.0, bottom: 16.0, left: 16.0 })
                .width(Fill),
        )
            .width(Fill)
            .height(content_h)
            .into()
    }

    /// 某分类页的全部设置行（P62）：热键页为**动态行**——标题 = 动作
    /// 说明、描述 = 当前生效组合（随重映射变化），其余页走静态目录。
    pub(crate) fn rows_for(&self, page: SettingsPage) -> Vec<SettingsRow> {
        if page == SettingsPage::Hotkeys {
            return HOTKEY_ACTIONS
                .iter()
                .map(|a| SettingsRow {
                    page,
                    key: a.id.to_owned(),
                    title: a.desc.to_owned(),
                    desc: effective_combo(a.id, &self.settings.hotkeys)
                        .unwrap_or(a.default_combo)
                        .to_owned(),
                })
                .collect();
        }
        settings_rows().filter(|r| r.page == page).collect()
    }

    /// 单个设置行（P47）：左「标题 + 灰色描述」、右对齐控件，行下 1px
    /// 分隔线（行式布局）。「正文字体」行下方附带字体挑选块。
    fn settings_row_widget(&self, r: SettingsRow) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let sc = settings_colors(&self.theme());

        let left = column![
            text(r.title).size(uipx).font(uifont),
            text(r.desc).size(uipx * 0.85).font(uifont).color(sc.desc),
        ]
        .spacing(2)
        .width(Fill);

        let mut body = row![left].spacing(12).align_y(Alignment::Center);
        if let Some(control) = self.settings_row_control(&r.key) {
            body = body.push(control);
        }

        let mut cell = column![container(body).width(Fill).padding([10, 4])];
        // 正文字体行附带过滤框 + 候选列表（P34 的选择 UI 原样收编）
        if r.key == FONT_ROW_KEY {
            cell = cell.push(self.settings_font_picker());
        }
        let sep: Element<'_, Message> = settings_separator(sc);
        cell = cell.push(sep);
        cell.into()
    }

    /// 按行键构建右侧控件（P47）；返回 None = 纯展示行（关于页）。
    /// 控件消息与 P27 完全一致，写回语义不变。
    pub(crate) fn settings_row_control(&self, key: &str) -> Option<Element<'_, Message>> {
        let s = &self.settings;
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();

        // P62 热键行（key = 动作 id）：修改按钮 / 捕获中提示
        if HOTKEY_ACTIONS.iter().any(|a| a.id == key) {
            if self.hotkey_capture == Some(key) {
                let sc = settings_colors(&self.theme());
                return Some(
                    text("按下新组合键…（Esc 取消）")
                        .size(uipx)
                        .font(uifont)
                        .color(sc.accent)
                        .into(),
                );
            }
            return Some(
                button(text("修改").size(uipx).font(uifont))
                    .padding([3, 12])
                    .style(chrome_button_style)
                    .on_press_maybe(
                        (!self.busy)
                            .then(|| {
                                HOTKEY_ACTIONS
                                    .iter()
                                    .find(|a| a.id == key)
                                    .map(|a| Message::HotkeyCaptureStarted(a.id))
                            })
                            .flatten(),
                    )
                    .into(),
            );
        }

        let control: Element<'_, Message> = match key {
            // ---- 外观 ----
            "主题" => button(
                text(if s.is_dark() { "深色" } else { "浅色" })
                    .size(uipx)
                    .font(uifont),
            )
            .padding([3, 12])
            .style(chrome_button_style)
            .on_press(Message::ThemeToggled)
            .into(),
            "字号" => self.settings_stepper(
                format!("{:.0}", self.display_font_size()),
                (self.display_font_size()
                    > editpad_core::settings::MIN_FONT_SIZE)
                    .then_some(Message::FontSizeDelta(-editor::FONT_ZOOM_STEP)),
                (self.display_font_size()
                    < editpad_core::settings::MAX_FONT_SIZE)
                    .then_some(Message::FontSizeDelta(editor::FONT_ZOOM_STEP)),
            ),
            // ---- 字体 ----
            FONT_ROW_KEY => button(text("回退默认").size(uipx).font(uifont))
                .padding([3, 12])
                .style(chrome_button_style)
                .on_press_maybe(
                    s.font_family.is_some().then_some(Message::SettingsFontReset),
                )
                .into(),
            // ---- 保存 ----
            settings_ui::AUTOSAVE_ROW_KEY => checkbox(s.autosave_enabled)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsAutosaveToggled)
                .into(),
            "自动写盘延迟（秒）" => self.settings_stepper(
                format!("{}s", s.autosave_delay_secs),
                (s.autosave_delay_secs
                    > editpad_core::settings::MIN_AUTOSAVE_DELAY_SECS)
                    .then_some(Message::SettingsAutosaveDelayDelta(-1)),
                (s.autosave_delay_secs
                    < editpad_core::settings::MAX_AUTOSAVE_DELAY_SECS)
                    .then_some(Message::SettingsAutosaveDelayDelta(1)),
            ),
            // ---- 第 64 轮 ⑭：保存时备份（三态循环按钮，仿关窗行为） ----
            "保存时备份" => button(
                text(match s.backup_mode.as_str() {
                    editpad_core::settings::BACKUP_MODE_SIMPLE => "覆盖式 name.bak",
                    editpad_core::settings::BACKUP_MODE_TIMESTAMPED => "时间戳历史",
                    _ => "关闭",
                })
                .size(uipx)
                .font(uifont),
            )
            .padding([3, 12])
            .style(chrome_button_style)
            .on_press(Message::SettingsBackupModeToggled)
            .into(),
            // ---- 会话与隐私 ----
            "显示空白字符" => checkbox(s.show_whitespace)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsShowWhitespaceToggled)
                .into(),
            "显示行尾符" => checkbox(s.show_line_endings)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsShowLineEndingsToggled)
                .into(),
            // ---- 第 73 轮 ⑯：自动换行（软换行） ----
            "自动换行" => checkbox(s.word_wrap)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsWordWrapToggled)
                .into(),
            "记住最近打开的文件" => checkbox(s.remember_recent_files)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsRememberRecentToggled)
                .into(),
            "会话快照" => checkbox(s.enable_snapshots)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsSnapshotsToggled)
                .into(),
            "启动时恢复上次界面" => checkbox(s.remember_session)
                .style(settings_checkbox_style)
                .on_toggle(Message::SettingsRememberSessionToggled)
                .into(),
            "关窗行为" => button(
                text(if s.exit_mode == editpad_core::settings::EXIT_MODE_SNAPSHOT {
                    "快照直退"
                } else {
                    "每次询问"
                })
                .size(uipx)
                .font(uifont),
            )
            .padding([3, 12])
            .style(chrome_button_style)
            .on_press(Message::SettingsExitModeToggled)
            .into(),
            "快照心跳间隔（秒）" => self.settings_stepper(
                format!("{}s", s.snapshot_interval_secs),
                (s.snapshot_interval_secs
                    > editpad_core::settings::MIN_SNAPSHOT_INTERVAL_SECS)
                    .then_some(Message::SettingsIntervalDelta(-5)),
                (s.snapshot_interval_secs
                    < editpad_core::settings::MAX_SNAPSHOT_INTERVAL_SECS)
                    .then_some(Message::SettingsIntervalDelta(5)),
            ),
            // 热键速查 / 关于：纯展示行（标题+描述已完整表达）
            _ => return None,
        };
        Some(control)
    }

    /// 「− 值 +」步进器（P47 右对齐控件；越界方向按钮禁用置灰）。
    fn settings_stepper(
        &self,
        value: String,
        dec: Option<Message>,
        inc: Option<Message>,
    ) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let mk = |label: &str, msg: Option<Message>| {
            button(text(label.to_owned()).size(uipx).font(uifont))
                .padding([2, 9])
                .style(chrome_button_style)
                .on_press_maybe(msg)
        };
        // 值列定宽居中（P51 打磨）：数值变宽（如 2s→60s）时 ± 按钮不再
        // 左右跳动，整列右缘与其他行控件保持对齐
        row![
            mk("−", dec),
            container(text(value).size(uipx).font(uifont))
                .width(48)
                .align_x(iced::alignment::Horizontal::Center),
            mk("+", inc),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    /// 「正文字体」行的附属块（P47 收编 P34 选择 UI）：当前生效说明 +
    /// 过滤框 + 候选列表（数据源 = 启动期 fontdb 枚举，名字即选即用）。
    fn settings_font_picker(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let sc = settings_colors(&self.theme());

        let current = text(match (&s.font_family, &self.active_font_family) {
            (Some(cfg), Some(eff)) if cfg.as_str() == *eff => {
                format!("当前：{eff}")
            }
            (Some(cfg), _) => {
                format!("当前：默认等宽（配置的「{cfg}」未安装）")
            }
            (None, _) => "当前：默认（等宽）".to_owned(),
        })
        .size(uipx * 0.85)
        .font(uifont)
        .color(sc.desc);

        let filter = text_input("输入关键字过滤字体", &self.font_filter)
            .size(uipx)
            .font(uifont)
            .on_input(Message::FontFilterChanged)
            .style(settings_input_style)
            .width(Fill);

        // 显式标注：两个分支的 widget 类型不同，靠 Into 目标统一
        let picker: Element<'_, Message> = if self.available_fonts.is_empty() {
            text("无法枚举系统字体（保持默认等宽）")
                .size(uipx)
                .font(uifont)
                .color([0.7, 0.4, 0.1])
                .into()
        } else {
            let needle = editor::normalize_family(&self.font_filter);
            let matches: Vec<&String> = self
                .available_fonts
                .iter()
                .filter(|f| {
                    needle.is_empty()
                        || editor::normalize_family(f).contains(&needle)
                })
                .collect();
            let total = matches.len();
            let mut list = column![].spacing(2);
            for name in matches.iter().take(FONT_PICKER_MAX_ROWS) {
                list = list.push(
                    button(
                        container(text(name.as_str()).size(uipx).font(uifont))
                            .width(Fill)
                            .align_x(iced::alignment::Horizontal::Left),
                    )
                    .width(Fill)
                    .padding([4, 8])
                    .style(chrome_menu_item_style)
                    .on_press(Message::SettingsFontSelected(
                        (*name).clone(),
                    )),
                );
            }
            if total > FONT_PICKER_MAX_ROWS {
                list = list.push(
                    text(format!(
                        "…共 {total} 个命中，请继续输入关键字缩小范围"
                    ))
                    .size(uipx * 0.85)
                    .font(uifont)
                    .color(sc.desc),
                );
            }
            scrollable(list).height(180).width(Fill).into()
        };

        column![current, filter, picker]
            .spacing(6)
            .padding(Padding { top: 0.0, right: 4.0, bottom: 10.0, left: 4.0 })
            .into()
    }

    

    /// P40：设置弹窗浮层——整窗背板（点击关闭）+ 居中卡片。P47 起卡片
    /// 为 两栏布局：侧栏固定、内容区内部滚动，宽 720、
    /// 高按窗口钳制（`settings_card_size`）。opaque 卡片让点击卡片
    /// 空白处（padding/标题行旁）不误触背板关闭。
    /// 注：第 82 轮 Phase 2a 自 view.rs 迁入，view() 跨模块调用 → pub(crate)。
    pub(crate) fn settings_overlay(&self) -> Element<'_, Message> {
        let (card_w, content_h) =
            settings_card_size(self.viewport_size.0, self.viewport_size.1);
        let card = opaque(
            container(self.settings_panel(content_h))
                .width(card_w)
                .style(settings_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(Alignment::Center)
                .padding(16),
        )
        .on_press(Message::SettingsToggled)
        .into()
    }

}
