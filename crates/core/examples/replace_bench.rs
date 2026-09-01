//! P11 计时对比：全部替换的三条路径（务必在 release 构建下运行）。
//!
//! * 旧路径：`doc.to_text()` 全量拷贝 + `Vec<char>` 收集后逐字符扫描
//!   ——修复前实现，4 字节/char，50MB 文档内存峰值 ≈ 原文 ×5；
//! * 新路径 A：`replace_all` —— UTF-8 字节两指针扫描（输入仍是 &str）；
//! * 新路径 B：`replace_all_document` —— rope 分块流式替换，零全文拷贝
//!   （app 层实际走这条）。
//!
//! 用法：
//! ```text
//! cargo run --release -p editpad-core --example replace_bench
//! cargo run --release -p editpad-core --example replace_bench <大文件路径>
//! ```
//! 每条路径各计 5 轮取中位数；三条路径输出必须完全一致，否则 panic。

use std::time::{Duration, Instant};

use editpad_core::{load_file, replace_all, replace_all_document, Document};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (doc, label) = match args.first() {
        Some(path) => {
            eprintln!("加载 {path} …");
            let t = Instant::now();
            let loaded =
                load_file(std::path::Path::new(path)).expect("加载失败");
            eprintln!("加载耗时 {:.1} ms，编码 {}", t.elapsed().as_secs_f64() * 1000.0, loaded.encoding);
            (Document::from_str(&loaded.text), format!("文件 {path}"))
        }
        None => (synthetic_doc(), "合成文档（约 45MB）".to_owned()),
    };

    // 高频命中 + 等长替换：隔离「扫描与搬运」本身的耗时，
    // 不让输出膨胀/收缩干扰对比（bench-50mb.log 是中英混排的
    // GBK 文本，每行一个 "abcdefg"，约 60 万处命中）
    let query = "abcdefg";
    let replacement = "ABCDEFG";
    println!(
        "== {label}：{} 字符，{query:?} → {replacement:?}，5 轮取中位数 ==",
        doc.text_len()
    );

    let mut legacy_runs: Vec<Duration> = Vec::new();
    let mut bytes_runs: Vec<Duration> = Vec::new();
    let mut rope_runs: Vec<Duration> = Vec::new();

    for round in 1..=5 {
        // 旧路径：to_text 全量拷贝 + Vec<char> 逐字符扫描（修复前实现）
        let t = Instant::now();
        let text = doc.to_text();
        let copy = t.elapsed();
        let (out_legacy, n_legacy) = replace_all_legacy(&text, query, replacement, true);
        let legacy = t.elapsed();

        // 新路径 A：字节级两指针（输入同为此处的 &str）
        let t = Instant::now();
        let (out_bytes, n_bytes) = replace_all(&text, query, replacement, true);
        let bytes = t.elapsed();

        // 新路径 B：rope 分块流式（app 层 ReplaceAll 的实际路径）
        let t = Instant::now();
        let (out_rope, n_rope) = replace_all_document(&doc, query, replacement, true);
        let rope = t.elapsed();

        assert_eq!(n_legacy, n_bytes, "新旧实现替换次数不一致");
        assert_eq!(n_legacy, n_rope, "rope 流式替换次数不一致");
        assert_eq!(out_legacy, out_bytes, "新旧实现输出文本不一致");
        assert_eq!(out_legacy, out_rope, "rope 流式输出文本不一致");

        println!(
            "第{round}轮  旧路径 {:8.1} ms（其中 to_text 拷贝 {:7.1} ms）   新A 字节 {:8.1} ms   新B rope流 {:8.1} ms   替换 {} 处",
            ms(legacy), ms(copy), ms(bytes), ms(rope), n_legacy
        );
        legacy_runs.push(legacy);
        bytes_runs.push(bytes);
        rope_runs.push(rope);
    }

    println!(
        "\n中位数    旧路径 {:8.1} ms        新A 字节 {:8.1} ms   新B rope流 {:8.1} ms",
        ms(median(legacy_runs)),
        ms(median(bytes_runs)),
        ms(median(rope_runs)),
    );
}

/// 修复前的实现原样保留作对照：`Vec<char>` 收集全文再逐字符窗口扫描。
/// 内存峰值 ≈ 原文 ×5（原文 + 4B/char 的 Vec + 输出 String）。
fn replace_all_legacy(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if query.is_empty() {
        return (text.to_owned(), 0);
    }
    let t: Vec<char> = text.chars().collect();
    let q: Vec<char> = query.chars().collect();
    if q.len() > t.len() {
        return (text.to_owned(), 0);
    }
    let fold = |a: char, b: char| {
        if case_sensitive {
            a == b
        } else {
            a.to_ascii_lowercase() == b.to_ascii_lowercase()
        }
    };
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut count = 0usize;
    while i <= t.len() - q.len() {
        if (0..q.len()).all(|k| fold(q[k], t[i + k])) {
            out.push_str(replacement);
            i += q.len();
            count += 1;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out.extend(t[i.min(t.len())..].iter());
    (out, count)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

/// 与 find_bench 同款合成日志样文：约 45MB、50 万行，每行含两个 "the"。
fn synthetic_doc() -> Document {
    let base = "2026-08-24 12:00:00 INFO the quick brown fox jumps over the lazy dog request_id=";
    let mut text = String::with_capacity(base.len() * 520_000);
    for i in 0..500_000 {
        text.push_str(base);
        text.push_str(&i.to_string());
        text.push('\n');
    }
    Document::from_str(&text)
}
