//! 基于 Rope 的文本文档。
//!
//! M0 阶段只提供最小接口，验证 ropey 的行为并建立测试基线；
//! M2 切换自绘渲染层时，`Document` 将成为编辑器的唯一数据源。

use ropey::Rope;

/// 行尾风格。加载时按全文统计检测主导行尾（P9），
/// 编辑层的回车插入、粘贴归一、退格/删除的 EOL 单元语义都以它为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// `\n` —— Unix 风格；空文档/无换行文本/平票时的保守默认。
    #[default]
    Lf,
    /// `\r\n` —— Windows 风格。
    CrLf,
    /// 孤立 `\r` —— 经典 Mac 风格（罕见；rope 渲染层并不把它当行界，
    /// 仅用于归一化时不再制造混合行尾）。
    Cr,
}

impl LineEnding {
    /// 按全文统计检测主导行尾：CRLF / LF / CR 三种计数，多者胜；
    /// 平票或全文没有换行时回退 [`LineEnding::Lf`]（与旧版行为一致）。
    pub fn detect(text: &str) -> Self {
        let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                        crlf += 1;
                    } else {
                        cr += 1;
                    }
                }
                '\n' => lf += 1,
                _ => {}
            }
        }
        if crlf > lf && crlf > cr {
            LineEnding::CrLf
        } else if cr > lf {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }

    /// 该风格对应的换行字符串。
    pub fn newline(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// 把任意来源文本里的换行（`\r\n`、`\n`、孤立 `\r`）统一改写为本风格。
    /// 粘贴、输入法上屏、按键字符在入文前都经过这里，从源头杜绝混合行尾。
    pub fn normalize(self, text: &str) -> String {
        let target = self.newline();
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    // \r\n 是一个逻辑换行：整体消费，避免改写成两个换行
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push_str(target);
                }
                '\n' => out.push_str(target),
                other => out.push(other),
            }
        }
        out
    }
}

/// 跨块的主导行尾计数器（P19 流式加载用）。
///
/// 语义与 [`LineEnding::detect`] 完全一致（三种换行计数、多者胜、
/// 平票回退 LF），但允许文本按任意块多次推送——块边界可能恰好落在
/// `\r\n` 中间，此时 `\r` 悬置到下一块首字符到达后再裁决。
#[derive(Debug, Default)]
pub struct EolCounter {
    crlf: usize,
    lf: usize,
    cr: usize,
    /// 上一块以 `\r` 结尾且尚未裁决
    pending_cr: bool,
}

impl EolCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推送一段解码后的文本（块大小任意、次数任意）。
    pub fn push(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        // 先裁决上一块悬置的 `\r`
        if self.pending_cr {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    self.crlf += 1;
                    self.pending_cr = false;
                }
                Some(_) => {
                    // 孤立 `\r` 定案；当前字符留给下方常规循环处理
                    self.cr += 1;
                    self.pending_cr = false;
                }
                None => return, // 空块：继续悬置
            }
        }
        while let Some(c) = chars.next() {
            match c {
                '\r' => match chars.peek() {
                    Some('\n') => {
                        chars.next();
                        self.crlf += 1;
                    }
                    Some(_) => self.cr += 1,
                    None => self.pending_cr = true, // 块尾：悬置待下块裁决
                },
                '\n' => self.lf += 1,
                _ => {}
            }
        }
    }

    /// 文本推送完毕，取主导行尾（多数判定与 `detect` 逐字一致）。
    pub fn finish(mut self) -> LineEnding {
        if self.pending_cr {
            self.cr += 1;
        }
        if self.crlf > self.lf && self.crlf > self.cr {
            LineEnding::CrLf
        } else if self.cr > self.lf {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    rope: Rope,
    /// 主导行尾（P9）：`from_str` 时自动检测；编辑层用它统一换行语义。
    eol: LineEnding,
}

impl Document {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            eol: LineEnding::Lf,
        }
    }

    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            eol: LineEnding::detect(text),
        }
    }

    /// 用已建好的 rope 与行尾组装文档。
    ///
    /// crate 内流式加载（P19）专用：rope 由 `RopeBuilder` 增量构建
    /// （全程只有 rope 一份正文），行尾由 [`EolCounter`] 跨块统计后传入。
    #[allow(dead_code)]
    pub(crate) fn from_parts(rope: Rope, eol: LineEnding) -> Self {
        Self { rope, eol }
    }

    /// 文档的主导行尾（编辑层据此归一插入文本、按 EOL 单元删除）。
    pub fn line_ending(&self) -> LineEnding {
        self.eol
    }

    /// 字符数（Unicode scalar 单位，与 ropey 一致）。
    pub fn text_len(&self) -> usize {
        self.rope.len_chars()
    }

    /// 字节数（UTF-8 编码口径，rope 根节点缓存，O(1)）。
    /// P31 心跳的大小节流按真实字节判——字符 ×3 的启发式会低估
    /// 4 字节 emoji 文档，这里不给启发式留误差空间。
    pub fn text_len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// 行数（末尾有换行时，换行后的空行也算一行，与多数编辑器一致）。
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn insert(&mut self, char_idx: usize, text: &str) {
        self.rope.insert(char_idx, text);
    }

    /// 删除 `[start_char, end_char)` 区间。
    pub fn remove_range(&mut self, start_char: usize, end_char: usize) {
        self.rope.remove(start_char..end_char);
    }

    pub fn to_text(&self) -> String {
        self.rope.to_string()
    }

    /// 按底层存储块零拷贝迭代正文。流式查找（P10）与将来的分块保存
    /// （P19）都以此为基础设施——不产生全文 String，内存峰值与文档大小无关。
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// 取第 `line` 行（0 起）的文本——将来虚拟化渲染按行取数的雏形：
    /// 只碰可见行，成本与全文大小无关。
    pub fn line_str(&self, line_idx: usize) -> String {
        self.rope.line(line_idx).to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    // ---------- 行列 <-> 字符偏移换算（自绘编辑器的定位基础） ----------

    /// 第 `line_idx` 行首字符的全文字符偏移。
    pub fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx)
    }

    /// 全文字符偏移所在的行号。
    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx)
    }

    /// 取 `[start, end)` 字符偏移区间的文本（选区读取用）。
    pub fn slice_text(&self, start_char: usize, end_char: usize) -> String {
        self.rope.slice(start_char..end_char).to_string()
    }

    /// 第 `line_idx` 行的字符数（含行尾换行符，若有）。
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        self.rope.line(line_idx).len_chars()
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_remove_keep_line_index_consistent() {
        let mut doc = Document::from_str("hello\nworld\n");
        assert_eq!(doc.line_count(), 3);
        assert_eq!(doc.line_str(1), "world\n");

        // 在第一行中间插入带换行的文本
        doc.insert(5, ",\neditpad");
        assert_eq!(doc.line_count(), 4);
        assert_eq!(doc.line_str(0), "hello,\n");
        assert_eq!(doc.line_str(1), "editpad\n");

        doc.remove_range(0, 6);
        assert_eq!(doc.to_text(), "\neditpad\nworld\n");
        assert_eq!(doc.line_count(), 4);
    }

    #[test]
    fn empty_document_basics() {
        let mut doc = Document::new();
        assert!(doc.is_empty());
        assert_eq!(doc.text_len(), 0);
        assert_eq!(doc.line_count(), 1);

        doc.insert(0, "中文测试");
        assert_eq!(doc.text_len(), 4);
        assert_eq!(doc.line_str(0), "中文测试");
    }

    #[test]
    fn line_char_offset_conversion_roundtrip() {
        let doc = Document::from_str("ab\ncd中文\n");
        assert_eq!(doc.line_count(), 3);

        assert_eq!(doc.line_to_char(0), 0);
        assert_eq!(doc.line_to_char(1), 3);
        // "ab\n"(3) + "cd中文"(4) = 7，第 2 行（末尾空行）从偏移 8 开始
        assert_eq!(doc.line_to_char(2), 8);

        assert_eq!(doc.char_to_line(0), 0);
        assert_eq!(doc.char_to_line(2), 0); // 换行符属于所在行的末尾
        assert_eq!(doc.char_to_line(3), 1);
        assert_eq!(doc.char_to_line(7), 1); // 第 1 行的换行符
        assert_eq!(doc.char_to_line(8), 2); // 文档长度处 = 末尾空行

        // 行长含换行符
        assert_eq!(doc.line_len_chars(0), 3);
        assert_eq!(doc.line_len_chars(2), 0);
    }

    #[test]
    fn clone_is_independent_snapshot() {
        let doc = Document::from_str("abc");
        let snapshot = doc.clone();
        // 将来撤销栈就靠这种廉价快照实现
        let _ = snapshot;
    }

    // ---------- P9 主导行尾 ----------

    #[test]
    fn line_ending_detection_picks_dominant_style() {
        use LineEnding::{CrLf, Cr, Lf};
        // 空文本 / 无换行文本 → 保守默认 LF
        assert_eq!(LineEnding::detect(""), Lf);
        assert_eq!(LineEnding::detect("no newline at all"), Lf);

        assert_eq!(LineEnding::detect("a\nb\nc"), Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\nc"), CrLf);
        assert_eq!(LineEnding::detect("a\rb\rc"), Cr);

        // 混合行尾：多者胜
        assert_eq!(LineEnding::detect("a\r\nb\r\nc\nd"), CrLf, "CRLF 多于 LF 应判 CRLF");
        assert_eq!(LineEnding::detect("a\nb\nc\r\nd"), Lf, "LF 多于 CRLF 应判 LF");

        // 三者平票 → 回退 LF
        assert_eq!(LineEnding::detect("a\r\nb\nc\rd"), Lf);
    }

    #[test]
    fn normalize_rewrites_every_eol_to_target() {
        let mixed = "a\r\nb\nc\rd";
        assert_eq!(LineEnding::Lf.normalize(mixed), "a\nb\nc\nd");
        assert_eq!(LineEnding::CrLf.normalize(mixed), "a\r\nb\r\nc\r\nd");
        assert_eq!(LineEnding::Cr.normalize(mixed), "a\rb\rc\rd");

        // \r\n 必须整体消费：不得膨胀成两个换行
        assert_eq!(LineEnding::Lf.normalize("x\r\ny"), "x\ny");
        assert_eq!(LineEnding::CrLf.normalize("x\ny"), "x\r\ny");

        // 无换行内容（含 4 字节 emoji）原样保留
        assert_eq!(LineEnding::CrLf.normalize("中文🚀"), "中文🚀");
        assert_eq!(LineEnding::Lf.normalize(""), "");
    }

    #[test]
    fn from_str_detects_and_preserves_crlf_content() {
        let doc = Document::from_str("first\r\nsecond\r\n");
        assert_eq!(doc.line_ending(), LineEnding::CrLf);
        // 内容字节原样保留——保存路径往返一致，不悄悄改写用户的行尾
        assert_eq!(doc.to_text(), "first\r\nsecond\r\n");

        assert_eq!(Document::from_str("a\nb\n").line_ending(), LineEnding::Lf);
        assert_eq!(Document::new().line_ending(), LineEnding::Lf, "空文档默认 LF");

        // 克隆快照（撤销栈的基础）必须带着同一行尾元数据
        let snapshot = doc.clone();
        assert_eq!(snapshot.line_ending(), LineEnding::CrLf);
    }
}
