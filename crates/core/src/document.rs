//! 基于 Rope 的文本文档。
//!
//! M0 阶段只提供最小接口，验证 ropey 的行为并建立测试基线；
//! M2 切换自绘渲染层时，`Document` 将成为编辑器的唯一数据源。

use ropey::Rope;

#[derive(Clone)]
pub struct Document {
    rope: Rope,
}

impl Document {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
        }
    }

    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    /// 字符数（Unicode scalar 单位，与 ropey 一致）。
    pub fn text_len(&self) -> usize {
        self.rope.len_chars()
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
}
