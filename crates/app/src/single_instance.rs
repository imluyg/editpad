//! 单实例互斥（README 遗留项）：同一份拷贝（同 exe 位置 → 同实例键，
//! P102 命名口径）同时只允许一个进程运行。第二个实例启动时弹原生
//! 提示框后直接退出——比静默退出友好（用户能看到「为什么没反应」），
//! 比跨进程转发轻量（不做 IPC/窗口唤醒，留作远期）。
//!
//! 互斥体名派生自实例键：**不同位置的拷贝各自单实例、彼此可共存**
//! ——与 P102 的数据目录隔离哲学一致，不把「单实例」变成全局锁。

/// 尝试成为唯一实例。返回 false = 已有同实例键的进程在运行。
///
/// 降级口径：exe 路径不可得（无法命名互斥体）或创建互斥体失败时
/// 返回 true 放行——互斥是体验优化，绝不能反过来挡住用户打开编辑器。
/// 成功句柄故意不关闭：生命周期即进程生命周期，退出时由 OS 回收。
#[cfg(windows)]
pub(crate) fn acquire_single_instance() -> bool {
    use std::os::windows::ffi::OsStrExt;

    const ERROR_ALREADY_EXISTS: u32 = 183;

    let Some(name) = editpad_core::instance_mutex_name() else {
        return true;
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, wide.as_ptr()) };
    if handle.is_null() {
        return true;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return false;
    }
    true
}

/// 非 Windows 平台无单实例约束（数据目录隔离本就按 exe 路径，多开无害）。
#[cfg(not(windows))]
pub(crate) fn acquire_single_instance() -> bool {
    true
}

/// 原生消息框（不依赖 iced 窗口——调用点都在应用尚未启动或已退出的时刻）。
///
/// 当前两处消费：单实例「已在运行」提示（[`notify_already_running`]）与
/// 发布构建的 `--help` / `--version` 输出——后者不能走 stdout：P24 起
/// 发布版是 windows 子系统应用，**没有控制台**（见 main.rs 顶部）。
#[cfg(windows)]
pub(crate) fn show_message(title: &str, body: &str) {
    use std::os::windows::ffi::OsStrExt;

    const MB_OK: u32 = 0;
    const MB_ICONINFORMATION: u32 = 0x40;
    const MB_TOPMOST: u32 = 0x4_0000;
    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let title_w = wide(title);
    let body_w = wide(body);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST,
        );
    }
}

/// 非 Windows 平台无原生消息框：调用方改走 stdout。
#[cfg(not(windows))]
pub(crate) fn show_message(_title: &str, _body: &str) {}

/// 第二实例退出前的原生提示（不依赖 iced 窗口——此刻应用尚未启动）。
pub(crate) fn notify_already_running() {
    show_message(
        "Editpad",
        "Editpad 已在运行（同一份拷贝只允许一个实例）。\n\
         如需打开文件，请切换到已打开的 Editpad 窗口。",
    );
}

/// 第二实例的文件转发：把待开路径写进实例目录的**本实例专属**批次
/// 文件（pid 命名）后正常退出（不弹提示——用户意图是打开文件，不是
/// 看弹窗）。已运行实例经轮询按文件逐个取走后在新标签页逐个打开。
/// exe 路径不可得（无法定位实例目录）＝无处可写，与写失败同口径**回报调用点**
/// （M7：不再静默放弃，见下方返回值说明）。
///
/// P148：曾写单个共享握手文件（读旧内容+追加+rename 覆盖）——两个
/// 第二实例并发转发时，后 rename 者的快照不含先写者的新行，整批文件
/// 被静默吞掉；共享临时名 `pending_open.txt.tmp` 同样互踩。pid 命名
/// 天然无并发写者，批次之间互不覆盖。
///
/// M7：返回值＝**这批路径到底交出去没有**。改前 `let _ = std::fs::write` 吞掉失败，
/// 而调用点紧接着就 `return`＝第二实例一声不响地退出：用户双击了文件，既没打开
/// 也没有任何解释（写失败并非罕见——实例目录不可写、磁盘满、同名残骸占位）。
/// 现在失败由调用点退回那条「已在运行，请切到已打开的窗口」提示（沿用既有文案，
/// 不发明新键）。
#[cfg(windows)]
pub(crate) fn forward_pending_open(files: &[std::path::PathBuf]) -> bool {
    let Some(base) = editpad_core::pending_open_path() else {
        return false;
    };
    let Some(dir) = base.parent() else {
        return false;
    };
    let batch = dir.join(format!("pending_open.{}.txt", std::process::id()));
    let mut content = String::new();
    for f in files {
        content.push_str(&f.to_string_lossy());
        content.push('\n');
    }
    std::fs::write(&batch, content.as_bytes()).is_ok()
}

/// 非 Windows 平台无单实例分支，自然也没有转发（返回值恒为"不必提示"）。
#[cfg(not(windows))]
pub(crate) fn forward_pending_open(_files: &[std::path::PathBuf]) -> bool {
    true
}

/// 已运行实例侧：取走实例目录里全部批次文件的待开路径。文件不存在 =
/// 无转发（常态快速路径）。
///
/// P148：读后即删曾是两步——间隙里第二实例 rename 覆盖的新握手文件
/// 会被本实例的 remove 一起删掉，用户双击打开的文件被静默吞掉。改为
/// 逐文件「rename 原子抢占」：先把握手文件改名为本进程独占的名字
/// （`taking-<pid>`；目标已存在或源已消失即失败，天然互斥），再读再删；
/// 抢不到 = 已被并发取走（多屏双击等极端时序），跳过即可。遗留的 `.taking-` 半途
/// 文件（取走后进程崩溃）会被下一轮照常认领读取，自愈——同名（自己上一拍留下的）
/// `rename` 在本机实测是**成功的空操作**，所以不依赖 pid 是否被回收。
/// **M7 改的两处**：①读失败不再删，改回放回原名等下一轮重试（残骸停在认领名上＝
/// 此后每一轮抢占都失败＝转发通道被一次读失败焊死）；②按字节读 + lossy 解码，
/// 半截 UTF-8 的批次只坏一行、不再带走整批。
pub(crate) fn take_pending_open() -> Vec<std::path::PathBuf> {
    let Some(base) = editpad_core::pending_open_path() else {
        return Vec::new();
    };
    let Some(dir) = base.parent() else {
        return Vec::new();
    };
    take_pending_open_in(dir)
}

/// `take_pending_open` 的目录参数化内核，拆出纯粹是为了可测：下面这段
/// 「rename 原子抢占」的并发契约此前**一条用例都没有**，而它守着的是「用户
/// 双击的文件会不会被静默吞掉」（P148）。测试据此在临时目录里跑，绝不碰
/// 真实实例目录。
pub(crate) fn take_pending_open_in(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    // M7（认领名保持 P148 的 taking-<pid>）：曾考虑再补一个 per-call nonce，
    // 理由是"本进程自己的半途文件与认领目标同名 ⇒ rename(源 → 自身) 会失败而
    // 永远认不回"——**本机实测不成立**（同路径 rename 是空操作、返回成功），
    // 相应用例在把命名改回只带 pid 之后照样绿＝恒绿的守卫比没有更糟，故撤掉。
    // 真正有牙、且被用例钉住的是下面两句：读失败不许删，且要把残骸放回它自己的名字。
    let claimed = dir.join(format!("pending_open.taking-{}.txt", std::process::id()));
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("pending_open.") || !name.ends_with(".txt") {
            continue;
        }
        if std::fs::rename(entry.path(), &claimed).is_err() {
            continue; // 已被并发轮询认领 / 目录抖动
        }
        // M7：抢到之后**读失败就不许删**——那是用户双击转发的整批文件，删了永久失踪；
        // 改名放回它自己的名字，下一轮照常重试（`fs::read` 按字节读，坏内容不再是读失败）。
        // 另外这里曾用 `read_to_string`：`fs::write` 不是原子的，第二实例写一半崩溃会留下
        // 半截多字节序列，那种内容整份解码报错 ⇒ 一行坏码带走整批。现在 lossy 解码，
        // 好行照常取出，坏行那一条变成打不开的路径、由装载侧如实报错。
        let Ok(bytes) = std::fs::read(&claimed) else {
            // 读不出来＝暂时读不出来（被杀软占着、目录抖动、这一条根本是个目录）：
            // 放回它自己的名字，下一轮照常重试——残骸若停在认领名上，此后每一轮
            // `rename(新批次 → 同名)` 都失败＝一次读失败把本会话的转发通道焊死。
            let _ = std::fs::rename(&claimed, entry.path());
            continue;
        };
        let _ = std::fs::remove_file(&claimed);
        let text = String::from_utf8_lossy(&bytes);
        out.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(std::path::PathBuf::from),
        );
    }
    out
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        lp_attributes: *mut core::ffi::c_void,
        b_initial_owner: i32,
        lp_name: *const u16,
    ) -> *mut core::ffi::c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    fn MessageBoxW(
        hwnd: *mut core::ffi::c_void,
        text: *const u16,
        caption: *const u16,
        utype: u32,
    ) -> i32;
}
