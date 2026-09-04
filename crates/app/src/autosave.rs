use super::*;

/// 一次自动保存的结局（P63）：写盘成功 / 撞上外部修改被拒写 / 写盘失败。
/// 拒写不是失败——磁盘上发生了别人（其他编辑器/同步工具）的改动，
/// 盲写会覆盖它；裁决权交给 P52 外部修改提示条。
#[derive(Debug, Clone)]
pub(crate) enum AutosaveOutcome {
    Written,
    SkippedExternalChange,
    Failed(String),
}

/// 自动保存任务的落盘配置快照（P63/第 64 轮 ⑭：后台线程无 &Settings/
/// &Tab 可用，调度时刻随任务下发）。
pub(crate) struct AutosaveTask {
    /// 按标签页保存编码落盘（与手动保存同参，防静默转码）
    pub(crate) encoding: editpad_core::SaveEncoding,
    /// 调度时刻的外部修改比对戳：醒来先校验再写，绝不盲写覆盖外部改动
    pub(crate) expected_stamp: Option<(std::time::SystemTime, u64)>,
    /// 防抖窗时长
    pub(crate) delay: std::time::Duration,
    /// 写前备份模式（自动保存静默口径）
    pub(crate) backup_mode: String,
}

/// 一次自动保存的驱动：专属 OS 线程「睡满防抖窗 → 写前校验 → 分块原子
/// 落盘」，结果经 std mpsc 桥接回异步端（P5/P10 同款；执行器仅阻塞等待
/// 结果，且按页 inflight 去重保证同一页至多一个这样的线程）。
///
/// P63 写前校验：防抖睡眠期间磁盘可能被外部修改（焦点巡检只在窗口
/// 重聚焦时跑，救不了后台线程）。期望戳不一致即拒写并回报
/// [`AutosaveOutcome::SkippedExternalChange`]——原文件绝不盲写覆盖外部
/// 内容。期望戳为 None（从未记录，如测试注入的不存在路径）时保持旧
/// 语义直接写。
pub(crate) async fn drive_autosave_once(
    tab: usize,
    path: PathBuf,
    doc: editpad_core::Document,
    version: u64,
    task: AutosaveTask,
) -> Message {
    let (tx, rx) = std_mpsc::channel::<AutosaveOutcome>();
    let thread_path = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(task.delay);
        let outcome = if autosave_must_skip(task.expected_stamp, file_stamp(&path)) {
            AutosaveOutcome::SkippedExternalChange
        } else {
            write_to_disk(&path, &doc, task.encoding, &task.backup_mode)
        };
        let _ = tx.send(outcome);
    });
    let outcome = rx
        .recv()
        .unwrap_or_else(|_| AutosaveOutcome::Failed("自动保存线程意外终止".to_owned()));
    // 路径本体已随闭包移入写盘线程；回报携带同内容的克隆
    Message::TabAutosaved(tab, version, thread_path, outcome)
}

/// 防抖窗睡满后的实际落盘动作（线程体调用；同步函数便于测试直击磁盘
/// 字节）。写前备份维持第 64 轮 ⑭ 的静默口径；按传入编码落盘（与手动
/// 保存同参），编码附带的不可映射告警同样不上浮打扰。
pub(crate) fn write_to_disk(
    path: &Path,
    doc: &editpad_core::Document,
    encoding: editpad_core::SaveEncoding,
    backup_mode: &str,
) -> AutosaveOutcome {
    let _ = perform_backup_before_overwrite(path, backup_mode);
    match editpad_core::save_document_encoded(path, doc, encoding) {
        Ok(_) => AutosaveOutcome::Written,
        Err(e) => AutosaveOutcome::Failed(e.to_string()),
    }
}

/// 自动保存写前判定（纯函数可单测，P63）：期望戳已知（Some）且与当前
/// 磁盘戳不一致 = 有外部修改（含文件被删），必须拒写。期望戳 None =
/// 从未记录（无从比对），不拦截——与 [`file_changed_externally`] 的
/// 「记录缺失不判定」口径一致，但这里反过来以期望戳为主语。
pub(crate) fn autosave_must_skip(
    expected: Option<(std::time::SystemTime, u64)>,
    current: Option<(std::time::SystemTime, u64)>,
) -> bool {
    expected.is_some() && current != expected
}

// ---------- 第 64 轮 ⑭：保存时备份磁盘旧版 ----------

/// 大文件豁免阈值：源文件超过此字节数跳过备份（复制耗时会拖慢保存，
/// 且 64MB+ 的日志类文件通常有专门的轮转手段）。取值对齐性能基准
/// bench-50mb.log 量级再留余量。
pub(crate) const MAX_BACKUP_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// 写前备份磁盘旧版（⑭）。返回状态栏提示文本；None = 无事发生。
///
/// 口径：
/// * 目标文件不存在（新建/另存到新路径）→ 无旧版可备份，None；
/// * 源超过 [`MAX_BACKUP_SOURCE_BYTES`] → 跳过并提示；
/// * simple → 同目录 `name.bak` 覆盖式；
/// * timestamped → 同目录 `name.bak.d/` **目录**内
///   `name.YYYYMMDD-HHMMSS.bak` 历史留存（`.bak.d` 与 simple 的
///   `name.bak` 文件不同名，两模式可自由切换互不污染）；
/// * 任何 IO 失败都**不阻断保存**——降级为状态栏提示（备份是锦上添
///   花，不能成为丢保存的理由）。
pub(crate) fn perform_backup_before_overwrite(path: &Path, mode: &str) -> Option<String> {
    use editpad_core::settings::{BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED};
    if mode == editpad_core::settings::BACKUP_MODE_NONE {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > MAX_BACKUP_SOURCE_BYTES {
        return Some("文件超过 64MB，按策略跳过备份".to_owned());
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let report = |r: std::io::Result<PathBuf>| match r {
        Ok(p) => Some(format!("已备份旧版 → {}", p.display())),
        Err(e) => Some(format!("备份失败（继续保存）：{e}")),
    };
    match mode {
        BACKUP_MODE_SIMPLE => {
            let bak = path.with_file_name(format!("{name}.bak"));
            report(std::fs::copy(path, &bak).map(|_| bak))
        }
        BACKUP_MODE_TIMESTAMPED => {
            let dir = path.with_file_name(format!("{name}.bak.d"));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Some(format!("备份失败（继续保存）：{e}"));
            }
            let stamp = editor::local_datetime_stamp_compact();
            let target = dir.join(format!("{name}.{stamp}.bak"));
            report(std::fs::copy(path, &target).map(|_| target))
        }
        _ => None,
    }
}
