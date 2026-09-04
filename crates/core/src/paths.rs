//! 数据目录解析（P102 用户点单：多份拷贝自动各搞各的，不要手动标记）。
//!
//! 背景：配置与会话快照曾固定写死在系统目录（`%APPDATA%\editpad`），与
//! exe 位置无关——任意拷贝共享同一份用户数据（设置、最近文件、未保存
//! 会话互相可见），多份部署无法隔离（用户实测「两个软件互通，打字内容
//! 互相可见」= 共享会话快照）。P101 曾引入 `portable.txt` 手动标记，
//! 用户明确不要手动标记。
//!
//! 本方案：**按 exe 位置自动分实例**——数据根目录 =
//! `%APPDATA%\editpad\instances\<规范化 exe 路径的 FNV-1a 64 哈希>`：
//! 每份拷贝/每个目录自动各搞各的数据，零配置零标记；exe 更新/重装到
//! 同一位置时数据延续（同一路径 → 同一哈希）。首次运行迁移：P101 前
//! 布局的遗留 `%APPDATA%\editpad\` 目录整体搬入首个实例目录（原拷贝
//! 数据无缝续用），之后所有拷贝各自建实例目录、互不互通。

use std::path::{Path, PathBuf};

/// 实例目录在 `%APPDATA%\editpad\` 下的子目录名。
const INSTANCES_DIR: &str = "instances";

/// 数据根目录（运行时）：exe 路径 → 对应实例目录；exe 路径不可得时
/// 回退遗留根（拿不到系统配置目录返回 None，功能降级）。
pub fn data_root() -> Option<PathBuf> {
    data_root_for(std::env::current_exe().ok().as_deref())
}

/// 可注入 exe 路径的判定（单测用；`exe` = None 等价于查不到自身路径）。
pub fn data_root_for(exe: Option<&Path>) -> Option<PathBuf> {
    data_root_for_base(exe, dirs::config_dir().as_deref())
}

/// 注入系统配置目录的判定（单测用）：`appdata` = `%APPDATA%` 或测试替身。
pub fn data_root_for_base(exe: Option<&Path>, appdata: Option<&Path>) -> Option<PathBuf> {
    let appdata = appdata?;
    let Some(exe) = exe else {
        // 查不到自身路径：无法定位实例目录，遗留根兜底（功能不变）
        return Some(appdata.join("editpad"));
    };
    let dir = appdata
        .join("editpad")
        .join(INSTANCES_DIR)
        .join(instance_key(exe));
    if dir.exists() {
        return Some(dir);
    }
    // 首次运行迁移：把遗留目录（P101 前布局）整体搬入本实例。
    // 多拷贝并发首启时只有先搬走 config.toml 的进程胜出，其余拷贝看到
    // 源缺失即自建实例目录；迁移失败无害（数据仍留在遗留目录，下次
    // 启动重试），绝不丢数据。
    migrate_legacy(appdata, &dir);
    Some(dir)
}

/// 实例键 = 规范化 exe 路径的 FNV-1a 64 十六进制（稳定、无随机种子：
/// 目录名必须跨进程/跨启动一致）。规范化解析链接与大小写形态；失败
/// （如测试中的虚构路径）回退原路径字符串。
fn instance_key(exe: &Path) -> String {
    let canonical = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    format!("{:016x}", fnv1a64(canonical.to_string_lossy().as_bytes()))
}

/// 单实例互斥体名：按当前 exe 的实例键命名——同一份拷贝（同 exe 位置）
/// 互斥，不同位置的拷贝各自单实例、彼此可共存（与数据目录的 P102
/// 隔离口径一致）。exe 路径不可得时返回 None，调用方降级为允许多开。
pub fn instance_mutex_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("editpad-instance-{}", instance_key(&exe)))
}

/// 单实例转发握手文件：第二实例把「想打开的文件路径」写入这里，
/// 已运行实例轮询读取后在新标签页打开。与互斥体同键（同 exe 位置），
/// 实例目录内平铺一个文件名。
pub fn pending_open_path() -> Option<PathBuf> {
    data_root().map(|root| root.join("pending_open.txt"))
}

/// FNV-1a 64b：无需依赖标准库哈希的随机种子（默认 SipHash 带随机 key，
/// 跨进程不稳定，不可用于目录名）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 把遗留 `%APPDATA%\editpad` 目录内容整体搬入 `dir`（尽力而为）。
///
/// 顺序固定 **config.toml 在先**：并发下最多一个进程搬走它，其余进程
/// 在「源缺失」判定处立即作罢，不会与胜者交错搬同一批文件。`instances`
/// 子目录本身（本目录的家）与已存在的条目跳过。
fn migrate_legacy(appdata: &Path, dir: &Path) {
    let legacy = appdata.join("editpad");
    if !legacy.join("config.toml").is_file() {
        return; // 无遗留数据（含已被并发进程抢先迁移的情形）
    }
    let _ = std::fs::create_dir_all(dir);
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_str() == Some(INSTANCES_DIR) {
            continue; // 不得把实例目录搬进自己
        }
        let target = dir.join(&name);
        if target.exists() {
            continue;
        }
        // 移动失败（占用/权限等）退化为复制：数据要到实例目录，目录
        // 里残留副本无碍（下次启动不再读遗留目录）
        if std::fs::rename(entry.path(), &target).is_err() {
            let _ = std::fs::copy(entry.path(), &target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn mutex_name_is_stable_and_prefixed() {
        // 互斥体名按当前 exe 实例键派生：同进程内两次调用必然一致
        //（跨进程稳定性由 instance_key 的 FNV 无种子性质保证）
        let a = instance_mutex_name().expect("测试进程必能取到自身路径");
        let b = instance_mutex_name().unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("editpad-instance-"), "实际 {a}");
        assert!(a.len() <= 260, "内核对象名长度上限，实际 {a}");
    }

    /// 每个 exe 位置一个实例目录：互不相同、同路径稳定；遗留目录整体
    /// 搬入首个实例且不再 互通（其余拷贝拿自己的空实例目录）。
    #[test]
    fn instances_isolated_by_exe_location_with_legacy_migration_once() {
        let appdata = std::env::temp_dir().join(format!("editpad-paths-{}", std::process::id()));
        let exe_a = appdata.join("prod").join("editpad.exe");
        let exe_b = appdata.join("dev").join("editpad.exe");

        // 无遗留：各拷贝各得一个实例目录，互不相同且同路径稳定
        let root_a = data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap();
        let root_b = data_root_for_base(Some(&exe_b), Some(&appdata)).unwrap();
        assert_ne!(root_a, root_b, "不同位置的拷贝必须各搞各的数据");
        assert!(
            root_a.starts_with(appdata.join("editpad").join("instances")),
            "实例目录必须在 instances 树下：{root_a:?}"
        );
        assert_eq!(
            root_a,
            data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap(),
            "同一 exe 路径必须映射到同一实例目录"
        );

        // exe 路径不可得 → 遗留根兜底（功能不降级为 None）
        assert_eq!(
            data_root_for_base(None, Some(&appdata)),
            Some(appdata.join("editpad"))
        );

        // 构造 P101 前遗留布局：config.toml + snapshot/ + 无关注入文件
        fs::create_dir_all(appdata.join("editpad").join("snapshot")).unwrap();
        fs::write(appdata.join("editpad").join("config.toml"), b"theme = \"dark\"").unwrap();
        fs::write(appdata.join("editpad").join("snapshot").join("s1.snap"), b"x").unwrap();

        // 首个实例（exe_a）整体迁入：config 与 snapshot 都进了自己的
        // 实例目录，遗留目录不再持有数据
        let adopted = data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap();
        assert_eq!(adopted, root_a);
        assert!(adopted.join("config.toml").is_file(), "config 必须搬入实例目录");
        assert!(adopted.join("snapshot").join("s1.snap").is_file(), "快照必须搬入实例目录");
        assert!(
            !appdata.join("editpad").join("config.toml").exists(),
            "遗留目录不得再留 config（互通根被拔掉）"
        );

        // 另一拷贝（exe_b）不再见遗留：拿到自己的空实例目录
        let root_b2 = data_root_for_base(Some(&exe_b), Some(&appdata)).unwrap();
        assert_eq!(root_b2, root_b);
        assert!(!root_b2.join("config.toml").exists(), "新拷贝必须零继承");

        // 迁移幂等：重复调用结果不变
        assert_eq!(data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap(), adopted);

        fs::remove_dir_all(&appdata).ok();
    }

    /// 无遗留数据时不得凭空迁出目录结构（空 editpad 目录保持不动）。
    #[test]
    fn no_legacy_means_fresh_empty_instance() {
        let appdata = std::env::temp_dir().join(format!("editpad-paths-none-{}", std::process::id()));
        fs::create_dir_all(&appdata).unwrap();
        fs::create_dir_all(appdata.join("editpad")).unwrap(); // 空遗留目录
        let exe = appdata.join("c").join("editpad.exe");
        let root = data_root_for_base(Some(&exe), Some(&appdata)).unwrap();
        assert!(root.starts_with(appdata.join("editpad").join("instances")));
        assert!(!root.join("config.toml").exists());
        fs::remove_dir_all(&appdata).ok();
    }
}