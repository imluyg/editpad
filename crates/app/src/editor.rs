//! 自绘虚拟化编辑器 —— Editpad M2 的技术核心。
//!
//! 设计要点：
//! * [`EditorCore`] 持有 ropey [`Document`]，是唯一数据源；光标/选区/滚动/撤销全在这层。
//! * 渲染只处理**可见行**（视口虚拟化）：50MB 与 5KB 的每帧排版成本相同。
//! * 状态放在 `Rc<RefCell<_>>`（[`EditorHandle`]）里跨帧共享——iced 每帧重建控件实例，
//!   官方 text_editor 的 Content 也是同样的手法。
//! * 实现真正的 `Widget` trait：只有控件层的 `Shell` 能开启输入法
//!   （`request_input_method`）并接收中文上屏事件（`InputMethod::Commit`）。
//!
//! 已知取舍（v1）：等宽字体假设（CJK 列映射近似）；预编辑串由系统浮窗显示；
//! 语法高亮留待 M2b 以逐行状态缓存接入。

use std::cell::RefCell;
use std::rc::Rc;

use iced::advanced::{
    input_method,
    layout::{self, Layout},
    renderer::{self, Renderer as _},
    text::{self as core_text, Renderer as _},
    widget::Tree,
    Clipboard, Shell, Widget,
};
use iced::{alignment, border::Radius, mouse, window, Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme};

use editpad_core::{Document, LazyHighlighter, StyledRun};

// ---------- 视觉常量 ----------
//
// 行高/列宽不再写死：由 `font_size` 驱动
// （line_height = 字号×1.375，char_width = 字号×0.5625），随设置实时变化。

const GUTTER_MIN: f32 = 12.0;
/// 默认字号（与 core 设置层的规范默认一致）。
const FONT_SIZE_DEFAULT: f32 = 16.0;
/// 行号栏字号相对正文的比例（16px 正文时即原来的 13px）。
const GUTTER_FONT_SCALE: f32 = 13.0 / 16.0;
const CARET_WIDTH: f32 = 2.0;
const MAX_UNDO: usize = 128;
const SCROLL_LINES_PER_NOTCH: f32 = 3.0;

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
fn luminance(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

fn lighten(c: Color, amount: f32) -> Color {
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
    viewport_h: f32,
    /// 视口宽度（像素，整控件含行号栏；RedrawRequested 时同步）。
    /// 正文区可视宽 = 本值 − 行号栏宽。
    viewport_w: f32,
    dragging: bool,
    /// 垂直滚动条拖拽中：Some(按下点相对滑块顶部的像素偏移)
    scrollbar_grab: Option<f32>,
    /// 水平滚动条拖拽中：Some(按下点相对滑块左缘的像素偏移)（P13）
    hscrollbar_grab: Option<f32>,
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
    /// 语法高亮器；None = 纯文本快速路径。RefCell 让只读的 draw 也能推进状态。
    highlight: Option<RefCell<LazyHighlighter>>,
    /// 输入法预编辑串（组字过程中的拼音/候选串），提交前显示在光标处。
    preedit: Option<String>,
    /// 正文字号（驱动行高与列宽）；默认 16，合法区间见 core 设置层。
    font_size: f32,
    /// 编辑器是否持有键盘焦点（点击编辑区置真，点击其他控件置假）。
    /// 输入法事件会广播给所有控件，必须靠它过滤——否则在查找框打字会进正文。
    pub focused: bool,
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
            highlight: None,
            preedit: None,
            font_size: FONT_SIZE_DEFAULT,
            focused: true,
        }
    }
}

// ---------- CJK 双宽字符的列宽换算 ----------
//
// 等宽假设下：普通字符占 1 列，CJK/全角占 2 列，Tab 展开到下一个
// 制表位（P14，仅显示层——文档里存的仍是真实 `\t` 字符）。
// 覆盖常用区间（CJK 统一表意、扩展A、兼容、假名、谚文、全角符号）。

/// 制表位间距（显示列）。
const TAB_STOP_COLS: usize = 4;

fn is_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x2FFFD
            | 0x30000..=0x3FFFD
    )
}

/// 字符 `c` 位于显示列 `col` 时占据的宽度（列数）。
/// Tab 推进到下一个 [`TAB_STOP_COLS`] 制表位，至少占 1 列。
fn char_cols(c: char, col: usize) -> f32 {
    match c {
        '\t' => (TAB_STOP_COLS - (col % TAB_STOP_COLS)).max(1) as f32,
        _ => {
            if is_wide(c) {
                2.0
            } else {
                1.0
            }
        }
    }
}

/// 文本的显示列数（1 列 = [`EditorCore::char_width`] 像素）。
fn display_cols(text: &str) -> f32 {
    let mut col = 0usize;
    for c in text.chars() {
        col += char_cols(c, col) as usize;
    }
    col as f32
}

/// 第 `col` 个字符之前的字符所占显示宽度（像素）；`col` 为字符索引。
fn prefix_width(text: &str, col: usize) -> f32 {
    let mut width = 0f32;
    for (i, c) in text.chars().enumerate() {
        if i >= col {
            break;
        }
        width += char_cols(c, width as usize);
    }
    width
}

/// 统计待插入文本的「换行单元数」与末行列数（P9）：
/// `\r\n` 与孤立 `\r` 也各算一次换行——旧实现 `split('\n')` 只认 `\n`，
/// CRLF 文本入文后光标列会漂移一个字符。
fn measure_insertion(text: &str) -> (usize, usize) {
    let mut lines = 0usize;
    let mut tail_cols = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines += 1;
                tail_cols = 0;
            }
            '\n' => {
                lines += 1;
                tail_cols = 0;
            }
            _ => tail_cols += 1,
        }
    }
    (lines, tail_cols)
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

    /// 单列字符宽（像素，等宽假设）= 字号 × 0.5625。
    pub fn char_width(&self) -> f32 {
        self.font_size * 0.5625
    }

    /// 设置字号：clamp 到合法区间后让滚动/可见性按新度量重新收敛
    /// （字号变大时可见行变少，光标必须仍落在视口内）。
    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = normalize_font_size(size);
        self.ensure_visible();
    }

    /// 用新文档整体替换（加载文件时用），清空历史。
    pub fn reset_document(&mut self, doc: Document) {
        self.doc = doc;
        self.cursor = CursorPos::default();
        self.anchor = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        self.undo_stack.clear();
        self.redo_stack.clear();
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
    fn highlight_runs(&self, line_idx: usize, target_text: &str) -> Vec<StyledRun> {
        let Some(hl) = &self.highlight else {
            return Vec::new();
        };
        let doc = &self.doc;
        hl.borrow_mut()
            .styled_line_limited(
                line_idx,
                target_text,
                doc.line_count(),
                LazyHighlighter::MAX_INLINE_STRIDES,
                &mut |i| doc.line_str(i).trim_end_matches(['\n', '\r']).to_owned(),
            )
            .unwrap_or_default()
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

    /// 当前高亮器代次；未启用高亮时为 None。
    pub fn highlight_generation(&self) -> Option<u64> {
        self.highlight.as_ref().map(|h| h.borrow().generation())
    }

    /// 使用的语法名（测试诊断用）。
    #[cfg(test)]
    pub fn highlight_syntax_name(&self) -> Option<String> {
        self.highlight
            .as_ref()
            .map(|h| h.borrow().syntax_name().to_owned())
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

    fn snapshot(&mut self) {
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
        self.ensure_visible();
        true
    }

    pub fn redo(&mut self) -> bool {
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
        self.ensure_visible();
        true
    }

    pub fn select_all(&mut self) {
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
    pub fn insert_str(&mut self, text: &str) {
        let text = self.doc.line_ending().normalize(text);
        self.snapshot();

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
        self.ensure_visible();
    }

    /// 用给定文本替换当前选区；无选区时退化为插入。
    pub fn replace_selection(&mut self, text: &str) {
        self.insert_str(text);
    }

    /// 把选区起点放到 `(line, col)` 并向右延伸 `len_chars` 个字符形成新选区。
    pub fn select_span(&mut self, line: usize, col: usize, len_chars: usize) {
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
        let target =
            (line_1based.saturating_sub(1)).min(self.doc.line_count().saturating_sub(1));
        self.anchor = None;
        self.cursor = CursorPos {
            line: target,
            col: 0,
        };
        self.ensure_visible();
    }

    pub fn scroll_by_lines(&mut self, lines: f32) {
        self.scroll_top = (self.scroll_top - lines).max(0.0);
        self.clamp_scroll();
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
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_top = self.scroll_top.max(0.0);
        let max = (self.doc.line_count() as f32 - self.viewport_h / self.line_height()).max(0.0);
        self.scroll_top = self.scroll_top.min(max + 1.0);
    }

    /// 正文区可视宽度（像素）= 视口宽 − 行号栏宽。
    fn text_viewport_w(&self) -> f32 {
        (self.viewport_w - self.gutter_width()).max(0.0)
    }

    /// 水平行程钳制：左缘不出负，右缘不超出最宽行
    /// （高水位偏大时允许滚进一小段空白，见字段注释）。
    pub fn clamp_scroll_horizontal(&mut self) {
        let content_w = self.max_line_cols as f32 * self.char_width();
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
        let cx = prefix_width(&text, col) * self.char_width();
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
        let first = self.scroll_top;
        let last = self.scroll_top + self.viewport_h / self.line_height() - 1.0;
        let line = self.cursor.line as f32;
        if line < first {
            self.scroll_top = line;
        } else if line > last {
            self.scroll_top = line - self.viewport_h / self.line_height() + 1.0;
        }
        self.clamp_scroll();
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
    fn line_text(&self, line: usize) -> String {
        if line >= self.doc.line_count() {
            return String::new();
        }
        self.doc
            .line_str(line)
            .trim_end_matches(['\n', '\r'])
            .to_owned()
    }

    /// 行号栏宽度。
    pub fn gutter_width(&self) -> f32 {
        let digits = self.doc.line_count().to_string().len().max(3);
        GUTTER_MIN + digits as f32 * self.char_width() * 0.7
    }

    /// 相对控件的光标矩形（供输入法定位候选框，双宽感知）。
    /// P13：x 含水平滚动偏移的抵扣——返回值是视口系坐标。
    pub fn caret_rect_relative(&self) -> Rectangle {
        let text = self.line_text(self.cursor.line);
        Rectangle {
            x: self.gutter_width()
                + prefix_width(&text, self.cursor.col.min(text.chars().count()))
                    * self.char_width()
                - self.scroll_left,
            y: (self.cursor.line as f32 - self.scroll_top) * self.line_height(),
            width: CARET_WIDTH,
            height: self.line_height(),
        }
    }

    pub fn set_viewport_height(&mut self, h: f32) {
        self.viewport_h = h.max(self.line_height());
        self.clamp_scroll();
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

// ---------- 垂直滚动条 ----------
//
// 覆盖式（overlay）设计：不改变文本排版与命中测试的坐标体系，
// 只在控件右缘绘制并在鼠标事件里优先拦截。内容不超出视口时整个
// 滚动条不存在（用户要求：没超就不需要）。

/// 滑块/轨道厚度。
const SCROLLBAR_WIDTH: f32 = 10.0;
/// 滚动条距控件右缘的间隙。
const SCROLLBAR_EDGE_INSET: f32 = 3.0;
/// 轨道距控件上下缘的内缩。
const SCROLLBAR_TRACK_PAD: f32 = 2.0;
/// 超长文档下滑块的最小高度（否则 50MB 文档的滑块只剩几个像素抓不住）。
const THUMB_MIN_H: f32 = 32.0;
/// 命中区总宽（比可视宽度略宽，好点中）。
const SCROLLBAR_ZONE_W: f32 = SCROLLBAR_WIDTH + SCROLLBAR_EDGE_INSET * 2.0;

/// 垂直滚动条几何。全部为**相对控件**的像素坐标；由
/// [`VScrollbar::measure`] 从当前状态推导——窗口缩放、字号调整、
/// 文档变化都会在下一帧自然反映，无需额外同步。
#[derive(Debug, Clone, Copy, PartialEq)]
struct VScrollbar {
    /// 内容超出视口才为 true
    needed: bool,
    track_y: f32,
    track_h: f32,
    thumb_y: f32,
    thumb_h: f32,
    /// 可滚动行程（行）。口径与 `clamp_scroll` 的 max+1 对齐：
    /// 允许滚到末行后留一行空白，与滚轮行为一致。
    range_lines: f32,
}

impl VScrollbar {
    fn measure(
        content_lines: usize,
        viewport_h: f32,
        line_h: f32,
        widget_h: f32,
        scroll_top: f32,
    ) -> Self {
        let content_h = content_lines.max(1) as f32 * line_h;
        let needed = line_h > 0.0 && viewport_h > 0.0 && content_h > viewport_h;
        let track_y = SCROLLBAR_TRACK_PAD;
        let track_h = (widget_h - SCROLLBAR_TRACK_PAD * 2.0).max(0.0);
        if !needed || track_h <= 0.0 {
            return Self {
                needed: false,
                track_y,
                track_h,
                thumb_y: track_y,
                thumb_h: 0.0,
                range_lines: 0.0,
            };
        }
        let rows_in_view = viewport_h / line_h;
        let range_lines = (content_lines as f32 - rows_in_view + 1.0).max(1.0);
        let thumb_h = (track_h * viewport_h / content_h).clamp(THUMB_MIN_H, track_h);
        let travel = (track_h - thumb_h).max(0.0);
        let ratio = (scroll_top / range_lines).clamp(0.0, 1.0);
        Self {
            needed: true,
            track_y,
            track_h,
            thumb_y: track_y + ratio * travel,
            thumb_h,
            range_lines,
        }
    }

    /// 滑块顶部目标 y → scroll_top（已夹紧到行程内）。
    fn scroll_for_thumb_y(&self, thumb_top_y: f32) -> f32 {
        let travel = (self.track_h - self.thumb_h).max(1e-3);
        let ratio = ((thumb_top_y - self.track_y) / travel).clamp(0.0, 1.0);
        ratio * self.range_lines
    }

    /// 点击轨道：把滑块中心对准点击处（连续按住可继续拖拽）。
    fn scroll_for_track_click(&self, click_y: f32) -> f32 {
        self.scroll_for_thumb_y(click_y - self.thumb_h * 0.5)
    }

    /// 控件局部坐标是否落在滚动条交互区。
    fn hits(&self, local_x: f32, local_y: f32, widget_w: f32) -> bool {
        self.needed
            && local_x >= widget_w - SCROLLBAR_ZONE_W
            && local_y >= self.track_y
            && local_y <= self.track_y + self.track_h
    }

    /// 滑块矩形（x/y 相对控件左上角），供绘制与拖拽命中。
    fn thumb_rect(&self, widget_w: f32) -> Rectangle {
        Rectangle {
            x: widget_w - SCROLLBAR_EDGE_INSET - SCROLLBAR_WIDTH,
            y: self.thumb_y,
            width: SCROLLBAR_WIDTH,
            height: self.thumb_h,
        }
    }
}

/// 超宽内容下水平滑块的最小宽度（与垂直侧同理由：必须抓得住）。
const THUMB_MIN_W: f32 = 32.0;
/// 滑块/轨道的可视厚度（垂直条的厚度常量复用于水平条的高度）。
const SCROLLBAR_THUMB_THICKNESS: f32 = 10.0;
/// 水平条命中区高度（下缘窄带，比可视厚度略高好点中）。
const SCROLLBAR_ZONE_H: f32 = SCROLLBAR_THUMB_THICKNESS + SCROLLBAR_EDGE_INSET * 2.0;

/// 水平滚动条几何（P13）。坐标体系与 [`VScrollbar`] 对称：
/// 全部为**相对控件**的像素坐标，贴控件下缘；轨道横贯整个控件宽度，
/// 行程比例按「正文区可视宽」计算（行号栏不参与横向滚动）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct HScrollbar {
    /// 内容超出视口宽才为 true
    needed: bool,
    track_x: f32,
    track_w: f32,
    thumb_x: f32,
    thumb_w: f32,
    /// 可横向滚动的行程（像素）。
    range_px: f32,
}

impl HScrollbar {
    /// * `content_px`：最宽行的像素宽度（高水位 × 列宽）；
    /// * `view_px`：正文区可视宽度（视口宽 − 行号栏）。
    fn measure(content_px: f32, view_px: f32, widget_w: f32, scroll_left: f32) -> Self {
        let needed = view_px > 0.0 && content_px > view_px;
        let track_x = SCROLLBAR_TRACK_PAD;
        let track_w = (widget_w - SCROLLBAR_TRACK_PAD * 2.0).max(0.0);
        if !needed || track_w <= 0.0 {
            return Self {
                needed: false,
                track_x,
                track_w,
                thumb_x: track_x,
                thumb_w: 0.0,
                range_px: 0.0,
            };
        }
        let range_px = (content_px - view_px).max(1.0);
        let thumb_w = (track_w * view_px / content_px).clamp(THUMB_MIN_W, track_w);
        let travel = (track_w - thumb_w).max(0.0);
        let ratio = (scroll_left / range_px).clamp(0.0, 1.0);
        Self {
            needed: true,
            track_x,
            track_w,
            thumb_x: track_x + ratio * travel,
            thumb_w,
            range_px,
        }
    }

    /// 滑块左缘目标 x → scroll_left（已夹紧到行程内）。
    fn scroll_for_thumb_x(&self, thumb_left_x: f32) -> f32 {
        let travel = (self.track_w - self.thumb_w).max(1e-3);
        let ratio = ((thumb_left_x - self.track_x) / travel).clamp(0.0, 1.0);
        ratio * self.range_px
    }

    /// 点击轨道：把滑块中心对准点击处（连续按住可继续拖拽）。
    fn scroll_for_track_click(&self, click_x: f32) -> f32 {
        self.scroll_for_thumb_x(click_x - self.thumb_w * 0.5)
    }

    /// 控件局部坐标是否落在滚动条交互区（下缘窄带）。
    fn hits(&self, local_x: f32, local_y: f32, widget_h: f32) -> bool {
        self.needed
            && local_y >= widget_h - SCROLLBAR_ZONE_H
            && local_x >= self.track_x
            && local_x <= self.track_x + self.track_w
    }

    /// 滑块矩形（相对控件），供绘制与拖拽命中。
    fn thumb_rect(&self, widget_h: f32) -> Rectangle {
        Rectangle {
            x: self.thumb_x,
            y: widget_h - SCROLLBAR_EDGE_INSET - SCROLLBAR_THUMB_THICKNESS,
            width: self.thumb_w,
            height: SCROLLBAR_THUMB_THICKNESS,
        }
    }
}

// ---------- 共享句柄 ----------

/// 跨帧共享的编辑器状态句柄（Clone 廉价）。
#[derive(Clone, Default)]
pub struct EditorHandle(Rc<RefCell<EditorCore>>);

impl EditorHandle {
    pub fn borrow(&self) -> std::cell::Ref<'_, EditorCore> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, EditorCore> {
        self.0.borrow_mut()
    }

    /// 构造可加入视图树的自绘控件。
    pub fn view(&self) -> Element<'_, super::Message> {
        Element::new(EditorView { core: self.clone() })
    }
}

// ---------- 控件实现 ----------

struct EditorView {
    core: EditorHandle,
}

/// 浅色主题的固定配色（保持 v1 观感）；深色主题在 draw 时由 palette 派生。
const SELECTION_COLOR: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.25);
const CARET_COLOR: Color = Color::from_rgb8(0x11, 0x11, 0x11);
const GUTTER_BG: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
const GUTTER_TEXT: Color = Color::from_rgb8(0x99, 0x99, 0x99);
const PREEDIT_TEXT: Color = Color::from_rgb8(0x33, 0x66, 0xCC);
const PREEDIT_UNDERLINE: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.6);

/// 一次 draw 用到的全部颜色（按当前主题解析）。
struct EditorColors {
    selection: Color,
    caret: Color,
    gutter_bg: Color,
    gutter_text: Color,
    preedit_text: Color,
    preedit_underline: Color,
    scrollbar_track: Color,
    scrollbar_thumb: Color,
}

impl EditorColors {
    /// 浅色：沿用固定值；深色：从 palette 派生
    /// （行号栏背景=背景提亮、行号/正文/光标/预编辑统一用 palette.text）。
    fn resolve(theme: &Theme) -> Self {
        let palette = theme.palette();
        // 前景比背景亮 → 视为深色主题（不依赖具体主题枚举，Custom 也适用）
        let dark = luminance(palette.text) > luminance(palette.background);
        if !dark {
            return Self {
                selection: SELECTION_COLOR,
                caret: CARET_COLOR,
                gutter_bg: GUTTER_BG,
                gutter_text: GUTTER_TEXT,
                preedit_text: PREEDIT_TEXT,
                preedit_underline: PREEDIT_UNDERLINE,
                // 滚动条用前景色低透明度叠加，两种主题都自然成立
                scrollbar_track: Color::from_rgba8(0x00, 0x00, 0x00, 0.05),
                scrollbar_thumb: Color::from_rgba8(0x00, 0x00, 0x00, 0.30),
            };
        }
        let text = palette.text;
        Self {
            selection: Color { a: 0.25, ..text },
            caret: text,
            gutter_bg: lighten(palette.background, 0.12),
            gutter_text: Color { a: 0.55, ..text },
            preedit_text: text,
            preedit_underline: Color { a: 0.6, ..text },
            scrollbar_track: Color { a: 0.06, ..palette.text },
            scrollbar_thumb: Color { a: 0.38, ..palette.text },
        }
    }
}

impl Widget<super::Message, Theme, iced::Renderer> for EditorView {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::INFINITE))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let core = self.core.borrow();
        let palette = theme.palette();
        let colors = EditorColors::resolve(theme);
        let lh = core.line_height();
        let char_w = core.char_width();
        // P13：横向滚动偏移——正文/选区/光标的文档系坐标统一扣减它，
        // 行号栏固定不动（与主流编辑器一致）
        let scroll_left = core.scroll_left;

        // 背景与行号栏
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            palette.background,
        );
        let gutter_w = core.gutter_width();
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    width: gutter_w,
                    ..bounds
                },
                ..renderer::Quad::default()
            },
            colors.gutter_bg,
        );

        // 选区高亮：只画与视口相交的行（双宽感知）
        if let Some((sel_start, sel_end)) = core.ordered_selection() {
            let last_line = core.doc.line_count().saturating_sub(1);
            for line in sel_start.line..=sel_end.line.min(last_line) {
                let row = line as f32;
                if row < core.scroll_top || row > core.scroll_top + core.viewport_h / lh {
                    continue;
                }
                let text = core.line_text(line);
                let start_col = if line == sel_start.line { sel_start.col } else { 0 };
                let end_col = if line == sel_end.line {
                    sel_end.col
                } else {
                    text.chars().count()
                };
                if end_col <= start_col {
                    continue;
                }
                let x0 = prefix_width(&text, start_col.min(text.chars().count())) * char_w;
                let x1 =
                    prefix_width(&text, end_col.min(text.chars().count())) * char_w;
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + gutter_w + x0 - scroll_left,
                            y: bounds.y + (row - core.scroll_top) * lh,
                            width: (x1 - x0).max(char_w),
                            height: lh,
                        },
                        ..renderer::Quad::default()
                    },
                    colors.selection,
                );
            }
        }

        // 文本与行号：只为可见行调用排版（虚拟化的核心）；启用高亮时按语法分色
        let (first, last) = core.visible_range();
        for line in first..=last {
            let y = bounds.y + (line as f32 - core.scroll_top) * lh;

            renderer.fill_text(
                core_text::Text {
                    content: (line + 1).to_string(),
                    bounds: Size::new(gutter_w - GUTTER_MIN, lh),
                    size: Pixels(core.font_size() * GUTTER_FONT_SCALE),
                    line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                    font: Font::MONOSPACE,
                    align_x: core_text::Alignment::Right,
                    align_y: alignment::Vertical::Top,
                    shaping: core_text::Shaping::Basic,
                    wrapping: core_text::Wrapping::None,
                },
                Point::new(bounds.x, y),
                colors.gutter_text,
                bounds,
            );

            let text = core.line_text(line);
            if text.is_empty() {
                continue;
            }
            let runs = core.highlight_runs(line, &text);

            if runs.is_empty() {
                renderer.fill_text(
                    core_text::Text {
                        content: text,
                        bounds: Size::new((bounds.width - gutter_w).max(0.0), lh),
                        size: Pixels(core.font_size()),
                        line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                        font: Font::MONOSPACE,
                        align_x: core_text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: core_text::Shaping::Advanced,
                        wrapping: core_text::Wrapping::None,
                    },
                    Point::new(bounds.x + gutter_w - scroll_left, y),
                    palette.text,
                    bounds,
                );
            } else {
                for run in &runs {
                    let segment: String = text
                        .chars()
                        .skip(run.start_col)
                        .take(run.end_col.saturating_sub(run.start_col))
                        .collect();
                    if segment.is_empty() {
                        continue;
                    }
                    let offset_px =
                        prefix_width(&text, run.start_col) * char_w;
                    let [r, g, b, a] = run.color;
                    renderer.fill_text(
                        core_text::Text {
                            content: segment,
                            bounds: Size::new(
                                (bounds.width - gutter_w).max(0.0),
                                lh,
                            ),
                            size: Pixels(core.font_size()),
                            line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                            font: Font::MONOSPACE,
                            align_x: core_text::Alignment::Default,
                            align_y: alignment::Vertical::Top,
                            shaping: core_text::Shaping::Advanced,
                            wrapping: core_text::Wrapping::None,
                        },
                        Point::new(bounds.x + gutter_w + offset_px - scroll_left, y),
                        Color::from_rgba8(
                            (r * 255.0).round() as u8,
                            (g * 255.0).round() as u8,
                            (b * 255.0).round() as u8,
                            a,
                        ),
                        bounds,
                    );
                }
            }
        }

        // 输入法预编辑串（组字中）：内联显示在光标处，带下划线
        if let Some(preedit) = core.preedit.clone() {
            if !preedit.is_empty() {
                let caret = core.caret_rect_relative();
                let width = (display_cols(&preedit) * char_w).max(24.0);
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + caret.x,
                            y: bounds.y + caret.y + lh - 3.0,
                            width,
                            height: 2.0,
                        },
                        ..renderer::Quad::default()
                    },
                    colors.preedit_underline,
                );
                renderer.fill_text(
                    core_text::Text {
                        content: preedit,
                        bounds: Size::new(width + 60.0, lh),
                        size: Pixels(core.font_size()),
                        line_height: core_text::LineHeight::Absolute(Pixels(lh)),
                        font: Font::MONOSPACE,
                        align_x: core_text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: core_text::Shaping::Advanced,
                        wrapping: core_text::Wrapping::None,
                    },
                    Point::new(bounds.x + caret.x, bounds.y + caret.y),
                    colors.preedit_text,
                    bounds,
                );
            }
        }

        // 光标竖线
        let caret = core.caret_rect_relative();
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x + caret.x,
                    y: bounds.y + caret.y,
                    ..caret
                },
                ..renderer::Quad::default()
            },
            colors.caret,
        );

        // 垂直滚动条：内容超出视口才绘制（覆盖在正文右缘之上）
        let sb = VScrollbar::measure(
            core.doc.line_count(),
            core.viewport_h,
            lh,
            bounds.height,
            core.scroll_top,
        );
        if sb.needed {
            let track_rect = Rectangle {
                x: bounds.x + bounds.width - SCROLLBAR_EDGE_INSET - SCROLLBAR_WIDTH,
                y: bounds.y + sb.track_y,
                width: SCROLLBAR_WIDTH,
                height: sb.track_h,
            };
            let mut track_quad = renderer::Quad::default();
            track_quad.bounds = track_rect;
            track_quad.border.radius = Radius::from(SCROLLBAR_WIDTH / 2.0);
            renderer.fill_quad(track_quad, colors.scrollbar_track);

            let thumb = sb.thumb_rect(bounds.width);
            let mut thumb_quad = renderer::Quad::default();
            thumb_quad.bounds = Rectangle {
                x: bounds.x + thumb.x,
                y: bounds.y + thumb.y,
                ..thumb
            };
            thumb_quad.border.radius = Radius::from(SCROLLBAR_WIDTH / 2.0);
            renderer.fill_quad(thumb_quad, colors.scrollbar_thumb);
        }

        // 水平滚动条（P13）：内容超宽才绘制（覆盖在正文下缘之上）
        let hsb = HScrollbar::measure(
            core.max_line_cols as f32 * char_w,
            (bounds.width - gutter_w).max(0.0),
            bounds.width,
            core.scroll_left,
        );
        if hsb.needed {
            let track_rect = Rectangle {
                x: bounds.x + hsb.track_x,
                y: bounds.y + bounds.height - SCROLLBAR_EDGE_INSET - SCROLLBAR_THUMB_THICKNESS,
                width: hsb.track_w,
                height: SCROLLBAR_THUMB_THICKNESS,
            };
            let mut track_quad = renderer::Quad::default();
            track_quad.bounds = track_rect;
            track_quad.border.radius = Radius::from(SCROLLBAR_THUMB_THICKNESS / 2.0);
            renderer.fill_quad(track_quad, colors.scrollbar_track);

            let thumb = hsb.thumb_rect(bounds.height);
            let mut thumb_quad = renderer::Quad::default();
            thumb_quad.bounds = Rectangle {
                x: bounds.x + thumb.x,
                y: bounds.y + thumb.y,
                ..thumb
            };
            thumb_quad.border.radius = Radius::from(SCROLLBAR_THUMB_THICKNESS / 2.0);
            renderer.fill_quad(thumb_quad, colors.scrollbar_thumb);
        }
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, super::Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // 输入法常开：winit 在 Windows 上默认禁用 IME。
        // 每次事件都续约请求（运行时在下一帧 RedrawRequested 时消费），
        // 候选框始终跟随光标；预编辑串由本控件内联绘制，故不传给系统浮窗。
        {
            let core = self.core.borrow();
            let caret = core.caret_rect_relative();
            let ime: input_method::InputMethod = input_method::InputMethod::Enabled {
                cursor: Rectangle {
                    x: bounds.x + caret.x,
                    y: bounds.y + caret.y,
                    ..caret
                },
                purpose: input_method::Purpose::Normal,
                preedit: None,
            };
            shell.request_input_method(&ime);
        }

        match event {
            // 每帧把真实视口尺寸同步给核心（可见行数/滚动夹紧依赖它）
            iced::Event::Window(window::Event::RedrawRequested(_)) => {
                {
                    let mut core = self.core.borrow_mut();
                    core.set_viewport_height(bounds.height);
                    core.set_viewport_width(bounds.width);
                    // P12：可见区缺档超内联预算 → 通知应用层安排后台分批补建。
                    // 重复发布无害（应用层对同代在途任务幂等跳过）；铺建推进/
                    // 编辑换代后由下一帧重新评估，无需额外状态。
                    if core.needs_paving() {
                        shell.publish(super::Message::HighlightPaveNeeded);
                    }
                }
            }

            // 跟踪修饰键（P13）：滚轮事件不携带修饰键，
            // Shift+滚轮横向滚动只能靠这里维护的最近状态
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                self.core.borrow_mut().mods = *mods;
            }

            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // P2 焦点裁决：点在编辑区内=接管焦点；点在区外（查找框/工具栏）
                // =交出焦点。必须在越界 early-return 之前完成，否则收不到区外点击。
                let inside = cursor.position_over(bounds).is_some();
                self.core.borrow_mut().pointer_focus(inside);
                let Some(pos) = cursor.position_over(bounds) else {
                    return;
                };

                // 滚动条优先于文本命中：落在交互区则进入拖拽/轨道跳转，
                // 不触发文本选区。垂直条优先判定，右下角归属垂直条。
                {
                    let core = self.core.borrow();
                    let sb = VScrollbar::measure(
                        core.doc.line_count(),
                        core.viewport_h,
                        core.line_height(),
                        bounds.height,
                        core.scroll_top,
                    );
                    let (local_x, local_y) = (pos.x - bounds.x, pos.y - bounds.y);
                    if sb.hits(local_x, local_y, bounds.width) {
                        drop(core);
                        let mut core = self.core.borrow_mut();
                        if local_y >= sb.thumb_y && local_y <= sb.thumb_y + sb.thumb_h {
                            core.scrollbar_grab = Some(local_y - sb.thumb_y);
                        } else {
                            core.scroll_top = sb.scroll_for_track_click(local_y);
                            core.clamp_scroll();
                            core.scrollbar_grab = Some(sb.thumb_h * 0.5);
                        }
                        core.dragging = false; // 绝不因此进入文本拖选
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }

                    // 水平滚动条（P13）：下缘窄带，交互语义与垂直条对称
                    let hsb = HScrollbar::measure(
                        core.max_line_cols as f32 * core.char_width(),
                        (bounds.width - core.gutter_width()).max(0.0),
                        bounds.width,
                        core.scroll_left,
                    );
                    if hsb.hits(local_x, local_y, bounds.height) {
                        drop(core);
                        let mut core = self.core.borrow_mut();
                        if local_x >= hsb.thumb_x && local_x <= hsb.thumb_x + hsb.thumb_w {
                            core.hscrollbar_grab = Some(local_x - hsb.thumb_x);
                        } else {
                            core.scroll_left = hsb.scroll_for_track_click(local_x);
                            core.clamp_scroll_horizontal();
                            core.hscrollbar_grab = Some(hsb.thumb_w * 0.5);
                        }
                        core.dragging = false;
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }

                    // 未命中任何滚动条：走普通文本按下流程
                    drop(core);
                    {
                        let mut core = self.core.borrow_mut();
                        let hit = core.hit_test(pos.x - bounds.x, pos.y - bounds.y);
                        core.dragging = true;
                        core.anchor = None;
                        core.cursor = hit;
                    }
                    shell.publish(super::Message::EditorNavChanged);
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(pos) = cursor.position_over(bounds) else {
                    return;
                };
                let mut core = self.core.borrow_mut();

                // 垂直滚动条拖拽中：按抓取偏移反解 scroll_top
                if let Some(grab) = core.scrollbar_grab {
                    let sb = VScrollbar::measure(
                        core.doc.line_count(),
                        core.viewport_h,
                        core.line_height(),
                        bounds.height,
                        core.scroll_top,
                    );
                    core.scroll_top = sb.scroll_for_thumb_y(pos.y - bounds.y - grab);
                    core.clamp_scroll();
                    drop(core);
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }

                // 水平滚动条拖拽中（P13）：按抓取偏移反解 scroll_left
                if let Some(grab) = core.hscrollbar_grab {
                    let hsb = HScrollbar::measure(
                        core.max_line_cols as f32 * core.char_width(),
                        (bounds.width - core.gutter_width()).max(0.0),
                        bounds.width,
                        core.scroll_left,
                    );
                    core.scroll_left = hsb.scroll_for_thumb_x(pos.x - bounds.x - grab);
                    core.clamp_scroll_horizontal();
                    drop(core);
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }

                if !core.is_dragging() {
                    return;
                }
                let hit = core.hit_test(pos.x - bounds.x, pos.y - bounds.y);
                if core.cursor != hit {
                    // 拖选：锚点固定在按下时的位置（即移动前的光标）
                    if core.anchor.is_none() {
                        core.anchor = Some(core.cursor);
                    }
                    core.cursor = hit;
                    core.ensure_visible_pub();
                    drop(core);
                    shell.publish(super::Message::EditorNavChanged);
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let mut core = self.core.borrow_mut();
                core.dragging = false;
                core.scrollbar_grab = None;
                core.hscrollbar_grab = None;
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if !cursor.is_over(bounds) {
                    return;
                }
                // P3：winit 0.30 Windows 上滚轮上推上报 LineDelta(y=+1)，其 changelog
                // 明确「positive Y means moving the content down」（视口向文档头走）；
                // scroll_by_lines 内部已是 scroll_top -= lines，这里再取负就会方向反转。
                // P13：Shift+滚轮改走横向（Windows 惯例，winit 不代做换轴，
                // 修饰键靠 KeyModifiersChanged 维护的最近状态）；触控板的
                // 原生横向分量（x≠0）也直接横滚。
                let (dx_cols, dy_lines, shift) = {
                    let core = self.core.borrow();
                    match delta {
                        mouse::ScrollDelta::Lines { x, y } => (
                            *x * SCROLL_LINES_PER_NOTCH,
                            *y * SCROLL_LINES_PER_NOTCH,
                            core.mods.shift(),
                        ),
                        mouse::ScrollDelta::Pixels { x, y } => (
                            *x / core.char_width().max(1e-3),
                            *y / core.line_height().max(1e-3),
                            core.mods.shift(),
                        ),
                    }
                };
                if shift {
                    self.core.borrow_mut().scroll_by_columns(dy_lines);
                } else {
                    let mut core = self.core.borrow_mut();
                    if dx_cols != 0.0 {
                        core.scroll_by_columns(dx_cols);
                    }
                    core.scroll_by_lines(dy_lines);
                }
                shell.request_redraw();
                shell.capture_event();
            }

            // 输入法：组字过程（预编辑串内联显示）与上屏。
            // P2：事件会被广播给所有控件，只有本控件持有焦点时才消费并捕获；
            // 否则原样放行，让真正持有焦点的控件（查找框等）处理。
            iced::Event::InputMethod(input_method::Event::Preedit(content, _range)) => {
                if self.core.borrow_mut().ime_preedit(content.clone()) {
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            iced::Event::InputMethod(input_method::Event::Commit(text)) => {
                match self.core.borrow_mut().ime_commit(text) {
                    ImeCommit::Ignored => {} // 焦点在别处：不捕获
                    ImeCommit::Consumed(inserted) => {
                        // 统一走应用层编辑入口：获得加载期守卫（active_load）
                        // 与置脏/撤销语义；不再直接 insert_str 绕过 apply_edit
                        if let Some(text) = inserted {
                            shell.publish(super::Message::Edit(EditOp::InsertText(text)));
                        }
                        shell.request_redraw();
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        if let Some(pos) = cursor.position_over(bounds) {
            let core = self.core.borrow();
            let sb = VScrollbar::measure(
                core.doc.line_count(),
                core.viewport_h,
                core.line_height(),
                bounds.height,
                core.scroll_top,
            );
            // 滚动条上：拖拽中给 Grabbing，悬停给 Grab；正文仍是文本光标
            if sb.hits(pos.x - bounds.x, pos.y - bounds.y, bounds.width) {
                return if core.scrollbar_grab.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Grab
                };
            }
            // 水平滚动条（P13）同款指针语义
            let hsb = HScrollbar::measure(
                core.max_line_cols as f32 * core.char_width(),
                (bounds.width - core.gutter_width()).max(0.0),
                bounds.width,
                core.scroll_left,
            );
            if hsb.hits(pos.x - bounds.x, pos.y - bounds.y, bounds.height) {
                return if core.hscrollbar_grab.is_some() {
                    mouse::Interaction::Grabbing
                } else {
                    mouse::Interaction::Grab
                };
            }
        }
        if cursor.is_over(bounds) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // P9 断言用；非测试代码只经 Document::line_ending() 间接接触该类型
    use editpad_core::LineEnding;

    fn core_with(text: &str) -> EditorCore {
        let mut c = EditorCore::default();
        c.reset_document(Document::from_str(text));
        c
    }

    #[test]
    fn insert_advances_cursor_and_updates_lines() {
        let mut c = core_with("hello\nworld\n");
        c.cursor = CursorPos { line: 0, col: 5 };
        c.insert_str(",\neditpad");
        assert_eq!(c.doc.to_text(), "hello,\neditpad\nworld\n");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 7 });
    }

    #[test]
    fn backspace_joins_lines_and_deletes_selection() {
        let mut c = core_with("ab\ncd\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.backspace(); // 吃掉换行符
        assert_eq!(c.doc.to_text(), "abcd\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });

        c.reset_document(Document::from_str("abcd"));
        c.anchor = Some(CursorPos { line: 0, col: 1 });
        c.cursor = CursorPos { line: 0, col: 3 };
        c.backspace(); // 删除选区 "bc"
        assert_eq!(c.doc.to_text(), "ad");
        assert_eq!(c.anchor, None);
    }

    #[test]
    fn delete_forward_removes_next_char() {
        let mut c = core_with("abc");
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "bc");
    }

    #[test]
    fn motions_respect_logical_lines() {
        let mut c = core_with("abc\nx\nlonger\n");
        c.cursor = CursorPos { line: 0, col: 3 };
        c.apply_motion(Motion::Right, false); // 折到下一行行首
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });

        c.apply_motion(Motion::End, false);
        assert_eq!(c.cursor.col, 1);

        c.apply_motion(Motion::Down, false); // 到 "longer"，列被夹紧
        assert_eq!(c.cursor, CursorPos { line: 2, col: 1 });

        c.apply_motion(Motion::DocEnd, false);
        // 与主流编辑器一致：文档以换行结尾时，末尾停在最后的空行上
        assert_eq!(c.cursor, CursorPos { line: 3, col: 0 });
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut c = core_with("");
        c.insert_str("第一版");
        c.insert_str("+第二版");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "第一版");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "第一版");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "第一版+第二版");
        assert!(!c.redo()); // 到底了
    }

    #[test]
    fn select_span_and_replace_selection() {
        let mut c = core_with("你好世界");
        c.select_span(0, 1, 2); // 选中 “好世”
        assert_eq!(c.selected_text().as_deref(), Some("好世"));
        c.replace_selection("-");
        assert_eq!(c.doc.to_text(), "你-界");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    }

    #[test]
    fn hit_test_maps_to_visible_columns() {
        let mut c = core_with("abcdef\nxy\n");
        c.set_viewport_height(200.0);
        let gutter = c.gutter_width();

        let hit = c.hit_test(gutter + 1.0, 1.0 * c.line_height());
        assert_eq!(hit, CursorPos { line: 1, col: 0 });

        // 超出该行尾的点击被夹紧到行尾
        let hit = c.hit_test(gutter + 100.0 * c.char_width(), 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 6 });
    }

    #[test]
    fn ensure_visible_follows_cursor_jump() {
        let mut c = core_with(&(0..500).map(|i| format!("line{i}\n")).collect::<String>());
        c.jump_to_line(480);
        let (first, _) = c.visible_range();
        let cursor_row = 479.0;
        assert!(
            (cursor_row >= first as f32)
                && (cursor_row <= first as f32 + c.viewport_height() / c.line_height())
        );
    }

    #[test]
    fn default_metrics_match_legacy_constants() {
        let c = core_with("");
        assert_eq!(c.font_size(), 16.0);
        // 与旧 LINE_HEIGHT/CHAR_WIDTH 常量完全一致，保证默认观感不变
        assert_eq!(c.line_height(), 22.0);
        assert_eq!(c.char_width(), 9.0);
    }

    #[test]
    fn set_font_size_clamps_and_rescales_metrics() {
        use editpad_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE};

        let mut c = core_with("hello\nworld\n");
        c.set_font_size(40.0); // 越上界被夹紧
        assert_eq!(c.font_size(), MAX_FONT_SIZE);
        c.set_font_size(1.0); // 越下界被夹紧
        assert_eq!(c.font_size(), MIN_FONT_SIZE);
        c.set_font_size(f32::NAN); // 非有限值回退默认
        assert_eq!(c.font_size(), 16.0);

        c.set_font_size(20.0);
        // 断言具体值：20 × 1.375 与 20 × 0.5625 均为二进制精确值
        assert_eq!(c.line_height(), 27.5);
        assert_eq!(c.char_width(), 11.25);

        // 字号变大后同样视口可见行数变少（用长文档避免被文档末尾夹平）
        let mut long = core_with(&(0..200).map(|i| format!("l{i}\n")).collect::<String>());
        long.set_viewport_height(220.0);
        long.set_font_size(16.0);
        let (_, last_16) = long.visible_range();
        long.set_font_size(MAX_FONT_SIZE);
        let (_, last_big) = long.visible_range();
        assert!(last_big < last_16, "字号变大后可见行应变少");
    }

    #[test]
    fn set_font_size_keeps_cursor_visible() {
        let mut c = core_with(&(0..500).map(|i| format!("line{i}\n")).collect::<String>());
        c.set_viewport_height(400.0);
        c.set_font_size(24.0);
        c.jump_to_line(480);
        // 字号再调大也不得把光标挤出视口（ensure_visible 的不变量以浮点 scroll_top 表述）
        c.set_font_size(28.0);
        let cursor_row = 479.0;
        let rows_in_view = c.viewport_height() / c.line_height();
        assert!(c.scroll_top <= cursor_row, "光标行应在视口顶之下");
        assert!(
            cursor_row <= c.scroll_top + rows_in_view - 1.0 + 1e-3,
            "光标行应在视口底之上"
        );
    }

    // ---------- P3 滚轮方向契约 ----------

    #[test]
    fn scroll_by_lines_sign_contract_matches_winit() {
        // winit 约定：滚轮上推上报 LineDelta(y=+1)（"positive Y means moving the
        // content down"，即视口向文档头方向走）。控件层把 y×3 直接传给本方法，
        // 因此契约是：正数 → scroll_top 减小（看到更早的内容）；负数反之。
        let mut c = core_with(&(0..200).map(|i| format!("l{i}\n")).collect::<String>());
        c.set_viewport_height(220.0);
        c.scroll_top = 50.0;
        c.clamp_scroll();

        c.scroll_by_lines(3.0); // 滚轮上推一格
        assert!((c.scroll_top - 47.0).abs() < 1e-4, "上推应向文档头滚动，实际 {}", c.scroll_top);

        c.scroll_by_lines(-6.0); // 滚轮下推两格
        assert!((c.scroll_top - 53.0).abs() < 1e-4, "下推应向文档尾滚动，实际 {}", c.scroll_top);

        // 头部钳制：继续上推不得变负
        c.scroll_by_lines(100.0);
        assert_eq!(c.scroll_top, 0.0);
    }

    // ---------- P2 输入法焦点过滤 ----------
    #[test]
    fn ime_events_are_gated_by_focus() {
        let mut c = core_with("");
        assert!(c.focused, "默认持有焦点（正文是初始焦点控件）");

        // 有焦点：组字串进入正文显示，上屏文本交由应用层插入
        assert!(c.ime_preedit("nihao".to_owned()));
        assert_eq!(c.preedit.as_deref(), Some("nihao"));
        assert_eq!(c.ime_commit("你好"), ImeCommit::Consumed(Some("你好".into())));
        assert_eq!(c.preedit, None, "上屏后预编辑串必须清掉");

        // 点击查找框（区外）→ 交出焦点；此后 IME 事件一律放行不消费
        c.pointer_focus(false);
        assert!(!c.focused);
        assert!(!c.ime_preedit("pinyin".to_owned()), "未聚焦不得消费预编辑");
        assert_eq!(c.preedit, None);
        assert_eq!(c.ime_commit("字"), ImeCommit::Ignored, "未聚焦时上屏不得进正文");

        // 点回编辑区 → 重新接管焦点，IME 恢复消费
        c.pointer_focus(true);
        assert!(c.ime_preedit("z".to_owned()));
    }

    #[test]
    fn focus_loss_discards_stale_preedit_and_empty_commit_only_clears() {
        let mut c = core_with("");
        c.ime_preedit("wei".to_owned());

        // 组字中途点去别的控件：残留预编辑串不能悬在正文里
        c.pointer_focus(false);
        assert_eq!(c.preedit, None);

        // 空提交（如输入法取消）：只清预编辑、无文本插入，但事件算已消费
        c.pointer_focus(true);
        c.ime_preedit("x".to_owned());
        assert_eq!(c.ime_commit(""), ImeCommit::Consumed(None));
        assert_eq!(c.preedit, None);
        assert_eq!(c.doc.to_text(), "", "空提交不得改动文档");
    }

    // ---------- P9 CRLF 按 EOL 单元处理 ----------

    #[test]
    fn backspace_joins_crlf_lines_in_one_press() {
        let mut c = core_with("ab\r\ncd");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.backspace();
        // 旧实现第一下只删 \r：两行没并上，看起来像按键无反应
        assert_eq!(c.doc.to_text(), "abcd", "一次退格必须删掉整个 \\r\\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
        assert_eq!(c.line_display_len(0), 4, "并行后不得残留孤立 \\r");

        // 撤销一步回到并行前（快照语义不受 EOL 单元影响）
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "ab\r\ncd");
    }

    #[test]
    fn delete_forward_removes_crlf_in_one_press() {
        let mut c = core_with("ab\r\ncd");
        c.cursor = CursorPos { line: 0, col: 2 }; // 行尾
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "abcd", "一次 Delete 必须删掉整个 \\r\\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });

        // 文档末尾的单个字符照常单删；越界探测不得 panic
        c.reset_document(Document::from_str("ab\r"));
        c.cursor = CursorPos { line: 0, col: 2 };
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "ab", "孤立尾部 \\r 单字符删除");
    }

    #[test]
    fn backspace_still_deletes_single_char_within_line() {
        let mut c = core_with("abc\r\ndef");
        c.cursor = CursorPos { line: 0, col: 3 }; // 行尾 'c' 之后
        c.backspace();
        assert_eq!(c.doc.to_text(), "ab\r\ndef", "行内退格不得误删行尾 CRLF");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    }

    #[test]
    fn enter_and_paste_follow_dominant_line_ending() {
        // CRLF 文档：回车（插 \n）与粘贴的混合行尾都归一为 \r\n
        let mut c = core_with("a\r\nb");
        c.cursor = CursorPos { line: 0, col: 1 }; // 'a' 之后
        c.insert_str("\n");
        assert_eq!(c.doc.to_text(), "a\r\n\r\nb", "回车应插入主导行尾 CRLF");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf);

        // 此时光标在第 1 行行首；粘贴混合行尾文本统一改写为主导行尾
        c.insert_str("剪贴板\r\n来了\n多行");
        assert_eq!(
            c.doc.to_text(),
            "a\r\n剪贴板\r\n来了\r\n多行\r\nb",
            "粘贴的 LF/CRLF 应统一改写为主导行尾"
        );
        assert_eq!(c.cursor, CursorPos { line: 3, col: 2 }, "光标推进按 EOL 单元计列");

        // LF 文档：CRLF 粘贴归一为 \n，行为与旧版一致
        let mut lf = core_with("");
        lf.insert_str("x\r\ny\rz");
        assert_eq!(lf.doc.to_text(), "x\ny\nz", "LF 文档把 CRLF/孤立 CR 归一为 LF");
        assert_eq!(lf.cursor, CursorPos { line: 2, col: 1 });

        // 无换行内容不产生任何改动（光标先移到文末再插入）
        let mut plain = core_with("plain");
        plain.cursor = CursorPos { line: 0, col: 5 };
        plain.insert_str("中文🚀");
        assert_eq!(plain.doc.to_text(), "plain中文🚀");
    }

    #[test]
    fn crlf_document_keeps_eol_through_undo() {
        let mut c = core_with("l1\r\nl2\r\n");
        c.cursor = CursorPos { line: 1, col: 2 }; // 行尾
        c.insert_str("\n");
        assert_eq!(c.doc.to_text(), "l1\r\nl2\r\n\r\n", "行尾回车追加一个 CRLF");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "l1\r\nl2\r\n", "撤销完整还原");
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "快照携带同一行尾元数据");
    }

    // ---------- P12 高亮后台分批补建 ----------

    #[test]
    fn needs_paving_tracks_visible_deficit() {
        let mut c = core_with(&(0..600).map(|i| format!("fn f{i}() {{}}\n")).collect::<String>());
        c.set_language(Some("rs"));
        c.set_viewport_height(400.0);
        c.clamp_scroll();
        assert!(
            !c.needs_paving(),
            "顶部可见区落在初始检查点预算内，无需后台铺建"
        );

        // 跳到文末：可见区末行 599 需 5 个档位、只有初始 1 个 → 必须请求铺建
        c.scroll_top = 100_000.0;
        c.clamp_scroll();
        assert!(c.needs_paving(), "大跳转后的可见区缺档必须被识别");

        // 纯文本路径（无高亮器）恒不需要
        let plain = core_with(&"x\n".repeat(600));
        assert!(!plain.needs_paving());
    }

    #[test]
    fn pave_snapshot_and_conditional_install_flow() {
        let mut c = core_with(&(0..600).map(|i| format!("let a{i} = {i};\n")).collect::<String>());
        c.set_language(Some("rs"));

        // 起点快照 + 后台式推进
        let (gen, snapshot) = c.highlight_pave_snapshot().expect("已启用高亮");
        assert_eq!(c.highlight_generation(), Some(gen));
        let mut worker = snapshot.clone();
        let built = worker.advance_checkpoints(32, 600, &mut |i| format!("let w{i} = {i};"));
        assert_eq!(built, 3, "600 行的完整档位起点为 128/256/384，共 3 个");
        assert!(worker.checkpoints_len() > snapshot.checkpoints_len());

        // 代次一致 → 安装成功且状态生效
        assert!(c.install_highlighter_if_current(gen, worker.clone()));
        assert_eq!(
            c.highlight_checkpoints_len(),
            Some(worker.checkpoints_len()),
            "安装后检查点应与成果一致"
        );

        // 代次不符（模拟安装前发生过编辑）→ 整体拒绝
        c.cursor = CursorPos { line: 599, col: 0 };
        c.insert_str("x"); // 真实编辑：内部 invalidate_from → 换代
        let stale_gen = gen;
        assert_ne!(c.highlight_generation(), Some(stale_gen), "编辑必须换代");
        assert!(
            !c.install_highlighter_if_current(stale_gen, worker),
            "换代后的迟到成果不得覆盖当前高亮器"
        );
    }

    // ---------- P14 Tab 输入与制表位列宽 ----------

    #[test]
    fn tab_widths_advance_to_next_tab_stop() {
        // 'a'=1 列；Tab 在第 1 列推进到第 4 列（占 3）；'b'=1 → 共 5
        assert_eq!(display_cols("a\tb"), 5.0);
        // Tab 恰在制表位上：至少仍占 1 列（"abc\t" = 3+1）
        assert_eq!(display_cols("abc\td"), 5.0);
        assert_eq!(display_cols("\t"), TAB_STOP_COLS as f32);

        // prefix_width 按字符索引取前缀宽：过 Tab 处发生跳跃
        assert_eq!(prefix_width("a\tb", 1), 1.0, "只有 'a'");
        assert_eq!(prefix_width("a\tb", 2), 4.0, "'a'+Tab 应到第 4 制表位");
        assert_eq!(prefix_width("a\tb", 3), 5.0, "再加 'b'");

        // 与像素换算单调一致（caret/选区/着色段共用此路径）
        let cw = EditorCore::default().char_width();
        for i in 0..=4 {
            let a = prefix_width("a\tb", i) * cw;
            let b = prefix_width("a\tb", (i + 1).min(3)) * cw;
            assert!(b >= a, "前缀宽必须随索引单调不减");
        }
    }

    #[test]
    fn tab_inserts_as_literal_char_with_tab_aware_caret() {
        // Tab 是真实字符进文档：保存往返不失真，光标按字符推进 1
        let mut c = core_with("");
        c.insert_str("\t");
        assert_eq!(c.doc.to_text(), "\t", "文档存真实 \\t 字符");
        assert_eq!(c.cursor.col, 1);

        // 光标矩形按制表位宽度落位（第 1 个字符之后 = 第 4 显示列）
        let expect_x = c.gutter_width() + prefix_width("\t", 1) * c.char_width();
        assert!((c.caret_rect_relative().x - expect_x).abs() < 1e-4);

        // 行首 Tab 后再打字：后续列宽从制表位起算（hit_test 往返验证）
        c.cursor = CursorPos { line: 0, col: 1 };
        c.insert_str("xy");
        assert_eq!(c.doc.to_text(), "\txy");
        let hit = c.hit_test(
            c.gutter_width() + prefix_width("\txy", 3) * c.char_width() + 1.0,
            0.0,
        );
        assert_eq!(hit.col, 3, "像素位置反查字符索引必须与正向宽度一致");
    }

    #[test]
    fn wide_line_cols_count_tab_expansion_for_hscroll_range() {
        // 含 Tab 的行：水平行程按展开后的显示列计（P13×P14 协同）
        let mut c = core_with(&format!("\t{}\n", "x".repeat(100)));
        c.set_viewport_width(400.0);
        assert_eq!(
            c.max_line_display_cols(),
            TAB_STOP_COLS + 100,
            "行首 Tab 占 4 列，高水位应按展开后计"
        );
    }

    // ---------- P13 水平滚动 ----------

    fn core_with_wide_line() -> EditorCore {
        // 一条 1000 列的超长行 + 若干短行：内容必然超出任何常规视口宽
        let mut c = core_with(&format!("{}\nshort\n", "x".repeat(1000)));
        c.set_viewport_width(400.0);
        c
    }

    #[test]
    fn hscrollbar_geometry_mirrors_vertical_contract() {
        let char_w = 16.0 * 0.5625;
        let content_px = 1000.0 * char_w;

        // 内容不超宽 → 不需要水平滚动条
        let fit = HScrollbar::measure(100.0, 400.0, 800.0, 0.0);
        assert!(!fit.needed);
        assert!(HScrollbar::measure(0.0, 400.0, 800.0, 0.0).needed == false);

        // 超宽 → 出现，滑块宽 ∝ 视口占比（未触底时）
        let sb = HScrollbar::measure(content_px, 400.0, 800.0, 0.0);
        assert!(sb.needed);
        let expect_w = (sb.track_w * 400.0 / content_px).clamp(THUMB_MIN_W, sb.track_w);
        assert!((sb.thumb_w - expect_w).abs() < 1e-3);
        assert!(sb.thumb_w > THUMB_MIN_W, "此比例下不应触底");

        // 极宽内容（20 万列）滑块触底到最小宽度
        let huge = HScrollbar::measure(200_000.0 * char_w, 400.0, 800.0, 0.0);
        assert_eq!(huge.thumb_w, THUMB_MIN_W);

        // 拖拽逆映射恒等；两端夹紧
        for &scroll in &[0.0f32, 100.0, 777.7] {
            let back = sb.scroll_for_thumb_x(sb.thumb_x + scroll / sb.range_px * (sb.track_w - sb.thumb_w));
            assert!(
                (back - scroll.min(sb.range_px)).abs() < 0.01,
                "scroll={scroll} 反解={back}"
            );
        }
        assert_eq!(sb.scroll_for_thumb_x(sb.track_x - 40.0), 0.0);
        assert!((sb.scroll_for_thumb_x(sb.track_x + sb.track_w) - sb.range_px).abs() < 1e-3);

        // 轨道点击把滑块中心对准点击处
        let click = sb.track_x + sb.track_w * 0.8;
        let scrolled = sb.scroll_for_track_click(click);
        let after = HScrollbar::measure(content_px, 400.0, 800.0, scrolled);
        let center_after = after.thumb_x + after.thumb_w / 2.0;
        assert!((center_after - click).abs() < 1.5);

        // 命中区只在下缘窄带
        assert!(!sb.hits(400.0, 700.0, 720.0), "中部不得算命中");
    }

    #[test]
    fn wide_document_tracks_max_cols_and_enables_hscroll() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        assert_eq!(c.max_line_display_cols(), 1000, "reset 时应精确重算最宽行");

        // 高水位只上调：编辑短行不缩水，编辑出更长的行要跟上
        c.cursor = CursorPos { line: 1, col: 5 };
        c.insert_str("yyyyyyyyyy"); // 短行 15 列，仍小于 1000
        assert_eq!(c.max_line_display_cols(), 1000);
        c.cursor = CursorPos { line: 2, col: 0 };
        c.insert_str(&"z".repeat(1200)); // 更长的新行
        assert_eq!(c.max_line_display_cols(), 1200);

        // 整体替换后精确重算（高水位回落）
        c.replace_whole_document(Document::from_str("tiny"));
        assert_eq!(c.max_line_display_cols(), 4);
    }

    #[test]
    fn ensure_visible_follows_cursor_horizontally() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        assert_eq!(c.scroll_left, 0.0);

        // 光标放到超长行末尾 → 视口右移让行尾可见
        c.cursor = CursorPos { line: 0, col: 1000 };
        c.ensure_visible_pub();
        assert!(c.scroll_left > 0.0, "行尾光标必须推动横向滚动");

        // 光标回到行首（Home/左移到 0 列）→ 视口回到最左
        c.cursor = CursorPos { line: 0, col: 0 };
        c.ensure_visible_pub();
        assert_eq!(c.scroll_left, 0.0, "第 0 列必须滚回左缘");

        // 手动横滚到中间再点视口内的位置 → 不打扰当前横向位置
        c.scroll_by_columns(50.0);
        let mid = c.scroll_left;
        c.cursor = CursorPos { line: 0, col: 60 };
        c.ensure_visible_pub();
        assert!((c.scroll_left - mid).abs() < f32::EPSILON.max(mid * 1e-6),
            "可见范围内的光标不得扰动横向滚动");
    }

    #[test]
    fn scroll_by_columns_clamps_to_content_width() {
        let mut c = core_with_wide_line();
        c.clamp_scroll_horizontal();
        assert_eq!(c.scroll_left, 0.0, "空行程时保持 0");

        c.scroll_by_columns(-5000.0);
        assert_eq!(c.scroll_left, 0.0, "左向越界夹到 0");

        c.scroll_by_columns(f32::MAX / 2.0); // 巨量右滚 → 夹到最大行程
        let max_expected =
            (c.max_line_display_cols() as f32 * c.char_width()) - c.text_viewport_w();
        assert!((c.scroll_left - max_expected).abs() < 1.0, "右缘应贴住最宽行");
    }

    #[test]
    fn hit_test_accounts_for_horizontal_scroll_offset() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        // 同一文档坐标的列，在滚动前后用相差 scroll_left 的 x 点击应命中同列
        let gutter = c.gutter_width();
        let char_w = c.char_width();
        let target_col = 20usize;

        c.scroll_left = 0.0;
        let hit_unscrolled = c.hit_test(gutter + target_col as f32 * char_w + 1.0, 10.0);
        c.scroll_by_columns(30.0); // 视口右移 30 列
        let hit_scrolled =
            c.hit_test(gutter + (target_col as f32 - 30.0) * char_w + 1.0, 10.0);
        assert_eq!(hit_unscrolled.col, target_col);
        assert_eq!(hit_scrolled.col, target_col, "横滚后命中测试必须补偿 scroll_left");
    }

    #[test]
    fn caret_rect_shifts_by_scroll_left() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        c.cursor = CursorPos { line: 0, col: 100 };
        c.ensure_visible_pub();
        let before = c.caret_rect_relative().x;

        c.scroll_by_columns(40.0);
        let after = c.caret_rect_relative().x;
        assert!(
            (before - after - 40.0 * c.char_width()).abs() < 1e-3,
            "光标矩形必须随横向滚动平移（IME 候选框与绘制共用此路径）"
        );
    }

    // ---------- 健壮性边界用例批 ----------

    #[test]
    fn oversized_line_number_jump_clamps_to_last_line() {
        // 「跳转到行」输入 usize::MAX / 手改配置等极端值不得 panic，
        // 必须夹紧到文档末行（ropey 越界是 panic 不是错误）
        let mut c = core_with("l1\nl2\nl3");
        c.jump_to_line(usize::MAX);
        assert_eq!(c.cursor.line, 2, "超大行号应落到末行");
        c.jump_to_line(0); // 0 与 1 等价（1 起）
        assert_eq!(c.cursor.line, 0);
        c.jump_to_line(2);
        assert_eq!(c.cursor.line, 1);

        // 带尾换行的文档：末行是换行后的空行，同样不得越界
        let mut trailing = core_with("l1\nl2\n");
        trailing.jump_to_line(usize::MAX);
        assert_eq!(trailing.cursor.line, 2);
    }

    #[test]
    fn single_line_document_edges_are_noops_not_panics() {
        let mut c = core_with("only");
        assert_eq!(c.doc.line_count(), 1);

        // 文档首退格 / 文档尾 Delete：无变化不崩溃
        c.backspace();
        assert_eq!(c.doc.to_text(), "only");
        c.cursor = CursorPos { line: 0, col: 4 };
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "only");

        // 行内移动到边界后继续同向移动被吸收
        c.apply_motion(Motion::DocEnd, false);
        c.apply_motion(Motion::Right, false);
        assert_eq!(c.cursor, CursorPos { line: 0, col: 4 });
        c.apply_motion(Motion::DocStart, false);
        c.apply_motion(Motion::Left, false);
        assert_eq!(c.cursor, CursorPos { line: 0, col: 0 });
    }

    #[test]
    fn four_byte_emoji_survives_edit_roundtrip() {
        // 4 字节字符（U+1F680）在插入/撤销/保存口径下都是普通标量
        let mut c = core_with("");
        c.insert_str("发射🚀!");
        assert_eq!(c.doc.to_text(), "发射🚀!");
        assert_eq!(c.doc.text_len(), 4, "emoji 按 1 个 Unicode 标量计");

        c.insert_str("\n更多🛰内容\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "发射🚀!", "撤销必须完整还原含 emoji 的状态");

        // 分块原子保存往返（P19 路径对非 ASCII 同样透明）
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("emoji-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("emoji.txt");
        editpad_core::save_document_atomic(&target, &c.doc).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "发射🚀!");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- 垂直滚动条 ----------

    #[test]
    fn scrollbar_hidden_when_content_fits() {
        // 内容不超视口 → 不需要滚动条（用户要求的核心行为）
        assert!(!VScrollbar::measure(10, 500.0, 22.0, 500.0, 0.0).needed);
        // 空文档 / 单行同样隐藏
        assert!(!VScrollbar::measure(0, 500.0, 22.0, 500.0, 0.0).needed);
        assert!(!VScrollbar::measure(1, 100.0, 22.0, 300.0, 0.0).needed);
    }

    #[test]
    fn scrollbar_appears_on_overflow_with_proportional_thumb() {
        let (lines, lh, vh, wh) = (1000usize, 20.0, 400.0, 400.0);
        let sb = VScrollbar::measure(lines, vh, lh, wh, 0.0);
        assert!(sb.needed);

        // 滑块高度 = 视口占比 × 轨道高（未触底最小值时）
        let expect_h = (sb.track_h * vh / (lines as f32 * lh)).max(THUMB_MIN_H);
        assert!((sb.thumb_h - expect_h).abs() < 1e-3);
        assert!(sb.thumb_y >= sb.track_y);

        // 滚动到 clamp 允许的最大值（行数-可见行+1）→ 滑块应贴到轨道底部
        let max_scroll = lines as f32 - vh / lh + 1.0;
        let bot = VScrollbar::measure(lines, vh, lh, wh, max_scroll + 10.0);
        assert!(
            (bot.thumb_y + bot.thumb_h - (bot.track_y + bot.track_h)).abs() < 1e-2,
            "超出行程的 scroll_top 应被夹到滑块贴底"
        );
    }

    #[test]
    fn thumb_drag_roundtrips_through_inverse_mapping() {
        let (lines, lh, vh, wh) = (2000usize, 22.0, 550.0, 800.0);
        for &scroll in &[0.0f32, 7.5, 123.4, 900.0] {
            let sb = VScrollbar::measure(lines, vh, lh, wh, scroll);
            assert!(sb.needed);
            // 滑块位置反解回 scroll_top 必须是恒等映射
            let back = sb.scroll_for_thumb_y(sb.thumb_y);
            assert!(
                (back - scroll.min(sb.range_lines)).abs() < 0.01,
                "scroll={scroll} 反解={back}"
            );
        }
        // 拖出上下边界都要夹紧
        let sb = VScrollbar::measure(lines, vh, lh, wh, 100.0);
        assert_eq!(sb.scroll_for_thumb_y(sb.track_y - 50.0), 0.0, "拖过头=回到顶部");
        let over = sb.scroll_for_thumb_y(sb.track_y + sb.track_h + 50.0);
        assert!((over - sb.range_lines).abs() < 1e-3, "拖到底=最大行程");
    }

    #[test]
    fn track_click_centers_thumb_on_cursor() {
        let sb = VScrollbar::measure(1500usize, 660.0, 22.0, 660.0, 10.0);
        assert!(sb.needed);
        let click = sb.track_y + sb.track_h * 0.8;
        let scrolled = sb.scroll_for_track_click(click);
        // 点击后滑块中心应落在点击处附近（±1px）
        let after = VScrollbar::measure(1500usize, 660.0, 22.0, 660.0, scrolled);
        let center_after = after.thumb_y + after.thumb_h / 2.0;
        assert!((center_after - click).abs() < 1.5, "点击 {click}，中心停在 {center_after}");
    }

    #[test]
    fn huge_document_keeps_grabbable_thumb_min_height() {
        // 50MB 场景：几十万行 → 视口占比极小，滑块不得小于可抓握的最小高度
        let sb = VScrollbar::measure(600_000usize, 800.0, 22.0, 800.0, 0.0);
        assert_eq!(sb.thumb_h, THUMB_MIN_H);
        // 最小滑块仍能覆盖完整行程
        let bottom = sb.scroll_for_thumb_y(sb.track_y + sb.track_h);
        assert!((bottom - sb.range_lines).abs() < 1e-3);
    }

    #[test]
    fn scrollbar_flows_through_core_state() {
        // 经 EditorCore 的真实链路：设视口 → 推导 → 用逆映射模拟拖拽 → 夹紧
        let mut c = core_with(&"line\n".repeat(500));
        c.set_viewport_height(400.0);
        c.scroll_top = 12345.0; // 故意越界
        c.clamp_scroll();

        let sb = VScrollbar::measure(
            c.doc.line_count(),
            c.viewport_h,
            c.line_height(),
            400.0,
            c.scroll_top,
        );
        assert!(sb.needed);
        c.scroll_top = sb.scroll_for_thumb_y(sb.track_y + (sb.track_h - sb.thumb_h) * 0.5);
        c.clamp_scroll();
        assert!(c.scroll_top > 0.0 && c.scroll_top <= sb.range_lines);

        // 文档缩到视口内后滚动条消失
        c.reset_document(Document::from_str("short"));
        c.set_viewport_height(400.0);
        let sb = VScrollbar::measure(
            c.doc.line_count(),
            c.viewport_h,
            c.line_height(),
            400.0,
            c.scroll_top,
        );
        assert!(!sb.needed, "内容装得下就必须隐藏滚动条");

        // 命中区只在右侧窄带
        assert!(sb.hits(798.0, 300.0, 800.0) == false || !sb.needed);
        assert!(!sb.hits(100.0, 300.0, 800.0), "正文区域不得算进滚动条命中区");
    }
}
