use super::*;

/// S-2 第一步的**穷尽**回归：Esc（`BarsDismissed`）必须收起所有临时裁决与浮层。
/// 逐字段点亮、再逐字段断言清零——新增一种浮层时，本用例就是那条 checklist 的
/// 第二半（第一半在 `dismiss_all_prompts` 里加一行）。历史上这类「漏收一条」
/// 是一轮一轮补出来的（P21/P27/P28/P50/P55/P151 与第 69 轮各补过一次），
/// 从来没有一条用例钉住全集；`encoding_menu`/`eol_menu`/`find_drag`/
/// `menu_bar_open` 四条在本用例之前无人断言过 Esc 会收起它们。
#[test]
fn esc_dismisses_every_transient_prompt() {
    let mut app = Editpad::default();
    let id0 = app.tabs[0].id;
    // ---- 点亮全集（能直接置态的就置态，不依赖各自入口消息） ----
    app.find_visible = true;
    app.goto_visible = true;
    app.recents_visible = true;
    app.palette_visible = true;
    app.settings_visible = true;
    app.confirm_visible = true;
    app.pending_close = true;
    app.open_confirm = Some(PathBuf::from("C:/w/x.txt"));
    app.pending_cli.push_back(PathBuf::from("C:/w/y.txt"));
    app.pending_link_goto = Some(7);
    app.pending_fif_goto = Some((0, 0, 0));
    app.close_tab_confirm = Some(id0);
    app.tab_context_menu = Some(0);
    app.batch_close_confirm = Some(vec![id0]);
    app.menu_bar_open = Some(1);
    app.external_change = Some(vec![id0]);
    app.renaming_tab = Some(id0);
    app.rename_input.push_str("new-name");
    app.encoding_menu = true;
    app.eol_menu = true;
    app.find_status = true;
    app.status = "第 1/3 处匹配".to_owned();
    app.find_drag = Some(FindDrag::Pending);
    // S-2 第二步新并进来的三处（外加一处**故意不并**的，见下方反向断言）
    app.find_all_visible = true;
    app.fif_visible = true;
    app.fif_scan = Some(3);
    app.recover_prompt = Some(editpad_core::snapshot::SessionManifest {
        generation: 7,
        tabs: Vec::new(),
        active: 0,
        next_untitled: 0,
        clean_exit: false,
    });
    app.column_editor_visible = true;

    dispatch(&mut app, Message::BarsDismissed);

    // ---- 断言全集清零 ----
    assert!(!app.find_visible, "查找栏");
    assert!(!app.find_all_visible, "「查找全部」结果面板（P82）");
    assert!(!app.fif_visible, "FIF 结果面板（A8）");
    assert_eq!(app.fif_scan, None, "关 FIF 面板必须连带取消在途目录扫描");
    assert!(
        app.fif_cancel.load(std::sync::atomic::Ordering::Relaxed),
        "取消标志置位——否则后台线程继续扫一块看不见的面板"
    );
    assert!(
        app.recover_prompt.is_none(),
        "崩溃恢复条随 Esc 收起。注意这只清**提示条**：本出口拿不到快照目录、\
         也不调 clear_session，所以磁盘快照原样留着，下次启动仍会再问一次\
         （真删快照的是〔丢弃〕= discard_session_recover）"
    );
    assert!(
        app.column_editor_visible,
        "列编辑器对话框是模态浮层，Esc 由按键分支顶层优先自收，不走本出口 \
         （并进来越界：点菜单背板会变成关对话框）"
    );
    assert!(!app.goto_visible, "转跳栏");
    assert!(!app.recents_visible, "最近文件");
    assert!(!app.palette_visible, "命令面板");
    assert!(!app.settings_visible, "设置弹窗");
    assert!(!app.confirm_visible, "退出保存确认");
    assert!(!app.pending_close, "关窗标记");
    assert_eq!(app.open_confirm, None, "打开确认");
    assert!(app.pending_cli.is_empty(), "收起打开确认时命令行批次作废");
    assert_eq!(app.pending_link_goto, None, "悬挂的跳行意图一并作废");
    assert_eq!(app.pending_fif_goto, None, "悬挂的 FIF 跳行意图一并作废");
    assert_eq!(app.close_tab_confirm, None, "单页关闭确认");
    assert_eq!(app.tab_context_menu, None, "标签右键菜单");
    assert_eq!(app.batch_close_confirm, None, "批量关闭确认");
    assert_eq!(app.menu_bar_open, None, "顶部菜单栏浮层");
    assert_eq!(app.external_change, None, "外部修改提示条");
    assert_eq!(app.renaming_tab, None, "就地重命名态");
    assert!(app.rename_input.is_empty(), "重命名输入框清空");
    assert!(!app.encoding_menu, "状态栏编码菜单");
    assert!(!app.eol_menu, "状态栏行尾菜单");
    assert!(!app.find_status, "查找进度标记");
    assert!(app.status.is_empty(), "查找进度写过的状态栏文本要一并清");
    assert!(app.find_drag.is_none(), "查找浮层拖动态（P209）");
    assert!(
        !app.busy && app.tabs.len() == 1 && app.tabs[0].id == id0,
        "收起动作不得顺带改文档/页集合状态"
    );
}

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
    assert_eq!(
        btn_off.background, btn_default.background,
        "不淡出 = 原按钮底色"
    );
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
    assert!(
        dis_dim.text_color.a < dis_off.text_color.a,
        "禁用文字同样变淡"
    );
    assert!(
        dis_dim.border.color.a < dis_off.border.color.a,
        "禁用描边同样变淡"
    );
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
    assert!(dim.icon_color.a < off.icon_color.a, "白色勾必须一起变淡");
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
    assert_eq!(
        (dim_bg.r, dim_bg.g, dim_bg.b),
        (off_bg.r, off_bg.g, off_bg.b)
    );
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
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
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
    let node = element
        .as_widget_mut()
        .layout(&mut tree, &renderer, &limits);
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

/// P259：弹层卡片的固定高度必须容得下它自己要画的所有条目。
///
/// 起因是编码弹层：条目从 3 种扩到 7 种（P127 补 Big5/Shift_JIS/EUC-JP/EUC-KR）
/// 时高度还写死 `ITEM_H * 3.0 + 12.0 = 108px`，而容器是固定 `height` 且**没有**
/// `scrollable` ⇒ 后四项被剪在卡片外，且锚点 `ay = vh - card_h - 36` 让剪掉的
/// 部分压在状态栏与窗口下缘之外，表现就是"点了没反应"。
///
/// ⚠️ 本用例能测的是**几何算法与条目表**这两头的关系（单调、退化、条目唯一），
/// **测不到**"有人在别的调用点又写死了一个数"——那一半只能靠 `encoding_menu_overlay`
/// 现在把高度写成从 [`ENCODING_MENU_ITEMS`] 推导这一**结构**保证。真正的像素级
/// 判据（每项 bounds 都在卡片内）留待接进本文件已有的 `ui` 布局夹具。
#[test]
fn status_menu_card_height_covers_every_encoding_item() {
    use crate::view::overlays::{status_menu_card_h, ENCODING_MENU_ITEMS, STATUS_MENU_CARD_PAD};
    // 与 encoding_menu_overlay 内的两个常数同值（改了那边不改这里会红）
    const ITEM_H: f32 = 32.0;
    const SPACING: f32 = 2.0;

    let n = ENCODING_MENU_ITEMS.len();
    assert!(
        n >= 4,
        "自检：编码表只剩 {n} 项，本用例已失去判别力（P127 扩展被删了？）"
    );
    let h = status_menu_card_h(n, ITEM_H, SPACING);
    // 每一项都必须占满一个行高，外加条目间距与上下内边距
    let need = n as f32 * ITEM_H + (n as f32 - 1.0) * SPACING + STATUS_MENU_CARD_PAD * 2.0;
    assert!(
        (h - need).abs() < 1e-4,
        "卡片高 {h} 与按条目算出的 {need} 不符"
    );
    // 改前那个写死值连 4 项都装不下——本条钉住"不再退回那个量级"
    assert!(h > 108.0, "{n} 项的卡片高只有 {h}，回到写死 3 项的老形状了");
    // 条目增减必须带着高度走（结构上同源，这里再钉一次单调性）
    assert!(
        status_menu_card_h(n + 1, ITEM_H, SPACING) > h,
        "多一项高度不长"
    );
    assert!(
        status_menu_card_h(n - 1, ITEM_H, SPACING) < h,
        "少一项高度不降"
    );
    // 退化入参：0 项也要给出一个正的高度（防 (n-1) 下溢成巨大浮点数）
    assert!(
        status_menu_card_h(0, ITEM_H, SPACING) >= ITEM_H,
        "空表退化高度异常"
    );
    // 条目表自身：标签唯一且非空（重复项会让用户以为有两种同名编码）
    let mut labels: Vec<&str> = ENCODING_MENU_ITEMS.iter().map(|(l, _)| *l).collect();
    for l in &labels {
        assert!(!l.is_empty(), "编码表里有空标签");
    }
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), n, "编码表里有同名条目");

    // P259 × P260：弹层条目表必须覆盖 core 的编码全集。少一项就是"那个编码在
    // 界面上不存在"（P259 的病根），多一项（core 已删的编码）同样是病。
    // 这一条把"菜单内容"与"枚举全集"两个真相钉在一起，不再靠人肉对齐。
    for e in editpad_core::SaveEncoding::ALL {
        assert!(
            ENCODING_MENU_ITEMS
                .iter()
                .any(|(l, enc)| *enc == e && *l == e.label()),
            "编码全集里的 {:?}（标签 {:?}）没有对应的弹层条目",
            e,
            e.label()
        );
    }
    assert_eq!(
        n,
        editpad_core::SaveEncoding::ALL.len(),
        "弹层条目数与编码全集条目数不等——有侧漏抄"
    );
}
