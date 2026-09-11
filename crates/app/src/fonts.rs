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

/// P154：UI 字体候选表（按界面语言）。**完整族名**列出，逐个查系统清单，
/// 首个命中即用——受系统差异影响（不同 Windows 版本/语言的族名本地化
/// 变体），故英文名与中文名都给上（fontdb 两种写法都在）。
///
/// * 中文简体：微软雅黑系（Win 自带；`Microsoft YaHei UI` 是 Win10+ 的
///   界面优化变体，优先）；
/// * English：Segoe UI 系（Win 原生界面字体；用户点单的 Courier New
///   **不作 UI 主字体**——打字机衬线体做按钮/菜单标签观感旧、小字号发虚，
///   且中文界面下大半字符仍要靠系统回退，收益与损失不成比例；Courier
///   New 保留在行号位候选链里）。
pub(crate) const UI_FONT_CANDIDATES_ZH: [&str; 4] = [
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "微软雅黑",
    "SimSun",
];
pub(crate) const UI_FONT_CANDIDATES_EN: [&str; 3] = [
    "Segoe UI Variable",
    "Segoe UI",
    "Tahoma",
];

/// P154：行号位字体候选链（**等宽**，只画 ASCII 数字故不要求 CJK 覆盖）。
///
/// 为什么行号单独一套：行号是数字位，等宽族的数字同宽、基线稳，13px 小
/// 字号下比正文字体（可能是楷体/比例字体）清晰得多；同时与正文解耦后
/// 「改正文字体」不再影响行号栏几何。候选按「清晰度 + 普及度」排序：
/// `Consolas`（Win 自带、小字号最清晰）→ `Cascadia Mono`（Win11/终端）
/// → `Courier New`（老系统兜底）→ `NSimSun`（含 CJK 的等宽兜底）。
/// **全不命中 → None**，调用方回落 UI 字体（不引入新失败模式）。
pub(crate) const GUTTER_FONT_CANDIDATES: [&str; 5] = [
    "Consolas",
    "Cascadia Mono",
    "Courier New",
    "NSimSun",
    "MS Gothic",
];

/// P154：按语言从系统清单里挑 UI 字体族（纯函数便于单测）。
/// 全不命中 → None（调用方回落 `Font::DEFAULT` 默认无衬线，不引入失败）。
///
/// P155：入参由字符串短码改为 [`editpad_core::Lang`]——语言自 P155 起是
/// 类型化的枚举（文案字符串表按它索引），字体只是它的**下游消费者之一**。
pub(crate) fn pick_ui_font_family(
    language: editpad_core::Lang,
    available: &[String],
) -> Option<&'static str> {
    let candidates: &[&str] = match language {
        editpad_core::Lang::En => &UI_FONT_CANDIDATES_EN,
        editpad_core::Lang::ZhCn => &UI_FONT_CANDIDATES_ZH,
    };
    candidates.iter().copied().find(|cand| {
        let want = editor::normalize_family(cand);
        available
            .iter()
            .any(|family| editor::normalize_family(family) == want)
    })
}

/// P154：从系统清单里挑行号位等宽族（纯函数便于单测）。
/// 全不命中 → None（调用方回落 UI 字体；行号仍受 P150 实测字宽保护）。
pub(crate) fn pick_gutter_font_family(available: &[String]) -> Option<&'static str> {
    GUTTER_FONT_CANDIDATES.iter().copied().find(|cand| {
        let want = editor::normalize_family(cand);
        available
            .iter()
            .any(|family| editor::normalize_family(family) == want)
    })
}

/// P154：把解析出的族名转成 iced 字形族（默认字重/风格）。
pub(crate) fn family_font(name: &'static str) -> Font {
    Font {
        family: iced::font::Family::Name(name),
        ..Font::DEFAULT
    }
}

/// 设置弹窗字体列表单帧最多渲染的行数；超出提示继续过滤。/// Windows 全量族名可达数百条——无上限的 widget 树会让弹窗每帧变重，
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
