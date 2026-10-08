//! 统一的错误类型。

use std::path::PathBuf;

use crate::{Key, Lang};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("无法读取文件 {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("无法写入文件 {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// P1 二进制防护：文件被识别为二进制内容，拒绝打开。
    /// 打开再保存会把原文件替换成乱码，宁可拒开也不损毁数据。
    #[error("{path} 疑似二进制文件（含 NUL 字节或替换符占比过高），已拒绝打开以保护原文件")]
    BinaryDetected { path: PathBuf },
}

impl CoreError {
    /// 面向**界面**的文案：按界面语言取 [`Key::ErrRead`] / [`Key::ErrWrite`] /
    /// [`Key::ErrBinaryDetected`]。
    ///
    /// ⚠️ `Display`（thiserror 生成）恒为中文，只该用于日志与 `?` 传播——它曾经直接
    /// 进状态栏，于是英文界面显示成「英文前缀 ＋ 中文原因」（P377＝池 ⑩ 的加载链那半）。
    /// 后台线程拿不到设置，所以调用方在**发起那一刻**把语言随任务一起带过去。
    pub fn message(&self, lang: Lang) -> String {
        match self {
            CoreError::Read { path, source } => {
                format!("{} {}: {source}", Key::ErrRead.text(lang), path.display())
            }
            CoreError::Write { path, source } => {
                format!("{} {}: {source}", Key::ErrWrite.text(lang), path.display())
            }
            CoreError::BinaryDetected { path } => {
                // 两种语言的键文本都是"谓语段"，拼在路径之后都读得通
                format!("{}: {}", path.display(), Key::ErrBinaryDetected.text(lang))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn io_err() -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, "Access is denied")
    }

    /// P377（池 ⑩）：三种错误的界面文案都跟着语言走，且路径与 OS 细节都不许丢。
    ///
    /// 判据取"英文那侧一个汉字都不许出现"——改前这三条走的是 `Display`（恒中文
    /// 模板），英文界面就变成「Failed to save: 无法写入文件 …」这种半中半英。
    #[test]
    fn core_error_message_follows_language() {
        let cases: [(&str, CoreError, &str, &str); 3] = [
            (
                "Read",
                CoreError::Read {
                    path: PathBuf::from("C:/data/note.txt"),
                    source: io_err(),
                },
                "无法读取文件",
                "Cannot read file",
            ),
            (
                "Write",
                CoreError::Write {
                    path: PathBuf::from("C:/data/note.txt"),
                    source: io_err(),
                },
                "无法写入文件",
                "Cannot write file",
            ),
            (
                "BinaryDetected",
                CoreError::BinaryDetected {
                    path: PathBuf::from("C:/data/note.txt"),
                },
                "疑似二进制文件",
                "binary file",
            ),
        ];
        for (name, err, zh_needle, en_needle) in cases {
            let zh = err.message(Lang::ZhCn);
            let en = err.message(Lang::En);
            assert!(zh.contains(zh_needle), "{name} 中文文：{zh}");
            assert!(en.contains(en_needle), "{name} 英文文：{en}");
            assert!(
                !en.chars().any(|c| c as u32 >= 0x4e00 && c as u32 <= 0x9fff),
                "{name} 的英文文案不许夹汉字（改前实测就是夹的）：{en}"
            );
            // 两种语言都得留下定位线索：路径 ＋ OS 细节（BinaryDetected 没有 source）
            assert!(en.contains("note.txt"), "{name} 英文文丢了路径：{en}");
            if name != "BinaryDetected" {
                assert!(
                    en.contains("Access is denied"),
                    "{name} 英文文丢了 OS 细节：{en}"
                );
            }
            // 默认语言仍是中文（切语言不改变没设过语言的路径）
            assert_eq!(err.message(Lang::default()), zh, "{name}");
        }
    }
}
