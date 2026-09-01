//! 健壮性边界输入批（P49，第 39 轮建议 1 落地 · core 层）。
//!
//! 钉住契约：空文档 / 单行无换行 / 超长行 / 4 字节 emoji / CRLF 混合 /
//! 非法 UTF-8 / 空文件 / 重复加载同一文件 / 保存往返——全程不 panic、
//! 度量（字符/字节/行数）一致、行尾与编码判定符合既有语义。
//! app 层的「超大行号跳转 / 重复打开 / 加载中关窗」见 main.rs 对应测试。

use editpad_core::{
    find_all_document, load_document_streaming, replace_all_document, CoreError, Document,
    LineEnding,
};
use std::path::PathBuf;

/// 测试专用临时目录（进程级唯一子目录，测试尾部自行清理）。
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "editpad-boundary-{}-{}",
        std::process::id(),
        tag
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

// ---------- 空文档 ----------

#[test]
fn empty_document_metrics_and_edits() {
    let doc = Document::from_str("");
    assert!(doc.is_empty());
    assert_eq!(doc.text_len(), 0);
    assert_eq!(doc.text_len_bytes(), 0);
    // ropey 语义：空文本也有 1 行（与多数编辑器一致，光标停在 0,0）
    assert_eq!(doc.line_count(), 1);
    assert_eq!(doc.line_str(0), "");
    assert_eq!(doc.line_ending(), LineEnding::Lf);

    // 空文档上插入 / 删空 / 查找 / 替换都不 panic 且结果正确
    let mut doc = doc;
    doc.insert(0, "first");
    assert_eq!(doc.to_text(), "first");
    doc.remove_range(0, 5);
    assert!(doc.is_empty());
    assert!(find_all_document(&doc, "x", true).is_empty());
    let (text, n) = replace_all_document(&doc, "x", "y", true);
    assert_eq!(n, 0);
    assert_eq!(text, "");
    assert!(doc.is_empty());

    // 两个空文档视为同一内容状态（P38 content_eq 契约）
    assert!(Document::from_str("").content_eq(&Document::from_str("")));
}

// ---------- 单行（无换行） ----------

#[test]
fn single_line_without_trailing_newline() {
    let mut doc = Document::from_str("hello");
    assert_eq!(doc.line_count(), 1);
    assert_eq!(doc.line_ending(), LineEnding::Lf, "无换行 → 默认 LF");
    assert_eq!(doc.line_str(0), "hello");
    assert_eq!(doc.line_to_char(0), 0);
    assert_eq!(doc.char_to_line(4), 0);

    // 追加换行 + 第二行：行数与定位跟随（eol 元数据保持建文档时的判定）
    doc.insert(5, "\nworld");
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_to_char(1), 6);
    assert_eq!(doc.char_to_line(8), 1);
    assert_eq!(doc.line_ending(), LineEnding::Lf);
}

// ---------- 超长行 ----------

#[test]
fn very_long_line_operations_and_metrics() {
    // 20 万字符单行（P13 超宽行场景的最小核心等价物；ropey 操作 O(log n)）
    let long = format!("{}end", "x".repeat(200_000));
    let mut doc = Document::from_str(&long);
    assert_eq!(doc.line_count(), 1);
    assert_eq!(doc.text_len(), 200_003);
    assert_eq!(doc.line_to_char(0), 0);
    assert_eq!(doc.char_to_line(0), 0);
    assert_eq!(doc.char_to_line(199_999), 0);
    assert_eq!(doc.char_to_line(200_002), 0, "整行都在第 0 行");

    // 行中插入不改变行数；删除区间后长度守恒
    doc.insert(100_000, "!");
    assert_eq!(doc.line_count(), 1);
    assert_eq!(doc.text_len(), 200_004);
    doc.remove_range(100_000, 100_001);
    assert_eq!(doc.to_text(), long);

    // 超长行内查找：命中列号即字符列
    let hits = find_all_document(&doc, "end", true);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 0);
    assert_eq!(hits[0].col, 200_000);
    assert_eq!(hits[0].len_chars, 3);
}

// ---------- 4 字节 emoji ----------

#[test]
fn four_byte_emoji_char_byte_and_search_boundaries() {
    // 😀 = U+1F600，4 字节。字符/字节两套口径必须分开算对（P31 字节节流的根基）
    let text = "a\u{1F600}b\nc\u{1F600}";
    let doc = Document::from_str(text);
    assert_eq!(doc.text_len(), 6, "Unicode scalar 口径：emoji 记 1");
    assert_eq!(doc.text_len_bytes(), 1 + 4 + 1 + 1 + 1 + 4, "UTF-8 字节口径");
    assert_eq!(doc.line_count(), 2);
    assert_eq!(doc.line_str(0), "a\u{1F600}b\n");
    assert_eq!(doc.line_str(1), "c\u{1F600}");
    assert_eq!(doc.line_to_char(1), 4);
    assert_eq!(doc.char_to_line(5), 1, "第二个 emoji 在第 1 行");

    // 删除区间恰好覆盖一个 emoji（按字符下标，不产生半字符）
    let mut doc2 = Document::from_str(text);
    doc2.remove_range(1, 3); // 删 "😀b"
    assert_eq!(doc2.to_text(), "a\nc\u{1F600}");

    // emoji 作为查询串：命中列号按字符计
    let hits = find_all_document(&doc, "\u{1F600}", true);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].line, 0);
    assert_eq!(hits[0].col, 1);
    assert_eq!(hits[1].line, 1);
    assert_eq!(hits[1].col, 1);
    let (out, n) =
        replace_all_document(&Document::from_str(text), "\u{1F600}", "*", true);
    assert_eq!(n, 2);
    assert_eq!(out, "a*b\nc*");
}

// ---------- CRLF 混合 ----------

#[test]
fn mixed_line_endings_detect_dominant() {
    // CRLF 多数 → CrLf
    assert_eq!(
        Document::from_str("a\r\nb\nc\r\n").line_ending(),
        LineEnding::CrLf
    );
    // LF 多数 → Lf
    assert_eq!(
        Document::from_str("a\rb\nc\n").line_ending(),
        LineEnding::Lf
    );
    // 平票 → Lf（严格多数判定，无多数即默认）
    assert_eq!(
        Document::from_str("a\r\nb\n").line_ending(),
        LineEnding::Lf
    );
    // 块尾悬置 \r（P19 跨块统计语义）：crlf 与 cr 平票、lf 为 0 → cr 胜出
    assert_eq!(Document::from_str("a\r\nb\r").line_ending(), LineEnding::Cr);
    // 孤立 \r（旧 Mac 行尾）多数 → Cr
    assert_eq!(Document::from_str("a\rb\rc\r").line_ending(), LineEnding::Cr);

    // 混合行尾文档的行数与逐行取数（ropey 按 \n / \r\n / \r 都断行；
    // len_lines = 换行数 + 1，末尾换行后的空行计入——Document 契约）
    let doc = Document::from_str("a\r\nb\nc\r\n");
    assert_eq!(doc.line_count(), 4);
    assert_eq!(doc.line_str(0), "a\r\n");
    assert_eq!(doc.line_str(1), "b\n");
    assert_eq!(doc.line_str(2), "c\r\n");
    assert_eq!(doc.line_str(3), "");
}

// ---------- 非法 UTF-8 / 空文件 / 二进制拒绝 ----------

#[test]
fn invalid_utf8_falls_back_to_gbk_and_pins_replacement() {
    let dir = scratch("utf8");

    // ① 合法 GBK 字节（「中文」= D6D0 CEC4）按 GBK 正确解码
    let gbk_path = dir.join("gbk.txt");
    std::fs::write(&gbk_path, [0xD6, 0xD0, 0xCE, 0xC4]).unwrap();
    let loaded = load_document_streaming(&gbk_path, |_| {}).unwrap();
    assert_eq!(loaded.encoding, "GBK");
    assert_eq!(loaded.doc.to_text(), "中文");

    // ② 非法 UTF-8（0xFF）短文件：替换率 1/13 远超二进制阈值（1%）→
    //    拒开而非 panic（与 NUL 同一保护伞，现有契约）
    let bad_path = dir.join("bad.txt");
    std::fs::write(&bad_path, b"hello \xFF world").unwrap();
    match load_document_streaming(&bad_path, |_| {}) {
        Err(CoreError::BinaryDetected { .. }) => {}
        other => panic!("高替换率文件必须按二进制拒开，实际 {other:?}"),
    }

    // ③ 长文里混入单个非法字节（1/300 < 1% 阈值）：不 panic，走 GBK
    //    解码降级，替换字符入文
    let mut long_bad = vec![b'a'; 300];
    long_bad[150] = 0xFF;
    let long_path = dir.join("long_bad.txt");
    std::fs::write(&long_path, &long_bad).unwrap();
    let loaded = load_document_streaming(&long_path, |_| {}).unwrap();
    assert_eq!(loaded.encoding, "GBK");
    assert!(
        loaded.doc.to_text().contains('\u{FFFD}'),
        "非法序列应解码为替换字符"
    );

    // ④ 含 NUL → 拒开（二进制检测），返回 BinaryDetected 而非 panic
    let nul_path = dir.join("nul.bin");
    std::fs::write(&nul_path, b"abc\0def").unwrap();
    match load_document_streaming(&nul_path, |_| {}) {
        Err(CoreError::BinaryDetected { .. }) => {}
        other => panic!("NUL 文件必须按二进制拒开，实际 {other:?}"),
    }

    // ⑤ 空文件：合法加载为空文档（扫描遍对空输入平凡成立）
    let empty_path = dir.join("empty.txt");
    std::fs::write(&empty_path, b"").unwrap();
    let loaded = load_document_streaming(&empty_path, |_| {}).unwrap();
    assert!(loaded.doc.is_empty());
    assert_eq!(loaded.encoding, "UTF-8");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------- 重复加载 / 保存往返 ----------

#[test]
fn reload_same_file_yields_independent_documents() {
    let dir = scratch("reload");
    let path = dir.join("same.txt");
    std::fs::write(&path, "same-content\r\nsecond\r\n").unwrap();

    let a = load_document_streaming(&path, |_| {}).unwrap();
    let b = load_document_streaming(&path, |_| {}).unwrap();
    assert_eq!(a.encoding, "UTF-8");
    assert!(a.doc.content_eq(&b.doc), "同文件两次加载内容一致");
    assert_eq!(a.doc.line_ending(), LineEnding::CrLf);

    // 独立性：改 a 不影响 b（重复打开不共享可变状态）
    let mut a = a.doc;
    a.insert(0, "edited-");
    assert!(!a.content_eq(&b.doc));
    assert_eq!(b.doc.to_text(), "same-content\r\nsecond\r\n");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn save_roundtrip_preserves_content_and_dominant_eol() {
    let dir = scratch("roundtrip");
    let path = dir.join("roundtrip.txt");

    // CRLF 主导文档：保存 → 重载，内容与主导行尾都不转码（P9 契约）
    let doc = Document::from_str("第一行\r\nsecond 😀 line\r\n");
    editpad_core::save_document_atomic(&path, &doc).unwrap();
    let reloaded = load_document_streaming(&path, |_| {}).unwrap();
    assert!(reloaded.doc.content_eq(&doc));
    assert_eq!(reloaded.doc.line_ending(), LineEnding::CrLf);
    assert_eq!(reloaded.encoding, "UTF-8");

    std::fs::remove_dir_all(&dir).ok();
}
