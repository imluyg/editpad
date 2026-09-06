//! P133：链接识别纯逻辑（路线图 E2，同类编辑器 hyperlink hotspot 同款）。
//!
//! 三类目标：http(s) URL 与 `file:///` URI（交系统默认处理打开）；
//! 带可选 `:行号` 后缀的文件路径（编辑器内打开，`src/main.rs:42` /
//! `C:\log\run.txt:10` 语法）。识别口径：
//! * token = 行内以空白分隔的连续非空白段（从给定列向两侧扩展）；
//! * URL：前缀判定 + 尾部收尾标点剥离（`,.` 等与中文句读）；
//! * 路径：必须含路径分隔符 `\` / `/`，且**磁盘存在**（绝对与相对一律
//!   经存在性判定——纯结构判定会把 `2026/09/02` 这类日期当路径，存在性
//!   一票否决最省心）；存在性/相对解析由调用方注入闭包（可含 base_dir
//!   回退），本模块不碰 IO。
//! * 已知取舍：含空格的路径被空白分词截断（`C:\Program Files\...`
//!   不可整段识别，同类编辑器同款局限）。

use std::path::PathBuf;

/// 点击解析出的目标（`Message::LinkClicked` 载荷）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// 交系统默认处理打开（浏览器/资源管理器）
    Url(String),
    /// 编辑器内打开；`line` = 1 起行号（`file.c:42` 的 `42`）
    File { path: PathBuf, line: Option<u32> },
}

/// 从行文本与列（行内字符索引，与 `hit_test`/光标列同口径）识别链接。
/// `resolve_path` 注入路径存在性判定：Some(解析后的绝对/相对路径) = 该
/// 路径可作为链接，None = 不算。返回 `(c0, c1, target)`，`[c0, c1)` 为
/// **链接本体**的字符区间（供下划线绘制；URL 前面的紧贴前文不算）。
pub(crate) fn link_at(
    text: &str,
    col: usize,
    resolve_path: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<(usize, usize, LinkTarget)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    // 从 col 向两侧扩展非空白段（col 允许 == len：行尾点击命中末 token）
    let at = col.min(chars.len() - 1);
    if chars[at].is_whitespace() {
        return None;
    }
    let mut c0 = at;
    while c0 > 0 && !chars[c0 - 1].is_whitespace() {
        c0 -= 1;
    }
    let mut c1 = at + 1;
    while c1 < chars.len() && !chars[c1].is_whitespace() {
        c1 += 1;
    }
    let token: String = chars[c0..c1].iter().collect();
    let (off, len, target) = classify(&token, resolve_path)?;
    Some((c0 + off, c0 + off + len, target))
}

/// token 分类。URL 在 token 内**任意位置**找 scheme（中文「详见URL」无
/// 空格分词，前文紧贴 scheme）；路径候选走存在性注入。返回
/// `(token 内字符偏移, 字符长度, target)`。
fn classify(
    token: &str,
    resolve_path: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<(usize, usize, LinkTarget)> {
    if let Some(byte_pos) = token.find("https://").or_else(|| token.find("http://")) {
        // URL 恒为 ASCII（非 ASCII 属百分号编码前的原文）——中文句读
        // 会并入 token，先在首个非 ASCII 字符处截断
        let rest = &token[byte_pos..];
        let ascii_end = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        // 再剥尾部收尾标点（"see https://a.b/c, end" 的逗号不成链）
        let t = rest[..ascii_end].trim_end_matches(|c| {
            matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | '"' | '\'')
        });
        // 剥完只剩 scheme（"https://"）不成链
        if t.ends_with("://") {
            return None;
        }
        let start = token[..byte_pos].chars().count();
        return Some((start, t.chars().count(), LinkTarget::Url(t.to_string())));
    }
    if token.starts_with("file://") {
        let n = token.chars().count();
        return Some((0, n, LinkTarget::Url(token.to_string())));
    }
    // 路径候选：必须含分隔符
    if !token.contains('\\') && !token.contains('/') {
        return None;
    }
    let t = token.trim_end_matches(['.', ',', ';', ':', '。', '，', '；', '：']);
    if t.is_empty() {
        return None;
    }
    // 行号后缀：最后一个 ':' 后全数字即拆出（1 起）。盘符冒号排除——
    // 单字符头（"C"）后的冒号是盘符而非行号分隔
    let (t, line) = match t.rfind(':') {
        Some(pos) if pos > 1 => {
            let (head, digits) = (&t[..pos], &t[pos + 1..]);
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                match digits.parse::<u32>() {
                    Ok(n) => (head, Some(n)),
                    Err(_) => (t, None), // 溢出 u32 的数字串不当行号
                }
            } else {
                (t, None)
            }
        }
        _ => (t, None),
    };
    if t.is_empty() || (!t.contains('\\') && !t.contains('/')) {
        return None;
    }
    resolve_path(t).map(|path| (0, t.chars().count(), LinkTarget::File { path, line }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入：一律不存在（URL 场景用，不触碰磁盘）
    const NO_PATH: fn(&str) -> Option<PathBuf> = |_| None;

    /// 注入：白名单内的路径视为存在（存在性判定的确定性替身）
    fn with_paths(paths: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |p: &str| {
            if paths.contains(&p) {
                Some(PathBuf::from(p))
            } else {
                None
            }
        }
    }

    #[test]
    fn url_detected_and_trailing_ascii_punct_trimmed() {
        // 列落在 URL 内任意位置都命中同一 URL 本体（尾部逗号已剥离）
        let text = "see https://a.b/c, end";
        for col in [4usize, 6, 15] {
            let (c0, c1, t) = link_at(text, col, &NO_PATH).expect("应识别出 URL");
            assert_eq!(&text[c0..c1], "https://a.b/c");
            assert_eq!(t, LinkTarget::Url("https://a.b/c".to_string()));
        }
        // 裸 scheme 不成链
        assert!(link_at("https://", 2, &NO_PATH).is_none());
        // http 同款
        let (_, _, t) = link_at("http://x.io/path?q=1.", 0, &NO_PATH).unwrap();
        assert_eq!(t, LinkTarget::Url("http://x.io/path?q=1".to_string()));
    }

    #[test]
    fn url_cut_at_first_non_ascii_cjk_adjacent() {
        // 中文句读无空格分词：token 会吞进整段后文，URL 须在首个
        // 非 ASCII 字符处截断；且 scheme 前紧贴的中文前文不算链接本体
        let text = "详见https://e.io/x。其中";
        let slice = |s: &str, c0: usize, c1: usize| -> String {
            s.chars().skip(c0).take(c1 - c0).collect()
        };
        for col in [0usize, 1, 5, 12] {
            let (c0, c1, t) = link_at(text, col, &NO_PATH).expect("应识别出 URL");
            assert_eq!(slice(text, c0, c1), "https://e.io/x");
            assert_eq!(t, LinkTarget::Url("https://e.io/x".to_string()));
        }
    }

    #[test]
    fn file_uri_and_path_with_line_number() {
        // file:/// 交系统打开
        let (_, _, t) = link_at("file:///C:/logs", 2, &NO_PATH).unwrap();
        assert_eq!(t, LinkTarget::Url("file:///C:/logs".to_string()));
        // 路径 :行号 拆分（1 起）
        let resolve = with_paths(&["C:\\log\\run.txt"]);
        let (_, _, t) = link_at("err C:\\log\\run.txt:10 boom", 5, &resolve).unwrap();
        assert_eq!(
            t,
            LinkTarget::File { path: PathBuf::from("C:\\log\\run.txt"), line: Some(10) }
        );
        // 无行号
        let (_, _, t) = link_at("err C:\\log\\run.txt boom", 5, &resolve).unwrap();
        assert_eq!(
            t,
            LinkTarget::File { path: PathBuf::from("C:\\log\\run.txt"), line: None }
        );
    }

    #[test]
    fn path_requires_existence_and_separator() {
        // 相对路径存在 → File
        let resolve = with_paths(&["src/main.rs"]);
        let (_, _, t) = link_at("at src/main.rs:42 end", 3, &resolve).unwrap();
        assert_eq!(
            t,
            LinkTarget::File { path: PathBuf::from("src/main.rs"), line: Some(42) }
        );
        // 不存在 → None（日期 "2026/09/02" 等假阳性一票否决）
        assert!(link_at("dated 2026/09/02 ok", 6, &NO_PATH).is_none());
        // 无分隔符的词不进路径分支
        assert!(link_at("plainword", 0, &with_paths(&["plainword"])).is_none());
        // 盘符歧义 "C:42"：冒号在盘符位，不拆行号，且无分隔符不成链
        assert!(link_at("C:42", 0, &with_paths(&["C:42"])).is_none());
    }

    #[test]
    fn token_boundaries_and_edge_columns() {
        // 空白列不成链
        assert!(link_at("a https://x.io b", 1, &NO_PATH).is_none());
        // 空行 / 越界列安全
        assert!(link_at("", 0, &NO_PATH).is_none());
        // 列 == 行尾（点击行尾右侧空白）仍命中末 token
        let text = "go https://x.io/a";
        let (_, _, t) = link_at(text, text.chars().count(), &NO_PATH).unwrap();
        assert_eq!(t, LinkTarget::Url("https://x.io/a".to_string()));
    }
}
