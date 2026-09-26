//! 基于 Rope 的文本文档。
//!
//! M0 阶段只提供最小接口，验证 ropey 的行为并建立测试基线；
//! M2 切换自绘渲染层时，`Document` 将成为编辑器的唯一数据源。

use ropey::Rope;

/// 行尾风格。加载时按全文统计检测主导行尾（P9），
/// 编辑层的回车插入、粘贴归一、退格/删除的 EOL 单元语义都以它为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// `\n` —— Unix 风格；空文档/无换行文本/平票时的保守默认。
    #[default]
    Lf,
    /// `\r\n` —— Windows 风格。
    CrLf,
    /// 孤立 `\r` —— 经典 Mac 风格（罕见；ropey 默认 `unicode_lines`
    /// 特性把它当行界，编辑器行号与查找坐标同源，仅用于归一化时不再
    /// 制造混合行尾）。
    Cr,
}

impl LineEnding {
    /// 统计文本的主导行尾：CRLF / LF / CR 三种计数，多者胜；
    /// 平票或全文没有换行时回退 [`LineEnding::Lf`]。
    ///
    /// ⚠️ L-21（第 216 轮，用户点单批准）：**按余量早停**——某一风格已经出现
    /// ≥ 4096 次（`EOL_DECISIVE_MIN`）且对第二名胜出 3 倍（`EOL_DECISIVE_RATIO`）
    /// 时即认定判定不再可能翻转，剩余文本不数。达不到门槛就一路数到底，
    /// 那种文件的行为与改前逐字相同（早停前是"整份都要数"）。
    /// 计数与判定与流式侧共用 `EolTally` 这一份实现。
    pub fn detect(text: &str) -> Self {
        let mut tally = EolTally::default();
        tally.feed(text);
        tally.settle()
    }

    /// 该风格对应的换行字符串。
    pub fn newline(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// 把任意来源文本里的换行（`\r\n`、`\n`、孤立 `\r`）统一改写为本风格。
    /// 粘贴、输入法上屏、按键字符在入文前都经过这里，从源头杜绝混合行尾。
    pub fn normalize(self, text: &str) -> String {
        let target = self.newline();
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    // \r\n 是一个逻辑换行：整体消费，避免改写成两个换行
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push_str(target);
                }
                '\n' => out.push_str(target),
                other => out.push(other),
            }
        }
        out
    }
}

/// 行尾早停的两道门槛（L-21）：换行次数下界，以及它对第二名的倍数。
///
/// 两个条件同时成立才认定"判定已悬殊"。4096 次这个下界保证：在**任何**后续
/// 输入下，第二名要翻盘都得再出现 ≥4096 次同类行界——那已经不是"这份文件
/// 的主导行尾"而是"两份不同来源拼起来的文件"。达不到就一路数到底。
const EOL_DECISIVE_MIN: usize = 4096;
/// 见 [`EOL_DECISIVE_MIN`]。
const EOL_DECISIVE_RATIO: usize = 3;

/// 行尾计数的**唯一**实现（L-21）：`LineEnding::detect`（一次性）与
/// `EolCounter`（流式，开档逐块喂）都走这里。
///
/// 改前这两处是**同一逻辑抄了两遍**——`EolCounter::finish` 的注释还写着
/// "多数判定与 `detect` 逐字一致"，那句是靠人盯的，不是结构保证（本仓第 7 次
/// 撞这个形状）。现在 `detect` 只是"把整份喂进同一个状态机再收尾"。
#[derive(Debug, Default)]
struct EolTally {
    crlf: usize,
    lf: usize,
    cr: usize,
    /// 上一块以 `\r` 结尾：它到底是 CRLF 的一半还是孤立 `\r`，要等下一块裁决
    pending_cr: bool,
    /// 已经判定 ⇒ 后续输入一律忽略（这就是省掉那 0.75 ms/MB 的地方）
    settled: Option<LineEnding>,
}

impl EolTally {
    /// 喂入一段文本（块大小任意、次数任意）。判定已定则 O(1) 返回。
    fn feed(&mut self, text: &str) {
        if self.settled.is_some() {
            return;
        }
        let mut chars = text.chars().peekable();
        // 先裁决上一块悬置的 `\r`
        if self.pending_cr {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    self.crlf += 1;
                    self.pending_cr = false;
                }
                Some(_) => {
                    // 孤立 `\r` 定案；当前字符留给下方常规循环处理
                    self.cr += 1;
                    self.pending_cr = false;
                }
                None => return, // 空块：继续悬置
            }
        }
        while let Some(c) = chars.next() {
            match c {
                '\r' => match chars.peek() {
                    Some('\n') => {
                        chars.next();
                        self.crlf += 1;
                    }
                    Some(_) => self.cr += 1,
                    None => {
                        // 块尾：悬置待下块裁决（与整份单遍切分逐字节等价）
                        self.pending_cr = true;
                        return;
                    }
                },
                '\n' => self.lf += 1,
                _ => continue, // 非行界字符不值得做一次判定检查
            }
            // 只在数到行界时检查，且先用下界那条最便宜的判断挡掉绝大多数
            if self.is_decisive() {
                self.settled = Some(self.majority());
                return;
            }
        }
    }

    /// 收尾：把悬置的 `\r` 定案后取多数。与 [`Self::feed`] 的早停判定同一条式子。
    fn settle(&mut self) -> LineEnding {
        if let Some(done) = self.settled {
            return done;
        }
        if self.pending_cr {
            self.cr += 1;
            self.pending_cr = false;
        }
        let v = self.majority();
        self.settled = Some(v);
        v
    }

    /// 多数判定（平票与"整份没有换行"都回退 LF）。`feed` 的早停与 `settle`
    /// 共用这一份，两者不可能各说一套。
    fn majority(&self) -> LineEnding {
        let (crlf, lf, cr) = (self.crlf, self.lf, self.cr);
        if crlf > lf && crlf > cr {
            LineEnding::CrLf
        } else if cr > lf {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }

    /// 是否已"悬殊到不必再数"：某一种 ≥ [`EOL_DECISIVE_MIN`] 且 ≥ 第二名的
    /// [`EOL_DECISIVE_RATIO`] 倍。
    fn is_decisive(&self) -> bool {
        let [mut top, mut second] = [0usize, 0usize];
        for n in [self.crlf, self.lf, self.cr] {
            if n > top {
                second = top;
                top = n;
            } else if n > second {
                second = n;
            }
        }
        top >= EOL_DECISIVE_MIN && top >= second.saturating_mul(EOL_DECISIVE_RATIO)
    }
}

/// 流式行尾计数器：开档按块喂入，判定一旦悬殊就停止计数。
#[derive(Debug, Default)]
pub struct EolCounter(EolTally);

impl EolCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推送一段解码后的文本（块大小任意、次数任意）。
    pub fn push(&mut self, text: &str) {
        self.0.feed(text);
    }

    /// 文本推送完毕，取主导行尾（多数判定与 [`LineEnding::detect`] 共用
    /// `EolTally` 这一份实现，结构上不可能分叉）。
    pub fn finish(mut self) -> LineEnding {
        self.0.settle()
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    rope: Rope,
    /// 主导行尾（P9）：`from_str` 时自动检测；编辑层用它统一换行语义。
    eol: LineEnding,
}

impl Document {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            eol: LineEnding::Lf,
        }
    }

    // clippy 豁免：与 std::str::FromStr::from_str 同名易混——但此处解析
    // 不可失败，FromStr 无从实现（需要 Err 类型），保留固有方法。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            eol: LineEnding::detect(text),
        }
    }

    /// 用已建好的 rope 与行尾组装文档。
    ///
    /// crate 内流式加载（P19）专用：rope 由 `RopeBuilder` 增量构建
    /// （全程只有 rope 一份正文），行尾由 [`EolCounter`] 跨块统计后传入。
    #[allow(dead_code)]
    pub(crate) fn from_parts(rope: Rope, eol: LineEnding) -> Self {
        Self { rope, eol }
    }

    /// 文档的主导行尾（编辑层据此归一插入文本、按 EOL 单元删除）。
    pub fn line_ending(&self) -> LineEnding {
        self.eol
    }

    /// 字符数（Unicode scalar 单位，与 ropey 一致）。
    pub fn text_len(&self) -> usize {
        self.rope.len_chars()
    }

    /// 字节数（UTF-8 编码口径，rope 根节点缓存，O(1)）。
    /// P31 心跳的大小节流按真实字节判——字符 ×3 的启发式会低估
    /// 4 字节 emoji 文档，这里不给启发式留误差空间。
    pub fn text_len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// 行数（末尾有换行时，换行后的空行也算一行，与多数编辑器一致）。
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn insert(&mut self, char_idx: usize, text: &str) {
        self.rope.insert(char_idx, text);
    }

    /// 删除 `[start_char, end_char)` 区间。
    pub fn remove_range(&mut self, start_char: usize, end_char: usize) {
        self.rope.remove(start_char..end_char);
    }

    pub fn to_text(&self) -> String {
        // L-17：按 `len_bytes` 预留一次，再逐块 `push_str`。改前走
        // `Rope::to_string()`（`Display` 那条 `to_string` 不预留容量），50MB
        // 文档实测 26.6 ms → 13.4 ms，过程中也不再出现「旧块＋翻倍新块」
        // 那一段额外峰值。正则档每次扫描都要付这份拷贝（见
        // `crate::find_all_regex_document`），所以这份账值得还。
        let mut out = String::with_capacity(self.rope.len_bytes());
        for chunk in self.rope.chunks() {
            out.push_str(chunk);
        }
        out
    }

    /// 按底层存储块零拷贝迭代正文。流式查找（P10）与将来的分块保存
    /// （P19）都以此为基础设施——不产生全文 String，内存峰值与文档大小无关。
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// 取第 `line` 行（0 起）的文本——将来虚拟化渲染按行取数的雏形：
    /// 只碰可见行，成本与全文大小无关。
    pub fn line_str(&self, line_idx: usize) -> String {
        self.rope.line(line_idx).to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    /// 两份文档**共同的开头**有多长，返回第一处不同的字符下标（按字符计）。
    /// 两串逐字节相同、或一方是另一方的前缀时，返回较短那方的字符长度。
    ///
    /// 给 undo/redo 用：正文整体换成历史快照时，只要开头那段字节完全相同，
    /// 那一段的高亮检查点就仍然有效（app 层 `EditorCore::undo` 据此将失效
    /// 点从第 0 行挪到真正的改动点）。成本 **O(共同前缀)**、与文档总长无关；
    /// 快路按存储块整段 `memcmp`，只在真正不同的那一块里逐字节定位。
    ///
    /// ⚠️ 判据是**字节**相同，不是"行数没变"：字节相同 ⇒ 字符相同 ⇒ 行界
    /// 与语法解析状态链逐格相同（UTF-8 自同步，前缀里的字节序列一致时字符
    /// 边界也一致，故返回的下标在两份文档里都落在合法字符边界上）。
    pub fn first_diff_char(&self, other: &Document) -> usize {
        let mut ait = self.rope.chunks();
        let mut bit = other.rope.chunks();
        let mut ac = ait.next();
        let mut bc = bit.next();
        let mut aoff = 0usize;
        let mut boff = 0usize;
        let mut pos = 0usize; // 已确认相同的字节数
        while let (Some(a), Some(b)) = (ac, bc) {
            // ⚠️ 必须**先转字节再切**：两块的剩余长度不等时，`min(len)` 那个
            // 切点可能落在较长一方的多字节字符中间，按 `&str` 切会直接 panic
            // （全量池里的三条 undo 模糊对拍用例就是这么逮到的）。
            let (ab, bb) = (&a.as_bytes()[aoff..], &b.as_bytes()[boff..]);
            let n = ab.len().min(bb.len());
            if ab[..n] == bb[..n] {
                pos += n;
                aoff += n;
                boff += n;
                if aoff == a.len() {
                    ac = ait.next();
                    aoff = 0;
                }
                if boff == b.len() {
                    bc = bit.next();
                    boff = 0;
                }
                continue;
            }
            // 唯一会逐字节走的一段：至多一块
            let k = (0..n).find(|&i| ab[i] != bb[i]).expect("上面已判不等");
            return self.rope.byte_to_char(pos + k);
        }
        // 一方是另一方的前缀：较短那方的末尾字节在较长那方里同样是字符边界
        let tail = pos.min(self.rope.len_bytes()).min(other.rope.len_bytes());
        self.rope.byte_to_char(tail)
    }

    /// 与另一文档做内容相等比较（P38 撤销回基线判定用）：长度不等直接
    /// 短路；等长时按底层存储块逐字节比对（ropey 的块级 ==，memcmp 量级），
    /// 全程无全文 String 分配。行尾元数据一并参与——编辑层虽不会原地
    /// 改写它，但「同文不同尾」的两个文档不应视为同一状态。
    pub fn content_eq(&self, other: &Document) -> bool {
        self.eol == other.eol && self.rope == other.rope
    }

    // ---------- 行列 <-> 字符偏移换算（自绘编辑器的定位基础） ----------

    /// 第 `line_idx` 行首字符的全文字符偏移。
    pub fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx)
    }

    /// 全文字符偏移所在的行号。
    pub fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx)
    }

    /// 全文字符偏移 → UTF-8 字节偏移（原生 str 搜索结果换算回字符
    /// 口径用；`char_idx` 越界时夹到文末，与 `line_to_char` 同口径）。
    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.rope.len_chars()))
    }

    /// UTF-8 字节偏移 → 全文字符偏移（要求 `byte_idx` 落在字符边界，
    /// 调用方保证；`match_indices`/词边界锚点天然满足）。
    pub fn byte_to_char(&self, byte_idx: usize) -> usize {
        self.rope.byte_to_char(byte_idx)
    }

    /// 取 `[start, end)` 字符偏移区间的文本（选区读取用）。
    pub fn slice_text(&self, start_char: usize, end_char: usize) -> String {
        self.rope.slice(start_char..end_char).to_string()
    }

    /// 第 `line_idx` 行的字符数（含行尾换行符，若有）。
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        self.rope.line(line_idx).len_chars()
    }

    /// 第 `line_idx` 行**不含行尾换行符**的字符数（P284）。
    ///
    /// 与 [`Self::line_str`] 再 `trim_end_matches(['\n','\r'])` 后数字符完全
    /// 等价，但**不扫全行**：ropey 把 `\r` 也算行 break ⇒ 一行的正文不可能以
    /// `\r`/`\n` 结尾，行尾只可能是 `\n` / `\r\n` / `\r` / 空（末行无换行），
    /// 所以从行末往回数最多看两个字符就够。长行文档（单行日志/压缩文件）里
    /// 这是每帧"这行有多少字符"从 O(行长) 变成常数的关键。
    /// 行号越界返回 0（与 `line_str` 的空串口径一致）。
    pub fn line_body_len_chars(&self, line_idx: usize) -> usize {
        if line_idx >= self.rope.len_lines() {
            return 0;
        }
        let line = self.rope.line(line_idx);
        let n = line.len_chars();
        let mut end = n;
        // 最多剥两个字符：`\r\n`。再多一个都不可能是行尾（见上）。
        for _ in 0..2 {
            if end == 0 {
                break;
            }
            match line.char(end - 1) {
                '\n' | '\r' => end -= 1,
                _ => break,
            }
        }
        end
    }

    /// 从 `char_idx`（含）向后的字符迭代器（第 61 轮括号匹配用；
    /// 零拷贝——rope 叶片级迭代，不产生全文 String）。
    /// 注：ropey 1.6 的 Chars 不支持 DoubleEndedIterator，反向扫描由
    /// app 层分块切片实现（见 EditorCore 的括号匹配）。
    pub fn chars_from(&self, char_idx: usize) -> impl Iterator<Item = char> + '_ {
        self.rope.chars_at(char_idx.min(self.rope.len_chars()))
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P304：`first_diff_char` 是"撤销只作废改动点之后"那步的地基，逐格钉住：
    /// 两串相同／一方是另一方的前缀／中间不同／交界处在多字节字符里／
    /// **两份 rope 的块边界不对齐**（编辑后 rope 会重排叶片——这是最容易
    /// 写成"逐块比第 k 块 vs 第 k 块"的地方，必须按绝对偏移对齐）。
    #[test]
    fn first_diff_char_aligns_across_uneven_chunk_bounds() {
        let cases: &[(&str, &str, usize)] = &[
            ("", "", 0),
            ("", "x", 0),
            ("abc", "abc", 3),
            ("abc", "abd", 2),
            ("abc", "abcd", 3),
            ("abcd", "abc", 3),
            ("中x", "中y", 1),
            ("中x", "文x", 0),
            ("\u{1F680}\u{1F680}b", "\u{1F680}\u{1F680}c", 2),
            ("a\r\nb", "a\nb", 1),
            // ⚠️ 两串字节长度不等、且切点（=较短者的长度）落在较长一方的
            // 多字节字符中间：这一格专门钉"先转字节再切"那句——按 `&str`
            // 切会 panic，而模糊对拍用例曾真的 panic 过。
            ("中文a", "中x", 1),
            ("中x", "中文a", 1),
        ];
        for (x, y, want) in cases {
            let a = Document::from_str(x);
            let b = Document::from_str(y);
            assert_eq!(a.first_diff_char(&b), *want, "{x:?} vs {y:?}");
            assert_eq!(b.first_diff_char(&a), *want, "反向必须同值：{x:?}/{y:?}");
        }

        // 块边界不对齐：5000 行文档在第 4000 行中间插一个字符，
        // 编辑后的 rope 叶片与原树不再逐块对齐，共同前缀必须精确等于插入点。
        let text = "line\n".repeat(5_000);
        let before = Document::from_str(&text);
        let mut after = Document::from_str(&text);
        let at = after.line_to_char(4_000) + 2;
        after.insert(at, "Z");
        assert_eq!(before.first_diff_char(&after), at, "插入点之前逐字符相同");
        assert_eq!(after.first_diff_char(&before), at, "反向同值");
        // 同一形状的不对齐，但正文是多字节 ⇒ 块边界落点与字符边界互相错开
        let cjk = "中文行\n".repeat(3_000);
        let cbefore = Document::from_str(&cjk);
        let mut cafter = Document::from_str(&cjk);
        let cat = cafter.line_to_char(2_500) + 1;
        cafter.insert(cat, "Z");
        assert_eq!(
            cbefore.first_diff_char(&cafter),
            cat,
            "多字节正文＋块不对齐"
        );
        assert_eq!(cafter.first_diff_char(&cbefore), cat, "反向同值");
        assert_eq!(
            before.first_diff_char(&before),
            before.text_len(),
            "同一份文档 ⇒ 差异落在末尾"
        );
    }

    /// L-21（第 216 轮）：早停**真的停**——决定性前缀之后的尾段一颗都不该数。
    /// 判据用计数器的读数本身（`lf == 0`），比"扫了多少字节"这种仪表更硬：
    /// 尾段有 5 万颗 `\n`，只要数过就一定看得见。
    #[test]
    fn eol_early_stop_leaves_the_tail_uncounted() {
        let text = format!("{}\n{}", "a\r\n".repeat(5_000), "b\n".repeat(50_000));
        let mut t = EolTally::default();
        t.feed(&text);
        let v = t.settle();
        assert_eq!(v, LineEnding::CrLf, "前缀已悬殊 ⇒ 判定取 CRLF");
        assert_eq!(t.lf, 0, "尾段那 5 万颗 LF 一颗都不该数（早停没生效）");
        assert_eq!(t.crlf, 4096, "应当在刚够悬殊的那一颗收工，不该数完 5000 颗");

        // 流式侧同一条性质：按 64 字节一块喂，也要在 4096 颗 CRLF 处停住
        let mut c = EolCounter::new();
        for chunk in text.as_bytes().chunks(64) {
            c.push(std::str::from_utf8(chunk).unwrap());
        }
        assert_eq!(c.finish(), LineEnding::CrLf);
    }

    /// L-21：不到"悬殊"就**不许**早停——此时必须数完全程，判定与改前逐字相同。
    ///
    /// ⚠️ 关键的一点（本轮被自己的测试纠正）：**悬殊与否是拿"已数过的部分"比的**。
    /// 所以"前 5000 行 CRLF、后 3000 行 LF"这种**分段排列**的文件会在第 4096 颗
    /// 就停（那时第二名还是 0，比值当然悬殊）——那正是 [`eol_early_stop_can_flip_a_two_source_file`]
    /// 钉住的那笔取舍。真正"不到悬殊"的是**交错**排列的混合文件：两种行界一起长，
    /// 谁都不到对方的 3 倍 ⇒ 一路数到底，答案与改前逐字相同。
    #[test]
    fn eol_stops_only_when_both_thresholds_are_met() {
        // ① 交错、比值不够悬殊：各 5000 颗 ⇒ 谁也没到对方的 3 倍 ⇒ 必须数完
        let mut interleaved = String::new();
        for _ in 0..5_000 {
            interleaved.push_str("a\r\nb\n");
        }
        let mut t = EolTally::default();
        t.feed(&interleaved);
        assert_eq!(
            t.settle(),
            LineEnding::Lf,
            "5000 对 5000 打平 ⇒ 回退 LF（平票口径未变）"
        );
        assert_eq!(t.crlf, 5_000, "不悬殊时该数到尾，实际只数了 {}", t.crlf);
        assert_eq!(t.lf, 5_000, "同上：LF 也该数满");

        // ② 悬殊但没到下界：交错到 1000 对（比值够不了，下界也没到）⇒ 数完
        let mut small = String::new();
        for _ in 0..1_000 {
            small.push_str("a\r\nb\nb\n");
        }
        let mut s = EolTally::default();
        s.feed(&small);
        assert_eq!(s.settle(), LineEnding::Lf, "LF 2000 > CRLF 1000");
        assert_eq!(s.crlf, 1_000, "没到下界时不许停");
        assert_eq!(s.lf, 2_000, "没到下界时不许停");

        // ③ 下界的另一侧：同一种行界连着来 ⇒ 恰好在第 4096 颗收工，
        //    后面再有多半篇同风格的也不数
        let edge = "a\r\n".repeat(9_096);
        let mut e = EolTally::default();
        e.feed(&edge);
        let _ = e.settle();
        assert_eq!(
            e.crlf, 4_096,
            "到界即停：不该把 9096 颗都数完（实际 {}",
            e.crlf
        );
    }

    /// L-21：流式与一次性**必须同源同值**——改前这是两份抄一遍的计数循环
    /// （注释还写着"逐字一致"，那是靠人盯的）。任何块大小、任何切点（含
    /// 恰好落在 `\\r\\n` 中间）都不许让两者分叉，早停也一样。
    #[test]
    fn eol_counter_matches_detect_at_every_chunk_size() {
        let cases = [
            "a\r\nb\nc\rd\n".to_owned(),
            "中文\r\n文\rc\n".to_owned(),
            "\r\n".to_owned(),
            "\r".to_owned(),
            "no breaks at all".to_owned(),
            // 过了早停下界的：一次性与分块都必须给出同一个答案
            "a\r\n".repeat(5_000) + &"b\n".repeat(50_000),
            "a\r\n".repeat(5_000),
            "a\n".repeat(5_000) + &"b\r\n".repeat(3_000),
        ];
        for text in &cases {
            let want = LineEnding::detect(text);
            // 按字符边界切块：块长 1/2 时切点会落在 `\r` 与 `\n` 中间，
            // 正是悬置裁决那条路；4096 那档则会跨过早停点。
            for size in [1usize, 2, 3, 40, 4_096] {
                let mut c = EolCounter::new();
                let mut buf = String::new();
                for ch in text.chars() {
                    buf.push(ch);
                    if buf.chars().count() >= size {
                        c.push(&buf);
                        buf.clear();
                    }
                }
                if !buf.is_empty() {
                    c.push(&buf);
                }
                assert_eq!(c.finish(), want, "{text:?} 按 {size} 字符切块分叉");
            }
        }
    }

    /// L-21：**明写的取舍**——早停会把"两份不同来源拼成的文件"的判定从
    /// "全文多数"改成"前缀多数"。这条测试把它钉成一个已知事实，而不是
    /// 等某天有人当 bug 修掉。门槛比值 3 ⇒ 前 4096 颗 CRLF 后哪怕全篇反向，
    /// 翻盘也需要对手再出现 ≥4096 颗同类——那已经是拼接文件而不是杂散混合。
    #[test]
    fn eol_early_stop_can_flip_a_two_source_file() {
        let text = format!("{}{}", "a\r\n".repeat(20_000), "b\n".repeat(20_000));
        // 全文口径（改前的算法，这里手算一份当参照）：CRLF 与 LF 打平 ⇒ 回退 LF
        assert_eq!(text.matches("\r\n").count(), 20_000);
        assert_eq!(text.matches("\n").count(), 40_000); // CRLF 里也含 \n
        assert_eq!(LineEnding::detect(&text), LineEnding::CrLf, "早停口径");
        // 同一份文件按流式喂也是同一个答案（两处共用一份实现，不许分叉）
        let mut c = EolCounter::new();
        for chunk in text.as_bytes().chunks(4_096) {
            c.push(std::str::from_utf8(chunk).unwrap());
        }
        assert_eq!(c.finish(), LineEnding::CrLf);
    }

    #[test]
    fn line_body_len_chars_matches_the_trimmed_string_on_every_ending() {
        // P284 的对拍：`line_body_len_chars` 只看行末两个字符，而参照口径是
        // "物化整行再 trim_end 掉所有 \n/\r 然后数"。行尾形态只有四种
        // （`\n` / `\r\n` / `\r` / 末行无换行），但"只看两个字符够不够"要由
        // 对拍说，不由推理说。
        let cases = [
            ("a\r\nbb\ncc", "CRLF 开头 + LF + 末行无换行"),
            ("plain\n", "全 LF"),
            (
                "one\rtwo\r\n",
                "混 CR 与 CRLF（ropey 里 \\r 本身也是行分隔）",
            ),
            ("no trailing newline", "整个文档无换行"),
            ("", "空文档"),
            ("\n\n", "只有两个空行"),
            ("中文字\ttab\nemoji 🚀 end\r\n", "宽字符 / Tab / emoji"),
            ("x\n\ny", "中间夹空行"),
        ];
        for (text, label) in cases {
            let doc = Document::from_str(text);
            for i in 0..doc.line_count() {
                let fast = doc.line_body_len_chars(i);
                let slow = doc
                    .line_str(i)
                    .trim_end_matches(['\n', '\r'])
                    .chars()
                    .count();
                assert_eq!(
                    fast,
                    slow,
                    "{label}：第 {i} 行（原始行 {:?}）快速口径 {fast} != 参照 {slow}",
                    doc.line_str(i)
                );
            }
            // 越界恒 0，与 `line_str` 返回空串的口径一致
            assert_eq!(
                doc.line_body_len_chars(doc.line_count() + 7),
                0,
                "{label}：越界行号须给 0"
            );
        }
    }

    #[test]
    fn insert_and_remove_keep_line_index_consistent() {
        let mut doc = Document::from_str("hello\nworld\n");
        assert_eq!(doc.line_count(), 3);
        assert_eq!(doc.line_str(1), "world\n");

        // 在第一行中间插入带换行的文本
        doc.insert(5, ",\neditpad");
        assert_eq!(doc.line_count(), 4);
        assert_eq!(doc.line_str(0), "hello,\n");
        assert_eq!(doc.line_str(1), "editpad\n");

        doc.remove_range(0, 6);
        assert_eq!(doc.to_text(), "\neditpad\nworld\n");
        assert_eq!(doc.line_count(), 4);
    }

    #[test]
    fn empty_document_basics() {
        let mut doc = Document::new();
        assert!(doc.is_empty());
        assert_eq!(doc.text_len(), 0);
        assert_eq!(doc.line_count(), 1);

        doc.insert(0, "中文测试");
        assert_eq!(doc.text_len(), 4);
        assert_eq!(doc.line_str(0), "中文测试");
    }

    #[test]
    fn line_char_offset_conversion_roundtrip() {
        let doc = Document::from_str("ab\ncd中文\n");
        assert_eq!(doc.line_count(), 3);

        assert_eq!(doc.line_to_char(0), 0);
        assert_eq!(doc.line_to_char(1), 3);
        // "ab\n"(3) + "cd中文"(4) = 7，第 2 行（末尾空行）从偏移 8 开始
        assert_eq!(doc.line_to_char(2), 8);

        assert_eq!(doc.char_to_line(0), 0);
        assert_eq!(doc.char_to_line(2), 0); // 换行符属于所在行的末尾
        assert_eq!(doc.char_to_line(3), 1);
        assert_eq!(doc.char_to_line(7), 1); // 第 1 行的换行符
        assert_eq!(doc.char_to_line(8), 2); // 文档长度处 = 末尾空行

        // 行长含换行符
        assert_eq!(doc.line_len_chars(0), 3);
        assert_eq!(doc.line_len_chars(2), 0);
    }

    #[test]
    fn clone_is_independent_snapshot() {
        let doc = Document::from_str("abc");
        let snapshot = doc.clone();
        // 将来撤销栈就靠这种廉价快照实现
        let _ = snapshot;
    }

    // ---------- P38 内容相等比较 ----------

    #[test]
    fn content_eq_compares_text_and_eol_metadata() {
        // 同源克隆：内容与行尾元数据全同
        let doc = Document::from_str("hello\r\nworld");
        assert!(doc.content_eq(&doc.clone()));

        // 长度不同的快速短路
        assert!(!doc.content_eq(&Document::from_str("hello\r\nworl")));
        // 等长但内容不同（块级逐字节比对路径）
        assert!(!doc.content_eq(&Document::from_str("hello\r\nworlD")));

        // 同文不同主导行尾元数据：不算同一状态
        assert!(!Document::from_str("a\nb").content_eq(&Document::from_str("a\r\nb")));

        // 跨块构建（流式加载的 from_parts 路径）与一次性构建结果一致：
        // 块边界不同也不影响比较正确性
        let mut builder = ropey::RopeBuilder::new();
        builder.append("分块");
        builder.append("构建的长文本");
        let streamed = Document::from_parts(builder.finish(), LineEnding::Lf);
        assert!(streamed.content_eq(&Document::from_str("分块构建的长文本")));
    }

    // ---------- P9 主导行尾 ----------
    #[test]
    fn line_ending_detection_picks_dominant_style() {
        use LineEnding::{Cr, CrLf, Lf};
        // 空文本 / 无换行文本 → 保守默认 LF
        assert_eq!(LineEnding::detect(""), Lf);
        assert_eq!(LineEnding::detect("no newline at all"), Lf);

        assert_eq!(LineEnding::detect("a\nb\nc"), Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\nc"), CrLf);
        assert_eq!(LineEnding::detect("a\rb\rc"), Cr);

        // 混合行尾：多者胜
        assert_eq!(
            LineEnding::detect("a\r\nb\r\nc\nd"),
            CrLf,
            "CRLF 多于 LF 应判 CRLF"
        );
        assert_eq!(
            LineEnding::detect("a\nb\nc\r\nd"),
            Lf,
            "LF 多于 CRLF 应判 LF"
        );

        // 三者平票 → 回退 LF
        assert_eq!(LineEnding::detect("a\r\nb\nc\rd"), Lf);
    }

    #[test]
    fn normalize_rewrites_every_eol_to_target() {
        let mixed = "a\r\nb\nc\rd";
        assert_eq!(LineEnding::Lf.normalize(mixed), "a\nb\nc\nd");
        assert_eq!(LineEnding::CrLf.normalize(mixed), "a\r\nb\r\nc\r\nd");
        assert_eq!(LineEnding::Cr.normalize(mixed), "a\rb\rc\rd");

        // \r\n 必须整体消费：不得膨胀成两个换行
        assert_eq!(LineEnding::Lf.normalize("x\r\ny"), "x\ny");
        assert_eq!(LineEnding::CrLf.normalize("x\ny"), "x\r\ny");

        // 无换行内容（含 4 字节 emoji）原样保留
        assert_eq!(LineEnding::CrLf.normalize("中文🚀"), "中文🚀");
        assert_eq!(LineEnding::Lf.normalize(""), "");
    }

    #[test]
    fn from_str_detects_and_preserves_crlf_content() {
        let doc = Document::from_str("first\r\nsecond\r\n");
        assert_eq!(doc.line_ending(), LineEnding::CrLf);
        // 内容字节原样保留——保存路径往返一致，不悄悄改写用户的行尾
        assert_eq!(doc.to_text(), "first\r\nsecond\r\n");

        assert_eq!(Document::from_str("a\nb\n").line_ending(), LineEnding::Lf);
        assert_eq!(
            Document::new().line_ending(),
            LineEnding::Lf,
            "空文档默认 LF"
        );

        // 克隆快照（撤销栈的基础）必须带着同一行尾元数据
        let snapshot = doc.clone();
        assert_eq!(snapshot.line_ending(), LineEnding::CrLf);
    }
}
