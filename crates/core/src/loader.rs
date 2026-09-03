//! 文件加载与编码识别。
//!
//! M0 为一次性读取（中小文件足够）；M1 会升级为后台线程流式加载 + 进度回报，
//! 届时 UI 线程任何时刻的阻塞都不超过一帧。

use std::fs;
use std::io::{BufReader, Read};
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
    let total_bytes = file.metadata().map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?.len();

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
    let total_bytes = file.metadata().map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?.len();

    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file);
    let mut chunk = vec![0u8; CHUNK_SIZE];

    // 读满第一块（read 允许少读），嗅探 BOM 决定解码策略
    let mut first_len = 0usize;
    while first_len < CHUNK_SIZE {
        let n = reader.read(&mut chunk[first_len..]).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        first_len += n;
    }
    report(&mut on_progress, first_len as u64, total_bytes);

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
            let mut head = HeadSample::default();
            let doc = build_pass(
                &mut reader, path, &chunk[..first_len], bom_len, encoding,
                total_bytes, 0, total_bytes, &mut on_progress, None, &mut head,
            )?;
            Ok(LoadedDocument {
                doc,
                encoding: label,
                sample: head.buf,
            })
        }
        Plan::ScanThenBuild => {
            // ---- 第一遍：全文扫描（NUL 短路 + 严格 UTF-8 增量校验）----
            let mut scan = Utf8Scan::default();
            let mut scanned = first_len as u64;
            scan.feed(&chunk[..first_len]);
            loop {
                let n = reader.read(&mut chunk).map_err(io_err(path))?;
                if n == 0 {
                    break;
                }
                scanned += n as u64;
                scan.feed(&chunk[..n]);
                // 校验遍进度映射到前半程（装载遍走后半程，整体单调）
                report(&mut on_progress, scanned.min(total_bytes) / 2, total_bytes);
            }
            if scan.saw_nul {
                return Err(CoreError::BinaryDetected {
                    path: path.to_path_buf(),
                });
            }
            let utf8_valid = !scan.invalid;

            // ---- 第二遍：按结论装载（进度走后半程）----
            // 重新打开文件回到起点（第一遍没有保留字节——这正是省内存的关键）
            drop(reader);
            let file = fs::File::open(path).map_err(|source| CoreError::Read {
                path: path.to_path_buf(),
                source,
            })?;
            let mut reader = BufReader::with_capacity(CHUNK_SIZE, file);
            let first_len = fill_chunk(&mut reader, &mut chunk).map_err(io_err(path))?;
            let base = total_bytes / 2;

            let (encoding, label, check_ratio) = if utf8_valid {
                (UTF_8, "UTF-8", false)
            } else {
                (GBK, "GBK", true)
            };
            let mut stats = BuildStats::default();
            let mut head = HeadSample::default();
            let doc = build_pass(
                &mut reader, path, &chunk[..first_len], 0, encoding,
                total_bytes, base, total_bytes - base, &mut on_progress,
                Some(&mut stats), &mut head,
            )?;
            if check_ratio
                && stats.replacements as f32 / stats.chars.max(1) as f32
                    > BINARY_REPLACEMENT_RATIO
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
    }
}

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
    Ok(filled)
}

/// 第一遍扫描的状态：NUL 是最强二进制信号（短路）；
/// 严格 UTF-8 校验用手写状态机（跨块续字节计数）。
#[derive(Default)]
struct Utf8Scan {
    saw_nul: bool,
    invalid: bool,
    /// 尚待接收的续字节数（0x80..=BF）
    expect: u8,
}

impl Utf8Scan {
    fn feed(&mut self, data: &[u8]) {
        if self.invalid {
            // 已判非 UTF-8：剩余唯一使命是把 NUL 扫完（语义对齐 decode()）
            self.saw_nul |= data.contains(&0);
            return;
        }
        for &b in data {
            if b == 0 {
                self.saw_nul = true;
                self.invalid = true; // NUL 之后无需再校验 UTF-8
                return;
            }
            if self.expect > 0 {
                if b & 0xC0 == 0x80 {
                    self.expect -= 1;
                } else {
                    self.expect = 0;
                    self.invalid = true;
                    return;
                }
            } else if b >= 0x80 {
                self.expect = match b {
                    0xC2..=0xDF => 1,
                    0xE0..=0xEF => 2,
                    0xF0..=0xF4 => 3,
                    _ => {
                        self.invalid = true;
                        return;
                    }
                };
            }
        }
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
    full: bool,
}

impl HeadSample {
    fn push(&mut self, text: &str) {
        if self.full {
            return;
        }
        for c in text.chars() {
            self.buf.push(c);
            if self.buf.chars().count() >= SAMPLE_HEAD_CHARS {
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
        absorb(&mut decoder, &first_chunk[skip..], false, &mut out, &mut builder, &mut eol, stats.as_deref_mut(), head);
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
        absorb(&mut decoder, &chunk[..n], false, &mut out, &mut builder, &mut eol, stats.as_deref_mut(), head);
        report(on_progress, progress_base + done.min(progress_span), total_bytes);
    }
    // 冲刷解码器尾部（未完的多字节序列 / 未配对代理）
    absorb(&mut decoder, b"", true, &mut out, &mut builder, &mut eol, None, head);
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
/// 1. UTF-8 BOM → 去掉 BOM 按 UTF-8；
/// 2. UTF-16 LE / BE BOM → 对应解码；
/// 3. 无 BOM：含 NUL 字节 → 判二进制（UTF-16 文件已在上面由 BOM 分流）；
/// 4. 严格 UTF-8 校验，通过即按 UTF-8；
/// 5. 校验失败 → GBK 兜底；U+FFFD 占比超阈值同样判二进制。
pub fn decode(bytes: &[u8]) -> LoadedText {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return LoadedText {
            text: String::from_utf8_lossy(&bytes[3..]).into_owned(),
            encoding: "UTF-8(BOM)",
            is_binary: false,
        };
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

    #[test]
    fn streaming_reports_monotonic_progress_and_decodes() {
        let dir = scratch_dir("stream");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("big.txt");
        // 3 个块以上的数据量，确保进度回调多次触发
        let content = "Editpad 流式加载测试行\n".repeat(20_000);
        fs::write(&target, &content).unwrap();

        let mut events = Vec::new();
        let loaded =
            load_file_streaming(&target, |p| events.push(p)).expect("加载应成功");

        assert_eq!(loaded.text, content);
        assert_eq!(loaded.encoding, "UTF-8");
        assert!(events.len() >= 3, "应触发多次进度回调，实际 {} 次", events.len());
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
    fn utf8_bom_and_utf16_paths_never_flagged_binary() {
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

    #[test]
    fn streaming_document_matches_decode_oracle_across_encodings() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("plain.txt", "hello 世界 editpad\nsecond line\r\nthird".as_bytes().to_vec()),
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
            ("gbk.txt", vec![0xD6, 0xD0, 0xCE, 0xC4, b',', b' ', b'G', b'B', b'K']),
            ("empty.txt", Vec::new()),
        ];

        let dir = scratch_dir("p19-oracle");
        for (name, bytes) in cases {
            let got = load_doc_via_stream(&dir, name, &bytes);
            let want = decode(&bytes);
            assert_eq!(
                got.0, want.text,
                "{name}: 正文必须与 decode 参照实现一致"
            );
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
        let samples = ["a\r\nb", "a\rb", "a\nb", "\r\n", "\r", "\n", "", "x",
                       "a\r\n\r\rb\n\r", "\r\r\n\n"];
        for sample in samples {
            for split in 0..=sample.len() {
                // 任意切分点（含切在 \r 与 \n 中间）都必须得到同一结论
                let mut counter = EolCounter::new();
                counter.push(&sample[..split]);
                counter.push(&sample[split..]);
                let streamed = counter.finish();
                assert_eq!(streamed, LineEnding::detect(sample),
                    "sample={sample:?} split={split}");
            }
        }
    }
}
