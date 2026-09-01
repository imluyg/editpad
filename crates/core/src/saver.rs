//! 原子保存：先写同目录临时文件，再用 rename 原子替换目标文件。
//!
//! 项目从一开始就贯彻的习惯——任何时刻断电或崩溃，目标文件要么是完整的旧内容、
//! 要么是完整的新内容，绝不会出现写了一半的损坏文件。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

pub fn save_atomic(path: &Path, contents: &str) -> Result<(), CoreError> {
    write_atomic(path, contents.as_bytes()).map_err(|source| CoreError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// 原子写核心：先写同目录临时文件再 rename 替换。
/// 返回原生 [`std::io::Error`] 供 crate 内复用（如设置层保持原有签名）。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_path = temp_sibling(path);

    // 临时文件与目标同目录 → 同一卷上，rename 才能原子完成
    if let Err(source) = write_all_and_sync(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path); // 失败时清理现场
        return Err(source);
    }

    if let Err(source) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(source);
    }

    Ok(())
}

/// 写入并强制落盘：sync 完成后再 rename，防掉电丢内容。
fn write_all_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
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
}
