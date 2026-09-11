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
// B10 多光标一期：附加光标集（增删/折叠/步进/caret_rect_at 重入）
mod cursors;

mod metrics;
mod scrollbars;
mod links;
mod dnd;
mod view;
// 第 72 轮：软换行（候选池⑯ Phase 1）地基组件；第 73 轮接线完成
// （EditorCore 视觉映射 / hit_test / motion / 滚动 / draw 全部消费），
// allow(dead_code) 已移除。
mod wrap;

pub use core::*;
pub use view::*;
// B10 Phase 2：同步编辑操作种类（update.rs apply_edit 分流消费）
pub(crate) use cursors::MultiEditKind;
// P133：链接识别（E2）——update.rs 消费 LinkTarget，view.rs 消费 link_at
pub(crate) use links::{link_at, LinkTarget};

/// P155：编辑器动作被拒的**类型化**原因。
///
/// 为什么不直接返回文案：文案是界面语言的一部分（切到英文要换），而
/// editor 层是纯逻辑——把「拒绝原因」与「怎么写给用户看」分开，文案
/// 由 app 层经 [`EditErr::text`] 按当前语言取（翻译表在 core::lang）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditErr {
    /// Ctrl+M 时既无选区、光标也不在词上
    CursorNotOnWord,
    /// 附加光标数量达上限（带实际上限值）
    ExtraCursorCap(usize),
    /// 环形扫描一圈都没有未占用实例
    NoMoreMatch,
    /// 文本工具需要非空选区
    NoSelection,
    /// Base64 解码失败（非法字符/长度）
    Base64DecodeInvalid,
    /// Base64 解码出的字节不是合法 UTF-8
    Base64DecodeNotUtf8,
    /// URL 解码遇到无效百分号转义
    UrlDecodeInvalid,
}

impl EditErr {
    /// 按界面语言取提示文案。
    pub(crate) fn text(self, lang: editpad_core::Lang) -> String {
        use editpad_core::Key as K;
        match self {
            EditErr::CursorNotOnWord => K::EmCursorNotOnWord.text(lang).to_owned(),
            EditErr::ExtraCursorCap(cap) => format!(
                "{}{cap}{}",
                K::EmExtraCursorCapPrefix.text(lang),
                K::EmExtraCursorCapSuffix.text(lang)
            ),
            EditErr::NoMoreMatch => K::EmNoMoreMatch.text(lang).to_owned(),
            EditErr::NoSelection => K::EmNoSelection.text(lang).to_owned(),
            EditErr::Base64DecodeInvalid => K::EmBase64DecodeInvalid.text(lang).to_owned(),
            EditErr::Base64DecodeNotUtf8 => K::EmBase64DecodeNotUtf8.text(lang).to_owned(),
            EditErr::UrlDecodeInvalid => K::EmUrlDecodeInvalid.text(lang).to_owned(),
        }
    }
}

// ---------- 视觉常量（core/view 共用） ----------

/// 行号与正文的最小间距。
pub(crate) const GUTTER_MIN: f32 = 12.0;
/// 行号文本盒的额外宽度余量（P150）：上游 `fill_text` 以 `bounds.width`
/// 为排版界，盒宽**恰好等于**文本宽度时末位字形仍会被判出界并整段丢弃
/// （用户复现：霞鹜臻楷 GB + 24px 下 `10/11/12` 只画出首位）。留 1px 后
/// 右对齐位置不变（左缘仍按「位数 × 行号字宽」算），只是盒子略宽。
pub(crate) const GUTTER_NUM_SLACK: f32 = 1.0;
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

// P154：软换行索引对账（行数变化后不得让滚动夹紧用偏小的总数）
#[cfg(test)]
#[path = "wrap_reconcile_tests.rs"]
mod wrap_reconcile_tests;
