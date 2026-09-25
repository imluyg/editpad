//! 软换行 Phase 2（§2 候选池「编辑全路径开态回归 fuzz」）：
//! 随机编辑序列下，**增量维护的折行索引**必须与**整表重建**逐行一致。
//!
//! 为什么拿「整表重建」当 oracle：折行索引有三条产生路径——初建（`reset`）、
//! 单行差值更新（编辑汇点）、逐行惰性对账（`reconcile_wrap_index`）。软换行
//! 这一路的缺陷史全部落在**增量路径与真值不一致**上：P154（行数变化整表重置后
//! 总数按「每行 1 段」偏小 → 行号闪一下）、P179（滚动条滞回让位自锁）、
//! P180（`pixel_breaks` 上界多减一次 → 末段整字超预算、视觉段数少算 1）。
//! 单点用例只能钉住当时想到的那一步，随机序列 + 双路径对照才能钉住整条不变式。
//!
//! 断言取**最强形态**：编辑后立刻读增量值（不预先收敛）——这正是 P154 立下的
//! 契约「编辑汇点已把索引对账到真值」，任何绕过汇点的编辑路径都会在这里现形。

use super::*;

/// 与 P86/P138/P141 同一族的最简可复现随机源（XorShift64，无新依赖）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

/// 独立路径的真值：关掉再打开软换行（走整表重建）+ 逐行收敛，
/// 返回（总视觉行数，每行的视觉行基址）。
fn rebuild_truth(c: &mut EditorCore) -> (u32, Vec<u32>) {
    c.set_word_wrap(false);
    c.set_word_wrap(true);
    let lines = c.doc.line_count();
    for l in 0..lines {
        let t = c.line_text(l);
        c.segments_of_line(l, &t);
    }
    let bases = (0..lines).map(|l| c.line_visual_base(l)).collect();
    (c.visual_rows_total(), bases)
}

/// 宽窄行混排、含 CJK（双宽）与 ASCII 词段的初始文档。
fn seed_text() -> String {
    let mut text = String::new();
    for i in 0..80 {
        match i % 5 {
            0 => text.push_str(&"中".repeat(9 + i)),
            1 => text.push_str(&format!("line {i} with some ascii words to wrap")),
            2 => text.push_str(&"x".repeat(200)),
            3 => text.push_str(&format!("短 {i}")),
            _ => {}
        }
        text.push('\n');
    }
    text
}

/// Phase 2 的「开态性能」判据（次数断言，不用耗时——同 O-1/O-4 的理由）：
/// 开态下**不改变行数**的单点插入，只应碰受影响的少数行；
/// 若哪天有人把任何一次编辑都接回整表重算，这条会当场红。
#[test]
fn wrap_within_line_insert_does_not_rebreak_whole_document() {
    const LINES: usize = 2000;
    let text: String = (0..LINES)
        .map(|i| format!("row{i} 一些要折行的中文内容 words\n"))
        .collect();
    let mut c = EditorCore::default();
    c.reset_document(editpad_core::Document::from_str(&text));
    c.set_viewport_width(400.0);
    c.set_viewport_height(200.0);
    c.set_word_wrap(true);
    // 先让索引建立起来（避免把首帧的整表建立算进本次编辑）
    for l in 0..LINES {
        let t = c.line_text(l);
        c.segments_of_line(l, &t);
    }
    c.cursor = CursorPos { line: 7, col: 3 };
    // 先把「刚开启软换行」那一次性对账消费掉——真实应用里它发生在开启的那一帧
    // （编辑汇点帧末调 `reconcile_wrap_index`），不计在后续每一次击键上。
    c.reconcile_wrap_index();
    c.take_line_text_calls();
    c.insert_str("a");
    let calls = c.take_line_text_calls();
    assert!(
        calls * 20 < LINES,
        "开态单点插入却取串 {calls} 次（文档 {LINES} 行）：折行索引被整表重算了"
    );
    // 稳态复查：再来一次仍应便宜（否则说明每击键都重新武装整表对账 = O-5 的
    // 已知形态，届时这条用例会替设计轮把量出来）
    c.take_line_text_calls();
    c.insert_str("b");
    let again = c.take_line_text_calls();
    // 参考读数（不设上限、只打印，同 P69 帧成本用法的口径）：开态稳态击键的取串次数
    eprintln!("[软换行开态] 稳态单点击键取串 {again} 次 / 文档 {LINES} 行");
    assert!(
        again * 20 < LINES,
        "稳态击键取串 {again} 次：整表对账被反复武装，开态打字退化为 O(文档)"
    );
}

#[test]
fn wrap_index_matches_full_rebuild_across_random_edits() {
    for seed in [
        0x9e37_79b9_5eed_0101u64,
        0x85eb_ca6b_5eed_0202,
        0xc2b2_ae3d_5eed_0303,
    ] {
        let mut rng = Rng(seed);
        let mut c = EditorCore::default();
        c.reset_document(editpad_core::Document::from_str(&seed_text()));
        c.set_viewport_width(400.0);
        c.set_viewport_height(200.0);
        c.set_word_wrap(true);

        for step in 0..120 {
            let lines = c.doc.line_count();
            let line = rng.below(lines as u64);
            let col = rng.below(c.line_display_len(line) as u64 + 1);
            c.cursor = CursorPos { line, col };

            let token = match rng.below(6) {
                0 => " word",
                1 => "\n",
                2 => "中文字词",
                3 => "\t\t",
                4 => "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                _ => " ",
            };
            if rng.next().is_multiple_of(8) {
                c.backspace();
            } else {
                c.insert_str(token);
            }

            // 增量侧：编辑后**不预先收敛**，直接读（P154 契约的最强形态）
            let inc_total = c.visual_rows_total();
            let lines = c.doc.line_count();
            let inc_bases: Vec<u32> = (0..lines).map(|l| c.line_visual_base(l)).collect();

            // 重建侧：另一条产生路径
            let (reb_total, reb_bases) = rebuild_truth(&mut c);

            assert_eq!(
                inc_total, reb_total,
                "seed {seed:#x} step {step}：总视觉行数漂移（增量 {inc_total} vs 整表重建 {reb_total}）"
            );
            assert_eq!(
                inc_bases, reb_bases,
                "seed {seed:#x} step {step}：逐逻辑行的视觉行基址漂移"
            );
            // 自洽性：每个视觉行反解回 (逻辑行, 段序) 必须落回同一编号
            for v in 0..reb_total {
                let (l, seg, _, _) = c.locate_visual(v);
                assert_eq!(
                    reb_bases[l] as usize + seg,
                    v as usize,
                    "seed {seed:#x} step {step}：视觉行 {v} 反解为 line {l} 段 {seg}，与基址不符"
                );
            }
        }
    }
}

// TEMP-DIAG-O5 已撤，改为下面的常驻契约用例

/// O-5 的「开态回车成本」契约（次数断言，与上面那条同行数插入判据同族）：
/// **改变行数**的击键（回车）不得再把折行索引整表重算——次数必须与文档规模
/// 脱钩。改前实测：500 行 505 次 / 2000 行 2005 次 / 1 万行 10 005 次
/// （每一次回车都付全文档代价），改后三档都是 5 次。
///
/// 正确性不在这里测：与本条无关心——`wrap_index_matches_full_rebuild_across_random_edits`
/// 拿「整表重建」当 oracle 逐行对照，增量平移一旦与真值不一致它先红。
#[test]
fn wrap_line_count_change_cost_is_independent_of_document_size() {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for lines in [500usize, 2_000, 10_000] {
        let text: String = (0..lines)
            .map(|i| format!("row{i} 一些要折行的中文内容 words\n"))
            .collect();
        let mut c = EditorCore::default();
        c.reset_document(editpad_core::Document::from_str(&text));
        c.set_viewport_width(400.0);
        c.set_viewport_height(200.0);
        c.set_word_wrap(true);
        for l in 0..lines {
            let t = c.line_text(l);
            c.segments_of_line(l, &t);
        }
        c.reconcile_wrap_index();
        c.cursor = CursorPos {
            line: lines / 2,
            col: 3,
        };
        // 一次回车 = 编辑汇点（增量平移 + 当场收敛受影响行）+ 帧末对账。
        // 两者都计入：真实应用里它们同帧发生，用户付的就是这一帧的钱。
        c.take_line_text_calls();
        c.insert_str("\n");
        c.reconcile_wrap_index();
        let calls = c.take_line_text_calls();
        assert!(
            calls > 0,
            "探针失效：{lines} 行文档按一次回车一次整行取串都没有"
        );
        // 紧接着退格：光标此刻在**新行行首**（col 0）⇒ 这次退格把两行并回去，
        // 是「行数 -1」的另一半热路径（申报点：edit.rs `backspace`）
        c.take_line_text_calls();
        assert!(c.backspace(), "退格应当真的删掉内容（并行）");
        c.reconcile_wrap_index();
        let merge = c.take_line_text_calls();
        assert!(merge > 0, "探针失效：并行退格一次取串都没有");
        counts.push((lines, calls.max(merge)));
    }
    eprintln!("[O-5] 开态「回车 / 并行退格」取串次数（各档取两者较大值）：{counts:?}");
    let (small, large) = (counts[0].1, counts[2].1);
    assert!(
        large * 20 < counts[2].0,
        "1 万行文档按一次回车取串 {large} 次：折行索引又被整表重算了"
    );
    assert!(
        large <= small * 2 + 5,
        "文档 ×20 使回车取串次数 {small} → {large}：成本仍按文档规模结算"
    );
}
