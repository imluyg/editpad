//! 原子保存：先写同目录临时文件，再用 rename 原子替换目标文件。
//!
//! 项目从一开始就贯彻的习惯——任何时刻断电或崩溃，目标文件要么是完整的旧内容、
//! 要么是完整的新内容，绝不会出现写了一半的损坏文件。

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::error::CoreError;

/// 落盘缓冲大小（O-15）：与 loader 侧读块 `crate::loader::CHUNK_SIZE` 同口径
/// 的 64 KB。ropey 的存储块只有 KB 级，三条保存路径原本逐块 `write_all`
/// 直写 `File`——50 MB 文档即几十万次 `WriteFile` syscall；经此缓冲后
/// syscall 次数 ≈ 文档字节数 / 64 KB。
const SAVE_BUF_BYTES: usize = 64 * 1024;

// 测试钩子（O-15 flush 时序契约）：原子写各步骤的**执行顺序**。
//
// 「缓冲必须在 rename 之前 flush」是数据损毁级的时序命题，但在返回值与
// 落盘结果上都不可观测——BufWriter 的 Drop 会在函数返回后补一次写，目标
// 文件照样完整，只有「flush 之前进程没了」这种真实掉电场景才暴露；耗时
// 断言在本仓库不被接受（假红假绿）。故记步骤序列，与 app 层
// `EditorCore::line_text_calls` 同形状，生产构建整块不参与编译。
#[cfg(test)]
thread_local! {
    static WRITE_STEPS: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn write_steps() -> Vec<&'static str> {
    WRITE_STEPS.with(|s| s.borrow().clone())
}

#[cfg(test)]
fn clear_write_steps() {
    WRITE_STEPS.with(|s| s.borrow_mut().clear());
}

pub fn save_atomic(path: &Path, contents: &str) -> Result<(), CoreError> {
    write_atomic(path, contents.as_bytes()).map_err(|source| CoreError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// 保存目标编码（P67）：v1 三种，覆盖中文 Windows 的常见需求
/// （与老工具交换 GBK 文件、需要 BOM 的消费方）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveEncoding {
    /// UTF-8 无 BOM（默认；历史行为）。
    Utf8,
    /// UTF-8 带 BOM（EF BB BF 前缀）。
    Utf8Bom,
    /// GBK。无法映射的字符（如 emoji）按 WHATWG 规则以 `&#N;` 数值
    /// 实体写入，[`EncodeNotice::unmappable`] 置位供 UI 告知。
    Gbk,
    // ---------- P127：CJK 传统编码扩展（encoding_rs 全部内置） ----------
    /// Big5（繁体中文）。
    Big5,
    /// Shift_JIS（日文）。
    ShiftJis,
    /// EUC-JP（日文）。
    EucJp,
    /// EUC-KR（韩文）。
    EucKr,
}

impl SaveEncoding {
    /// 状态栏/提示用的规范标签（与 loader 嗅探标签同口径）。
    pub fn label(&self) -> &'static str {
        match self {
            SaveEncoding::Utf8 => "UTF-8",
            SaveEncoding::Utf8Bom => "UTF-8(BOM)",
            SaveEncoding::Gbk => "GBK",
            SaveEncoding::Big5 => "Big5",
            SaveEncoding::ShiftJis => "Shift_JIS",
            SaveEncoding::EucJp => "EUC-JP",
            SaveEncoding::EucKr => "EUC-KR",
        }
    }
}

/// 一次编码落盘的附带信息（P67）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EncodeNotice {
    /// 存在无法用目标编码表示的字符（已按 `&#N;` 数值实体写入）。
    pub unmappable: bool,
}

/// 按指定编码落盘文档（P67）：UTF-8 走原 [`save_document_atomic`] 快路径；
/// BOM 变体先写前缀再原样分块；GBK 用 encoding_rs 编码器流式转换——
/// rope 块不劈字符（ropey 保证），逐块喂给有状态编码器，全程不产生
/// 整文档编码副本。原子性（临时文件 + rename + sync）与 `write_atomic_with`
/// 完全一致。
pub fn save_document_encoded(
    path: &Path,
    doc: &Document,
    encoding: SaveEncoding,
) -> Result<EncodeNotice, CoreError> {
    match encoding {
        SaveEncoding::Utf8 => {
            save_document_atomic(path, doc).map(|_| EncodeNotice::default())
        }
        SaveEncoding::Utf8Bom => write_atomic_with(path, |file| {
            file.write_all(&[0xEF, 0xBB, 0xBF])?;
            for chunk in doc.chunks() {
                file.write_all(chunk.as_bytes())?;
            }
            Ok(())
        })
        .map(|_| EncodeNotice::default())
        .map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        }),
        SaveEncoding::Gbk => {
            save_with_legacy_encoder(path, doc, encoding_rs::GBK)
        }
        SaveEncoding::Big5 => {
            save_with_legacy_encoder(path, doc, encoding_rs::BIG5)
        }
        SaveEncoding::ShiftJis => {
            save_with_legacy_encoder(path, doc, encoding_rs::SHIFT_JIS)
        }
        SaveEncoding::EucJp => {
            save_with_legacy_encoder(path, doc, encoding_rs::EUC_JP)
        }
        SaveEncoding::EucKr => {
            save_with_legacy_encoder(path, doc, encoding_rs::EUC_KR)
        }
    }
}

/// P127：CJK 传统编码的通用落盘路径（GBK 原实现泛化）：有状态编码器
/// 逐 rope 块流式转换，无法映射字符按数值实体写入（encoding_rs
/// encode 语义），原子性与 [`write_atomic_with`] 一致。GBK/Big5/
/// Shift_JIS/EUC 系均为无跨块状态编码器，分块喂入安全。
fn save_with_legacy_encoder(
    path: &Path,
    doc: &Document,
    enc: &'static encoding_rs::Encoding,
) -> Result<EncodeNotice, CoreError> {
    let mut encoder = enc.new_encoder();
    let mut buf: Vec<u8> = Vec::new();
    let mut unmappable = false;
    let io = write_atomic_with(path, |file| {
        for chunk in doc.chunks() {
            feed_encoder(&mut encoder, chunk, false, &mut buf, file, &mut unmappable)?;
        }
        // 收尾：last=true 让编码器冲刷内部状态
        feed_encoder(&mut encoder, "", true, &mut buf, file, &mut unmappable)
    });
    io.map(|_| EncodeNotice { unmappable })
        .map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        })
}

/// 把 `src` 喂给编码器直到全部消费，产出的字节即时经 `file` 落盘。
///
/// ⚠️ `encode_from_utf8_to_vec` 只写 `dst` 的**现有富余容量**（它把 `len`
/// 临时抬到 `capacity`、按实际写入长度还原，全程不 realloc），容量不足时
/// 返回 `OutputFull` 且**只消费前 `read` 个输入字节**。因此必须按 `read`
/// 前进游标、续喂剩余输入——丢弃返回值会让块尾静默蒸发。预留系数也不能
/// 按「GBK ≤2 字节/字符」估：不可映射字符走 HTML 十进制实体，2 字节字符
/// （希伯来/阿拉伯/亚美尼亚等）的 `&#NNNN;` 是 7 字节 = **3.5×输入字节**，
/// ×3 的预留在输入 >32 字节时必然溢出。×4 + 16 可让绝大多数块一次通过。
fn feed_encoder(
    encoder: &mut encoding_rs::Encoder,
    src: &str,
    last: bool,
    buf: &mut Vec<u8>,
    file: &mut std::io::BufWriter<fs::File>,
    unmappable: &mut bool,
) -> std::io::Result<()> {
    let mut rest = src;
    loop {
        buf.reserve(rest.len() * 4 + 16);
        let (result, read, had_errors) = encoder.encode_from_utf8_to_vec(rest, buf, last);
        *unmappable |= had_errors;
        file.write_all(buf)?;
        buf.clear();
        // `read` 恒落在字符边界上（编码器不会劈开多字节序列）
        rest = &rest[read..];
        if matches!(result, encoding_rs::CoderResult::InputEmpty) {
            return Ok(());
        }
        if read == 0 {
            // 一个输入单元都写不下（预留已覆盖 10 字节实体上界，正常到不了）：
            // 强制扩容续喂，避免原地打转
            buf.reserve(buf.capacity() + 64);
        }
    }
}

/// 文档原子保存（P19 行动项 3）：按 rope 存储块逐块写临时文件，
/// 全程不产生全文 String——50MB 文档的保存峰值从「rope + 全文拷贝」
/// 降回「只有 rope」。字节产物与 [`save_atomic`](`save_atomic`)
/// 写 `doc.to_text()` 完全一致。
pub fn save_document_atomic(path: &Path, doc: &Document) -> Result<(), CoreError> {
    write_atomic_with(path, |file| {
        for chunk in doc.chunks() {
            file.write_all(chunk.as_bytes())?;
        }
        Ok(())
    })
    .map_err(|source| CoreError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// 原子写核心：先写同目录临时文件再 rename 替换。
/// 返回原生 [`std::io::Error`] 供 crate 内复用（如设置层保持原有签名）。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

/// 原子写的内容生产端抽象：调用方拿到已创建的**带缓冲**临时文件写入器，
/// 自行决定一次性写还是分块写；flush、sync 与 rename 由本函数统一收口。
///
/// ⚠️ 闭包**不需要**自己 flush（也 flush 不掉什么）：本函数在闭包返回后
/// 显式 `flush()` 并把其结果向上报错，随后才 `sync_all()` + rename。
/// 顺序若写成「先 rename 后 flush」或忽略 flush 的错误，掉电/崩溃后目标
/// 文件会是空文件或半截文件——数据损毁级回归，比性能问题严重得多。
fn write_atomic_with<F>(path: &Path, produce: F) -> std::io::Result<()>
where
    F: FnOnce(&mut std::io::BufWriter<fs::File>) -> std::io::Result<()>,
{
    let tmp_path = temp_sibling(path);

    // 临时文件与目标同目录 → 同一卷上，rename 才能原子完成
    let result = (|| -> std::io::Result<()> {
        let file = fs::File::create(&tmp_path)?;
        let mut writer = io::BufWriter::with_capacity(SAVE_BUF_BYTES, file);
        produce(&mut writer)?;
        #[cfg(test)]
        WRITE_STEPS.with(|s| s.borrow_mut().push("produce"));
        // 缓冲里剩的字节必须先落到文件，再 sync、再 rename
        writer.flush()?;
        #[cfg(test)]
        WRITE_STEPS.with(|s| s.borrow_mut().push("flush"));
        // flush 之后缓冲已空，经 get_mut 拿回句柄做 sync_all
        // （不用 into_inner：其失败会把缓冲连同句柄一起吞掉，这里无状态可丢）
        let file = writer.get_mut();
        // sync 完成后再 rename，防掉电丢内容
        file.sync_all()?;
        #[cfg(test)]
        WRITE_STEPS.with(|s| s.borrow_mut().push("sync"));
        Ok(())
    })();
    if let Err(source) = result {
        let _ = fs::remove_file(&tmp_path); // 失败时清理现场
        return Err(source);
    }

    if let Err(source) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(source);
    }
    #[cfg(test)]
    WRITE_STEPS.with(|s| s.borrow_mut().push("rename"));

    Ok(())
}

/// 目标同目录下的兄弟临时文件名：
/// `note.txt` → `note.txt.<pid>.<seq>.editpad-tmp`
///
/// P146：曾恒为 `note.txt.editpad-tmp`——手动保存与自动保存（或连按
/// Ctrl+S）并发时，两个写者对同一临时文件各自 create（截断）+分块写
/// +rename，大文件写入窗口内交错会把目标文件写坏。pid + 进程内单调
/// 序号让每个写者独占自己的临时文件：rename 原子性保证目标最终是
/// 「某一次完整写入」，绝不交错损坏。
fn temp_sibling(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.{}.editpad-tmp", std::process::id(), seq));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        // 项目内落盘：系统 TEMP 在部分沙箱下不可写
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("saver-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_roundtrip_and_overwrite_leaves_no_tmp() {
        let dir = scratch_dir("atomic");
        let target = dir.join("note.txt");

        save_atomic(&target, "第一版").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "第一版");

        save_atomic(&target, "第二版").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "第二版");

        assert!(!temp_sibling(&target).exists(), "临时文件必须被 rename 消费掉");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unicode_content_survives() {
        let dir = scratch_dir("unicode");
        let target = dir.join("中文.md");
        let content = "第一行\n中文内容 emoji 🚀\nlast\n";

        save_atomic(&target, content).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), content);

        fs::remove_dir_all(&dir).ok();
    }

    /// O-15 时序契约：写内容 → **flush** → sync → rename，四步顺序钉死。
    ///
    /// 为什么只能用步骤序列：若哪天把顺序改成「先 rename 后 flush」或吞掉
    /// flush 的错误，**返回值与落盘结果都看不出问题**——`BufWriter` 的 Drop 会
    /// 在函数返回后补写，只有进程中途没了/掉电才暴露为空文件或半截文件。这类
    /// 时序命题无法用「结果」观测，而耗时断言在本仓不接受（忙机器假红、温热
    /// 假绿）→ 记步骤序列，形状同 app 层 `EditorCore::line_text_calls`，
    /// 生产构建整块不参与编译。
    #[test]
    fn atomic_write_flushes_before_sync_and_rename() {
        let dir = scratch_dir("flush-order");
        let target = dir.join("order.txt");

        clear_write_steps();
        save_atomic(&target, "第一版内容").unwrap();
        assert_eq!(
            write_steps(),
            vec!["produce", "flush", "sync", "rename"],
            "缓冲必须先落到文件，再 sync、再 rename"
        );

        // 跨缓冲阈值（64 KB）的整篇内容必须逐字节完整：BufWriter 用错会截尾
        let big = "中文 emoji 🚀 x\n".repeat(8192);
        assert!(big.len() > SAVE_BUF_BYTES, "样本须超出缓冲才有判别力");
        let big_target = dir.join("big.txt");
        clear_write_steps();
        save_atomic(&big_target, &big).unwrap();
        assert_eq!(fs::read_to_string(&big_target).unwrap(), big, "超阈值内容不得截断");
        assert_eq!(
            write_steps(),
            vec!["produce", "flush", "sync", "rename"],
            "超缓冲阈值的路径同样由本函数单点收口"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P19 行动项 3：文档分块原子保存 ----------

    #[test]
    fn document_save_is_byte_identical_to_string_save() {
        let dir = scratch_dir("doc-bytes");
        let doc = Document::from_str("第一行\r\n中文 emoji 🚀\r\n孤立行\n尾行");

        let doc_target = dir.join("via-doc.txt");
        save_document_atomic(&doc_target, &doc).unwrap();

        let str_target = dir.join("via-str.txt");
        save_atomic(&str_target, &doc.to_text()).unwrap();

        assert_eq!(
            fs::read(&doc_target).unwrap(),
            fs::read(&str_target).unwrap(),
            "分块写与全文 String 写必须逐字节一致"
        );
        assert_eq!(fs::read_to_string(&doc_target).unwrap(), doc.to_text());
        // CRLF 文档保存往返：字节原样保留
        assert!(fs::read(&doc_target).unwrap().windows(2).any(|w| w == b"\r\n"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn chunked_save_overwrites_and_leaves_no_tmp() {
        let dir = scratch_dir("doc-overwrite");
        let target = dir.join("note.md");

        let first = Document::from_str(&"v1 行\n".repeat(5_000));
        save_document_atomic(&target, &first).unwrap();

        let second = Document::from_str("完全不同的第二版内容");
        save_document_atomic(&target, &second).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), second.to_text());
        // P146：临时名带 pid+序号不再可预测——按后缀模式断言目录里
        // 没有任何临时残留
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".editpad-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "rename 后不得有临时残留: {leftovers:?}"
        );
        // 多块规模的内容（>64KB）也走同一收口
        assert!(second.to_text().len() < first.to_text().len(), "前置条件：首轮为多块规模");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_document_save_keeps_old_content_intact() {
        let dir = scratch_dir("doc-atomic-fail");
        let old = Document::from_str("old content\n");
        let target = dir.join("config.toml");
        save_document_atomic(&target, &old).unwrap();

        // 同名目录占住目标路径 → 写临时文件必败
        let blocker = dir.join("occupied.toml.editpad-tmp");
        std::fs::create_dir_all(&blocker).unwrap();

        let doomed = Document::from_str("new content that must not land\n");
        assert!(save_document_atomic(&blocker, &doomed).is_err());

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "old content\n",
            "失败的保存不得破坏既有文件"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P67 按编码落盘 ----------

    #[test]
    fn gbk_save_roundtrips_via_encoding_rs() {
        let dir = scratch_dir("gbk-roundtrip");
        let target = dir.join("gbk.txt");
        let text = "第一行中文\nASCII line 2\n尾行";

        let notice =
            save_document_encoded(&target, &Document::from_str(text), SaveEncoding::Gbk)
                .expect("GBK 保存应成功");
        assert!(!notice.unmappable, "纯中英文本无不可映射字符");

        // 磁盘字节必须真的是 GBK：用 encoding_rs 解码回原文
        let bytes = fs::read(&target).unwrap();
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&bytes);
        assert!(!had_errors);
        assert_eq!(decoded, text, "GBK 往返必须逐字还原");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utf8_bom_save_prefixes_bom_and_keeps_bytes() {
        let dir = scratch_dir("bom-save");
        let target = dir.join("bom.txt");
        let doc = Document::from_str("BOM 内容\n第二行");

        save_document_encoded(&target, &doc, SaveEncoding::Utf8Bom)
            .expect("BOM 保存应成功");

        let bytes = fs::read(&target).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "必须以 UTF-8 BOM 开头");
        assert_eq!(&bytes[3..], doc.to_text().as_bytes(), "BOM 后为原样 UTF-8");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gbk_unmappable_char_becomes_numeric_reference() {
        let dir = scratch_dir("gbk-unmappable");
        let target = dir.join("emoji.txt");
        let doc = Document::from_str("中文 ok 🚀 tail");

        let notice =
            save_document_encoded(&target, &doc, SaveEncoding::Gbk)
                .expect("保存应成功（不可映射不失败）");
        assert!(notice.unmappable, "emoji 无法映射 GBK，必须置位告警");

        let bytes = fs::read(&target).unwrap();
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&bytes);
        assert!(!had_errors, "数值实体本身是合法 GBK");
        assert!(decoded.contains("&#"), "emoji 应以 &#N; 实体形式存在：{decoded}");
        assert!(decoded.contains("中文 ok") && decoded.contains("tail"));

        fs::remove_dir_all(&dir).ok();
    }

    /// 不可映射字符的数值实体上界是 **3.5 倍输入字节**（2 字节 UTF-8 字符
    /// 的码位落在 1000..2047 时实体为 `&#NNNN;` 7 字节），而不是注释里写的 3 倍。
    /// 原实现把 `encode_from_utf8_to_vec` 的 `OutputFull` 与已读长度一并丢弃，
    /// 溢出部分静默蒸发。取样用希伯来文：Cyrillic/Greek 在 GB2312 里有映射位，
    /// 只有希伯来/阿拉伯/亚美尼亚这一类 2 字节字符才既不可映射、又是 4 位数字。
    #[test]
    fn gbk_long_unmappable_run_is_not_truncated() {
        let dir = scratch_dir("gbk-long-run");
        let target = dir.join("hebrew.txt");
        let text = "א".to_string().repeat(200); // U+05D0，2 字节 UTF-8

        let notice = save_document_encoded(&target, &Document::from_str(&text), SaveEncoding::Gbk)
            .expect("保存应成功");
        assert!(notice.unmappable, "希伯来字母不在 GB2312 映射表内");

        let expected = "&#1488;".repeat(200);
        let bytes = fs::read(&target).unwrap();
        assert_eq!(
            bytes,
            expected.as_bytes(),
            "200 个希伯来字符必须逐个落成实体，尾部不得丢失"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// 溢出发生在多块文档的中途：可映射的 CJK 与不可映射的亚美尼亚交错，
    /// 逐块写入时每一块的尾巴都得续上（编码器有状态，`last=false` 贯穿全程）。
    #[test]
    fn gbk_mixed_multichunk_document_is_complete() {
        let dir = scratch_dir("gbk-multichunk");
        let target = dir.join("mixed.txt");
        // 9KB CJK（每块都放得下）+ 亚美尼亚段（每块都需要 3.5× 预算）
        let mut text = "中".to_string().repeat(3000);
        text.push_str(&"Ա".to_string().repeat(2000)); // U+0531
        let doc = Document::from_str(&text);
        assert!(
            doc.chunks().count() > 1,
            "用例前提：文档必须跨多个 rope 块"
        );

        save_document_encoded(&target, &doc, SaveEncoding::Gbk).expect("保存应成功");

        let bytes = fs::read(&target).unwrap();
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&bytes);
        assert!(!had_errors, "实体与 GBK 字节都是合法 GBK");
        assert_eq!(decoded.matches("&#1329;").count(), 2000, "亚美尼亚段必须完整");
        assert_eq!(
            decoded.chars().filter(|&c| c == '中').count(),
            3000,
            "CJK 段必须完整"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_encoding_labels_match_loader_vocabulary() {
        // 标签与 loader 嗅探口径一致：保存后回填状态栏不产生「新词」
        assert_eq!(SaveEncoding::Utf8.label(), "UTF-8");
        assert_eq!(SaveEncoding::Utf8Bom.label(), "UTF-8(BOM)");
        assert_eq!(SaveEncoding::Gbk.label(), "GBK");
    }

    #[test]
    fn failed_encoded_save_keeps_old_content_intact() {
        // 编码变体同样走原子写：目标被目录占用必败，旧文件原封不动
        let dir = scratch_dir("encoded-atomic");
        let old = Document::from_str("old\n");
        let target = dir.join("note.txt");
        save_document_encoded(&target, &old, SaveEncoding::Utf8Bom).unwrap();

        let blocker = dir.join("occupied.txt.editpad-tmp");
        std::fs::create_dir_all(&blocker).unwrap();

        let doomed = Document::from_str("new\n");
        assert!(
            save_document_encoded(&blocker, &doomed, SaveEncoding::Gbk).is_err()
        );
        // 目标是 BOM 变体：字节 = BOM 前缀 + 原内容
        assert_eq!(
            fs::read(&target).unwrap(),
            [0xEF, 0xBB, 0xBF].iter().copied().chain(b"old\n".iter().copied()).collect::<Vec<u8>>(),
            "失败的保存不得破坏既有文件"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P127：CJK 传统编码扩展 ----------

    #[test]
    fn legacy_cjk_encodings_round_trip_and_flag_unmappable() {
        // 各编码一个代表性字符 + ASCII：编码后用 encoding_rs 解码应还原
        let cases = [
            (SaveEncoding::Big5, encoding_rs::BIG5, "繁體測試 ascii"),
            (SaveEncoding::ShiftJis, encoding_rs::SHIFT_JIS, "日本語テスト"),
            (SaveEncoding::EucJp, encoding_rs::EUC_JP, "日本語EUC"),
            (SaveEncoding::EucKr, encoding_rs::EUC_KR, "한국어"),
            (SaveEncoding::Gbk, encoding_rs::GBK, "简体中文"),
        ];
        for (enc, rs, text) in cases {
            let dir = std::env::temp_dir().join("editpad-p127-test");
            let _ = std::fs::create_dir_all(&dir);
            let target = dir.join(format!("{}.bin", enc.label()));
            let notice = save_document_encoded(&target, &Document::from_str(text), enc)
                .expect("编码落盘应成功");
            assert!(!notice.unmappable, "{:?} 全部字符可映射", enc);
            let bytes = std::fs::read(&target).expect("读回");
            let (decoded, _, had_errors) = rs.decode(&bytes);
            assert_eq!(decoded, text, "{:?} 往返一致", enc);
            assert!(!had_errors);
            let _ = std::fs::remove_file(&target);
        }
        // emoji 对任何传统编码都不可映射 → 数值实体 + unmappable 置位
        let dir = std::env::temp_dir().join("editpad-p127-test");
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("big5-emoji.bin");
        let notice =
            save_document_encoded(&target, &Document::from_str("ok🚀"), SaveEncoding::Big5)
                .expect("落盘成功");
        assert!(notice.unmappable);
        let bytes = std::fs::read(&target).expect("读回");
        let (decoded, _, _) = encoding_rs::BIG5.decode(&bytes);
        assert!(decoded.contains("&#"), "emoji 应以数值实体写入");
        assert!(decoded.starts_with("ok"));
    }
}
