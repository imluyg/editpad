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
/// * 断点优选（v2，设计 §3.1）：溢出时先在本段内回找**断行机会**
///   （空白后 / 连字符后 / 宽字符后——CJK 逐字可断、拉丁按词保持），
///   找不到再回退字符级断点；新段首字符属行首禁则集合时再回退
///   （见 [`choose_break`]）；
/// * 与渲染同源：宽度累计用 `char_cols`（宽字符 2、Tab 跳制表位），
///   保证「折行位置」与「关闭软换行时的像素列」逐列一致。
pub(crate) fn wrap_breaks(body: &str, max_cols: usize) -> Vec<usize> {
    let max_cols = max_cols.max(1);
    let chars: Vec<char> = body.chars().collect();
    let mut breaks = vec![0usize];
    let mut w: usize = 0;
    for i in 0..chars.len() {
        let cw = char_cols(chars[i], w) as usize;
        if w > 0 && w + cw > max_cols {
            // 当前字符放不进本段：从优选断点（回退至多到段首+1，至少
            // 占它自身宽度）起新段；新段宽按段首重算——Tab 的制表位
            // 展开依赖段内列，断点前移后宽度必须逐字符重新累计
            let seg_start = breaks[breaks.len() - 1];
            let b = choose_break(&chars, seg_start, i);
            breaks.push(b);
            let mut w2 = 0usize;
            for &c in &chars[b..=i] {
                w2 += char_cols(c, w2) as usize;
            }
            w = w2;
        } else {
            w += cw;
        }
    }
    breaks
}

/// 断点优选（v2，设计 §3.1 词边界优先 + 行首禁则回避）。
///
/// 在 `[seg_start+1, i]` 内**自右向左**找最后一个断行机会——前一字符
/// 是空白 / 连字符 / 宽字符（CJK 逐字可断），或当前字符是宽字符
/// （拉丁→CJK 边界，CJK 词整体不被拆进上一段）；`i` 自身即机会时
/// 保持字符级断点，全无机会（如纯拉丁长词）同样回退 `i`。
/// 选定后做行首禁则回避：新段首字符属 [`HEAD_FORBIDDEN`] 时逐字符
/// 回退，直到段首合法或退无可退（段内至少保留 1 字符，不留空段）。
fn choose_break(chars: &[char], seg_start: usize, i: usize) -> usize {
    let mut b = i;
    for j in (seg_start + 1..=i).rev() {
        let prev = chars[j - 1];
        if prev.is_whitespace()
            || prev == '-'
            || char_cols(prev, 0) >= 2.0
            || char_cols(chars[j], 0) >= 2.0
        {
            b = j;
            break;
        }
    }
    while b > seg_start + 1 && is_head_forbidden(chars[b]) {
        b -= 1;
    }
    b
}

/// 行首禁则字符（v2，设计 §3.1「`,。》」… 等」）：全角闭合标点不得
/// 出现在视觉行首。刻意**不含半角** `.,?!` 等——代码文本里行首
/// `)` / `...` 是合法常态，禁则只针对 CJK 行文。
const HEAD_FORBIDDEN: &str = "，。、：；！？》〉」』）】〕｝…‥・ー～’”";

fn is_head_forbidden(c: char) -> bool {
    HEAD_FORBIDDEN.contains(c)
}

/// 按**真实字形像素宽**断行（P96 用户点单根治）：`xs` = 与绘制同源的
/// 逐字符起点像素 x（`xs[k]` = 第 k 字符左缘，末项 = 行尾 x，即
/// `shape_row_xs` 的输出；要求单调非降）；`body` = 行正文（与 `xs`
/// 同序，供断点优选的字符分类——词边界/禁则判定，见 [`choose_break`]）。
///
/// 断点语义与 [`wrap_breaks`] 一致（段首列向量、恒含 0、恰好压线不产
/// 空段），但段宽累计用真实 advance——非等宽字体（CJK 全宽 ≠ 2×半宽
/// 实测列宽、用户自选比例字体）下列模型的「1 列 = char_w」不再成立，
/// 用列模型断行会在每段尾部留下系统性空白（第 75 轮实测：28px 字号
/// CJK 文本段尾缺 ~10% ≈ 6 字符）。真实 xs 由控件层每帧注入（与正文
/// 绘制同段落、同 shaping），无注入时调用方回退列模型 [`wrap_breaks`]。
///
/// * `max_px` = 段宽预算（像素）；≤0 防御为 1px；
/// * 单字符超宽（emoji/超大字）自动自成一格，不产生空段（k==prev 时
///   强制容纳，下一字符起再判）。
pub(crate) fn pixel_breaks(xs: &[f32], max_px: f32, body: &str) -> Vec<usize> {
    let max_px = max_px.max(1.0);
    let chars: Vec<char> = body.chars().collect();
    // 防御：契约上 xs.len() == 字符数 + 1，越界时按可用前缀截断
    let n = chars.len().min(xs.len().saturating_sub(1));
    let mut breaks = vec![0usize];
    let mut prev = 0usize;
    for k in 0..n.saturating_sub(1) {
        if k > prev && xs[k + 1] - xs[prev] > max_px {
            let b = choose_break(&chars, prev, k);
            breaks.push(b);
            prev = b;
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
            let j = i + i.isolate_lowest_one();
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
    ///
    /// * `max_cols` = 列模型预算（`real_xs` 为 None 时使用）；
    /// * `max_px` = 像素预算（`real_xs` 为 Some 时使用，P96）；
    /// * `real_xs` = 真实字形逐字符 x（与绘制同源；None = 列模型回退）。
    pub(crate) fn set_line(
        &mut self,
        line: usize,
        body: &str,
        max_cols: usize,
        max_px: f32,
        real_xs: Option<&[f32]>,
    ) -> Rc<Vec<usize>> {
        if let Some((g, b)) = self.memo.get(&line) {
            if *g == self.gen {
                return b.clone();
            }
        }
        let breaks = Rc::new(match real_xs {
            // P96：真实字形断行——段尾 = xs[段末字符右缘] ≤ max_px，
            // 非等宽/CJK 字体下也贴满右缘（列模型对非常规字距系统性
            // 留白：28px 字号 CJK 实测段尾缺 ~10% ≈ 6 字符）
            Some(xs) => pixel_breaks(xs, max_px, body),
            None => wrap_breaks(body, max_cols),
        });
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

    fn bit_add(&mut self, i: usize, d: i64) {
        let mut j = i + 1; // 转 1-based
        while j < self.bit.len() {
            self.bit[j] += d;
            j += j.isolate_lowest_one();
        }
    }

    fn bit_prefix(&self, i: usize) -> u32 {
        let mut sum = 0i64;
        let mut j = i; // 前 i 行 = 1-based 前缀 i
        while j > 0 {
            sum += self.bit[j];
            // ⚠️ 步进是「减去 lowbit」：j &= -j 会停在 lowbit 自身造成
            // 无限循环（第 72 轮实测 3 测试挂起 60s+ 定位）
            j -= j.isolate_lowest_one();
        }
        sum.max(0) as u32
    }
}

/// 接线态包装（第 73 轮 Phase 1 接线）：把 [`WrapIndex`] 与软换行开关、
/// 列预算、行数同步绑在一起，供 `EditorCore` 以 `RefCell` 持有。
///
/// 同步规则（设计 §3.2 落地）：
/// * 开关开启 = 整表重置（`enable`）；
/// * 内容代次失效（`after_edit`）：行数变化 → 整表重置；否则只推 gen，
///   由下次查询懒惰重算（未查询行 BIT 短暂陈旧随窗口收敛，v1 已披露）；
/// * 列预算/像素预算/行数漂移（窗口缩放、字号、gutter 变宽、行数变化
///   漏网）在每次查询入口 `ensure_synced` 兜底全清。
pub(crate) struct WrapCache {
    pub(crate) enabled: bool,
    /// 最近同步的显示列预算（≥1，列模型回退路径用）。
    pub(crate) max_cols: usize,
    /// 最近同步的段宽像素预算（真实字形断行路径用，P96）。
    pub(crate) max_px: f32,
    pub(crate) index: WrapIndex,
    /// 最近同步的逻辑行数。
    last_lines: usize,
}

impl WrapCache {
    pub(crate) fn new() -> Self {
        Self {
            enabled: false,
            max_cols: 1,
            max_px: 100.0,
            index: WrapIndex {
                bit: vec![0],
                seg_now: vec![],
                memo: HashMap::new(),
                gen: 0,
            },
            last_lines: 0,
        }
    }

    /// 开启软换行：整表重置起步（每行暂记 1 段，可见行随查询填充收敛）。
    pub(crate) fn enable(&mut self, lines: usize, max_cols: usize, max_px: f32) {
        self.enabled = true;
        self.max_cols = max_cols.max(1);
        self.max_px = max_px.max(1.0);
        self.index.reset(lines);
        self.last_lines = lines;
    }

    /// 关闭软换行：只翻开关，缓存原地保留（重新开启时 enable 全清）。
    pub(crate) fn disable(&mut self) {
        self.enabled = false;
    }

    /// 编辑汇点：行数变化 → 整表重置；否则代次失效（memo 全部过期，
    /// BIT 保留旧计数，由窗口行重算差值收敛——模块注释的 v1 取舍）。
    pub(crate) fn after_edit(&mut self, lines: usize) {
        if lines != self.last_lines {
            self.index.reset(lines);
            self.last_lines = lines;
        } else {
            self.index.bump_gen();
        }
    }

    /// 查询入口兜底同步：列/像素预算或行数与现状不符（窗口缩放/字号变更/
    /// gutter 变宽/行数变化漏网）即整表重置（设计 §3.2「宽 W 变化→全清」）。
    pub(crate) fn ensure_synced(&mut self, lines: usize, max_cols: usize, max_px: f32) {
        let mc = max_cols.max(1);
        let mpx = max_px.max(1.0);
        if mc != self.max_cols || (mpx - self.max_px).abs() > 0.5 {
            self.max_cols = mc;
            self.max_px = mpx;
            self.index.reset(lines);
            self.last_lines = lines;
        } else if lines != self.last_lines {
            self.index.reset(lines);
            self.last_lines = lines;
        }
    }

    /// 行 `line` 的当前断点表（memo 同代命中直接返回，否则重算并差值
    /// 更新 BIT）。调用方需先 `ensure_synced`。
    ///
    /// `real_xs` = 该行与绘制同源的真实字形 x（`shape_row_xs` 输出，
    /// 行字符数 + 1 项）；Some 时按像素预算断行（P96：非等宽字体下
    /// 段尾贴满真实右缘，杜绝列模型的系统性空白），None 回退列模型。
    pub(crate) fn segments_of(
        &mut self,
        line: usize,
        body: &str,
        real_xs: Option<&[f32]>,
    ) -> Rc<Vec<usize>> {
        self.index.set_line(line, body, self.max_cols, self.max_px, real_xs)
    }
}

/// 字符列 `col` 所在段序号：断点向量（段首列，恒含 0）中最后一个
/// `break ≤ col` 的下标；`col` 越界（≥ 行长）时归到末段。
pub(crate) fn segment_index(breaks: &[usize], col: usize, body_len: usize) -> usize {
    let c = col.min(body_len);
    breaks.partition_point(|&b| b <= c).saturating_sub(1)
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
            idx.set_line(i, b, 12, 10.0 * 12.0, None);
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
        idx.set_line(0, "aaaa", 2, 20.0, None); // 2 段
        assert_eq!(idx.total(), 2);
        idx.bump_gen();
        // 同代 memo 失效：同内容重算 → 结果一致（BIT 差值 0，不重复累加）
        idx.set_line(0, "aaaa", 2, 20.0, None);
        assert_eq!(idx.total(), 2, "同内容重算不得重复计段");
        // ⚠️ 内容突变必须先推进代次（接线契约：正文突变汇点调用
        // bump_gen）——否则同 gen 命中上次写入的 memo 会读到旧断点
        idx.bump_gen();
        idx.set_line(0, "aaaaaaa", 2, 20.0, None); // 4 段
        assert_eq!(idx.total(), 4, "内容变长：代次推进后重算，段数差值正确更新");
    }

    #[test]
    fn pixel_breaks_fills_to_pixel_budget_even_with_nonuniform_advances() {
        // 模拟非等宽：汉字 28px、半宽 15.6px（第 75 轮真实场景）。
        // 40 个汉字 = 1120px；max_px=955 → 段 0 应容纳 floor(955/28)=34
        // 个汉字（段尾 952 ≤ 955），段 1 从第 34 字符起——列模型
        // （2×15.6=31.2/字 → 30 字/段）会少塞 4 字留下 ~115px 空白，
        // 像素断行必须按真实 advance 塞满。
        let body = "中".repeat(40);
        let xs: Vec<f32> = (0..=40).map(|i| i as f32 * 28.0).collect();
        let breaks = pixel_breaks(&xs, 955.0, &body);
        assert_eq!(breaks, vec![0, 34], "段 0 恰容纳 34 汉字（952 ≤ 955）");
        // 段尾贴满：最后一段也 ≤ 预算（40 汉字全部放进 1 个后续段）
        assert_eq!(breaks[1], 34);
        // 恰好压线不产空段：100px 预算 + 每字符 25px → 4 字符/段
        let xs2: Vec<f32> = (0..=10).map(|i| i as f32 * 25.0).collect();
        assert_eq!(pixel_breaks(&xs2, 100.0, "abcdefghij"), vec![0, 4, 8]);
        // 单字符超宽自成一格（emoji）：200px 预算 + 首字符 300px；
        // 后续两个 1px 字符在预算内共段
        let xs3 = vec![0.0, 300.0, 301.0, 302.0];
        assert_eq!(pixel_breaks(&xs3, 200.0, "😀aa"), vec![0, 1]);
        // 空行
        assert_eq!(pixel_breaks(&[0.0], 100.0, ""), vec![0]);
    }

    #[test]
    fn wrap_breaks_v2_prefers_word_opportunities_and_kinsoku() {
        // 词边界（设计 §3.1 v2）：拉丁词在空格后断行，空格留在上一段尾，
        // 单词不被劈成两半；无机会的纯拉丁长词回退字符级断点（既有行为）
        assert_eq!(wrap_breaks("hello world", 8), vec![0, 6]);
        assert_eq!(wrap_breaks("abcdefgh", 4), vec![0, 4]);
        // 拉丁→CJK 边界：宽字符前可断，拉丁词整体留在上一段
        assert_eq!(wrap_breaks("abc中文", 4), vec![0, 3]);
        // 行首禁则：'、' 不得起段——断点回退一字，'う、' 一起下移
        assert_eq!(wrap_breaks("xあいう、えお", 8), vec![0, 3]);
        // 像素路径同享词边界优选：等宽 10px 的 "hello world"，80px 预算
        // 在 'r' 溢出 → 回退到空格后
        let xs: Vec<f32> = (0..=11).map(|i| i as f32 * 10.0).collect();
        assert_eq!(pixel_breaks(&xs, 80.0, "hello world"), vec![0, 6]);
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
        idx.set_line(0, "aaaaaaaa", 4, 40.0, None); // 2 段
        idx.set_line(1, "bb", 4, 40.0, None);
        assert_eq!(idx.total(), 3);
        // 行数变化：整表重置（每行 1 段起步）
        idx.reset(4);
        assert_eq!(idx.total(), 4);
        assert_eq!(idx.prefix_rows(3), 3);
        // 重新填充后收敛
        for (i, b) in ["a", "bbbbbbbb", "", "c"].iter().enumerate() {
            idx.set_line(i, b, 4, 40.0, None);
        }
        assert_eq!(idx.total(), 1 + 2 + 1 + 1);
    }

    #[test]
    fn segment_index_maps_col_to_its_visual_segment() {
        let breaks = [0usize, 4, 8];
        assert_eq!(segment_index(&breaks, 0, 12), 0);
        assert_eq!(segment_index(&breaks, 3, 12), 0);
        assert_eq!(segment_index(&breaks, 4, 12), 1);
        assert_eq!(segment_index(&breaks, 7, 12), 1);
        // col 越界（行尾/超界）→ 归末段
        assert_eq!(segment_index(&breaks, 8, 12), 2);
        assert_eq!(segment_index(&breaks, 99, 12), 2);
        // 空正文：只有 [0] 一个断点
        assert_eq!(segment_index(&[0], 0, 0), 0);
    }

    #[test]
    fn wrap_cache_enable_edit_and_ensure_sync_paths() {
        let mut wc = WrapCache::new();
        assert!(!wc.enabled);
        wc.enable(2, 4, 40.0);
        assert!(wc.enabled);
        assert_eq!(wc.index.total(), 2, "开启即整表重置，每行暂记 1 段");
        wc.segments_of(0, "aaaaaaaa", None); // 2 段（列模型）
        assert_eq!(wc.index.total(), 3);
        // 编辑但行数不变：只推代次，同一行重算后 BIT 差值收敛
        wc.after_edit(2);
        wc.segments_of(0, "a", None); // 缩成 1 段
        assert_eq!(wc.index.total(), 2);
        // 行数变化：整表重置
        wc.after_edit(3);
        assert_eq!(wc.index.total(), 3);
        // 像素预算路径：与列模型同预算的 xs 应给出贴合结果
        let xs: Vec<f32> = (0..=8).map(|i| i as f32 * 10.0).collect();
        wc.segments_of(1, "bbbbbbbb", Some(&xs)); // max_px 40 → 4 字符/段
        assert_eq!(wc.index.total(), 4);
        // 预算变化：兜底全清
        wc.ensure_synced(3, 9, 90.0);
        assert_eq!(wc.index.total(), 3, "预算变化整表重置为 1 段/行");
        // 关闭开关不影响缓存同步
        wc.disable();
        assert!(!wc.enabled);
        wc.after_edit(3);
        wc.segments_of(2, "x", None);
        assert_eq!(wc.index.total(), 3);
    }
}
