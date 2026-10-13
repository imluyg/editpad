//! 第 316 轮仪表：Markdown **整篇重解析**的墙钟规模（务必 release 构建）。
//!
//! 病因：预览开态下 `view()` 里缓存键（页 id＋版本＋字节数＋行数）一变就
//! `doc.to_text()` 拷全文 ＋ `parse_markdown` 全量重解，而 300 ms 换版节流
//! （`crates/app/src/md_preview.rs` 的 `md_preview_can_swap`）**只管 widget 树**，
//! 管不到这一步 ⇒ 左边连打 N 键＝整篇解析 N 遍。
//!
//! ⚠️ 本仪表只**记录规模**，不作判据：耗时读数在忙机器假红、温热假绿（本仓口径）。
//! 成本契约的看守是计数器——`MD_PARSE_COUNT`（app 侧：连打期间全篇重解析几次）与
//! `MD_TABLE_CELL_PARSES`／`MD_FENCE_BODY_PUSHES`／`MD_NOTE_SCANS`（core 侧：为答
//! 一个是非题多做了多少实解析），它们只在 `#[cfg(test)]` 下存在，所以这里看不到。
//!
//! 用法：
//! ```text
//! cargo run --release -p editpad-core --example md_parse_bench [重复次数]
//! ```

use std::time::{Duration, Instant};

/// 每个夹具的画像：名字、正文、以及"这格为什么要测"。
struct Case {
    name: &'static str,
    text: String,
    why: &'static str,
}

fn main() {
    let repeats: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let cases = cases();

    println!("== Markdown 整篇重解析规模（release，重复 {repeats} 次取最好一轮）==");
    println!(
        "{:<14}{:>10}{:>9}{:>10}{:>12}  为什么要测",
        "夹具", "字节", "块数", "最好 ms", "每 KB ms"
    );
    for case in &cases {
        // 先跑一遍预热（首轮带分配器与缓存冷启动，不代表打字路径）
        let blocks = editpad_core::markdown::parse_markdown(&case.text);
        let blocks = std::cell::Cell::new(blocks.len());
        let mut best = Duration::MAX;
        for _ in 0..repeats {
            let t = Instant::now();
            let parsed = editpad_core::markdown::parse_markdown(&case.text);
            let el = t.elapsed();
            blocks.set(parsed.len());
            if el < best {
                best = el;
            }
        }
        let ms = best.as_secs_f64() * 1000.0;
        let kb = case.text.len() as f64 / 1024.0;
        println!(
            "{:<14}{:>10}{:>9}{:>10.2}{:>12.3}  {}",
            case.name,
            case.text.len(),
            blocks.get(),
            ms,
            ms / kb,
            case.why
        );
    }
    println!(
        "\n参照：预览关态一帧 1.1～2.0 ms，预览开态的空闲复用帧 0.1～1.0 ms\
         \n（`docs/days/2026-10-12.md:246-251` 现量）。上面任何一个数落在**每次按键**上都是掉帧。"
    );

    // 另一把尺子：**换版帧**还要把整篇块表过一次内容指纹
    // （`crates/app/src/md_preview.rs` 的 `md_block_fingerprint`＝`block.hash()`）。
    // 这一份落在换版那一帧，与上面那份同阶；用它判断"指纹增量化"值不值得做。
    use std::hash::{Hash, Hasher};
    println!("\n== 换版帧的整篇指纹（`block.hash()` 全过一遍）==");
    println!("{:<14}{:>10}", "夹具", "指纹 ms");
    for case in &cases {
        let blocks = editpad_core::markdown::parse_markdown(&case.text);
        let mut best = Duration::MAX;
        for _ in 0..repeats {
            let t = Instant::now();
            for b in &blocks {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                b.hash(&mut h);
                std::hint::black_box(h.finish());
            }
            let el = t.elapsed();
            if el < best {
                best = el;
            }
        }
        println!("{:<14}{:>10.2}", case.name, best.as_secs_f64() * 1000.0);
    }

    // 第三把尺子：**一块解析缓存的槽**大约装着多少字节——按页多槽那份的内存代价
    // （`crates/app/src/state.rs` 的 `MdPreviewCache`，槽数＝`MD_PREVIEW_CACHE_SLOTS`）。
    // 量的是块表里所有文本的字节和（正文拷贝一份＋块结构，真实占用比这个再大一截）。
    println!("\n== 一块槽里装着多少正文文本（多槽缓存的内存代价）==");
    println!(
        "{:<14}{:>10}{:>10}{:>9}",
        "夹具", "正文字节", "块表字节", "倍率"
    );
    for case in &cases {
        let mut held = 0usize;
        for b in &editpad_core::markdown::parse_markdown(&case.text) {
            count_text(b, &mut held);
        }
        println!(
            "{:<14}{:>10}{:>10}{:>8.2}x",
            case.name,
            case.text.len(),
            held,
            held as f64 / case.text.len() as f64
        );
    }
}

/// 块表里落着的**文本字节**和（结构与 Vec 的容量不算在里面）。
fn count_text(block: &editpad_core::markdown::MdBlock, out: &mut usize) {
    use editpad_core::markdown::MdBlock as B;
    let spans_len = |spans: &[editpad_core::markdown::MdSpan]| -> usize {
        spans.iter().map(|s| s.text.len()).sum()
    };
    *out += match block {
        B::Heading { spans, .. } | B::Paragraph { spans } => spans_len(spans),
        B::ListItem { spans, .. } => spans_len(spans),
        B::CodeBlock { lang, lines } => {
            lang.as_ref().map_or(0, |l| l.len()) + lines.iter().map(String::len).sum::<usize>()
        }
        B::Quote { blocks, .. } => {
            let mut inner = 0usize;
            for b in blocks {
                count_text(b, &mut inner);
            }
            inner
        }
        B::Table { header, rows, .. } => {
            header.iter().flatten().map(|s| s.text.len()).sum::<usize>()
                + rows
                    .iter()
                    .flatten()
                    .flatten()
                    .map(|s| s.text.len())
                    .sum::<usize>()
        }
        B::Footnotes { items } => items.iter().map(|i| spans_len(&i.spans)).sum(),
        B::Rule => 0,
    };
}

fn cases() -> Vec<Case> {
    let readme = std::fs::read_to_string(format!("{}/../../README.md", env!("CARGO_MANIFEST_DIR")))
        .expect("仓库根的 README.md 读不到（仪表夹具依赖真实文档）");

    vec![
        Case {
            name: "README",
            why: "真实文档基线：短小、行内标记稀疏",
            text: readme.clone(),
        },
        Case {
            name: "README×10",
            why: "2026-10-12 那批读数里的 353 KB 档＝1421 块",
            text: readme.repeat(10),
        },
        Case {
            name: "表格 300×4",
            why: "`is_table_start` 为答布尔把每格走行内解析",
            text: table_doc(300, 4),
        },
        Case {
            name: "未闭围栏 20KB",
            why: "`fence()` 判据未闭栏要扫到 EOF 并逐行抄正文",
            text: unclosed_fence_doc(20 * 1024),
        },
        Case {
            name: "脚注 300",
            why: "标签按 Vec 线性比对 ⇒ 工作量随「引用×定义」涨（看下面 600 那一档）",
            text: footnote_doc(300),
        },
        Case {
            name: "脚注 600",
            why: "文档只大一倍，比对数却涨四倍（现量 18 万→72 万次）",
            text: footnote_doc(600),
        },
        Case {
            name: "段落顶表",
            why: "段落续行判据为答布尔把整表再解析一遍（1204→2408 格）",
            text: para_then_table(300, 4),
        },
        Case {
            name: "粗体不配对",
            why: "贪心配对扫到段尾再重试三种样式",
            text: runaway_bold_doc(400),
        },
    ]
}

fn table_doc(rows: usize, cols: usize) -> String {
    let mut out = String::from("# 表格密集\n\n");
    out.push_str(&row(cols, |c| format!("列{c}")));
    out.push_str(&row(cols, |_| "-".repeat(8)));
    for r in 0..rows {
        out.push_str(&row(cols, |c| format!("第{r}行第{c}格的说明文字")));
    }
    out.push_str("\n正文收尾一段。\n");
    out
}

fn row(cols: usize, cell: impl Fn(usize) -> String) -> String {
    let mut s = String::new();
    for c in 0..cols {
        s.push_str("| ");
        s.push_str(&cell(c));
        s.push(' ');
    }
    s.push_str("|\n");
    s
}

/// 段落**紧贴**表格（中间没空行）⇒ 段落续行判据替表格首行问一次 `is_table_start`，
/// 而那一问把整张表的每一格都做了行内解析，做完只为回答「是不是新块」。
fn para_then_table(rows: usize, cols: usize) -> String {
    let mut out = String::from("# 段落顶着的表格\n\n正文第一段最后一行\n");
    out.push_str(&row(cols, |c| format!("列{c}")));
    out.push_str(&row(cols, |_| "-".repeat(8)));
    for r in 0..rows {
        out.push_str(&row(cols, |c| format!("第{r}行第{c}格的说明文字")));
    }
    out
}

fn unclosed_fence_doc(bytes: usize) -> String {
    let mut out = String::from("# 未闭栏\n\n```rust\n");
    while out.len() < bytes {
        out.push_str("let x = compute(a, b); // 这一行是围栏里的内容\n");
    }
    out
}

fn footnote_doc(n: usize) -> String {
    let mut out = String::from("# 脚注密集\n\n正文：");
    for i in 0..n {
        out.push_str(&format!("第一{i}处引用[^{i}]，"));
    }
    out.push_str("收尾。\n\n");
    for i in 0..n {
        out.push_str(&format!("[^{i}]: 第{i}条注脚正文。\n"));
    }
    out
}

fn runaway_bold_doc(n: usize) -> String {
    let mut out = String::from("# 不配对的强调\n\n");
    for i in 0..n {
        out.push_str(&format!(
            "第{i}段里有一处开了栏的粗体**可是这一段再也没有闭栏，\
             后面还跟着斜体*也没闭，删除线~~同样没闭。\n"
        ));
    }
    out
}
