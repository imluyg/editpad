//! 会话快照底座（P29）：把「退出时未保存的标签页」写进快照区，
//! 配合清单文件形成可恢复的会话记录——P30 启动恢复 / P31 周期心跳的共同底座。
//!
//! **写序 = 先页文件、后清单**（write-ahead，§3 P29 第 2 条）：清单是提交点。
//! 页文件名带代次前缀（`s{代次}-t{页序}.snap`），新旧会话互不覆盖——
//! 任何中间时刻崩溃，磁盘上要么是旧清单完整、要么是新清单完整，
//! 绝不出现「清单说有页而内容缺半截」；半截代次只留孤儿文件，
//! 下次成功提交时被统一回收。
//!
//! 磁盘配额是内存预算总则的姊妹条款：快照总量超限按最旧优先淘汰
//! （本会话引用的文件永不淘汰）；明文快照的隐私风险对齐 P20 先例，
//! 由设置层 `enable_snapshots` 总开关兜底。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::error::CoreError;
use crate::saver::save_document_atomic;

/// 快照目录名（位于配置目录下）：`%APPDATA%\editpad\snapshot\`。
pub const SNAPSHOT_DIR_NAME: &str = "snapshot";
/// 会话清单文件名：快照区的提交点。
pub const MANIFEST_NAME: &str = "session.toml";
/// 页快照文件名模板前缀/后缀：`s{generation}-t{index}.snap`。
const PAGE_SUFFIX: &str = ".snap";

/// 快照总量配额（字节）：超过即按最旧优先淘汰页文件（§3 P29 第 5 条）。
/// 64MB ≈ 数十个常规文档的未保存改动量级。
pub const SNAPSHOT_QUOTA_BYTES: u64 = 64 * 1024 * 1024;
/// 单页参与周期心跳（P31）的大小上限：更大的页只在退出时写，
/// 不进入 10 秒级的心跳巡检（防大文档反复全量落盘）。
pub const HEARTBEAT_MAX_PAGE_BYTES: u64 = 16 * 1024 * 1024;

/// P31：单页是否参与本轮心跳重写（代次去重 + 大小节流的合并判定，
/// 纯函数便于测试）。
///
/// * 只覆盖**置脏**页——干净页的内容就在磁盘原件里，快照区无需副本；
/// * 大小节流：估算字节（app 层传 rope 真实字节数）超过
///   [`HEARTBEAT_MAX_PAGE_BYTES`] 的页不进心跳，只在退出时随全量
///   提交落盘（§3 P29 第 5 条的 IO 保护）；
/// * 代次去重：内容版本自上次心跳快照后没有推进（`last_snapshotted ==
///   Some(current)`）的页跳过——这是「仅写有变化的页」的判定核心；
///   从未参与过心跳的页（None）一律视为有变化。
pub fn heartbeat_page_selected(
    dirty: bool,
    approx_bytes: u64,
    last_snapshotted_version: Option<u64>,
    current_version: u64,
) -> bool {
    dirty
        && approx_bytes <= HEARTBEAT_MAX_PAGE_BYTES
        && last_snapshotted_version != Some(current_version)
}

/// 单个标签页的会话元数据（内容在 `file` 指向的页快照里）。
///
/// 字段口径与 app 层一致：光标行/列 0 基；滚动为像素值；
/// `path == None` 即未命名页（kind 的两种取值由此派生，不单设字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    /// 打开文件的完整路径；None = 未命名页。
    #[serde(default)]
    pub path: Option<String>,
    /// 未命名页序号（P25 全局单调口径），启动恢复时延续编号。
    #[serde(default)]
    pub untitled_num: Option<u64>,
    /// 该页退出时有未保存改动（= 内容快照存在且有效）。
    #[serde(default)]
    pub dirty: bool,
    /// 内容快照文件名（相对快照目录）；干净页为 None。
    #[serde(default)]
    pub file: Option<String>,
    /// 光标行（0 基）。
    #[serde(default)]
    pub cursor_line: usize,
    /// 光标列（0 基）。
    #[serde(default)]
    pub cursor_col: usize,
    /// 垂直滚动位置（像素）。
    #[serde(default)]
    pub scroll_top: f32,
    /// 水平滚动位置（像素）。
    #[serde(default)]
    pub scroll_left: f32,
    /// P134：本页自动换行覆盖（None = 跟随全局设置；C7 随会话保存）。
    #[serde(default)]
    pub wrap_override: Option<bool>,
    /// P134：本页字号覆盖（None = 跟随全局设置；C7 随会话保存）。
    #[serde(default)]
    pub font_size_override: Option<f32>,
}

/// 会话清单：一次快照的提交点。存在且可解析 = 有会话可恢复（P30 消费）；
/// 缺失或损坏一律按无会话处理。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionManifest {
    /// 快照代次：与本清单引用的全部页文件名前缀一致。
    pub generation: u64,
    /// 各标签页（顺序 = 标签条顺序）。
    pub tabs: Vec<SessionTab>,
    /// 退出时的活动页下标。
    pub active: usize,
    /// 未命名页编号计数器（P25），恢复时延续单调性、不复用号码。
    #[serde(default)]
    pub next_untitled: u64,
    /// 正常收尾标记（P30 孤儿检测判据）：false = 上次异常退出
    /// （P31 心跳写的中间清单即此形态），应弹恢复提示而非静默还原。
    #[serde(default)]
    pub clean_exit: bool,
}

/// 一次待写入会话的单页输入：`tab` 为元数据，`doc` 为内容。
///
/// `doc` 由调用方以 rope 结构共享克隆传入（O(1)），仅 `tab.dirty`
/// 为真时会被读取并分块原子落盘（全程无全文 String）。
#[derive(Debug, Clone)]
pub struct SessionPage {
    pub tab: SessionTab,
    pub doc: Document,
}

/// 默认快照目录；拿不到数据目录时返回 None（功能自动降级）。
/// P102：实例隔离——数据根目录按 exe 路径哈希分出实例（Windows 上
/// `%APPDATA%\editpad\instances\<实例键>\snapshot\`），每份拷贝各搞
/// 各的数据，见 [`crate::paths`]。
pub fn snapshot_dir() -> Option<PathBuf> {
    crate::paths::data_root().map(|d| d.join(SNAPSHOT_DIR_NAME))
}

/// 新代次号：纳秒级时钟。同进程连续多次快照 / 多进程并发都不会撞号。
fn fresh_generation() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1) // 时钟倒退的极端情形给个非零值，避免与解析逻辑纠缠
}

/// 页快照文件名：`s{代次}-t{页序}.snap`。
fn page_file_name(generation: u64, index: usize) -> String {
    format!("s{generation}-t{index}{PAGE_SUFFIX}")
}

/// 从页快照文件名解出代次号；非本项目的页快照命名返回 None。
fn parse_page_generation(file_name: &str) -> Option<u64> {
    let rest = file_name.strip_prefix('s')?;
    let dash = rest.find("-t")?;
    let (gen_str, tail) = rest.split_at(dash);
    let index_str = tail.strip_suffix(PAGE_SUFFIX)?.strip_prefix("-t")?;
    let all_digits =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(gen_str) || !all_digits(index_str) {
        return None;
    }
    gen_str.parse().ok()
}

/// 把一代会话写进快照目录（write-ahead，见模块注释）。返回写入的清单
/// （含分配的代次号与各页的实际快照文件名）。
///
/// 保证语义：任何一步失败都不碰旧清单——磁盘上仍保持上一个完整会话；
/// 已写出的半截页文件留待下次成功提交时被 GC 回收。
pub fn write_session(
    dir: &Path,
    pages: &[SessionPage],
    active: usize,
    next_untitled: u64,
) -> Result<SessionManifest, CoreError> {
    write_session_at(dir, fresh_generation(), pages, active, next_untitled, WriteMode::Full)
}

/// P31 心跳增量提交的单页输入。
///
/// 三种形态（心跳模式下按序判定）：
/// * `rewrite = true`：内容有变化 → 落新文件 `s{代次}-t{页序}.snap`；
/// * `rewrite = false` 且 `tab.file` 已填：内容未变 → 沿用旧文件名不重写；
/// * `rewrite = false` 且 `tab.file` 为 None：**超限节流页**——清单里保持
///   置脏记录但不落内容快照。读侧把「置脏而无文件」解释为「无可用快照」
///   （启动恢复按单页失败跳过并汇总提示），不得当作损坏或 panic。
pub struct HeartbeatPage {
    pub page: SessionPage,
    /// 本轮是否需要重写该页的内容文件。
    pub rewrite: bool,
}

/// P31 心跳增量提交：与 [`write_session`] 相同的 write-ahead 写序与失败
/// 原子性，两处差异——
///
/// * 清单以 `clean_exit = false` 落盘：运行中的中间态，正是 P30 孤儿检测
///   的判据（崩溃/杀进程后启动弹「未保存的工作区」恢复条）；
/// * **逐页意图**由 [`HeartbeatPage::rewrite`] 表达：变化页重写、未变页
///   复用旧文件、超限页只记账不落内容。复用的旧代名由 GC 与配额护栏
///   按「清单引用」保护，不会被当作残留回收。
pub fn write_heartbeat_session(
    dir: &Path,
    pages: &[HeartbeatPage],
    active: usize,
    next_untitled: u64,
) -> Result<SessionManifest, CoreError> {
    let flags: Vec<bool> = pages.iter().map(|p| p.rewrite).collect();
    let plain: Vec<SessionPage> = pages.iter().map(|p| p.page.clone()).collect();
    write_session_at(
        dir,
        fresh_generation(),
        &plain,
        active,
        next_untitled,
        WriteMode::Heartbeat(&flags),
    )
}

/// 提交模式：普通全量（退出流） vs 心跳增量（运行中周期巡检）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum WriteMode<'a> {
    /// 置脏页一律重写新文件；清单 clean_exit=true。
    Full,
    /// 心跳增量：携带每页的 rewrite 标记（见 [`HeartbeatPage`]）；
    /// 清单 clean_exit=false。
    Heartbeat(&'a [bool]),
}

/// [`write_session`] 的可注入代次版本：测试用固定代次即可预判全部
/// 页文件名，从而精确构造「第 N 页写盘失败」等中间态场景。
pub(crate) fn write_session_at(
    dir: &Path,
    generation: u64,
    pages: &[SessionPage],
    active: usize,
    next_untitled: u64,
    mode: WriteMode<'_>,
) -> Result<SessionManifest, CoreError> {
    fs::create_dir_all(dir).map_err(|source| CoreError::Write {
        path: dir.to_path_buf(),
        source,
    })?;

    // 写序前半：全部置脏页先各自原子落盘。任何一页失败即整体放弃。
    let mut tabs = Vec::with_capacity(pages.len());
    for (index, page) in pages.iter().enumerate() {
        let mut tab = page.tab.clone();
        if tab.dirty {
            // 心跳模式按逐页意图分流；普通模式一律写新文件
            let skip_rewrite = match mode {
                WriteMode::Full => false,
                WriteMode::Heartbeat(flags) => !flags.get(index).copied().unwrap_or(false),
            };
            if skip_rewrite {
                // file 已填 = 内容未变，沿用旧文件；file 为 None = 超限
                // 节流页，保持「置脏而无内容文件」的记账形态交给读侧容忍
                if tab.file.is_some() {
                    tabs.push(tab);
                    continue;
                }
            } else {
                let name = page_file_name(generation, index);
                save_document_atomic(&dir.join(&name), &page.doc)?;
                tab.file = Some(name);
            }
        } else {
            tab.file = None;
        }
        // 防御性消毒：滚动值理论上恒有限（clamp 层已保证），
        // 一旦混入 NaN 会让整个清单序列化失败，这里就地归零。
        if !tab.scroll_top.is_finite() {
            tab.scroll_top = 0.0;
        }
        if !tab.scroll_left.is_finite() {
            tab.scroll_left = 0.0;
        }
        tabs.push(tab);
    }

    // 提交点：清单原子替换。此前任何崩溃，磁盘上仍是旧会话。
    let manifest = SessionManifest {
        generation,
        tabs,
        active: active.min(pages.len().saturating_sub(1)),
        next_untitled,
        clean_exit: matches!(mode, WriteMode::Full),
    };
    let serialized = toml::to_string_pretty(&manifest).map_err(|e| CoreError::Write {
        path: dir.join(MANIFEST_NAME),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;
    crate::saver::write_atomic(&dir.join(MANIFEST_NAME), serialized.as_bytes()).map_err(
        |source| CoreError::Write {
            path: dir.join(MANIFEST_NAME),
            source,
        },
    )?;

    // 提交成功后回收残留页文件（上一代 / 中途失败孤儿）；
    // 本清单引用的文件（含心跳复用的旧代名）受保护不回收
    let referenced: std::collections::HashSet<String> = manifest
        .tabs
        .iter()
        .filter_map(|t| t.file.clone())
        .collect();
    gc_stale_pages(dir, generation, &referenced);
    // 配额护栏：超限时按最旧优先淘汰，永不触碰本会话引用的文件
    enforce_quota(dir, SNAPSHOT_QUOTA_BYTES);
    Ok(manifest)
}

/// 删除目录中不属于指定代次、也不被当前清单引用的页快照残留，
/// 返回删除的文件数。非页快照命名的文件一律不动（目录可能被用户另作他用）。
///
/// P31 起多带一个受保护名单：心跳复用的页文件带着旧代次前缀被新清单
/// 引用，「代次不同」不能再作为回收依据——引用关系才是活会话的真判据。
fn gc_stale_pages(dir: &Path, keep_generation: u64, protected: &std::collections::HashSet<String>) -> usize {
    let mut removed = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        match parse_page_generation(name) {
            Some(gen) if gen == keep_generation || protected.contains(name) => {}
            // 守卫内完成删除（clippy collapsible_match 合并形态）：语义与
            // 「臂体内 if」完全一致——仅删除成功才计数。
            Some(_) if fs::remove_file(&path).is_ok() => removed += 1,
            Some(_) | None => {}
        }
    }
    removed
}

/// 读会话清单；缺失或损坏一律返回 None（按无会话处理，绝不 panic）。
pub fn read_manifest(dir: &Path) -> Option<SessionManifest> {
    let text = fs::read_to_string(dir.join(MANIFEST_NAME)).ok()?;
    toml::from_str(&text).ok()
}

/// 读一页的内容快照；缺失/损坏/非 UTF-8 返回 None——
/// 单页失败不得阻断整体恢复（§3 P30 第 6 条的读侧前提）。
pub fn read_page(dir: &Path, tab: &SessionTab) -> Option<Document> {
    let name = tab.file.as_deref()?;
    let text = fs::read_to_string(dir.join(name)).ok()?;
    Some(Document::from_str(&text))
}

/// 清空整个快照区（「放弃更改并退出」= 连快照一起丢，§3 P29 第 4 条；
/// 以及总开关关闭后的清场，语义对齐 P20「只关开关不清数据等于没关」）。
/// 尽力而为：删不掉的条目忽略，不向上抛错。
pub fn clear_session(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// 配额护栏：页快照总量超过 `cap_bytes` 时按修改时间从旧到新淘汰，
/// 直到回到限额以内。当前清单引用的文件永不淘汰（它们是活会话）。
/// 返回被淘汰的文件数。
pub fn enforce_quota(dir: &Path, cap_bytes: u64) -> usize {
    let protected: std::collections::HashSet<String> = read_manifest(dir)
        .map(|m| m.tabs.iter().filter_map(|t| t.file.clone()).collect())
        .unwrap_or_default();

    // 不受保护页的 (修改时间, 大小, 路径)；受保护页只累计体积
    let mut evictable: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut protected_bytes = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if parse_page_generation(&name).is_none() {
            continue; // 清单与其他文件不参与淘汰
        }
        if protected.contains(&name) {
            protected_bytes += meta.len();
        } else {
            evictable.push((
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
                dir.join(&name),
            ));
        }
    }

    // 最旧的先出局
    evictable.sort_by_key(|(mtime, _, _)| *mtime);
    let mut total: u64 =
        protected_bytes + evictable.iter().map(|(_, len, _)| *len).sum::<u64>();
    let mut removed = 0;
    for (_, len, path) in &evictable {
        if total <= cap_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*len);
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 项目内落盘目录：系统 TEMP 在部分沙箱下不可写。
    fn scratch_dir(tag: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("snapshot-{tag}-{}", std::process::id()))
    }

    fn page(tab: SessionTab, text: &str) -> SessionPage {
        SessionPage {
            tab,
            doc: Document::from_str(text),
        }
    }

    fn named_tab(path: &str, dirty: bool) -> SessionTab {
        SessionTab {
            path: Some(path.to_owned()),
            untitled_num: None,
            dirty,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        }
    }

    fn untitled_tab(num: u64) -> SessionTab {
        SessionTab {
            path: None,
            untitled_num: Some(num),
            dirty: false,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        }
    }

    // ---------- 快照 roundtrip ----------

    #[test]
    fn session_roundtrip_named_dirty_untitled_and_clean_tabs() {
        let dir = scratch_dir("roundtrip");

        let mut first = untitled_tab(3);
        first.dirty = true;
        first.cursor_line = 2;
        first.cursor_col = 7;
        first.scroll_top = 12.5;

        let mut third = named_tab("C:/work/report.md", false);
        third.cursor_col = 4;

        let pages = vec![
            page(first.clone(), "第一行\r\n中文 🚀\r\n第三处编辑"),
            page(untitled_tab(4), ""),
            page(third.clone(), ""),
        ];
        let written = write_session(&dir, &pages, 0, 5).expect("写入应成功");

        assert!(written.clean_exit, "正常退出写的清单必须带收尾标记");
        assert_eq!(written.active, 0);
        assert_eq!(written.next_untitled, 5);

        let read = read_manifest(&dir).expect("清单必须可解析");
        assert_eq!(read, written, "清单 roundtrip 一致");
        assert_eq!(read.tabs.len(), 3);

        // 置脏未命名页：元数据与内容都完整回来（CRLF/emoji 逐字节保真）
        let back_first = &read.tabs[0];
        assert!(back_first.path.is_none());
        assert_eq!(back_first.untitled_num, Some(3));
        assert!(back_first.dirty);
        assert_eq!((back_first.cursor_line, back_first.cursor_col), (2, 7));
        assert_eq!(back_first.scroll_top, 12.5);
        let doc = read_page(&dir, back_first).expect("置脏页必须有内容");
        assert_eq!(doc.to_text(), "第一行\r\n中文 🚀\r\n第三处编辑");
        assert!(
            doc.to_text().contains("\r\n"),
            "CRLF 必须原样保留（不做行尾归一）"
        );

        // 干净未命名页：无内容文件
        assert!(!read.tabs[1].dirty);
        assert_eq!(read.tabs[1].file, None);
        assert!(read_page(&dir, &read.tabs[1]).is_none());

        // 干净命名页：路径记住了，但没有内容文件
        assert_eq!(read.tabs[2].path.as_deref(), Some("C:/work/report.md"));
        assert_eq!(read.tabs[2].file, None);

        clear_session(&dir);
    }

    // ---------- 缺失/损坏按无会话处理 ----------

    #[test]
    fn missing_or_corrupt_manifest_reads_as_no_session_without_panic() {
        let dir = scratch_dir("no-manifest");
        fs::create_dir_all(&dir).unwrap();

        // 目录为空 = 无会话
        assert!(read_manifest(&dir).is_none());

        // 清单损坏（半截 TOML）同样按无会话处理
        fs::write(dir.join(MANIFEST_NAME), "tabs = [ { broken").unwrap();
        assert!(read_manifest(&dir).is_none(), "损坏清单必须被吞掉而不是 panic");

        // 页文件缺失 → 单页读取失败返回 None
        let mut t = named_tab("C:/x.txt", true);
        t.file = Some("s1-t0.snap".to_owned());
        assert!(read_page(&dir, &t).is_none());

        // 页文件损坏（非 UTF-8 字节）→ 同样视为缺失而非 panic
        let real = dir.join("s1-t0.snap");
        fs::write(&real, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_page(&dir, &t).is_none());

        clear_session(&dir);
    }

    // ---------- 代次回收（write-ahead 孤儿清理） ----------

    #[test]
    fn recommit_recycles_previous_generation_pages_even_after_crash_gaps() {
        let dir = scratch_dir("gc");

        // 第一代：一个置脏页
        let mut t1 = named_tab("C:/a.txt", true);
        t1.dirty = true;
        let first = write_session(&dir, &[page(t1, "gen A")], 0, 2).unwrap();
        let gen_a_file = first.tabs[0].file.clone().unwrap();

        // 第二代提交后：上一代的页文件必须被回收，只留本代引用的文件
        let mut t2 = named_tab("C:/b.txt", true);
        t2.dirty = true;
        let second = write_session(&dir, &[page(t2, "gen B")], 0, 2).unwrap();
        assert_ne!(first.generation, second.generation, "代次号必须推进");
        let remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| parse_page_generation(e.file_name().to_str().unwrap()).is_some())
            .collect();
        assert_eq!(remaining.len(), 1, "旧代页文件必须被回收");
        assert_eq!(
            remaining[0].file_name().to_str().unwrap(),
            second.tabs[0].file.as_deref().unwrap()
        );
        assert!(read_page(&dir, &second.tabs[0]).unwrap().to_text() == "gen B");

        // 「崩溃中间态」模拟：清单丢失 + 遗留孤儿页文件（模拟写序中途崩），
        // 下一次成功提交时孤儿一并回收，不会越积越多
        fs::remove_file(dir.join(MANIFEST_NAME)).unwrap();
        fs::write(dir.join("s999-t9.snap"), "orphan").unwrap();
        assert!(read_manifest(&dir).is_none());
        let third = write_session(
            &dir,
            &[page({
                let mut t = named_tab("C:/c.txt", true);
                t.dirty = true;
                t
            }, "gen C")],
            0,
            2,
        )
        .unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|n| parse_page_generation(n).is_some())
            .collect();
        assert_eq!(names, vec![third.tabs[0].file.clone().unwrap()], "孤儿页必须清干净");

        // 第一代的旧文件名确实属于被回收之列（防呆：确认它曾真实存在过）
        assert!(parse_page_generation(&gen_a_file).is_some());
        assert_ne!(gen_a_file, second.tabs[0].file.clone().unwrap());

        clear_session(&dir);
    }

    // ---------- 失败原子性（write-ahead 后半失败不碰旧清单） ----------

    #[test]
    fn failed_midway_write_keeps_previous_manifest_intact() {
        let dir = scratch_dir("atomic");
        const GEN: u64 = 42; // 注入固定代次，页文件名因此可预判

        // 先建立一个完好会话
        let baseline = write_session_at(
            &dir,
            GEN,
            &[page({
                let mut t = named_tab("C:/keep.txt", true);
                t.dirty = true;
                t
            }, "keep")],
            0,
            1,
            WriteMode::Full,
        )
        .unwrap();
        let before = read_manifest(&dir).unwrap();
        assert_eq!(before, baseline);

        // 构造「第 2 页写盘必败」：占住其目标文件位（临时名带 pid+序号
        // 不可预判，P146；目录占住 rename 目标同样触发中途失败）
        let blocker = dir.join(format!("s{}-t1.snap", GEN + 1));
        fs::create_dir_all(&blocker).unwrap();

        let doomed = write_session_at(
            &dir,
            GEN + 1,
            &[
                page(named_tab("C:/ok.txt", true), "written first"),
                page(named_tab("C:/doomed.txt", true), "must not land"),
            ],
            0,
            1,
            WriteMode::Full,
        );
        assert!(doomed.is_err(), "第 2 页写盘应失败");

        // 旧清单原封不动：磁盘上仍是上一个完整会话
        let after = read_manifest(&dir).unwrap();
        assert_eq!(after, before, "失败的提交不得破坏既有会话");
        assert_eq!(
            read_page(&dir, &after.tabs[0]).unwrap().to_text(),
            "keep",
            "旧页内容不受影响"
        );

        // 半截代次留下的孤儿页在下次成功提交时被回收
        fs::remove_dir_all(&blocker).unwrap();
        let recovered = write_session_at(
            &dir,
            GEN + 2,
            &[page(named_tab("C:/fresh.txt", true), "fresh")],
            0,
            1,
            WriteMode::Full,
        )
        .unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|n| parse_page_generation(n).is_some())
            .collect();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], recovered.tabs[0].file.as_deref().unwrap());

        clear_session(&dir);
    }

    // ---------- 配额护栏 ----------

    #[test]
    fn quota_evicts_oldest_unprotected_first_and_spares_current_session() {
        let dir = scratch_dir("quota");
        let big = "a".repeat(200_000); // 200KB

        // 两代各一个 200KB 置脏页：第二代提交后第一代已被 GC，
        // 再手工放一个更旧的「历史遗留」大页制造超额
        let mut t = named_tab("C:/cur.txt", true);
        t.dirty = true;
        let session = write_session(&dir, &[page(t, &big)], 0, 1).unwrap();
        let stale = dir.join("s1-t0.snap"); // 伪造最老代次的残留页
        fs::write(&stale, &big).unwrap();

        // 配额 300KB：总额 400KB → 必须淘汰不受保护的最旧者（伪造页）
        let removed = enforce_quota(&dir, 300_000);
        assert_eq!(removed, 1, "恰好淘汰一个最旧页");
        assert!(!stale.exists());
        // 本会话引用的文件安然无恙，内容仍可读
        let kept = read_page(&dir, &session.tabs[0]).unwrap();
        assert_eq!(kept.to_text(), big);

        // 配额远小于本会话自身：受保护文件也不得被淘汰
        let removed = enforce_quota(&dir, 1);
        assert_eq!(removed, 0, "活会话的页永不淘汰");
        assert!(read_manifest(&dir).is_some());

        // 宽松配额：什么都不动
        assert_eq!(enforce_quota(&dir, SNAPSHOT_QUOTA_BYTES), 0);

        clear_session(&dir);
    }

    // ---------- P31 周期快照心跳 ----------

    #[test]
    fn heartbeat_page_selection_dedupes_by_version_and_throttles_by_size() {
        const CAP: u64 = HEARTBEAT_MAX_PAGE_BYTES;
        // 置脏是前提
        assert!(!heartbeat_page_selected(false, 100, None, 3), "干净页永不参与");
        // 从未快照过（None）的置脏页必须参与——未命名页的首个兜底
        assert!(heartbeat_page_selected(true, 100, None, 0));
        // 代次去重：版本没推进就跳过；推进了才重写
        assert!(!heartbeat_page_selected(true, 100, Some(3), 3));
        assert!(heartbeat_page_selected(true, 100, Some(2), 3));

        // 大小节流：恰好压线允许（>上限才出局），超一字节即跳过
        assert!(heartbeat_page_selected(true, CAP, Some(1), 2), "恰好等于上限应允许");
        assert!(!heartbeat_page_selected(true, CAP + 1, None, 9), "超限页只在退出时写");
    }

    #[test]
    fn heartbeat_commit_reuses_unchanged_pages_and_flags_clean_exit_false() {
        let dir = scratch_dir("heartbeat");

        // 第一代（普通全量提交）：两页都置脏，各自拿到内容文件
        let mut a = named_tab("C:/keep.txt", true);
        a.cursor_col = 3;
        let mut b = named_tab("C:/churn.txt", true);
        b.dirty = true;
        let first = write_session(&dir, &[page(a.clone(), "stable"), page(b, "v1")], 0, 1).unwrap();
        let keep_file = first.tabs[0].file.clone().unwrap();
        assert!(first.clean_exit, "退出流的全量提交必须带收尾标记");

        // 心跳提交：页 A 内容未变 → 复用旧文件名（不重写）；
        // 页 B 又编辑过 → 重写；页 C 超限节流（rewrite=false 且无旧文件）
        // → 保持置脏记账但不落内容；清单必须是 clean_exit=false 的中间态
        let mut a_reuse = a.clone();
        a_reuse.file = Some(keep_file.clone()); // 调用方从页上的心跳账目预填
        let mut c_throttled = named_tab("C:/huge.bin", true);
        c_throttled.cursor_line = 9;
        let second = write_heartbeat_session(
            &dir,
            &[
                HeartbeatPage { page: page(a_reuse, "stable"), rewrite: false },
                HeartbeatPage { page: page(named_tab("C:/churn.txt", true), "v2"), rewrite: true },
                HeartbeatPage { page: page(c_throttled.clone(), "HUGE"), rewrite: false },
            ],
            0,
            1,
        )
        .unwrap();
        assert!(!second.clean_exit, "心跳写的中间清单必须置 clean_exit=false");
        assert_eq!(
            second.tabs[0].file.as_deref(),
            Some(keep_file.as_str()),
            "未变化页必须复用旧文件名"
        );
        assert_ne!(
            second.tabs[1].file, first.tabs[1].file,
            "变化页必须落新文件"
        );
        assert_eq!(second.tabs[2].file, None, "超限页不得落内容文件");
        assert!(second.tabs[2].dirty && second.tabs[2].cursor_line == 9,
            "超限页保持置脏记账与元数据");

        // 复用的旧代名文件在 GC 后仍然健在且内容可读——「引用关系」
        // 取代「代次相同」成为回收判据的直接后果
        assert_eq!(
            read_page(&dir, &second.tabs[0]).unwrap().to_text(),
            "stable",
            "复用文件不得被 GC 当残留回收"
        );
        assert_eq!(read_page(&dir, &second.tabs[1]).unwrap().to_text(), "v2");
        assert!(
            read_page(&dir, &second.tabs[2]).is_none(),
            "超限页无内容快照可读（读侧按无可用快照处理）"
        );

        // 磁盘上恰好只剩新清单引用的两个文件：B 的旧文件已被回收，
        // 超限页从未产生过文件
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|n| parse_page_generation(n).is_some())
            .collect();
        names.sort();
        let mut expected: Vec<String> =
            second.tabs.iter().filter_map(|t| t.file.clone()).collect();
        expected.sort();
        assert_eq!(names, expected, "GC 后应只剩活会话引用的页文件");

        // 心跳之后照常退出：全量提交把 clean_exit 翻回 true，
        // 且此刻起 A 页转干净 → 只剩 B、C 两个内容文件（含超限页——
        // 「只在退出时写」的最终落点）
        let third = write_session(
            &dir,
            &[
                page(named_tab("C:/keep.txt", false), ""),
                page(named_tab("C:/churn.txt", true), "v3"),
                page(c_throttled, "HUGE-FINAL"),
            ],
            0,
            1,
        )
        .unwrap();
        assert!(third.clean_exit);
        assert_eq!(third.tabs[0].file, None, "已保存页退出时无需内容文件");
        assert_eq!(
            read_page(&dir, &third.tabs[2]).unwrap().to_text(),
            "HUGE-FINAL",
            "超限页的内容由退出流全量提交兜底"
        );
        assert!(read_manifest(&dir).unwrap().clean_exit);

        clear_session(&dir);
    }

    // ---------- 文件名解析契约 ----------

    #[test]
    fn page_file_name_parsing_roundtrips_and_rejects_lookalikes() {
        assert_eq!(parse_page_generation(&page_file_name(7, 0)), Some(7));
        assert_eq!(parse_page_generation(&page_file_name(1_234_567_890, 12)), Some(1_234_567_890));
        // 相似但非本项目的命名必须拒绝
        assert_eq!(parse_page_generation("session.toml"), None);
        assert_eq!(parse_page_generation("sabc-t0.snap"), None);
        assert_eq!(parse_page_generation("s7-t.snap"), None);
        assert_eq!(parse_page_generation("s7-x0.snap"), None);
        assert_eq!(parse_page_generation("s7-t0.bak"), None);
        assert_eq!(parse_page_generation(""), None);
        // 往返：生成→解析得到的代次一致
        for gen in [1u64, 42, u64::MAX] {
            let name = page_file_name(gen, 3);
            assert_eq!(parse_page_generation(&name), Some(gen), "{name}");
        }
    }
}
