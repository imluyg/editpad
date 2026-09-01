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
}
