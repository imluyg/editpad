//! 自绘虚拟化编辑器 —— Editpad M2 的技术核心。
//!
//! （P68 拆分自单文件 editor.rs：纯移动零行为变更，方法同 P60——字节级
//! 行段搬运 + 行数守恒核对，无任何文本解码回写。）
//!
//! 设计要点：
//! * [`EditorCore`]（core.rs）持有 ropey [`Document`](editpad_core::Document)，
//!   是唯一数据源；光标/选区/滚动/撤销全在这层。
//! * 渲染只处理**可见行**（视口虚拟化）：50MB 与 5KB 的每帧排版成本相同。
//! * 状态放在 `Rc<RefCell<_>>`（[`EditorHandle`]）里跨帧共享——iced 每帧
//!   重建控件实例，官方 text_editor 的 Content 也是同样的手法。
//! * 实现真正的 `Widget` trait（view.rs）：只有控件层的 `Shell` 能开启
//!   输入法（`request_input_method`）并接收中文上屏事件。
//! * metrics.rs = CJK 双宽列宽换算 + P42 运行时实测；scrollbars.rs =
//!   纵横滚动条几何。测试经 `#[path]` 挂为 core/view 的子模块
//!   （core_tests.rs / view_tests.rs），保留对私有字段的直接访问。
//!
//! 已知取舍（v1）：等宽字体假设（CJK 列映射近似）；预编辑串由系统浮窗
//! 显示；语法高亮以逐行状态缓存接入（M2b 起）。

mod core;
// Phase 2c: EditorCore impl split by domain into undo/motion/edit/block/highlight (pub(crate) bump)
mod undo;
mod motion;
mod edit;
mod block;
mod highlight;

mod metrics;
mod scrollbars;
mod view;
// 第 72 轮：软换行（候选池⑯ Phase 1）地基组件；第 73 轮接线完成
// （EditorCore 视觉映射 / hit_test / motion / 滚动 / draw 全部消费），
// allow(dead_code) 已移除。
mod wrap;

pub use core::*;
pub use view::*;

// ---------- 视觉常量（core/view 共用） ----------

/// 行号与正文的最小间距。
pub(crate) const GUTTER_MIN: f32 = 12.0;
/// 行号栏左侧书签条带宽（第 60 轮）：书签圆点的专属槽位，与行号数字
/// 互不挤占（行号仍右对齐于条带右侧，gutter 总宽 = 条带 + 间距 + 数字宽）。
pub(crate) const BOOKMARK_STRIP: f32 = 10.0;
/// 书签圆点直径（第 60 轮，条带内水平居中、行内垂直居中）。
pub(crate) const BOOKMARK_DOT: f32 = 6.0;
/// 文本图层四边内缩（P66）：层边界必须严格小于传给 fill_text 的 clip
/// 参数，上游 Cached 分支的 is_within 快路径才会判假、掩码才强制生效。
/// 内缩 1 逻辑像素的代价是控件最外圈 1px 不渲染字形——视觉不可感知。
pub(crate) const TEXT_LAYER_INSET: f32 = 1.0;
/// 默认字号（与 core 设置层的规范默认一致）。
pub(crate) const FONT_SIZE_DEFAULT: f32 = 16.0;
/// 行号栏字号相对正文的比例（16px 正文时即原来的 13px）。
pub(crate) const GUTTER_FONT_SCALE: f32 = 13.0 / 16.0;
