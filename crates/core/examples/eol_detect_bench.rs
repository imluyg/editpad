//! 一次性仪表（第 212 轮 L-20）：开档路径里"逐字符统计行尾"这一程值多少。
//!
//! ```text
//! cargo run --release -p editpad-core --example eol_detect_bench
//! QUICK=1 cargo run -p editpad-core --example eol_detect_bench
//! ```
//!
//! 三个数：整份 `LineEnding::detect`（与 `EolCounter::push` 同一形状的逐字符
//! 循环）、同样的统计改成**按字节**走的原型、以及真实开档
//! `load_document_streaming` 的全程（分母）。
//!
//! **第 212 轮的结论＝量否，不改生产代码**：把逐字符统计换成按字节统计在
//! release 档只有 **1.1~1.3×**（56MB 样本 42.5→36.1 ms），而 debug 档是 6.9×
//! ——按定案 R-1，只有 debug 一侧成立的收益对用户无效，故**不做**（LLVM 在
//! release 下已把 ASCII 的 UTF-8 解码循环优化得很好）。
//! 留下的两个基线数（本机实测，回归可对照）：
//! * 开档 71MB ⇒ release **208~214 ms ≈ 320 MB/s**；debug 2.5~2.75 s ≈ 26 MB/s。
//! * 行尾统计这一程 ≈0.75 ms/MB ⇒ 占 release 开档约 **两成**；要再降只能改成
//!   抽样，而"按全文统计主导行尾"是写明的契约（改了会换掉混合行尾文件的判定）
//!   ＝**行为决策，等用户点单**，不是机械活。
//!
//! ⚠️ 顺带核过并否掉的另一条：开档**没有**多遍整份扫描可以合——UTF-8 路径的
//! NUL 扫描、解码、rope 构建、行尾统计、首块采样已在同一个分块循环里一次做完
//! （`scan_and_build_utf8` 调 `absorb(..)`），与 P298 在 UI 侧清掉的那两处不同。

use std::time::Instant;

use editpad_core::{load_document_streaming, LineEnding};

fn main() {
    let quick = std::env::var("QUICK").is_ok();
    let base = "2026-08-24 12:00:00 INFO the quick brown fox request_id=";
    let lines = if quick { 90_000 } else { 900_000 };
    let mut text = String::with_capacity(base.len() * lines);
    for i in 0..lines {
        text.push_str(base);
        text.push_str(&i.to_string());
        text.push_str(if i % 7 == 0 { "\r\n" } else { "\n" });
    }
    println!("样本：{} 字节 / {} 行", text.len(), lines);
    for f in equivalence_fixtures() {
        assert_eq!(
            LineEnding::detect(&f),
            detect_bytes(&f),
            "夹具 {f:?} 上两版分叉"
        );
    }

    for round in 0..3 {
        let t = Instant::now();
        let a = LineEnding::detect(&text);
        let chars_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let b = detect_bytes(&text);
        let bytes_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(a, b, "按字节统计必须与逐字符统计同判据");
        println!(
            "第{round}轮  逐字符 {chars_ms:8.1} ms   按字节 {bytes_ms:8.1} ms   {:.1}×",
            chars_ms / bytes_ms.max(0.01)
        );
    }

    let path = std::path::Path::new("dev-assets/bench-50mb.log");
    if path.exists() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        for round in 0..3 {
            let t = Instant::now();
            let loaded = load_document_streaming(path, |_| {}).expect("开档应成功");
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let doc = loaded;
            println!(
                "第{round}轮  真实开档 {ms:8.1} ms（{} 字节 ⇒ {:.0} MB/s）",
                size,
                size as f64 / 1024.0 / 1024.0 / (ms / 1000.0).max(0.001)
            );
            let _ = doc.doc.line_count();
        }
    } else {
        println!("（dev-assets/bench-50mb.log 不在，跳过真实开档那一格）");
    }
}

/// 与 `LineEnding::detect` 同判据的按字节版：UTF-8 里 `\n`/`\r` 这两个字节
/// 不可能出现在多字节序列内部（后续字节恒 ≥ 0x80），所以按字节数与按字符数
/// 在这三种计数上逐格相同。
fn detect_bytes(text: &str) -> LineEnding {
    let b = text.as_bytes();
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'\r' => {
                if i + 1 < b.len() && b[i + 1] == b'\n' {
                    crlf += 1;
                    i += 2;
                } else {
                    cr += 1;
                    i += 1;
                }
            }
            b'\n' => {
                lf += 1;
                i += 1;
            }
            _ => i += 1,
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

/// 等价性不只在那份大样本上断：CR-only／平票／无换行／多字节夹 ASCII 这些格子
/// 都得同判据，否则"按字节数"这句话就是眼力判断。
fn equivalence_fixtures() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        "\r",
        "\n",
        "\r\n",
        "a\rb\rc",
        "a\nb\nc",
        "a\r\nb\r\n",
        "a\r\nb\nc",
        "a\nb\r\nc",
        "中文\r\n文\rc\n",
        "\u{1F680}\r\nx",
        "no newline at all",
        "trailing\r",
        "lone \u{000B} vt\n",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // 平票各档（CRLF 与 LF 数量相等、CR 与 LF 相等）
    v.push("a\r\nb\nc\r\n".to_owned());
    v.push("a\rb\nc\rd\n".to_owned());
    v.push("a\rb\nc".to_owned());
    v
}
