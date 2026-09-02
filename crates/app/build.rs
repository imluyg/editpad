//! 构建脚本（P71）：Windows 下把图标与版本信息编译进 exe 资源段。
//!
//! 直接调用 Windows SDK 的 rc.exe（零新增构建依赖——离线环境拉不到
//! winresource 这类构建 crate，P70 已有先例）。产物 .res 经
//! `cargo:rustc-link-arg` 交给 MSVC link.exe（link 原生接受 .res 输入）。
//! 非 Windows 目标直接跳过。

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

    // 资源产物比源都新 → 跳过 rc.exe 调用（增量构建不重复编译资源）
    let up_to_date = matches!(
        (fs::metadata(&res), fs::metadata(&rc_src)),
        (Ok(res_m), Ok(src_m)) if res_m.modified().ok().zip(src_m.modified().ok()).is_some_and(
            |(r, s)| r >= s
        )
    );
    if up_to_date {
        println!("cargo:rustc-link-arg={}", res.display());
        return;
    }

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
            rc_src.to_string_lossy().into_owned().as_str(),
        ])
        .status()
        .expect("启动 rc.exe 失败");
    if !status.success() {
        panic!("rc.exe 编译资源失败（assets/app.rc → {res:?}）");
    }

    // MSVC link.exe 原生接受 .res 文件作为链接输入
    println!("cargo:rustc-link-arg={}", res.display());
}

/// 在 Windows SDK 安装目录里找 rc.exe（版本取最新，架构 x64 优先）。
fn find_rc() -> Option<PathBuf> {
    const ROOTS: [&str; 2] = [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ];
    for root in ROOTS {
        let entries = match fs::read_dir(root) {
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
