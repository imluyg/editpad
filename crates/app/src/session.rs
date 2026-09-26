use super::*;

/// 多开内存护栏上限（字节）：全部页内容 + 待载文件的保守估算。
/// 256 MiB ≈ 五份 50MB 级 ASCII 大文档同时驻留（rope 存的就是 UTF-8 字节）。
pub(crate) const MULTI_TAB_MEM_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// 内存护栏判定（纯函数便于测试）：现有页内容字节数 + 待载文件字节数与上限比较。
///
/// P300（第 209 轮，用户点单）：已打开侧原先按「字符数 ×3 字节」估算，而**待打开侧按
/// 文件字节数**记 ⇒ 同一份 ASCII 文档"打开时算 1×、开完之后算 3×"，纯 ASCII 大文档的
/// 有效容量被压到 2/5（50MB 那份只能开 2 份而非 5 份）。rope 里存的本就是 UTF-8 字节，
/// 按字节记不是放宽估算，是把单位对准它一直在估的那个量。恢复链的
/// [`plan_restore_order`] 从来就是按字节估（磁盘／快照文件长度），这次两条路径同单位。
/// ⚠️ 仍未计入的是派生结构（折行索引、逐行缓存、字形表等）——上限管的是正文体量，
/// 这句话别读成"256 MiB 就是进程总占用"。
pub(crate) fn mem_guard_allows(existing_bytes: usize, incoming_bytes: u64, cap_bytes: u64) -> bool {
    (existing_bytes as u64).saturating_add(incoming_bytes) <= cap_bytes
}

// ---------- 启动会话恢复（P30） ----------

/// 启动会话恢复的总开关判定（纯函数便于测试）：
/// 快照底座与「记住会话」两个开关都开启才允许恢复/写清单。
pub(crate) fn session_restore_allowed(enable_snapshots: bool, remember_session: bool) -> bool {
    enable_snapshots && remember_session
}

/// 恢复链中一个待载入的命名干净页：路径、目标占位页下标与待还原视图。
#[derive(Debug, Clone)]
pub(crate) struct RestoreLoad {
    pub(crate) path: PathBuf,
    pub(crate) tab: usize,
    pub(crate) cursor_line: usize,
    pub(crate) cursor_col: usize,
    pub(crate) scroll_top: f32,
    pub(crate) scroll_left: f32,
}

/// 恢复截断规划（纯函数便于测试）：估算各页字节量——置脏页按其快照
/// 文件、干净命名页按磁盘文件、干净未命名页为 0——先保激活页再按下标
/// 顺序装填，累计超上限的页放弃恢复（§3 P30 第 5 条）。
/// 返回 (保留下标升序, 被截断页数)。
pub(crate) fn plan_restore_order(
    manifest: &editpad_core::snapshot::SessionManifest,
    snapshot_dir: &Path,
    cap_bytes: u64,
) -> (Vec<usize>, usize) {
    fn page_estimate(tab: &editpad_core::snapshot::SessionTab, snapshot_dir: &Path) -> u64 {
        if let Some(file) = &tab.file {
            fs::metadata(snapshot_dir.join(file))
                .map(|m| m.len())
                .unwrap_or(0)
        } else if let Some(path) = &tab.path {
            fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        } else {
            0 // 干净未命名页 = 空文档
        }
    }

    let count = manifest.tabs.len();
    let active = manifest.active.min(count.saturating_sub(1));
    // 装填顺序：激活页优先（「先保激活页」），其余按下标升序
    let mut order = vec![active];
    order.extend((0..count).filter(|&i| i != active));

    let mut kept = Vec::new();
    let mut total = 0u64;
    for i in order {
        let size = page_estimate(&manifest.tabs[i], snapshot_dir);
        if total.saturating_add(size) > cap_bytes {
            continue; // 截断：该页放弃恢复，不阻断其余页
        }
        total += size;
        kept.push(i);
    }
    kept.sort_unstable();
    let dropped = count - kept.len();
    (kept, dropped)
}

/// 取文档头部样本（≤4096 字符）供语言嗅探：迭代 rope 存储块拼接，
/// 不产生全文拷贝（P19 内存口径）。
pub(crate) fn head_sample(doc: &editpad_core::Document) -> String {
    const LIMIT: usize = 4096;
    let mut sample = String::new();
    'outer: for chunk in doc.chunks() {
        for ch in chunk.chars() {
            sample.push(ch);
            if sample.chars().count() >= LIMIT {
                break 'outer;
            }
        }
    }
    sample
}
