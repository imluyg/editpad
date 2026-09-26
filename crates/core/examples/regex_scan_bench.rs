//! L-17 一次性仪表：正则档扫描的**阶段分解**与跨度换算两档的**交叉点**。
//!
//! 两个构建档都要跑（定案 R-1：只报一个档的 ms 按无效处理）：
//! ```text
//! cargo run --release -p editpad-core --example regex_scan_bench
//! QUICK=1 cargo run -p editpad-core --example regex_scan_bench   # 只跑交叉点两侧
//! ```
//! 末位参数给 `many`／`one` 可只跑一种文档形态。
//!
//! 每格三个数都取自同一次运行：
//! * `旧`＝`to_text()` + `find_all_regex`（改前：换算无论几条命中都要把正文再走一遍）；
//! * `新`＝`find_all_regex_document`（改后：换算按密度在「游标走过」与「rope 树查表」
//!   之间择一）；两档结果逐格必须相同，否则 panic；
//! * `拷贝`＝单独计一次 `to_text()`，看「整份物化」占这格账多少。
//!
//! 夹具把命中密度做成唯一旋钮 `间隔字节`：正文由「gap-1 个填充字符 + 一颗 `K`」
//! 重复铺成（多行档额外每 65 字符断一行），所以命中数＝文档长度/gap，两档的
//! 成本差只由这一个维度决定。

use std::time::{Duration, Instant};

use editpad_core::{find_all_regex, find_all_regex_document, Document};

const TOTAL: usize = 49_000_000;

fn main() {
    let only = std::env::args().nth(1);
    // 交叉点两侧那两格（`QUICK=1`）——debug 档跑全表要十几分钟
    let quick = std::env::var("QUICK").is_ok();
    let shapes: Vec<(&str, bool)> = match only.as_deref() {
        Some("one") => vec![("49MB·单行", false)],
        Some("many") => vec![("49MB·多行", true)],
        _ => vec![("49MB·多行", true), ("49MB·单行", false)],
    };
    for (label, multiline) in shapes {
        println!("\n== {label} ==");
        println!(
            "  {:>10} {:>10} {:>9} {:>9} {:>9} {:>7}",
            "间隔字节", "命中数", "旧 ms", "新 ms", "拷贝 ms", "旧/新"
        );
        let all_gaps = [
            1_000_000usize,
            100_000,
            10_000,
            2_000,
            500,
            200,
            100,
            65,
            20,
        ];
        let gaps: &[usize] = if quick { &[1_000_000, 200] } else { &all_gaps };
        let rounds = if quick { 1 } else { 3 };
        for &gap in gaps {
            let text = build(gap, multiline);
            let doc = Document::from_str(&text);
            let (mut old, mut new, mut copy) = (Vec::new(), Vec::new(), Vec::new());
            let mut n = 0usize;
            for _ in 0..rounds {
                let t = Instant::now();
                let a = find_all_regex(&text, "K", true).expect("合法模式");
                old.push(t.elapsed());
                let t = Instant::now();
                let b = find_all_regex_document(&doc, "K", true).expect("合法模式");
                new.push(t.elapsed());
                assert_eq!(a, b, "两档结果分叉（gap={gap}）");
                n = a.len();
                let t = Instant::now();
                std::hint::black_box(doc.to_text());
                copy.push(t.elapsed());
            }
            let (o, w, c) = (ms(median(old)), ms(median(new)), ms(median(copy)));
            println!(
                "  {gap:>10} {n:>10} {:>9.1} {:>9.1} {:>9.1} {:>6.2}×",
                o,
                w,
                c,
                o / w.max(0.01)
            );
        }
    }
}

/// 「gap-1 个填充 + 一颗 K」铺到 `TOTAL` 字节；`multiline` 时每 65 字符断一行。
fn build(gap: usize, multiline: bool) -> String {
    let mut out = String::with_capacity(TOTAL);
    let mut since_break = 0usize;
    while out.len() < TOTAL {
        for _ in 0..gap - 1 {
            out.push('x');
            since_break += 1;
            if multiline && since_break == 65 {
                out.push('\n');
                since_break = 0;
            }
        }
        out.push('K');
        since_break += 1;
    }
    out
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}
