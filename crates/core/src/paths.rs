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
//! 数据无缝续用），之后所有拷贝各自建实例目录、互不互通。迁移判据是
//! 「遗留树里还有没搬走的条目吗」，所以半途而废的那次会在下次启动自动续上
//! （P350 把闸门从「实例目录是否存在」改过来，P365 又把它从「遗留 config 还在吗」
//! 改到位——后者只要 config 搬成功就会关门，把失败的快照永远留在原地）。

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
    // P350（池 ⑫）：闸门不能是「实例目录已存在」。`create_dir_all(dir)` 跑在
    // 搬运**之前**，进程在这两步之间被杀、或整批 rename/copy 失败，都会留下一
    // 个空目录 ⇒ 从此每次启动都在这里早退，遗留 config 与崩前快照被永久埋掉，
    // 用户看到的是「设置没了」且没有任何一处会再试。真正的判据是「遗留树里
    // 还有没搬走的条目吗」（`migrate_legacy` 自己问这一问，并逐条跳过已存在的
    // 目标 ⇒ 天然幂等），代价只是每次多一个 read_dir（P365 把这一问补全：旧写法
    // 问的是「遗留 config 还在吗」，而 config 恰恰是那条最先被搬走、一搬走就把
    // 门关上的条目）。
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

/// 把遗留 `%APPDATA%\editpad` 目录内容搬入 `dir`（尽力而为，可反复调用）。
///
/// 判据是「遗留树里还有 `migration_order` 看得见的条目吗」，而不是「`dir` 不存在」
/// 也不是「`legacy/config.toml` 还在」——前者让半途而废的迁移在下次启动自动续上
/// （P350），后者只要 config 搬成功就关门、把同一批里失败的快照永久留下（P365）。
/// 次序由 `migration_order` 做出来：config.toml 恒在最前、其余按名字升序，所以
/// 「并发下最多一个进程搬走 config、其余进程在已存在判定处作罢」这句论证现在有
/// 代码撑着（`read_dir` 本身不保证顺序）。`instances` 子目录（本目录的家）永不进
/// 名单，已存在的目标一律跳过。
fn migrate_legacy(appdata: &Path, dir: &Path) {
    let legacy = appdata.join("editpad");
    // P365（池 ②）：闸门问的是「遗留树里还有没搬走的条目吗」，不是「config 还在吗」。
    // 旧判据的问题在于 config 是**唯一会被搬走**的那一格判据：它一搬成功闸门就关，
    // 而同一批里失败的条目（典型＝`snapshot/`，里面正是崩前未保存的页）从此没人再搬，
    // 用户看到的只是「设置和上次没存的会话一起没了」，且再也没有一次启动会去试。
    let entries = migration_order(&legacy);
    if entries.is_empty() {
        return; // 无遗留数据（含已被并发进程搬完的情形）
    }
    let _ = std::fs::create_dir_all(dir);
    for entry in entries {
        let name = entry.file_name();
        let target = dir.join(&name);
        if target.exists() {
            continue;
        }
        // 移动失败（占用/权限等）退化为复制：数据要到实例目录，遗留侧留下的副本
        // 由上面那道 `target.exists()` 挡着，不会反过来盖掉实例里更新的那一份
        if std::fs::rename(entry.path(), &target).is_err() {
            let _ = copy_entry(&entry.path(), &target);
        }
    }
}

/// 遗留目录里本轮要搬的条目，按**可复现的次序**：`config.toml` 恒在最前，其余按
/// 名字升序；`instances`（实例目录的家，就在遗留根底下）永不进名单。
///
/// P365（池 ②）：排序前那句「顺序固定 config.toml 在先，所以并发下最多一个进程搬走
/// 它、其余进程立即作罢」只写在注释里——`read_dir` 不保证顺序，那句论证从没成立过。
/// 次序做成代码之后它还有一层用处：名单非空 = 「遗留树里还有没搬走的东西」，
/// 这正是迁移闸门该问的问题（旧闸门问的是「config 还在吗」，config 一旦搬走闸门就关，
/// 半途失败的 `snapshot/` 从此没人再搬＝崩前会话永久失踪且零提示）。
fn migration_order(legacy: &Path) -> Vec<std::fs::DirEntry> {
    let Ok(entries) = std::fs::read_dir(legacy) else {
        return Vec::new();
    };
    let mut items: Vec<(String, std::fs::DirEntry)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name != INSTANCES_DIR).then_some((name, entry))
        })
        .collect();
    // `false < true` ⇒ 第一键为 0 的只有 config.toml，其余按名字升序排在它后面
    items.sort_by_key(|(name, _)| (name != "config.toml", name.clone()));
    items.into_iter().map(|(_, entry)| entry).collect()
}

/// 条目复制：`std::fs::copy` 只认文件（对目录返回 InvalidInput），所以
/// 「rename 失败就 copy」那条退路对 `snapshot/` 目录一直是空转的——本函数对
/// 目录自己走下去。尽力而为：单个条目失败不影响其余，剩下的靠下次启动重试
/// （P350 已把闸门从「实例目录是否存在」改成「遗留数据是否还在」）。
fn copy_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let Some(name) = path.file_name() else {
            continue;
        };
        let inner = to.join(name);
        if copy_entry(&path, &inner).is_err() {
            return Err(std::io::Error::other(format!(
                "目录条目复制失败：{}",
                path.display()
            )));
        }
    }
    Ok(())
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
        fs::write(
            appdata.join("editpad").join("config.toml"),
            b"theme = \"dark\"",
        )
        .unwrap();
        fs::write(
            appdata.join("editpad").join("snapshot").join("s1.snap"),
            b"x",
        )
        .unwrap();

        // 首个实例（exe_a）整体迁入：config 与 snapshot 都进了自己的
        // 实例目录，遗留目录不再持有数据
        let adopted = data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap();
        assert_eq!(adopted, root_a);
        assert!(
            adopted.join("config.toml").is_file(),
            "config 必须搬入实例目录"
        );
        assert!(
            adopted.join("snapshot").join("s1.snap").is_file(),
            "快照必须搬入实例目录"
        );
        assert!(
            !appdata.join("editpad").join("config.toml").exists(),
            "遗留目录不得再留 config（互通根被拔掉）"
        );

        // 另一拷贝（exe_b）不再见遗留：拿到自己的空实例目录
        let root_b2 = data_root_for_base(Some(&exe_b), Some(&appdata)).unwrap();
        assert_eq!(root_b2, root_b);
        assert!(!root_b2.join("config.toml").exists(), "新拷贝必须零继承");

        // 迁移幂等：重复调用结果不变
        assert_eq!(
            data_root_for_base(Some(&exe_a), Some(&appdata)).unwrap(),
            adopted
        );

        fs::remove_dir_all(&appdata).ok();
    }

    /// 无遗留数据时不得凭空迁出目录结构（空 editpad 目录保持不动）。
    #[test]
    fn no_legacy_means_fresh_empty_instance() {
        let appdata =
            std::env::temp_dir().join(format!("editpad-paths-none-{}", std::process::id()));
        fs::create_dir_all(&appdata).unwrap();
        fs::create_dir_all(appdata.join("editpad")).unwrap(); // 空遗留目录
        let exe = appdata.join("c").join("editpad.exe");
        let root = data_root_for_base(Some(&exe), Some(&appdata)).unwrap();
        assert!(root.starts_with(appdata.join("editpad").join("instances")));
        assert!(!root.join("config.toml").exists());
        fs::remove_dir_all(&appdata).ok();
    }

    /// 池 ⑫ 的探针：**实例目录存在过 ≠ 数据搬进来了**。旧闸门是
    /// `if dir.exists() { return }`，而 `create_dir_all(dir)` 跑在搬运**之前**
    /// ⇒ 进程在两步之间被杀（或 rename/copy 整批失败）之后，遗留 config 与
    /// 快照被永久埋掉：下次启动直接返回那个空实例目录，设置与崩前会话一起
    /// 无声消失，而且没有任何一处会再试。
    #[test]
    fn an_adopted_but_empty_instance_retries_the_migration() {
        let appdata =
            std::env::temp_dir().join(format!("editpad-paths-retry-{}", std::process::id()));
        let legacy = appdata.join("editpad");
        fs::create_dir_all(legacy.join("snapshot")).unwrap();
        fs::write(legacy.join("config.toml"), b"theme = \"dark\"").unwrap();
        fs::write(legacy.join("snapshot").join("s1.snap"), b"draft").unwrap();

        let exe = appdata.join("prod").join("editpad.exe");
        let dir = legacy.join(INSTANCES_DIR).join(instance_key(&exe));
        // 夹具自证：造出「目录已存在、里面什么都没有」这一半程状态
        fs::create_dir_all(&dir).unwrap();
        assert!(dir.exists() && !dir.join("config.toml").is_file());
        assert!(
            legacy.join("config.toml").is_file(),
            "夹具：遗留数据还原封不动"
        );

        let root = data_root_for_base(Some(&exe), Some(&appdata)).unwrap();
        assert_eq!(root, dir);
        assert!(
            dir.join("config.toml").is_file(),
            "半途而废的迁移必须重试，否则设置永久失踪：{:?}",
            dir
        );
        assert!(dir.join("snapshot").join("s1.snap").is_file());
        assert!(
            !legacy.join("config.toml").exists(),
            "搬完的遗留 config 不该再留着（否则另一拷贝会抢）"
        );

        fs::remove_dir_all(&appdata).ok();
    }

    /// 池 ②：**闸门问错了问题 ⇒ 半途失败之后崩前会话永久失踪**。旧闸门只问
    /// 「`legacy/config.toml` 还在吗」，而搬运次序根本没保证 config 在先（`read_dir`
    /// 不保证顺序），config 又是唯一会被搬走的那一格判据 ⇒ 只要 config 先搬成功、
    /// 别的条目（典型＝`snapshot/`，里面正是崩前未保存的页）那次搬失败，下一次启动
    /// 闸门已经关上，剩下的条目就永远躺在遗留目录里，且零提示。
    #[test]
    fn leftovers_without_legacy_config_still_get_moved() {
        let appdata =
            std::env::temp_dir().join(format!("editpad-paths-leftover-{}", std::process::id()));
        let legacy = appdata.join("editpad");
        let exe = appdata.join("prod").join("editpad.exe");
        let dir = legacy.join(INSTANCES_DIR).join(instance_key(&exe));
        // 夹具直接造出"半途失败之后"的现场：config 已进实例，快照还留在遗留目录
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), b"theme = \"dark\"").unwrap();
        fs::create_dir_all(legacy.join("snapshot")).unwrap();
        fs::write(legacy.join("snapshot").join("s1.snap"), b"draft").unwrap();
        assert!(
            !legacy.join("config.toml").is_file(),
            "夹具自证：旧闸门的判据在这一现场已经关上了"
        );

        let root = data_root_for_base(Some(&exe), Some(&appdata)).unwrap();
        assert_eq!(root, dir);
        assert!(
            dir.join("snapshot").join("s1.snap").is_file(),
            "遗留目录里还有没搬的条目就必须续搬，否则崩前会话永久失踪：{:?}",
            dir
        );
        assert!(
            !legacy.join("snapshot").exists(),
            "搬成功的那条不该在遗留目录留下副本（否则每次启动都重搬）"
        );

        fs::remove_dir_all(&appdata).ok();
    }

    /// 「顺序固定 config.toml 在先」原本只写在注释里，代码没有任何排序——所以
    /// 「并发下最多一个进程搬走它」这句论证从来没成立过。改法是把次序做出来，
    /// 这一格钉的就是次序本身：config 恒为第一个，其余按名字升序（可复现），
    /// 而 `instances`（本目录的家）永不进名单。
    #[test]
    fn migration_order_puts_legacy_config_first_and_skips_instances_dir() {
        let appdata =
            std::env::temp_dir().join(format!("editpad-paths-order-{}", std::process::id()));
        let legacy = appdata.join("editpad");
        fs::create_dir_all(legacy.join(INSTANCES_DIR)).unwrap();
        fs::create_dir_all(legacy.join("snapshot")).unwrap();
        fs::write(legacy.join("z.log"), b"").unwrap();
        fs::write(legacy.join("a.txt"), b"").unwrap();
        fs::write(legacy.join("config.toml"), b"theme = \"dark\"").unwrap();

        let names = migration_order(&legacy)
            .iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "config.toml".to_owned(),
                "a.txt".to_owned(),
                "snapshot".to_owned(),
                "z.log".to_owned()
            ],
            "config 必须在最前、其余按名升序、instances 不进名单"
        );

        fs::remove_dir_all(&appdata).ok();
    }

    /// 兜底复制的机制自证：`fs::copy` 只认文件，对**目录**直接返回 Err，
    /// 所以「rename 失败就 copy」这条退路对 `snapshot/` 一直是空转的。
    #[test]
    fn copy_entry_falls_back_to_a_recursive_copy_for_directories() {
        let appdata =
            std::env::temp_dir().join(format!("editpad-paths-copy-{}", std::process::id()));
        let from = appdata.join("legacy").join("snapshot");
        fs::create_dir_all(from.join("nested")).unwrap();
        fs::write(from.join("s1.snap"), b"one").unwrap();
        fs::write(from.join("nested").join("s2.snap"), b"two").unwrap();
        let to = appdata.join("instance").join("snapshot");
        fs::create_dir_all(to.parent().unwrap()).unwrap();

        // 旧退路的形状：对目录 `fs::copy` 就是失败
        assert!(
            fs::copy(&from, &to).is_err(),
            "夹具自证：fs::copy 不处理目录，所以这条退路上轮一直空转"
        );

        copy_entry(&from, &to).expect("目录必须能整棵复制过去");
        assert_eq!(fs::read(to.join("s1.snap")).unwrap(), b"one");
        assert_eq!(fs::read(to.join("nested").join("s2.snap")).unwrap(), b"two");

        // 文件那条支路照常
        let lone = appdata.join("legacy").join("config.toml");
        let lone_to = appdata.join("instance").join("config.toml");
        fs::write(&lone, b"theme = \"dark\"").unwrap();
        copy_entry(&lone, &lone_to).unwrap();
        assert!(lone_to.is_file());

        fs::remove_dir_all(&appdata).ok();
    }
}
