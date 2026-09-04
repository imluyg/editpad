//! [`EditorCore`]：rope 文档 + 光标/选区/滚动/撤销/IME 的唯一数据源
//! （P68 自 editor.rs 拆出，纯移动零行为变更）。

// Phase 2c（第 84 轮）：EditorCore impl 已按域拆至 undo/motion/edit/block/highlight——
// 下方导入经 pub(crate) 再导出作为分域文件的共享枢纽（各文件 `use super::*` 消费）。
pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::{BTreeSet, HashMap, HashSet};
pub(crate) use std::rc::Rc;

pub(crate) use iced::{Color, Font, Rectangle};

pub(crate) use editpad_core::{
    bracket_kind, scan_forward, Document, LazyHighlighter, StyledRun, MAX_BRACKET_SCAN_CHARS,
};

pub(crate) use super::metrics::{
    char_cols, display_cols, measure_insertion, prefix_width, validate_measured_char_width,
    RECOMPUTE_MAX_COLS_COOLDOWN, TAB_STOP_COLS,
};
pub(crate) use super::scrollbars::VERTICAL_SCROLLBAR_RESERVE;
pub(crate) use super::wrap::{segment_index, WrapCache};
pub(crate) use super::FONT_SIZE_DEFAULT;

pub(crate) const CARET_WIDTH: f32 = 2.0;
/// 撤销组上限（P37 打字成组后，一组 ≈ 一次连续输入；快照是 rope 结构
/// 共享克隆，每组只钉住差异分块，512 组的最坏常驻增量仍在 MB 量级——
/// 内存入账见 HANDOFF §6 本轮）。
pub(crate) const MAX_UNDO: usize = 512;
/// 滚轮一格的行数（Windows 记事本口径）。
pub(crate) const SCROLL_LINES_PER_NOTCH: f32 = 3.0;
/// P115/P116：折行行尾与文本区右缘的余量（px）= **默认字号的一个汉字
/// 宽**（16px 固定）——用户点单「右侧边缘和字体距离一个汉字」；放大
/// 字号后余量不再跟随膨胀（修前 = font_size，48px 字号右缘空 48px，
/// 「段变短右侧空白大」）。
pub(crate) const RIGHT_EDGE_HAN_GAP: f32 = 16.0;
/// P116：垂直滚动条 needed ↔ 折行预算让位的**滞回死区**（行）——判定
/// 需跨过视口 ± 死区才翻转，杜绝惰性收敛期 BIT 行程低估导致的逐帧翻转
/// （滚动条反复出现/消失、段落不停出现消失，用户复报）。
pub(crate) const WRAP_SB_RESERVE_HYSTERESIS_LINES: f32 = 2.0;

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
    // ---------- P121：编辑手感——智能缩进与选区块缩进 ----------
    /// 回车：插入「换行 + 当前行行首空白」（智能缩进）；换行经 insert_str
    /// 统一入口归一为主导行尾。列块态不存活（apply_edit 白名单外先收块）
    Enter,
    /// Tab 键：bool = 是否 Shift 反缩进。有选区 = 触及行整块缩进/反缩进；
    /// 无选区 = 加缩进仍是插入制表符（原语义），反缩进作用于当前行。
    /// 列块态下加缩进仍走块内插制表符（apply_edit 白名单保路）
    TabKey(bool),
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
pub(crate) struct LineBlock {
    /// 光标钳制用的原始触及范围（含被排除的幻影行）
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// 重建时是否在块尾补主导行尾（块不在文档末尾恒真；在文档末尾
    /// 时等于「原文档以换行收尾」）
    pub(crate) push_nl: bool,
    pub(crate) lines: Vec<String>,
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
pub(crate) struct Snapshot {
    pub(crate) doc: Document,
    pub(crate) cursor: CursorPos,
    pub(crate) anchor: Option<CursorPos>,
    /// 第 60 轮：书签集合随快照一并入栈——撤销/重做是「整体换文档」，
    /// 行号全盘漂移，唯一可靠的还原方式就是把书签当可回滚状态存档。
    /// 副作用：书签开关/清除也会占用撤销槽（与打字同待遇，见
    /// `toggle_bookmark` 注释）；`remove_bookmarked_lines` 因此能一次
    /// 撤销同时找回文本和书签。
    pub(crate) bookmarks: BTreeSet<usize>,
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
    pub(crate) max_line_cols: usize,
    pub(crate) undo_stack: Vec<Snapshot>,
    pub(crate) redo_stack: Vec<Snapshot>,
    /// P37 打字成组撤销：Some(组内上次插入的结束字符偏移) = 当前处于
    /// 连续单字符输入组中。任何其他操作（光标移动、选区变更、删除类
    /// 编辑、撤销/重做、换文档、焦点离开）都会置 None 打断；
    /// 合并条件见 [`EditorCore::insert_str`]。
    pub(crate) typing_run: Option<usize>,
    /// 语法高亮器；None = 纯文本快速路径。RefCell 让只读的 draw 也能推进状态。
    pub(crate) highlight: Option<RefCell<LazyHighlighter>>,
    /// 输入法预编辑串（组字过程中的拼音/候选串），提交前显示在光标处。
    pub(crate) preedit: Option<String>,
    /// 正文字号（驱动行高与列宽）；默认 16，合法区间见 core 设置层。
    pub(crate) font_size: f32,
    /// 编辑器是否持有键盘焦点（点击编辑区置真，点击其他控件置假）。
    /// 输入法事件会广播给所有控件，必须靠它过滤——否则在查找框打字会进正文。
    pub focused: bool,
    /// 光标闪烁相位（打磨项）：应用层心跳每 ~530ms 翻转一次；
    /// 任何光标活动会重置为可见并刷新活动时刻，静止超时后按相位隐现。
    pub(crate) blink_on: bool,
    pub(crate) last_activity: Option<std::time::Instant>,
    /// P53：上次闪烁相位翻转的真实时刻——淡出动画期间心跳临时加密到
    /// 33ms 快拍，相位翻转仍按 CARET_BLINK_MS 门控，光标节奏不受影响。
    pub(crate) last_blink_at: Option<std::time::Instant>,
    /// P53：竖直滚动条最近活动时刻（滚动/滚动条交互/悬停命中区）。
    /// 淡出判定与绘制 alpha 都以它为基准；None = 从未活动（恒隐藏）。
    pub(crate) sb_activity: Option<std::time::Instant>,
    /// P38 落盘基线：最近一次已知「磁盘内容」的文档快照（rope 结构
    /// 共享克隆，O(1)）。撤销/重做后据此判断内容是否回到了已保存状态，
    /// 让 dirty 如实反映「与磁盘的差异」而不是「自保存后动过没有」。
    pub(crate) saved_baseline: Option<Document>,
    /// P42 实测列宽（真实字形 advance，像素/列）：控件层用排版段落实测
    /// 后注入。None = 未实测或测值无效，[`Self::char_width`] 回退固定假设。
    pub(crate) measured_char_w: Option<f32>,
    /// P88 字形墨迹在行盒内的上边距（px）：光标/选区等行盒装饰矩形的
    /// 纵向对齐基准——字形在行盒（行高 = 字号 × 1.375）内按字体度量
    /// 下浮 1~6px（实测 CJK 等宽钉字 ≈ 4px），行盒顶对齐会让装饰墨迹
    /// 悬在首行上方空带（用户截图「黑点/色带残影」根因）。控件层按
    /// (字体, 字号) 用同源渲染管线实测注入；未测量保持 0（旧行为）。
    pub(crate) ink_offset: f32,
    /// P89：字形墨迹高度（px，与 [`Self::ink_offset`] 成对注入）：行盒内
    /// 墨迹顶→底跨度（metrics 同源扫描实测）。选区/列块等「面状」装饰
    /// 按墨迹盒垂直居中需要它（P88 只对齐顶边 → 单选字被顶在带顶不
    /// 居中）。默认 0 = 未实测 → [`Self::decoration_inset`] 公式自动得 0
    /// （整行盒装饰 = 旧行为），恒安全。
    pub(crate) ink_height: f32,
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
    pub(crate) row_layouts: HashMap<usize, Vec<f32>>,
    /// P116 字号戳：row_layouts 注入时的字号——缩放下旧字号布局会
    /// 「长度对齐但字宽过期」（旧 xs 在缩放帧被 px_of/断行误用 →
    /// 缩小留白/放大超右缘，用户复报；trusted_xs/px_of 必须按字号
    /// 过滤，失配回退按折算 char_w 的列模型，正确近似到下一帧注入）。
    pub(crate) row_layouts_font_size: f32,
    /// 已注入行宽的最大值（与 row_layouts 同步维护）：水平行程钳制上界的
    /// 真实补充——列模型 `max_line_cols` 对分数/超宽字形可能欠估，
    /// 不补则超宽行滚不到头、光标越界。
    pub(crate) max_row_width_px: f32,
    /// 缩短类编辑/撤销重做后置位：`max_line_cols` 高水位可能过估
    /// （只升不降的既有取舍），由 [`Self::clamp_scroll_horizontal`] 按
    /// 冷却窗惰性全量重算收敛——否则删除超宽行后水平滚动条永不消失。
    pub(crate) max_cols_stale: bool,
    /// 上次全量重算 `max_line_cols` 的时刻（限流用，见上字段）。
    pub(crate) max_cols_checked: Option<std::time::Instant>,
    /// 书签行集合（0 起行号，升序）。第 60 轮书签套件的状态底座：
    /// * 会话级标注——不写入文件、不入会话快照（重启即清，主流编辑器
    ///   同口径），也不参与置脏判定；
    /// * 行结构编辑同步平移/删除（各编辑路径内的 remap 注释）；
    /// * 撤销/重做经快照整体回滚（见 `Snapshot::bookmarks`）。
    pub(crate) bookmarks: BTreeSet<usize>,
    /// 括号匹配查询缓存（第 61 轮）：键 = 光标位置，值 = 该位置的匹配
    /// 结果（含 None）。draw 每帧查询、命中即零扫描——孤立括号的封顶
    /// 扫描（MAX_BRACKET_SCAN_CHARS）只在新光标位付一次。RefCell 让
    /// 只读的 draw 也能维护缓存（与高亮器同手法）；内容变更经
    /// `invalidate_highlight_from` 统一失效（全部正文突变路径的唯一汇点）。
    pub(crate) bracket_cache: BracketCache,
    /// 选区显示跨度缓存（第 63 轮状态栏统计）：键 = 选区字节偏移对，
    /// 值 = 显示字符数（None = 无选区）。跨行精确计数是 O(选区行数)，
    /// 状态栏每帧查询必须有缓存；失效走同一汇点（P81 口径）。
    pub(crate) sel_span_cache: SelSpanCache,
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
    pub(crate) wrap: RefCell<WrapCache>,
    /// 竖向移动的目标像素列（第 73 轮 ⑯，主流 goal-column 口径）：进入
    /// Up/Down/PageUp/PageDown 序列时记录当前光标 x，序列内沿用；任一
    /// 非竖向操作（左右移/编辑/点击/跳转/撤销重做）清除（P37 打断口）。
    pub(crate) goal_px: Option<f32>,
    /// P99：软换行折行预算是否按垂直滚动条可视带宽让位（滚动条需要
    /// 出现时为 true，由控件层 draw 每帧按 `VScrollbar::measure` 的
    /// needed 更新）。开态内容超出视口 → 折行文本在滑块左侧收尾，
    /// 行尾字符不被盖住/显得截断；内容放得下 → 零预留全宽贴边
    /// （P95 口径保留）。判定稳定性见 [`Self::set_wrap_sb_reserve`]。
    pub(crate) wrap_sb_reserve: bool,
}

/// 光标闪烁半周期。
pub const CARET_BLINK_MS: u64 = 530;
/// 光标活动后的常显窗口（移动/输入时不受闪烁相位影响）。
pub(crate) const CARET_ACTIVE_MS: u128 = 450;

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
        let remaining = (SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS).saturating_sub(idle_ms);
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
            ink_offset: 0.0,
            // P89：默认 0 = 未实测（decoration_inset 自动退化为整行盒）
            ink_height: 0.0,
            metric_key: None,
            row_layouts: HashMap::new(),
            row_layouts_font_size: 0.0,
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
        self.measured_char_w.unwrap_or(self.font_size * 0.5625)
    }

    // ---------- 行级真实布局（第 40 轮根治） ----------

    /// 注入一行字符起点的真实像素 x（`xs[i]` = 第 i 个字符起点，末项 = 行尾）。
    /// 同时抬升真实行宽高水位（水平行程钳制用）。
    /// P116：记录注入时的字号（row_layouts_font_size）——缩放帧旧 xs
    /// 必须因字号失配失效。
    pub fn set_row_layout(&mut self, line: usize, xs: Vec<f32>) {
        if let Some(&w) = xs.last() {
            self.max_row_width_px = self.max_row_width_px.max(w);
        }
        self.row_layouts_font_size = self.font_size;
        self.row_layouts.insert(line, xs);
    }

    /// 清空全部行级布局与行宽高水位（控件层每帧重建可见行时调用，
    /// 保证无陈旧残留——编译/滚动后旧行布局不会冒充新内容）。
    pub fn clear_row_layouts(&mut self) {
        self.row_layouts.clear();
        self.row_layouts_font_size = 0.0;
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
        self.row_layouts
            .get(&line)
            .and_then(|xs| xs.last().copied())
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
    /// 规则：行布局新鲜（长度覆盖到目标列 **且字号与当前一致**）用真实
    /// 字形位置；滞后回退列模型**实时文本**计算（`text` 必须为当前文档
    /// 该行文本）。draw 期布局恒新鲜，此函数两期通用。
    /// P116：字号失配（缩放帧旧布局）一律回退列模型——旧 xs 长度对齐
    /// 但字宽过期，直接用即光标/选区/断行按旧字号错位。
    pub(crate) fn px_of(&self, line: usize, text: &str, col: usize) -> f32 {
        let col = col.min(text.chars().count());
        let size_ok = (self.row_layouts_font_size - self.font_size).abs() < 0.01;
        match self.row_layouts.get(&line) {
            Some(xs) if size_ok && xs.len().saturating_sub(1) >= col => xs[col],
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

    /// P88/P89：注入字形墨迹盒——上边距 + 墨迹高（px，见字段注释）。
    /// 两值须**同时**通过校验才落库（任一非法返回 false 且保持现状），
    /// 避免只更新一半产生非对称几何。上边距上界 = 字号 × 0.75；高度
    /// 上界 = 行盒高（墨迹不可能超出行盒，实测扫描窗口即行盒）。
    pub fn set_ink_box(&mut self, offset: f32, height: f32) -> bool {
        let max_top = (self.font_size * 0.75).max(1.0);
        let max_h = self.line_height() + 0.01;
        if offset.is_finite()
            && height.is_finite()
            && (0.0..=max_top).contains(&offset)
            && (0.0..=max_h).contains(&height)
        {
            self.ink_offset = offset;
            self.ink_height = height;
            true
        } else {
            false
        }
    }

    /// P89：面状行盒装饰（选区/列块高亮）相对行盒顶的纵向偏移（px）：
    /// 带 = 行盒整高 + 按字形墨迹盒垂直居中——带顶 = 行盒顶 + 返回值、
    /// 带高 = 行盒高，带内墨迹上下留白相等（(行盒高 − 墨迹高) / 2），
    /// 单选一字时字居中不再顶边；多行选区相邻带首尾相接（带距 = 行高），
    /// 高亮连续不断裂。推导：偏移 = ink_offset − (行盒高 − ink_height) / 2，
    /// 钳制 ≥ 0（墨迹贴行盒顶的字体退化为行盒顶对齐，不重返 P88 悬墨；
    /// 未实测时 ink_height = 0 且 ink_offset = 0，公式自动得 0 = 旧行为）。
    pub fn decoration_inset(&self) -> f32 {
        ((2.0 * self.ink_offset + self.ink_height - self.line_height()) * 0.5).max(0.0)
    }

    /// 设置字号：clamp 到合法区间后让滚动/可见性按新度量重新收敛
    /// （字号变大时可见行变少，光标必须仍落在视口内）。
    /// P42：实测列宽按字号比例折算（同一字体的 advance 与字号线性），
    /// 折算后仍落合法区间；控件层下一帧按新度量键重测校准。
    /// P116 勘误：**先失效旧字号度量再收敛**——行布局（旧 xs 字宽过期，
    /// px_of/trusted_xs 按字号戳自动回退折算列模型）与折行断点 memo
    /// （键 = 代次+断行路径、不感知字号；ensure_visible 等即时查询会
    /// 用旧 xs 固化错断点 → 缩放后缩小留白/放大超右缘不换行，用户复报）。
    pub fn set_font_size(&mut self, size: f32) {
        let old = self.font_size;
        self.font_size = normalize_font_size(size);
        if old > 0.0 && self.font_size != old {
            self.row_layouts.clear();
            self.row_layouts_font_size = 0.0;
            self.wrap.borrow_mut().after_edit(self.doc.line_count());
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

    // ---------- 高亮后台分批补建（P12 协作面） ----------

    // ---------- 落盘基线（P38：dirty 如实反映与磁盘的差异） ----------

    // ---------- 行操作套件（第 57 轮，仿主流编辑器编辑菜单） ----------

    // ---------- 大小写转换与行首尾清理（第 58 轮，仿主流编辑器编辑菜单） ----------

    // ---------- 行排序与去重（第 59 轮，仿主流编辑器行操作菜单） ----------

    // ---------- 行操作扩充（第 62 轮）：Tab↔空格 / 合并拆分 / 删空行 ----------

    // ---------- 第 63 轮：选区统计 + 插入日期时间 ----------

    // ---------- 第 67 轮 ⑮：列块（矩形）选区与编辑 ----------

    // ---------- 书签套件（第 60 轮，仿主流编辑器书签导航） ----------
    //
    // 状态底座 = [`EditorCore::bookmarks`]（升序 BTreeSet，行号 0 起）。
    // 本节后半是各编辑路径共用的书签再映射助手——约定两条：
    // 1. 只在「确认要改动」之后调用（no-op 提前返回的路径不碰集合）；
    // 2. 映射一律从旧集合函数式重建新集合，杜绝逐条 remove/insert 的
    //    顺序陷阱。

    // ---------- 书签再映射助手（各编辑路径调用） ----------

    // ---------- 括号匹配（第 61 轮） ----------

    // ---------- P13 水平行程的宽度追踪 ----------

    // ---------- 软换行（第 73 轮 ⑯）视觉行映射 ----------
    //
    // 模型（设计文档 §3）：折行完全建立在显示列上，
    // rope 仍是唯一事实源，本组函数是纯派生缓存。开关关闭时全部函数
    // 恒等退化（视觉行 = 逻辑行），生产路径零行为变更。
    // 列预算：正文区可视宽 − 滚动条覆盖区，与渲染同口径（char_cols）。

    // ---------- 命中测试 ----------

    // ---------- 光标闪烁（打磨项） ----------

    // ---------- P53 竖直滚动条淡入淡出 ----------

    // ---------- 输入法事件的焦点裁决（P2） ----------
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
pub(crate) fn expand_tabs_in(s: &str, leading_only: bool) -> String {
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
pub(crate) fn entab_leading_ws(s: &str) -> String {
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
pub(crate) fn now_local_fields() -> (u16, u16, u16, u16, u16, u16) {
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

// Phase 3b（第 85 轮）：core_tests 按 2c 域拆出的测试文件（随 undo/motion/edit/
// block/highlight 拆分同步，纯搬移；共享助手留在 tests 模块并 pub(super) 提升）
#[cfg(test)]
#[path = "block_tests.rs"]
mod block_tests;
#[cfg(test)]
#[path = "edit_tests.rs"]
mod edit_tests;
#[cfg(test)]
#[path = "highlight_tests.rs"]
mod highlight_tests;
#[cfg(test)]
#[path = "motion_tests.rs"]
mod motion_tests;
#[cfg(test)]
#[path = "undo_tests.rs"]
mod undo_tests;
