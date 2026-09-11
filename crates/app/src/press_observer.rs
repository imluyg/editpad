use super::*;

/// P153：**点击观察层**——原样转发内容的一切行为，另在「左键在自身
/// 范围内按下」时发一条消息，**无论该事件是否已被内容消费**，并**消费
/// 该事件**（不再向更下层传播）。
///
/// 为什么不用 `mouse_area`：`iced_widget::MouseArea` 的 `update` 是
/// 「先派发给内容，再 `if shell.is_event_captured() { return; }` 自查」
/// ——查找卡片内的输入框/按钮/复选框都会捕获按下事件，于是卡片上的
/// `mouse_area.on_press` 收不到「点输入框」这一类点击（用户复报：
/// **点半透明查找框里的输入框，框没有恢复不透明**）。
///
/// 本控件把顺序反过来：**先派发给内容，再无条件自查**（不看捕获位），
/// 因此卡片内任何位置（输入框、按钮、拖动条、空白）的按下都能被观察到。
///
/// **为什么还要 `capture_event`**（用户复报「旧 bug 又回来了」的根因）：
/// 查找卡片浮在正文自绘控件**之上**，但正文控件的 bounds 仍是整个编辑区
/// ——它把「区内按下」当作接管键盘/IME 焦点的信号（`EditorView` 的
/// `pointer_focus`，见 editor/view.rs 的 `ButtonPressed` 臂）。而 `Stack`
/// 的派发是自上而下、**未被消费才继续往下**：观察层若不消费，点查找框的
/// 输入框会继续下传到正文控件 → 正文重新持焦并请求 IME → 拼音组字串在
/// **查找框与正文各画一份**（P151 复报过的症状），上屏还可能落进文档。
/// 消费该按下 = 与 P153 之前的卡片实现（`mouse_area` 在其 `on_press` 里
/// `capture_event`）及更早基线一致。
///
/// 自身消费事件不影响框内控件：内容（输入框/按钮/拖动条）在①里已经拿到
/// 并处理过该按下。
pub(crate) struct PressObserver<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_press: Message,
}

impl<'a, Message, Theme, Renderer> PressObserver<'a, Message, Theme, Renderer> {
    /// 包住 `content`：内容范围内左键按下时额外发 `on_press`。
    pub(crate) fn new(
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
        on_press: Message,
    ) -> Self {
        Self {
            content: content.into(),
            on_press,
        }
    }

    /// 收尾成 [`Element`]（视图树里直接 `PressObserver::new(..).into_element()`）。
    pub(crate) fn into_element(self) -> Element<'a, Message, Theme, Renderer>
    where
        Message: 'a + Clone,
        Theme: 'a,
        Renderer: 'a + iced::advanced::Renderer,
    {
        Element::new(self)
    }
}

impl<Message, Theme, Renderer> iced::advanced::Widget<Message, Theme, Renderer>
    for PressObserver<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<iced::advanced::widget::Tree> {
        vec![iced::advanced::widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> iced::Size<iced::Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        // ①内容先处理（按钮点击、输入框聚焦、光标定位……一切原有行为）
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );

        // ②再看是否需要额外上报：**刻意不检查 `shell.is_event_captured()`**
        // ——内容（输入框/按钮）消费了按下事件正是本控件要覆盖的盲区。
        if matches!(
            event,
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Left
            ))
        ) && cursor.is_over(layout.bounds())
        {
            shell.publish(self.on_press.clone());
            // ③并**消费**该按下：查找卡片浮在正文控件之上，若继续下传，
            // 正文会把「区内按下」当成接管焦点的信号（重新请求 IME →
            // 组字串在查找框与正文各画一份，见类型注释）。框内控件的原有
            // 交互已在①完成，此处消费不影响它们。
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &Renderer,
    ) -> iced::mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        renderer_style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            renderer_style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'b>,
        renderer: &Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
    ) -> Option<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
