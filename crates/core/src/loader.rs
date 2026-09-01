//! 文件加载与编码识别。
//!
//! M0 为一次性读取（中小文件足够）；M1 会升级为后台线程流式加载 + 进度回报，
//! 届时 UI 线程任何时刻的阻塞都不超过一帧。

use std::fs;
use std::io::{BufReader, Read};
use std::path::Path;
use std::str;

use encoding_rs::{Encoding, GBK, UTF_16BE, UTF_16LE};

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

    Ok(reject_binary(path, decode(&buffer))?)
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
}
