//! 编辑器配色常量与主题派生（`EditorColors::resolve`）。
//!
//! （P160 自 view.rs 拆出，纯移动零行为变更。）
use super::*;


/// 浅色主题的固定配色（保持 v1 观感）；深色主题在 draw 时由 palette 派生。
pub(super) const SELECTION_COLOR: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.25);
pub(super) const CARET_COLOR: Color = Color::from_rgb8(0x11, 0x11, 0x11);
pub(super) const GUTTER_BG: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
pub(super) const GUTTER_TEXT: Color = Color::from_rgb8(0x99, 0x99, 0x99);
/// P115：组字串视觉与正文同色（用户点单：不再蓝字）——浅/深主题统一
/// 取 palette.text（见 EditorColors::resolve 两分支），下划线同色系
/// 0.6 透明度，与深色主题既有口径一致。
/// 书签圆点（第 60 轮）：琥珀色在浅灰行号栏与深色主题上都醒目，
/// 深浅主题共用一值（与选区/光标不同，它不承担「正文可读性」职能）。
pub(super) const BOOKMARK_COLOR: Color = Color::from_rgb8(0xE0, 0x96, 0x2E);
/// 滚动条命中刻度（P131）：橙色，与书签刻度（琥珀 = BOOKMARK_COLOR）
/// 区分——命中随查找消失属临时态、书签常驻。深浅主题共用一值
///（与书签圆点同款取舍：装饰性标注，不承担正文可读性职能）。
pub(super) const FIND_MARK_COLOR: Color = Color::from_rgb8(0xE0, 0x5A, 0x1E);
/// 括号匹配下划线（第 61 轮）：浅色主题用与查找/预编辑同族的蓝，
/// 深色主题从前景派生（EditorColors::resolve）。
pub(super) const BRACKET_LIGHT: Color = Color::from_rgba8(0x33, 0x66, 0xCC, 0.85);
/// 查找命中底色（P123）：两种主题都用琥珀黄系（主流编辑器惯例），
/// 与选区（蓝系）、书签圆点（琥珀实心）错开——浅色饱和度更高、
/// 深色降透明度防刺眼。
pub(super) const FIND_MATCH_LIGHT: Color = Color::from_rgba8(0xFF, 0xC9, 0x33, 0.45);
pub(super) const FIND_MATCH_DARK: Color = Color::from_rgba8(0xFF, 0xC9, 0x33, 0.28);
/// 一次 draw 用到的全部颜色（按当前主题解析）。
pub(super) struct EditorColors {
    pub(super) selection: Color,
    pub(super) caret: Color,
    pub(super) gutter_bg: Color,
    pub(super) gutter_text: Color,
    pub(super) preedit_text: Color,
    pub(super) preedit_underline: Color,
    pub(super) scrollbar_track: Color,
    pub(super) scrollbar_thumb: Color,
    pub(super) bookmark: Color,
    pub(super) bracket: Color,
    /// 查找命中底色（P123）：查找栏开态全部命中的视口内高亮。
    pub(super) find: Color,
    /// 缩进参考线（P132）：制表位倍数处的淡竖线。
    pub(super) indent_guide: Color,
    /// 右缘标尺线（P132）：固定显示列处的纵向辅助线。
    pub(super) edge_ruler: Color,
    /// 链接悬停下划线（P133）：与括号匹配同族的蓝。
    pub(super) link_underline: Color,
    /// 不可见字符标记（第 64 轮）：与选区同族的淡蓝（低透明度），
    /// 深浅主题都足够「隐」又不至于在白/黑底上消失。
    pub(super) invisibles: Color,
}

impl EditorColors {
    /// 浅色：沿用固定值；深色：从 palette 派生
    /// （行号栏背景=背景提亮、行号/正文/光标/预编辑统一用 palette.text）。
    pub(super) fn resolve(theme: &Theme) -> Self {
        let palette = theme.palette();
        // 前景比背景亮 → 视为深色主题（不依赖具体主题枚举，Custom 也适用）
        let dark = luminance(palette.text) > luminance(palette.background);
        if !dark {
            return Self {
                selection: SELECTION_COLOR,
                caret: CARET_COLOR,
                gutter_bg: GUTTER_BG,
                gutter_text: GUTTER_TEXT,
                // P115：组字串与正文同色（正文恒用 palette.text）
                preedit_text: palette.text,
                preedit_underline: Color { a: 0.6, ..palette.text },
                // 滚动条用前景色低透明度叠加，两种主题都自然成立
                scrollbar_track: Color::from_rgba8(0x00, 0x00, 0x00, 0.05),
                scrollbar_thumb: Color::from_rgba8(0x00, 0x00, 0x00, 0.30),
                bookmark: BOOKMARK_COLOR,
                bracket: BRACKET_LIGHT,
                find: FIND_MATCH_LIGHT,
                // P132：辅助线族——前景低透明度（参考线比标尺更淡），
                // 两种主题都「隐而不失」
                indent_guide: Color { a: 0.14, ..palette.text },
                edge_ruler: Color { a: 0.22, ..palette.text },
                link_underline: BRACKET_LIGHT,
                // 与选区同族的淡蓝（更淡），像素对拍可复用蓝色判据
                invisibles: Color::from_rgba8(0x33, 0x66, 0xCC, 0.30),
            };
        }
        let text = palette.text;
        Self {
            selection: Color { a: 0.25, ..text },
            caret: text,
            gutter_bg: lighten(palette.background, 0.12),
            gutter_text: Color { a: 0.55, ..text },
            preedit_text: text,
            preedit_underline: Color { a: 0.6, ..text },
            scrollbar_track: Color { a: 0.06, ..palette.text },
            scrollbar_thumb: Color { a: 0.38, ..palette.text },
            bookmark: BOOKMARK_COLOR,
            bracket: Color { a: 0.85, ..text },
            find: FIND_MATCH_DARK,
            indent_guide: Color { a: 0.14, ..text },
            edge_ruler: Color { a: 0.22, ..text },
            link_underline: Color { a: 0.85, ..text },
            invisibles: Color { a: 0.32, ..text },
        }
    }
}
