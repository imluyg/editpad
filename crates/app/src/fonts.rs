use super::*;

/// 启动期把 `Family::Monospace` 的解析目标钉到系统里第一个可用的 CJK
/// 等宽字体（方案 c，候选与优先级见 [`editor::CJK_MONO_CANDIDATES`]）。
///
/// 根因回顾：正文主字体 MONOSPACE 在 Windows 上不含 CJK 字形，cosmic-text
/// 对缺字形的文本按内建回退表（font/fallback/windows.rs：Han 按 locale 分流
/// ja→Yu Gothic / zh→Microsoft YaHei UI…）逐 run 兜底——共享码位汉字
/// （如「我」）与简化专有字（如「现/试」）落入不同回退字体，正是用户截图里
/// 「逐字字形不一」的主缺陷。主字体一旦覆盖 CJK，逐字回退不再参与。
///
/// 实现要点：
/// * 入口 = `iced::advanced::graphics::text::font_system()`（iced 0.14 公开
///   全局，wgpu / tiny-skia 两后端共用）；`raw().db_mut()` 直达 fontdb；
/// * 只改解析目标、不装载任何字体字节——零体积、零内存增量（预算总则入账）；
/// * 进程内一次（AtomicBool 幂等），且发生在首帧排版之前，无缓存失效问题；
/// * 无候选命中（非 CJK 环境/极简系统）静默保持现状，零行为变化。
pub(crate) fn apply_default_cjk_mono_pin() {
    static APPLIED: AtomicBool = AtomicBool::new(false);
    if APPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
        return;
    };
    let families: Vec<String> = font_system
        .raw()
        .db_mut()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    if let Some(family) = editor::pick_cjk_mono_family(&families) {
        font_system.raw().db_mut().set_monospace_family(family);
    }
}

// ---------- 字体选择（P34） ----------

/// 设置弹窗字体列表单帧最多渲染的行数；超出提示继续过滤。
/// Windows 全量族名可达数百条——无上限的 widget 树会让弹窗每帧变重，
/// v1 用「过滤词 + 上限」控制规模（滚动列表本身有高度限制）。
pub(crate) const FONT_PICKER_MAX_ROWS: usize = 200;

/// 枚举系统已装字体的全部族名（精确去重、不区分大小写排序）。
///
/// 数据源与 P33 钉字同路：iced 全局 font_system 的 fontdb——Windows 上
/// 已自动装载 `C:\Windows\Fonts` 等系统目录，无需手写目录扫描（§3 P34
/// 的「手写扫描」路线就此作废，连文件名→族名的换算都省了）。
/// 本地化别名（如「新宋体」与 NSimSun）都会出现在清单里，均为 fontdb
/// 认可的合法名字。失败（锁中毒等）返回空表：弹窗降级为提示文案。
/// 注：包装器的 `raw()` 签名是 `&mut self`，读清单也只能拿写锁（P33 同款，
/// 启动期一次性的短暂临界区）。
pub(crate) fn enumerate_available_families() -> Vec<String> {
    let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
        return Vec::new();
    };
    let mut names: Vec<String> = font_system
        .raw()
        .db()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// 配置字体名 → 实际生效的系统族名（纯函数便于测试）。
///
/// * 配置为空 → None（用默认等宽）；
/// * 精确命中清单 → Some(该条目)；
/// * 宽松命中（去空白 + 小写相等，复用 [`editor::normalize_family`]）→
///   Some(规范条目)——手改 config.toml 的大小写/空白变体被自动矫正；
/// * 未命中 → None：**只回退本次渲染，不抹掉配置**（用户重装字体后
///   自动恢复；启动时给一次状态栏提示）。
pub(crate) fn effective_font_family(configured: Option<&str>, available: &[String]) -> Option<String> {
    let name = configured?;
    if available.iter().any(|f| f == name) {
        return Some(name.to_owned());
    }
    let want = editor::normalize_family(name);
    available
        .iter()
        .find(|f| editor::normalize_family(f) == want)
        .cloned()
}

/// 启动期解析（纯函数便于测试）：返回 (生效族名, 是否提示「未安装」)。
pub(crate) fn resolve_startup_font(
    configured: Option<&str>,
    available: &[String],
) -> (Option<String>, bool) {
    let effective = effective_font_family(configured, available);
    let missing = configured.is_some() && effective.is_none();
    (effective, missing)
}

/// 把族名 `'static` 化：iced 0.14 的 `Family::Name(&'static str)` 不收
/// String，运行期选定的族名只能泄漏进进程生存期。
/// 内存入账（预算总则第 1 条）：每次**切换到不同字体**泄漏一个族名
/// 字符串（几十字节）；字体选择是设置级低频操作，进程内总量可忽略。
/// 启动期至多一次、选择消息每次至多一次，无循环放大路径。
pub(crate) fn leak_font_family(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}
