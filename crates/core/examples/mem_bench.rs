//! P19 内存基线测量（口径 b）：大文档加载的内存峰值与稳态开销。
//!
//! 本示例只负责**阶段化运行**并打印里程碑；内存数字由外部脚本对进程
//! 做 WorkingSet 轮询采样（见仓库 HANDOFF.md 第 9 轮记录的驱动命令），
//! 避免 core 引入平台相关依赖。阶段划分：
//!
//! ```text
//! [phase] tiny     —— 小文档（~1KB）就位：进程地板值（口径 a 的下界参考）
//! [phase] sleeping —— 停 1s 让外部采到小文档稳态
//! [phase] loading  —— load_file_streaming 大文件（加载中，外部采峰值）
//! [phase] steady   —— rope 就位后停 5s（外部采稳态）
//! ```
//!
//! 用法（务必 release 构建，与 P19 验收口径一致）：
//! ```text
//! cargo run --release -p editpad-core --example mem_bench [文件路径]
//! ```

use std::path::PathBuf;
use std::time::Duration;

use editpad_core::{load_document_streaming, Document};

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bench-large.log"));

    // 阶段 0：小文档地板值（空进程 + ropey 运行时的常驻底数）
    let tiny = Document::from_str("hello, editpad!\n");
    println!("[phase] tiny held {} bytes", tiny.text_len());

    println!("[phase] sleeping");
    std::thread::sleep(Duration::from_secs(1));

    // 阶段 1：流式直入 rope（P19 行动项 2 落地后的测量口径）
    println!("[phase] loading {}", path.display());
    let started = std::time::Instant::now();
    let loaded = load_document_streaming(&path, &mut |_progress| {}).expect("加载基准文件失败");
    let doc = loaded.doc;
    println!(
        "[phase] loaded {:?} chars={} encoding={}",
        started.elapsed(),
        doc.text_len(),
        loaded.encoding
    );

    // 阶段 2：稳态保持（只有 rope 活着）
    println!("[phase] steady");
    std::thread::sleep(Duration::from_secs(5));
    println!("[phase] done chars={}", doc.text_len());
}
