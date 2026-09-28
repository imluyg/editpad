//! 行级差异——B12「变更历史行边条」的数据源。
//!
//! 只回答一件事：**相对基线，当前文档里哪几行变了／新增了**。纯函数、可同帧
//! 对拍，不认识渲染、也不写回文档。
//!
//! 形状是"两头各用一次块级比较收敛，中间那一小窗才逐行对齐"：
//! [`Document::first_diff_char`] 给出公共前缀、[`Document::suffix_align_chars`]
//! 给出公共后缀，于是**窗口之外的行一次哈希
//! 都不碰**（一次按键就 hash 全文是这里最初版踩到的形状，`keys_examined`
//! 那条用例就是钉它的）。
//!
//! ⚠️ 这是**显示用判据**：逐行等同类走 64 位内容哈希，哈希相同就当两行相同。
//! 冲撞的后果只是边条少画／多画一格；置脏与保存仍以 [`Document::content_eq`]
//! 的逐字节比对为准（那条不经这里），所以本模块不会误伤用户内容。

use crate::document::Document;

/// 单侧超过这么多字节就不算。边条是被动显示，不值得为一堆 50 MB 的日志
/// 每次按键把全文扫一遍——本仓的打开路径刚花了好几笔把"整份物化"请出热路（P298）。
pub const DIFF_MAX_TOTAL_BYTES: usize = 8 << 20;

/// 窗口任一侧行数超这个数就走保守档（整段标成"改过"）。
pub const DIFF_MAX_MIDDLE_LINES: usize = 2_000;

/// LCS 的格子预算：方向表按 1 字节／格，100 万格＝1 MB 临时内存。
pub const DIFF_MAX_LCS_CELLS: usize = 1_000_000;

/// 两处改动之间隔这么多行以内仍算**同一个变更块**，块内那些没变的行走
/// 「回退段」色（撤销回来的那一小段，夹在还改着的行中间）。
pub const CHANGE_GAP_MERGE: usize = 3;

/// 三道封顶的预算（默认用上面那组常数）。抽成参数是为了让"超上限怎么办"
/// 这一档能被用例用几行文档钉住，而不是去构造 8 MB 的夹具。
#[derive(Clone, Copy, Debug)]
pub struct DiffBudget {
    pub max_total_bytes: usize,
    pub max_middle_lines: usize,
    pub max_lcs_cells: usize,
}

impl Default for DiffBudget {
    fn default() -> Self {
        Self {
            max_total_bytes: DIFF_MAX_TOTAL_BYTES,
            max_middle_lines: DIFF_MAX_MIDDLE_LINES,
            max_lcs_cells: DIFF_MAX_LCS_CELLS,
        }
    }
}

/// 差分的可信度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffQuality {
    /// 逐行对齐算出来的，行号粒度可信。
    Exact,
    /// 窗口太大没对齐，整段都标成"改过"——范围可信、粒度不可信。
    Coarse,
    /// 没算（文档超字节预算）。空集配上这一档＝"不是没改，是没去看"。
    Skipped,
}

/// 边条一格的种类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    /// 这一行相对基线变了或新增了。
    Changed,
    /// 这一行本身没变，但夹在同一变更块里（回退段／块内空隙）。
    RevertedGap,
}

/// 一次行级差异的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineDiff {
    /// 变过或新增了哪些行（升序去重）。
    pub changed: Vec<usize>,
    pub quality: DiffQuality,
    /// 本次做了几次"行内容哈希"。上限护栏的**读数**：调用方每帧都算，
    /// 必须能盯住它别退化成"整份文档"。
    pub keys_examined: usize,
}

/// 第 `line` 行的内容哈希（不含行尾换行符）。
///
/// 行尾不进哈希是刻意的：一份文档只有一个主导行尾，逐行比对比的是这行写了
/// 什么，不是它带哪种换行。
pub fn line_key(doc: &Document, line: usize) -> u64 {
    // FNV-1a 64：常数乘一步走，够散、无第三方依赖
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let start = doc.line_to_char(line);
    let n = doc.line_body_len_chars(line);
    for ch in doc.chars_from(start).take(n) {
        h ^= ch as u32 as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 长度混进去：把"内容相同但一行拆成两行"这类形状差异也带出来
    h ^ (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// 相对基线，`current` 里变过或新增了哪些行。
pub fn changed_lines(baseline: &Document, current: &Document) -> LineDiff {
    changed_lines_in(baseline, current, &DiffBudget::default())
}

/// [`changed_lines`] 的预算可注入版。
pub fn changed_lines_in(baseline: &Document, current: &Document, budget: &DiffBudget) -> LineDiff {
    if baseline.text_len_bytes() > budget.max_total_bytes
        || current.text_len_bytes() > budget.max_total_bytes
    {
        return LineDiff {
            changed: Vec::new(),
            quality: DiffQuality::Skipped,
            keys_examined: 0,
        };
    }
    let (bl, cl) = (baseline.line_count(), current.line_count());
    if baseline.content_eq(current) {
        return LineDiff {
            changed: Vec::new(),
            quality: DiffQuality::Exact,
            keys_examined: 0,
        };
    }
    // ① 公共前缀（块级比较）：这一行之前的两边逐字符相同 ⇒ 行号也相同
    let pre = baseline.first_diff_char(current);
    let lo = baseline.char_to_line(pre).min(current.char_to_line(pre));
    // ② 公共后缀（同样块级比较）：最后一处不同之后的行两边也相同。
    //    ⚠️ 两侧**各算各的**窗口末端：纯插入时基线那侧的后缀起点会比当前那侧
    //    早一整行，共用一个 max 会把旁边那条没改的行也标上
    //    （用例 `insertion_shifts_nothing_below_it` 逮到的正是这个）
    let (lb, lc) = baseline.suffix_align_chars(current);
    let be = baseline.char_to_line(lb).max(lo).min(bl - 1);
    let ce = current.char_to_line(lc).max(lo).min(cl - 1);
    let (bn, cn) = (be - lo + 1, ce - lo + 1);
    // 乘爆了（理论上要到 usize 上限）也当作超预算走保守档：保守一侧永远更安全
    let too_many_cells = match bn.checked_mul(cn) {
        Some(cells) => cells > budget.max_lcs_cells,
        None => true,
    };
    if bn > budget.max_middle_lines || cn > budget.max_middle_lines || too_many_cells {
        // 保守档：整段标"改过"。少算是撒谎（用户会以为中间那行没动），
        // 多算只是边条粗一点
        return LineDiff {
            changed: (lo..=ce).collect(),
            quality: DiffQuality::Coarse,
            keys_examined: 0,
        };
    }

    let mut keys_examined = 0usize;
    let bk: Vec<u64> = (lo..=be)
        .map(|i| {
            keys_examined += 1;
            line_key(baseline, i)
        })
        .collect();
    let ck: Vec<u64> = (lo..=ce)
        .map(|j| {
            keys_examined += 1;
            line_key(current, j)
        })
        .collect();
    let cols = cn + 1;
    // 方向表：1＝左上（这两行配对），2＝上（基线这行被删），3＝左（当前这行新增／改过）
    let mut dir = vec![0u8; (bn + 1) * cols];
    let mut prev = vec![0u32; cols];
    let mut cur = vec![0u32; cols];
    for i in 1..=bn {
        for j in 1..=cn {
            if bk[i - 1] == ck[j - 1] {
                cur[j] = prev[j - 1] + 1;
                dir[i * cols + j] = 1;
            } else if prev[j] >= cur[j - 1] {
                cur[j] = prev[j];
                dir[i * cols + j] = 2;
            } else {
                cur[j] = cur[j - 1];
                dir[i * cols + j] = 3;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
        cur[0] = 0;
    }
    let mut hit = vec![false; cn];
    let (mut i, mut j) = (bn, cn);
    while i > 0 || j > 0 {
        let d = if i > 0 && j > 0 {
            dir[i * cols + j]
        } else if i > 0 {
            2
        } else {
            3
        };
        match d {
            1 => {
                i -= 1;
                j -= 1;
            }
            2 => {
                // 基线这一行被删了：它在新文档里不存在，把删除点标在它**紧后面**
                // 那一行上（与主流编辑器的变更块同形——删除总要落在一个还在的行旁边）。
                // 回溯状态 (i,j) 的语义是"基线第 i-1 行排在当前第 j 行之前"，所以标 j；
                // 删在窗口末尾时退到窗口最后一行（宁可多标一行，不许漏）。
                i -= 1;
                hit[j.min(cn - 1)] = true;
            }
            _ => {
                j -= 1;
                hit[j] = true;
            }
        }
    }
    let mut changed: Vec<usize> = (0..cn).filter(|&k| hit[k]).map(|k| lo + k).collect();
    changed.sort_unstable();
    changed.dedup();
    LineDiff {
        changed,
        quality: DiffQuality::Exact,
        keys_examined,
    }
}

/// 把升序的改动行号并成变更块，并给块内每一行定性。
///
/// 块外（第一处改动之前、最后一处之后）的行**不进结果**——边条只画"动过的
/// 那一片"，否则一次编辑就让整页左边多出一条带子。
pub fn classify_marks(changed: &[usize], gap_merge: usize) -> Vec<(usize, MarkKind)> {
    let mut set = changed.to_vec();
    set.sort_unstable();
    set.dedup();
    let mut out: Vec<(usize, MarkKind)> = Vec::new();
    let mut start = 0usize;
    while start < set.len() {
        // 往后并块：相邻两处改动之间的空隙 ≤ gap_merge 就还算同一块
        let mut end = start;
        while end + 1 < set.len() && set[end + 1] - set[end] - 1 <= gap_merge {
            end += 1;
        }
        for line in set[start]..=set[end] {
            // 块里点名存在的行＝真改过；块内被跳过的行＝回退段／块内空隙
            let kind = if set[start..=end].binary_search(&line).is_ok() {
                MarkKind::Changed
            } else {
                MarkKind::RevertedGap
            };
            out.push((line, kind));
        }
        start = end + 1;
    }
    out
}

/// [`classify_marks`] 用默认间距（[`CHANGE_GAP_MERGE`]）。
pub fn classify_changed_lines(changed: &[usize]) -> Vec<(usize, MarkKind)> {
    classify_marks(changed, CHANGE_GAP_MERGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::from_str(text)
    }

    /// 同帧 oracle：与实现**同一份规格**，但用字符串相等而不是内容哈希，独立
    /// 走一遍前缀/后缀裁剪＋LCS＋删除落点标记（连"相等时优先往上"的回溯取舍
    /// 也照抄）。它钉的是"哈希键有没有算错行"这一整类：行体长度取错、行首字符
    /// 偏移错、把行尾换行算进行内容……
    /// ⚠️ oracle 用 `split('\n')` 分行，喂它的夹具一律 LF。
    fn oracle_changed(base: &str, cur: &str) -> Vec<usize> {
        let a: Vec<&str> = base.split('\n').collect();
        let b: Vec<&str> = cur.split('\n').collect();
        let (bl, cl) = (a.len(), b.len());
        let mut p = 0usize;
        while p < bl && p < cl && a[p] == b[p] {
            p += 1;
        }
        let mut s = 0usize;
        while s < bl - p && s < cl - p && a[bl - 1 - s] == b[cl - 1 - s] {
            s += 1;
        }
        let (bn, cn) = (bl - p - s, cl - p - s);
        if bn == 0 && cn == 0 {
            return Vec::new();
        }
        if bn == 0 {
            return (p..cl - s).collect();
        }
        if cn == 0 {
            return if cl == 0 {
                Vec::new()
            } else {
                vec![p.min(cl - 1)]
            };
        }
        let mut dp = vec![vec![0usize; cn + 1]; bn + 1];
        for i in 1..=bn {
            for j in 1..=cn {
                dp[i][j] = if a[p + i - 1] == b[p + j - 1] {
                    dp[i - 1][j - 1] + 1
                } else {
                    dp[i - 1][j].max(dp[i][j - 1])
                };
            }
        }
        let mut hit = vec![false; cn];
        let (mut i, mut j) = (bn, cn);
        while i > 0 || j > 0 {
            let matched =
                i > 0 && j > 0 && a[p + i - 1] == b[p + j - 1] && dp[i][j] == dp[i - 1][j - 1] + 1;
            if matched {
                i -= 1;
                j -= 1;
            } else if i > 0 && (j == 0 || dp[i - 1][j] >= dp[i][j - 1]) {
                i -= 1;
                hit[j.min(cn - 1)] = true;
            } else {
                j -= 1;
                hit[j] = true;
            }
        }
        (0..cn).filter(|&k| hit[k]).map(|k| p + k).collect()
    }

    /// 朴素逐行硬比（行号当锚）——本模块存在的理由就是别退化成它。
    fn naive_index_diff(base: &str, cur: &str) -> Vec<usize> {
        let a: Vec<&str> = base.split('\n').collect();
        let b: Vec<&str> = cur.split('\n').collect();
        (0..b.len())
            .filter(|&i| i >= a.len() || a[i] != b[i])
            .collect()
    }

    #[test]
    fn identical_documents_have_no_marks() {
        let a = doc("one\ntwo\nthree\n");
        let b = doc("one\ntwo\nthree\n");
        let d = changed_lines(&a, &b);
        assert_eq!(d.quality, DiffQuality::Exact);
        assert!(d.changed.is_empty(), "实得 {:?}", d.changed);
        assert_eq!(d.keys_examined, 0, "相同文档一次哈希都不该做");
    }

    #[test]
    fn single_line_edit_marks_only_that_line() {
        let a = doc("alpha\nbravo\ncharlie\ndelta\n");
        let b = doc("alpha\nBRAVO x\ncharlie\ndelta\n");
        let d = changed_lines(&a, &b);
        assert_eq!(d.quality, DiffQuality::Exact);
        assert_eq!(d.changed, vec![1], "只有第 2 行变了，实得 {:?}", d.changed);
    }

    #[test]
    fn insertion_shifts_nothing_below_it() {
        // 本用例是"别拿行号硬比"这一整个缺陷形状的反面教材：顶部插一行后，
        // 朴素逐行比对会把**每一行**都标成改过（因为整体错位）
        let a = doc("one\ntwo\nthree\nfour\nfive\n");
        let b = doc("fresh\none\ntwo\nthree\nfour\nfive\n");
        let d = changed_lines(&a, &b);
        assert_eq!(d.quality, DiffQuality::Exact);
        assert_eq!(
            d.changed,
            vec![0],
            "只该标新插入的那一行，实得 {:?}",
            d.changed
        );
        assert!(
            naive_index_diff(
                "one\ntwo\nthree\nfour\nfive\n",
                "fresh\none\ntwo\nthree\nfour\nfive\n"
            )
            .len()
                > 1,
            "夹具自证：朴素硬比在这格上会多标（本用例才有对照意义）"
        );
        // 中间插入同理
        let c = doc("one\ntwo\nINSERTED\nthree\nfour\nfive\n");
        let m2 = changed_lines(&a, &c);
        assert_eq!(m2.quality, DiffQuality::Exact);
        assert_eq!(
            m2.changed,
            vec![2],
            "中间插入只标插入行，实得 {:?}",
            m2.changed
        );
    }

    #[test]
    fn deletion_marks_the_line_it_landed_against() {
        let a = doc("keep\ngone\nkeep2\nkeep3\n");
        let b = doc("keep\nkeep2\nkeep3\n");
        let d = changed_lines(&a, &b);
        assert_eq!(d.quality, DiffQuality::Exact);
        assert_eq!(
            d.changed,
            vec![1],
            "删除画在紧接其后的那一行上，实得 {:?}",
            d.changed
        );
        // 删掉最末一行：后面只剩"末尾换行之后的空行"，落点就是它（下标 3）——
        // 这是本模块的**规格**，不是漏算
        let c = doc("keep\ngone\nkeep2\n");
        assert_eq!(changed_lines(&a, &c).changed, vec![3]);
    }

    #[test]
    fn changed_lines_match_a_string_equality_oracle() {
        // 中文／emoji／增删／移序／重复行混起来的一批夹具，逐个与"同一规格、
        // 字符串相等"的独立实现逐元素对拍（P297 那条方法账：剪枝/跳算必须配
        // 旧算法对拍，光有上界护栏看不见"少算导致漏项"）
        let cases: [(&str, &str); 9] = [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("一\n二\n三\n", "一\n二\n三\n四\n"),
            ("x\ny\nz\n", "y\nz\n"),
            ("alpha\nbeta\ngamma\ndelta\n", "alpha\ngamma\nbeta\ndelta\n"),
            ("🙂\n😀\n🙈\n", "🙂\n🙈\n"),
            (
                "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n",
                "l1\nnew\nl2\nl3\nl4x\nl5\nl6\nl7\nl8\nl9\n",
            ),
            ("dup\ndup\ndup\n", "dup\ndup\nx\ndup\n"),
            ("keep\ngone\nkeep2\nkeep3\n", "keep\ngone\nkeep2\n"),
            (
                "\n\nblank above\nblank below\n",
                "\n\nblank above\nchanged\nblank below\n",
            ),
        ];
        for (base, cur) in cases {
            let d = changed_lines(&doc(base), &doc(cur));
            assert_eq!(
                d.quality,
                DiffQuality::Exact,
                "夹具 {base:?}→{cur:?} 不该走保守档"
            );
            assert_eq!(
                d.changed,
                oracle_changed(base, cur),
                "与字符串 oracle 不同：{base:?}→{cur:?}"
            );
        }
        // 非空自证：这批夹具里至少有一格真的产出了标记，否则整轮对拍可以是"两边都空"
        assert!(
            cases
                .iter()
                .any(|(b, c)| !changed_lines(&doc(b), &doc(c)).changed.is_empty()),
            "对拍夹具里要有会动的格子"
        );
    }

    #[test]
    fn window_trims_so_a_mid_document_edit_hashes_only_a_few_lines() {
        // 成本护栏：2 万行的文档改中间一行，"行内容哈希"的次数只该与改动窗口
        // 同量级，不该与文档行数同量级（前缀/后缀两头都是 memcmp 量级收敛）
        let line = |i: usize| format!("row {i} filler filler filler");
        let base: String = (0..20_000).map(line).collect::<Vec<_>>().join("\n") + "\n";
        let mut lines: Vec<String> = (0..20_000).map(line).collect();
        lines[10_000] = "EDITED line contents here".to_owned();
        let cur = lines.join("\n") + "\n";
        let d = changed_lines(&doc(&base), &doc(&cur));
        assert_eq!(d.quality, DiffQuality::Exact);
        assert_eq!(d.changed, vec![10_000], "实得 {:?}", d.changed);
        assert!(
            d.keys_examined <= 8,
            "改一行不该扫全文：本次哈希了 {} 行",
            d.keys_examined
        );
        // 正对照：改动横跨两端时窗口确实会张开（这条不是"永远只算 8 行"的假哨兵）
        let wide_base = "a\n".repeat(300);
        let wide_cur = "z\n".repeat(300);
        let w = changed_lines(&doc(&wide_base), &doc(&wide_cur));
        assert!(
            w.keys_examined > 8,
            "夹具自证：两端都改时窗口要张开，否则上面那条护栏是瞎的"
        );
    }

    #[test]
    fn budget_decides_who_falls_back_and_who_is_skipped() {
        // 用几行文档就能钉住"超预算"的三档，不必构造 8 MB 夹具
        let base = doc("a\nb\nc\nd\ne\n");
        let cur = doc("a\nX\nY\nZ\ne\n");
        let tight = DiffBudget {
            max_total_bytes: 1 << 20,
            max_middle_lines: 1,
            max_lcs_cells: 1_000,
        };
        let d = changed_lines_in(&base, &cur, &tight);
        assert_eq!(d.quality, DiffQuality::Coarse);
        assert_eq!(
            d.changed,
            vec![1, 2, 3],
            "保守档整段标改，实得 {:?}",
            d.changed
        );
        assert_eq!(d.keys_examined, 0, "保守档一次哈希都不该做");
        // 默认预算下同一对文档是 Exact
        let e = changed_lines(&base, &cur);
        assert_eq!(e.quality, DiffQuality::Exact);
        assert_eq!(e.changed, vec![1, 2, 3]);

        let small = DiffBudget {
            max_total_bytes: 4,
            ..DiffBudget::default()
        };
        let s = changed_lines_in(&base, &cur, &small);
        assert_eq!(s.quality, DiffQuality::Skipped, "超预算要说「没算」");
        assert!(
            s.changed.is_empty(),
            "超预算不得交一张空表当作「没改」的 Exact 结果，实得 {:?}",
            s.changed
        );
    }

    #[test]
    fn coarse_window_never_drops_a_real_change() {
        // 保守档的方向性：宁可多画，不许漏
        let base = doc("k1\nk2\nk3\nk4\nk5\nk6\n");
        let cur = doc("k1\nk2\nCHANGED\nk4\nk5\nk6\n");
        let loose = DiffBudget {
            max_middle_lines: 0,
            ..DiffBudget::default()
        };
        let d = changed_lines_in(&base, &cur, &loose);
        assert_eq!(d.quality, DiffQuality::Coarse);
        assert!(
            d.changed.contains(&2),
            "真改了的那行必须在内，实得 {:?}",
            d.changed
        );
    }

    #[test]
    fn classify_merges_nearby_changes_and_fills_the_gap() {
        let marks = classify_marks(&[2, 3, 7], CHANGE_GAP_MERGE);
        assert_eq!(
            marks,
            vec![
                (2, MarkKind::Changed),
                (3, MarkKind::Changed),
                (4, MarkKind::RevertedGap),
                (5, MarkKind::RevertedGap),
                (6, MarkKind::RevertedGap),
                (7, MarkKind::Changed),
            ],
            "2、3 相连；3→7 之间空 3 行＝间距上限 ⇒ 并成同一块，空隙走回退段色"
        );
        // 空隙小于上限也并块
        let gap = classify_marks(&[1, 4], CHANGE_GAP_MERGE);
        assert_eq!(
            gap,
            vec![
                (1, MarkKind::Changed),
                (2, MarkKind::RevertedGap),
                (3, MarkKind::RevertedGap),
                (4, MarkKind::Changed),
            ],
            "实得 {gap:?}"
        );
        // 空隙超过间距上限就分两块，块外一行都不进结果
        let two = classify_marks(&[1, 6], CHANGE_GAP_MERGE);
        assert_eq!(
            two,
            vec![(1, MarkKind::Changed), (6, MarkKind::Changed)],
            "两块之间不该有边条，实得 {two:?}"
        );
        assert!(classify_marks(&[], CHANGE_GAP_MERGE).is_empty());
    }

    #[test]
    fn classify_is_order_and_duplicate_insensitive() {
        // 上游给乱序或重复的行号也不该画歪
        let a = classify_marks(&[5, 1, 5, 2], CHANGE_GAP_MERGE);
        let b = classify_marks(&[1, 2, 5], CHANGE_GAP_MERGE);
        assert_eq!(a, b, "实得 {a:?} vs {b:?}");
        assert_eq!(
            b,
            vec![
                (1, MarkKind::Changed),
                (2, MarkKind::Changed),
                (3, MarkKind::RevertedGap),
                (4, MarkKind::RevertedGap),
                (5, MarkKind::Changed),
            ]
        );
    }

    #[test]
    fn line_key_distinguishes_content_not_line_endings() {
        let lf = doc("same\nline\n");
        assert_eq!(line_key(&lf, 1), line_key(&doc("x\nline\ny"), 1));
        // 同一份内容换行位置不同 ⇒ 行形状不同，哈希必须能带出来
        assert_ne!(line_key(&lf, 0), line_key(&doc("sameline\n"), 0));
    }
}
