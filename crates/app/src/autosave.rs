use super::*;

/// 一次自动保存的结局（P63）：写盘成功 / 撞上外部修改被拒写 / 写盘失败 /
/// P146 代次过期作废（调度后页被编辑/回基线/改路径/关页——写盘动作
/// 本身被跳过，什么都没发生）。拒写不是失败——磁盘上发生了别人（其他
/// 编辑器/同步工具）的改动，盲写会覆盖它；裁决权交给 P52 外部修改提示条。
#[derive(Debug, Clone)]
pub(crate) enum AutosaveOutcome {
    Written,
    SkippedExternalChange,
    Failed(String),
    Superseded,
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
}

/// 一次自动保存的驱动：专属 OS 线程「睡满防抖窗 → 代次校验 → 写前校验
/// → 分块原子落盘」，结果经 [`await_on_thread`] 桥接回异步端（P5/P10 同款
/// 桥接，等待端不占执行器 worker；按页 inflight 去重保证同一页至多一个这样
/// 的线程）。
///
/// P63 写前校验：防抖睡眠期间磁盘可能被外部修改（焦点巡检只在窗口
/// 重聚焦时跑，救不了后台线程）。期望戳不一致即拒写并回报
/// [`AutosaveOutcome::SkippedExternalChange`]——原文件绝不盲写覆盖外部
/// 内容。期望戳为 None（从未记录，如测试注入的不存在路径）时保持旧
/// 语义直接写。
///
/// P146 代次校验：调度后页被编辑/撤销回基线/改路径/关页都会推进页的
/// 代次计数器（`gen`），本任务带着调度时刻的 `my_gen`——醒来不符即
/// 作废，不落盘（曾只校验磁盘戳：已作废快照照样写入，磁盘与 UI 双向
/// 失真；「放弃更改并关闭」的页也会被复活写盘）。
pub(crate) async fn drive_autosave_once(
    tab_id: u64,
    path: PathBuf,
    doc: editpad_core::Document,
    version: u64,
    gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    my_gen: u64,
    task: AutosaveTask,
) -> Message {
    use std::sync::atomic::Ordering;
    let thread_path = path.clone();
    let outcome = await_on_thread(move || {
        std::thread::sleep(task.delay);
        if gen.load(Ordering::Relaxed) != my_gen {
            AutosaveOutcome::Superseded
        } else if autosave_must_skip(task.expected_stamp, file_stamp(&path)) {
            AutosaveOutcome::SkippedExternalChange
        } else {
            write_to_disk(&path, &doc, task.encoding)
        }
    })
    .await
    // P155 取舍披露：这条只在「写盘线程 panic 到连 channel 都断了」时
    // 出现，且 AutosaveOutcome::Failed 收的是 String（IO 错误本就是
    // 系统 locale 文本，无法可靠翻译）。故此处固定取默认语言的文案，
    // 不为一个近乎不可达的分支把 lang 一路透传到后台任务。
    .unwrap_or_else(|_| {
        AutosaveOutcome::Failed(
            editpad_core::Key::StAutosaveThreadGone
                .text(editpad_core::Lang::default())
                .to_owned(),
        )
    });
    // 路径本体已随闭包移入写盘线程；回报携带同内容的克隆
    Message::TabAutosaved(tab_id, version, thread_path, outcome)
}

/// 防抖窗睡满后的实际落盘动作（线程体调用；同步函数便于测试直击磁盘
/// 字节）。按传入编码落盘（与手动保存同参），编码附带的不可映射告警
/// 不上浮打扰。
///
/// **不做写前备份**（与手动保存的唯一差异）：自动保存是 2 秒节拍的重复
/// 覆写，备份到的只是「上一秒的自己」——没有保留价值，却带来持续的整文件
/// IO 放大，timestamped 模式下还是无界增长。备份语义属「用户显式覆写磁盘」，
/// 留在 `save()` 那条路径上。
pub(crate) fn write_to_disk(
    path: &Path,
    doc: &editpad_core::Document,
    encoding: editpad_core::SaveEncoding,
) -> AutosaveOutcome {
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
/// 几十 MB 日志量级再留余量。
pub(crate) const MAX_BACKUP_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// timestamped 目录的留存上限。时间戳粒度为秒，显式保存连点、或长时间编辑
/// 一个大文件都能轻松攒出成百上千份整文件副本（「逐次留存」不该等于无限
/// 增长：既吃磁盘，也让目录里再也找不到有用的那一份）。
pub(crate) const MAX_TIMESTAMPED_BACKUPS: usize = 20;

/// 回收 timestamped 目录里超出上限的历史档，**留最新的 N 份**。
///
/// 档名是 `{name}.YYYYMMDD-HHMMSS[-序号].bak`，日期段时间戳的字典序即时间
/// 序（同秒档的 `-N` 后缀让它在同秒内排在无后缀档之后，仍是「越新越靠后」），
/// 所以「排序后从头多出来的部分删掉」就等价于「留最新」。只处理本项目命名
/// 的文件，删不掉则忽略（回收是收尾动作，不该让备份本身失败）。
fn prune_timestamped_backups(dir: &Path, name: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{name}.");
    let mut archive: Vec<std::path::PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let file = e.path();
            let stem = file.file_name()?.to_string_lossy().into_owned();
            (stem.starts_with(&prefix) && stem.ends_with(".bak")).then_some(file)
        })
        .collect();
    if archive.len() <= MAX_TIMESTAMPED_BACKUPS {
        return;
    }
    archive.sort();
    for stale in archive.into_iter().rev().skip(MAX_TIMESTAMPED_BACKUPS) {
        let _ = std::fs::remove_file(stale);
    }
}

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
///
/// P155：写盘前备份的结果说明（**语言无关**）。
///
/// 文案是界面语言的一部分，故这里只带「哪种结果 + 必要的动态值」，
/// 由展示侧（状态栏）按当前语言取文——与 [`crate::editor::EditErr`]
/// 同一口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupNote {
    /// 源文件超过 64MB，按策略跳过
    SkippedLarge,
    /// 已备份到该路径
    Backed(std::path::PathBuf),
    /// 备份失败（不阻断保存）
    Failed(String),
}

impl BackupNote {
    /// 按界面语言取状态栏文案。
    pub(crate) fn text(&self, lang: editpad_core::Lang) -> String {
        use editpad_core::Key as K;
        match self {
            BackupNote::SkippedLarge => K::StBackupSkipLarge.text(lang).to_owned(),
            BackupNote::Backed(p) => {
                format!("{}{}", K::StBackupDone.text(lang), p.display())
            }
            BackupNote::Failed(e) => format!("{}{e}", K::StBackupFailed.text(lang)),
        }
    }
}

pub(crate) fn perform_backup_before_overwrite(path: &Path, mode: &str) -> Option<BackupNote> {
    use editpad_core::settings::{BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED};
    if mode == editpad_core::settings::BACKUP_MODE_NONE {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > MAX_BACKUP_SOURCE_BYTES {
        return Some(BackupNote::SkippedLarge);
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let report = |r: std::io::Result<PathBuf>| match r {
        Ok(p) => Some(BackupNote::Backed(p)),
        Err(e) => Some(BackupNote::Failed(e.to_string())),
    };
    match mode {
        BACKUP_MODE_SIMPLE => {
            let bak = path.with_file_name(format!("{name}.bak"));
            report(std::fs::copy(path, &bak).map(|_| bak))
        }
        BACKUP_MODE_TIMESTAMPED => {
            let dir = path.with_file_name(format!("{name}.bak.d"));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Some(BackupNote::Failed(e.to_string()));
            }
            let stamp = editor::local_datetime_stamp_compact();
            // P148：时间戳粒度为秒——同秒内的第二次保存曾直接覆盖前一次
            // 的「历史留存」，与模式承诺的逐次留存不符。目标已存在时追加
            // 序号（-1、-2…），同秒多次保存各自成档。
            let mut target = dir.join(format!("{name}.{stamp}.bak"));
            let mut seq = 1u32;
            while target.exists() {
                target = dir.join(format!("{name}.{stamp}-{seq}.bak"));
                seq += 1;
            }
            let note = report(std::fs::copy(path, &target).map(|_| target));
            // 本轮确实留了档才回收：失败时目录内容没变，不必动它
            if matches!(note, Some(BackupNote::Backed(_))) {
                prune_timestamped_backups(&dir, &name);
            }
            note
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落在**系统 TEMP**下的 `editpad-app-tests/<tag>-<pid>`（与
    /// `tests/mod.rs::scratch_dir` 同口径）。
    /// ⚠️ 原注释写的是「项目内落盘目录……TEMP 在部分沙箱不可写」，与本函数
    /// 实现正好相反，且那半句理由在这里并不成立——本 crate 里两套口径并存：
    /// 会话/保存类测试（`tests/session.rs`、core 的 `saver.rs`）挂
    /// `target/test-scratch`，本文件与 `tests/mod.rs` 挂 TEMP。二者各自的用例
    /// 别混用。
    fn scratch_dir(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join("editpad-app-tests")
            .join(format!("{tag}-{}", std::process::id()))
    }

    #[test]
    fn timestamped_backups_are_pruned_to_the_newest_cap() {
        // 「逐次留存」不等于无限增长：目录里堆几百份整文件副本既吃穿磁盘，
        // 也让人再也找不到有用的那一份。
        let dir = scratch_dir("p-backup-cap");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("note.txt");
        std::fs::write(&target, "current").unwrap();
        let bak_dir = dir.join("note.txt.bak.d");
        std::fs::create_dir_all(&bak_dir).unwrap();
        // 40 份历史档，字典序即时间序（100000 最旧 → 100039 最新）
        for i in 0..40 {
            std::fs::write(
                bak_dir.join(format!("note.txt.20260101-{:06}.bak", 100_000 + i)),
                b"old",
            )
            .unwrap();
        }

        perform_backup_before_overwrite(&target, editpad_core::settings::BACKUP_MODE_TIMESTAMPED)
            .expect("timestamped 备份应成功");

        let names: Vec<String> = std::fs::read_dir(&bak_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names.len(),
            MAX_TIMESTAMPED_BACKUPS,
            "总数（含本轮新档）必须封顶在 {MAX_TIMESTAMPED_BACKUPS}，实际 {names:?}"
        );
        let has = |stamp: &str| names.iter().any(|n| n.ends_with(&format!("-{stamp}.bak")));
        assert!(has("100039"), "最新的历史档不能被删：{names:?}");
        assert!(has("100021"), "留存窗口内的档不能被删：{names:?}");
        assert!(!has("100020"), "超出上限的最旧档必须被回收：{names:?}");
        assert!(!has("100000"), "最旧档必须最先被回收：{names:?}");
        // 本轮新写的备份（时间戳非 20260101-1xxxxx）也在留存内 → 19 旧 + 1 新
        assert_eq!(
            names.iter().filter(|n| n.contains("20260101-")).count(),
            MAX_TIMESTAMPED_BACKUPS - 1,
            "留存的 19 份历史档 + 本轮新档 = 上限：{names:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 自动保存的 2 秒节拍不该驱动「写前备份」：那只是把上一秒的自己复制一
    /// 份，既无保留价值，又是持续的整文件 IO 放大（timestamped 模式下更是
    /// 无界增长）。备份语义属「用户显式覆写磁盘」，即手动保存那条路径。
    #[test]
    fn autosave_write_does_not_produce_backup_copies() {
        let dir = scratch_dir("p-autosave-no-backup");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("note.txt");
        std::fs::write(&target, "disk v1").unwrap();

        let outcome = write_to_disk(
            &target,
            &editpad_core::Document::from_str("disk v2"),
            editpad_core::SaveEncoding::Utf8,
        );
        assert!(matches!(outcome, AutosaveOutcome::Written), "实际 {outcome:?}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "disk v2",
            "内容照常落盘"
        );
        assert!(
            !dir.join("note.txt.bak.d").exists() && !dir.join("note.txt.bak").exists(),
            "自动保存不得做写前备份"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timestamped_backup_same_second_saves_keeps_each_copy() {
        // P148 回归：时间戳粒度为秒——同秒内的两次保存曾互相覆盖，
        // 「逐次留存」承诺失守。同秒第二次备份应落 -1 序号档。
        let dir = scratch_dir("backup-same-second");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("note.txt");
        std::fs::write(&target, "v1").unwrap();

        assert!(perform_backup_before_overwrite(&target, "timestamped").is_some());
        // 同秒内再备份一次（若跨秒则时间戳不同、各成一档，同样成立）
        std::fs::write(&target, "v2").unwrap();
        assert!(perform_backup_before_overwrite(&target, "timestamped").is_some());

        let bak_dir = dir.join("note.txt.bak.d");
        let backups: Vec<_> = std::fs::read_dir(&bak_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 2, "两次备份必须各自成档：{backups:?}");
        // 两份内容分别是 v1 / v2（顺序无关）
        let mut contents: Vec<String> = backups
            .iter()
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect();
        contents.sort();
        assert_eq!(contents, vec!["v1".to_owned(), "v2".to_owned()]);
        fs::remove_dir_all(&dir).ok();
    }
}
