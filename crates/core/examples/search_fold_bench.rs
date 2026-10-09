//! M10b 性能对拍（第 306 轮）：把字面折叠统一到 Unicode 之后，O-11／O-13 那两组
//! 量出来的收益点还在不在。
//!
//! 跑法：`cargo run --release -p editpad-core --example search_fold_bench`
//!
//! 三张正文各代表一个形状：
//! * `ascii` —— 日志型正文（纯 ASCII）：快路径原本要救的就是这种文件；
//! * `mixed` —— 每 50 行里有一行非 ASCII（中文注释／作者名）：**本轮新加的分派**在这里
//!   逐行改档，成本变化最灵敏；
//! * `cjk`   —— 整片非 ASCII：快路径本来就用不上，是"统一口径"付的底价。
//!
//! ⚠️ 对照臂＝把 [`LiteralQuery::byte_scan_ok`] 临时写成 `|_| true`（＝改前的形状：
//! 不敏感也一律走字节窗口）复跑同一份例子。两遍数字一起进台账，见
//! `docs/days/` 与 `docs/决策记录.md` R-21 改判那节。
//!
//! 计时口径：每项取 N 次的**中位数**（单次读数在本机不被接受，定案 R-1 那一族）；
//! 每项开始前先往 stderr 打一行标记——哪一项卡住一眼可见（第 306 轮就是这么抓到
//! 第一版字符档的 O(正文 × 命中数) 退化的）。

use std::time::Instant;

fn build_lines(count: usize, mut make: impl FnMut(usize) -> String) -> String {
    let mut s = String::with_capacity(count * 48);
    for i in 0..count {
        s.push_str(&make(i));
        s.push('\n');
    }
    s
}

fn time_it(runs: usize, label: &str, mut f: impl FnMut() -> usize) {
    eprintln!("→ {label}");
    let mut samples = Vec::with_capacity(runs);
    let mut hits = 0usize;
    for _ in 0..runs {
        let t = Instant::now();
        hits = f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    println!(
        "{label:46} {:9.1} ms  (中位/{})  产出 {hits}",
        samples[samples.len() / 2],
        runs
    );
}

fn main() {
    let lines = 200_000usize;
    let runs = 3usize;
    let log = |i: usize| {
        format!("2026-10-09 12:00:{i:02} INFO worker/{i} processed batch {i} items in {i} ms")
    };
    let zh = |i: usize| format!("2026-10-09 12:00:{i:02} 处理完成 批次 {i} 条，用时 {i} 毫秒");
    let ascii = build_lines(lines, log);
    let mixed = build_lines(lines, |i| if i % 50 == 0 { zh(i) } else { log(i) });
    let cjk = build_lines(lines, zh);
    println!(
        "正文规模：ascii {} MB ／ mixed {} MB ／ cjk {} MB（{} 行）",
        ascii.len() / (1 << 20),
        mixed.len() / (1 << 20),
        cjk.len() / (1 << 20),
        lines
    );

    // 1) 查找：打不中＝整篇扫一遍（纯扫描成本）；密集命中＝每行一处（放大两条档的差距）
    for (name, text, dense) in [
        ("ascii", &ascii, "batch"),
        ("mixed", &mixed, "batch"),
        ("cjk", &cjk, "批次"),
    ] {
        time_it(runs, &format!("find_all 不命中 {name}"), || {
            editpad_core::find_all(text, "ZZZZZZZZ", false).len()
        });
        time_it(runs, &format!("find_all 密集 {name}"), || {
            editpad_core::find_all(text, dense, false).len()
        });
        time_it(runs, &format!("find_all 敏感档 {name}"), || {
            editpad_core::find_all(text, dense, true).len()
        });
    }

    // 2) O-13 封顶档：取满即收工。limit=1 在一命中就到位＝早退没被分派吃掉
    let doc = editpad_core::Document::from_str(&ascii);
    for limit in [1usize, 100, 10_000] {
        time_it(runs, &format!("document 封顶 limit={limit}"), || {
            editpad_core::find_all_document_limited(&doc, "batch", false, limit).len()
        });
    }
    let cjkdoc = editpad_core::Document::from_str(&cjk);
    time_it(runs, "document 封顶 cjk limit=100", || {
        editpad_core::find_all_document_limited(&cjkdoc, "批次", false, 100).len()
    });

    // 3) 替换家族：四入口共用同一个判据，成本也必须共用这一份分派
    time_it(runs, "replace_all ascii", || {
        editpad_core::replace_all(&ascii, "batch", "X", false).1
    });
    time_it(runs, "replace_all_word ascii", || {
        editpad_core::replace_all_word(&ascii, "batch", "X", false).1
    });
    time_it(runs, "replace_all_document ascii", || {
        editpad_core::replace_all_document(&doc, "batch", "X", false).1
    });
    time_it(runs, "replace_all cjk", || {
        editpad_core::replace_all(&cjk, "批次", "X", false).1
    });
    time_it(runs, "replace_all_document cjk", || {
        editpad_core::replace_all_document(&cjkdoc, "批次", "X", false).1
    });

    // 4) 跨行查询：本来就走字符档（MultiLineScanner），这条是"没被本轮拖慢"的对照
    time_it(2, "find_all 跨行 ascii", || {
        editpad_core::find_all(&ascii, "worker\n2", false).len()
    });
}
