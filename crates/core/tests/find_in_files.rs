//! A8「在文件中查找」Phase 1：core 三纯函数单测（设计
//! docs/find-in-files-design.md §7 Phase 1）。
//!
//! 钉住契约：walk_files 的排除规则（隐藏目录/噪音目录常量）、文件数
//! 封顶与截断明示、跨目录字典序确定性；find_in_file 的字面/正则/整
//! 词/单文件封顶组装；filter_whole_word_text 的行界口径（\r\n / 孤立
//! \r / \n 皆行界）、行首尾边界、跨行命中保留。

use editpad_core::{filter_whole_word_text, find_all, find_in_file, walk_files, WalkOutput,
    IGNORED_DIRS};
use std::path::PathBuf;

/// 测试专用临时目录（进程级唯一子目录，测试尾部自行清理）——
/// boundary_inputs.rs 的 scratch 同款。
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "editpad-fif-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&root); // 上次残留清场
        std::fs::create_dir_all(&root).expect("创建临时目录失败");
        Scratch { root }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建子目录失败");
        }
        std::fs::write(&path, content).expect("写文件失败");
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(&path).expect("创建子目录失败");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn names(out: &WalkOutput) -> Vec<String> {
    out.files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect()
}

// ---------- walk_files ----------

#[test]
fn walk_files_skips_hidden_and_ignored_dirs_and_is_deterministic() {
    let s = Scratch::new("walk");
    s.write("a.txt", "hello");
    s.write("sub/b.txt", "world");
    s.write(".hidden/h.txt", "secret");
    s.write("node_modules/n.js", "js");
    s.write("target/t.rs", "rust");
    s.dir("empty_dir");

    let out = walk_files(&s.root, 100);
    assert_eq!(
        names(&out),
        vec!["a.txt".to_owned(), "b.txt".to_owned()],
        "隐藏目录与噪音目录整棵跳过；文件按字典序"
    );
    assert!(!out.truncated);

    // 确定性：重复调用结果逐路径相等
    let again = walk_files(&s.root, 100);
    assert_eq!(out, again, "跨目录字典序确定性（read_dir 顺序无关）");

    // 隐藏文件（`.` 开头的文件）参与扫描：排除规则只作用于目录，
    // .gitignore 这类文件内容本身常是检索对象（模块注释钉契约）
    s.write(".dotfile", "content");
    let out = walk_files(&s.root, 100);
    assert!(names(&out).contains(&".dotfile".to_owned()));
}

#[test]
fn walk_files_cap_and_truncated_flag() {
    let s = Scratch::new("cap");
    s.write("1.txt", "");
    s.write("2.txt", "");
    s.write("3.txt", "");

    // 恰好等于封顶：不判截断
    let out = walk_files(&s.root, 3);
    assert_eq!(out.files.len(), 3);
    assert!(!out.truncated, "文件数恰等于封顶 = 未截断");

    // 超过封顶：截断明示，只保留前 max 个
    let out = walk_files(&s.root, 2);
    assert_eq!(out.files.len(), 2);
    assert!(out.truncated);

    // 封顶 0：不收任何文件，遇文件即截断
    let out = walk_files(&s.root, 0);
    assert!(out.files.is_empty());
    assert!(out.truncated);

    // 空目录：无文件无截断
    let empty = Scratch::new("cap-empty");
    let out = walk_files(&empty.root, 10);
    assert!(out.files.is_empty() && !out.truncated);
}

#[test]
fn walk_files_nested_dirs_recursed_and_root_file_only_tree() {
    let s = Scratch::new("deep");
    s.write("x/y/z/deep.txt", "deep");
    let out = walk_files(&s.root, 10);
    assert_eq!(names(&out), vec!["deep.txt".to_owned()]);

    // 不存在的根：空结果不 panic（无权限/已消失同口径静默跳过）
    let missing = s.root.join("no_such_dir");
    let out = walk_files(&missing, 10);
    assert!(out.files.is_empty() && !out.truncated);
}

// ---------- find_in_file ----------

#[test]
fn find_in_file_literal_case_and_whole_word() {
    let text = "Word word words wordy Word\nword";

    // 字面不分大小写：6 处
    let hits = find_in_file(text, "word", false, false, false, 100);
    assert_eq!(hits.len(), 6);
    // 字面区分大小写：4 处（word/words/wordy/word 的四个小写前缀）
    assert_eq!(find_in_file(text, "word", true, false, false, 100).len(), 4);

    // 整词（字面模式才生效）：不分大小写下保留 4 处独立词——
    // Word(0,0)、word(0,5)、Word(0,21)、word(1,0)；words/wordy 剔除
    let hits = find_in_file(text, "word", false, false, true, 100);
    assert_eq!(hits.len(), 4, "words/wordy 的命中被整词过滤剔除");
    assert_eq!(
        hits.iter().map(|h| (h.line, h.col)).collect::<Vec<_>>(),
        vec![(0, 0), (0, 5), (0, 22), (1, 0)]
    );

    // 正则模式下整词不生效（边界语义由模式自身表达）
    let re_hits = find_in_file(text, "word", false, true, false, 100);
    let re_hits_ww = find_in_file(text, "word", false, true, true, 100);
    assert_eq!(re_hits, re_hits_ww);
    assert_eq!(re_hits.len(), 6);
}

#[test]
fn find_in_file_regex_and_invalid_regex_fallback() {
    let text = "a1 b22 c333";

    let hits = find_in_file(text, r"\d+", true, true, false, 100);
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].len_chars, 1);
    assert_eq!(hits[2].len_chars, 3, "正则命中跨度 = 实际匹配长度");

    // 无效正则：空表兜底不 panic（UI 层已预校验，此处是竞态防御）
    assert!(find_in_file(text, "(unclosed", true, true, false, 100).is_empty());
}

#[test]
fn find_in_file_max_hits_truncates() {
    let text = "x x x x x";
    assert_eq!(find_in_file(text, "x", true, false, false, 100).len(), 5);
    let hits = find_in_file(text, "x", true, false, false, 3);
    assert_eq!(hits.len(), 3, "单文件命中封顶截断");
    // 空查询恒空（find_all 契约透传）
    assert!(find_in_file(text, "", true, false, false, 100).is_empty());
}

// ---------- filter_whole_word_text ----------

#[test]
fn whole_word_text_boundaries_and_adjacent_words() {
    // 行首/行尾 = 边界；紧贴词字符 = 剔除
    let text = "cat catalog cat";
    let hits = find_all(text, "cat", true);
    assert_eq!(hits.len(), 3);
    let kept = filter_whole_word_text(text, hits);
    assert_eq!(kept, vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
        editpad_core::MatchPos { line: 0, col: 12, len_chars: 3 }],
        "catalog 的命中被剔除，行首/行尾两处保留");

    // 前后都是标点 = 边界成立
    let kept = filter_whole_word_text("(cat)-cat.", find_all("(cat)-cat.", "cat", true));
    assert_eq!(kept.len(), 2);
}

#[test]
fn whole_word_text_line_ending_semantics() {
    // CRLF：行界口径与 find_all 一致——\r\n 整体是行界，
    // 行首/行尾判定按剥掉行尾单元的内容算
    let text = "ab\r\ncd ab\r\n";
    let hits = find_all(text, "ab", true);
    assert_eq!(hits.len(), 2, "行 0 的 ab 与行 1 的 ab");
    let kept = filter_whole_word_text(text, hits);
    assert_eq!(kept.len(), 2, "两处命中各自独立成词，全部保留");

    // 孤立 \r 同样是行界（P78：ropey 三种行尾单元口径一致）
    let text = "ab\rcd ab";
    let kept = filter_whole_word_text(text, find_all(text, "ab", true));
    assert_eq!(kept.len(), 2);

    // 跨行命中（查询含 \n）：行内边界语义不成立，一律保留
    let text = "ab\ncd";
    let hits = find_all(text, "b\nc", true);
    assert_eq!(hits.len(), 1);
    assert_eq!(filter_whole_word_text(text, hits).len(), 1);
}

#[test]
fn whole_word_text_multi_line_line_numbers_match_find_all() {
    // 多行文本上命中行号/列与 find_all 输出严格一致（整词过滤不改坐标）
    let text = "one two\nthree four five\nsix seven";
    let hits = find_all(text, "seven", true);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);
    assert_eq!(hits[0].col, 4);
    assert_eq!(filter_whole_word_text(text, hits.clone()), hits,
        "整词命中原样保留（坐标不动）");

    // 忽略大小写口径下 word 边界判定不受大小写影响
    let text = "Word WORD word";
    let hits = find_all(text, "word", false);
    assert_eq!(filter_whole_word_text(text, hits).len(), 3);
}

#[test]
fn ignored_dirs_constant_pins_phase_one_scope() {
    // 钉住一期排除范围（设计 §1）：改动须同步设计文档与 README 披露
    assert_eq!(
        IGNORED_DIRS,
        &[".git", "node_modules", "target", "dist"]
    );
}

#[test]
fn whole_word_text_full_line_break_set_matches_find_all() {
    // P147 回归：切行曾只认 \r\n——VT/FF/NEL/LS/PS 之后的命中行号大于
    // 过滤器自身推进的行号，被防御分支整体丢弃（`ab\u{000B}cd` 查 `cd`
    // 整词返回 0 命中，应为 1）。现与 find_all 共用同一 for_each_line。
    // 全集：\n、\r\n、孤立 \r、VT(\u{B})、FF(\u{C})、NEL(\u{85})、
    // LS(\u{2028})、PS(\u{2029})。
    let cases: Vec<(&str, &str)> = vec![
        ("ab\u{000B}cd", "VT"),
        ("ab\u{000C}cd", "FF"),
        ("ab\u{0085}cd", "NEL"),
        ("ab\u{2028}cd", "LS"),
        ("ab\u{2029}cd", "PS"),
        ("ab\rcd", "孤立 CR"),
        ("ab\r\ncd", "CRLF"),
        ("ab\ncd", "LF"),
    ];
    for (text, note) in cases {
        let hits = find_all(text, "cd", true);
        assert_eq!(hits.len(), 1, "{note}: find_all 应有 1 命中");
        let kept = filter_whole_word_text(text, hits);
        assert_eq!(kept.len(), 1, "{note}: 整词过滤不得丢弃行界后的命中");
        assert_eq!(kept[0].line, 1, "{note}: 命中应在第 1 行");
    }

    // 对照：VT 前的命中照常按词边界过滤（不是「全部保留」的假阳性）
    let text = "cd\u{000B}xcd";
    let kept = filter_whole_word_text(text, find_all(text, "cd", true));
    assert_eq!(kept.len(), 1, "VT 前独立 cd 保留，VT 后词中 cd 滤除");
    assert_eq!(kept[0].line, 0);
}
