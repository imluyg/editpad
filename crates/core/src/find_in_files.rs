//! A8「在文件中查找」core 纯逻辑（设计 docs/find-in-files-design.md）。
//!
//! 三件套：[`walk_files`]（目录遍历，排除规则 + 封顶 + 字典序确定性）、
//! [`find_in_file`]（单文件命中组装：字面/正则/整词）、
//! [`filter_whole_word_text`]（整词过滤的 &str 口径——补齐
//! [`crate::search::filter_whole_word`] 只有 Document 版的缺口）。
//!
//! 已知取舍（设计 §1 披露）：遍历不跟随目录符号链接（防环），符号链
//! 接文件一律跳过（Windows 场景罕见，换无环保证与语义简单）；排除
//! 规则只作用于**目录**——`.` 开头的隐藏文件（如 `.gitignore`）参与
//! 扫描，文件内容本身常是检索对象。

use crate::search::{find_all, find_all_regex, is_word_char, MatchPos};
use std::path::{Path, PathBuf};

/// 遍历时跳过的噪音目录名（「遵守忽略规则」的一期口径；.gitignore
/// 解析不做——离线零依赖约束，见设计 §1）。
pub const IGNORED_DIRS: &[&str] = &[".git", "node_modules", "target", "dist"];

/// 单文件扫描的大小豁免（字节）：超过即跳过该文件（与保存备份
/// 64 MB 豁免同口径）。执行器在装载前以 metadata 预检。
pub const MAX_SCAN_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 遍历结果：文件按全路径**字典序**（跨目录确定性——read_dir 顺序平
/// 台相关，展示与测试都要求稳定）；`truncated` = 实际遇到的文件数超
/// 过封顶（超出者不收入，与「截断明示」的 UI 口径对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOutput {
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

/// 递归遍历 `root` 下的普通文件。跳过：`.` 开头的隐藏目录、
/// [`IGNORED_DIRS`] 中的目录、一切符号链接（目录防环，文件防出界）。
/// 无权限 / 已消失的子目录静默跳过（不中断整个扫描）。
///
/// `max_files` = 文件数封顶（0 = 不收任何文件，遇到文件即判截断）。
pub fn walk_files(root: &Path, max_files: usize) -> WalkOutput {
    let mut out = WalkOutput { files: Vec::new(), truncated: false };
    walk_dir(root, max_files, &mut out);
    out.files.sort();
    out.files.dedup();
    out
}

fn walk_dir(dir: &Path, max_files: usize, out: &mut WalkOutput) {
    // read_dir 失败（无权限/已消失）：跳过该子树，不中断
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 隐藏目录（`.` 开头）与噪音目录常量：整棵子树跳过
            if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk_dir(&entry.path(), max_files, out);
        } else if ft.is_file() {
            if out.files.len() >= max_files {
                out.truncated = true; // 封顶后仍遇到真实文件 → 截断明示
                return;
            }
            out.files.push(entry.path());
        }
        // 符号链接（file_type 既非 dir 也非 file）：一律跳过（防环）
    }
}

/// 单文件命中组装（设计 §3.2）。字面走 [`find_all`]、正则走
/// [`find_all_regex`]；整词只作用于字面模式（正则的边界语义由模式自
/// 身表达，与编辑器查找栏同口径）；正则无效返回空表（UI 层启动扫描
/// 前已预校验，此处兜底不 panic）。`max_hits` = 单文件命中封顶，
/// 超出截断（截断事实由调用方按总量口径统一明示）。
pub fn find_in_file(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex: bool,
    whole_word: bool,
    max_hits: usize,
) -> Vec<MatchPos> {
    let mut hits = if regex {
        find_all_regex(text, query, case_sensitive).unwrap_or_default()
    } else {
        let hits = find_all(text, query, case_sensitive);
        if whole_word {
            filter_whole_word_text(text, hits)
        } else {
            hits
        }
    };
    hits.truncate(max_hits);
    hits
}

/// [`crate::search::filter_whole_word`] 的 &str 版：只保留命中起点前
/// 一字符与终点后一字符**均非词字符**的命中（行首/行尾视为边界）；
/// 跨行命中（`len_chars` 含行界单元，行内边界语义不成立）一律保留。
///
/// 行界口径与 [`find_all`] 对齐：`\r\n` / 孤立 `\r` / `\n` 皆一个行
/// 界，行内容不含行尾换行单元（P78 同哲学）。`hits` 须按行序升序
/// （`find_all` / `find_all_regex` 的输出序）；单遍切行消费，
/// O(text 长度 + 命中数)。
pub fn filter_whole_word_text(text: &str, hits: Vec<MatchPos>) -> Vec<MatchPos> {
    let mut out = Vec::with_capacity(hits.len());
    let mut pending = hits.into_iter().peekable();
    let mut line_idx = 0usize;
    let mut rest = text;
    loop {
        // 当前行内容 = 到首个行界字符前（\r 与 \n 均是行界起点）
        let content_end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let content: Vec<char> = rest[..content_end].chars().collect();
        // 消费属于本行的全部命中（行号回退属防御性分支，正常输出序
        // 不会出现——出现时丢弃该命中保持单调推进）
        while let Some(hit) = pending.peek().copied() {
            if hit.line < line_idx {
                pending.next();
                continue;
            }
            if hit.line > line_idx {
                break;
            }
            pending.next();
            let end = hit.col + hit.len_chars;
            if end > content.len() {
                out.push(hit); // 跨行命中：保留
                continue;
            }
            let before_ok = hit.col == 0 || !is_word_char(content[hit.col - 1]);
            let after_ok = end == content.len() || !is_word_char(content[end]);
            if before_ok && after_ok {
                out.push(hit);
            }
        }
        if pending.peek().is_none() {
            break;
        }
        if content_end >= rest.len() {
            break; // 无更多行；剩余命中（行号越界）按防御口径丢弃
        }
        // 推进到下一行：\r\n 整体吞掉，孤立 \r 与 \n 各吞一个字符
        if rest[content_end..].starts_with("\r\n") {
            rest = &rest[content_end + 2..];
        } else {
            rest = &rest[content_end + 1..];
        }
        line_idx += 1;
    }
    out
}
