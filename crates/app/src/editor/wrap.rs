//! 软换行（候选池⑯ Phase 1）地基组件。
//!
//! 设计文档：`docs/soft-wrap-design.md`（第 68 轮入库）。本模块提供两块
//! 纯逻辑地基，供 Phase 1 后续轮接线 draw/hit_test/motion：
//!
//! * [`wrap_breaks`]：按**显示列**（与渲染 `char_cols` 同源：宽字符 2、
//!   Tab 推进制表位）把逻辑行切为若干视觉段的段首列向量；
//! * [`WrapIndex`]：Fenwick（BIT）视觉段计数前缀 + 行断点 memo +
//!   内容代次失效——`prefix_rows(line)` 与 `total()` 均 O(log n)，
//!   单行重算 O(行长) + O(log n) 点更新。
//!
//! 失效模型（v1，披露于设计 §3.2/§9）：内容代次 `gen` 每次正文突变 +1；
//! memo 条目带 gen，过期条目在**下次被查询到**时重算并差值更新 BIT——
//! 未查询行的 BIT 计数短暂陈旧，表现为滚动范围在编辑后可能短暂偏差，
//! 随窗口行重算收敛。行数变化 = 整表重置（BIT 全 1 起步，窗口行逐步
//! 填充），滚动范围短暂按「每行一段」估算——同为已披露的 v1 取舍。

use std::collections::HashMap;
use std::rc::Rc;

use super::metrics::char_cols;

/// 按显示列把逻辑行正文切为视觉段。返回**段首列**向量（恒含 0）。
///
/// * `max_cols` = 视口可用显示列数（整数化）；0 视作 1（防御，单字符段）；
/// * 断点为字符级（词边界优先与行首禁则回避留 v2，设计 §1 非目标）；
/// * 与渲染同源：宽度累计用 `char_cols`（宽字符 2、Tab 跳制表位），
///   保证「折行位置」与「关闭软换行时的像素列」逐列一致。
pub(crate) fn wrap_breaks(body: &str, max_cols: usize) -> Vec<usize> {
    let max_cols = max_cols.max(1);
    let mut breaks = vec![0usize];
    let mut w: usize = 0;
    for (i, ch) in body.chars().enumerate() {
        let cw = char_cols(ch, w) as usize;
        if w > 0 && w + cw > max_cols {
            // 当前字符放不进本段：从它开始新段（至少占它自身宽度）
            breaks.push(i);
            w = cw;
        } else {
            w += cw;
        }
    }
    breaks
}

/// 视觉段计数索引：Fenwick 前缀 + 行断点 memo + 内容代次。
///
/// * `set_line`：重算某行断点并差值更新 BIT（O(行长) + O(log n)）；
/// * `reset(n)`：行数变化时整表重置（每行暂记 1 段，窗口行逐步填充）；
/// * `prefix_rows(line)`：逻辑行 `line` 首个视觉行的行号（= 之前所有行
///   的段数和）；`total()`：总视觉行数——均 O(log n)。
///
/// BIT 用 `Vec<i64>` 存差分树；`seg_now` 记录每行当前计入 BIT 的段数
/// （与 memo 解耦：memo 可被 gen 过期，seg_now 恒与 BIT 一致）。
pub(crate) struct WrapIndex {
    bit: Vec<i64>, // 1-based Fenwick；bit[0] 占位
    seg_now: Vec<u32>,
    memo: HashMap<usize, (u64, Rc<Vec<usize>>)>,
    gen: u64,
}

impl WrapIndex {
    /// 行数变化（或首次/开关切换）：整表重置为「每行 1 段」。
    pub(crate) fn reset(&mut self, n: usize) {
        self.seg_now = vec![1; n];
        // Fenwick O(n) 构建：初值全 1
        self.bit = vec![0; n + 1];
        for i in 1..=n {
            self.bit[i] += 1;
            let j = i + (i & i.wrapping_neg());
            if j <= n {
                self.bit[j] += self.bit[i];
            }
        }
        self.memo.clear();
    }

    /// 内容代次推进（正文突变汇点调用）。memo 全部过期，BIT 保留旧计数
    /// （陈旧值由窗口行重算逐步收敛，见模块注释）。
    pub(crate) fn bump_gen(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    /// 确保某行按当前代次与内容计入索引：命中同代 memo 直接返回；
    /// 否则重算断点、差值更新 BIT 并写 memo。
    pub(crate) fn set_line(
        &mut self,
        line: usize,
        body: &str,
        max_cols: usize,
    ) -> Rc<Vec<usize>> {
        if let Some((g, b)) = self.memo.get(&line) {
            if *g == self.gen {
                return b.clone();
            }
        }
        let breaks = Rc::new(wrap_breaks(body, max_cols));
        let seg = breaks.len() as u32;
        let diff = seg as i64 - self.seg_now[line] as i64;
        if diff != 0 {
            self.bit_add(line, diff);
        }
        self.seg_now[line] = seg;
        self.memo.insert(line, (self.gen, breaks.clone()));
        breaks
    }

    /// 逻辑行 `line` 首个视觉行的行号（之前所有行的段数和）。
    pub(crate) fn prefix_rows(&self, line: usize) -> u32 {
        self.bit_prefix(line.min(self.seg_now.len()))
    }

    /// 总视觉行数。
    pub(crate) fn total(&self) -> u32 {
        self.bit_prefix(self.seg_now.len())
    }

    pub(crate) fn gen(&self) -> u64 {
        self.gen
    }

    fn bit_add(&mut self, i: usize, d: i64) {
        let mut j = i + 1; // 转 1-based
        while j < self.bit.len() {
            self.bit[j] += d;
            j += j & j.wrapping_neg();
        }
    }

    fn bit_prefix(&self, i: usize) -> u32 {
        let mut sum = 0i64;
        let mut j = i; // 前 i 行 = 1-based 前缀 i
        while j > 0 {
            sum += self.bit[j];
            // ⚠️ 步进是「减去 lowbit」：j &= -j 会停在 lowbit 自身造成
            // 无限循环（第 72 轮实测 3 测试挂起 60s+ 定位）
            j -= j & j.wrapping_neg();
        }
        sum.max(0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_character_level_matches_render_columns() {
        // ASCII：每 4 列一段；8 列恰 2 段——段首向量不含尾边界（不产空段）
        assert_eq!(wrap_breaks("abcdefgh", 4), vec![0, 4]);
        // 恰好压线：不产生空段
        assert_eq!(wrap_breaks("abcd", 4), vec![0]);
        // 宽字符占 2 列：中中中（6 列）在 max=4 时第二段从第 2 字符起
        assert_eq!(wrap_breaks("中中中", 4), vec![0, 2]);
        // 短行不断
        assert!(wrap_breaks("", 4) == vec![0]);
        assert_eq!(wrap_breaks("ab", 4), vec![0]);
        // max=0 防御为 1：每字符一段（2 字符 = 2 段，段首 [0,1]）
        assert_eq!(wrap_breaks("ab", 0), vec![0, 1]);
        // Tab 推进制表位（与渲染同源）：col0 的 Tab 占满 4 列，其后 X 放不下
        assert_eq!(wrap_breaks("\tX", 4), vec![0, 1], "Tab 满段，X 另起一段");
        assert_eq!(wrap_breaks("\tXX", 4), vec![0, 1], "第二段 XX 共 2 列放得下");
    }

    #[test]
    fn wrap_index_prefix_total_and_point_update() {
        let mut idx = WrapIndex {
            bit: vec![0],
            seg_now: vec![],
            memo: HashMap::new(),
            gen: 0,
        };
        let bodies = ["short", "a-very-long-line-that-wraps", "x"];
        idx.reset(bodies.len());
        assert_eq!(idx.total(), 3, "重置后每行暂记 1 段");
        for (i, b) in bodies.iter().enumerate() {
            idx.set_line(i, b, 12);
        }
        // 段数：short(1) + 长行(?) + x(1)；长行 27 列 / 12 → 3 段
        let long = wrap_breaks(bodies[1], 12).len() as u32;
        assert!(long >= 3);
        assert_eq!(idx.total(), 1 + long + 1);
        assert_eq!(idx.prefix_rows(0), 0);
        assert_eq!(idx.prefix_rows(1), 1);
        assert_eq!(idx.prefix_rows(2), 1 + long);
        assert_eq!(idx.prefix_rows(3), 1 + long + 1, "越界钳为总数");
    }

    #[test]
    fn wrap_index_gen_bump_invalidates_memo_only() {
        let mut idx = WrapIndex {
            bit: vec![0],
            seg_now: vec![],
            memo: HashMap::new(),
            gen: 0,
        };
        idx.reset(1);
        idx.set_line(0, "aaaa", 2); // 2 段
        assert_eq!(idx.total(), 2);
        idx.bump_gen();
        // 同代 memo 失效：同内容重算 → 结果一致（BIT 差值 0，不重复累加）
        idx.set_line(0, "aaaa", 2);
        assert_eq!(idx.total(), 2, "同内容重算不得重复计段");
        // ⚠️ 内容突变必须先推进代次（接线契约：正文突变汇点调用
        // bump_gen）——否则同 gen 命中上次写入的 memo 会读到旧断点
        idx.bump_gen();
        idx.set_line(0, "aaaaaaa", 2); // 4 段
        assert_eq!(idx.total(), 4, "内容变长：代次推进后重算，段数差值正确更新");
    }

    #[test]
    fn wrap_index_reset_on_row_count_change() {
        let mut idx = WrapIndex {
            bit: vec![0],
            seg_now: vec![],
            memo: HashMap::new(),
            gen: 0,
        };
        idx.reset(2);
        idx.set_line(0, "aaaaaaaa", 4); // 2 段
        idx.set_line(1, "bb", 4);
        assert_eq!(idx.total(), 3);
        // 行数变化：整表重置（每行 1 段起步）
        idx.reset(4);
        assert_eq!(idx.total(), 4);
        assert_eq!(idx.prefix_rows(3), 3);
        // 重新填充后收敛
        for (i, b) in ["a", "bbbbbbbb", "", "c"].iter().enumerate() {
            idx.set_line(i, b, 4);
        }
        assert_eq!(idx.total(), 1 + 2 + 1 + 1);
    }
}
