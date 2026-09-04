//! CJK 双宽字符的列宽换算 + 列宽运行时实测（P42）
//! （P68 自 editor.rs 拆出，纯移动零行为变更）。

use iced::advanced::text::{self as core_text, Renderer as _, Paragraph as _};
use iced::{alignment, Color, Font, Pixels, Point, Rectangle, Size};

// ---------- CJK 双宽字符的列宽换算 ----------
//
// 等宽假设下：普通字符占 1 列，CJK/全角占 2 列，Tab 展开到下一个
// 制表位（P14，仅显示层——文档里存的仍是真实 `\t` 字符）。
// 覆盖常用区间（CJK 统一表意、扩展A、兼容、假名、谚文、全角符号）。

/// 制表位间距（显示列）。
pub(crate) const TAB_STOP_COLS: usize = 4;

/// P45：`max_line_cols` 惰性收敛的冷却窗——缩短编辑后在 500ms 内最多
/// 全量重算一次（50MB 文档约 50~100ms），连续删除不会每次付出 O(n)。
pub(crate) const RECOMPUTE_MAX_COLS_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(500);

/// 字符是否按「全宽（2 列）」计。
///
/// 基础段来自 Unicode East Asian Width W/F 并集；**第 40 轮按等宽 CJK 字体
/// 实测补充**：以下符号段在 NSimSun（P33 默认钉字）内真实渲染为 2 列全宽，
/// 旧表按 1 列计 → 光标压在这些字符中间且后续逐字符累计漂移
/// （对照测试 `wide_classification_matches_real_glyph_advance` 是护栏）。
/// 取舍：默认正文 = CJK 等宽字体，按全宽分类；用户 P34 另选纯西文等宽字体
/// 时这些字符将反向有 ~1 列误差，属「非等宽/CJK 外字体列对齐漂移」已知取舍。
pub(crate) fn is_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x00A1..=0x00BF // ¡¢£¤¥¦§¨©ª«¬®¯°±²³´µ¶·¸¹º»¼½¾¿（CJK 字体全宽）
            | 0x00D7 // × 乘号
            | 0x00F7 // ÷ 除号
            | 0x1100..=0x115F
            | 0x2014..=0x2027 // — ― ‖ ‘ ’ ‚ ‛ “ ” „ ‟ † ‡ • … ‰/引号族
            | 0x2030..=0x203B // ‰ ‱ ′ ″ ‴ ※ 等
            | 0x2100..=0x213A // ℃ ℅ № 等字母符号
            | 0x2190..=0x21FF // ← ↑ → ↓ ↔ 等箭头
            | 0x2200..=0x22FF // ∀ ∂ ∑ ≈ ≠ ≤ ≥ 等数学符号
            | 0x2500..=0x25FF // ─ │ ┌ ┐ └ ┘ ├ ┤ ▀ ▄ █ ■ □ ◆ 制表/块/几何
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x2FFFD
            | 0x30000..=0x3FFFD
    )
}

/// 字符 `c` 位于显示列 `col` 时占据的宽度（列数）。
/// Tab 推进到下一个 [`TAB_STOP_COLS`] 制表位，至少占 1 列。
pub(crate) fn char_cols(c: char, col: usize) -> f32 {
    match c {
        '\t' => (TAB_STOP_COLS - (col % TAB_STOP_COLS)).max(1) as f32,
        _ => {
            if is_wide(c) {
                2.0
            } else {
                1.0
            }
        }
    }
}

/// 文本的显示列数（1 列 = [`EditorCore::char_width`] 像素）。
    pub(crate) fn display_cols(text: &str) -> f32 {
    let mut col = 0usize;
    for c in text.chars() {
        col += char_cols(c, col) as usize;
    }
    col as f32
}

/// 第 `col` 个字符之前的字符所占显示宽度（像素）；`col` 为字符索引。
    pub(crate) fn prefix_width(text: &str, col: usize) -> f32 {
    let mut width = 0f32;
    for (i, c) in text.chars().enumerate() {
        if i >= col {
            break;
        }
        width += char_cols(c, width as usize);
    }
    width
}

/// 统计待插入文本的「换行单元数」与末行列数（P9）：
/// `\r\n` 与孤立 `\r` 也各算一次换行——旧实现 `split('\n')` 只认 `\n`，
/// CRLF 文本入文后光标列会漂移一个字符。
    pub(crate) fn measure_insertion(text: &str) -> (usize, usize) {
    let mut lines = 0usize;
    let mut tail_cols = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines += 1;
                tail_cols = 0;
            }
            '\n' => {
                lines += 1;
                tail_cols = 0;
            }
            _ => tail_cols += 1,
        }
    }
    (lines, tail_cols)
}

// ---------- 列宽实测（P42） ----------
//
// 列模型的「1 列多宽」必须与真实字形 advance 一致，光标/点击/选区才贴合
// 文字。固定假设（字号×0.5625）按 Consolas 估算，换字体即漂移（NSimSun
// 0.5em → 每字符 +1px）；治本 = 控件层用排版段落实测，见
// [`EditorView::ensure_measured_char_width`]。

/// 实测采样字符数（'0' 的重复次数；样本越大舍入误差越小，成本可忽略）。
const MEASURE_SAMPLE_CHARS: usize = 16;

/// P42：实测列宽有效性校验。等宽字体的 ASCII advance 应落在字号的
/// [0.3, 0.9] 倍（Consolas≈0.55、NSimSun/更纱=0.5、MS Gothic≈0.5）；
/// 超出区间视为测量异常（字体未就绪/排版未生效等），保持固定假设。
pub(crate) fn validate_measured_char_width(w: f32, font_size: f32) -> Option<f32> {
    if !w.is_finite() || w <= 0.0 || font_size <= 0.0 {
        return None;
    }
    let ratio = w / font_size;
    if (0.3..=0.9).contains(&ratio) {
        Some(w)
    } else {
        None
    }
}

/// P42：用排版段落实测等宽列宽（像素/字符）。
///
/// 段落构造即完成 shaping（cosmic-text 走全局 font_system——P33 钉字/
/// P34 选字自动生效），无需 renderer 实例；参数与正文绘制完全同款
/// （Shaping::Advanced + Wrapping::None，见 draw 的 fill_text 调用），
/// 保证量出来的就是画出来的。任何异常返回 None（保持固定假设）。
pub(crate) fn measure_char_width(font: Font, size: f32) -> Option<f32> {
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    let paragraph = <iced::Renderer as core_text::Renderer>::Paragraph::with_text(
        core_text::Text {
            content: "0".repeat(MEASURE_SAMPLE_CHARS).as_str(),
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: Pixels(size),
            line_height: core_text::LineHeight::Absolute(Pixels(size * 1.375)),
            font,
            align_x: core_text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: core_text::Shaping::Advanced,
            wrapping: core_text::Wrapping::None,
        },
    );
    let total = paragraph.min_bounds().width;
    if !(total.is_finite() && total > 0.0) {
        return None;
    }
    Some(total / MEASURE_SAMPLE_CHARS as f32)
}

/// 第 40 轮根治：对单行文本按与正文绘制**完全同源**的段落（同 font/字号/
/// Shaping::Advanced/Wrapping::None）做 shaping，收集每个字符起点的真实
/// 像素 x。`xs[i]` = 第 i 个字符左缘，末项 = 行尾 x（= 段落总宽）。
///
/// * 字形选择（P33 钉字/P34 用户字体/逐字回退）、连字、TAB 展开、分数宽度
///   （如 ▀ 15.01px）全部来自 cosmic-text 同一布局引擎——光标/选区/点击
///   按此定位即与绘制零误差，静态列模型的任何假设破缺都不再累计漂移；
/// * `glyph.cluster` = 字符起始字节偏移（cosmic-text 0.15）；同簇多字形
///   （连字）取最左 x；输出做单调兜底防御乱序；
/// * 空行/量度失败返回可用的最小布局或 None（调用方回退列模型）。
pub(crate) fn shape_row_xs(font: Font, size: f32, text: &str) -> Option<Vec<f32>> {
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    if text.is_empty() {
        return Some(vec![0.0]);
    }
    let paragraph = <iced::Renderer as core_text::Renderer>::Paragraph::with_text(
        core_text::Text {
            content: text,
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: Pixels(size),
            line_height: core_text::LineHeight::Absolute(Pixels(size * 1.375)),
            font,
            align_x: core_text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: core_text::Shaping::Advanced,
            wrapping: core_text::Wrapping::None,
        },
    );
    let mut xs = vec![0.0f32];
    for run in paragraph.buffer().layout_runs() {
        for g in run.glyphs.iter() {
            // cosmic-text 0.15：g.start = run 起始字符索引 + rustybuzz
            // cluster（**字节**偏移，见 shape.rs `start_run + info.cluster`）
            // ——对多字节字符必须按字节回切计数；整行单 run 时 start_run=0，
            // start 即纯字节偏移。绘制位置 = g.x + g.x_offset×字号
            // （physical() 仅多做像素取整 ≤0.5px，取逻辑口径三处自洽）。
            let x = g.x + g.x_offset * g.font_size;
            let char_idx = text[..g.start.min(text.len())].chars().count();
            if char_idx >= xs.len() {
                xs.resize(char_idx + 1, x);
            }
            // 同簇多字形（连字断点）取最左起点
            if x < xs[char_idx] {
                xs[char_idx] = x;
            }
        }
    }
    // 行尾 x = 段落总宽（与整行绘制同源；覆盖末字符无字形/尾随空白）
    xs.push(paragraph.min_bounds().width);
    // 单调兜底：布局引擎按簇序输出，理论已单调；防御性收敛
    for i in 1..xs.len() {
        if xs[i] < xs[i - 1] {
            xs[i] = xs[i - 1];
        }
    }
    Some(xs)
}

/// 字形墨迹在行盒内的纵向范围：上边距 + 墨迹高（px，P88 残影根治 +
/// P89 居中勘误）。
///
/// 光标/选区等「行盒装饰」矩形本按行盒顶（y = 视觉行 × 行高）绘制，而
/// 字形墨迹在行盒（行高 = 字号 × 1.375）内按字体度量下浮 1~6px——
/// 实测（经完整绘制链路）：CJK 等宽钉字上边距 ≈ 4px、拉丁等宽 ≈ 6px、
/// 无钉字回退字体更高。行盒顶对齐时装饰的顶部悬墨落在首行上方空带，
/// 即用户截图的「黑点/色带残影」（P88 根因，headless 测试因字体
/// 管线差异测不出——此前 P42 只实测了列宽，漏了纵轴）。P88 只把装饰
/// 顶边下移到墨迹顶，高度仍 = 行盒高：单选一字时带底悬出行盒下缘、
/// 字被顶在带顶（用户复报「没居中」）——故本函数额外扫描墨迹**底行**，
/// 把「上边距 + 墨迹高」一并交给调用方，选区/列块带按墨迹盒垂直居中
/// （见 `EditorCore::decoration_inset`）。
///
/// 实现：用与正文绘制**完全同源**的渲染管线（同 font/字号/行高/
/// Shaping::Advanced + tiny-skia 光栅化）把采样行画到离屏像素图，
/// 扫描字形墨迹的最高/最低行。采样串用 CJK 字符：捕获钉字/回退后的
/// CJK 字形度量（正文混排的主字体）；渲染失败/参数无效返回 None
/// （调用方保持默认 0 / 整行高 = 旧行为，恒安全）。
pub(crate) fn measure_ink_box(font: Font, size: f32) -> Option<(f32, f32)> {
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    let (w, h) = (160u32, 48u32);
    let mut renderer = iced::Renderer::new(font, Pixels(size));
    renderer.fill_text(
        core_text::Text {
            content: "中中中中中中中中".to_string(),
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: Pixels(size),
            line_height: core_text::LineHeight::Absolute(Pixels(size * 1.375)),
            font,
            align_x: core_text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: core_text::Shaping::Advanced,
            wrapping: core_text::Wrapping::None,
        },
        Point::ORIGIN,
        Color::BLACK,
        Rectangle::with_size(Size::new(w as f32, h as f32)),
    );
    let mut pixels = tiny_skia::Pixmap::new(w, h)?;
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h)?;
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![Rectangle::with_size(Size::new(w as f32, h as f32))];
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
    // 只扫行盒高度内的墨迹（超出视为异常：字体未就绪 → 顶 0 / 整行高
    // 保守回退，装饰保持行盒顶对齐的旧观感）
    let lh = size * 1.375;
    let scan_rows = (lh as i32).clamp(1, h as i32);
    let row_has_ink = |y: i32| -> bool {
        for x in 0..w as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if p.red() < 250 || p.green() < 250 || p.blue() < 250 {
                    return true;
                }
            }
        }
        false
    };
    let mut top = None;
    for y in 0..scan_rows {
        if row_has_ink(y) {
            top = Some(y);
            break;
        }
    }
    let Some(top) = top else {
        return Some((0.0, lh));
    };
    let mut bottom = top;
    for y in (top..scan_rows).rev() {
        if row_has_ink(y) {
            bottom = y;
            break;
        }
    }
    Some((top as f32, (bottom - top + 1) as f32))
}

