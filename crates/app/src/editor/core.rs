//! [`EditorCore`]：rope 文档 + 光标/选区/滚动/撤销/IME 的唯一数据源
//! （P68 自 editor.rs 拆出，纯移动零行为变更）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use iced::{Color, Font, Rectangle};

use editpad_core::{Document, LazyHighlighter, StyledRun};

use super::metrics::{
    char_cols, display_cols, measure_insertion, prefix_width,
    validate_measured_char_width, RECOMPUTE_MAX_COLS_COOLDOWN,
};
use super::{FONT_SIZE_DEFAULT, GUTTER_MIN};

const CARET_WIDTH: f32 = 2.0;
/// 撤销组上限（P37 打字成组后，一组 ≈ 一次连续输入；快照是 rope 结构
/// 共享克隆，每组只钉住差异分块，512 组的最坏常驻增量仍在 MB 量级——
/// 内存入账见 HANDOFF §6 本轮）。
const MAX_UNDO: usize = 512;
/// 滚轮一格的行数（Windows 记事本口径）。
pub(crate) const SCROLL_LINES_PER_NOTCH: f32 = 3.0;

/// 把任意来源的字号归一成合法值：非有限值回退默认，其余 clamp 到设置层允许区间。
pub fn normalize_font_size(size: f32) -> f32 {
    use editpad_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE};
    if size.is_finite() {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        FONT_SIZE_DEFAULT
    }
}

/// 感知亮度的颜色工具：判断主题深浅、把背景提亮。
/// P47 起设置弹窗配色（main.rs `settings_colors`）复用同一深浅判别口径。
pub(crate) fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

pub(crate) fn lighten(c: Color, amount: f32) -> Color {
    Color {
        r: c.r + (1.0 - c.r) * amount,
        g: c.g + (1.0 - c.g) * amount,
        b: c.b + (1.0 - c.b) * amount,
        a: c.a,
    }
}

/// 编辑器光标位置（逻辑行 / 行内字符列，均从 0 计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorPos {
    pub line: usize,
    pub col: usize,
}

/// 光标移动语义（与逻辑行对齐——本编辑器不软换行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

/// 应用层按键编辑操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// 插入文本（可多行；输入法上屏与普通字符共用）
    InsertText(String),
    Backspace,
    Delete,
    SelectAll,
    Undo,
    Redo,
    /// 光标移动；bool = 是否按住 Shift 形成选区
    Motion(Motion, bool),
}

/// 输入法上屏事件的裁决结果（P2 焦点过滤）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeCommit {
    /// 未持有焦点：不消费，放行给其他控件（如查找框）。
    Ignored,
    /// 已消费；Some 为需要插入正文的文本（空提交仅清除预编辑）。
    Consumed(Option<String>),
}

#[derive(Clone)]
struct Snapshot {
    doc: Document,
    cursor: CursorPos,
    anchor: Option<CursorPos>,
}

// ---------- 核心状态 ----------

pub struct EditorCore {
    pub doc: Document,
    pub cursor: CursorPos,
    /// 选区锚点；Some 时选区为 anchor..cursor（无序，取用时归一）
    pub anchor: Option<CursorPos>,
    /// 视口顶部的行号（浮点支持像素级平滑滚动）
    pub scroll_top: f32,
    /// 视口左缘的列偏移（像素；P13 水平滚动，行号栏不随之移动）
    pub scroll_left: f32,
    pub(crate) viewport_h: f32,
    /// 视口宽度（像素，整控件含行号栏；RedrawRequested 时同步）。
    /// 正文区可视宽 = 本值 − 行号栏宽。
    pub(crate) viewport_w: f32,
    pub(crate) dragging: bool,
    /// 垂直滚动条拖拽中：Some(按下点相对滑块顶部的像素偏移)
    pub(crate) scrollbar_grab: Option<f32>,
    /// 水平滚动条拖拽中：Some(按下点相对滑块左缘的像素偏移)（P13）
    pub(crate) hscrollbar_grab: Option<f32>,
    /// 最近一次键盘修饰键状态（控件层跟踪；滚轮事件不带修饰键，
    /// Shift+滚轮横向滚动依赖它，P13）
    pub mods: iced::keyboard::Modifiers,
    /// 全文档最大显示列数的高水位（P13）：水平行程的尺寸依据。
    /// 编辑只上调（量受影响行），整体替换文档时精确重算；
    /// 撤销/重做后可能略高于真实值（可向右滚出一小段空白），
    /// 属已知取舍——精确收缩需要 O(n) 重扫，不值得每步撤销付出。
    max_line_cols: usize,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    /// P37 打字成组撤销：Some(组内上次插入的结束字符偏移) = 当前处于
    /// 连续单字符输入组中。任何其他操作（光标移动、选区变更、删除类
    /// 编辑、撤销/重做、换文档、焦点离开）都会置 None 打断；
    /// 合并条件见 [`EditorCore::insert_str`]。
    typing_run: Option<usize>,
    /// 语法高亮器；None = 纯文本快速路径。RefCell 让只读的 draw 也能推进状态。
    highlight: Option<RefCell<LazyHighlighter>>,
    /// 输入法预编辑串（组字过程中的拼音/候选串），提交前显示在光标处。
    pub(crate) preedit: Option<String>,
    /// 正文字号（驱动行高与列宽）；默认 16，合法区间见 core 设置层。
    font_size: f32,
    /// 编辑器是否持有键盘焦点（点击编辑区置真，点击其他控件置假）。
    /// 输入法事件会广播给所有控件，必须靠它过滤——否则在查找框打字会进正文。
    pub focused: bool,
    /// 光标闪烁相位（打磨项）：应用层心跳每 ~530ms 翻转一次；
    /// 任何光标活动会重置为可见并刷新活动时刻，静止超时后按相位隐现。
    blink_on: bool,
    last_activity: Option<std::time::Instant>,
    /// P53：上次闪烁相位翻转的真实时刻——淡出动画期间心跳临时加密到
    /// 33ms 快拍，相位翻转仍按 CARET_BLINK_MS 门控，光标节奏不受影响。
    last_blink_at: Option<std::time::Instant>,
    /// P53：竖直滚动条最近活动时刻（滚动/滚动条交互/悬停命中区）。
    /// 淡出判定与绘制 alpha 都以它为基准；None = 从未活动（恒隐藏）。
    sb_activity: Option<std::time::Instant>,
    /// P38 落盘基线：最近一次已知「磁盘内容」的文档快照（rope 结构
    /// 共享克隆，O(1)）。撤销/重做后据此判断内容是否回到了已保存状态，
    /// 让 dirty 如实反映「与磁盘的差异」而不是「自保存后动过没有」。
    saved_baseline: Option<Document>,
    /// P42 实测列宽（真实字形 advance，像素/列）：控件层用排版段落实测
    /// 后注入。None = 未实测或测值无效，[`Self::char_width`] 回退固定假设。
    pub(crate) measured_char_w: Option<f32>,
    /// P42 度量键：最近一次实测尝试的 (字体, 字号)。与上字段配对去重——
    /// 键相同即「已按当前字体/字号测过（无论成败）」，避免每帧重测；
    /// 字体切换（P34）/字号变更（set_font_size 折算后仍会重测校准）时换键。
    pub(crate) metric_key: Option<(Font, f32)>,
    /// 行级真实布局缓存（第 40 轮根治）：line → 每个字符起点的真实像素 x
    /// （长度 = 行字符数 + 1，末项 = 行尾 x）。由控件层每帧按可见行 shaping
    /// 注入（与正文绘制同源段落）；未注入的行回退列模型。命中时光标/选区/
    /// 点击/高亮分片一律按字形真实位置定位——与绘制零误差，字体回退、
    /// 分数宽度、连字、TAB 实际展开全部如实反映，静态列模型的任何假设
    /// 破缺（非等宽字体、非整倍字号）都不再产生累计漂移。
    row_layouts: HashMap<usize, Vec<f32>>,
    /// 已注入行宽的最大值（与 row_layouts 同步维护）：水平行程钳制上界的
    /// 真实补充——列模型 `max_line_cols` 对分数/超宽字形可能欠估，
    /// 不补则超宽行滚不到头、光标越界。
    max_row_width_px: f32,
    /// 缩短类编辑/撤销重做后置位：`max_line_cols` 高水位可能过估
    /// （只升不降的既有取舍），由 [`Self::clamp_scroll_horizontal`] 按
    /// 冷却窗惰性全量重算收敛——否则删除超宽行后水平滚动条永不消失。
    max_cols_stale: bool,
    /// 上次全量重算 `max_line_cols` 的时刻（限流用，见上字段）。
    max_cols_checked: Option<std::time::Instant>,
}

/// 光标闪烁半周期。
pub const CARET_BLINK_MS: u64 = 530;
/// 光标活动后的常显窗口（移动/输入时不受闪烁相位影响）。
const CARET_ACTIVE_MS: u128 = 450;

/// P53：竖直滚动条闲置窗——距上次滚动/滚动条交互不超过该时长保持全显。
pub const SCROLLBAR_IDLE_MS: u32 = 900;
/// P53：淡出动画时长（闲置后从全显线性降到隐藏）。
pub const SCROLLBAR_FADE_MS: u32 = 300;
/// P53：淡出动画的重绘节拍（CaretTick 常驻链在淡出期间切换到的快拍间隔）。
pub const SCROLLBAR_FADE_TICK_MS: u64 = 33;

/// P53：竖直滚动条淡出 alpha（纯函数可单测）——闲置窗内全显 1.0；其后
/// [`SCROLLBAR_FADE_MS`] 内线性降到 0.0；再后恒 0（不回弹）。
pub fn scrollbar_alpha(idle_ms: u32) -> f32 {
    if idle_ms <= SCROLLBAR_IDLE_MS {
        1.0
    } else {
        let remaining =
            (SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS).saturating_sub(idle_ms);
        remaining as f32 / SCROLLBAR_FADE_MS as f32
    }
}

impl Default for EditorCore {
    fn default() -> Self {
        Self {
            doc: Document::new(),
            cursor: CursorPos::default(),
            anchor: None,
            scroll_top: 0.0,
            scroll_left: 0.0,
            viewport_h: 400.0,
            viewport_w: 800.0,
            dragging: false,
            scrollbar_grab: None,
            hscrollbar_grab: None,
            mods: iced::keyboard::Modifiers::empty(),
            max_line_cols: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            typing_run: None,
            highlight: None,
            preedit: None,
            font_size: FONT_SIZE_DEFAULT,
            focused: true,
            blink_on: true,
            last_activity: None,
            last_blink_at: None,
            // P53：boot 即全显（打开长文档时滚动条立即可用），闲置后淡出
            sb_activity: Some(std::time::Instant::now()),
            // P38：未命名页的基线 = 初始空内容——撤销回空白即可安全关页
            saved_baseline: Some(Document::new()),
            // P42：默认未实测，走 0.5625 固定假设（既有契约不变）
            measured_char_w: None,
            metric_key: None,
            row_layouts: HashMap::new(),
            max_row_width_px: 0.0,
            max_cols_stale: false,
            max_cols_checked: None,
        }
    }
}

impl EditorCore {
    // ---------- 字号与几何度量 ----------

    /// 当前正文字号。
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// 行高（像素）= 字号 × 1.375。
    pub fn line_height(&self) -> f32 {
        self.font_size * 1.375
    }

    /// 单列字符宽（像素）。
    ///
    /// P42：优先用 [`Self::measured_char_w`]——控件层按真实字形 advance
    /// 实测的等宽列宽；未实测（或测值无效）回退固定假设 `字号 × 0.5625`。
    /// 假设值按 Consolas（≈0.55em）估的：P33 钉 NSimSun（0.5em）后每字符
    /// 累计 +1px 漂移，光标压字/离字（P42）皆源于此，实测后归零。
    pub fn char_width(&self) -> f32 {
        self.measured_char_w
            .unwrap_or_else(|| self.font_size * 0.5625)
    }

    // ---------- 行级真实布局（第 40 轮根治） ----------

    /// 注入一行字符起点的真实像素 x（`xs[i]` = 第 i 个字符起点，末项 = 行尾）。
    /// 同时抬升真实行宽高水位（水平行程钳制用）。
    pub fn set_row_layout(&mut self, line: usize, xs: Vec<f32>) {
        if let Some(&w) = xs.last() {
            self.max_row_width_px = self.max_row_width_px.max(w);
        }
        self.row_layouts.insert(line, xs);
    }

    /// 清空全部行级布局与行宽高水位（控件层每帧重建可见行时调用，
    /// 保证无陈旧残留——编译/滚动后旧行布局不会冒充新内容）。
    pub fn clear_row_layouts(&mut self) {
        self.row_layouts.clear();
        self.max_row_width_px = 0.0;
    }

    /// 行内第 `col` 个字符起点的真实像素 x；未注入返回 None（调用方回退
    /// 列模型 `prefix_width × char_width`）。越界 col 取行尾（与列模型
    /// 的 min(chars.count()) 口径一致）。
    ///
    /// P45 起生产路径改用滞后感知的 [`Self::px_of`]（update 期布局滞后
    /// 时回退实时列模型）；本查询保留为 public API（外部兼容 + 诊断）。
    #[allow(dead_code)]
    pub fn row_x(&self, line: usize, col: usize) -> Option<f32> {
        let xs = self.row_layouts.get(&line)?;
        let last = xs.len() - 1;
        Some(xs[col.min(last)])
    }

    /// 已注入的行宽（像素）。当前由水平行程钳制经由
    /// [`Self::max_row_width_px`] 消费；本查询供测试断言行级布局内容。
    #[allow(dead_code)]
    pub fn row_width_px(&self, line: usize) -> Option<f32> {
        self.row_layouts.get(&line).and_then(|xs| xs.last().copied())
    }

    /// 已注入行宽的最大值（水平行程钳制的真实补充上界）。
    pub fn max_row_width_px(&self) -> f32 {
        self.max_row_width_px
    }

    /// 第 P45 轮：行内第 `col` 个字符起点的水平像素——**布局滞后感知**。
    ///
    /// update 期（键盘/点击事件处理）早于本帧 `layout` 注入，`row_layouts`
    /// 里还是**上一帧的旧文本布局**：行刚变长时布局覆盖不到目标列，
    /// 若直接取用会得到旧行尾位置——光标可见性判断失灵，超宽后新字符
    /// 全部画在视口外，表现为「打字吞字」（P45 根因）。
    ///
    /// 规则：行布局新鲜（长度覆盖到目标列）用真实字形位置；滞后回退
    /// 列模型**实时文本**计算（`text` 必须为当前文档该行文本）。draw 期
    /// 布局恒新鲜，此函数两期通用。
    pub(crate) fn px_of(&self, line: usize, text: &str, col: usize) -> f32 {
        let col = col.min(text.chars().count());
        match self.row_layouts.get(&line) {
            Some(xs) if xs.len().saturating_sub(1) >= col => xs[col],
            _ => prefix_width(text, col) * self.char_width(),
        }
    }

    /// 水平内容宽（像素）：列模型高水位与真实行宽证据取较大者。
    /// 滚动条行程/钳制/绘制统一走本口径，避免与真实字形位置脱节。
    pub fn content_width_px(&self) -> f32 {
        (self.max_line_cols as f32 * self.char_width()).max(self.max_row_width_px())
    }

    /// P42：注入实测列宽（控件层量得真实字形 advance 后调用）。
    /// 校验失败（非有限/非正/超出字号的合理倍率区间）返回 false 并保持现状。
    pub fn set_measured_char_width(&mut self, width: f32) -> bool {
        match validate_measured_char_width(width, self.font_size) {
            Some(w) => {
                self.measured_char_w = Some(w);
                true
            }
            None => false,
        }
    }

    /// 设置字号：clamp 到合法区间后让滚动/可见性按新度量重新收敛
    /// （字号变大时可见行变少，光标必须仍落在视口内）。
    /// P42：实测列宽按字号比例折算（同一字体的 advance 与字号线性），
    /// 折算后仍落合法区间；控件层下一帧按新度量键重测校准。
    pub fn set_font_size(&mut self, size: f32) {
        let old = self.font_size;
        self.font_size = normalize_font_size(size);
        if old > 0.0 && self.font_size != old {
            if let Some(w) = self.measured_char_w {
                let scaled = w * self.font_size / old;
                // 比例折算不改变 w/字号 倍率，校验恒应通过；万一浮点
                // 边界翻车就丢弃实测回退假设，下一帧重测兜底
                if validate_measured_char_width(scaled, self.font_size).is_some() {
                    self.measured_char_w = Some(scaled);
                } else {
                    self.measured_char_w = None;
                }
            }
        }
        self.ensure_visible();
    }

    /// 用新文档整体替换（加载文件时用），清空历史。
    pub fn reset_document(&mut self, doc: Document) {
        // P38：新文档即新的落盘基线（加载完成 = 磁盘内容已就位）
        self.saved_baseline = Some(doc.clone());
        self.doc = doc;
        self.cursor = CursorPos::default();
        self.anchor = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.typing_run = None; // P37：换文档即一切成组状态作废
        self.recompute_max_line_cols();
        if let Some(hl) = &self.highlight {
            hl.borrow_mut().invalidate_from(0);
        }
        self.preedit = None;
    }

    /// 按文件扩展名启用语法高亮（测试辅助；生产路径已改走
    /// [`Self::set_language_by_name`]）。
    #[cfg(test)]
    pub fn set_language(&mut self, extension: Option<&str>) {
        self.highlight = extension.and_then(LazyHighlighter::new).map(RefCell::new);
    }

    /// 按语法名启用高亮（P22）：名字来自 core 的别名层/嗅探层解析；
    /// None 关闭（纯文本快速路径）。
    pub fn set_language_by_name(&mut self, name: Option<&str>) {
        self.highlight = name
            .and_then(LazyHighlighter::new_by_name)
            .map(RefCell::new);
    }

    /// 第 `offset` 字符偏移之后的高亮状态失效。
    fn invalidate_highlight_from(&mut self, offset: usize) {
        if let Some(hl) = &self.highlight {
            let line = self.doc.char_to_line(offset.min(self.doc.text_len()));
            hl.borrow_mut().invalidate_from(line);
        }
    }

    /// 计算第 `line_idx` 行的着色片段；未启用高亮时返回空表。
    ///
    /// P12：补建预算受限——缺失检查点档位超过
    /// [`LazyHighlighter::MAX_INLINE_STRIDES`] 时不再于本线程现算
    /// （旧实现会在 UI 线程一次性冻结数十秒），而是返回无色渲染，
    /// 同时经 [`Self::needs_paving`] 由应用层安排后台分批补建；
    /// 铺建推进到该行后自然恢复配色。
    ///
    /// P61 渐进上色：精确预算不足时改走**可视区近似上色**（anchor =
    /// 可视首行的全新语法状态，独立缓存、不污染精确路径）——大跳转后
    /// 视口立即有近似配色，后台精确铺建到达后自然替换。可视区之外的
    /// 行保持无色（反正画不出来）。
    pub(crate) fn highlight_runs(&self, line_idx: usize, target_text: &str) -> Vec<StyledRun> {
        let Some(hl) = &self.highlight else {
            return Vec::new();
        };
        let doc = &self.doc;
        let total = doc.line_count();
        if let Some(runs) = hl.borrow_mut().styled_line_limited(
            line_idx,
            target_text,
            total,
            LazyHighlighter::MAX_INLINE_STRIDES,
            &mut |i| doc.line_str(i).trim_end_matches(['\n', '\r']).to_owned(),
        ) {
            return runs;
        }
        let anchor = self.visible_range().0;
        hl.borrow_mut().styled_line_approx(
            line_idx,
            anchor,
            target_text,
            total,
            &mut |i| doc.line_str(i).trim_end_matches(['\n', '\r']).to_owned(),
        )
    }

    // ---------- 高亮后台分批补建（P12 协作面） ----------

    /// 可见区域是否还有「内联预算外」的缺档（需要安排后台铺建）。
    /// 以视口最末行判断：O(1)，每帧在 RedrawRequested 里调用。
    pub fn needs_paving(&self) -> bool {
        let Some(hl) = &self.highlight else {
            return false;
        };
        let last_visible = self.visible_range().1;
        hl.borrow().strides_missing(last_visible) > LazyHighlighter::MAX_INLINE_STRIDES
    }

    /// 当前使用的语法名（None = 纯文本路径）。
    /// 状态栏展示与「按当前语法判断」的功能开关共用。
    pub fn highlight_syntax_name(&self) -> Option<String> {
        self.highlight
            .as_ref()
            .map(|h| h.borrow().syntax_name().to_owned())
    }

    /// 当前高亮器代次；未启用高亮时为 None。
    pub fn highlight_generation(&self) -> Option<u64> {
        self.highlight.as_ref().map(|h| h.borrow().generation())
    }

    /// 检查点数量（测试诊断用；生产路径经 [`Self::needs_paving`] 间接消费）。
    #[cfg(test)]
    pub fn highlight_checkpoints_len(&self) -> Option<usize> {
        self.highlight.as_ref().map(|h| h.borrow().checkpoints_len())
    }

    /// 后台铺建的起点快照：(代次, 高亮器克隆)。文档克隆由调用方另行完成。
    /// 未启用高亮时为 None。
    pub fn highlight_pave_snapshot(&self) -> Option<(u64, LazyHighlighter)> {
        self.highlight.as_ref().map(|h| {
            let hl = h.borrow();
            (hl.generation(), hl.clone())
        })
    }

    /// 安装一份后台铺建成果；仅当其代次与当前高亮器一致时生效（P12）。
    /// 返回是否安装。代次不符 = 期间发生过编辑，成果整体丢弃。
    ///
    /// 整体替换会丢掉铺建期间同步路径攒下的少量行缓存（重新按需推导，
    /// 只影响速度不影响正确性），换来的是无需合并逻辑的简单协议。
    pub fn install_highlighter_if_current(&self, generation: u64, paved: LazyHighlighter) -> bool {
        let Some(hl) = &self.highlight else {
            return false;
        };
        let mut hl = hl.borrow_mut();
        if hl.generation() != generation {
            return false;
        }
        *hl = paved;
        true
    }

    /// 用新文档整体替换（全部替换时用），保留撤销链以便反悔。
    pub fn replace_whole_document(&mut self, doc: Document) {
        self.snapshot();
        self.doc = doc;
        self.cursor = CursorPos::default();
        self.anchor = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        self.recompute_max_line_cols();
        if let Some(hl) = &self.highlight {
            hl.borrow_mut().invalidate_from(0);
        }
    }

    /// P37：打断当前打字组。凡不经过 [`Self::snapshot`] 的状态变更
    /// （光标移动、选区变更、撤销/重做、焦点离开、点击落点）都必须调用，
    /// 保证组内只有真正连续的单字符输入。
    pub(crate) fn break_typing(&mut self) {
        self.typing_run = None;
    }

    // ---------- 落盘基线（P38：dirty 如实反映与磁盘的差异） ----------

    /// 登记落盘基线 = **当前内容**。仅在「当前内容确已写入磁盘」的时刻
    /// 调用（保存/自动保存成功且版本守卫通过）；版本不符时不得调用——
    /// 那时磁盘上是旧快照，基线保持不动才能维持 dirty 的正确性。
    /// rope 克隆是结构共享，O(1)。
    pub fn mark_saved(&mut self) {
        self.saved_baseline = Some(self.doc.clone());
    }

    /// 当前内容是否与最近一次落盘基线一致。撤销/重做后由应用层查询，
    /// 决定 dirty 是否可以回清。无基线时保守返回 false。
    pub fn is_at_saved_content(&self) -> bool {
        self.saved_baseline
            .as_ref()
            .is_some_and(|base| base.content_eq(&self.doc))
    }

    /// 撤销落盘基线（置 None）。用于 P30 会话恢复：恢复出来的置脏页内容
    /// 来自快照而非磁盘，若把快照当基线，「编辑→撤销回快照态」会错误
    /// 清脏——命名页下次启动从磁盘旧内容重载、未命名页直接空白，未存
    /// 改动将静默丢失。清掉后 `is_at_saved_content` 保守返回 false，该页
    /// 在真正落盘一次之前撤销永不回清，宁可不便利也不能丢内容。
    pub fn clear_saved_baseline(&mut self) {
        self.saved_baseline = None;
    }

    fn snapshot(&mut self) {
        // P37：开新快照 = 上一组就此终结（删除类编辑/整体替换/非合并
        // 插入都经此处打断成组；合并插入走的是跳过本函数的路径）
        self.typing_run = None;
        // P45：任何经快照的编辑都可能改变文档宽度结构——高水位可能
        // 过估，标记惰性收敛（合并插入只增不减，无需标记）
        self.max_cols_stale = true;
        self.undo_stack.push(Snapshot {
            doc: self.doc.clone(), // rope 克隆是结构共享，廉价
            cursor: self.cursor,
            anchor: self.anchor,
        });
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        self.break_typing(); // P37：撤销本身打断组，防后续输入混入历史组
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        // 文档被整体替换，高亮状态全量失效
        if let Some(hl) = &self.highlight {
            hl.borrow_mut().invalidate_from(0);
        }
        // P45：撤销 = 整个文档替换，宽度结构可能缩短——标记惰性收敛
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.break_typing(); // P37 同上
        let Some(snap) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        if let Some(hl) = &self.highlight {
            hl.borrow_mut().invalidate_from(0);
        }
        // P45：重做同样整体替换文档——标记惰性收敛（对称 undo）
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    pub fn select_all(&mut self) {
        self.break_typing(); // P37：选区变更打断组
        let last = self.doc.line_count().saturating_sub(1);
        self.anchor = Some(CursorPos::default());
        self.cursor = CursorPos {
            line: last,
            col: self.line_display_len(last),
        };
        self.ensure_visible();
    }

    /// 当前选区文本（无选区或零宽返回 None）。
    pub fn selected_text(&self) -> Option<String> {
        let (start_off, end_off) = self.selection_offsets()?;
        if start_off >= end_off {
            return None;
        }
        Some(self.doc.slice_text(start_off, end_off))
    }

    /// 选区的字符偏移区间（有序）。
    fn selection_offsets(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (a, b) = if (anchor.line, anchor.col) <= (self.cursor.line, self.cursor.col) {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        let start = self.doc.line_to_char(a.line) + a.col.min(self.line_display_len(a.line));
        let end = self.doc.line_to_char(b.line) + b.col.min(self.line_display_len(b.line));
        Some((start.min(end), end.max(start)))
    }

    /// 在光标处插入文本（先吃掉当前选区）。支持多行文本。
    /// P9：入文前把 `\r\n` / `\n` / 孤立 `\r` 统一归一为文档主导行尾，
    /// 回车（插 `\n`）、输入法上屏、剪贴板粘贴共用本入口，不再产生混合行尾。
    /// P37 打字成组：仅当「上一操作就是本组内的一次合格插入、本次也是
    /// 单个非换行字符、无选区、插入点恰好接在上次结束处」才并入当前组
    /// （跳过快照，一次撤销撤掉整段连续输入）；换行、粘贴（多字符）、
    /// 选区替换一律开新组。
    pub fn insert_str(&mut self, text: &str) {
        let text = self.doc.line_ending().normalize(text);
        let single = {
            let mut it = text.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let at = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        let merges = match (self.typing_run, single) {
            (Some(run_end), Some(c)) => {
                c != '\n'
                    && c != '\r'
                    && self.selection_offsets().is_none()
                    && at == run_end
            }
            _ => false,
        };
        // 本次插入是否合格为「组内一员」——决定下一字符能否继续并入
        let eligible =
            matches!(single, Some(c) if c != '\n' && c != '\r') && self.selection_offsets().is_none();
        if !merges {
            self.snapshot();
        }
        self.typing_run = None; // 插入成功且合格后在本函数末尾重立

        let start_offset = match self.selection_offsets() {
            Some((start, end)) => {
                if end > start {
                    self.doc.remove_range(start, end);
                }
                let line = self.doc.char_to_line(start);
                let col = start - self.doc.line_to_char(line);
                self.cursor = CursorPos { line, col };
                start
            }
            None => self.doc.line_to_char(self.cursor.line) + self.cursor.col,
        };
        self.anchor = None;
        let first_line = self.cursor.line;

        self.doc.insert(start_offset, &text);
        self.invalidate_highlight_from(start_offset);

        // 推进光标到插入文本的末尾（EOL 单元感知，CRLF 算一次换行）
        let (new_lines, tail_cols) = measure_insertion(&text);
        if new_lines > 0 {
            self.cursor.line += new_lines;
            self.cursor.col = tail_cols;
        } else {
            self.cursor.col += tail_cols;
        }
        // P13：受影响行（插入跨行时含沿途各行）宽度只上调高水位
        self.raise_max_line_cols(first_line..=self.cursor.line);
        // P37：合格单字符插入把组延伸到新的结束偏移；换行/粘贴/选区替换
        // 保持 None——下一字符开新组
        self.typing_run = if eligible { Some(start_offset + 1) } else { None };
        self.ensure_visible();
    }

    /// 用给定文本替换当前选区；无选区时退化为插入。
    pub fn replace_selection(&mut self, text: &str) {
        self.insert_str(text);
    }

    /// 把选区起点放到 `(line, col)` 并向右延伸 `len_chars` 个字符形成新选区。
    pub fn select_span(&mut self, line: usize, col: usize, len_chars: usize) {
        self.break_typing(); // P37：选区变更打断组（查找跳转/替换当前都经此）
        let start = CursorPos { line, col };
        let mut cur = start;
        let mut remain = len_chars;
        while remain > 0 {
            let line_len = self.line_display_len(cur.line);
            let room = line_len.saturating_sub(cur.col);
            if room >= remain {
                cur.col += remain;
                remain = 0;
            } else if cur.line + 1 < self.doc.line_count() {
                remain -= room + 1; // 吃掉换行符
                cur.line += 1;
                cur.col = 0;
            } else {
                cur.col = line_len;
                break;
            }
        }
        self.anchor = Some(start);
        self.cursor = cur;
        self.ensure_visible();
    }

    /// 全文字符偏移处的字符；越界返回 None
    /// （ropey 切片越界会 panic，探测相邻字符前必须先夹紧）。
    fn char_at(&self, offset: usize) -> Option<char> {
        if offset >= self.doc.text_len() {
            return None;
        }
        self.doc.slice_text(offset, offset + 1).chars().next()
    }

    /// 光标偏移处是否为完整的 CRLF 换行单元（`\r\n` 相邻成对）。
    fn is_crlf_at(&self, offset: usize) -> bool {
        self.char_at(offset) == Some('\r') && self.char_at(offset + 1) == Some('\n')
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == CursorPos::default() {
            return;
        }
        self.snapshot();
        self.move_local(Motion::Left);
        let start = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        // P9：跨行回退落在 CRLF 上时把 `\r\n` 当一个换行单元整体移除。
        // 旧行为只删一半字符：第一下视觉无反应，第二下才真正并行的两行。
        let end = start + 1 + usize::from(self.is_crlf_at(start));
        self.doc.remove_range(start, end);
        self.invalidate_highlight_from(start);
        // P13：并行后的新行可能更宽（也可能只是收缩——高水位不回退）
        self.raise_max_line_cols(self.cursor.line..=self.cursor.line);
        self.ensure_visible();
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let offset = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        if offset >= self.doc.text_len() {
            return;
        }
        self.snapshot();
        // P9：行尾 Delete 同样按 EOL 单元处理，一下删掉整个 `\r\n`
        let end = offset + 1 + usize::from(self.is_crlf_at(offset));
        self.doc.remove_range(offset, end);
        self.invalidate_highlight_from(offset);
        // P13：下一行并入当前行，合并结果可能更宽
        let merged = self.cursor.line;
        self.raise_max_line_cols(merged..=merged);
        self.ensure_visible();
    }

    /// 有选区时删除之（含快照）；返回是否发生了删除。零宽选区仅清除标记。
    fn delete_selection(&mut self) -> bool {
        if self.selected_text().is_some() {
            self.snapshot();
            if let Some((start, end)) = self.selection_offsets() {
                self.doc.remove_range(start, end);
                let line = self.doc.char_to_line(start);
                let col = start - self.doc.line_to_char(line);
                self.cursor = CursorPos { line, col };
                self.invalidate_highlight_from(start);
                // P13：跨行删除后首尾两行拼成一行的宽度可能变化
                let joined = self.cursor.line;
                self.raise_max_line_cols(joined..=joined);
            }
            self.anchor = None;
            self.ensure_visible();
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// 应用光标移动；`extend` 为 true 时保持锚点形成选区。
    pub fn apply_motion(&mut self, motion: Motion, extend: bool) {
        self.break_typing(); // P37：光标移动打断组
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        if !extend {
            self.anchor = None;
        }
        self.move_local(motion);
        self.ensure_visible();
    }

    fn move_local(&mut self, motion: Motion) {
        let page_rows = ((self.viewport_h / self.line_height()) as usize).max(1);
        let last_line = self.doc.line_count().saturating_sub(1);

        match motion {
            Motion::Left => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                } else if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.line_display_len(self.cursor.line);
                }
            }
            Motion::Right => {
                if self.cursor.col < self.line_display_len(self.cursor.line) {
                    self.cursor.col += 1;
                } else if self.cursor.line < last_line {
                    self.cursor.line += 1;
                    self.cursor.col = 0;
                }
            }
            Motion::Up => {
                if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col =
                        self.cursor.col.min(self.line_display_len(self.cursor.line));
                }
            }
            Motion::Down => {
                if self.cursor.line < last_line {
                    self.cursor.line += 1;
                    self.cursor.col =
                        self.cursor.col.min(self.line_display_len(self.cursor.line));
                }
            }
            Motion::Home => self.cursor.col = 0,
            Motion::End => self.cursor.col = self.line_display_len(self.cursor.line),
            Motion::PageUp => {
                self.cursor.line = self.cursor.line.saturating_sub(page_rows);
                self.cursor.col =
                    self.cursor.col.min(self.line_display_len(self.cursor.line));
            }
            Motion::PageDown => {
                self.cursor.line = (self.cursor.line + page_rows).min(last_line);
                self.cursor.col =
                    self.cursor.col.min(self.line_display_len(self.cursor.line));
            }
            Motion::DocStart => self.cursor = CursorPos::default(),
            Motion::DocEnd => {
                self.cursor = CursorPos {
                    line: last_line,
                    col: self.line_display_len(last_line),
                };
            }
        }
    }

    /// 跳转到第 `line_1based` 行行首（1 起）。
    pub fn jump_to_line(&mut self, line_1based: usize) {
        self.break_typing(); // P37：跳转打断组
        let target =
            (line_1based.saturating_sub(1)).min(self.doc.line_count().saturating_sub(1));
        self.anchor = None;
        self.cursor = CursorPos {
            line: target,
            col: 0,
        };
        self.ensure_visible();
    }

    /// 会话恢复的无副作用定位入口（P30）：把光标与滚动放回上次退出时的位置。
    ///
    /// 与编辑/移动入口的区别：不产生撤销快照、不触发置脏、不唤醒光标闪烁；
    /// 行列越界夹紧到文档范围，滚动值先消毒（非有限归零）再按行程钳制。
    /// 视口尺寸在首次布局前未知——此处只做行数域的宽松钳制，
    /// 像素级精钳制由布局后的 [`Self::set_viewport_height`] 收口。
    pub fn restore_view(&mut self, line: usize, col: usize, scroll_top: f32, scroll_left: f32) {
        self.break_typing(); // P37：定位打断组（会话恢复/最近文件定位路径）
        let last = self.doc.line_count().saturating_sub(1);
        let line = line.min(last);
        // 列按显示口径夹紧（行尾 \r 不计，与光标移动语义一致）
        let col = col.min(self.line_display_len(line));
        self.anchor = None;
        self.cursor = CursorPos { line, col };
        self.scroll_top = if scroll_top.is_finite() { scroll_top } else { 0.0 };
        self.scroll_left = if scroll_left.is_finite() { scroll_left } else { 0.0 };
        self.clamp_scroll();
        self.clamp_scroll_horizontal();
    }

    pub fn scroll_by_lines(&mut self, lines: f32) {
        self.scroll_top = (self.scroll_top - lines).max(0.0);
        self.clamp_scroll();
        // P53：滚动即活动——竖直滚动条点亮（淡出中的条立即回全显）
        self.touch_scrollbar_activity();
    }

    /// 拖选时视口边缘的自动滚动量（打磨项）：
    /// 返回 `(列增量, 行增量)`——语义与 `scroll_by_columns` /
    /// `scroll_by_lines` 的入参一致（行增量为负 = 视口向文档尾推进）。
    ///
    /// 指针进入控件四缘的窄带即产生滚动，越深入越快（线性放大）；
    /// 位于安全区返回 (0, 0)。事件驱动方案：仅鼠标移动时推进，
    /// 无定时器参与（已知取舍：指针完全静止时不滚动）。
    pub fn edge_scroll_delta(
        &self,
        local_x: f32,
        local_y: f32,
        widget_w: f32,
        widget_h: f32,
    ) -> (f32, f32) {
        const ZONE: f32 = 32.0;

        // 强度 ∈ (0,1]：越贴近边缘滚得越快
        fn strength(dist: f32, zone: f32) -> f32 {
            1.0 + ((zone - dist) / zone).max(0.0) * 3.0
        }

        let mut dy = 0.0f32;
        if local_y < ZONE {
            dy = strength(local_y, ZONE) * SCROLL_LINES_PER_NOTCH;
        } else if local_y > widget_h - ZONE {
            dy = -strength(widget_h - local_y, ZONE) * SCROLL_LINES_PER_NOTCH;
        }

        let mut dx = 0.0f32;
        if local_x < ZONE {
            dx = -strength(local_x, ZONE);
        } else if local_x > widget_w - ZONE {
            dx = strength(widget_w - local_x, ZONE);
        }

        (dx, dy)
    }

    /// 横向滚动 `cols` 列（正数向右看更后的内容，P13）。
    /// Shift+滚轮、触控板横向分量、水平滚动条共用本入口。
    pub fn scroll_by_columns(&mut self, cols: f32) {
        let target = self.scroll_left + cols * self.char_width();
        // 巨量滚动时中间量可能溢出为 ±inf：按方向饱和，交给钳制收口
        self.scroll_left = if target.is_finite() {
            target
        } else if cols > 0.0 {
            f32::MAX
        } else {
            f32::MIN
        };
        self.clamp_scroll_horizontal();
        // P54：横向滚动同样点亮滚动条（两条共用一个活动戳，见 draw）
        self.touch_scrollbar_activity();
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_top = self.scroll_top.max(0.0);
        let max = (self.doc.line_count() as f32 - self.viewport_h / self.line_height()).max(0.0);
        // P66：恢复小数滚动位置。P59 的整行对齐（round）是「tiny-skia 对
        // Cached 文本无真裁剪」年代的权宜——半可见行会画出控件边界且部分
        // 重绘清不掉，只能让每行要么完整要么不可见；副作用是滚动永远整行
        // 瞬跳、行号栏钉死在固定槽位（用户反馈「行号不跟随内容移动」）。
        // 现绘制侧改用 start_layer 图层掩码在光栅期硬裁剪一切图元（见
        // draw），半可见行可以安全上屏，滚动回到像素级平滑。
        self.scroll_top = self.scroll_top.min(max + 1.0).max(0.0);
    }

    /// 正文区可视宽度（像素）= 视口宽 − 行号栏宽。
    fn text_viewport_w(&self) -> f32 {
        (self.viewport_w - self.gutter_width()).max(0.0)
    }

    /// 水平行程钳制：左缘不出负，右缘不超出最宽行。
    ///
    /// P45：缩短类编辑/撤销重做后置 [`Self::max_cols_stale`]，这里按
    /// 冷却窗（可重现的限流，避免 50MB 文档每次删除都 O(n) 全扫）惰性
    /// 全量重算——`max_line_cols` 只升不降的旧取舍会让删除超宽行后
    /// 水平滚动条永不消失。重算结果仍取「列模型 ∪ 真实行宽」。
    pub fn clamp_scroll_horizontal(&mut self) {
        if self.max_cols_stale {
            let now = std::time::Instant::now();
            if self
                .max_cols_checked
                .is_none_or(|t| now.duration_since(t) >= RECOMPUTE_MAX_COLS_COOLDOWN)
            {
                self.max_cols_checked = Some(now);
                self.recompute_max_line_cols();
                self.max_cols_stale = false;
            }
        }
        // 第 40 轮：真实行宽高水位并取最大——列模型对分数/超宽字形可能
        // 欠估，不补则超宽行（emoji/全宽符号）滚不到头、光标被钳出视口
        let content_w = self.content_width_px();
        let max = (content_w - self.text_viewport_w()).max(0.0);
        if self.scroll_left.is_finite() {
            self.scroll_left = self.scroll_left.clamp(0.0, max);
        } else {
            self.scroll_left = 0.0;
        }
    }

    /// 水平方向的光标可见性：光标像素位置越出左右缘即平移视口。
    fn ensure_visible_horizontal(&mut self) {
        let text = self.line_text(self.cursor.line);
        let col = self.cursor.col.min(text.chars().count());
        // P45：滞后感知——布局新鲜走真实字形位置，滞后回退列模型
        // （实时文本），保证超宽打字立即滚动、不吞字
        let cx = self.px_of(self.cursor.line, &text, col);
        let view_w = self.text_viewport_w();
        if view_w <= 0.0 {
            return;
        }
        if cx < self.scroll_left {
            self.scroll_left = cx;
        } else if cx > self.scroll_left + view_w - CARET_WIDTH {
            // 光标竖线本身也要完整可见
            self.scroll_left = cx - view_w + CARET_WIDTH;
        }
        self.clamp_scroll_horizontal();
    }

    fn ensure_visible(&mut self) {
        self.poke_caret();
        let first = self.scroll_top;
        // P66：可见行数按浮点口径（viewport_h / lh）——半可见行现在照常
        // 绘制，光标收敛目标不再依赖「底缘留白 ≤ 一行」的旧假设；小数
        // scroll_top 下 last 为小数，光标行（整数）越过它即触发平移，
        // 收敛结果与整行对齐时代一致且随平滑滚动连续
        let rows_visible = (self.viewport_h / self.line_height()).max(1.0);
        let last = self.scroll_top + rows_visible - 1.0;
        let line = self.cursor.line as f32;
        if line < first {
            self.scroll_top = line;
        } else if line > last {
            self.scroll_top = line - rows_visible + 1.0;
        }
        self.clamp_scroll();
        // P53：视口确实随光标移动才点亮滚动条（行内打字不无谓点亮）
        if (self.scroll_top - first).abs() > f32::EPSILON {
            self.touch_scrollbar_activity();
        }
        self.ensure_visible_horizontal();
    }

    // ---------- P13 水平行程的宽度追踪 ----------

    /// 全量重算最大显示列数（整体换文档时调用；O(n)，加载路径本来 O(n)）。
    fn recompute_max_line_cols(&mut self) {
        let count = self.doc.line_count();
        let mut max = 0usize;
        for line in 0..count {
            let cols = display_cols(self.line_text(line).as_str()) as usize;
            if cols > max {
                max = cols;
            }
        }
        self.max_line_cols = max;
    }

    /// 把 `[first_line, last_line]` 内各行宽度并入高水位（编辑后调用）。
    fn raise_max_line_cols(&mut self, lines: std::ops::RangeInclusive<usize>) {
        for line in lines {
            let cols = display_cols(self.line_text(line).as_str()) as usize;
            if cols > self.max_line_cols {
                self.max_line_cols = cols;
            }
        }
    }

    /// 当前最大显示列数（测试诊断用；水平滚动条以高水位 × 列宽定行程）。
    #[cfg(test)]
    pub fn max_line_display_cols(&self) -> usize {
        self.max_line_cols
    }

    /// 可见的行号闭区间 [first, last]（已夹紧到文档范围）。
    pub fn visible_range(&self) -> (usize, usize) {
        let count = self.doc.line_count();
        let first = (self.scroll_top.floor() as i64).max(0) as usize;
        let rows = (self.viewport_h / self.line_height()).ceil() as usize + 1;
        let last = (first + rows).min(count.saturating_sub(1));
        (first, last)
    }

    /// 第 `line` 行的可见长度（不含换行符）。
    pub fn line_display_len(&self, line: usize) -> usize {
        if line >= self.doc.line_count() {
            return 0;
        }
        self.doc
            .line_str(line)
            .trim_end_matches(['\n', '\r'])
            .chars()
            .count()
    }

    /// 命中测试：控件内坐标 -> 光标位置（CJK 双宽感知）。
    /// P13：x 需先减去水平滚动偏移——点击坐标在视口系，字符列在文档系。
    pub fn hit_test(&self, x: f32, y: f32) -> CursorPos {
        let gutter = self.gutter_width();
        let char_w = self.char_width();
        let line_f = self.scroll_top + (y.max(0.0) / self.line_height());
        let line = ((line_f.floor() as i64).clamp(0, self.doc.line_count() as i64 - 1)) as usize;
        let text = self.line_text(line);
        let rel = (x - gutter + self.scroll_left).max(0.0);

        // P45：真实字形布局命中（与绘制同源，字符边界 = 真实 x 中点，
        // 与列模型的「左半/右半」选位语义一致）；布局滞后（注入的是
        // 上一帧旧文本，长度覆盖不到当前文本）整体回退列模型实时计算
        if let Some(xs) = self.row_layouts.get(&line) {
            if xs.len().saturating_sub(1) >= text.chars().count() {
                let mut col = xs.len() - 1;
                for (k, pair) in xs.windows(2).enumerate() {
                    let mid = (pair[0] + pair[1]) * 0.5;
                    if rel < mid {
                        col = k;
                        break;
                    }
                    col = k + 1;
                }
                return CursorPos { line, col };
            }
        }

        let mut col = text.chars().count();
        let mut acc = 0.0f32; // 累计显示列（含 Tab 制表位推进）
        for (i, ch) in text.chars().enumerate() {
            let w = char_cols(ch, acc as usize);
            // 落在字符左半边选前位，右半边选后位
            if rel < (acc + w * 0.5) * char_w {
                col = i;
                break;
            }
            acc += w;
        }
        CursorPos { line, col }
    }

    /// 第 `line` 行的不含换行文本。
    pub(crate) fn line_text(&self, line: usize) -> String {
        if line >= self.doc.line_count() {
            return String::new();
        }
        self.doc
            .line_str(line)
            .trim_end_matches(['\n', '\r'])
            .to_owned()
    }

    /// 行号栏宽度。
    ///
    /// P59：系数 0.7→1.0——行号以正文字号×1.0 渲染，数字宽 = char_w，
    /// 旧系数让右对齐的行号盒比数字窄 ~30%，右对齐向左溢出画出控件左缘
    /// （iced 文本裁剪对 Cached 文本不生效，无法兜底）。盒宽 ≥ 数字宽后
    /// 溢出消失；GUTTER_MIN 继续充当行号与正文的间距。
    pub fn gutter_width(&self) -> f32 {
        let digits = self.doc.line_count().to_string().len().max(3);
        GUTTER_MIN + digits as f32 * self.char_width()
    }

    // ---------- 光标闪烁（打磨项） ----------

    /// 心跳：翻转闪烁相位（由应用层节拍驱动）。P53：按真实流逝间隔门控
    /// ——淡出动画期间心跳临时加密到 33ms 快拍，相位翻转仍保持
    /// ~CARET_BLINK_MS 节奏，光标闪烁不受动画驱动影响。
    pub fn tick_blink(&mut self) {
        self.tick_blink_at(std::time::Instant::now());
    }

    /// [`Self::tick_blink`] 的可注入时钟版（单测用）。
    fn tick_blink_at(&mut self, now: std::time::Instant) {
        let blink_due = self.last_blink_at.is_none_or(|t| {
            now.duration_since(t).as_millis() >= CARET_BLINK_MS as u128
        });
        if blink_due {
            if self
                .last_activity
                .is_none_or(|t| now.duration_since(t).as_millis() >= CARET_ACTIVE_MS)
            {
                self.blink_on = !self.blink_on;
            }
            self.last_blink_at = Some(now);
        }
    }

    /// 光标活动（移动/输入/点击）：立即常显并重置活动时刻。
    pub fn poke_caret(&mut self) {
        self.blink_on = true;
        self.last_activity = Some(std::time::Instant::now());
    }

    /// 此刻是否应绘制光标：活动窗口期内常显，静止期按相位隐现。
    pub fn caret_visible(&self) -> bool {
        if let Some(t) = self.last_activity {
            if t.elapsed().as_millis() < CARET_ACTIVE_MS {
                return true;
            }
        }
        self.blink_on
    }

    // ---------- P53 竖直滚动条淡入淡出 ----------

    /// 滚动/滚动条交互活动：重置竖直滚动条显示计时（淡出中的条立即回全显）。
    pub fn touch_scrollbar_activity(&mut self) {
        self.sb_activity = Some(std::time::Instant::now());
    }

    /// 当前竖直滚动条绘制 alpha（0 = 隐藏，1 = 全显）。
    pub fn scrollbar_visibility(&self) -> f32 {
        self.scrollbar_visibility_at(std::time::Instant::now())
    }

    /// [`Self::scrollbar_visibility`] 的可注入时钟版（单测用）。
    fn scrollbar_visibility_at(&self, now: std::time::Instant) -> f32 {
        match self.sb_activity {
            None => 0.0,
            Some(t) => scrollbar_alpha(now.duration_since(t).as_millis() as u32),
        }
    }

    /// 淡出动画是否进行中（应用层 CaretTick 常驻链据此切换 33ms 快拍）。
    pub fn scrollbar_fading(&self) -> bool {
        self.scrollbar_fading_at(std::time::Instant::now())
    }

    fn scrollbar_fading_at(&self, now: std::time::Instant) -> bool {
        match self.sb_activity {
            None => false,
            Some(t) => {
                let e = now.duration_since(t).as_millis() as u32;
                e > SCROLLBAR_IDLE_MS && e < SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS
            }
        }
    }

    /// 相对控件的光标矩形（供输入法定位候选框，双宽感知）。
    /// P13：x 含水平滚动偏移的抵扣——返回值是视口系坐标。
    pub fn caret_rect_relative(&self) -> Rectangle {
        let text = self.line_text(self.cursor.line);
        let col = self.cursor.col.min(text.chars().count());
        // P45：滞后感知（同 ensure_visible_horizontal）——布局新鲜走真实
        // 字形位置，滞后回退列模型实时计算；IME 候选框定位同样受益
        let x_px = self.px_of(self.cursor.line, &text, col);
        Rectangle {
            x: self.gutter_width() + x_px - self.scroll_left,
            y: (self.cursor.line as f32 - self.scroll_top) * self.line_height(),
            width: CARET_WIDTH,
            height: self.line_height(),
        }
    }

    pub fn set_viewport_height(&mut self, h: f32) {
        self.viewport_h = h.max(self.line_height());
        self.clamp_scroll();
        // P53：几何变化（窗口缩放/字号调整）后滚动条重新点亮
        self.touch_scrollbar_activity();
    }

    /// 同步视口宽度（P13；RedrawRequested 时与高度一起更新）。
    pub fn set_viewport_width(&mut self, w: f32) {
        self.viewport_w = w.max(0.0);
        self.clamp_scroll_horizontal();
    }

    /// 视口高度（像素）。应用层暂未消费，供测试与后续里程碑（如状态栏显示）使用。
    #[allow(dead_code)]
    pub fn viewport_height(&self) -> f32 {
        self.viewport_h
    }

    /// 控件层专用的可见性保证入口。
    pub(crate) fn ensure_visible_pub(&mut self) {
        self.ensure_visible();
    }

    // ---------- 输入法事件的焦点裁决（P2） ----------

    /// 鼠标按下决定焦点归属：落点在编辑区内 → 接管焦点；
    /// 落点在区外（查找框、工具栏……） → 交出焦点并清掉残留组字串。
    pub fn pointer_focus(&mut self, inside: bool) {
        self.focused = inside;
        if !inside {
            // 焦点离开时组字中断，内联预编辑串不能悬在正文里
            self.preedit = None;
            // P37：失焦打断打字组（去查找框搜一圈再回来，输入不该并组）
            self.break_typing();
        }
    }

    /// 输入法预编辑事件：仅持有焦点时消费（存串并返回 true 供控件捕获），
    /// 否则放行给真正持有焦点的控件（如查找框）。
    pub fn ime_preedit(&mut self, content: String) -> bool {
        if !self.focused {
            return false;
        }
        self.preedit = if content.is_empty() { None } else { Some(content) };
        true
    }

    /// 输入法上屏事件：仅持有焦点时消费。文本的真正插入由应用层
    /// [`Message::Edit`] 统一分发（获得加载期守卫与撤销/置脏语义）。
    pub fn ime_commit(&mut self, text: &str) -> ImeCommit {
        if !self.focused {
            return ImeCommit::Ignored;
        }
        self.preedit = None;
        ImeCommit::Consumed((!text.is_empty()).then(|| text.to_owned()))
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 归一化选区（起点<=终点）；无锚点返回 None。
    pub fn ordered_selection(&self) -> Option<(CursorPos, CursorPos)> {
        let anchor = self.anchor?;
        if (anchor.line, anchor.col) <= (self.cursor.line, self.cursor.col) {
            Some((anchor, self.cursor))
        } else {
            Some((self.cursor, anchor))
        }
    }
}

// ---------- 共享句柄 ----------

/// 跨帧共享的编辑器状态句柄（Clone 廉价）。
#[derive(Clone, Default)]
pub struct EditorHandle(Rc<RefCell<EditorCore>>);

impl std::fmt::Debug for EditorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 内部是 RefCell<EditorCore>（含大量 GUI 态），只输出标识
        f.pad("EditorHandle")
    }
}

impl EditorHandle {
    pub fn borrow(&self) -> std::cell::Ref<'_, EditorCore> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, EditorCore> {
        self.0.borrow_mut()
    }
}


#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
