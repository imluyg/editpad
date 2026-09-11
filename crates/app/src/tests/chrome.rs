use super::*;

// ---------- P153：查找框淡出（点正文后查找框转半透明） ----------

/// 淡出系数是查找框内所有控件的**单一来源**：1.0 = 不淡出，
/// 淡出 = [`FIND_CARD_DIM_ALPHA`]（用户点单「保留文字可读」≈0.55）。
#[test]
fn p153_find_dim_factor_pins_two_states() {
    assert_eq!(find_dim_factor(false), 1.0, "不淡出 = 原样（系数 1.0）");
    assert_eq!(find_dim_factor(true), FIND_CARD_DIM_ALPHA);
    assert!(
        (FIND_CARD_DIM_ALPHA - 0.4).abs() < 1e-6,
        "透明度契约：0.4（第三轮用户点单「所有元素一起更淡」）"
    );
}

/// 用户复报（第二轮）：**输入框/替换框/按钮也必须一起半透明**——它们自带
/// 底色（输入框主题底 / 按钮白底 / 勾选点缀色），只压卡片背板会留下
/// 「实心白块浮在淡卡片上」。
///
/// 口径：系数 ≥ 1.0 时与既有样式**逐字段等价**（关闭态零行为变化）；
/// 淡出时各自底色的 alpha 严格变小且 ≈ 原值 × 0.55。
#[test]
fn p153_dim_styles_dampen_alpha_and_are_identity_when_off() {
    let theme = Theme::Light;

    // ①输入框：默认样式（主题底）→ 不淡出逐字段等价；淡出底色 alpha 缩小
    let input_off = find_input_style(&theme, text_input::Status::Active, find_dim_factor(false));
    let input_default = text_input::default(&theme, text_input::Status::Active);
    assert_eq!(
        input_off.background, input_default.background,
        "不淡出 = iced 默认输入框底色"
    );
    assert_eq!(input_off.value, input_default.value);
    assert_eq!(input_off.border.color, input_default.border.color);

    let input_dim = find_input_style(&theme, text_input::Status::Active, FIND_CARD_DIM_ALPHA);
    let (Background::Color(off_bg), Background::Color(dim_bg)) =
        (input_off.background, input_dim.background)
    else {
        panic!("输入框底色应为纯色");
    };
    assert!(
        (dim_bg.a - off_bg.a * FIND_CARD_DIM_ALPHA).abs() < 1e-6,
        "输入框底色 alpha 应按系数压：{} -> {}",
        off_bg.a,
        dim_bg.a
    );
    assert!(dim_bg.a < off_bg.a, "输入框底色必须真的变淡");
    assert!(
        (input_dim.value.a - input_off.value.a * FIND_CARD_DIM_ALPHA).abs() < 1e-6,
        "输入框文字色一并变淡"
    );

    // ②按钮：既有 chrome_button_style → 淡出后底色/文字/描边全部变淡
    let btn_off = find_button_style(&theme, button::Status::Active, find_dim_factor(false));
    let btn_default = chrome_button_style(&theme, button::Status::Active);
    assert_eq!(btn_off.background, btn_default.background, "不淡出 = 原按钮底色");
    assert_eq!(btn_off.text_color, btn_default.text_color);
    assert_eq!(btn_off.border.color, btn_default.border.color);

    let btn_dim = find_button_style(&theme, button::Status::Active, FIND_CARD_DIM_ALPHA);
    let (Some(Background::Color(btn_off_bg)), Some(Background::Color(btn_dim_bg))) =
        (btn_off.background, btn_dim.background)
    else {
        panic!("按钮底色应为纯色");
    };
    assert!(
        (btn_dim_bg.a - btn_off_bg.a * FIND_CARD_DIM_ALPHA).abs() < 1e-6,
        "按钮底色 alpha 应按系数压"
    );
    assert!(
        btn_dim.text_color.a < btn_off.text_color.a,
        "按钮文字一并变淡（否则淡框里留着清晰文字，观感割裂）"
    );
    assert!(
        btn_dim.border.color.a < btn_off.border.color.a,
        "按钮描边一并变淡"
    );
    // 悬停态同口径（否则一悬停就跳回不透明的实心块）
    let hover_off = find_button_style(&theme, button::Status::Hovered, 1.0);
    let hover_dim = find_button_style(&theme, button::Status::Hovered, FIND_CARD_DIM_ALPHA);
    let (Some(Background::Color(h_off)), Some(Background::Color(h_dim))) =
        (hover_off.background, hover_dim.background)
    else {
        panic!("悬停底色应为纯色");
    };
    assert!(h_dim.a < h_off.a, "悬停底色同样变淡");
    // 禁用态（如扫描在途的「全部替换」）不得漏掉
    let dis_off = find_button_style(&theme, button::Status::Disabled, 1.0);
    let dis_dim = find_button_style(&theme, button::Status::Disabled, FIND_CARD_DIM_ALPHA);
    assert!(dis_dim.text_color.a < dis_off.text_color.a, "禁用文字同样变淡");
    assert!(dis_dim.border.color.a < dis_off.border.color.a, "禁用描边同样变淡");
}

/// 复选框（区分大小写 / 正则 / 整词）：选中态点缀色底 + **纯白勾**——
/// 勾色不压就是淡出框里唯一的纯白亮点，故三者（底/勾/描边/文字）都要压。
#[test]
fn p153_dim_checkbox_dampens_fill_tick_and_label() {
    let theme = Theme::Light;
    let checked = checkbox::Status::Active { is_checked: true };
    let unchecked = checkbox::Status::Active { is_checked: false };

    let off = find_checkbox_style(&theme, checked, find_dim_factor(false));
    let base = settings_checkbox_style(&theme, checked);
    assert_eq!(off.background, base.background, "不淡出 = 原复选框样式");
    assert_eq!(off.icon_color, base.icon_color);
    assert_eq!(off.text_color, base.text_color);

    let dim = find_checkbox_style(&theme, checked, FIND_CARD_DIM_ALPHA);
    let (Background::Color(off_bg), Background::Color(dim_bg)) = (off.background, dim.background)
    else {
        panic!("复选框底色应为纯色");
    };
    assert!(
        (dim_bg.a - off_bg.a * FIND_CARD_DIM_ALPHA).abs() < 1e-6,
        "选中底（点缀色）alpha 应按系数压"
    );
    assert!(
        dim.icon_color.a < off.icon_color.a,
        "白色勾必须一起变淡"
    );
    assert!(dim.border.color.a < off.border.color.a, "描边一并变淡");
    assert!(
        dim.text_color.unwrap().a < off.text_color.unwrap().a,
        "标签文字一并变淡"
    );

    // 未选中态：控件底 + 描边
    let dim_un = find_checkbox_style(&theme, unchecked, FIND_CARD_DIM_ALPHA);
    let off_un = find_checkbox_style(&theme, unchecked, 1.0);
    let (Background::Color(un_off), Background::Color(un_dim)) =
        (off_un.background, dim_un.background)
    else {
        panic!("未选中底色应为纯色");
    };
    assert!(un_dim.a < un_off.a, "未选中底同样变淡");
    assert!(dim_un.border.color.a < off_un.border.color.a);
}

/// 卡片背板淡出后仍保留 1px 不透明描边（否则整框边界糊进正文）：
/// 形状/圆角/边宽与不淡出态一致，只有背景 alpha 变化。
#[test]
fn p153_dim_card_keeps_border_and_shape_only_fades_background() {
    let theme = Theme::Light;
    let off = popup_card_style(&theme);
    let dim = popup_card_dim_style(&theme);
    assert_eq!(dim.border, off.border, "描边/圆角/边宽不变（边界仍清晰）");
    assert_eq!(dim.shadow, off.shadow);
    let (Some(Background::Color(off_bg)), Some(Background::Color(dim_bg))) =
        (off.background, dim.background)
    else {
        panic!("卡片底色应为纯色");
    };
    assert!((dim_bg.a - FIND_CARD_DIM_ALPHA).abs() < 1e-6);
    assert!(dim_bg.a < off_bg.a);
    // 色相不变（只改 alpha，不改 RGB）
    assert_eq!((dim_bg.r, dim_bg.g, dim_bg.b), (off_bg.r, off_bg.g, off_bg.b));
}

/// P153：纯文本标签（拖动条文案 / 命中计数「无匹配」/ 目录行 / 扫描进度）
/// 的淡出用色——用户复报「无匹配提示没跟着变淡」：这类文本没有底色可压
/// alpha，只能显式给色。`None`（未淡出）= 调用方不设色 → 沿用 iced 默认
/// 文字样式（关闭态逐字等价）。
#[test]
fn p153_label_color_is_none_when_off_and_dampened_when_dim() {
    let theme = Theme::Light;
    assert!(
        find_label_color(&theme, find_dim_factor(false)).is_none(),
        "不淡出必须返回 None（不设色 = 默认文字样式）"
    );
    let base = theme.palette().text;
    let dim = find_label_color(&theme, FIND_CARD_DIM_ALPHA).expect("淡出应给出标签色");
    assert!(
        (dim.a - base.a * FIND_CARD_DIM_ALPHA).abs() < 1e-6,
        "标签色 alpha 应按系数压：{} -> {}",
        base.a,
        dim.a
    );
    assert_eq!(
        (dim.r, dim.g, dim.b),
        (base.r, base.g, base.b),
        "只改 alpha，色相不变"
    );
    assert!(dim.a < base.a, "标签必须真的变淡");
}

/// 用户复报（第三轮）：**点半透明查找框里的输入框没有恢复不透明**。
///
/// 根因 = `iced_widget::MouseArea::update` 先派发给内容、内容已捕获事件时
/// 不再自查（上游 `is_event_captured` 早退）；查找卡片里的输入框/按钮都会
/// 捕获按下事件，于是卡片上的 `mouse_area.on_press` 收不到这类点击。
/// [`PressObserver`] 反过来：先派发内容、**无条件自查**。
///
/// 本用例用「会捕获事件并回消息的自绘内容」模拟输入框：断言观察层在事件
/// 已被消费的情况下**仍然**发消息——把实现换回 `mouse_area` 本用例即变红。
#[test]
fn p153_press_observer_reports_press_even_when_content_captures() {
    use iced::advanced::clipboard::Null;
    use iced::advanced::layout::{self, Layout};
    use iced::advanced::widget::{Tree, Widget};
    use iced::advanced::Shell;
    use iced::{mouse, Font, Pixels, Point, Rectangle, Size};

    /// 模拟输入框/按钮：左键在自身范围内按下即「捕获事件 + 回一条消息」
    /// ——与 iced 内建 `text_input` / `button` 对按下的处理同口径。
    struct CapturingContent;

    impl Widget<Message, Theme, iced::Renderer> for CapturingContent {
        fn size(&self) -> Size<iced::Length> {
            Size::new(Fill, Fill)
        }

        fn layout(
            &mut self,
            _tree: &mut Tree,
            _renderer: &iced::Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            layout::Node::new(limits.max())
        }

        fn update(
            &mut self,
            _tree: &mut Tree,
            event: &iced::Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            _renderer: &iced::Renderer,
            _clipboard: &mut dyn iced::advanced::Clipboard,
            shell: &mut Shell<'_, Message>,
            _viewport: &Rectangle,
        ) {
            if matches!(
                event,
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            ) && cursor.is_over(layout.bounds())
            {
                // 内容先回自己的消息并消费事件（= mouse_area 会早退的那种情形）
                shell.publish(crate::Message::FindBoxPressed);
                shell.capture_event();
            }
        }

        fn draw(
            &self,
            _tree: &Tree,
            _renderer: &mut iced::Renderer,
            _theme: &Theme,
            _style: &iced::advanced::renderer::Style,
            _layout: Layout<'_>,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
        ) {
        }
    }

    let content: Element<'_, Message> = Element::new(CapturingContent);
    let mut element: Element<'_, Message> =
        PressObserver::new(content, Message::EditorBodyPressed).into_element();
    let mut tree = Tree::new(element.as_widget());
    let renderer = iced::Renderer::new(Font::MONOSPACE, Pixels(16.0));
    let limits = layout::Limits::new(Size::new(200.0, 100.0), Size::new(200.0, 100.0));
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);

    let mut messages: Vec<Message> = Vec::new();
    let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let mut shell = Shell::new(&mut messages);
    let mut clipboard = Null;
    element.as_widget_mut().update(
        &mut tree,
        &press,
        root,
        mouse::Cursor::Available(Point::new(100.0, 50.0)),
        &renderer,
        &mut clipboard,
        &mut shell,
        &Rectangle::INFINITE,
    );

    assert!(
        shell.is_event_captured(),
        "内容应已消费该按下事件（模拟输入框捕获）——这正是 mouse_area 的盲区"
    );
    assert!(
        matches!(
            messages.as_slice(),
            [Message::FindBoxPressed, Message::EditorBodyPressed]
        ),
        "内层（捕获者）先回消息，观察层的消息随后——事件已被消费也必须上报；实际 {messages:?}"
    );

    // 范围外按下：不上报，也**不得消费**（正文的按下要照旧下传到正文控件）
    let mut outside: Vec<Message> = Vec::new();
    {
        let mut shell = Shell::new(&mut outside);
        element.as_widget_mut().update(
            &mut tree,
            &press,
            root,
            mouse::Cursor::Available(Point::new(500.0, 500.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::INFINITE,
        );
        assert!(
            !shell.is_event_captured(),
            "范围外按下不得消费——否则正文收不到自己的点击（选区/光标/滚动条全废）"
        );
    }
    assert!(outside.is_empty(), "范围外按下不应上报，实际 {outside:?}");
}

/// 用户复报（第四轮）：**「旧 bug 又回来了」**——在查找框里打拼音时，
/// 组字串既画在查找框里、又出现在正文上（P151 修过的 IME 双份症状），
/// 上屏还可能落进文档。
///
/// 根因：查找卡片浮在正文自绘控件**之上**，而正文控件的 bounds 是整个
/// 编辑区，它把「区内按下」当作**接管键盘/IME 焦点**的信号
/// （`EditorView` 的 `pointer_focus`）。`Stack` 自上而下派发、**未被消费
/// 才继续往下**：观察层只上报不消费时，点查找框的输入框会继续下传到正文
/// → 正文重新持焦并请求 IME → 组字串两处各画一份。
///
/// 本用例用「**不消费**事件的静态内容」（模拟卡片空白/拖动条之外的区域）
/// 断言观察层**自己**消费了按下；范围外则不得消费（正文照旧收得到点击）。
/// 去掉 `capture_event` 本用例立刻变红。
#[test]
fn p153_press_observer_captures_press_so_editor_cannot_steal_ime_focus() {
    use iced::advanced::layout::{self, Layout};
    use iced::advanced::widget::{Tree, Widget};
    use iced::advanced::Shell;
    use iced::{mouse, Font, Pixels, Rectangle, Size};

    /// 静态内容：不处理任何事件（模拟卡片里不会消费按下的表面）。
    struct StaticContent;

    impl Widget<Message, Theme, iced::Renderer> for StaticContent {
        fn size(&self) -> Size<iced::Length> {
            Size::new(Fill, Fill)
        }

        fn layout(
            &mut self,
            _tree: &mut Tree,
            _renderer: &iced::Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            layout::Node::new(limits.max())
        }

        fn draw(
            &self,
            _tree: &Tree,
            _renderer: &mut iced::Renderer,
            _theme: &Theme,
            _style: &iced::advanced::renderer::Style,
            _layout: Layout<'_>,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
        ) {
        }
    }

    let content: Element<'_, Message> = Element::new(StaticContent);
    let mut element: Element<'_, Message> =
        PressObserver::new(content, Message::FindBoxPressed).into_element();
    let mut tree = Tree::new(element.as_widget());
    let renderer = iced::Renderer::new(Font::MONOSPACE, Pixels(16.0));
    let limits = layout::Limits::new(Size::new(200.0, 100.0), Size::new(200.0, 100.0));
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);

    let mut messages: Vec<Message> = Vec::new();
    let mut clipboard = iced::advanced::clipboard::Null;
    {
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            root,
            mouse::Cursor::Available(Point::new(10.0, 10.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::INFINITE,
        );
        assert!(
            shell.is_event_captured(),
            "卡片内的按下必须被消费：否则事件继续下传到正文控件，正文会重新持焦并请求 IME，\
             组字串在查找框与正文各画一份（用户复报的「旧 bug 又回来了」）"
        );
    }
    assert!(
        matches!(messages.as_slice(), [Message::FindBoxPressed]),
        "卡片内的按下要上报，实际 {messages:?}"
    );

    // 范围外：不消费（否则正文点击全废）
    let mut outside: Vec<Message> = Vec::new();
    {
        let mut shell = Shell::new(&mut outside);
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            root,
            mouse::Cursor::Available(Point::new(400.0, 400.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::INFINITE,
        );
        assert!(!shell.is_event_captured(), "范围外不得消费");
    }
    assert!(outside.is_empty());
}

/// P153：**点正文**必须让应用收到 `EditorBodyPressed`——查找框据此整框转半透明。
///
/// 本用例是对上面两条 `p153_press_observer_*` 的**端到端补位**：那两条手工
/// 拼装 `PressObserver` 与模拟内容，验的是观察层自身的语义；而「真实
/// `view()` 里正文控件是否真的把按下转成了这条消息」此前无人验证——P153
/// 恰被复报 5 轮，病根正在这种「测试跑的不是用户那棵树」。
///
/// 经 [`ViewTree`] 走**生产视图树 + 真实事件**。
#[test]
fn p153_clicking_editor_body_emits_press_message_through_view_tree() {
    let mut app = Editpad::default();
    app.viewport_size = (1024.0, 768.0);
    let mut ui = ViewTree::layout_default(&app);

    let body = ui.editor_body();
    let hit = ui.click(body.center());
    assert!(
        hit.iter().any(|m| matches!(m, Message::EditorBodyPressed)),
        "点正文必须发出 EditorBodyPressed（查找框据此淡出）；实际 {hit:?}"
    );

    // 反证：点菜单栏不得产生这条消息——否则淡出会被误触发
    let menu = ui.bounds_at(&[0, 0, 0]);
    let miss = ui.click(iced::Point::new(menu.x + 10.0, menu.y + menu.height * 0.5));
    assert!(
        !miss.iter().any(|m| matches!(m, Message::EditorBodyPressed)),
        "菜单栏区域的点击不得触发正文淡出；实际 {miss:?}"
    );
}
