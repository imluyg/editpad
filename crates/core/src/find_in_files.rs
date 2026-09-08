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

/// 目录遍历的深度封顶（P148）：`walk_dir` 是递归下降，万级深的目录树
/// （恶意构造或路径异常）可击穿扫描线程栈——栈溢出是进程 abort，
/// catch_unwind 兜不住。96 层远超正常工程嵌套（node_modules ≈15 层）；
/// 超深处静默跳过（与无权限子目录同口径，不算截断）。
pub const MAX_WALK_DEPTH: usize = 96;

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
    walk_dir(root, max_files, 0, &mut out);
    out.files.sort();
    out.files.dedup();
    out
}

fn walk_dir(dir: &Path, max_files: usize, depth: usize, out: &mut WalkOutput) {
    // P148：深度封顶——超深处整棵子树跳过（递归下降无界曾可击穿栈）
    if depth >= MAX_WALK_DEPTH {
        return;
    }
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
            walk_dir(&entry.path(), max_files, depth + 1, out);
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

/// P146：预编译查找器——FIF 扫描对至多 2 万文件逐个匹配，
/// 正则曾每文件重新编译一次（O(文件数) 次编译放大）。编译一次、
/// [`find_in_file_with`] 跨文件复用；字面模式零成本直通。
pub enum FifMatcher {
    Literal { query: String, case_sensitive: bool },
    Regex(fancy_regex::Regex),
}

impl FifMatcher {
    /// 组装查找器：正则模式编译失败返回 None（与 find_in_file 的
    /// 「正则无效返回空表」口径一致，调用方据此整批提前收尾）。
    pub fn build(query: &str, case_sensitive: bool, regex: bool) -> Option<Self> {
        if regex {
            Some(Self::Regex(crate::search::compile_regex(
                query, case_sensitive,
            )
            .ok()?))
        } else {
            Some(Self::Literal {
                query: query.to_owned(),
                case_sensitive,
            })
        }
    }
}

/// 单文件命中组装的复用版：语义与 [`find_in_file`] 完全一致
/// （整词只作用于字面模式），只是正则来自预编译的 [`FifMatcher`]。
pub fn find_in_file_with(
    text: &str,
    matcher: &FifMatcher,
    whole_word: bool,
    max_hits: usize,
) -> Vec<MatchPos> {
    let mut hits = match matcher {
        FifMatcher::Regex(re) => {
            crate::search::find_all_regex_compiled(text, re).unwrap_or_default()
        }
        FifMatcher::Literal { query, case_sensitive } => {
            let hits = find_all(text, query, *case_sensitive);
            if whole_word {
                filter_whole_word_text(text, hits)
            } else {
                hits
            }
        }
    };
    hits.truncate(max_hits);
    hits
}

/// [`crate::search::filter_whole_word`] 的 &str 版：只保留命中起点前
/// 一字符与终点后一字符**均非词字符**的命中（行首/行尾视为边界）；
/// 跨行命中（`len_chars` 含行界单元，行内边界语义不成立）一律保留。
///
/// 行界口径与 [`crate::search::find_all`] 对齐（P147）：切行复用同一
/// [`for_each_line`](crate::search::for_each_line)——行界全集为
/// `\r\n` / 孤立 `\r` / `\n` / VT / FF / NEL / LS / PS。曾自写
/// `find(['\r','\n'])` 切行：VT 等之后的命中行号大于过滤器自身推进的
/// 行号，被防御分支按「行号越界」整体丢弃（`ab\u{000B}cd` 查 `cd`
/// 整词返回 0 命中，应为 1）。`hits` 须按行序升序（`find_all` /
/// `find_all_regex` 的输出序）。
pub fn filter_whole_word_text(text: &str, hits: Vec<MatchPos>) -> Vec<MatchPos> {
    let mut out = Vec::with_capacity(hits.len());
    let mut pending = hits.into_iter().peekable();
    crate::search::for_each_line(text, |line_idx, content| {
        // 只为本行确有命中时切 Vec（无命中行零分配，大文件不再逐行小分配）
        if !matches!(pending.peek(), Some(h) if h.line == line_idx) {
            return;
        }
        let content: Vec<char> = content.chars().collect();
        while let Some(hit) = pending.peek().copied() {
            if hit.line < line_idx {
                pending.next();
                continue; // 行号回退：防御性丢弃（正常输出序不出现）
            }
            if hit.line > line_idx {
                break; // 本行无命中，交由后续行处理
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
    });
    // 行界之后仍有残留命中（行号越过末行）按防御口径丢弃
    out
}
