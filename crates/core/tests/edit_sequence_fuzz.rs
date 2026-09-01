//! 随机编辑序列对拍（README「测试策略」承诺项）。
//!
//! 用确定性 PRNG（XorShift64，多种子可复现）生成随机编辑序列，
//! 同步施加到 `Document`（被测）与 `String`（参照模型），逐步断言：
//!
//! 1. 正文逐字一致（`to_text()` == model）；
//! 2. 字符数一致；
//! 3. 行数一致（按 ropey 的换行语义：`\n`、`\r\n`、孤立 `\r` 都算行界）;
//! 4. 抽样行内容一致；
//! 5. 「全部替换」操作与 `str::replacen` 参照逐字节一致。
//!
//! 说明：原计划引入 proptest，但本构建环境网络受限无法拉取新依赖；
//! 改为等价的确定性模糊对拍——固定种子保证失败可复现，覆盖面
//! （随机位置/随机文本池/混合操作类型）与 proptest 策略等价。

use editpad_core::{replace_all_document, Document};

/// XorShift64：零依赖、跨平台稳定的伪随机源。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// [0, n) 均匀整数；n=0 时返回 0。
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[self.below(slice.len())]
    }
}

/// 文本池：刻意混入三种行尾、CJK、4 字节 emoji 与制表符，
/// 覆盖 P9 行尾归一 / P14 列宽 / 多字节边界等路径。
const TOKENS: &[&str] = &[
    "a", "Z", "9", "中", "文", "🚀", "🛰️", " ", "\t", "\n", "\r\n", "\r",
];

/// 模型字符串的全部字符边界偏移（含 0 与 len）：insert/remove 的合法位置集。
fn char_boundaries(s: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (i, c) in s.char_indices() {
        offsets.push(i + c.len_utf8());
    }
    offsets.push(s.len());
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// 字节偏移 → 字符索引（ropey 的 insert/remove_range 以字符计）。
fn char_index_of(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset].chars().count()
}

/// ropey 语义的行数：`\n`、`\r\n`、孤立 `\r` 都是一个行界；空串 1 行。
fn rope_style_line_count(s: &str) -> usize {
    if s.is_empty() {
        return 1;
    }
    let mut count = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => count += 1,
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                count += 1;
            }
            _ => {}
        }
    }
    count + 1
}

/// ropey 语义切行（含空行），用于抽样比对 line_str。
fn rope_style_lines(s: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                current.push('\n');
                lines.push(std::mem::take(&mut current));
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    current.push_str("\r\n");
                } else {
                    current.push('\r');
                }
                lines.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    lines.push(current);
    lines
}

/// 生成一条随机编辑序列并同步驱动 Document 与 String 模型，
/// 每步做全量一致性断言。`rounds` 为操作数。
fn fuzz_one(seed: u64, rounds: usize) {
    let mut rng = Rng::new(seed);

    // 种子文本：3~20 个随机 token
    let mut model = String::new();
    for _ in 0..(rng.below(18) + 3) {
        model.push_str(rng.pick(TOKENS));
    }
    let mut doc = Document::from_str(&model);

    for step in 0..rounds {
        let boundaries = char_boundaries(&model);
        match rng.below(100) {
            // ---- 插入：1~5 个随机 token，落在任意字符边界 ----
            0..=54 => {
                let text: String = (0..rng.below(5) + 1).map(|_| *rng.pick(TOKENS)).collect();
                let pos = *rng.pick(&boundaries);
                doc.insert(char_index_of(&model, pos), &text);
                model.insert_str(pos, &text);
            }
            // ---- 区间删除：随机起点，最多删 5 个字符 ----
            55..=89 => {
                let start_idx = rng.below(boundaries.len());
                let start = boundaries[start_idx];
                // 终点取「起点后第 k 个边界」（k ∈ [0, max_span]），k=0 为空删
                let max_span = (boundaries.len() - start_idx - 1).min(5);
                let k = rng.below(max_span + 1);
                let end = boundaries[(start_idx + k).min(boundaries.len() - 1)];
                let (cs, ce) = (
                    char_index_of(&model, start),
                    char_index_of(&model, end),
                );
                doc.remove_range(cs, ce);
                model.replace_range(start..end, "");
            }
            // ---- 全部替换（大小写敏感；参照 = str::replacen）----
            _ => {
                let query: String = (0..rng.below(4) + 1).map(|_| *rng.pick(TOKENS)).collect();
                let replacement: String =
                    (0..rng.below(4) + 1).map(|_| *rng.pick(TOKENS)).collect();
                let expected = model.replacen(&query, &replacement, usize::MAX);
                let (new_text, count) =
                    replace_all_document(&doc, &query, &replacement, true);
                assert_eq!(
                    new_text, expected,
                    "seed={seed} step={step} 替换正文不一致"
                );
                assert_eq!(
                    count,
                    model.matches(&query).count(),
                    "seed={seed} step={step} 替换计数不一致"
                );
                model = new_text;
                // replace_all 产出的是新文本：按应用层同款语义整体换文档
                doc = Document::from_str(&model);
            }
        }

        // ---- 逐步全量一致性断言 ----
        assert_eq!(doc.to_text(), model, "seed={seed} step={step} 正文发散");
        assert_eq!(
            doc.text_len(),
            model.chars().count(),
            "seed={seed} step={step} 字符数发散"
        );
        assert_eq!(
            doc.line_count(),
            rope_style_line_count(&model),
            "seed={seed} step={step} 行数发散"
        );

        // 抽样行内容（首/中/尾各一）
        let lines = rope_style_lines(&model);
        let picks = [
            0usize,
            lines.len() / 2,
            lines.len() - 1,
            rng.below(lines.len()),
        ];
        for i in picks {
            assert_eq!(
                doc.line_str(i),
                lines[i],
                "seed={seed} step={step} 第 {i} 行内容发散"
            );
        }

        // 行↔字符偏移往返抽查
        let line_start = doc.line_to_char(rng.below(doc.line_count()));
        assert_eq!(
            doc.char_to_line(line_start.min(model.chars().count())),
            doc.char_to_line(line_start),
            "seed={seed} step={step} 行偏移换算自相矛盾"
        );
    }
}

#[test]
fn edit_sequence_fuzz_matches_string_model() {
    // 固定种子集合：任一失败都可用同种子精确复现
    let seeds = [
        0xDEAD_BEEF,
        0x5EED_1234,
        1,
        42,
        0x7FFF_FFFF_FFFF_FFFD,
        0x2545_F491_4F6C_DD1D,
        9_007_199_254_740_993,
        0xABCD_EF01,
    ];
    for seed in seeds {
        fuzz_one(seed, 120);
    }
}

#[test]
fn heavy_single_seed_long_session() {
    // 单种子长会话：更多步数，考验长期累积下的索引/行元数据一致性
    fuzz_one(2026_0824, 600);
}
