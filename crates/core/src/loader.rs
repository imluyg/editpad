//! 文件加载与编码识别。
//!
//! M0 为一次性读取（中小文件足够）；M1 会升级为后台线程流式加载 + 进度回报，
//! 届时 UI 线程任何时刻的阻塞都不超过一帧。

use std::fs;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::str;

use encoding_rs::{Encoding, GBK, UTF_16BE, UTF_16LE, UTF_8};
use ropey::RopeBuilder;

use crate::document::{Document, EolCounter};
use crate::error::CoreError;

/// 单块读取大小：进度回调的粒度。
const CHUNK_SIZE: usize = 64 * 1024;

/// GBK 兜底解码后 U+FFFD 占比超过该阈值即判为二进制：
/// 真正的 GBK/GB18030 文本几乎不含替换符，而压缩包/可执行文件的随机字节
/// 会被大量替换成 U+FFFD（远超 1%）。
const BINARY_REPLACEMENT_RATIO: f32 = 0.01;

/// 加载结果：解码后的全文 + 实际使用的编码标签（显示在状态栏）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedText {
    pub text: String,
    pub encoding: &'static str,
    /// 是否被识别为二进制内容（P1 防护）。`load_file`/`load_file_streaming`
    /// 会把它转成 [`CoreError::BinaryDetected`] 拒绝打开；直接调用 [`decode`]
    /// 的场合需自行检查该标记，避免把乱码写回磁盘损毁原文件。
    pub is_binary: bool,
}

pub fn load_file(path: &Path) -> Result<LoadedText, CoreError> {
    let bytes = fs::read(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    reject_binary(path, decode(&bytes))
}

/// 流式加载的进度快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadProgress {
    pub bytes_read: u64,
    pub total_bytes: u64,
}

/// 后台流式加载：按 64KB 块读取并逐块回调进度，UI 线程全程只收轻量事件。
///
/// M1 的实现把解码放在读完后统一进行（内存占用与一次性读取相同），
/// 但进度回调让 UI 可以立即响应、显示进度条；真正的增量解码随 M2
/// 切换 rope 数据源时一并落地。
pub fn load_file_streaming<F>(path: &Path, mut on_progress: F) -> Result<LoadedText, CoreError>
where
    F: FnMut(LoadProgress),
{
    let file = fs::File::open(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let total_bytes = file
        .metadata()
        .map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?
        .len();

    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file);
    let mut buffer: Vec<u8> = Vec::with_capacity(total_bytes as usize);
    let mut chunk = vec![0u8; CHUNK_SIZE];

    loop {
        let n = reader.read(&mut chunk).map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        on_progress(LoadProgress {
            bytes_read: buffer.len() as u64,
            total_bytes,
        });
    }

    reject_binary(path, decode(&buffer))
}

// ---------- P19 行动项 2：流式加载直入 rope ----------
//
// 旧路径 load_file_streaming 的内存峰值 ≈ 文件 ×3（整读 Vec<u8> +
// 解码全量 String + 复制进 rope）；本节把 rope 构建搬进读取循环，
// 全程只有 rope 一份正文（+64KB 块缓冲），峰值降到 ~×1.05。
//
// 编码判定语义与 [`decode`] 完全一致（BOM 分流 → NUL → 严格 UTF-8 →
// GBK 兜底 + U+FFFD 占比），差异只在实现方式：
// * NUL/UTF-8 校验改为「第一遍流式扫描」——不保留任何字节，
//   NUL 一旦出现立即短路拒绝，全文检测语义不缩水；
// * UTF-8 合法时第二遍边读边推进 `RopeBuilder`；
// * 非 UTF-8 时第二遍用 encoding_rs 增量解码器（跨块序列自动缝合）。

/// 流式加载直入 rope 的结果（P19）。
///
/// `sample` 是解码后正文的前若干字符（≤4KB），供 P22 无扩展名
/// 内容嗅探使用——对 UTF-16 等宽编码文件同样有效（嗅探看解码文本）。
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub doc: Document,
    pub encoding: &'static str,
    /// 解码后文本头部样本（供语言嗅探）
    pub sample: String,
}

/// 流式加载为文档：内存峰值与文件大小近似线性（≈rope 本身）。
///
/// 进度回调语义：无 BOM 文件经历「校验遍」与「装载遍」两趟读取，
/// 进度被映射到前半程/后半程保持单调递增至 total；BOM 文件单趟直达。
pub fn load_document_streaming<F>(
    path: &Path,
    mut on_progress: F,
) -> Result<LoadedDocument, CoreError>
where
    F: FnMut(LoadProgress),
{
    let file = fs::File::open(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let total_bytes = file
        .metadata()
        .map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?
        .len();

    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file);
    let mut chunk = vec![0u8; CHUNK_SIZE];

    // 读满第一块（read 允许少读），嗅探 BOM 决定解码策略
    let mut first_len = 0usize;
    while first_len < CHUNK_SIZE {
        let n = reader.read(&mut chunk[first_len..]).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        bump_read(n); // 首块的真实读入量在这里，后续消费者不得重复记账
        first_len += n;
    }
    // O-9：进度首报**移进各分派支**。若在分派之前按原始字节数无条件报一次
    // 首块（64KB），而 ScanThenBuild 之后把读量折算到 [0, 3/4] 程，两者就会
    // 构成**进度倒退**（65536 → 49152），破坏 `load_document_streaming` 的
    // 单调契约——既有 streaming_reports_monotonic_progress_and_decodes 挡着。
    enum Plan {
        /// 单趟：增量解码即装载（进度线性）
        Direct(&'static Encoding, &'static str, usize),
        /// 先全量扫描（NUL + 严格 UTF-8），再按结论二选一装载
        ScanThenBuild,
    }

    let plan = if chunk.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Plan::Direct(UTF_8, "UTF-8(BOM)", 3)
    } else if chunk.starts_with(&[0xFF, 0xFE]) {
        Plan::Direct(UTF_16LE, "UTF-16LE", 2)
    } else if chunk.starts_with(&[0xFE, 0xFF]) {
        Plan::Direct(UTF_16BE, "UTF-16BE", 2)
    } else {
        Plan::ScanThenBuild
    };

    match plan {
        Plan::Direct(encoding, label, bom_len) => {
            // BOM / UTF-16 单趟直达：进度线性，首报仍是原始字节数（与改前一致）
            report(&mut on_progress, first_len as u64, total_bytes);
            let mut head = HeadSample::default();
            // P265：`UTF-8(BOM)` 这一支必须**统计**替换字符并按同一阈值判二进制。
            // 改前一律传 `None`：贴了 BOM 的文件里凡是非法 UTF-8 序列都被 lossy
            // 换成 U+FFFD 无声收下，用户一按保存就把这堆损毁文本当原文写回磁盘
            // （与 `decode` 那侧同源，见 [`decode_body`] 的说明）。
            // UTF-16LE/BE 两支**继续不判**：那里的 0x00 是合法字符字节，
            // 「含 NUL 即二进制」这条判据对宽编码根本不成立。
            let guard_replacements = label == "UTF-8(BOM)";
            let mut stats = BuildStats::default();
            let doc = build_pass(
                &mut reader,
                path,
                &chunk[..first_len],
                bom_len,
                encoding,
                total_bytes,
                0,
                total_bytes,
                &mut on_progress,
                if guard_replacements {
                    Some(&mut stats)
                } else {
                    None
                },
                &mut head,
            )?;
            if guard_replacements
                && stats.replacements as f32 / stats.chars.max(1) as f32 > BINARY_REPLACEMENT_RATIO
            {
                return Err(CoreError::BinaryDetected {
                    path: path.to_path_buf(),
                });
            }
            Ok(LoadedDocument {
                doc,
                encoding: label,
                sample: head.buf,
            })
        }
        Plan::ScanThenBuild => {
            // O-9：校验遍与装载遍**融成一趟**——同一遍读取里既喂 Utf8Scan
            // （NUL 短路 + 严格 UTF-8 增量校验），又把字节喂进 UTF-8 解码器
            // 推进 rope。校验通过即装载完成：读字节数 = 文件大小（旧写法是
            // 2 倍），也不再需要「第二趟重开文件」。
            // 只有真不是 UTF-8 时才丢弃这段乐观 rope、在**同一 fd** 上 seek
            // 回起点走 GBK 兜底——GBK 文本通常在头几 KB 就破格，被丢弃的部分
            // 占比可忽略；而重开文件曾是 TOCTOU：两趟之间被构建脚本/云盘同步
            // 改写，第一趟得出的「UTF-8 合法」结论会落到第二趟的新字节上。
            let utf8_span = total_bytes - total_bytes / 4; // 单趟映射到 [0, 3/4]
            let scanned = scan_and_build_utf8(
                &mut reader,
                path,
                &chunk[..first_len],
                total_bytes,
                utf8_span,
                &mut on_progress,
            )?;
            match scanned {
                Some(loaded) => Ok(loaded),
                None => {
                    // 回卷重读：用同一个已打开的 fd，不重新 open
                    let mut file = reader.into_inner();
                    file.seek(std::io::SeekFrom::Start(0))
                        .map_err(|source| CoreError::Read {
                            path: path.to_path_buf(),
                            source,
                        })?;
                    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file);
                    let first_len = fill_chunk(&mut reader, &mut chunk).map_err(io_err(path))?;
                    let mut stats = BuildStats::default();
                    let mut head = HeadSample::default();
                    let doc = build_pass(
                        &mut reader,
                        path,
                        &chunk[..first_len],
                        0,
                        GBK,
                        total_bytes,
                        utf8_span,
                        total_bytes - utf8_span,
                        &mut on_progress,
                        Some(&mut stats),
                        &mut head,
                    )?;
                    if stats.replacements as f32 / stats.chars.max(1) as f32
                        > BINARY_REPLACEMENT_RATIO
                    {
                        return Err(CoreError::BinaryDetected {
                            path: path.to_path_buf(),
                        });
                    }
                    Ok(LoadedDocument {
                        doc,
                        encoding: "GBK",
                        sample: head.buf,
                    })
                }
            }
        }
    }
}

/// 单趟「边扫边装」：字节流同时喂 [`Utf8Scan`] 与 UTF-8 增量解码器。
///
/// 返回 `Some(loaded)` 表示全文严格 UTF-8 合法、rope 已就地建成；返回 `None`
/// 表示**不是** UTF-8（调用方回卷走 GBK 兜底）——此时本函数内部构造的 rope
/// 被直接丢弃，不产生任何副作用。NUL 一律 [`CoreError::BinaryDetected`]。
///
/// 解码器与 [`build_pass`] 用同一个构造（`encoding.new_decoder()`）、收尾同
/// 一个 `absorb(.., last=true, stats=None, ..)`，故与「先扫一遍再 build_pass
/// (UTF_8)」的旧两趟写法**逐字节等价**（`load_document_streaming` 与 `decode`
/// 的对拍用例即钉这一点）。分块边界不影响结论：两者都是跨块增量的状态机。
#[allow(clippy::too_many_arguments)]
fn scan_and_build_utf8<F>(
    reader: &mut BufReader<fs::File>,
    path: &Path,
    first_chunk: &[u8],
    total_bytes: u64,
    progress_span: u64,
    on_progress: &mut F,
) -> Result<Option<LoadedDocument>, CoreError>
where
    F: FnMut(LoadProgress),
{
    let mut scan = Utf8Scan::default();
    let mut builder = RopeBuilder::new();
    let mut eol = EolCounter::new();
    let mut decoder = UTF_8.new_decoder();
    let mut out = String::with_capacity(CHUNK_SIZE * 4);
    let mut head = HeadSample::default();
    let mut stats: Option<&mut BuildStats> = None; // UTF-8 路径不统计占比

    scan.feed(first_chunk);
    // 首块的读入量已由调用点记账，这里不重复计
    absorb(
        &mut decoder,
        first_chunk,
        false,
        &mut out,
        &mut builder,
        &mut eol,
        stats.as_deref_mut(),
        &mut head,
    );
    let mut done = first_chunk.len() as u64;
    // 乐观装载被放弃的标志：置位后**只读不装**（不解码、不再分配）
    let mut dropped = false;
    report(
        on_progress,
        scaled_done(done, total_bytes, progress_span),
        total_bytes,
    );

    let mut chunk = vec![0u8; CHUNK_SIZE];
    loop {
        let n = fill_chunk(reader, &mut chunk).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        let bytes = &chunk[..n];
        scan.feed(bytes);
        // 首个非法序列一出现就就地丢弃已攒的 rope（换成新 builder 即释放旧块）
        // ——否则非 UTF-8 文件会「乐观攒完整篇再丢」，峰值变成两份 rope，
        // 直接顶破 P19 的内存预算（流式装载把峰值从 ×2.24 压到 ×1.22 就是它）。
        // ⚠️ 但**不能提前 break 回卷**：NUL 可能出现在破格点之后，今天的行为
        // 是「全文扫完再裁决」，短路会让含 NUL 的伪文本从「拒绝打开」变成
        // 「按 GBK 打开」——那是 P1 二进制防护的倒退（打开后保存即毁原文件）。
        if !dropped && scan.invalid {
            builder = RopeBuilder::new();
            head = HeadSample::default();
            dropped = true;
        }
        if !dropped {
            absorb(
                &mut decoder,
                bytes,
                false,
                &mut out,
                &mut builder,
                &mut eol,
                stats.as_deref_mut(),
                &mut head,
            );
        }
        done += n as u64;
        report(
            on_progress,
            scaled_done(done, total_bytes, progress_span),
            total_bytes,
        );
    }

    // 与旧写法同：扫完才裁决（不在 NUL 处提前 break，保持进度序列与结果一致）
    if scan.saw_nul {
        return Err(CoreError::BinaryDetected {
            path: path.to_path_buf(),
        });
    }
    if dropped || !scan.utf8_valid() {
        return Ok(None); // 丢弃乐观 rope，交调用方回卷走 GBK
    }
    // 冲刷解码器尾部（未完的多字节序列）——合法 UTF-8 下应为空操作，
    // 但与 build_pass 保持一致地执行，确保两条形形状完全相同
    absorb(
        &mut decoder,
        b"",
        true,
        &mut out,
        &mut builder,
        &mut eol,
        None,
        &mut head,
    );
    report(on_progress, total_bytes, total_bytes);
    Ok(Some(LoadedDocument {
        doc: Document::from_parts(builder.finish(), eol.finish()),
        encoding: "UTF-8",
        sample: head.buf,
    }))
}

/// 把已读字节数映射进 `[0, progress_span]`（O-9：单趟只走到 3/4，留给可能的
/// GBK 回卷重读走到满程，保证进度**单调不减**——`load_document_streaming`
/// 的既定契约）。
fn scaled_done(done: u64, total_bytes: u64, progress_span: u64) -> u64 {
    if total_bytes == 0 {
        return 0;
    }
    (done.min(total_bytes) * progress_span) / total_bytes
}

// 测试钩子（O-9 趟数契约）：实际读到的字节总数。
// 「打开一个 UTF-8 文件只读一遍」在返回值与落盘结果上都不可观测（两趟读法
// 给出同一份内容），耗时断言在本仓又不被接受 → 记字节数，形状同 app 层
// `EditorCore::line_text_calls`，生产构建整块不参与编译。
#[cfg(test)]
thread_local! {
    static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// 记账一次真实读入量。非测试构建里是空函数（编译器内联掉，零成本）。
#[cfg(test)]
fn bump_read(n: usize) {
    READ_BYTES.with(|c| c.set(c.get() + n as u64));
}

#[cfg(not(test))]
#[inline]
fn bump_read(_n: usize) {}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> CoreError + '_ {
    move |source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    }
}

fn report(on_progress: &mut dyn FnMut(LoadProgress), bytes_read: u64, total_bytes: u64) {
    on_progress(LoadProgress {
        bytes_read: bytes_read.min(total_bytes),
        total_bytes,
    });
}

/// 把 reader 读满一个块，返回实际字节数。
fn fill_chunk(reader: &mut BufReader<fs::File>, chunk: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0usize;
    while filled < chunk.len() {
        let n = reader.read(&mut chunk[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    bump_read(filled);
    Ok(filled)
}

/// 第一遍扫描的状态：NUL 是最强二进制信号（短路）；
/// 严格 UTF-8 校验用手写状态机（跨块续字节计数 + WHATWG 首续字节范围）。
struct Utf8Scan {
    saw_nul: bool,
    invalid: bool,
    /// 尚待接收的续字节数
    expect: u8,
    /// 下一个续字节的合法区间下/上界：首续字节按 lead 收窄
    /// （E0→A0..、ED→..9F、F0→90..、F4→..8F），其余续字节恒 80..=BF。
    /// 曾只查「是不是续字节」不查范围：超长编码 / CESU 代理对 /
    /// >U+10FFFF 被判合法，与 [`decode`]（from_utf8 严格口径）发散。
    next_min: u8,
    next_max: u8,
}

impl Default for Utf8Scan {
    fn default() -> Self {
        Self {
            saw_nul: false,
            invalid: false,
            expect: 0,
            // 全区间（派生 Default 会给 0/0，首续字节永不合法——实测踩坑）
            next_min: 0x80,
            next_max: 0xBF,
        }
    }
}

impl Utf8Scan {
    fn feed(&mut self, data: &[u8]) {
        if self.invalid {
            // 已判非 UTF-8：剩余唯一使命是把 NUL 扫完（语义对齐 decode()）
            self.saw_nul |= data.contains(&0);
            return;
        }
        // 带下标遍历：判定非法的那一刻，**本块剩余字节仍要扫一遍 NUL**——
        // 否则「非法序列在前、NUL 在后」的输入（无 BOM 的 UTF-16LE 中文最
        // 典型）会漏掉最多一个块长的 NUL，与 decode() 的整块检测分叉。
        for (i, &b) in data.iter().enumerate() {
            if b == 0 {
                self.saw_nul = true;
                self.invalid = true; // NUL 之后无需再校验 UTF-8
                return;
            }
            if self.expect > 0 {
                if b >= self.next_min && b <= self.next_max {
                    self.expect -= 1;
                    // 只有 lead 后的首个续字节范围收窄，后续回到全区间
                    self.next_min = 0x80;
                    self.next_max = 0xBF;
                } else {
                    self.expect = 0;
                    self.invalid = true;
                    self.saw_nul |= data[i + 1..].contains(&0);
                    return;
                }
            } else if b >= 0x80 {
                match b {
                    0xC2..=0xDF => self.expect = 1,
                    0xE0 => {
                        self.expect = 2;
                        self.next_min = 0xA0;
                    }
                    0xED => {
                        self.expect = 2;
                        self.next_max = 0x9F;
                    }
                    0xF0 => {
                        self.expect = 3;
                        self.next_min = 0x90;
                    }
                    0xF4 => {
                        self.expect = 3;
                        self.next_max = 0x8F;
                    }
                    0xE1..=0xEC | 0xEE..=0xEF => self.expect = 2,
                    0xF1..=0xF3 => self.expect = 3,
                    // 孤立续字节 / 过长 lead（C0/C1）/ 非法 lead（F5..FF）
                    _ => {
                        self.invalid = true;
                        self.saw_nul |= data[i + 1..].contains(&0);
                        return;
                    }
                }
            }
        }
    }

    /// 流结束后的最终判定：悬挂的半截多字节序列 = 非 UTF-8（与
    /// from_utf8 一致）。曾漏查 `expect > 0`——GBK 双字节字符大量落在
    /// 「E0-EF + 合法续字节」形态上，以截断序列结尾的 GBK 文件曾被判成
    /// UTF-8 解出 U+FFFD 而非走 GBK 兜底，保存后原字节被 FFFD 覆写。
    fn utf8_valid(&self) -> bool {
        !self.invalid && self.expect == 0
    }
}

/// 装载遍的统计（GBK 需要 U+FFFD 占比）。
#[derive(Default)]
struct BuildStats {
    replacements: usize,
    chars: usize,
}

/// 解码文本头部样本采集上限（字符数）：足够 shebang/XML/JSON/YAML 嗅探。
const SAMPLE_HEAD_CHARS: usize = 4096;

/// 采集解码输出的前 [`SAMPLE_HEAD_CHARS`] 个字符（P22 嗅探用）。
#[derive(Default)]
struct HeadSample {
    buf: String,
    /// `buf.chars().count()` 的增量记账（O-10）：改前每推一个字符就把整个
    /// 样本重数一遍，填满 4096 字符样本约 8.4M 次字符迭代——**每个文件打开
    /// 都固定付一次**，与文件大小无关。
    chars: usize,
    full: bool,
}

impl HeadSample {
    fn push(&mut self, text: &str) {
        if self.full {
            return;
        }
        for c in text.chars() {
            self.buf.push(c);
            self.chars += 1;
            if self.chars >= SAMPLE_HEAD_CHARS {
                self.full = true;
                break;
            }
        }
    }
}

/// 把一段解码输出吸收进构建器：rope 推送 + 行尾计数 + 占比统计。
// 与 build_pass 同款取舍：8 个参数多为管线上下文，打包结构体反而增加
// 摩擦，维持现状（见下方 build_pass 的 allow 先例）。
#[allow(clippy::too_many_arguments)]
fn absorb(
    decoder: &mut encoding_rs::Decoder,
    src: &[u8],
    last: bool,
    out: &mut String,
    builder: &mut RopeBuilder,
    eol: &mut EolCounter,
    stats: Option<&mut BuildStats>,
    head: &mut HeadSample,
) {
    use encoding_rs::CoderResult;
    out.clear();
    let mut rest = src;
    loop {
        // decode_to_string 只写进 dst 的备用容量：先保证余量 ≥ 最坏膨胀
        out.reserve(rest.len() * 4 + 4);
        let (result, read, _replaced) = decoder.decode_to_string(rest, out, last);
        rest = &rest[read..];
        match result {
            CoderResult::InputEmpty => break,
            CoderResult::OutputFull => {} // 已按需扩容，继续消费剩余输入
        }
        if read == 0 && !last {
            break; // 防御性兜底：避免零进度死循环
        }
    }
    builder.append(out);
    eol.push(out);
    head.push(out);
    if let Some(s) = stats {
        s.chars += out.chars().count();
        s.replacements += out.matches('\u{FFFD}').count();
    }
}

/// 第二遍装载：增量解码 → RopeBuilder + 行尾计数。返回组装好的文档。
///
/// * `first_chunk`/`skip`：已读入的首块与需跳过的 BOM 字节数；
/// * 进度映射到 `[progress_base, progress_base + progress_span]`；
/// * `stats`：GBK 路径传入以统计 U+FFFD 占比。
#[allow(clippy::too_many_arguments)]
fn build_pass(
    reader: &mut BufReader<fs::File>,
    path: &Path,
    first_chunk: &[u8],
    skip: usize,
    encoding: &'static Encoding,
    total_bytes: u64,
    progress_base: u64,
    progress_span: u64,
    on_progress: &mut dyn FnMut(LoadProgress),
    mut stats: Option<&mut BuildStats>,
    head: &mut HeadSample,
) -> Result<Document, CoreError> {
    let mut builder = RopeBuilder::new();
    let mut eol = EolCounter::new();
    let mut decoder = encoding.new_decoder();
    // 输出缓冲：最坏情况（UTF-16 双字节单元→代理对、GBK 双字节→3 字节）
    // 每输入字节膨胀 ≤3，CHUNK×4 绰绰有余
    let mut out = String::with_capacity(CHUNK_SIZE * 4);

    // 首块（跳过 BOM 字节）
    if first_chunk.len() > skip {
        absorb(
            &mut decoder,
            &first_chunk[skip..],
            false,
            &mut out,
            &mut builder,
            &mut eol,
            stats.as_deref_mut(),
            head,
        );
    }
    let mut done = first_chunk.len() as u64;

    let mut chunk = vec![0u8; CHUNK_SIZE];
    loop {
        let n = fill_chunk(reader, &mut chunk).map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        done += n as u64;
        absorb(
            &mut decoder,
            &chunk[..n],
            false,
            &mut out,
            &mut builder,
            &mut eol,
            stats.as_deref_mut(),
            head,
        );
        report(
            on_progress,
            progress_base + done.min(progress_span),
            total_bytes,
        );
    }
    // 冲刷解码器尾部（未完的多字节序列 / 未配对代理）
    // P216：stats 必须一并喂进去——悬在半路的尾字节的 U+FFFD **只**出现在这
    // 一次输出里。此前传 None，占比统计系统性少算，二进制防护（P1）在流式
    // 路径上形同虚设（详见 `load_document_streaming` 的判定处）。
    absorb(
        &mut decoder,
        b"",
        true,
        &mut out,
        &mut builder,
        &mut eol,
        stats,
        head,
    );
    report(on_progress, total_bytes, total_bytes);

    Ok(Document::from_parts(builder.finish(), eol.finish()))
}

/// 二进制防护：`LoadedText.is_binary` 为真时拒绝加载（转成错误），
/// 从根上杜绝「打开二进制 → 敲字符置脏 → 保存」把原文件替换成乱码的损毁链。
fn reject_binary(path: &Path, loaded: LoadedText) -> Result<LoadedText, CoreError> {
    if loaded.is_binary {
        return Err(CoreError::BinaryDetected {
            path: path.to_path_buf(),
        });
    }
    Ok(loaded)
}

/// 解码策略（顺序敏感）：
/// 1. UTF-8 BOM → **剥掉 BOM 后走与无 BOM 主路径同一份判据链**（`decode_body`），
///    只有正文确实是合法 UTF-8 时才把标签写成 `UTF-8(BOM)`；改前这里
///    `from_utf8_lossy` + 恒 `is_binary: false`，把两道二进制防护一起绕过（P265）；
/// 2. UTF-16 LE / BE BOM → 对应解码；
/// 3. 无 BOM：含 NUL 字节 → 判二进制（UTF-16 文件已在上面由 BOM 分流）；
/// 4. 严格 UTF-8 校验，通过即按 UTF-8；
/// 5. 校验失败 → GBK 兜底；U+FFFD 占比超阈值同样判二进制。
pub fn decode(bytes: &[u8]) -> LoadedText {
    if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let mut loaded = decode_body(body);
        // 只有正文真是合法 UTF-8 才配得上 "UTF-8(BOM)" 这个标签：标签一路
        // 决定状态栏、转码知情（P6）与自动保存选码（P263），不能替一坨
        // 解码失败的字节作证。GBK 兜底成功时标签保持 "GBK"（那才是它本来的
        // 编码，存回去也不该再贴 BOM）。
        if loaded.encoding == "UTF-8" {
            loaded.encoding = "UTF-8(BOM)";
        }
        return loaded;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let mut loaded = decode_with(UTF_16LE, &bytes[2..], "UTF-16LE");
        loaded.is_binary = false;
        return loaded;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let mut loaded = decode_with(UTF_16BE, &bytes[2..], "UTF-16BE");
        loaded.is_binary = false;
        return loaded;
    }
    decode_body(bytes)
}

/// 无 BOM 主体的判据链：NUL → 严格 UTF-8 → GBK 兜底 + U+FFFD 占比。
///
/// 抽成一处的理由是**共用**：P265 之前 UTF-8 BOM 分支自己走
/// `String::from_utf8_lossy` 并硬编 `is_binary: false`，于是
/// ① 含 NUL 的二进制只要前面贴个 BOM 就当文本载入，② 非法字节被 lossy 换成
/// U+FFFD 后既不打二进制标记、也没人统计占比——一个 GBK 文件被工具误加 BOM
/// 就变成一屏乱码进编辑器，用户一按保存就把这堆 U+FFFD 当"原文"写回磁盘。
/// 那是数据损毁，不是显示问题（与本仓 P216 那次"二进制防护被某条路径绕过"
/// 同族）。共用判据链之后这类"分叉只守一边"结构上不再可能。
fn decode_body(bytes: &[u8]) -> LoadedText {
    // NUL 字节是最强的二进制信号：任何合法文本编码（除带 BOM 的 UTF-16 外）
    // 都不会出现它。放在 UTF-16 分流之后，避免误伤 ASCII 段含 0x00 的宽编码。
    if bytes.contains(&0) {
        return LoadedText {
            text: String::new(),
            encoding: "binary",
            is_binary: true,
        };
    }
    match str::from_utf8(bytes) {
        Ok(text) => LoadedText {
            text: text.to_owned(),
            encoding: "UTF-8",
            is_binary: false,
        },
        // 不是合法 UTF-8，大概率是 GBK/GB18030；GBK 是其子集超集折中，M1 再细化。
        // 兜底解码永不失败（非法序列→U+FFFD），用占比区分「遗留中文文本」与「随机字节」。
        Err(_) => {
            let mut loaded = decode_with(GBK, bytes, "GBK");
            loaded.is_binary = replacement_ratio(&loaded.text) > BINARY_REPLACEMENT_RATIO;
            loaded
        }
    }
}

/// 文本中 U+FFFD（replacement character）所占比例。
fn replacement_ratio(text: &str) -> f32 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let bad = text.chars().filter(|&c| c == '\u{FFFD}').count();
    bad as f32 / total as f32
}

fn decode_with(encoding: &'static Encoding, bytes: &[u8], label: &'static str) -> LoadedText {
    // decode 返回 (解码文本, 实际使用的编码, 是否有替换错误)
    let (decoded, _, _) = encoding.decode(bytes);
    LoadedText {
        text: decoded.into_owned(),
        encoding: label,
        is_binary: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::LineEnding;

    /// 项目内落盘目录：系统 TEMP 在部分沙箱下不可写。
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("{tag}-{}", std::process::id()))
    }

    /// O-9 主护栏：非 BOM 的 UTF-8 文件**只读一遍**。
    ///
    /// 改前是无条件两趟（校验遍 + 装载遍），读入字节数 = 2 × 文件大小。这个
    /// 事实**无法**从返回值或落盘结果观测（两趟给出同一份内容），重开文件的
    /// TOCTOU 也只有并发写者才暴露，耗时断言本仓又不接受 → 记 `READ_BYTES`。
    /// 顺带钉住进度契约仍是「单调不减、末值到 total」（旧写法的两半程映射改成
    /// 单趟 [0, 3/4] + 终值 total，仍然单调）。
    #[test]
    fn document_streaming_reads_a_utf8_file_exactly_once() {
        let dir = scratch_dir("o9-onepass");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("u8.txt");
        let content = "行 1 Editpad\nhello world\n中文 🚀\n".repeat(6_000);
        fs::write(&target, &content).unwrap();
        let size = fs::metadata(&target).unwrap().len();
        assert!(size > 3 * CHUNK_SIZE as u64, "样本须跨多块，实际 {size}");

        let mut events = Vec::new();
        READ_BYTES.with(|c| c.set(0));
        let loaded =
            load_document_streaming(&target, |p| events.push(p)).expect("UTF-8 应加载成功");
        let reads = READ_BYTES.with(|c| c.take());

        assert_eq!(loaded.encoding, "UTF-8");
        assert_eq!(
            loaded.doc.to_text(),
            content,
            "单趟装载的正文必须与源文件逐字一致"
        );
        assert_eq!(reads, size, "读入字节数应恰等于文件大小（改前是 2×）");
        assert!(events.len() >= 3, "进度回调次数过少：{}", events.len());
        for pair in events.windows(2) {
            assert!(pair[0].bytes_read <= pair[1].bytes_read, "进度必须单调不减");
        }
        let last = *events.last().expect("至少一次回调");
        assert_eq!(last.bytes_read, last.total_bytes, "末值必须到 total");
        fs::remove_dir_all(&dir).ok();
    }

    /// O-9 的另一半：真不是 UTF-8 时**仍然**要回卷走 GBK 兜底，且回卷用同一
    /// fd 的 seek 而非重开文件——所以第二趟读到的字节必须与第一趟同量（说明
    /// 回到起点了），正文还须与 `decode` 逐字相等。
    /// P216 回归：**同一条二进制判据**必须给两条装载路径同一个结论。
    ///
    /// 截断的 GBK 文件（尾部悬着半个多字节字符；真实来源：下载被截断、tail
    /// 跟到半行、写盘被杀）的 U+FFFD **只**出自解码器的末次 flush。流式路径
    /// 此前把 stats 传成 `None`，占比统计系统性少算，于是：
    /// - `decode()`（`load_file`，FIF 用的就是它）→ 判二进制、拒开；
    /// - `load_document_streaming()`（编辑器打开文件用的就是它）→ 放行，
    ///   正文里带 U+FFFD；用户改一笔再保存，原字节就被改写成文本 —— 这正是
    ///   P1 防护要拦的那件事。
    #[test]
    fn streaming_gbk_binary_verdict_matches_strict_decode() {
        let dir = scratch_dir("p216-gbk-tail");
        fs::create_dir_all(&dir).unwrap();
        let mut tail_one = vec![0xB5, 0xC4]; // 合法 GBK「的」
        tail_one.push(0x81); // 悬着的尾字节 → 坏输出全在 flush 里
        let mut ratio_edge = Vec::new();
        for _ in 0..1040 {
            ratio_edge.extend_from_slice(&[0xB5, 0xC4]);
        }
        ratio_edge.extend_from_slice(&[0xFF; 10]); // 中段坏字节：10/1050 < 1%
        ratio_edge.push(0x81); // 再加一个悬尾字节 → 计入 flush 才越过 1%
        let cases = vec![("tail-one.bin", tail_one), ("ratio-edge.bin", ratio_edge)];

        for (name, bytes) in cases {
            let target = dir.join(name);
            fs::write(&target, &bytes).unwrap();
            let reference = decode(&bytes);
            let streamed = load_document_streaming(&target, |_| {});
            match (&streamed, reference.is_binary) {
                (Ok(loaded), true) => panic!(
                    "{name}: decode 判二进制拒开，流式却放行（encoding {}，text {:?}）",
                    loaded.encoding,
                    loaded.doc.to_text()
                ),
                (Err(CoreError::BinaryDetected { .. }), false) => panic!(
                    "{name}: decode 放行，流式却拒开（text {:?}）",
                    reference.text
                ),
                (Err(CoreError::BinaryDetected { .. }), true) | (Ok(_), false) => {}
                (Err(e), _) => panic!("{name}: 非预期错误 {e}"),
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn document_streaming_falls_back_to_gbk_by_rewinding_the_same_fd() {
        let dir = scratch_dir("o9-gbk");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("gbk.txt");
        // GBK 编码的中文在严格 UTF-8 下必然破格（首块即可判非 UTF-8）
        let (chunk, _, _) = encoding_rs::GBK.encode("编辑器的中文测试内容，重复若干遍。\r\n");
        let mut body: Vec<u8> = Vec::new();
        while (body.len() as u64) < 3 * CHUNK_SIZE as u64 {
            body.extend_from_slice(&chunk);
        }
        fs::write(&target, &body).unwrap();
        let size = fs::metadata(&target).unwrap().len();

        READ_BYTES.with(|c| c.set(0));
        let loaded = load_document_streaming(&target, |_| {}).expect("GBK 应加载成功");
        let reads = READ_BYTES.with(|c| c.take());

        assert_eq!(loaded.encoding, "GBK", "非 UTF-8 必须落到 GBK 兜底");
        assert_eq!(
            loaded.doc.to_text(),
            crate::loader::decode(&body).text,
            "回卷重读的兜底结果必须与 decode() 一致"
        );
        assert_eq!(
            reads,
            size * 2,
            "兜底路径应恰好读两趟（同 fd 回卷，非重开）"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn streaming_reports_monotonic_progress_and_decodes() {
        let dir = scratch_dir("stream");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("big.txt");
        // 3 个块以上的数据量，确保进度回调多次触发
        let content = "Editpad 流式加载测试行\n".repeat(20_000);
        fs::write(&target, &content).unwrap();

        let mut events = Vec::new();
        let loaded = load_file_streaming(&target, |p| events.push(p)).expect("加载应成功");

        assert_eq!(loaded.text, content);
        assert_eq!(loaded.encoding, "UTF-8");
        assert!(
            events.len() >= 3,
            "应触发多次进度回调，实际 {} 次",
            events.len()
        );
        for pair in events.windows(2) {
            assert!(pair[0].bytes_read <= pair[1].bytes_read, "进度必须单调递增");
        }
        let last = *events.last().expect("至少一次回调");
        assert_eq!(last.bytes_read, last.total_bytes);
        assert_eq!(last.bytes_read as usize, content.len());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plain_utf8() {
        let loaded = decode("hello 世界".as_bytes());
        assert_eq!(loaded.text, "hello 世界");
        assert_eq!(loaded.encoding, "UTF-8");
    }

    #[test]
    fn utf8_with_bom_strips_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("内容".as_bytes());
        let loaded = decode(&bytes);
        assert_eq!(loaded.text, "内容");
        assert_eq!(loaded.encoding, "UTF-8(BOM)");
    }

    #[test]
    fn gbk_fallback() {
        // “中文” 的 GBK 编码
        let loaded = decode(&[0xD6, 0xD0, 0xCE, 0xC4]);
        assert_eq!(loaded.text, "中文");
        assert_eq!(loaded.encoding, "GBK");
        assert!(!loaded.is_binary, "合法 GBK 文本不得误判为二进制");
    }

    /// P265：UTF-8 BOM 只说明"作者意图是 UTF-8"，**不保证正文真是 UTF-8**。
    ///
    /// 改前的 BOM 分支是 `String::from_utf8_lossy(&bytes[3..])` + 恒
    /// `is_binary: false`，于是主路径那两道防护（含 NUL 判二进制、U+FFFD 占比
    /// 判二进制）被一起绕过：一个被工具误加 BOM 的 GBK 文件会解成一屏
    /// U+FFFD 当作文本收下，用户一按保存就把这堆损毁字节当原文写回磁盘——
    /// 那是数据损毁，不是显示问题。现在 BOM 分支共用 [`decode_body`]。
    #[test]
    fn utf8_bom_body_shares_the_binary_guards() {
        // ① 正文确实是合法 UTF-8：标签与文本一律照旧（这一条防"修过头"）
        let mut ok = vec![0xEF, 0xBB, 0xBF];
        ok.extend_from_slice("内容\n".as_bytes());
        let loaded = decode(&ok);
        assert_eq!(loaded.text, "内容\n");
        assert_eq!(loaded.encoding, "UTF-8(BOM)");
        assert!(!loaded.is_binary);

        // ② GBK 正文被误加 BOM：以前是一屏乱码当文本收下，现在按主路径同款
        //    兜底解出正确汉字，且**标签跟着说实话**（不再声称是 UTF-8(BOM)——
        //    标签一路决定状态栏、转码知情与自动保存选码，不能替解不开的字节作证）
        let mut gbk_bom = vec![0xEF, 0xBB, 0xBF];
        gbk_bom.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]); // “中文” 的 GBK
        let loaded = decode(&gbk_bom);
        assert_eq!(loaded.text, "中文", "BOM 版 GBK 正文应被兜底解出");
        assert_eq!(loaded.encoding, "GBK", "标签必须说实话，不是 UTF-8(BOM)");
        assert!(!loaded.is_binary);

        // ③ BOM + 含 NUL 的"二进制"：改前 NUL 判据根本轮不到
        let mut nul_bom = vec![0xEF, 0xBB, 0xBF];
        nul_bom.extend_from_slice(&[0x4D, 0x5A, 0x00, 0x90]);
        assert!(
            decode(&nul_bom).is_binary,
            "贴了 BOM 的二进制照样得判为二进制（改前这条恒 false）"
        );

        // ④ BOM + GBK 也未分配的字节：兜底也解不出东西 ⇒ 占比判据必须生效
        //    （0x80 不行——GBK 把它映射成「€」，实测解出 64 个合法字符；
        //    0xFF 在 GBK 里未分配，才是真的解不出）
        let mut junk_bom = vec![0xEF, 0xBB, 0xBF];
        junk_bom.extend_from_slice(&[0xFFu8; 64]);
        assert!(
            decode(&junk_bom).is_binary,
            "全是 U+FFFD 的正文不能被当作文本收下，实际 {:?}",
            decode(&junk_bom)
        );
    }

    /// P265 的流式孪生：**大文件也不能绕过占比判据**。
    ///
    /// 与 `decode` 那侧同源的老问题。这里 `Plan::Direct(UTF_8, "UTF-8(BOM)", 3)`
    /// 改前给 `build_pass` 传 `None`（根本不统计）。
    /// ⚠️ 如实披露一处**残余不对称**：流式直达分支没有 GBK 兜底重读那一步
    /// （那要动 `ScanThenBuild` 的回卷与 `bom_len`，而那条链上有 TOCTOU 与前
    /// 进进度上报的历史），所以"误加 BOM 的 GBK 文件"在小文件上是**按 GBK
    /// 正确恢复**（见上一条用例 ②），在大文件上是**拒绝打开**。两边都不再
    /// 损毁数据，但行为不一致，已记台账 §2。
    #[test]
    fn streaming_utf8_bom_applies_the_replacement_guard() {
        let dir = scratch_dir("p265-bom-stream");
        fs::create_dir_all(&dir).unwrap();

        // 合法 BOM 文件照旧正常载入
        let good = dir.join("good.txt");
        let mut good_bytes = vec![0xEF, 0xBB, 0xBF];
        good_bytes.extend_from_slice("中文内容\n第二行\n".as_bytes());
        fs::write(&good, &good_bytes).unwrap();
        let loaded = load_document_streaming(&good, |_| {}).expect("合法 BOM 文件应能载入");
        assert_eq!(loaded.doc.to_text(), "中文内容\n第二行\n");
        assert_eq!(loaded.encoding, "UTF-8(BOM)");

        // 纯高位字节的"BOM 文件"必须被拒（改前：整屏 U+FFFD 静默收下）
        let junk = dir.join("junk.txt");
        let mut junk_bytes = vec![0xEF, 0xBB, 0xBF];
        junk_bytes.extend_from_slice(&[0x80u8; 4096]);
        fs::write(&junk, &junk_bytes).unwrap();
        match load_document_streaming(&junk, |_| {}) {
            Err(CoreError::BinaryDetected { .. }) => {}
            Ok(other) => panic!(
                "BOM + 全非法字节应判二进制，实际收下 {} 字符、标签 {:?}",
                other.doc.to_text().chars().count(),
                other.encoding
            ),
            Err(e) => panic!("期望 BinaryDetected，实际 {e:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P1 二进制防护 ----------

    #[test]
    fn nul_byte_is_detected_as_binary() {
        let loaded = decode(&[0x64, 0x65, 0x00, 0x6C, 0x6C]);
        assert!(loaded.is_binary, "含 NUL 字节必须判为二进制");

        // 落盘后经完整加载路径验证：应返回 BinaryDetected 而非解码文本
        let dir = scratch_dir("binary-nul");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("fake.dll");
        fs::write(&target, [0x4D, 0x5A, 0x00, 0x90]).unwrap();
        match load_file(&target) {
            Err(CoreError::BinaryDetected { .. }) => {}
            other => panic!("应拒绝打开二进制文件，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn high_replacement_ratio_without_nul_is_binary() {
        // 0x81 是合法 GBK 首字节，但 0xFF 不是合法次字节 → 每对都解出 U+FFFD，
        // 全文不含 NUL，模拟「压缩包/图片被当文本打开」的典型形态。
        let bytes = [0x81u8, 0xFF].repeat(512);
        let loaded = decode(&bytes);
        assert!(loaded.is_binary, "U+FFFD 占比 100% 应判为二进制");
    }

    #[test]
    fn borderline_replacement_ratio_stays_text() {
        // 正常 GBK 中文里只混入一个孤立坏字节（占比远低于阈值）仍应可打开：
        // 200 对「的」(0xB5 0xC4) + 尾部孤立首字节 0x81 → 仅 1 个 U+FFFD
        let mut bytes = Vec::new();
        for _ in 0..200 {
            bytes.extend_from_slice(&[0xB5, 0xC4]);
        }
        bytes.push(0x81);
        let loaded = decode(&bytes);
        assert!(!loaded.is_binary, "零星坏字节的 GBK 文本仍应可打开");
        assert_eq!(loaded.encoding, "GBK");
    }

    #[test]
    fn utf16_bom_and_plain_files_are_not_flagged_binary() {
        // ⚠️ 本用例原名 `utf8_bom_and_utf16_paths_never_flagged_binary`——**名字里
        // 那半 "utf8_bom" 它从来没测过**（通篇没有 `EF BB BF` 输入）。第 179 轮
        // 修 P265 时正是这个"看着切题"的名字让那个洞多活了一段：BOM 分支的
        // 防护被整体绕过，而这条用例照绿。UTF-8 BOM 现在由
        // [`utf8_bom_body_shares_the_binary_guards`] 与
        // [`streaming_utf8_bom_applies_the_replacement_guard`] 覆盖，本用例
        // 改名叫它真正断言的东西。
        // 带 BOM 的 UTF-16LE：ASCII 段每字符都带 0x00，但已由 BOM 分流，不得误判
        let loaded = decode(&[0xFF, 0xFE, 0x68, 0x00, 0x69, 0x00]);
        assert!(!loaded.is_binary);
        assert_eq!(loaded.text, "hi");

        // 空文件与纯 ASCII 文件照常打开
        assert!(!decode(b"").is_binary);
        assert!(!decode(b"plain text").is_binary);
    }

    #[test]
    fn streaming_loader_rejects_binary_too() {
        let dir = scratch_dir("binary-stream");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("blob.bin");
        fs::write(&target, [0x00, 0x01, 0x02, 0x03]).unwrap();

        let result = load_file_streaming(&target, |_| {});
        assert!(
            matches!(result, Err(CoreError::BinaryDetected { .. })),
            "流式加载路径同样要拦截二进制"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utf16le_with_bom() {
        // “中” U+4E2D，UTF-16LE 小端字节序为 2D 4E
        let loaded = decode(&[0xFF, 0xFE, 0x2D, 0x4E]);
        assert_eq!(loaded.text, "中");
        assert_eq!(loaded.encoding, "UTF-16LE");
    }

    // ---------- P19 行动项 2：流式加载直入 rope ----------
    //
    // 对拍策略：以既有 decode() 为参照实现，逐编码验证
    // load_document_streaming 的正文与编码标签完全一致。

    /// 把字节落盘并走新路径加载，返回 (正文, 编码标签)。
    fn load_doc_via_stream(dir: &Path, name: &str, bytes: &[u8]) -> (String, String) {
        fs::create_dir_all(dir).unwrap();
        let target = dir.join(name);
        fs::write(&target, bytes).unwrap();
        let loaded = load_document_streaming(&target, |_| {}).expect("加载应成功");
        (loaded.doc.to_text(), loaded.encoding.to_string())
    }

    /// 无 BOM 的 UTF-16LE 中文文本里，「非法 lead」可能先于 NUL 出现
    /// （`文` = `87 65` 的 0x87 就是孤立续字节）。旧实现在判非法的那一刻
    /// 直接 return，同一块剩下的字节没人扫 NUL 了 —— 与 [`decode`] 的
    /// 「整块 contains(0)」口径分叉，二进制/宽编码文件被当成 GBK 文本打开。
    #[test]
    fn streaming_scans_nul_after_an_invalid_sequence_in_the_same_block() {
        // UTF-16LE("文一") = 65 87? 逐字节写实：文=U+6587→[87,65]，一=U+4E00→[00,4E]
        let unit: Vec<u8> = vec![0x87, 0x65, 0x00, 0x4E];
        let mut bytes = Vec::new();
        for _ in 0..8 {
            bytes.extend_from_slice(&unit);
        }
        assert!(bytes.len() < CHUNK_SIZE, "用例前提：整个文件落在同一块内");
        assert!(bytes.contains(&0) && !bytes.starts_with(&[0xFF, 0xFE]));

        let dir = scratch_dir("nul-tail");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("u16le.txt");
        fs::write(&target, &bytes).unwrap();

        let verdict = load_document_streaming(&target, |_| {}).err();
        assert!(
            matches!(verdict, Some(CoreError::BinaryDetected { .. })),
            "含 NUL 必须按二进制拒绝（与 decode 同口径），实际 {verdict:?}"
        );
        assert!(decode(&bytes).is_binary, "对照实现本来就判二进制");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn streaming_document_matches_decode_oracle_across_encodings() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "plain.txt",
                "hello 世界 editpad\nsecond line\r\nthird"
                    .as_bytes()
                    .to_vec(),
            ),
            ("bom.txt", {
                let mut b = vec![0xEF, 0xBB, 0xBF];
                b.extend_from_slice("BOM 内容\n第二行".as_bytes());
                b
            }),
            ("utf16le.txt", {
                let mut b = vec![0xFF, 0xFE];
                for unit in "UTF-16 小端 中文🚀\n".encode_utf16() {
                    b.extend_from_slice(&unit.to_le_bytes());
                }
                b
            }),
            ("utf16be.txt", {
                let mut b = vec![0xFE, 0xFF];
                for unit in "UTF-16 大端 中文\n".encode_utf16() {
                    b.extend_from_slice(&unit.to_be_bytes());
                }
                b
            }),
            // 已知 GBK 字节对：中=D6D0 文=CEC4
            (
                "gbk.txt",
                vec![0xD6, 0xD0, 0xCE, 0xC4, b',', b' ', b'G', b'B', b'K'],
            ),
            ("empty.txt", Vec::new()),
        ];

        let dir = scratch_dir("p19-oracle");
        for (name, bytes) in cases {
            let got = load_doc_via_stream(&dir, name, &bytes);
            let want = decode(&bytes);
            assert_eq!(got.0, want.text, "{name}: 正文必须与 decode 参照实现一致");
            assert_eq!(got.1, want.encoding, "{name}: 编码标签必须一致");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multibyte_and_crlf_split_across_chunk_boundary() {
        // 构造恰好让多字节字符与 \r\n 骑在 64KB 边界上的内容：
        // 'a' ×(CHUNK-1) 后接 3 字节 '中'（跨边界）与 CRLF
        let mut content = Vec::new();
        content.extend(std::iter::repeat_n(b'a', CHUNK_SIZE - 1));
        content.extend_from_slice("中\r\n文🚀\r\n".as_bytes());
        content.extend(std::iter::repeat_n(b'b', CHUNK_SIZE));

        let dir = scratch_dir("p19-boundary");
        let (text, encoding) = load_doc_via_stream(&dir, "boundary.txt", &content);
        let reference = decode(&content);
        assert_eq!(encoding, "UTF-8");
        assert_eq!(text, reference.text, "跨块多字节/CRLF 必须与参照一致");
        assert_eq!(
            crate::document::Document::from_str(&text).line_ending(),
            LineEnding::CrLf,
            "主导行尾应随内容正确检出"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nul_deep_in_file_is_still_short_circuited() {
        // NUL 藏在 300KB 深处：全文检测语义不得因流式化而缩水
        let dir = scratch_dir("p19-deep-nul");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("deep.bin");
        let mut content = b"clean text\n".repeat(30_000); // ≈330KB
        content.extend_from_slice(b"\0tail");
        fs::write(&target, &content).unwrap();

        match load_document_streaming(&target, |_| {}) {
            Err(CoreError::BinaryDetected { .. }) => {}
            other => panic!("深处的 NUL 也必须拒绝打开，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gbk_ratio_semantics_survive_streaming() {
        let dir = scratch_dir("p19-gbk");
        // 合法 GBK 文本 → 正常打开且编码标签 GBK
        let good: Vec<u8> = [0xB5, 0xC4].repeat(500); // 「的」×500
        let (text, encoding) = load_doc_via_stream(&dir, "good.gbk", &good);
        assert_eq!(encoding, "GBK");
        assert!(text.chars().all(|c| c == '的'), "GBK 文本应完整解码");

        // 高替换率垃圾字节 → 二进制拒绝
        let garbage = [0x81u8, 0xFF].repeat(CHUNK_SIZE * 3); // 多块规模
        let target = dir.join("garbage.bin");
        fs::write(&target, &garbage).unwrap();
        match load_document_streaming(&target, |_| {}) {
            Err(CoreError::BinaryDetected { .. }) => {}
            other => panic!("高 FFFD 占比必须判二进制，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn eol_counter_cross_chunk_parity() {
        use crate::document::EolCounter;
        let samples = [
            "a\r\nb",
            "a\rb",
            "a\nb",
            "\r\n",
            "\r",
            "\n",
            "",
            "x",
            "a\r\n\r\rb\n\r",
            "\r\r\n\n",
        ];
        for sample in samples {
            for split in 0..=sample.len() {
                // 任意切分点（含切在 \r 与 \n 中间）都必须得到同一结论
                let mut counter = EolCounter::new();
                counter.push(&sample[..split]);
                counter.push(&sample[split..]);
                let streamed = counter.finish();
                assert_eq!(
                    streamed,
                    LineEnding::detect(sample),
                    "sample={sample:?} split={split}"
                );
            }
        }
    }

    #[test]
    fn streaming_utf8_scan_matches_strict_decode_on_edge_sequences() {
        // P147 回归：手写扫描器曾与 from_utf8 判定发散——EOF 截断序列
        // 不判无效、不查首续字节范围（超长/代理/>U+10FFFF）。GBK 双字节
        // 字符大量落在「E0-EF+续字节」形态上，以截断序列结尾的 GBK 文件
        // 曾被流式路径错标 UTF-8 解出 FFFD，保存后原字节被覆写。
        let cases: Vec<(&str, Vec<u8>)> = vec![
            // EOF 截断的多字节序列（GBK 常见形态）
            ("truncated.bin", b"ab\xE4\xB8".to_vec()),
            ("truncated2.bin", b"\xE5\xA5".to_vec()),
            // CESU 代理对（ED 的首续字节须 ..9F）
            ("surrogate.bin", b"ok\xED\xA0\x80".to_vec()),
            // 超长编码（E0 的首续字节须 A0..）
            ("overlong.bin", b"ok\xE0\x9F\xBF".to_vec()),
            // > U+10FFFF（F4 的首续字节须 ..8F）
            ("overflow.bin", b"ok\xF4\x90\x80\x80".to_vec()),
            // 对照：边界内的合法序列必须仍判 UTF-8（U+D7FF / U+10FFFF）
            ("edge-valid.bin", b"ok\xED\x9F\xBF\xF4\x8F\xBF\xBF".to_vec()),
            ("emoji.bin", "ok🚀".as_bytes().to_vec()),
        ];

        let dir = scratch_dir("p147-utf8-edge");
        for (name, bytes) in cases {
            fs::create_dir_all(&dir).unwrap();
            let target = dir.join(name);
            fs::write(&target, &bytes).unwrap();
            let reference = decode(&bytes);
            let streamed = load_document_streaming(&target, |_| {});
            if reference.encoding == "UTF-8" {
                let loaded = streamed.expect("参照判 UTF-8，流式不得拒绝");
                assert_eq!(loaded.encoding, "UTF-8", "{name}: 标签必须一致");
                assert_eq!(loaded.doc.to_text(), reference.text, "{name}: 正文必须一致");
            } else {
                match streamed {
                    Ok(loaded) => assert_ne!(
                        loaded.encoding, "UTF-8",
                        "{name}: 流式不得把非 UTF-8 错标成 UTF-8（输入 {bytes:02x?}）"
                    ),
                    Err(CoreError::BinaryDetected { .. }) => {} // 双方一致拒绝
                    Err(e) => panic!("{name}: 非预期错误 {e}"),
                }
            }
        }
        fs::remove_dir_all(&dir).ok();
    }

    /// O-10 护栏：`HeadSample` 的增量字符记账必须与「每推一个字符重数一遍
    /// 全串」逐段等价——上限按**字符**计（不是字节），到点即置 `full`，
    /// 之后任何输入都不再改变样本。
    #[test]
    fn head_sample_stops_exactly_at_char_limit() {
        // 8 字符 / 12 字节：按字节计数会得到 6144，当场露馅
        let chunk = "x中x中x中x中";
        assert_eq!(chunk.chars().count(), 8);
        let mut s = HeadSample::default();
        for _ in 0..(SAMPLE_HEAD_CHARS / 8) {
            s.push(chunk);
        }
        assert_eq!(
            s.buf.chars().count(),
            SAMPLE_HEAD_CHARS,
            "样本必须停在字符上限"
        );
        assert!(s.full, "达到上限必须置 full");
        let before = s.buf.clone();
        s.push(chunk);
        assert_eq!(s.buf, before, "full 之后不得再增长");
    }
}
