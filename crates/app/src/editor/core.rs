//! [`EditorCore`]：rope 文档 + 光标/选区/滚动/撤销/IME 的唯一数据源
//! （P68 自 editor.rs 拆出，纯移动零行为变更）。

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use iced::{Color, Font, Rectangle};

use editpad_core::{
    bracket_kind, scan_forward, Document, LazyHighlighter, StyledRun, MAX_BRACKET_SCAN_CHARS,
};

use super::metrics::{
    char_cols, display_cols, measure_insertion, prefix_width,
    validate_measured_char_width, RECOMPUTE_MAX_COLS_COOLDOWN, TAB_STOP_COLS,
};
use super::scrollbars::VERTICAL_SCROLLBAR_RESERVE;
use super::wrap::{segment_index, WrapCache};
use super::{BOOKMARK_STRIP, FONT_SIZE_DEFAULT, GUTTER_MIN};

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

/// 列块（矩形）选区（第 67 轮 ⑮ v1）：anchor=按下角、head=当前对角。
/// 归一化矩形 = 两角的行/列分别取 min/max（含端）；行列全等即空块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSel {
    pub anchor: CursorPos,
    pub head: CursorPos,
}

impl BlockSel {
    /// 归一化 → (r0, r1, c0, c1)，均含端且 r0≤r1、c0≤c1。
    pub(crate) fn normalized(&self) -> (usize, usize, usize, usize) {
        let (r0, r1) = if self.anchor.line <= self.head.line {
            (self.anchor.line, self.head.line)
        } else {
            (self.head.line, self.anchor.line)
        };
        let (c0, c1) = if self.anchor.col <= self.head.col {
            (self.anchor.col, self.head.col)
        } else {
            (self.head.col, self.anchor.col)
        };
        (r0, r1, c0, c1)
    }

    /// 空块 = 行列区间都为零宽（拖拽未展开/单击）。
    pub(crate) fn is_empty(&self) -> bool {
        let (r0, r1, c0, c1) = self.normalized();
        r0 == r1 && c0 == c1
    }
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
    // ---------- 行操作套件（第 57 轮，仿主流编辑器编辑菜单） ----------
    /// 删除光标/选区触及的所有整行（含行尾）
    DeleteLines,
    /// 在触及块正下方复制一份相同内容
    DuplicateLines,
    /// 触及块与上一相邻行整体换位（已在顶行时为 no-op）
    MoveLinesUp,
    /// 触及块与下一相邻行整体换位（已在底行时为 no-op）
    MoveLinesDown,
    // ---------- 大小写转换与行首尾清理（第 58 轮，仿主流编辑器编辑菜单） ----------
    /// 转大小写：有选区只转选区字符，无选区转整个文档
    ConvertCase(CaseKind),
    /// 去行首/行尾空白：有选区只清触及行，无选区清全文档
    TrimLines(TrimMode),
    // ---------- 行排序与去重（第 59 轮，仿主流编辑器行操作菜单） ----------
    /// 行排序：有选区只排触及块，无选区排全文档（尾随换行的幻影末行不参与）
    SortLines(SortOrder),
    /// 去除重复行（保留首次出现、其余行相对次序不变）：有选区只清触及块，
    /// 无选区清全文档
    RemoveDuplicateLines,
    // ---------- 行操作扩充（第 62 轮） ----------
    /// Tab↔空格互转：有选区只转触及块，无选区转全文档
    ConvertTabsSpaces(TabSpaceKind),
    /// 合并行：触及块合成一行；无选区 = 当前行并入下一行（末行 no-op）
    MergeLines,
    /// 拆分行：无选区在光标处断行（回车等价）；有选区把选区独立成行
    SplitLine,
    /// 删除空行/空白行：有选区只清触及块，无选区清全文档（幻影末行不参与）
    DeleteEmptyLines(BlankKind),
    // ---------- 书签套件（第 60 轮，仿主流编辑器书签导航） ----------
    /// 当前行书签开关（有则摘、无则加；随撤销/重做一并回滚）
    ToggleBookmark,
    /// 跳到下一个书签（光标之后最近者，到文档尾回绕开头）
    BookmarkNext,
    /// 跳到上一个书签（光标之前最近者，到文档头回绕末尾）
    BookmarkPrev,
    /// 清除全部书签
    BookmarksClearAll,
    /// 删除全部标记行（含各自行尾；随撤销一并恢复文本与书签）
    RemoveBookmarkedLines,
    /// 复制全部标记行到剪贴板（不改文档、不置脏）
    CopyBookmarkedLines,
    // ---------- 括号匹配（第 61 轮） ----------
    /// 跳到配对括号的另一侧（光标须邻接括号；纯光标移动不产快照）
    JumpToMatchingBracket,
    // ---------- 第 63 轮：插入日期时间 ----------
    /// 在光标处插入当前本地日期时间（F5，记事本同款）
    InsertDateTime,
    // ---------- 第 64 轮：行注释切换 ----------
    /// 行注释切换：触及块全部已注释则去掉一层前缀，否则加上；
    /// 前缀按当前语法查表（未知语法默认 `//`），Ctrl+Q
    ToggleLineComment,
    // ---------- 第 67 轮：列块编辑 ----------
    /// 取消列块选区（Esc；不入热键注册表，固定语义）
    CancelBlock,
}

/// 大小写转换方向（第 58 轮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseKind {
    Upper,
    Lower,
}

/// 行首尾清理模式（第 58 轮）：去行首 / 去行尾 / 两端都去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimMode {
    Leading,
    Trailing,
    Both,
}

/// 行排序方向（第 59 轮）。比较口径 = UTF-8 字节序（即 Unicode 码点序），
/// 大小写敏感：`Z` < `a`；排序稳定，相等行保持原相对次序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Tab↔空格转换方向与范围（第 62 轮）。制表位宽度恒等于渲染层
/// [`super::metrics::TAB_STOP_COLS`]（4），转换结果与绘制对齐严格一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSpaceKind {
    /// 行首制表符 → 空格（首个非空白字符之前的 Tab 展开）
    LeadingTabsToSpaces,
    /// 全部制表符 → 空格
    AllTabsToSpaces,
    /// 行首空白段中的空格 → 制表符（行中空格不参与：行中空格常承担
    /// 对齐语义，收拢会改变视觉列——主流轻量编辑器同口径）
    LeadingSpacesToTabs,
}

/// 空行删除口径（第 62 轮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlankKind {
    /// 仅删除零字符的空行
    Empty,
    /// 「只含空白」的行一并删除（全角空格/NBSP 等 `char::is_whitespace` 口径）
    Whitespace,
}

/// 排序/去重的作用域块（第 59 轮）：有选区 = 触及块，无选区 = 全文档。
/// `lines` 为剥掉行尾的各行正文；幻影末行（文档以换行收尾时 ropey 多出的
/// 末尾空行）不是真实内容——参与排序会把末尾换行挪到文档头、参与去重会
/// 凭空吃掉尾随换行——故排除之，形态由 `push_nl` 在重建时补回。
struct LineBlock {
    /// 光标钳制用的原始触及范围（含被排除的幻影行）
    a: usize,
    b: usize,
    start: usize,
    end: usize,
    /// 重建时是否在块尾补主导行尾（块不在文档末尾恒真；在文档末尾
    /// 时等于「原文档以换行收尾」）
    push_nl: bool,
    lines: Vec<String>,
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
    /// 第 60 轮：书签集合随快照一并入栈——撤销/重做是「整体换文档」，
    /// 行号全盘漂移，唯一可靠的还原方式就是把书签当可回滚状态存档。
    /// 副作用：书签开关/清除也会占用撤销槽（与打字同待遇，见
    /// `toggle_bookmark` 注释）；`remove_bookmarked_lines` 因此能一次
    /// 撤销同时找回文本和书签。
    bookmarks: BTreeSet<usize>,
}

// ---------- 核心状态 ----------

// 两个缓存字段类型较深（clippy type_complexity）：别名让字段声明恢复
// 可读，语义见下方各字段文档（bracket_cache / sel_span_cache）。
type BracketCache = RefCell<Option<(CursorPos, Option<(usize, usize)>)>>;
type SelSpanCache = RefCell<Option<((usize, usize), Option<usize>)>>;

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
    /// 书签行集合（0 起行号，升序）。第 60 轮书签套件的状态底座：
    /// * 会话级标注——不写入文件、不入会话快照（重启即清，主流编辑器
    ///   同口径），也不参与置脏判定；
    /// * 行结构编辑同步平移/删除（各编辑路径内的 remap 注释）；
    /// * 撤销/重做经快照整体回滚（见 `Snapshot::bookmarks`）。
    bookmarks: BTreeSet<usize>,
    /// 括号匹配查询缓存（第 61 轮）：键 = 光标位置，值 = 该位置的匹配
    /// 结果（含 None）。draw 每帧查询、命中即零扫描——孤立括号的封顶
    /// 扫描（MAX_BRACKET_SCAN_CHARS）只在新光标位付一次。RefCell 让
    /// 只读的 draw 也能维护缓存（与高亮器同手法）；内容变更经
    /// `invalidate_highlight_from` 统一失效（全部正文突变路径的唯一汇点）。
    bracket_cache: BracketCache,
    /// 选区显示跨度缓存（第 63 轮状态栏统计）：键 = 选区字节偏移对，
    /// 值 = 显示字符数（None = 无选区）。跨行精确计数是 O(选区行数)，
    /// 状态栏每帧查询必须有缓存；失效走同一汇点（P81 口径）。
    sel_span_cache: SelSpanCache,
    /// 不可见字符覆盖标记开关（第 63 轮）：渲染层读取；不影响文档
    /// 模型/命中测试/查找。经 set_invisibles 由应用层从 Settings 下发。
    pub(crate) show_whitespace: bool,
    pub(crate) show_line_endings: bool,
    /// 列块选区（第 67 轮 ⑮）：None = 普通模式。与单选区（anchor/cursor）
    /// 互斥——建立/存在期间任何普通导航或编辑先清除块态。
    /// pub(crate) 仅限测试直接构造状态；生产路径走 begin/update/finish。
    pub(crate) block_sel: Option<BlockSel>,
    /// 列块拖拽进行中（view 层鼠标状态机的 core 侧镜像）：
    /// true = Alt+Shift 按下未松开，CursorMoved 持续更新 head。
    pub(crate) block_dragging: bool,
    /// 软换行接线态（第 73 轮 ⑯）：开关 + Fenwick 视觉行索引 + 代次失效。
    /// RefCell 手法同高亮器/括号缓存——只读的 draw/hit_test 也要能懒惰
    /// 推进 memo（visible 查询是唯一消费方；确保生产路径读校验）。
    wrap: RefCell<WrapCache>,
    /// 竖向移动的目标像素列（第 73 轮 ⑯，主流 goal-column 口径）：进入
    /// Up/Down/PageUp/PageDown 序列时记录当前光标 x，序列内沿用；任一
    /// 非竖向操作（左右移/编辑/点击/跳转/撤销重做）清除（P37 打断口）。
    goal_px: Option<f32>,
    /// P99：软换行折行预算是否按垂直滚动条可视带宽让位（滚动条需要
    /// 出现时为 true，由控件层 draw 每帧按 `VScrollbar::measure` 的
    /// needed 更新）。开态内容超出视口 → 折行文本在滑块左侧收尾，
    /// 行尾字符不被盖住/显得截断；内容放得下 → 零预留全宽贴边
    /// （P95 口径保留）。判定稳定性见 [`Self::set_wrap_sb_reserve`]。
    wrap_sb_reserve: bool,
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
            bookmarks: BTreeSet::new(),
            bracket_cache: RefCell::new(None),
            sel_span_cache: RefCell::new(None),
            show_whitespace: false,
            show_line_endings: false,
            block_sel: None,
            block_dragging: false,
            wrap: RefCell::new(WrapCache::new()),
            goal_px: None,
            wrap_sb_reserve: false,
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
            .unwrap_or(self.font_size * 0.5625)
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
        // 第 60 轮：新文档 = 新坐标系，旧书签一律作废（会话级标注不入快照）
        self.bookmarks.clear();
        self.recompute_max_line_cols();
        // 第 61 轮：经唯一汇点失效——顺带清括号匹配缓存（光标复位 (0,0)
        // 恰是常见缓存键，静默重载后不得吐旧文档的陈旧配对）
        self.invalidate_highlight_from(0);
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
        // 第 61 轮：本函数是全部正文突变路径的唯一汇点——括号匹配缓存
        // 在此统一失效（光标键控的缓存对「同位异文」不可见，必须显式清）
        self.bracket_cache.borrow_mut().take();
        // 第 63 轮：选区跨度缓存同汇点失效（偏移键控对「同位异文」同理）
        self.sel_span_cache.borrow_mut().take();
        // 第 73 轮 ⑯：软换行缓存同汇点失效——行数变化整表重置，
        // 否则只推代次（memo 过期由下次查询懒惰重算）
        self.wrap.borrow_mut().after_edit(self.doc.line_count());
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
        // 第 60 轮：全部替换后行号与旧内容的对应关系不可信，书签整体作废
        // （撤销仍可经快照找回——书签已随 snapshot() 入栈）
        self.bookmarks.clear();
        self.recompute_max_line_cols();
        // 第 61 轮：经唯一汇点失效——顺带清括号匹配缓存（光标复位 (0,0)，
        // 全部替换后不得吐旧文档的陈旧配对）
        self.invalidate_highlight_from(0);
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
            bookmarks: self.bookmarks.clone(),
        });
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        self.break_typing(); // P37：撤销本身打断组，防后续输入混入历史组
        self.goal_px = None; // 第 73 轮 ⑯：撤销 = 非竖向操作，清 goal
        self.clear_block(); // 第 67 轮：列块不参与快照回滚，一并清除
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
            // 第 60 轮：书签随快照对换回滚（见 Snapshot::bookmarks 注释）
            bookmarks: std::mem::replace(&mut self.bookmarks, snap.bookmarks),
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        // 文档被整体替换，高亮状态全量失效；第 61 轮：经唯一汇点，
        // 括号匹配缓存一并清（撤销落点恰为缓存键时防陈旧命中）
        self.invalidate_highlight_from(0);
        // P45：撤销 = 整个文档替换，宽度结构可能缩短——标记惰性收敛
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.break_typing(); // P37 同上
        self.goal_px = None; // 第 73 轮 ⑯：重做 = 非竖向操作，清 goal
        self.clear_block(); // 第 67 轮：同 undo
        let Some(snap) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
            // 第 60 轮：重做对称回滚书签（同 undo）
            bookmarks: std::mem::replace(&mut self.bookmarks, snap.bookmarks),
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        // 第 61 轮：经唯一汇点失效（同 undo，清括号匹配缓存）
        self.invalidate_highlight_from(0);
        // P45：重做同样整体替换文档——标记惰性收敛（对称 undo）
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    pub fn select_all(&mut self) {
        self.break_typing(); // P37：选区变更打断组
        self.goal_px = None; // 第 73 轮 ⑯：选区变更 = 非竖向操作
        self.clear_block(); // 第 67 轮：块态与单选区互斥
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
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
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

        // 第 60 轮：跨行选区将被替换——先记录 (起点行, 起点列>0, 终点行,
        // 消失行数) 供书签再映射（口径与 touched_lines 一致：末点在行首
        // 时该行不算触及，其内容整体并入结果行）
        let replaced_span: Option<(usize, bool, usize, usize)> =
            self.ordered_selection().and_then(|(s, e)| {
                (e.line > s.line).then(|| {
                    let vanished =
                        if e.col == 0 { e.line - 1 - s.line } else { e.line - s.line };
                    (s.line, s.col > 0, e.line, vanished)
                })
            });
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
        // 第 60 轮：书签再映射——被替换的跨行选区按精化规则搬迁（起点行
        // 前缀幸存才保留自身书签；终点行内容必有幸存 → 书签恒并入结果
        // 行；中间整行丢弃）；插入的换行再把变化点之后的行号下推
        if let Some((sa, keeps, eb, van)) = replaced_span {
            self.remap_replaced_span(sa, keeps, eb, van);
        }
        self.remap_shift_below(first_line, new_lines as isize);
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
        self.clear_block(); // 第 67 轮：块态与单选区互斥
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
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        if self.delete_selection() {
            return;
        }
        if self.cursor == CursorPos::default() {
            return;
        }
        self.snapshot();
        // 第 60 轮：col==0 且非首行 = 将删掉换行单元并上一行（书签取并集）
        let merges_up = self.cursor.col == 0 && self.cursor.line > 0;
        self.move_local(Motion::Left);
        let start = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        // P9：跨行回退落在 CRLF 上时把 `\r\n` 当一个换行单元整体移除。
        // 旧行为只删一半字符：第一下视觉无反应，第二下才真正并行的两行。
        let end = start + 1 + usize::from(self.is_crlf_at(start));
        self.doc.remove_range(start, end);
        if merges_up {
            // 幸存行 = 当前行（move_local 已回退到上一行），被吞行 = 下一行
            self.remap_merge_pair(self.cursor.line, self.cursor.line + 1);
        }
        self.invalidate_highlight_from(start);
        // P13：并行后的新行可能更宽（也可能只是收缩——高水位不回退）
        self.raise_max_line_cols(self.cursor.line..=self.cursor.line);
        self.ensure_visible();
    }

    pub fn delete_forward(&mut self) {
        self.goal_px = None; // 第 73 轮 ⑯：编辑 = 非竖向操作，清 goal
        if self.delete_selection() {
            return;
        }
        let offset = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        if offset >= self.doc.text_len() {
            return;
        }
        // 第 60 轮：光标已在行尾（显示口径）= 将删掉换行单元并下一行进来
        let merges_down = self.cursor.col >= self.line_display_len(self.cursor.line);
        self.snapshot();
        // P9：行尾 Delete 同样按 EOL 单元处理，一下删掉整个 `\r\n`
        let end = offset + 1 + usize::from(self.is_crlf_at(offset));
        self.doc.remove_range(offset, end);
        if merges_down {
            // 幸存行 = 当前行，被吞行 = 下一行（文档末尾的孤立换行也适用：
            // 幻影行消失，并集与平移都是无害 no-op）
            self.remap_merge_pair(self.cursor.line, self.cursor.line + 1);
        }
        self.invalidate_highlight_from(offset);
        // P13：下一行并入当前行，合并结果可能更宽
        let merged = self.cursor.line;
        self.raise_max_line_cols(merged..=merged);
        self.ensure_visible();
    }

    // ---------- 行操作套件（第 57 轮，仿主流编辑器编辑菜单） ----------

    /// 光标/选区触及的行范围（含首尾）。选区末点落在某行行首（col 0）时
    /// 该行不算触及——视觉上选区没有盖到它的任何字符。
    fn touched_lines(&self) -> (usize, usize) {
        match self.ordered_selection() {
            Some((start, end)) => {
                let last =
                    if end.col == 0 && end.line > start.line { end.line - 1 } else { end.line };
                (start.line, last)
            }
            None => (self.cursor.line, self.cursor.line),
        }
    }

    /// 删除光标/选区触及的所有整行（含各自行尾）。返回是否发生删除。
    ///
    /// 边界口径：
    /// - 整个文档只剩一个空行时无事可做；
    /// - 光标停在文末空行（尾随换行产生的幻影行）时退化为吃掉其前面的
    ///   换行单元（`\r\n` 整体），与主流编辑器的 Ctrl+L 手感一致；
    /// - 删除文档末尾若干行后若还剩前文，末尾换行随块一并回收。
    pub fn delete_current_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let start = self.doc.line_to_char(a);
        let end =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let (start, end, removed_rows) = if start == end {
            if a == 0 {
                return false; // 唯一内容为空：无可删
            }
            // 幻影末行：改为移除前一行行尾的换行单元（\r\n 整体或 \n）。
            // start ≥ 1 恒成立（前面至少有一个换行才轮得到空行）。
            let crlf = self.doc.slice_text(start - 2, start) == "\r\n";
            let s = start - usize::from(crlf) - 1;
            (s, start, 1)
        } else {
            (start, end, b - a + 1)
        };
        self.snapshot();
        self.doc.remove_range(start, end);
        // 第 60 轮：书签随行消失/上移（幻影分支恒移除 1 行——空壳随其
        // 换行单元一起消失；常规分支移除触及块 b-a+1 行）
        self.remap_removed_rows(a, removed_rows);
        let at = start.min(self.doc.text_len());
        let line = self.doc.char_to_line(at);
        let col = at - self.doc.line_to_char(line);
        self.cursor = CursorPos { line, col };
        self.anchor = None;
        self.invalidate_highlight_from(start);
        // 行结构整体变化，列高水位可能过估——交惰性收敛（P45/P13 同口径）
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    /// 在触及块的正下方复制一份相同内容，光标落到副本首行。返回是否复制。
    pub fn duplicate_current_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let start = self.doc.line_to_char(a);
        let end =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let text = self.doc.slice_text(start, end);
        if text.is_empty() {
            return false; // 空文档 / 幻影末行无内容可复制
        }
        self.snapshot();
        let nl = self.doc.line_ending().newline();
        // 尾部块本身不带行尾时先补一个主导行尾再插到文档末，
        // 保证副本独立成行而不是拼在原块后面。
        // P80 勘误（第 60 轮书签测试揪出的既有缺陷）：判定依据应为
        // 「块文本是否已带行尾单元」，而非 end < len——文档以换行收尾时
        // 末真实行的块文本自带行尾但 end == len，旧条件误补行尾，
        // 副本前凭空多出一个空行。
        let ins = if text.ends_with('\n') || text.ends_with('\r') {
            text
        } else {
            format!("{nl}{text}")
        };
        self.doc.insert(end, &ins);
        self.invalidate_highlight_from(end);
        self.max_cols_stale = true;
        // 第 60 轮：块副本插入 b-a+1 行，原块之后的书签整体下推；
        // 副本本身不带书签（主流编辑器同口径——书签标的是旧行）
        self.remap_shift_below(b, (b - a + 1) as isize);
        self.anchor = None;
        // 插入点恒为第 b+1 行行首（块内已有行尾 / 已补行尾）
        self.cursor = CursorPos { line: b + 1, col: 0 };
        self.ensure_visible();
        true
    }

    /// 上移/下移光标触及的行块（与相邻行整体换位）。已在边界时不动并返回
    /// false（不产生快照）。多行选区整块旋转、内部相对次序保持。
    pub fn move_current_lines(&mut self, up: bool) -> bool {
        let count = self.doc.line_count();
        let (a, b) = self.touched_lines();
        let (first, last) = if up {
            if a == 0 {
                return false;
            }
            (a - 1, b)
        } else {
            if b + 1 >= count {
                return false;
            }
            (a, b + 1)
        };
        // 光标列尽量保持（钳到原行长度；换位后行长可能不同，仅取近似）
        let keep_col = self.cursor.col.min(self.doc.line_len_chars(self.cursor.line));
        let rs = self.doc.line_to_char(first);
        let re =
            if last + 1 < count { self.doc.line_to_char(last + 1) } else { self.doc.text_len() };
        // 区域内各行内容按方向旋转一行后以主导行尾重建；原区域以行尾
        // 结尾则重建串同样收尾（文档末行无行尾的形态保持）。
        // 混合行尾经此归一到主导行尾——与 P9「编辑不产生混合行尾」同哲学。
        // 第 59 轮起经共用助手剥行尾（口径：`\r\n`/`\n`/孤立 `\r` 皆换行）。
        let mut lines: Vec<String> =
            (first..=last).map(|i| self.line_body_without_eol(i)).collect();
        if up {
            lines.rotate_left(1);
        } else {
            lines.rotate_right(1);
        }
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if re < self.doc.text_len() {
            rebuilt.push_str(nl);
        }
        self.snapshot();
        self.doc.remove_range(rs, re);
        self.doc.insert(rs, &rebuilt);
        self.invalidate_highlight_from(rs);
        self.max_cols_stale = true;
        // 第 60 轮：块内轮转的书签跟随——上移时相邻上行换入块尾、块内
        // 各上移一位；下移对称（相邻下行换入块头、块内各下移一位）
        let (enter_from, enter_to, inner) =
            if up { (a - 1, b, -1i64) } else { (b + 1, a, 1i64) };
        self.bookmarks = self
            .bookmarks
            .iter()
            .map(|&l| {
                if l == enter_from {
                    enter_to
                } else if l >= a && l <= b {
                    (l as i64 + inner) as usize
                } else {
                    l
                }
            })
            .collect();
        self.anchor = None;
        self.cursor = CursorPos { line: if up { a - 1 } else { b + 1 }, col: keep_col };
        self.ensure_visible();
        true
    }

    // ---------- 大小写转换与行首尾清理（第 58 轮，仿主流编辑器编辑菜单） ----------

    /// 大小写转换：有选区只转选区字符，无选区转整个文档。返回是否改动。
    ///
    /// - Unicode 全量映射（`str::to_uppercase/to_lowercase`）：`ß→SS`、
    ///   `ﬁ→FI` 一类映射会改变字符数——区域整体重写，光标/选区按新偏移
    ///   重建（有选区时转换结果保持选中，便于连续操作）；
    /// - 换行符不在大小写映射表里：行数与主导行尾元数据恒不变；
    /// - 幂等保护：内容无可转换字符（纯数字/CJK/空白）时不动、不产快照。
    pub fn convert_case(&mut self, kind: CaseKind) -> bool {
        let (start, end) = match self.selection_offsets() {
            Some((s, e)) if s < e => (s, e),
            _ => (0, self.doc.text_len()),
        };
        if start >= end {
            return false;
        }
        let src = self.doc.slice_text(start, end);
        let out = match kind {
            CaseKind::Upper => src.to_uppercase(),
            CaseKind::Lower => src.to_lowercase(),
        };
        if out == src {
            return false; // 无可转换内容：no-op 不产生撤销快照
        }
        let had_selection = self.anchor.is_some();
        // 行号在换行结构不变的前提下保持；列钳到行长防越界
        let keep = self.cursor;
        self.snapshot();
        self.doc.remove_range(start, end);
        self.doc.insert(start, &out);
        self.invalidate_highlight_from(start);
        // ß→SS 一类长度变化让列高水位可能过估——交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        if had_selection {
            // 新文本整体重新选中：起止偏移换算回行列（end 可等于 text_len）
            let end_off = start + out.chars().count();
            let sl = self.doc.char_to_line(start);
            let sc = start - self.doc.line_to_char(sl);
            let el = self.doc.char_to_line(end_off.min(self.doc.text_len()));
            let ec = end_off.min(self.doc.text_len()) - self.doc.line_to_char(el);
            self.anchor = Some(CursorPos { line: sl, col: sc });
            self.cursor = CursorPos { line: el, col: ec };
        } else {
            self.anchor = None;
            self.cursor = CursorPos {
                col: keep.col.min(self.line_display_len(keep.line)),
                ..keep
            };
        }
        self.ensure_visible();
        true
    }

    /// 去除行首/行尾空白：有选区只清触及行，无选区清全文档。返回是否改动。
    ///
    /// - 空白口径 = `char::is_whitespace`（Unicode White_Space：半角/全角
    ///   空格、Tab、NBSP 等都算），行尾判定先剥换行单元再 trim；
    /// - 与 P73 行操作同哲学：触及块按主导行尾重建，不产生混合行尾；
    ///   文档末行无行尾的形态保持；
    /// - 无任何行需要清理时不动、不产快照（幂等 no-op）。
    pub fn trim_touched_lines(&mut self, mode: TrimMode) -> bool {
        let count = self.doc.line_count();
        let (a, b) = if self.anchor.is_some() {
            self.touched_lines()
        } else {
            (0, count.saturating_sub(1))
        };
        let rs = self.doc.line_to_char(a);
        let re =
            if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        let mut changed = false;
        let mut lines: Vec<String> = Vec::with_capacity(b - a + 1);
        for i in a..=b {
            // 第 59 轮起经共用助手剥行尾（ropey 口径：`\r\n`/`\n`/孤立
            // `\r` 皆换行单元，与 P73 原实现一致）
            let s = self.line_body_without_eol(i);
            let t = match mode {
                TrimMode::Leading => s.trim_start(),
                TrimMode::Trailing => s.trim_end(),
                TrimMode::Both => s.trim(),
            };
            // trim 只删前后缀，字节数相同即内容未变
            changed |= t.len() != s.len();
            lines.push(t.to_owned());
        }
        if !changed {
            return false;
        }
        // 光标行钳进触及范围、列钳到清理后的行长（行可能变短）
        let keep_line = self.cursor.line.clamp(a, b);
        let keep_col = self.cursor.col;
        self.snapshot();
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if re < self.doc.text_len() {
            rebuilt.push_str(nl); // 区域不是文档末尾：补回块尾换行
        }
        self.doc.remove_range(rs, re);
        self.doc.insert(rs, &rebuilt);
        self.invalidate_highlight_from(rs);
        self.max_cols_stale = true;
        self.anchor = None;
        self.cursor =
            CursorPos { line: keep_line, col: keep_col.min(self.line_display_len(keep_line)) };
        self.ensure_visible();
        true
    }

    // ---------- 行排序与去重（第 59 轮，仿主流编辑器行操作菜单） ----------

    /// 取第 `i` 行正文：剥掉行尾换行单元。ropey 换行口径 = `\r\n`、`\n`、
    /// **孤立 `\r`** 三者皆是换行（第 59 轮实测钉死），故末尾的 `\r` 同样
    /// 属换行单元而非内容，必须剥除。
    fn line_body_without_eol(&self, i: usize) -> String {
        let mut s = self.doc.line_str(i);
        if s.ends_with("\r\n") {
            s.truncate(s.len() - 2);
        } else if s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    fn collect_line_block(&self) -> LineBlock {
        let count = self.doc.line_count();
        let len = self.doc.text_len();
        let (a, b) =
            if self.anchor.is_some() { self.touched_lines() } else { (0, count.saturating_sub(1)) };
        let start = self.doc.line_to_char(a);
        let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { len };
        // 幻影末行检测：块到达文档末尾且原文以换行单元收尾（`\n` 或孤立
        // `\r`，ropey 口径两者皆换行）→ 最后一行是空壳。仅在块内还有
        // 更前面的行时才排除（a == b 时无从回退，交由调用方的「不足两行」
        // no-op 兜底，避免下溢）。
        // 第 64 轮勘误（🟠 既有缺陷）：补 b == count-1 守卫——幻影只可能
        // 是文档最后一行；此前凡块区域顶到文档尾就把 b 行当幻影剔除，
        // 「选中含末真实行、未含末尾换行后的空壳」的局部选区会把该行
        // 从重建内容里丢掉（排序/去重/注释切换共用此骨架，均受影响）。
        let phantom_tail = end == len
            && len > start
            && b == count - 1
            && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        // 块尾换行补回判定与幻影排除解耦（第 64 轮勘误第二半）：只要块
        // 区域以换行单元收尾（含「选中真实末行、其行尾即文档末尾换行」），
        // 行数不变式重建就必须补回，否则丢尾随换行。
        let nl_tail = end == len
            && len > start
            && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        let last = if phantom_tail && b > a { b - 1 } else { b };
        let lines: Vec<String> = (a..=last).map(|i| self.line_body_without_eol(i)).collect();
        LineBlock {
            a,
            b,
            start,
            end,
            push_nl: end < len || nl_tail,
            lines,
        }
    }

    /// 用新内容替换收集时的块区域（排序/去重共用的写回路径）：按主导行尾
    /// 重建不产生混合行尾；块尾/文档末尾换行形态保持；光标行列钳回块内，
    /// 选区清空。调用前必须已确认内容确有变化（no-op 不产快照）。
    ///
    /// `map[rel]` = 块内原第 rel 行的新块内位置（None = 内容消失）——
    /// 先快照（携带重排前的书签集）再按映射搬迁书签，最后重写文档字节。
    fn apply_line_block(&mut self, blk: &LineBlock, lines: &[String], map: &[Option<usize>]) {
        let keep_line = self.cursor.line.clamp(blk.a, blk.b);
        let keep_col = self.cursor.col;
        self.snapshot();
        // 第 60 轮：书签跟随行内容搬到新位置
        self.remap_block_mapping(blk.a, map);
        let nl = self.doc.line_ending().newline();
        let mut rebuilt = lines.join(nl);
        if blk.push_nl {
            rebuilt.push_str(nl);
        }
        self.doc.remove_range(blk.start, blk.end);
        self.doc.insert(blk.start, &rebuilt);
        self.invalidate_highlight_from(blk.start);
        // 行结构整体变化 + 行长可能增减——列高水位交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        self.anchor = None;
        // 第 66 轮勘误：keep_line 还要收敛到**新行数域**——块被压缩
        // （删空行/合并）后旧光标行可能超出新行数（随机对拍当场抓住：
        // 全删空文档只剩 1 行而 keep_line 仍是旧的大行号）
        let keep_line = keep_line.min(self.doc.line_count() - 1);
        self.cursor =
            CursorPos { line: keep_line, col: keep_col.min(self.line_display_len(keep_line)) };
        self.ensure_visible();
    }

    /// 行排序：有选区只排触及块，无选区排全文档。返回是否改动。
    ///
    /// - 比较口径 = UTF-8 字节序（码点序），大小写敏感、稳定排序；
    /// - 空白行是真实内容，正常参与排序（只有幻影末行除外，见
    ///   [`Self::collect_line_block`]）；
    /// - 已有序 / 块内不足两行时不动、不产快照（幂等 no-op）；
    /// - 书签跟随行内容走：按稳定排序的排列反演映射搬迁（第 60 轮）。
    pub fn sort_lines(&mut self, order: SortOrder) -> bool {
        let blk = self.collect_line_block();
        if blk.lines.len() <= 1 {
            return false; // 0/1 行无从排序
        }
        let n = blk.lines.len();
        // perm[new_rel] = old_rel（稳定排序）；反演成旧→新映射供书签搬迁
        let mut perm: Vec<usize> = (0..n).collect();
        match order {
            SortOrder::Ascending => perm.sort_by(|&i, &j| blk.lines[i].cmp(&blk.lines[j])),
            SortOrder::Descending => perm.sort_by(|&i, &j| blk.lines[j].cmp(&blk.lines[i])),
        }
        let mut map = vec![None; n];
        for (new_rel, &old_rel) in perm.iter().enumerate() {
            map[old_rel] = Some(new_rel);
        }
        let sorted: Vec<String> = perm.iter().map(|&i| blk.lines[i].clone()).collect();
        if sorted == blk.lines {
            return false; // 已有序：幂等 no-op 不产生撤销快照
        }
        self.apply_line_block(&blk, &sorted, &map);
        true
    }

    /// 去除重复行：保留首次出现、其余行相对次序不变。返回是否改动。
    ///
    /// - 有选区只清触及块，无选区清全文档；逐字节整行比对（含空白差异）；
    /// - 无重复 / 块内不足两行时不动、不产快照（幂等 no-op）；
    /// - 书签跟随：保留行的书签搬到新位置，重复项的书签随行丢弃。
    pub fn remove_duplicate_lines(&mut self) -> bool {
        let blk = self.collect_line_block();
        if blk.lines.len() <= 1 {
            return false;
        }
        let n = blk.lines.len();
        let mut seen: HashSet<&str> = HashSet::with_capacity(n);
        // 旧块内相对位置 → 新块内位置；None = 该行是重复项，内容消失
        let mut map: Vec<Option<usize>> = vec![None; n];
        let mut kept: Vec<String> = Vec::with_capacity(n);
        for (rel, line) in blk.lines.iter().enumerate() {
            if seen.insert(line.as_str()) {
                map[rel] = Some(kept.len());
                kept.push(line.clone());
            }
        }
        if kept.len() == n {
            return false; // 本就无重复：幂等 no-op 不产生撤销快照
        }
        self.apply_line_block(&blk, &kept, &map);
        true
    }

    // ---------- 行操作扩充（第 62 轮）：Tab↔空格 / 合并拆分 / 删空行 ----------

    /// Tab↔空格互转：有选区只转触及块，无选区转全文档。返回是否改动。
    ///
    /// - 制表位宽度 = 渲染层 `TAB_STOP_COLS`，转换与绘制对齐严格一致
    ///   （宽字符占 2 显示列也计入制表位推进，`中\tX` 的 Tab 落到列 4）；
    /// - 无变化时不动、不产快照（幂等 no-op）；行数不变 → 恒等映射，
    ///   书签原位保留。
    pub fn convert_tabs_spaces(&mut self, kind: TabSpaceKind) -> bool {
        let blk = self.collect_line_block();
        let mut changed = false;
        let lines: Vec<String> = blk
            .lines
            .iter()
            .map(|s| {
                let t = match kind {
                    TabSpaceKind::LeadingTabsToSpaces => expand_tabs_in(s, true),
                    TabSpaceKind::AllTabsToSpaces => expand_tabs_in(s, false),
                    TabSpaceKind::LeadingSpacesToTabs => entab_leading_ws(s),
                };
                // 长度可能碰巧相等（列 3 处的 1 个 Tab ↔ 1 空格），必须按值比对
                changed |= t != *s;
                t
            })
            .collect();
        if !changed {
            return false;
        }
        let map: Vec<Option<usize>> = (0..lines.len()).map(Some).collect();
        self.apply_line_block(&blk, &lines, &map);
        true
    }

    /// 合并行：触及块合成一行；无选区 = 当前行并入下一行。返回是否改动。
    ///
    /// - 连接口径：各行 `trim()` 后以**单个空格**连接，空白行直接消失；
    ///   全空块合并为一个空行（行数缩减本身就是改动）；
    /// - 书签：首行内容前缀幸存 → 块内书签全部收敛到合并后的首行；
    ///   其余行的书签随行并入丢弃（撤销可整体找回）；
    /// - 无选区且已在末行 / 块内不足两行 → no-op 不产快照。
    pub fn merge_lines(&mut self) -> bool {
        let count = self.doc.line_count();
        let (a, b) = if self.anchor.is_some() {
            self.touched_lines()
        } else {
            let a = self.cursor.line.min(count.saturating_sub(1));
            if a + 1 >= count {
                return false; // 末行没有「下一行」可并
            }
            (a, a + 1)
        };
        let start = self.doc.line_to_char(a);
        let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { self.doc.text_len() };
        // 区域尾字节是否换行单元（\n / 孤立 \r）：决定重建时是否补回
        // 块尾换行——选中真实末行（其行尾即文档末尾换行）时同样成立，
        // 不能只看「块不在文档末尾」
        let len = self.doc.text_len();
        let nl_tail =
            end == len && end > start
                && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        // 幻影末行排除：仅当 b 本身就是文档最后一行（ropey 在尾随换行后
        // 多出的空壳行）且区域确以换行收尾。b 是唯一真实行的场景无从回
        // 退，直接按无可并内容处理。
        let last = if nl_tail && b == count - 1 {
            if b > a {
                b - 1
            } else {
                return false;
            }
        } else {
            b
        };
        if last <= a {
            return false; // 触及块实际不足两行
        }
        let bodies: Vec<String> =
            (a..=last).map(|i| self.line_body_without_eol(i)).collect();
        let mut joined = String::new();
        for part in &bodies {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(t);
        }
        let blk = LineBlock { a, b, start, end, push_nl: end < len || nl_tail, lines: bodies };
        let n = last - a + 1;
        let map: Vec<Option<usize>> =
            (0..n).map(|rel| if rel == 0 { Some(0) } else { None }).collect();
        self.apply_line_block(&blk, &[joined], &map);
        // 块由 n 行并为 1 行：块尾之下的书签整体上移 n-1 行
        self.shift_bookmarks_below(blk.b, -(n as isize - 1));
        true
    }

    /// 拆分行。返回是否改动。
    ///
    /// - 无选区：在光标处断行（回车等价的可重映射动作）；
    /// - 有选区：把选中文本独立成行——选区前后各插一个换行，前后残文
    ///   各留原行（「摘出片段」语义）；
    /// - 单次快照、主导行尾重建（选区内混合行尾归一）、书签按行结构
    ///   下推/保留：首行（残文前缀所在行）书签原位，其余被拆行的书签
    ///   并入块尾残文行；
    /// - 断在空行正中（前后皆空）→ 内容不变，no-op 不产快照。
    pub fn split_line(&mut self) -> bool {
        let (s, e) = match self.anchor {
            None => (self.cursor, self.cursor),
            Some(_) => self.ordered_selection().expect("有锚必有有序选区"),
        };
        let ls = s.line;
        let le = e.line;
        // 三段重建：pre | 选中文本（内部换行归一为主流行尾再切开）| post
        // （行尾符号本身由 apply_line_block 按主导行尾统一补回，此处不碰）
        let off_s = self.doc.line_to_char(ls) + s.col.min(self.line_display_len(ls));
        let off_e = self.doc.line_to_char(le) + e.col.min(self.line_display_len(le));
        let pre = {
            let body = self.line_body_without_eol(ls);
            let cut = s.col.min(body.chars().count());
            body.chars().take(cut).collect::<String>()
        };
        let post = {
            let body = self.line_body_without_eol(le);
            let cut = e.col.min(body.chars().count());
            body.chars().skip(cut).collect::<String>()
        };
        let middle_raw = if off_e > off_s {
            self.doc.slice_text(off_s, off_e)
        } else {
            String::new()
        };
        // 选区文本内的行尾统一到主流行尾（P26 口径的逆用：\r\n/\r → \n）
        let middle_normalized = {
            let eol = self.doc.line_ending();
            eol.normalize(&middle_raw)
        };
        let middle: Vec<String> = if middle_normalized.is_empty() {
            Vec::new()
        } else {
            middle_normalized
                .trim_end_matches('\n')
                .split('\n')
                .map(|p| p.to_owned())
                .collect()
        };
        // 区域尾形态：与 merge 同一通判——选中真实末行时其行尾即文档
        // 末尾换行，重建必须补回；无尾随换行的文档则不加
        let count = self.doc.line_count();
        let start_off = self.doc.line_to_char(ls);
        let end_off =
            if le + 1 < count { self.doc.line_to_char(le + 1) } else { self.doc.text_len() };
        let len = self.doc.text_len();
        let nl_tail =
            end_off == len && end_off > start_off
                && matches!(self.doc.slice_text(len - 1, len).as_str(), "\n" | "\r");
        let mut new_lines = Vec::with_capacity(middle.len() + 2);
        new_lines.push(pre);
        new_lines.extend(middle.iter().cloned());
        new_lines.push(post);
        // no-op 守卫：新旧逐行一致（如空行正中断行）
        let old_lines: Vec<String> = (ls..=le).map(|i| self.line_body_without_eol(i)).collect();
        if new_lines == old_lines {
            return false;
        }
        // 书签映射：rel0 = pre 行原位；rel k>0 的行内容主体搬到新位置 k
        // （k ≤ middle.len() 时），越界（单行摘出场景）并入块尾 post 行
        let n_new = new_lines.len();
        let span = le - ls;
        let mut map: Vec<Option<usize>> = Vec::with_capacity(span + 1);
        for rel in 0..=span {
            map.push(if rel == 0 {
                Some(0)
            } else if rel <= middle.len() {
                Some(rel)
            } else {
                Some(n_new - 1)
            });
        }
        let blk = LineBlock {
            a: ls,
            b: le,
            start: start_off,
            end: end_off,
            push_nl: end_off < len || nl_tail,
            lines: old_lines,
        };
        self.apply_line_block(&blk, &new_lines, &map);
        // 块行数增量 = 新行数 - 原行数：块尾之下的书签整体平移
        self.shift_bookmarks_below(
            le,
            new_lines.len() as isize - blk.lines.len() as isize,
        );
        // 光标落到断出的新内容行首（无选区 = 后半段行首；选区 = 摘出行首）
        let target = (ls + 1).min(self.doc.line_count().saturating_sub(1));
        self.cursor = CursorPos { line: target, col: 0 };
        self.ensure_visible();
        true
    }

    /// 删除空行/空白行：有选区只清触及块，无选区清全文档。返回是否改动。
    ///
    /// - 幻影末行不参与（尾随换行的有无形态保持，P77 教训口径）；
    /// - 书签随幸存行搬到压缩后的新位置，被删行的书签丢弃（可撤销找回）；
    /// - 无可删行 → no-op 不产快照。
    pub fn delete_empty_lines(&mut self, kind: BlankKind) -> bool {
        let mut blk = self.collect_line_block();
        let mut kept: Vec<String> = Vec::with_capacity(blk.lines.len());
        let mut map: Vec<Option<usize>> = Vec::with_capacity(blk.lines.len());
        for line in &blk.lines {
            let blank = match kind {
                BlankKind::Empty => line.is_empty(),
                BlankKind::Whitespace => line.trim().is_empty(),
            };
            if blank {
                map.push(None);
            } else {
                map.push(Some(kept.len()));
                kept.push(line.clone());
            }
        }
        if kept.len() == blk.lines.len() {
            return false; // 无可删行：幂等 no-op 不产生撤销快照
        }
        let delta = kept.len() as isize - blk.lines.len() as isize;
        // 全删空的特殊形态学：块内一行不剩时，重建串为空，push_nl 的
        // 直觉（「补回块尾换行」）不再适用——此时它决定的是**前后内容的
        // 衔接符**：
        // * 块在文档头或顶到文档尾：外侧本无另一段内容（或其行尾已在
        //   块外），多插一个换行就是凭空多出的空行 → 不补；
        // * 块夹在两段内容中间：左右行各自的换行一个在块前、一个被并进
        //   了删除区，必须补一个换行否则相邻内容粘连成一行。
        if kept.is_empty() {
            blk.push_nl = blk.start > 0 && blk.end < self.doc.text_len();
        }
        self.apply_line_block(&blk, &kept, &map);
        // 净删了若干行：块尾之下的书签整体上移
        self.shift_bookmarks_below(blk.b, delta);
        true
    }

    // ---------- 第 63 轮：选区统计 + 插入日期时间 ----------

    // ---------- 第 67 轮 ⑮：列块（矩形）选区与编辑 ----------

    /// 是否有非空列块选区。
    pub fn has_block(&self) -> bool {
        self.block_sel.is_some_and(|b| !b.is_empty())
    }

    /// 清除列块选区。返回是否原本存在块（调用方据此决定是否刷新）。
    pub fn clear_block(&mut self) -> bool {
        self.block_dragging = false;
        self.block_sel.take().is_some()
    }

    /// Alt+Shift 按下：以命中点为锚角开始块选拖拽。
    /// 第 73 轮 ⑯：软换行开态直接拒绝（设计 §4.10 互斥——列块依赖整行
    /// 矩形列几何，折行形态下无意义；开启软换行本身也会清掉既有块）。
    pub(crate) fn begin_block_select(&mut self, at: CursorPos) {
        if self.wrap.borrow().enabled {
            return;
        }
        self.block_dragging = true;
        self.block_sel = Some(BlockSel { anchor: at, head: at });
        // 块态与单选区互斥
        self.anchor = None;
        self.break_typing();
    }

    /// 拖拽中：更新对角（渲染实时跟随；无效中间态允许存在）。
    pub(crate) fn update_block_select(&mut self, to: CursorPos) {
        if let Some(b) = &mut self.block_sel {
            b.head = to;
        }
    }

    /// 松开鼠标：结束拖拽。返回是否留下有效块（空块自动清除——单击
    /// Alt+Shift 不应残留任何状态）。
    pub(crate) fn finish_block_select(&mut self) -> bool {
        self.block_dragging = false;
        match self.block_sel {
            Some(b) if !b.is_empty() => true,
            _ => {
                self.block_sel = None;
                false
            }
        }
    }

    /// 归一化块矩形 (r0, r1, c0, c1)；每行的实际右界会被该行长度截断
    /// （短行自动到行尾——主流编辑器同口径），由消费方各自处理。
    pub(crate) fn active_block(&self) -> Option<(usize, usize, usize, usize)> {
        let b = self.block_sel?;
        let (r0, r1, c0, c1) = b.normalized();
        (!b.is_empty()).then_some((r0, r1, c0, c1))
    }

    /// 复制块内容：各行截取片段以 `\n` 连接（不含行尾符；短行取到行尾）。
    /// 无块 → None。
    pub fn block_copy_text(&self) -> Option<String> {
        let (r0, r1, c0, c1) = self.active_block()?;
        let mut parts: Vec<String> = Vec::with_capacity(r1 - r0 + 1);
        for line in r0..=r1 {
            let start = self.doc.line_to_char(line);
            let len = self.line_display_len(line);
            let a = start + c0.min(len);
            let b = start + c1.min(len);
            if b > a {
                parts.push(self.doc.slice_text(a, b));
            } else {
                parts.push(String::new());
            }
        }
        Some(parts.join("\n"))
    }

    /// 删除块内容：逐行移除 [c0, c1) 片段（短行无内容则跳过该行），
    /// 行数不变 → 书签原位。产快照；成功后清块、光标落块左上角。
    /// 返回是否改动。
    pub fn delete_block_content(&mut self) -> bool {
        let Some((r0, r1, c0, c1)) = self.active_block() else {
            return false;
        };
        // 预检是否有任一行确有可删内容，全空则 no-op 不产快照
        let spans: Vec<(usize, usize, usize)> = (r0..=r1)
            .filter_map(|line| {
                let start = self.doc.line_to_char(line);
                let len = self.line_display_len(line);
                let a = start + c0.min(len);
                let b = start + c1.min(len);
                (b > a).then_some((a, b, len))
            })
            .collect();
        if spans.is_empty() {
            return false;
        }
        self.snapshot();
        // 从后往前删，前面的偏移不受影响
        for (a, b, _) in spans.iter().rev() {
            self.doc.remove_range(*a, *b);
        }
        self.invalidate_highlight_from(self.doc.line_to_char(r0));
        self.max_cols_stale = true;
        self.block_sel = None;
        self.cursor = CursorPos { line: r0, col: c0.min(self.line_display_len(r0)) };
        self.ensure_visible();
        true
    }

    /// 向块内插入文本（v1：单行文本；含换行时只取首段，文档披露的
    /// 简化口径）。逐行把 [c0, c1) 替换为该文本；行数不变 → 书签原位。
    /// 产快照；成功后清块、光标落首行插入文本之后。返回是否改动。
    pub fn insert_into_block(&mut self, text: &str) -> bool {
        let first_line = text.split('\n').next().unwrap_or("");
        if first_line.is_empty() {
            return false;
        }
        let Some((r0, r1, c0, c1)) = self.active_block() else {
            return false;
        };
        self.snapshot();
        // 先删旧块内容（从后往前），再从后往前逐行插入新文本
        for line in (r0..=r1).rev() {
            let start = self.doc.line_to_char(line);
            let len = self.line_display_len(line);
            let a = start + c0.min(len);
            let b = start + c1.min(len);
            if b > a {
                self.doc.remove_range(a, b);
            }
            self.doc.insert(a, first_line);
        }
        self.invalidate_highlight_from(self.doc.line_to_char(r0));
        self.max_cols_stale = true;
        self.block_sel = None;
        let new_col = c0 + first_line.chars().count();
        self.cursor = CursorPos { line: r0, col: new_col.min(self.line_display_len(r0)) };
        self.ensure_visible();
        true
    }

    /// 下发不可见字符标记开关（设置保存/建页时调用）。
    pub fn set_invisibles(&mut self, whitespace: bool, line_endings: bool) {
        self.show_whitespace = whitespace;
        self.show_line_endings = line_endings;
    }

    /// 光标的全文字符偏移（第 69 轮状态栏「位置」用，1 起显示由调用方
    /// +1）。列即行内字符索引，无宽字符折算——与查找偏移同口径。
    pub fn cursor_offset(&self) -> usize {
        self.doc.line_to_char(self.cursor.line) + self.cursor.col
    }

    /// 行注释前缀查表（第 64 轮）：按当前语法名（syntect 名，大小写
    /// 不敏感匹配）取行注释前缀。未知语法默认 `//`。块注释不入表
    /// （`/* */` 需要成对处理，超出本动作的「整行」范畴）。
    fn comment_prefix_for(&self) -> &'static str {
        let lang = self.highlight_syntax_name().unwrap_or_default();
        let lang = lang.to_ascii_lowercase();
        match lang.as_str() {
            "python" | "ruby" | "shellscript" | "bash" | "yaml" | "toml" | "ini"
            | "properties" | "r" | "perl" => "#",
            "sql" | "lua" => "--",
            "batch file" | "bat" | "dosbatch" => "::",
            _ => "//", // rust/c/cpp/java/js/ts/go/json/css/php/未知……
        }
    }

    /// 行注释切换（Ctrl+Q）。有选区只作用触及块，无选区全文。
    /// 返回是否改动。
    ///
    /// - 判定：块内**全部非空行的缩进后都已带本前缀** → 去（每行剥一层：
    ///   前缀 + 紧随的一个空格，若有）；否则 → 加（在每行首个非空白字符
    ///   前插 `前缀 + 空格`；纯空行跳过不参与）；
    /// - 行数不变 → 恒等映射书签原位；无变化 no-op 不产快照；
    /// - 前缀只认「缩进后紧跟」——字符串里出现的 `//` 不受影响（不做
    ///   语法感知是既有取舍，与括号匹配同口径）。
    pub fn toggle_line_comment(&mut self) -> bool {
        let prefix = self.comment_prefix_for();
        let blk = self.collect_line_block();
        let is_commented = |s: &str| -> bool {
            let t = s.trim_start();
            !t.is_empty() && t.starts_with(prefix)
        };
        let commentable: Vec<bool> = blk.lines.iter().map(|s| !s.trim().is_empty()).collect();
        // 全部非空行已注释 → 去一层；否则加一层
        let unwrap_mode = blk
            .lines
            .iter()
            .zip(commentable.iter())
            .all(|(s, &c)| !c || is_commented(s));
        let mut changed = false;
        let lines: Vec<String> = blk
            .lines
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if !commentable[i] {
                    return s.clone();
                }
                let out = if unwrap_mode {
                    // 剥：缩进保留，去前缀 + 至多一个紧随空格
                    let t = s.trim_start();
                    let ind = &s[..s.len() - t.len()];
                    let rest = &t[prefix.len()..];
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{ind}{rest}")
                } else {
                    // 加：插到首个非空白字符前
                    let t = s.trim_start();
                    let ind = &s[..s.len() - t.len()];
                    format!("{ind}{prefix} {t}")
                };
                changed |= out != *s;
                out
            })
            .collect();
        if !changed {
            return false;
        }
        let map: Vec<Option<usize>> = (0..lines.len()).map(Some).collect();
        self.apply_line_block(&blk, &lines, &map);
        true
    }

    /// 当前选区的显示字符数（None = 无选区/零宽）。口径与查找命中的
    /// `len_chars` 一致：行尾 `\r\n` 计 1、每跨一行计 1——可直接与
    /// 「字符数」相加对账。
    ///
    /// 跨行计数是 O(选区行数)，而状态栏每帧都会查询：经偏移对键控缓存
    /// （sel_span_cache，同一失效汇点），锚点/光标不动时命中帧零开销。
    pub fn selection_display_len(&self) -> Option<usize> {
        let (s, e) = self.ordered_selection()?;
        let key = (
            self.doc.line_to_char(s.line) + s.col,
            self.doc.line_to_char(e.line) + e.col,
        );
        if let Some(hit) = self.sel_span_cache.borrow().as_ref() {
            if hit.0 == key {
                return hit.1;
            }
        }
        let span: Option<usize> = if s.line == e.line {
            Some(e.col.saturating_sub(s.col))
        } else if s.line < self.doc.line_count() && e.line < self.doc.line_count() {
            // 首行余部 + 其换行占 1；中间各行 disp+1；末行前缀 e.col
            let mut n = self.line_display_len(s.line) - s.col.min(self.line_display_len(s.line)) + 1;
            for l in (s.line + 1)..e.line {
                n += self.line_display_len(l) + 1;
            }
            Some(n + e.col)
        } else {
            None
        };
        *self.sel_span_cache.borrow_mut() = Some((key, span));
        span
    }

    /// F5：在光标处插入当前本地日期时间（记事本同款）。
    /// 格式 `YYYY-MM-DD HH:MM`（24 小时制）；本地时区不可得时降级 UTC
    /// 并在状态栏提示（time 的 now_local 在极端环境可能 Err）。
    /// 返回插入的时间戳文本（应用层状态栏反馈用）。
    pub fn insert_date_time(&mut self) -> String {
        let stamp = local_datetime_stamp();
        self.insert_str(&stamp);
        stamp
    }

    // ---------- 书签套件（第 60 轮，仿主流编辑器书签导航） ----------
    //
    // 状态底座 = [`EditorCore::bookmarks`]（升序 BTreeSet，行号 0 起）。
    // 本节后半是各编辑路径共用的书签再映射助手——约定两条：
    // 1. 只在「确认要改动」之后调用（no-op 提前返回的路径不碰集合）；
    // 2. 映射一律从旧集合函数式重建新集合，杜绝逐条 remove/insert 的
    //    顺序陷阱。

    /// 当前行是否带书签（渲染层逐可见行查询，O(log n)）。
    pub fn is_bookmarked(&self, line: usize) -> bool {
        self.bookmarks.contains(&line)
    }

    /// 全部书签行（升序）。测试与应用层诊断消费。
    pub fn bookmarked_lines(&self) -> Vec<usize> {
        self.bookmarks.iter().copied().collect()
    }

    /// 当前行书签开关。返回切换后的状态（true = 现在带书签）。
    ///
    /// 每次实际翻转都先入撤销栈（快照携带翻转前的书签集，见
    /// [`Snapshot::bookmarks`]）——书签是可回滚状态，代价是开关会打断
    /// 打字组并清空重做栈，与一切状态变更操作同待遇。
    pub fn toggle_bookmark(&mut self) -> bool {
        self.snapshot();
        let line = self.cursor.line;
        if !self.bookmarks.remove(&line) {
            self.bookmarks.insert(line);
            return true;
        }
        false
    }

    /// 清除全部书签。返回是否有清除动作（空集合为幂等 no-op，不入栈）。
    pub fn clear_bookmarks(&mut self) -> bool {
        if self.bookmarks.is_empty() {
            return false;
        }
        self.snapshot();
        self.bookmarks.clear();
        true
    }

    /// 下一个/上一个书签的行号（[`Self::next_bookmark`] 的纯查询半边）：
    /// 光标之后（前）最近者；没有更近的就回绕到第一个（最后一个）。
    fn next_bookmark_line(&self, forward: bool) -> Option<usize> {
        let cur = self.cursor.line;
        if forward {
            self.bookmarks
                .range((std::ops::Bound::Excluded(cur), std::ops::Bound::Unbounded))
                .next()
                .or_else(|| self.bookmarks.iter().next())
                .copied()
        } else {
            self.bookmarks
                .range((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(cur)))
                .next_back()
                .or_else(|| self.bookmarks.iter().next_back())
                .copied()
        }
    }

    /// 跳到下一个（`forward`）/上一个书签。唯一书签恰为当前行时原地不动
    /// 并返回 false；无任何书签同理。纯光标移动：不产生快照，但按 P37
    /// 口径打断打字组。
    pub fn next_bookmark(&mut self, forward: bool) -> bool {
        let Some(target) = self.next_bookmark_line(forward) else {
            return false;
        };
        if target == self.cursor.line {
            return false;
        }
        self.break_typing();
        self.anchor = None;
        self.cursor = CursorPos { line: target, col: 0 };
        self.ensure_visible();
        true
    }

    /// 复制全部标记行：按升序取各行正文（剥换行单元）以主导行尾连接，
    /// 每行末尾都带行尾——粘贴到他处保持整行语义。无书签返回 None。
    /// 只读操作：不改文档、不置脏、不产快照。
    pub fn copy_bookmarked_lines(&self) -> Option<String> {
        if self.bookmarks.is_empty() {
            return None;
        }
        let nl = self.doc.line_ending().newline();
        let mut out = String::new();
        for &l in &self.bookmarks {
            if l >= self.doc.line_count() {
                continue; // 防御：悬空行号不参与（正常路径不可达）
            }
            out.push_str(&self.line_body_without_eol(l));
            out.push_str(nl);
        }
        (!out.is_empty()).then_some(out)
    }

    /// 删除全部标记行（含各自行尾）。返回是否发生删除。
    ///
    /// * 字节区间按原文档一次算齐、自底向上逐段移除——高地址段先行，
    ///   低地址偏移全程有效；
    /// * 连续标记行合并成一段，最小化 remove 次数；
    /// * 幻影末行（文档以换行收尾时空出的末行）单独被标记时退化为吃掉
    ///   它前面的换行单元，与 [`Self::delete_current_lines`] 的幻影分支
    ///   同口径；
    /// * 成功后书签集合自然清空（删的就是全部书签行）；撤销经快照把
    ///   文本与书签一并找回。
    pub fn remove_bookmarked_lines(&mut self) -> bool {
        if self.bookmarks.is_empty() {
            return false;
        }
        let count = self.doc.line_count();
        let len = self.doc.text_len();
        let marked = self.bookmarked_lines();

        // 连续行合并成段 [r0..=r1]
        let mut runs: Vec<(usize, usize)> = Vec::with_capacity(marked.len());
        let mut r0 = marked[0];
        let mut prev = marked[0];
        for &l in &marked[1..] {
            if l == prev + 1 {
                prev = l;
            } else {
                runs.push((r0, prev));
                r0 = l;
                prev = l;
            }
        }
        runs.push((r0, prev));

        // 段 → 字节区间
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(runs.len());
        let mut first_deleted_at = usize::MAX;
        for (a, b) in runs {
            let start = self.doc.line_to_char(a);
            let end = if b + 1 < count { self.doc.line_to_char(b + 1) } else { len };
            if start < end {
                spans.push((start, end));
            } else if start > 0 {
                // 幻影末行空壳单标：改为移除其前面的换行单元（\r\n 整体）
                let crlf = self.doc.slice_text(start - 2, start) == "\r\n";
                spans.push((start - usize::from(crlf) - 1, start));
            }
            // start == end == 0：空文档唯一空行被标，无可删（跳过）
            first_deleted_at = first_deleted_at.min(spans.last().map_or(usize::MAX, |r| r.0));
        }
        if spans.is_empty() {
            return false;
        }

        self.snapshot();
        spans.sort_unstable_by_key(|x| std::cmp::Reverse(x.0)); // 自底向上
        for (s, e) in spans {
            self.doc.remove_range(s, e);
        }
        let at = first_deleted_at.min(self.doc.text_len());
        let line = self.doc.char_to_line(at);
        let col = at - self.doc.line_to_char(line);
        self.cursor = CursorPos { line, col };
        self.anchor = None;
        self.invalidate_highlight_from(at);
        // 行结构整体变化——列高水位交惰性收敛（P45 口径）
        self.max_cols_stale = true;
        self.bookmarks.clear();
        self.ensure_visible();
        true
    }

    // ---------- 书签再映射助手（各编辑路径调用） ----------

    /// 变化点之后的书签整体平移 `delta`（行号 > `from_exclusive` 参与；
    /// 平移出文档范围（≤0）的书签丢弃——防御，正常路径不可达）。
    fn remap_shift_below(&mut self, from_exclusive: usize, delta: isize) {
        if delta == 0 || self.bookmarks.is_empty() {
            return;
        }
        self.bookmarks = self
            .bookmarks
            .iter()
            .filter_map(|&l| {
                let moved =
                    if l > from_exclusive { l as isize + delta } else { l as isize };
                usize::try_from(moved).ok()
            })
            .collect();
    }

    /// 删除 `[first, first+count)` 行后的重映射：区间内书签随行消失，
    /// 其后整体上移 count（[`Self::delete_current_lines`] /
    /// [`Self::remove_bookmarked_lines`] 幻影分支共用）。
    fn remap_removed_rows(&mut self, first: usize, count: usize) {
        let gone_end = first + count;
        self.bookmarks = self
            .bookmarks
            .iter()
            .filter_map(|&l| match l {
                _ if l >= gone_end => Some(l - count),
                _ if l >= first => None,
                _ => Some(l),
            })
            .collect();
    }

    /// 两行并成一行的重映射（回退删除换行单元 / Delete 吞掉换行单元）：
    /// 被吞行与幸存行的书签取**并集**落在幸存行上（任一来源有标记即视
    /// 为合并结果有标记），其后行号整体 −1。
    fn remap_merge_pair(&mut self, survivor: usize, gone: usize) {
        let had_gone = self.bookmarks.remove(&gone);
        if had_gone {
            self.bookmarks.insert(survivor);
        }
        self.remap_shift_below(gone, -1);
    }

    /// 跨行区域被替换（选区删除/替换、跨行选区粘贴）的重映射：
    /// * 起点行前缀幸存（`start_keeps` = 选区起点列 > 0）时保留自身书签，
    ///   整行被吃掉则丢弃；
    /// * 终点行内容必有幸存部分（终点列 > 0 剩后缀；= 0 则整行并入结果
    ///   行）→ 其书签恒**并入**结果行；
    /// * 中间整行内容消失 → 书签丢弃；
    /// * 其后行按消失行数 `vanished` 上移。
    fn remap_replaced_span(
        &mut self,
        start_line: usize,
        start_keeps: bool,
        end_line: usize,
        vanished: usize,
    ) {
        // 结果行书签 = (起点行自身 ∧ 前缀幸存) ∨ 终点行并入——两项独立
        // 成立即保留，起点行整行被吃不得连坐终点行并进来的书签
        let had_start = self.bookmarks.contains(&start_line) && start_keeps;
        let had_end = self.bookmarks.remove(&end_line);
        if had_start || had_end {
            self.bookmarks.insert(start_line);
        } else {
            self.bookmarks.remove(&start_line);
        }
        let interior: Vec<usize> =
            self.bookmarks.range(start_line + 1..end_line).copied().collect();
        for l in interior {
            self.bookmarks.remove(&l);
        }
        self.remap_shift_below(end_line, -(vanished as isize));
    }

    /// 块级重排（排序/去重）的按映射搬迁：`map[rel]` = 块内原第 rel 行的
    /// 新位置；`None` = 该行内容消失（去重的重复项），书签丢弃。映射表
    /// 不覆盖的行（如块外的幻影末行）原位保留——排序/去重不改变块前行数
    /// 与尾随换行的有无，幻影行号恒不变。
    fn remap_block_mapping(&mut self, start_line: usize, map: &[Option<usize>]) {
        if self.bookmarks.is_empty() {
            return;
        }
        let mut out = BTreeSet::new();
        for &l in &self.bookmarks {
            let new_line = match l.checked_sub(start_line).and_then(|rel| map.get(rel)) {
                Some(Some(new_rel)) => start_line + new_rel,
                Some(None) => continue,
                None => l,
            };
            out.insert(new_line);
        }
        self.bookmarks = out;
    }

    /// 第 62 轮：块外书签平移。合并/拆分/删空行经 [`Self::apply_line_block`]
    /// 改变行数时，块内映射管不到的「块尾之下」行号必须按块的行数增量
    /// 整体平移（排序/去重行数不变故无此需求）。`boundary` = 块的最后一
    /// 个原始行号，`delta` 为负即上方净删了行。须在 apply 之后调用
    /// （apply 内部已先行快照，撤销可整体回滚）。
    fn shift_bookmarks_below(&mut self, boundary: usize, delta: isize) {
        if delta == 0 || self.bookmarks.is_empty() {
            return;
        }
        self.bookmarks = self
            .bookmarks
            .iter()
            .map(|&l| {
                if l > boundary {
                    (l as isize + delta).max(0) as usize
                } else {
                    l
                }
            })
            .collect();
    }

    // ---------- 括号匹配（第 61 轮） ----------

    /// 光标邻接括号与其配对位置（渲染高亮与跳转共用的查询，带缓存）。
    ///
    /// 邻接口径：光标**右侧**字符是括号优先（光标停在括号前），否则看
    /// **左侧**字符（光标停在括号后）。返回 `(括号偏移, 配对偏移)`——
    /// 全文字符偏移口径（CRLF 的 `\r` 计 1 字符）。无邻接括号 / 扫描
    /// 超上限 / 未配平 → None。
    pub fn bracket_match(&self) -> Option<(usize, usize)> {
        let key = self.cursor;
        if let Some(hit) = self.bracket_cache.borrow().as_ref() {
            if hit.0 == key {
                return hit.1;
            }
        }
        let result = self.bracket_match_uncached();
        *self.bracket_cache.borrow_mut() = Some((key, result));
        result
    }

    /// [`Self::bracket_match`] 的无缓存实现。
    fn bracket_match_uncached(&self) -> Option<(usize, usize)> {
        let off = self.doc.line_to_char(self.cursor.line)
            + self.cursor.col.min(self.line_display_len(self.cursor.line));
        // 右侧字符优先，其次左侧；char_at 越界返回 None 天然覆盖文档尾
        let probe = self
            .char_at(off)
            .and_then(bracket_kind)
            .map(|k| (off, k))
            .or_else(|| {
                (off > 0)
                    .then(|| self.char_at(off - 1))
                    .flatten()
                    .and_then(bracket_kind)
                    .map(|k| (off - 1, k))
            });
        let (boff, (open, close, is_open)) = probe?;
        let other = if is_open {
            boff + scan_forward(open, close, self.doc.chars_from(boff + 1))?
        } else {
            boff - self.scan_backward_chunks(boff, open, close)?
        };
        Some((boff, other))
    }

    /// 反向括号扫描（第 61 轮）：`boff` 处是**闭**括号，向文档头找配对
    /// 开括号，返回**含端字符距离**（`配对偏移 = boff - 返回值`）。
    ///
    /// 实现注记：ropey 1.6 的字符迭代器不支持反向（Chars 非
    /// DoubleEndedIterator），故按 64K 字符分块取切片、块内逆序计数——
    /// 缓冲有界（块大小 × 1），不产生全文 String；总步数受
    /// [`MAX_BRACKET_SCAN_CHARS`] 封顶（与正向同口径）。
    fn scan_backward_chunks(&self, boff: usize, open: char, close: char) -> Option<usize> {
        const CHUNK: usize = 65536;
        let mut consumed = 0usize;
        let mut depth = 1usize;
        let mut buf: Vec<char> = Vec::with_capacity(CHUNK.min(boff));
        let mut end = boff;
        loop {
            let start = end.saturating_sub(CHUNK);
            buf.clear();
            buf.extend(self.doc.slice_text(start, end).chars());
            for &c in buf.iter().rev() {
                consumed += 1;
                if consumed > MAX_BRACKET_SCAN_CHARS {
                    return None;
                }
                if c == close {
                    depth += 1;
                } else if c == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(consumed);
                    }
                }
            }
            if start == 0 {
                return None;
            }
            end = start;
        }
    }

    /// 跳到配对括号的另一侧。光标停在括号前 → 落到配对括号后；停在
    /// 括号后 → 落到配对括号前（保持「相对括号同侧」的手感，来回按即
    /// 在两侧往返）。无邻接括号/无匹配返回 false 不动。纯光标移动：
    /// 不产生快照，按 P37 口径打断打字组。
    pub fn jump_to_matching_bracket(&mut self) -> bool {
        let Some((boff, other)) = self.bracket_match() else {
            return false;
        };
        let cursor_off = self.doc.line_to_char(self.cursor.line) + self.cursor.col;
        let target = if cursor_off <= boff { other + 1 } else { other };
        let target = target.min(self.doc.text_len());
        let line = self.doc.char_to_line(target);
        let col = target - self.doc.line_to_char(line);
        self.break_typing();
        self.anchor = None;
        self.cursor = CursorPos { line, col };
        self.ensure_visible();
        true
    }

    /// 有选区时删除之（含快照）；返回是否发生了删除。零宽选区仅清除标记。
    fn delete_selection(&mut self) -> bool {
        if self.selected_text().is_some() {
            // 第 60 轮：跨行选区删除 = 多行并一行——先记 (起点行, 起点列>0,
            // 终点行, 消失行数)，删除后按精化规则再映射书签（同 insert_str）
            let span = self.ordered_selection().and_then(|(s, e)| {
                (e.line > s.line).then(|| {
                    let vanished =
                        if e.col == 0 { e.line - 1 - s.line } else { e.line - s.line };
                    (s.line, s.col > 0, e.line, vanished)
                })
            });
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
                if let Some((sa, keeps, eb, van)) = span {
                    self.remap_replaced_span(sa, keeps, eb, van);
                }
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
        self.clear_block(); // 第 67 轮：键盘移动退出块态（块内编辑走专属分支）
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

        // 第 73 轮 ⑯：软换行开态，Up/Down/PageUp/PageDown 按视觉行行走
        // （goal-column 口径）；其余动作为非竖向操作，先清 goal
        if self.wrap.borrow().enabled {
            match motion {
                Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                    let Some((line, col)) = self.vertical_target(motion, page_rows) else {
                        return;
                    };
                    self.cursor = CursorPos {
                        line,
                        col: col.min(self.line_display_len(line)),
                    };
                    return;
                }
                _ => self.goal_px = None,
            }
        }

        match motion {
            Motion::Left => {
                self.goal_px = None;
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                } else if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.line_display_len(self.cursor.line);
                }
            }
            Motion::Right => {
                self.goal_px = None;
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
            Motion::Home => {
                self.goal_px = None;
                self.cursor.col = 0
            }
            Motion::End => {
                self.goal_px = None;
                self.cursor.col = self.line_display_len(self.cursor.line)
            }
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
            Motion::DocStart => {
                self.goal_px = None;
                self.cursor = CursorPos::default()
            }
            Motion::DocEnd => {
                self.goal_px = None;
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
        self.goal_px = None; // 第 73 轮 ⑯：跳转 = 非竖向操作
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
        self.goal_px = None; // 第 73 轮 ⑯：恢复定位 = 非竖向操作
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
        // 第 73 轮 ⑯：行程按视觉行总数（软换行开态 = 段计数前缀，
        // 关态 = 逻辑行数，恒等退化）
        let rows_total = self.scroll_content_lines() as f32;
        let max = (rows_total - self.viewport_h / self.line_height()).max(0.0);
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
        // 第 73 轮 ⑯：软换行开态锁水平滚动为 0（设计 §0：hscroll 隐藏、
        // scroll_left 锁 0——折行形态不存在横向行程）
        if self.wrap.borrow().enabled {
            self.scroll_left = 0.0;
            return;
        }
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
        // 第 73 轮 ⑯：软换行开态按视觉行收敛（光标所在段 = 视觉行）
        if self.wrap.borrow().enabled {
            let cur_v = self.visual_row_of(self.cursor.line, self.cursor.col) as f32;
            let last = self.scroll_top + rows_visible - 1.0;
            if cur_v < first {
                self.scroll_top = cur_v;
            } else if cur_v > last {
                self.scroll_top = cur_v - rows_visible + 1.0;
            }
            self.clamp_scroll();
            // P53：视口确实随光标移动才点亮滚动条（行内打字不无谓点亮）
            if (self.scroll_top - first).abs() > f32::EPSILON {
                self.touch_scrollbar_activity();
            }
            return; // 开态无水平行程（scroll_left 恒 0），横向收敛跳过
        }
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
    /// 第 73 轮 ⑯：软换行开态反解为「覆盖可见视觉行的逻辑行区间」。
    pub fn visible_range(&self) -> (usize, usize) {
        let count = self.doc.line_count();
        if self.wrap.borrow().enabled {
            if count == 0 {
                return (0, 0);
            }
            let total = self.visual_rows_total();
            let first_raw = (self.scroll_top.floor() as i64).max(0) as u32;
            let first = first_raw.min(total.saturating_sub(1));
            let rows = (self.viewport_h / self.line_height()).ceil() as u32 + 1;
            let last = first.saturating_add(rows).min(total.saturating_sub(1));
            return (self.locate_visual(first).0, self.locate_visual(last).0);
        }
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

    // ---------- 软换行（第 73 轮 ⑯）视觉行映射 ----------
    //
    // 模型（docs/soft-wrap-design.md §3）：折行完全建立在显示列上，
    // rope 仍是唯一事实源，本组函数是纯派生缓存。开关关闭时全部函数
    // 恒等退化（视觉行 = 逻辑行），生产路径零行为变更。
    // 列预算：正文区可视宽 − 滚动条覆盖区，与渲染同口径（char_cols）。

    /// 软换行开关（设置项 word_wrap 下发）。
    pub fn wrap_enabled(&self) -> bool {
        self.wrap.borrow().enabled
    }

    /// 设置软换行开关（应用层 Settings 下发，fresh_tab 幂等）。
    /// 开启瞬间：清列块（设计 §4.10 互斥）、锁水平滚动为 0、整表重置；
    /// 关闭瞬间：清竖向目标列（goal 只在开态有意义）。
    pub fn set_word_wrap(&mut self, enabled: bool) {
        if enabled == self.wrap.borrow().enabled {
            return;
        }
        if enabled {
            self.clear_block();
            self.scroll_left = 0.0;
            let (lines, cols) = (self.doc.line_count(), self.wrap_max_cols());
            let px = self.wrap_max_px();
            self.wrap.borrow_mut().enable(lines, cols, px);
        } else {
            self.wrap.borrow_mut().disable();
            self.goal_px = None;
        }
        self.clamp_scroll();
    }

    /// P99：设定折行预算是否预留垂直滚动条可视带宽。判定 = 垂直滚动条
    /// needed（内容超出视口），由控件层 draw 每帧按当前行程测量注入。
    ///
    /// 稳定性（无逐帧翻转）：折行段宽预算收缩只会**增加**（不会减少）
    /// 视觉行数——判定「需要滚动条」时预留，预留后行数更多、滚动条仍
    /// needed，状态自持；判定「放得下」时取消预留，取消后行数更少、
    /// 依然放得下，同样自持。两个方向各自自洽，仅内容/视口跨越边界
    /// 时切换一次。
    ///
    /// 预算变化经 [`Self::wrap_max_px`] 反映到 WrapCache 同步键 → 整表
    /// 重置 → 可见行经 draw 惰性收敛（v1 已披露的收敛模型），切换帧
    /// 内渲染以新预算排布，滚动范围下一帧对齐。
    pub fn set_wrap_sb_reserve(&mut self, reserve: bool) {
        self.wrap_sb_reserve = reserve;
    }

    /// 软换行可用显示列预算 = 折行预算 ÷ 列宽，≥1。
    /// P96 起仅作**列模型回退**用（真实字形布局未注入时，见
    /// [`Self::wrap_line_breaks`]）；有真实布局时按
    /// [`Self::wrap_max_px`] 像素断行。
    fn wrap_max_cols(&self) -> usize {
        let cw = self.char_width().max(0.1);
        let w = self.wrap_budget_px();
        (w / cw).floor().max(1.0) as usize
    }

    /// 软换行段宽像素预算 = 折行预算（来源见 [`Self::wrap_budget_px`]）。
    fn wrap_max_px(&self) -> f32 {
        self.wrap_budget_px()
    }

    /// 折行预算（像素，列模型与像素断行的单一来源）。
    ///
    /// P95：开态**不预留水平滚动条槽位**——hscroll 在软换行下恒隐藏
    /// （HScrollbar needed=false），预留的 SCROLLBAR_ZONE_W+2 会让折行
    /// 文本在窗口右缘留下一片可见空白（贴边留白观感）。
    ///
    /// P99：**垂直**滚动条需要出现（内容超出视口）时按
    /// [`VERTICAL_SCROLLBAR_RESERVE`] 让位——折行文本在滑块左侧收尾，
    /// 行尾字符不再被盖住/显得截断；内容放得下（无滚动条）时零预留
    /// 全宽贴边（P95 口径保留）。判定与稳定性见
    /// [`Self::set_wrap_sb_reserve`]。
    fn wrap_budget_px(&self) -> f32 {
        let full = self.text_viewport_w().max(4.0);
        if self.wrap_sb_reserve {
            (full - VERTICAL_SCROLLBAR_RESERVE).max(4.0)
        } else {
            full
        }
    }

    /// 总视觉行数（开关关 = 逻辑行数；开 = Fenwick 段数前缀总和）。
    pub fn visual_rows_total(&self) -> u32 {
        if !self.wrap.borrow().enabled {
            return self.doc.line_count() as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
        w.index.total()
    }

    /// (line, col) 的视觉行号（开关关 = line）。
    pub fn visual_row_of(&self, line: usize, col: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return line as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
        let lines = self.doc.line_count();
        let line = line.min(lines);
        let prefix = w.index.prefix_rows(line);
        if line >= lines {
            return prefix;
        }
        let body = self.line_text(line);
        let real_xs = self.row_layouts.get(&line).map(|v| v.as_slice());
        let breaks = w.segments_of(line, &body, real_xs);
        let seg = segment_index(&breaks, col, body.chars().count());
        prefix + seg as u32
    }

    /// 逻辑行 `line` 的视觉段数（测试诊断用）。
    #[cfg(test)]
    pub fn line_visual_segments(&self, line: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return 1;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
        let body = self.line_text(line);
        let real_xs = self.row_layouts.get(&line).map(|v| v.as_slice());
        w.segments_of(line, &body, real_xs).len() as u32
    }

    /// 逻辑行 `line` 的段首列向量（调用方常已持有正文，免二次取串；
    /// 内部按需重算并差值更新 BIT）。
    pub(crate) fn segments_of_line(&self, line: usize, body: &str) -> Rc<Vec<usize>> {
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
        let real_xs = self.row_layouts.get(&line).map(|v| v.as_slice());
        w.segments_of(line, body, real_xs)
    }

    /// 视觉行反解：`(逻辑行, 段序, 段首列, 段末列)`。段末列 = 下一段
    /// 段首或行尾（含）。`visual` 越界钳到末行末段。
    /// 关态恒等退化：返回 `(visual, 0, 0, 行长)`。
    pub fn locate_visual(&self, visual: u32) -> (usize, usize, usize, usize) {
        let lines = self.doc.line_count();
        if !self.wrap.borrow().enabled {
            let line = (visual as usize).min(lines.saturating_sub(1));
            return (line, 0, 0, self.line_display_len(line));
        }
        if lines == 0 {
            return (0, 0, 0, 0);
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(lines, self.wrap_max_cols(), self.wrap_max_px());
        let total = w.index.total();
        let v = visual.min(total.saturating_sub(1));
        // 二分：最大 line 使 prefix_rows(line) ≤ v（prefix_rows = 该行
        // 首个视觉行号；行号 = 之前所有行的段数和——O(log n) 一次）
        let (mut lo, mut hi) = (0usize, lines);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if w.index.prefix_rows(mid) <= v {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let line = lo.saturating_sub(1);
        let body = self.line_text(line);
        let lens = body.chars().count();
        let real_xs = self.row_layouts.get(&line).map(|v| v.as_slice());
        let breaks = w.segments_of(line, &body, real_xs);
        let off = (v.saturating_sub(w.index.prefix_rows(line))) as usize;
        let seg = off.min(breaks.len().saturating_sub(1));
        let seg_start = breaks[seg];
        let seg_end = breaks.get(seg + 1).copied().unwrap_or(lens);
        (line, seg, seg_start, seg_end)
    }

    /// 逻辑行 `line` 的首视觉行号（书签/行号/选区锚定用；O(log n)）。
    pub fn line_visual_base(&self, line: usize) -> u32 {
        if !self.wrap.borrow().enabled {
            return line as u32;
        }
        let mut w = self.wrap.borrow_mut();
        w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
        w.index.prefix_rows(line.min(self.doc.line_count()))
    }

    /// 滚动条/行程口径的「内容行数」（开态 = 视觉行数；关态 = 逻辑行数）。
    pub(crate) fn scroll_content_lines(&self) -> usize {
        if self.wrap.borrow().enabled {
            self.visual_rows_total() as usize
        } else {
            self.doc.line_count()
        }
    }

    /// 水平行程内容像素宽：软换行开态恒 0（hscroll 隐藏、needed=false）。
    pub(crate) fn hscroll_content_px(&self) -> f32 {
        if self.wrap.borrow().enabled {
            0.0
        } else {
            self.content_width_px()
        }
    }

    /// 清除竖向移动目标列（P37 打断口：任何非竖向操作调用）。
    pub(crate) fn clear_vertical_goal(&mut self) {
        self.goal_px = None;
    }

    /// 竖向移动的目标解析：目标视觉行内按 goal 像素列反解字符列。
    /// 返回 (line, col, visual)。视觉行越界（首行上/末行下）返回 None。
    fn vertical_target(&mut self, motion: Motion, page_rows: usize) -> Option<(usize, usize)> {
        let total = self.visual_rows_total();
        let cur_v = self.visual_row_of(self.cursor.line, self.cursor.col);
        let step: i64 = match motion {
            Motion::Up => -1,
            Motion::Down => 1,
            Motion::PageUp => -(page_rows.max(1) as i64),
            Motion::PageDown => page_rows.max(1) as i64,
            _ => return None,
        };
        let target_v = (cur_v as i64 + step).clamp(0, total as i64 - 1) as u32;
        if target_v == cur_v {
            return None;
        }
        // goal-column：竖向序列首步记录当前光标 x，序列内沿用
        if self.goal_px.is_none() {
            let text = self.line_text(self.cursor.line);
            let col = self.cursor.col.min(text.chars().count());
            self.goal_px = Some(self.px_of(self.cursor.line, &text, col));
        }
        let goal = self.goal_px.unwrap_or(0.0);
        let (line, _seg, s0, s1) = self.locate_visual(target_v);
        let text = self.line_text(line);
        let lens = text.chars().count();
        let (s0, s1) = (s0.min(lens), s1.min(lens));
        // 段相对 goal：续行从文本区左缘起排——goaI 换算为「段起点之后
        // 的像素偏移」再按字符中点反解（与 hit_test 同口径）
        let seg_base_px = self.px_of(line, &text, s0);
        let goal_seg = goal - seg_base_px;
        let xs_fresh: Option<&Vec<f32>> = self
            .row_layouts
            .get(&line)
            .filter(|xs| xs.len().saturating_sub(1) >= lens);
        let mut col = if goal_seg <= 0.0 {
            s0 // goal 在段起点左侧 → 段首
        } else if let Some(xs) = xs_fresh {
            let mut c = s1; // 未命中（goal 越过段尾）→ 段尾
            for k in s0..s1 {
                let mid = (xs[k] + xs[k + 1]) * 0.5 - seg_base_px;
                if mid > goal_seg {
                    c = k;
                    break;
                }
            }
            c
        } else {
            // 列模型回退（无布局注入/布局滞后；绝对列累计——Tab 宽度
            // 依赖绝对列——再换算段相对）
            let mut acc = 0.0f32; // 绝对列
            let mut base = 0.0f32; // s0 之前的列数
            let mut c = s1;
            for (k, ch) in text.chars().enumerate() {
                if k >= s1 {
                    break;
                }
                let w = char_cols(ch, acc as usize);
                if k < s0 {
                    base += w;
                } else {
                    let mid_rel = (acc + w * 0.5) * self.char_width()
                        - base * self.char_width();
                    if mid_rel > goal_seg {
                        c = k;
                        break;
                    }
                }
                acc += w;
            }
            c
        };
        // 越段钳制：goaI 超出段尾 → 停在段末字符格（不跳到下一视觉段，
        // 光标视觉行保持目标行；末段（s1 == lens）允许到行尾）
        if col >= s1 && s1 < lens {
            col = s1.saturating_sub(1);
        }
        Some((line, col))
    }

    // ---------- 命中测试 ----------

    /// 命中测试：控件内坐标 -> 光标位置（CJK 双宽感知）。
    /// P13：x 需先减去水平滚动偏移——点击坐标在视口系，字符列在文档系。
    pub fn hit_test(&self, x: f32, y: f32) -> CursorPos {
        let gutter = self.gutter_width();
        let char_w = self.char_width();
        // 第 73 轮 ⑯：折行开态——y 先反解视觉行，再映射 (逻辑行, 段)，
        // x 命中落在该段字符区间内（段起点与绘制同源 px_of）
        if self.wrap.borrow().enabled {
            let total = self.visual_rows_total();
            if total == 0 {
                return CursorPos::default();
            }
            let v_f = self.scroll_top + (y.max(0.0) / self.line_height());
            let v = ((v_f.floor() as i64).clamp(0, total as i64 - 1)) as u32;
            let (line, _seg, s0, s1) = self.locate_visual(v);
            let text = self.line_text(line);
            let lens = text.chars().count();
            let (s0, s1) = (s0.min(lens), s1.min(lens));
            // 段相对 x：续行从文本区**左缘**起排（P94 左缘模型）——点击
            // 偏移（x−gutter）即段内偏移，**不得**再减 px_of(seg_start)：
            // 那会把注入布局的真实 x（如 xs[34]=952px）当段起点，所有
            // 续行点击全部落回段首（P96 像素测试当场暴露的潜伏 bug，
            // P94 测试全走列模型回退路径所以未被抓到）
            let rel = (x - gutter + self.scroll_left).max(0.0);
            let col = self.hit_col_in_range(line, &text, s0, s1, rel);
            return CursorPos { line, col };
        }
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

    /// 折行开态：在段字符区间 `[s0, s1)` 内按**段相对** x（已扣 gutter/
    /// 水平偏移与段起点像素）反解字符列。真实布局新鲜走字形中点；
    /// 滞后回退列模型（段内累计从零起步）。
    fn hit_col_in_range(&self, line: usize, text: &str, s0: usize, s1: usize, rel: f32) -> usize {
        let char_w = self.char_width();
        let lens = text.chars().count();
        if s1 <= s0 {
            return s0;
        }
        if let Some(xs) = self.row_layouts.get(&line) {
            if xs.len().saturating_sub(1) >= lens {
                let base = xs[s0];
                for k in s0..s1 {
                    let mid = (xs[k] + xs[k + 1]) * 0.5 - base;
                    if rel < mid {
                        return k;
                    }
                }
                return s1;
            }
        }
        let mut acc = 0.0f32; // 绝对列累计（Tab 宽度依赖绝对列，不能段内重算）
        let mut base = 0.0f32; // s0 之前的列数（段起点）
        for (i, ch) in text.chars().enumerate() {
            if i >= s1 {
                break;
            }
            let w = char_cols(ch, acc as usize);
            if i < s0 {
                base += w;
            } else {
                let mid_rel = (acc + w * 0.5) * char_w - base * char_w;
                if rel < mid_rel {
                    return i;
                }
            }
            acc += w;
        }
        s1
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
    /// 第 60 轮：左侧加 BOOKMARK_STRIP 书签条带（圆点槽位），行号右缘
    /// 相对条带右缘的位置不变——所有消费方都经本函数取宽，无硬编码。
    pub fn gutter_width(&self) -> f32 {
        let digits = self.doc.line_count().to_string().len().max(3);
        BOOKMARK_STRIP + GUTTER_MIN + digits as f32 * self.char_width()
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
    /// 第 73 轮 ⑯：软换行开态 y 走视觉行映射（光标所在段），x 走**段
    /// 相对**定位（续行从文本区左缘起排，主流折行口径）。
    pub fn caret_rect_relative(&self) -> Rectangle {
        let text = self.line_text(self.cursor.line);
        let col = self.cursor.col.min(text.chars().count());
        // P45：滞后感知（同 ensure_visible_horizontal）——布局新鲜走真实
        // 字形位置，滞后回退列模型实时计算；IME 候选框定位同样受益
        let x_px = self.px_of(self.cursor.line, &text, col);
        let (v, seg_start) = if self.wrap.borrow().enabled {
            let mut w = self.wrap.borrow_mut();
            w.ensure_synced(self.doc.line_count(), self.wrap_max_cols(), self.wrap_max_px());
            let lens = text.chars().count();
            let real_xs = self.row_layouts.get(&self.cursor.line).map(|x| x.as_slice());
            let breaks = w.segments_of(self.cursor.line, &text, real_xs);
            let seg = segment_index(&breaks, col, lens);
            let v = w.index.prefix_rows(self.cursor.line) + seg as u32;
            (v, breaks[seg])
        } else {
            (self.cursor.line as u32, 0)
        };
        Rectangle {
            x: self.gutter_width() + (x_px - self.px_of(self.cursor.line, &text, seg_start))
                - self.scroll_left,
            y: (v as f32 - self.scroll_top) * self.line_height(),
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

// ---------- Tab↔空格纯转换助手（第 62 轮） ----------
//
// 与渲染层同源：制表位推进用 `char_cols`（宽字符计 2 显示列、Tab 跳到
// 下一个 TAB_STOP_COLS 制表位），保证「转换后的对齐」与「转换前的绘制
// 对齐」逐列一致。独立成自由函数便于纯函数级单测。

/// Tab → 空格展开。`leading_only=true` 只展开首个非空白字符之前的部分；
/// 其余字符（含宽字符）按真实显示宽度推进制表位列。
fn expand_tabs_in(s: &str, leading_only: bool) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut col: usize = 0;
    let mut in_leading = true;
    for c in s.chars() {
        if leading_only && in_leading && !c.is_whitespace() {
            in_leading = false; // 首个非空白之后不再展开
        }
        if c == '\t' && (in_leading || !leading_only) {
            let adv = char_cols(c, col) as usize; // 1..=TAB_STOP_COLS，至少 1 列
            for _ in 0..adv {
                out.push(' ');
            }
            col += adv;
        } else {
            out.push(c);
            col += char_cols(c, col) as usize;
        }
    }
    out
}

/// 行首空白段中的空格收拢为制表符（detab/entab）：只有当累积空格恰好
/// 到达制表位且 ≥2 个时才换成 Tab；既有 Tab 原样保留（其前悬置的零星
/// 空格不收拢，避免改变既有对齐）；首个非空白之后的空格一律不动。
fn entab_leading_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col: usize = 0;
    let mut run: usize = 0; // 自上次输出以来悬置的空格数
    let mut in_leading = true;
    for c in s.chars() {
        if !in_leading {
            out.push(c);
            continue;
        }
        match c {
            ' ' => {
                run += 1;
                col += 1;
                // 恰到制表位且攒够 2 个才值得换（1 空格 ↔ 1 Tab 不划算，
                // 也保证幂等：输出里不会再出现可收拢段）
                if run >= 2 && col.is_multiple_of(TAB_STOP_COLS) {
                    out.push('\t');
                    run = 0;
                }
            }
            '\t' => {
                for _ in 0..run {
                    out.push(' ');
                }
                run = 0;
                out.push('\t');
                // 与 char_cols 的 Tab 推进同式：跳到下一制表位（整倍数
                // 时推进满一档，不是原地不动）
                col += TAB_STOP_COLS - col % TAB_STOP_COLS;
            }
            ws if ws.is_whitespace() => {
                // 全角空格等其他空白：不参与收拢，原样保留并按显示宽推进
                for _ in 0..run {
                    out.push(' ');
                }
                run = 0;
                out.push(ws);
                col += char_cols(ws, col) as usize;
            }
            other => {
                in_leading = false;
                for _ in 0..run {
                    out.push(' ');
                }
                run = 0;
                out.push(other);
                col += char_cols(other, col) as usize;
            }
        }
    }
    // 整行全是空白：冲刷悬置空格
    for _ in 0..run {
        out.push(' ');
    }
    out
}

/// F5 时间戳（第 63 轮）：`YYYY-MM-DD HH:MM` 24 小时制，记事本同款
/// 场景（日志打点）。
pub(crate) fn local_datetime_stamp() -> String {
    let (y, mo, d, h, mi, _s) = now_local_fields();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

/// 备份文件名用紧凑时间戳（第 64 轮 ⑭）：`YYYYMMDD-HHMMSS`。
pub(crate) fn local_datetime_stamp_compact() -> String {
    let (y, mo, d, h, mi, s) = now_local_fields();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// 本地时间六元组 (年, 月, 日, 时, 分, 秒)。
///
/// 本地时区获取：Windows 上直接 FFI kernel32!GetLocalTime（本应用本就
/// Windows 专属构建，build.rs 已依赖 SDK 工具链；零新依赖——曾试
/// time+local-offset 特性，会引入缓存中没有的 num_threads，离线环境
/// 无法解析，弃）。其他平台回退 UTC 历法换算（Hinnant civil_from_days）。
fn now_local_fields() -> (u16, u16, u16, u16, u16, u16) {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SysTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            millis: u16,
        }
        extern "system" {
            fn GetLocalTime(lp_system_time: *mut SysTime);
        }
        let mut st = SysTime {
            year: 0,
            month: 1,
            day_of_week: 0,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            millis: 0,
        };
        unsafe { GetLocalTime(&mut st) };
        (st.year, st.month, st.day, st.hour, st.minute, st.second)
    }
    #[cfg(not(windows))]
    {
        // UTC 兜底：Unix 秒 → 民用日期（Howard Hinnant civil_from_days 算法）
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (
            y.clamp(0, 65535) as u16,
            m as u16,
            d as u16,
            (rem / 3600) as u16,
            ((rem % 3600) / 60) as u16,
            (rem % 60) as u16,
        )
    }
}


#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
