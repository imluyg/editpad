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
    let wide: Vec<u16> =
        std::ffi::OsStr::new(&name).encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateMutexW(std::ptr::null_mut(), 0, wide.as_ptr())
    };
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

/// 第二实例退出前的原生提示（不依赖 iced 窗口——此刻应用尚未启动）。
#[cfg(windows)]
pub(crate) fn notify_already_running() {
    use std::os::windows::ffi::OsStrExt;

    const MB_OK: u32 = 0;
    const MB_ICONINFORMATION: u32 = 0x40;
    const MB_TOPMOST: u32 = 0x4_0000;
    let title = "Editpad";
    let body = "Editpad 已在运行（同一份拷贝只允许一个实例）。\n\
                如需打开文件，请切换到已打开的 Editpad 窗口。";
    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
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

/// 非 Windows 平台永远不会走到第二实例分支。
#[cfg(not(windows))]
pub(crate) fn notify_already_running() {}

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
