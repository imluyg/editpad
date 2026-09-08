use super::*;

/// 查找防抖窗口：窗口内的新输入会作废旧任务（置位其取消标志）并另排新任务。
pub(crate) const FIND_DEBOUNCE_MS: u64 = 200;

/// 一次后台查找扫描的输入快照。`doc` 是 rope 的结构共享克隆（O(1)，
/// 不拷贝正文）；`cancelled` 指向应用状态里的当前代取消标志——
/// 新输入会把上一代的标志置位，睡醒后的旧任务检查到即放弃扫描。
#[derive(Clone)]
pub(crate) struct FindScanPayload {
    pub(crate) seq: u64,
    pub(crate) doc: editpad_core::Document,
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    /// P70：正则模式（query 为原始正则；扫描走全文 to_text + fancy-regex）
    pub(crate) regex: bool,
    /// 本代任务的取消标志（新任务排队时把上一代置位）
    pub(crate) cancelled: Arc<AtomicBool>,
    /// 防抖窗口毫秒数（生产走 [`FIND_DEBOUNCE_MS`]；测试注入小值）
    pub(crate) debounce_ms: u64,
}

/// 查找任务的事件驱动（扫描函数与防抖时长均可注入以便测试，同 [`drive_load`] 做法）。
///
/// 保证语义：无论扫描成功、被取消还是 **panic**，都恰好回一条 `FindScanDone`
/// ——否则查找栏会永久停在「查找中…」。过期结果由 update 按 seq 二次过滤。
pub(crate) async fn drive_find_scan<F>(
    payload: FindScanPayload,
    scan: F,
) -> Message
where
    F: FnOnce(&editpad_core::Document, &str, bool, bool) -> Vec<editpad_core::MatchPos>
        + Send
        + 'static,
{
    let (notify_tx, notify_rx) = std_mpsc::channel::<Vec<editpad_core::MatchPos>>();
    std::thread::spawn(move || {
        // 防抖：真正的取消由 cancelled 标志完成——新输入排队时置位上一代，
        // 这里睡满窗口后检查，被作废的任务直接退出、不浪费一次全文扫描
        std::thread::sleep(std::time::Duration::from_millis(payload.debounce_ms));
        let matches = if payload.cancelled.load(Ordering::Relaxed) {
            Vec::new()
        } else {
            // P5 同款兜底：扫描崩溃也要回消息（空表），不能让 UI 永久等待
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scan(&payload.doc, &payload.query, payload.case_sensitive, payload.regex)
            }))
            .unwrap_or_default()
        };
        let _ = notify_tx.send(matches);
    });

    // 阻塞 recv 与 drive_load 的取舍相同：OS 线程结果桥接到异步端，
    // iced 线程池可承受短暂阻塞
    let matches = notify_rx.recv().unwrap_or_default();
    Message::FindScanDone(payload.seq, matches)
}

/// 与 core::search 一致的大小写语义：实现已下沉到 core（P15 去重），
/// 这里只保留「区分大小写走整串比较」的快速路径。
pub(crate) fn strings_equal(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.chars().count() == b.chars().count()
            && a.chars()
                .zip(b.chars())
                .all(|(x, y)| editpad_core::ascii_case_eq(x, y, false))
    }
}

/// 查找/替换输入的转义解析（P22 补充能力）：
/// `\n` `\r` `\t` `\\` 分别解析为换行、回车、制表符、反斜杠；
/// 其他未知转义保持原样（`\q` 仍是 `\q`），不做半截猜测。
///
/// 这让用户可以搜索/替换换行与制表符等不可见字符——
/// 也是未来多行查询的输入入口。
pub(crate) fn unescape_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    let mut chars = q.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            // 未知转义或孤立反斜杠：按字面保留
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

// ---------- A8：在文件中查找（设计 docs/find-in-files-design.md §3.2） ----------

/// 一条命中：位置 + 扫描时预计算的行摘录（面板展示零文件回读——
/// 结果只存命中不驻留全文的设计约束）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHit {
    pub(crate) pos: editpad_core::MatchPos,
    pub(crate) excerpt: String,
}

/// 一个文件的命中结果（面板按文件分组展示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHits {
    pub(crate) path: std::path::PathBuf,
    pub(crate) hits: Vec<FileHit>,
}

/// 三重封顶（设计 §3.2）：任一触达即截断明示并提前收尾。
pub(crate) const FIF_MAX_FILES: usize = 20_000;
pub(crate) const FIF_MAX_TOTAL_HITS: usize = 5_000;
pub(crate) const FIF_MAX_HITS_PER_FILE: usize = 1_000;
/// 面板摘录窗口列数（与查找全部面板 FIND_ALL_EXCERPT_COLS 同量级）。
pub(crate) const FIF_EXCERPT_COLS: usize = 96;

/// 一次「在文件中查找」后台扫描的输入快照。封顶参数显式入参
/// （生产走上面常量，测试注入小值）；`cancelled`/`progress` 与应用
/// 状态共享——前者供 UI 取消，后者供面板标题实时读已扫文件数。
#[derive(Clone)]
pub(crate) struct FifScanPayload {
    pub(crate) seq: u64,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    pub(crate) whole_word: bool,
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) progress: Arc<AtomicUsize>,
    pub(crate) max_files: usize,
    pub(crate) max_total_hits: usize,
    pub(crate) max_hits_per_file: usize,
}

/// 目录扫描本体（同步、可直接单测）：walk_files 遍历 → 逐文件大小
/// 预检 → load_file（编码嗅探/二进制拒绝全继承）→ find_in_file。
/// 任一文件失败（二进制/IO）跳过不中断；每文件 progress +1。
pub(crate) fn fif_scan_dir(payload: &FifScanPayload) -> (Vec<FileHits>, bool) {
    if payload.cancelled.load(Ordering::Relaxed) {
        return (Vec::new(), false);
    }
    let mut results: Vec<FileHits> = Vec::new();
    let mut total_hits = 0usize;
    let walk = editpad_core::walk_files(&payload.dir, payload.max_files);
    let mut truncated = walk.truncated;
    for path in walk.files {
        if payload.cancelled.load(Ordering::Relaxed) {
            // 取消：已扫出的部分结果仍然有效，如实返回
            return (results, truncated);
        }
        payload.progress.fetch_add(1, Ordering::Relaxed);
        // 大文件豁免（64 MB，同备份口径）
        if std::fs::metadata(&path)
            .map(|m| m.len() > editpad_core::MAX_SCAN_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        // 编码嗅探/二进制拒绝全继承：二进制或读取失败跳过该文件
        let Ok(loaded) = editpad_core::load_file(&path) else {
            continue;
        };
        let hits = editpad_core::find_in_file(
            &loaded.text,
            &payload.query,
            payload.case_sensitive,
            payload.regex,
            payload.whole_word,
            payload.max_hits_per_file,
        );
        if hits.is_empty() {
            continue;
        }
        total_hits += hits.len();
        // 摘录随扫描一次算好（单遍切行；find_in_file 输出按行序升序）
        let excerpts = collect_excerpts(&loaded.text, &hits, FIF_EXCERPT_COLS);
        results.push(FileHits {
            path,
            hits: hits
                .into_iter()
                .zip(excerpts)
                .map(|(pos, excerpt)| FileHit { pos, excerpt })
                .collect(),
        });
        if total_hits >= payload.max_total_hits {
            truncated = true;
            break;
        }
    }
    (results, truncated)
}

/// 按行序升序的命中批量提取行摘录（单遍切行，与 find_all 的行界口径
/// 一致：`\r\n` / 孤立 `\r` / `\n` 皆行界）。
fn collect_excerpts(
    text: &str,
    hits: &[editpad_core::MatchPos],
    max_cols: usize,
) -> Vec<String> {
    let mut out = vec![String::new(); hits.len()];
    let mut hit_i = 0usize;
    let mut line_idx = 0usize;
    let mut rest = text;
    loop {
        let content_end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        while hit_i < hits.len() && hits[hit_i].line == line_idx {
            out[hit_i] = crate::view::match_excerpt(&rest[..content_end], hits[hit_i].col, max_cols);
            hit_i += 1;
        }
        if hit_i >= hits.len() || content_end >= rest.len() {
            break;
        }
        rest = if rest[content_end..].starts_with("\r\n") {
            &rest[content_end + 2..]
        } else {
            &rest[content_end + 1..]
        };
        line_idx += 1;
    }
    out
}

/// 「在文件中查找」任务的事件驱动（与 [`drive_find_scan`] 同骨架：
/// OS 线程 + mpsc + catch_unwind 兜底）。无论成功、取消还是 panic 都
/// 恰好回一条 `FifScanDone`，UI 永不永久等待；过期结果由 update 按
/// seq 二次过滤。
pub(crate) async fn drive_find_in_files(payload: FifScanPayload) -> Message {
    let seq = payload.seq;
    let (notify_tx, notify_rx) = std_mpsc::channel::<(Vec<FileHits>, bool)>();
    std::thread::spawn(move || {
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fif_scan_dir(&payload)
        }))
        .unwrap_or((Vec::new(), false));
        let _ = notify_tx.send(out);
    });
    let (results, truncated) = notify_rx.recv().unwrap_or((Vec::new(), false));
    Message::FifScanDone(seq, results, truncated)
}
