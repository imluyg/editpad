//! 构建脚本（P71）：Windows 下把图标与版本信息编译进 exe 资源段。
//!
//! 直接调用 Windows SDK 的 rc.exe（零新增构建依赖——离线环境拉不到
//! winresource 这类构建 crate，P70 已有先例）。产物 .res 经
//! `cargo:rustc-link-arg` 交给 MSVC link.exe（link 原生接受 .res 输入）。
//! 非 Windows 目标直接跳过。
//!
//! 版本注入（0.1.1 打包勘误）：assets/app.rc 是模板，@VERSION@ /
//! @VERSION_COMMA@ 由 CARGO_PKG_VERSION 替换后写入 OUT_DIR 再编译——
//! 此前版本号硬编码在 .rc 里，升工作区版本后 exe 资源仍报旧号
//! （0.1.1 打包实测：FileVersion 恒 0.1.0）。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_src = manifest_dir.join("assets").join("app.rc");
    let res = out_dir.join("app.res");

    // 版本注入：数值段 0,1,1,0（FILEVERSION 四元组 = 三段 + 0）、
    // 字符串段 0.1.1，与工作区版本严格同源
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let version_comma = format!("{},0", version.replace('.', ","));

    // 资源产物比所有输入（.rc 与其引用的 .ico）都新 → 跳过 rc.exe 调用
    // （增量构建不重复编译资源）。⚠️ 必须把 .ico 一并纳入：P74 教训——
    // 只盯 .rc 时，换图标不动 .rc 会导致 .res 永不重编，exe 一直嵌旧图标。
    // 另有版本戳：升工作区版本不触碰模板文件，仅靠 mtime 会复用旧 .res。
    let ico_src = manifest_dir.join("assets").join("app.ico");
    let newest_src = [&rc_src, &ico_src]
        .into_iter()
        .filter_map(|p| fs::metadata(p).ok()?.modified().ok())
        .max();
    let stamp_path = out_dir.join("app.res.version");
    let version_ok =
        fs::read_to_string(&stamp_path).ok().is_some_and(|s| s == version);
    let up_to_date = version_ok
        && matches!(
            (fs::metadata(&res).ok().and_then(|m| m.modified().ok()), newest_src),
            (Some(r), Some(s)) if r >= s
        );
    if up_to_date {
        println!("cargo:rustc-link-arg={}", res.display());
        return;
    }

    // 模板 → 注入版本 → OUT_DIR。rc.exe 以 manifest_dir 为工作目录运行，
    // 模板内的 "assets\\app.ico" 相对路径仍指向原位。
    let generated = out_dir.join("app_versioned.rc");
    let template = fs::read_to_string(&rc_src).expect("read app.rc template");
    let generated_src = template
        .replace("@VERSION_COMMA@", &version_comma)
        .replace("@VERSION@", &version);
    fs::write(&generated, generated_src).expect("write versioned app.rc");

    let rc_exe = find_rc().unwrap_or_else(|| {
        panic!(
            "找不到 rc.exe（Windows SDK）：资源编译需要 Windows Kits 10 bin 目录；\
             请确认已安装 Windows SDK 或手动设置环境变量后重试"
        )
    });

    let status = Command::new(&rc_exe)
        .current_dir(&manifest_dir)
        .args([
            "/nologo",
            "/c65001", // .rc 为 UTF-8（含中文描述）
            "/fo",
            // into_owned 后走 String::as_str（Cow::as_str 会解析到
            // 不稳定的 str::as_str，E0658）
            res.to_string_lossy().into_owned().as_str(),
            generated.to_string_lossy().into_owned().as_str(),
        ])
        .status()
        .expect("启动 rc.exe 失败");
    if !status.success() {
        panic!("rc.exe 编译资源失败（{generated:?} → {res:?}）");
    }
    fs::write(&stamp_path, &version).expect("write version stamp");

    // MSVC link.exe 原生接受 .res 文件作为链接输入
    println!("cargo:rustc-link-arg={}", res.display());
}

/// 在 Windows SDK 安装目录里找 rc.exe（版本取最新，架构 x64 优先）。
/// 位置解析次序：注册表 `KitsRoot10`（2026-09-05 电脑重置后 SDK 装在
/// `E:\software\Windows Kits\10`，不在默认盘——注册表是安装器写下的
/// 权威记录，机器无关）→ 两个默认安装盘的硬编码候选（老口径兜底）。
fn find_rc() -> Option<PathBuf> {
    let mut roots = Vec::new();
    // reg query 输出形如 "    KitsRoot10    REG_SZ    <路径>"，取末段
    if let Ok(out) = Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Windows Kits\Installed Roots",
            "/v",
            "KitsRoot10",
        ])
        .output()
    {
        if out.status.success() {
            if let Some(line) = std::str::from_utf8(&out.stdout)
                .ok()
                .and_then(|s| s.lines().find(|l| l.contains("REG_SZ")))
            {
                if let Some(path) = line.split("REG_SZ").nth(1) {
                    // KitsRoot10 指向 SDK 根目录（…\Windows Kits\10\），
                    // rc.exe 在其 bin\<版本>\<arch>\ 之下
                    let root = PathBuf::from(path.trim()).join("bin");
                    roots.push(root.to_string_lossy().into_owned());
                }
            }
        }
    }
    roots.push(r"C:\Program Files (x86)\Windows Kits\10\bin".to_owned());
    roots.push(r"C:\Program Files\Windows Kits\10\bin".to_owned());
    for root in roots {
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        for v in versions.iter().rev() {
            for arch in ["x64", "x86"] {
                let candidate = v.join(arch).join("rc.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}
