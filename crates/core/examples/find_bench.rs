//! P10 计时对比：查找旧路径 vs 新路径（务必在 release 构建下运行）。
//!
//! * 旧路径：`doc.to_text()`（全量拷贝出一份 String）+ `find_all`
//!   ——即修复前 UI 线程每敲一个键都要付出的代价；
//! * 新路径：`find_all_document` 直接在 rope 上按存储块扫描，零全文拷贝
//!   ——修复后 UI 线程只做 O(log n) 的 rope 结构共享克隆。
//!
//! 用法：
//! ```text
//! cargo run --release -p editpad-core --example find_bench
//! cargo run --release -p editpad-core --example find_bench <大文件路径>
//! ```
//! 每条路径各计 5 轮取中位数；两路径命中数必须一致，否则 panic。

use std::time::{Duration, Instant};

use editpad_core::{find_all, find_all_document, load_file, Document};

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

    let query = "the";
    println!("== {label}：{} 字符，查询 {query:?}，5 轮取中位数 ==", doc.text_len());

    let mut old_runs: Vec<Duration> = Vec::new();
    let mut new_runs: Vec<Duration> = Vec::new();
    let mut copy_runs: Vec<Duration> = Vec::new();

    for round in 1..=5 {
        // 旧路径：to_text 全量拷贝 + find_all
        let t = Instant::now();
        let text = doc.to_text();
        let copy = t.elapsed();
        let found = find_all(&text, query, true);
        let old = t.elapsed();

        // 新路径：rope 分块直查
        let t = Instant::now();
        let found_new = find_all_document(&doc, query, true);
        let new = t.elapsed();

        assert_eq!(found.len(), found_new.len(), "两条路径命中数不一致");
        println!(
            "第{round}轮  旧路径 {:8.1} ms（其中 to_text 拷贝 {:8.1} ms）   新路径 {:8.1} ms   命中 {}",
            ms(old), ms(copy), ms(new), found.len()
        );
        old_runs.push(old);
        copy_runs.push(copy);
        new_runs.push(new);
    }

    println!(
        "\n中位数    旧路径 {:8.1} ms（拷贝 {:8.1} ms）        新路径 {:8.1} ms",
        ms(median(old_runs)), ms(median(copy_runs)), ms(median(new_runs))
    );
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

/// 确定性合成日志样文：约 45MB、50 万行，每行含两个 "the"。
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
