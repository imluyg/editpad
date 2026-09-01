//! P12 计时对比：Ctrl+End 大跳转触发的检查点补建冻结（务必 release 构建）。
//!
//! 冻结机理：跳到文档尾部时，`styled_line(末行)` 的补检查点循环要一次性
//! parse 沿途全部行（约 行数/STRIDE 个档位 × STRIDE 行），全部同步在
//! UI 线程发生。本样例在无 GUI 环境复现同一代码路径并计时。
//!
//! 用法：
//! ```text
//! cargo run --release -p editpad-core --example highlight_bench [行数]
//! ```

use std::time::{Duration, Instant};

use editpad_core::{Document, LazyHighlighter};

fn main() {
    let lines: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300_000);

    let doc = synthetic_rust_doc(lines);
    let mut hl = LazyHighlighter::new("rs").expect("rust 语法存在");
    println!("== 合成 Rust 文档 {lines} 行（{} 字符），STRIDE=128 ==", doc.text_len());

    // 单项成本：Highlighter 构造（styled_line 每次调用都会重建一个）
    let t = Instant::now();
    let mut constructions = 0;
    while t.elapsed() < Duration::from_millis(200) {
        let _ = highlight_probe();
        constructions += 1;
    }
    println!(
        "Highlighter::new(theme) 单次      {:8.3} ms（{} 次取均值）",
        ms(t.elapsed()) / constructions as f64,
        constructions
    );

    // Ctrl+End 冻结：冷启动直接请求末行（触发全程补建）
    let last = doc.line_count() - 1;
    let total = doc.line_count();
    let tail_text = doc.line_str(last);
    let t = Instant::now();
    let runs = hl.styled_line(
        last,
        tail_text.trim_end_matches(['\n', '\r']),
        total,
        &mut |i| doc.line_str(i),
    );
    let freeze = t.elapsed();
    println!(
        "冷启动 styled_line(末行)          {:8.1} ms ← Ctrl+End 冻结量级，{} 段",
        ms(freeze),
        runs.len()
    );
    println!(
        "  折算每行补建成本                 {:8.4} ms/行",
        ms(freeze) / last as f64
    );

    // 热态随机访问（检查点齐备后的日常渲染成本）
    let mut warm_total = Duration::ZERO;
    let samples = 50;
    for k in 0..samples {
        let line = (k * 6_005 + 7) % last; // 散开的伪随机行号
        let text = doc.line_str(line);
        let t = Instant::now();
        let _ = hl.styled_line(
            line,
            text.trim_end_matches(['\n', '\r']),
            total,
            &mut |i| doc.line_str(i),
        );
        warm_total += t.elapsed();
    }
    println!(
        "热态 styled_line（{samples} 次均值）      {:8.3} ms/次",
        ms(warm_total) / samples as f64
    );
}

/// 直接探测 Highlighter 构造成本（主题走本样例的 'static 缓存，
/// 与 highlight.rs 内部同一路径，构造开销不计入主题加载）。
fn highlight_probe() -> syntect::highlighting::Highlighter<'static> {
    fn theme() -> &'static syntect::highlighting::Theme {
        static THEME: std::sync::OnceLock<syntect::highlighting::Theme> = std::sync::OnceLock::new();
        THEME.get_or_init(|| {
            syntect::highlighting::ThemeSet::load_defaults()
                .themes
                .remove("InspiredGitHub")
                .expect("内置主题存在")
        })
    }
    syntect::highlighting::Highlighter::new(theme())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 确定性合成 Rust 源码文档：多种行型轮转，让语法状态机有真实工作可做。
fn synthetic_rust_doc(lines: usize) -> Document {
    let mut text = String::with_capacity(lines * 48);
    for i in 0..lines {
        match i % 10 {
            0 => text.push_str("pub struct Item {\n"),
            1 => text.push_str("    pub name: String,\n"),
            2 => text.push_str("    pub weight: f64, // 千克\n"),
            3 => text.push_str("}\n"),
            4 => text.push_str("fn compute(total: f64, factor: f64) -> f64 {\n"),
            5 => text.push_str("    let scaled = total * factor;\n"),
            6 => text.push_str("    if scaled > 1.0 { return scaled.sqrt(); }\n"),
            7 => text.push_str("    /* 块注释 */ format!(\"item-{scaled}\")\n"),
            8 => text.push_str("}\n"),
            _ => text.push_str("// ---- 分隔线 ----\n"),
        }
    }
    Document::from_str(&text)
}
