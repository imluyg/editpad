//! 原子保存：先写同目录临时文件，再用 rename 原子替换目标文件。
//!
//! 项目从一开始就贯彻的习惯——任何时刻断电或崩溃，目标文件要么是完整的旧内容、
//! 要么是完整的新内容，绝不会出现写了一半的损坏文件。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::error::CoreError;

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
}

impl SaveEncoding {
    /// 状态栏/提示用的规范标签（与 loader 嗅探标签同口径）。
    pub fn label(&self) -> &'static str {
        match self {
            SaveEncoding::Utf8 => "UTF-8",
            SaveEncoding::Utf8Bom => "UTF-8(BOM)",
            SaveEncoding::Gbk => "GBK",
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
/// 整文档编码副本。原子性（临时文件 + rename + sync）与 [`write_atomic_with`]
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
            let mut encoder = encoding_rs::GBK.new_encoder();
            let mut buf: Vec<u8> = Vec::new();
            let mut unmappable = false;
            write_atomic_with(path, |file| {
                for chunk in doc.chunks() {
                    // ⚠️ encode_from_utf8_to_vec 把结果写进 dst 的**现有富余
                    // 容量**（不自动扩容）——零容量 Vec 什么都编不出来。
                    // 预留充足空间：GBK ≤2 字节/字符，数值实体最长 ~10
                    // 字节/字符，按输入字节数 ×3 预留绰绰有余。
                    buf.reserve(chunk.len() * 3 + 16);
                    let (_, _, had_errors) =
                        encoder.encode_from_utf8_to_vec(chunk, &mut buf, false);
                    unmappable |= had_errors;
                    file.write_all(&buf)?;
                    buf.clear();
                }
                // 收尾：last=true 让编码器冲刷内部状态（GBK 无状态，但
                // 统一口径；空串调用零输出）
                buf.reserve(16);
                let (_, _, had_errors) =
                    encoder.encode_from_utf8_to_vec("", &mut buf, true);
                unmappable |= had_errors;
                file.write_all(&buf)?;
                Ok(())
            })
            .map(|_| EncodeNotice { unmappable })
            .map_err(|source| CoreError::Write {
                path: path.to_path_buf(),
                source,
            })
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

/// 原子写的内容生产端抽象：调用方拿到已创建的临时文件句柄，
/// 自行决定一次性写还是分块写；sync 与 rename 由本函数统一收口。
fn write_atomic_with<F>(path: &Path, produce: F) -> std::io::Result<()>
where
    F: FnOnce(&mut fs::File) -> std::io::Result<()>,
{
    let tmp_path = temp_sibling(path);

    // 临时文件与目标同目录 → 同一卷上，rename 才能原子完成
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp_path)?;
        produce(&mut file)?;
        // sync 完成后再 rename，防掉电丢内容
        file.sync_all()?;
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

    Ok(())
}

/// 目标同目录下的兄弟临时文件名：`note.txt` → `note.txt.editpad-tmp`
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".editpad-tmp");
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
        assert!(!temp_sibling(&target).exists(), "rename 后不得有临时残留");
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
}
