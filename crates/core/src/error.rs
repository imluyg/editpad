//! 统一的错误类型。

use std::path::PathBuf;

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
