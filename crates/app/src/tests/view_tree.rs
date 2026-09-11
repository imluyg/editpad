//! 生产视图树夹具：让测试跑「用户真正看到的那棵树」。
//!
//! ## 为什么需要
//! app 层 500+ 条测试里，绝大多数是**手工拼装组件**或**直接调函数**——测的
//! 不是 `Editpad::view()` 真正产出的那棵树。这留下一个结构性盲区：
//! **用户在真实视图里遇到的问题，测试里不复现**。P153（查找浮层）被复报
//! 5 轮、P115（组字）3 轮、P118（组字整行消失），病根都在这里。
//!
//! 此前只有 P100 一条用例走了全链路（`tests/tabs.rs` 里布局整棵应用树 +
//! 注入真实事件），但它的样板是**一次性的、且把树路径硬编码成
//! `child(0).child(0).child(2)`**——view 结构一变就错位，没人愿意照着写
//! 第二条。本夹具把那套手法提炼为可复用设施。
//!
//! ## 用法
//! ```ignore
//! let app = Editpad::default();
//! let mut ui = ViewTree::layout_default(&app);
//! let strip = ui.find(|b| b.width > 1000.0 && b.height < 40.0).expect("标签条");
//! let messages = ui.double_click(Point::new(strip.x + strip.width - 16.0, strip.center_y()));
//! assert!(messages.iter().any(|m| matches!(m, Message::TabStripBlankPressed)));
//! ```
//! 找不到路径时先 `eprintln!("{}", ui.dump())` 把整棵树打出来看。

use super::*;

use iced::advanced::clipboard::Null as NullClipboard;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::Tree;
use iced::advanced::Shell;
use iced::{Event, Font, Pixels, Point, Rectangle, Size};

/// 默认测试视口（与多数既有用例的 1280×800 对齐）。
pub(crate) const DEFAULT_VIEWPORT: Size = Size::new(1280.0, 800.0);

/// 已布局的生产视图树：可注入事件、可按几何查询节点。
///
/// 生命周期 `'a` 绑定被借用的 `Editpad`——夹具不拥有应用状态，
/// 需要改状态时照常在夹具外用 `dispatch(&mut app, msg)`。
///
/// ## ⚠️ 借用用法：优先「临时值」，别绑定成变量
/// `ViewTree` 内部持有 `Element` / `Tree`（**都实现 `Drop`**），所以借用会
/// 一直活到作用域末尾，NLL 不会提前释放它。于是这样写会编译失败：
/// ```ignore
/// let ui = ViewTree::layout_default(&app);
/// assert!(ui.editor_body().height > 100.0);
/// dispatch(&mut app, Message::FindToggled); // E0502：app 仍被不可变借用
/// ```
/// 正确写法是让夹具作为**临时值**，借用随语句结束即刻释放：
/// ```ignore
/// let body = ViewTree::layout_default(&app).editor_body();   // ✅
/// let card = ViewTree::layout_default(&app).find_layer_card(); // ✅
/// ```
/// 确实要多查几项时，用一个块把借用收紧：
/// ```ignore
/// let (a, b) = {
///     let ui = ViewTree::layout_default(&app);
///     (ui.editor_body(), ui.find_layer_card())
/// };
/// ```
pub(crate) struct ViewTree<'a> {
    element: Element<'a, Message>,
    tree: Tree,
    renderer: iced::Renderer,
    root: layout::Node,
    viewport: Rectangle,
}

impl<'a> ViewTree<'a> {
    /// 按给定视口布局 `app.view()`。
    ///
    /// 下界取 `(0, 0)`：允许 Shrink 收缩——若强制满宽，就测不出「命中面
    /// 缺损」这类问题（P100 的教训：strip 行漏了 `width(Fill)` 时，
    /// 强制满宽会让断言永远为真）。
    pub(crate) fn layout(app: &'a Editpad, viewport: Size) -> Self {
        let mut element = app.view();
        let mut tree = Tree::new(element.as_widget());
        let renderer = iced::Renderer::new(Font::MONOSPACE, Pixels(16.0));
        let limits = layout::Limits::new(Size::new(0.0, 0.0), viewport);
        let root = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
        Self {
            element,
            tree,
            renderer,
            root,
            viewport: Rectangle::with_size(viewport),
        }
    }

    /// 按应用自己的视口布局。
    ///
    /// 用 `app.viewport_size` 而非硬编码尺寸——浮层居中/卡片宽度钳制这类
    /// 契约本来就依赖真实窗口尺寸（P150 的教训）。该字段默认 `(0, 0)`
    /// （还没收到过 Resized 事件），零尺寸会退化成无意义的树，故回退到
    /// [`DEFAULT_VIEWPORT`]。
    pub(crate) fn layout_default(app: &'a Editpad) -> Self {
        let (w, h) = app.viewport_size;
        let size = if w > 0.0 && h > 0.0 {
            Size::new(w, h)
        } else {
            DEFAULT_VIEWPORT
        };
        Self::layout(app, size)
    }

    /// 查找浮层卡片 = 定宽（`view::FIND_CARD_W` ≈ 560）且足够高的那一块。
    ///
    /// 卡片宽度常量是 view 模块私有的，故按区间认卡片——P150 / P152 / P153
    /// 三处一直各写一遍同一区间，收拢到这儿。
    pub(crate) fn find_layer_card(&self) -> Rectangle {
        self.find(|b| (480.0..=600.0).contains(&b.width) && b.height > 80.0)
            .unwrap_or_else(|| panic!("应能找到查找浮层卡片；\n树形：\n{}", self.dump()))
    }

    /// 正文编辑器节点 = 全宽、位于菜单/标签条之下、最高的那一块。
    ///
    /// 把这个几何谓词内置：调用点不必各自拼（P150 起一直散在调用处）。
    pub(crate) fn editor_body(&self) -> Rectangle {
        self.find(|b| b.width > 1000.0 && b.y > 40.0 && b.height > 150.0)
            .expect("应能找到正文编辑器节点")
    }

    /// 根布局节点。
    pub(crate) fn root(&self) -> Layout<'_> {
        Layout::new(&self.root)
    }

    /// 本次布局实际使用的视口。
    ///
    /// 断言「居中」这类契约时要用它、而不是直接读 `app.viewport_size`——
    /// 后者可能是 `(0, 0)`，而夹具已回退到默认视口。
    pub(crate) fn viewport(&self) -> Size {
        self.viewport.size()
    }

    /// 按树路径取 bounds（路径 = 逐层的 child 下标）。
    ///
    /// 保留 P100 的原始手法；**新用例优先用 [`Self::find`]**——硬编码路径
    /// 会随 view 结构变动而错位，谓词查询则不会。
    pub(crate) fn bounds_at(&self, path: &[usize]) -> Rectangle {
        let mut layout = self.root();
        for &i in path {
            layout = layout.child(i);
        }
        layout.bounds()
    }

    /// 深度优先找**第一个**满足谓词的节点 bounds。
    ///
    /// 用几何谓词（如「宽度 > 1000 的那条横带」）代替树路径：view 里插一层
    /// 容器不会让用例变红，只有真正的契约破了才会红。
    pub(crate) fn find(&self, pred: impl Fn(Rectangle) -> bool) -> Option<Rectangle> {
        fn walk(layout: Layout<'_>, pred: &impl Fn(Rectangle) -> bool) -> Option<Rectangle> {
            let bounds = layout.bounds();
            if pred(bounds) {
                return Some(bounds);
            }
            layout.children().find_map(|child| walk(child, pred))
        }
        walk(self.root(), &pred)
    }

    /// 整棵树的 bounds 缩进打印——测试作者找节点路径/尺寸时先跑这个。
    pub(crate) fn dump(&self) -> String {
        fn walk(layout: Layout<'_>, depth: usize, out: &mut String) {
            let b = layout.bounds();
            out.push_str(&format!(
                "{}{:.1}x{:.1} @ ({:.1}, {:.1})\n",
                "  ".repeat(depth),
                b.width,
                b.height,
                b.x,
                b.y
            ));
            for child in layout.children() {
                walk(child, depth + 1, out);
            }
        }
        let mut out = String::new();
        walk(self.root(), 0, &mut out);
        out
    }

    /// 注入一次事件，返回它触发的消息（`Task` 与真实运行一样在测试里丢弃）。
    pub(crate) fn send(&mut self, event: Event, at: Point) -> Vec<Message> {
        let mut messages: Vec<Message> = Vec::new();
        let mut clipboard = NullClipboard;
        let Self {
            element,
            tree,
            renderer,
            root,
            viewport,
        } = self;
        {
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                tree,
                &event,
                Layout::new(root),
                iced::mouse::Cursor::Available(at),
                renderer,
                &mut clipboard,
                &mut shell,
                viewport,
            );
        }
        messages
    }

    /// 左键单击。
    pub(crate) fn click(&mut self, at: Point) -> Vec<Message> {
        self.send(
            Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            at,
        )
    }

    /// 左键双击：连发两次按下。
    ///
    /// iced 的双击判定是「同一位置（<6px）300ms 内两次」——同一棵树状态里
    /// 连发两次必然落在窗内成对，与 P100 用例的做法一致。
    pub(crate) fn double_click(&mut self, at: Point) -> Vec<Message> {
        let mut out = self.click(at);
        out.extend(self.click(at));
        out
    }

}
