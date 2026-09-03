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
