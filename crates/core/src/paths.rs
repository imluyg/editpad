//! 数据目录解析（P101 用户点单：生产/开发隔离）。
//!
//! 常规模式 = 系统配置目录（Windows 上为 `%APPDATA%\editpad`）——配置
//! 与会话快照因此**与 exe 所在位置无关**：任意拷贝的程序都读写同一份
//! 用户数据（改设置、恢复的会话、最近文件全部互通），多份部署无法靠
//! 移动 exe 隔离（用户实测：两个目录的软件「互通」，打字内容互相可见——
//! 机制 = 共享 `%APPDATA%\editpad\snapshot\` 心跳快照）。
//!
//! 便携模式 = exe 同目录放 [`PORTABLE_MARKER`]（`portable.txt`）即启用：
//! 数据根目录改指 exe 所在目录，配置与会话随程序走——一份拷贝自成一体，
//! 与开发版/其他拷贝互不干扰。标记不存在（或 exe 路径不可得）时行为与
//! 既往完全一致，存量用户零变化。

use std::path::{Path, PathBuf};

/// 便携模式标记文件名：与 exe 同目录存在**该文件**（内容可为空）即启用
/// 便携模式。放文件的动作本身就是显式意图，比「检测 exe 目录可写性」
/// 更不易误判（UAC/资源管理器位置也可能恰好可写）。
pub const PORTABLE_MARKER: &str = "portable.txt";

/// 数据根目录（运行时）：exe 旁有 [`PORTABLE_MARKER`] 时 = exe 目录；
/// 否则 = 系统配置目录下的 `editpad` 目录（拿不到返回 None，功能降级）。
pub fn data_root() -> Option<PathBuf> {
    data_root_for(std::env::current_exe().ok().as_deref())
}

/// 可注入 exe 路径的纯判定（单测用；`exe` = None 等价于「查不到自身
/// 路径」——这种情况下无法进入便携模式，恒回退系统配置目录）。
pub fn data_root_for(exe: Option<&Path>) -> Option<PathBuf> {
    let portable = exe
        .and_then(|p| p.parent())
        .map(|dir| dir.join(PORTABLE_MARKER).is_file())
        .unwrap_or(false);
    if portable {
        exe.and_then(|p| p.parent()).map(|d| d.to_path_buf())
    } else {
        dirs::config_dir().map(|d| d.join("editpad"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便携标记切换数据根目录：有标记 → exe 目录；无标记 → 系统配置
    /// 目录（与「移动 exe 无法隔离」的既有行为一致）；exe 路径不可得
    /// 时即使标记存在也不进便携模式（查不到自身位置无从定位标记）。
    #[test]
    fn portable_marker_switches_data_root_to_exe_dir() {
        let dir = std::env::temp_dir()
            .join(format!("editpad-paths-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("editpad.exe");

        // 无标记：系统配置目录（常规模式，行为与既往一致）
        assert_eq!(
            data_root_for(Some(&exe)),
            dirs::config_dir().map(|d| d.join("editpad")),
            "无便携标记必须维持 %APPDATA% 口径"
        );
        // 有标记（空文件即可）：数据根目录 = exe 目录
        std::fs::write(dir.join(PORTABLE_MARKER), b"").unwrap();
        assert_eq!(
            data_root_for(Some(&exe)),
            Some(dir.clone()),
            "便携标记存在时数据必须落在 exe 目录"
        );
        // 标记只认 exe 同目录：别处的标记不影响
        let elsewhere = std::env::temp_dir()
            .join(format!("editpad-paths-elsewhere-{}", std::process::id()));
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join(PORTABLE_MARKER), b"").unwrap();
        assert_eq!(
            data_root_for(Some(&elsewhere.join("other").join("editpad.exe"))),
            dirs::config_dir().map(|d| d.join("editpad")),
            "标记不在 exe 同目录时不得启用便携模式"
        );
        // exe 路径不可得：恒回退系统配置目录
        assert_eq!(
            data_root_for(None),
            dirs::config_dir().map(|d| d.join("editpad"))
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&elsewhere).ok();
    }
}