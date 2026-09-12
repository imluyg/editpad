//! 字体与字号：正文字体常量、界面字号换算、CJK 等宽族选择、缩放步进。
//!
//! （P160 自 view.rs 拆出，纯移动零行为变更。）
use super::*;


/// 正文与 UI 共用的字形族。
///
/// 现状 = `Font::MONOSPACE`，其 CJK 缺口由 [`apply_default_cjk_mono_pin`]
/// 在启动期把 `Family::Monospace` 的解析目标钉到系统里的 CJK 等宽字体来补齐
/// （方案 c：零体积治本，不捆绑字体文件）。
pub const BODY_FONT: Font = Font::MONOSPACE;
/// UI 控件字号的固定基准（px）。**P36 用户裁决：UI 不随正文字号缩放**——
/// Ctrl+滚轮（P48 落地）与设置弹窗的步进只调节文件内容，UI 控件保持
/// 固定尺寸。取 16px = iced 默认文本尺寸（`Settings::default_text_size`），
/// 与未缩放时的既有观感持平。
pub const UI_FONT_BASE_PX: f32 = 16.0;
/// Ctrl+滚轮缩放与设置面板步进共用的单步字号增量（px）。单一来源：
/// 两处入口的手感必须一致。
pub(crate) const FONT_ZOOM_STEP: f32 = 2.0;
/// UI 字号相对基准的微调系数（[`GUTTER_FONT_SCALE`] 先例；1.0 = 持平）。
pub const UI_FONT_SCALE: f32 = 1.0;
/// UI 控件统一字号（全项目唯一换算点）：`UI_FONT_BASE_PX × UI_FONT_SCALE`。
///
/// 勘误留痕：iced 0.14 默认文本尺寸实测为 **16px**
/// （`iced_core::settings::Settings::default_text_size`），与默认正文字号相同
/// ——第 25 轮「UI 14px vs 正文 16px」的记录有误，两层割裂实际只在字形族。
pub fn ui_font_px() -> f32 {
    UI_FONT_BASE_PX * UI_FONT_SCALE
}
/// CJK 等宽候选优先级表（P33 方案 c 钉字）：启动期从左到右扫描，
/// 第一个系统已安装的族名被设为 fontdb `Family::Monospace` 的解析目标。
///
/// - 更纱黑体 / Noto Mono CJK SC：社区推荐的中文等宽（用户自装时最优）；
/// - NSimSun（新宋体）/ MingLiU（细明体）：Windows 自带简/繁中文等宽；
/// - MS Gothic（ＭＳ ゴシック）/ Yu Gothic Mono：Windows 自带日文等宽；
/// - 全部未命中 → 不动（保持系统默认等宽解析，非 CJK 环境零行为变化）。
///
/// 刻意不含 SimSun（宋体）：其 ASCII 半宽非严格等宽，会破坏 P14 列映射。
pub const CJK_MONO_CANDIDATES: [&str; 6] = [
    "Sarasa Mono SC",
    "Noto Sans Mono CJK SC",
    "NSimSun",
    "MingLiU",
    "MS Gothic",
    "Yu Gothic Mono",
];
/// 族名归一：去空白 + 小写。候选表条目都是 ASCII 形态族名，
/// 与 fontdb 枚举出的本地化族名做同样宽松的比较即可覆盖大小写/空格变体。
/// P34 起公开：app 层的「配置字体 ↔ 系统清单」宽松匹配复用同一实现。
pub fn normalize_family(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}
/// 从已装字体族名集中挑出第一个命中的 CJK 等宽候选（纯函数便于测试）：
/// 候选表顺序即优先级；无命中返回 None（调用方保持现状不动）。
pub fn pick_cjk_mono_family(available: &[String]) -> Option<&'static str> {
    CJK_MONO_CANDIDATES.iter().find_map(|cand| {
        let want = normalize_family(cand);
        available
            .iter()
            .any(|family| normalize_family(family) == want)
            .then_some(*cand)
    })
}
/// P48：Ctrl+滚轮缩放的单步判定（纯函数可单测）。
/// * `Lines`（滚轮格）：非零即一步，方向取符号（Windows 一格 y=±1）；
/// * `Pixels`（触控板）：增量已折算成行数累积，|累积| ≥ 1 行发一步并
///   **清零**（保留余量会触控板轻扫连发多步；清零 = 一步一格，与滚轮
///   手感一致）。反向增量先抵消同向累积。
///   返回 (新累积值, 步数符号；0 = 本帧不发)。
pub(crate) fn wheel_zoom_step(accum: f32, delta_lines: f32, is_pixels: bool) -> (f32, f32) {
    if is_pixels {
        let accum = accum + delta_lines;
        if accum.abs() >= 1.0 {
            (0.0, if accum > 0.0 { 1.0 } else { -1.0 })
        } else {
            (accum, 0.0)
        }
    } else if delta_lines != 0.0 {
        (accum, delta_lines.signum())
    } else {
        (accum, 0.0)
    }
}
