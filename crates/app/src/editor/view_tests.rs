//! 控件层无头渲染测试（P68 自 editor.rs tests 拆出；经 #[path] 挂为
//! view 子模块）。全部只走公开 API 构造状态。
use super::*;

/// P88 残影根治回归：光标/选区等「行盒装饰」与字形墨迹纵向对齐——
/// 字体度量让字形在行盒（行高 = 字号 × 1.375）内下浮（实测 CJK 等宽
/// 字体 ≈ 4px），旧实现按行盒顶画装饰，顶部的悬墨落在首行上方空带
/// （用户截图「黑点/色带残影」）。本测试用显式 CJK 字体名（NSimSun，
/// Windows 自带；等价于 GUI 启动 P33 钉字后的字体解析结果）渲染
/// 540 字长行，断言：首行墨迹上方的空带内（行号栏外）零墨迹——光标
/// 与选区两个状态分别验证。用显式字体名而非进程级钉字，避免污染
/// 全局 font_system（钉字是一次性全局变更，会改写其它像素测试的
/// 字体解析结果）。
#[test]
fn headless_caret_and_selection_never_ink_above_first_row() {
    use super::super::CursorPos;
    // 显式 CJK 等宽字体：与 GUI 启动 P33 钉字后的正文字体解析一致
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (500u32, 220u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 460.0f32, 190.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&"中文折行测试".repeat(90)));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 0 };
        c.set_word_wrap(true);
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let gutter = {
        let c = core.borrow();
        c.gutter_width()
    };
    // 首行墨迹最顶行：逐行扫描（含光标列——光标此时已与墨迹对齐）
    let mut first_ink_row = i32::MAX;
    for y in ey as i32..(ey + 2.0 * 22.0) as i32 {
        let mut row_ink = 0u32;
        for x in (ex + gutter) as i32..(ex + ew) as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let v = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if v < 200 {
                    row_ink += 1;
                }
            }
        }
        if row_ink > 0 {
            first_ink_row = y;
            break;
        }
    }
    assert_ne!(first_ink_row, i32::MAX, "首行应有墨迹（渲染管线失效？）");
    // P88：光标矩形顶必须与墨迹顶对齐（±2px 抗锯齿/层裁剪容差）——
    // 修前 = 行盒顶（悬墨 4px，必然超差），修后 = 墨迹顶（零空带）
    let caret_top = {
        let c = core.borrow();
        ey + c.caret_rect_relative().y
    };
    assert!(
        (caret_top - first_ink_row as f32).abs() <= 2.0,
        "光标顶应贴合字形墨迹顶（行盒装饰悬墨未根治）：caret={caret_top} ink={first_ink_row}"
    );
    // 空带 [ey, 首墨行)：正文字形/光标一律不得出现（行号栏除外）
    let mut strip_ink = 0u32;
    for y in ey as i32..first_ink_row {
        for x in (ex + gutter) as i32..(ex + ew) as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let v = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if v < 200 {
                    strip_ink += 1;
                }
            }
        }
    }
    assert_eq!(strip_ink, 0, "光标态：首行上方空带出现墨迹（行盒装饰悬墨）");
    // 选区态复测：跨首段起点选区同样不得在空带留墨
    {
        let mut c = core.borrow_mut();
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 0, col: 2 };
    }
    let mut renderer2 = iced::Renderer::new(font, Pixels(16.0));
    let mut tree2 = Tree::empty();
    let node2 = view.layout(&mut tree2, &renderer2, &limits);
    let node2 = node2.translate(iced::Vector::new(ex, ey));
    let lyt2 = Layout::new(&node2);
    let mut pixels2 = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels2.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask2 = tiny_skia::Mask::new(w, h).expect("mask");
    view.draw(
        &tree2,
        &mut renderer2,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt2,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer2.draw(&mut pixels2.as_mut(), &mut mask2, &viewport, &damage, Color::WHITE);
    let mut sel_strip = 0u32;
    for y in ey as i32..first_ink_row {
        for x in (ex + gutter) as i32..(ex + ew) as i32 {
            if let Some(p) = pixels2.pixel(x as u32, y as u32) {
                let v = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if v < 200 {
                    sel_strip += 1;
                }
            }
        }
    }
    assert_eq!(sel_strip, 0, "选区态：首行上方空带出现墨迹（选区带悬墨）");
}

/// P89 单选字高亮带居中回归（headless 像素级）：P88 只把选区带**顶边**
/// 对齐字形墨迹顶、带高仍 = 行盒高——单选一字时带底悬出行盒下缘、字
/// 被顶在带顶（用户复报「字置顶没居中」）。修复 = 带按字形墨迹盒垂直
/// 居中（带高不变，带内上下留白相等）。本测试渲染「首行单选一个中文
/// 字」断言：①带纵向中心 ≈ 字形墨迹纵向中心（±2px 抗锯齿/取整容差）；
/// ②带顶不低于行盒顶（P88 悬墨不复发）；③带在墨迹上下都有可见余量
/// （居中确有实体，修前态带顶 ≈ 墨迹顶必挂）。
#[test]
fn headless_single_char_selection_band_centered_on_glyph_ink() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 160u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 360.0f32, 120.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str("中"));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 0, col: 1 };
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let (gutter, lh, io, ih, inset) = {
        let c = core.borrow();
        (
            c.gutter_width(),
            c.line_height(),
            c.ink_offset,
            c.ink_height,
            c.decoration_inset(),
        )
    };
    // 墨迹 = 深墨行（字形 + 光标，avg < 180）；带 = 选区蓝混合行
    // （偏离纯白且非深墨：B < 250 且 avg ≥ 180）。行号栏（x < gutter）
    // 与控件外一律不扫。
    let mut ink_top = i32::MAX;
    let mut ink_bot = i32::MIN;
    let mut band_top = i32::MAX;
    let mut band_bot = i32::MIN;
    for y in ey as i32..(ey + eh) as i32 {
        let mut ink_row = false;
        let mut band_row = false;
        for x in (ex + gutter) as i32..(ex + ew) as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let avg = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if avg < 180 {
                    ink_row = true;
                } else if p.blue() < 250 {
                    band_row = true;
                }
            }
        }
        if ink_row {
            ink_top = ink_top.min(y);
            ink_bot = ink_bot.max(y);
        }
        if band_row {
            band_top = band_top.min(y);
            band_bot = band_bot.max(y);
        }
    }
    eprintln!(
        "[P89] io={io:.1} ih={ih:.1} inset={inset:.2} lh={lh:.1} \
         ink={ink_top}..{ink_bot} band={band_top}..{band_bot}"
    );
    assert_ne!(ink_top, i32::MAX, "首行应有字形墨迹（渲染管线失效？）");
    assert_ne!(band_top, i32::MAX, "选区高亮带未渲染");
    // ①墨迹盒访问值须与像素吻合（同源管线，±2 取整/AA 容差）
    assert!(
        (ey + io - ink_top as f32).abs() <= 2.0,
        "墨迹上边距与渲染不符：io={io} ink_top={ink_top}"
    );
    assert!(
        (ey + io + ih - (ink_bot as f32 + 1.0)).abs() <= 2.0,
        "墨迹高度与渲染不符：ih={ih} ink_bot={ink_bot}"
    );
    // ②带顶不低于行盒顶（P88 悬墨不复发）
    assert!(band_top >= ey as i32, "高亮带顶越过行盒顶（悬墨复发）");
    // ③带在墨迹上下都有可见余量（修前态：带顶 ≈ 墨迹顶，必挂此项）
    assert!(band_top < ink_top, "高亮带顶未高于墨迹顶（仍顶对齐）");
    assert!(band_bot > ink_bot, "高亮带底未低于墨迹底（仍顶对齐）");
    // 带纵向中心 ≈ 墨迹纵向中心（±2px）
    let band_mid = (band_top + band_bot) as f32 / 2.0;
    let ink_mid = (ink_top + ink_bot) as f32 / 2.0;
    assert!(
        (band_mid - ink_mid).abs() <= 2.0,
        "高亮带未按字形墨迹居中：带中心 {band_mid} vs 墨迹中心 {ink_mid}"
    );
}

/// P89/P115 组字串纵向定位回归（headless 像素级）：P88 把光标矩形
/// 下移到字形墨迹顶（caret.y 含 ink_offset）后，preedit 一度直接画在
/// caret.y——组字中的字比行内正文低 ink_offset（≈4px），用户复报
/// 「正在输入的字偏下没居中」。正文恒按行盒顶对齐绘制，组字必须同
/// 基准。P115 起组字与正文同色（黑，用户点单），按 x 分区区分两段
/// 墨迹，断言纵向行范围一致（修前组字区低 4px 必挂）。
#[test]
fn headless_preedit_text_same_vertical_band_as_body_text() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 160u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 360.0f32, 120.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str("中"));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 1 };
        assert!(c.ime_preedit("中".to_owned()));
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let lh = core.borrow().line_height();
    let gutter = core.borrow().gutter_width();
    // P115：组字与正文同色（黑）——按 x 分区统计深墨行：
    // 正文区 = [gutter, gutter+16)（col 0 全宽字）；组字区 =
    // [gutter+16, gutter+16+pre_w)（col 1 起，shape 同源实测宽）。
    // 扫描 y 上界留出 C 层下划线带（行盒底 − 3 起 2px）。
    let pre_w = shape_row_xs(font, 16.0, "中")
        .expect("shape 失败")
        .last()
        .copied()
        .unwrap();
    assert!(pre_w > 4.0, "组字宽异常：{pre_w}");
    let body_x0 = (ex + gutter) as i32;
    let pre_x0 = body_x0 + 16; // NSimSun 全宽 16px（col 1 起点）
    let pre_x1 = pre_x0 + pre_w as i32;
    let scan_end = (ey + lh - 4.0) as i32;
    let mut body_top = i32::MAX;
    let mut body_bot = i32::MIN;
    let mut pre_top = i32::MAX;
    let mut pre_bot = i32::MIN;
    for y in ey as i32..scan_end {
        let mut body_ink = false;
        let mut pre_ink = false;
        for x in body_x0..pre_x0 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let avg = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if avg < 180 {
                    body_ink = true;
                }
            }
        }
        for x in pre_x0..pre_x1 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let avg = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if avg < 180 {
                    pre_ink = true;
                }
            }
        }
        if body_ink {
            body_top = body_top.min(y);
            body_bot = body_bot.max(y);
        }
        if pre_ink {
            pre_top = pre_top.min(y);
            pre_bot = pre_bot.max(y);
        }
    }
    eprintln!(
        "[P89] 组字区深墨 {pre_top}..{pre_bot} vs 正文区深墨 {body_top}..{body_bot} (pre_w={pre_w:.1})"
    );
    assert_ne!(body_top, i32::MAX, "正文区墨迹未渲染");
    assert_ne!(pre_top, i32::MAX, "组字区墨迹未渲染（IME 态未生效？）");
    assert!(
        (pre_top - body_top).abs() <= 2 && (pre_bot - body_bot).abs() <= 2,
        "组字与正文纵向错位：组字 {pre_top}..{pre_bot} vs 正文 {body_top}..{body_bot}（修前组字低 ink_offset≈4px）"
    );
}

/// P114 组字 preedit 超右缘回归（headless 像素级）：自动换行开态下
/// preedit 是浮层、不受折行约束，组字中的拼音串会画出文本区右缘（用户
/// 复报「输入超右缘，没受换行影响」）。修复 = 显示裁剪到折行边界
/// （wrap_max_px 同源预算）内，下划线同宽收窄。测试：折行开 + 滚动条
/// 预留 + 光标贴段尾 + 8 拼音字符 preedit——断言折行边界右侧无任何
/// preedit 墨迹（蓝字/下划线），边界左侧（preedit 前部）仍可见。
#[test]
fn headless_preedit_clipped_at_wrap_right_edge_when_wrap_on() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 160u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 300.0f32, 120.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        // 8 行 × 每行 2 视觉段 = 16 视觉行 × 22px > 视口 120px——
        // 垂直滚动条 needed=true → 折行预算按 P99 让位（真实场景）
        let doc = format!("{}\n", "中".repeat(20)).repeat(8);
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.set_word_wrap(true);
        // 与 draw 内 P99 判定同向：内容超视口 → 预算让位
        c.set_wrap_sb_reserve(true);
        // 滚动条恒隐藏（alpha 0）：P115 起组字与正文同色（黑），
        // 深灰 thumb 会误判黑墨——强制不画，界外判据只认组字墨迹
        c.sb_activity = None;
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    // 先 layout 一次：注入实测列宽/墨迹盒/行布局（px_of 真实字形基准）
    let node = view.layout(&mut tree, &renderer, &limits);
    let (gutter, budget, tail_col, seg_tail_px) = {
        let mut c = core.borrow_mut();
        let g = c.gutter_width();
        let px = c.wrap_max_px();
        let text = c.line_text(0);
        let n = text.chars().count();
        // 段 0 尾字符 = 最后一个「右缘 ≤ 预算」的列（像素断行口径，
        // 与段绘制同源；修前用 ASCII 半宽 char_width 算列数会错位）
        let mut t = n.saturating_sub(1);
        while t > 0 && c.px_of(0, &text, t + 1) > px {
            t -= 1;
        }
        let tp = c.px_of(0, &text, t);
        // 光标贴段尾，组一个超预算的长拼音 preedit
        c.cursor = CursorPos { line: 0, col: t };
        assert!(c.ime_preedit("zhongguo".to_owned()));
        (g, px, t, tp)
    };
    // preedit 应被裁到折行边界：边界内仍有墨迹、边界外（含下划线带）零墨迹
    let right_edge = ex + gutter + budget; // scroll_left 开态锁 0
    assert!(
        right_edge < ex + ew,
        "测试前提失效：折行边界必须落在控件内（让位未生效？）"
    );
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    // P115：组字与正文同色（黑）——界外判据 = 深墨（avg < 230；正文
    // 在该 x 区无字：段 0 右缘 ≤ 预算 < right_edge，第二视觉行从
    // 文本区左缘起排；滚动条已禁画）。光标随组字停到折行边界
    // （pre_dx → 竖线在 [right_edge, +2)），排除光标列
    let is_ink = |p: tiny_skia::PremultipliedColorU8| {
        (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230
    };
    let mut outside_ink = 0u32;
    let mut inside_ink = 0u32;
    let caret_excl = right_edge as i32 + 3;
    for y in ey as i32..(ey + 2.0 * 22.0 + 12.0) as i32 {
        for x in caret_excl..(ex + ew) as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if is_ink(p) {
                    outside_ink += 1;
                }
            }
        }
        // 组字起点（段尾光标原处）到折行边界之间的前部
        let start_x = (ex + gutter + seg_tail_px) as i32;
        for x in start_x..right_edge as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if is_ink(p) {
                    inside_ink += 1;
                }
            }
        }
    }
    eprintln!(
        "[P114] gutter={gutter:.0} budget={budget:.0} 段尾col={tail_col} \
         右缘={right_edge:.0} 界内墨px={inside_ink} 界外墨px={outside_ink}（已除光标列）"
    );
    assert!(
        outside_ink <= 4,
        "组字/后文墨迹越过折行边界（>{outside_ink}px；修前 ≈60px 超右缘可见，4px 内为裁剪边界 AA 残留）"
    );
    assert!(inside_ink > 0, "组字前部墨迹缺失（裁过头/未渲染）");
}

/// P115 行中组字回归（headless 像素级）：组字串作为「虚拟插入文本」
/// 参与行绘制——光标在**行中**时后文整体右移组字实测宽（不再与组字
/// 重叠，用户复报「行中打字组字与已打的字重叠」；修前组字浮层直接
/// 盖在后文上、后文原位不动）。关态行 `abcdefghij`、光标 col 3、
/// 组字 `mn`——断言：①组字区 [px(3), px(3)+w) 有墨；②行尾字符 j
/// 不在原位（被推开）；③j 出现在 [px(9)+w, px(9)+w+字宽)；④后文
/// 首字 d 出现在 [px(3)+w, ...)（组字紧跟处起排）。
#[test]
fn headless_mid_line_preedit_pushes_tail_text() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 120u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 360.0f32, 100.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str("abcdefghij"));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 3 };
        assert!(c.ime_preedit("mn".to_owned()));
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let gutter = core.borrow().gutter_width();
    let (px3, px8, px9, ch_w, pre_w) = {
        let c = core.borrow();
        let text = "abcdefghij";
        let p3 = c.px_of(0, text, 3);
        let p8 = c.px_of(0, text, 8);
        let p9 = c.px_of(0, text, 9);
        let cw = c.px_of(0, text, 1); // 1 个 ASCII 字符宽（等宽行）
        let pw = shape_row_xs(font, 16.0, "mn")
            .expect("shape 失败")
            .last()
            .copied()
            .unwrap();
        (p3, p8, p9, cw, pw)
    };
    let x_base = (ex + gutter) as i32;
    let (y0, y1) = (ey as i32, (ey + 22.0) as i32);
    // 深墨计数（avg < 200；字形内部命中，AA 边缘 220+ 不误计）
    let ink_in = |x0: f32, x1: f32| -> u32 {
        let mut n = 0u32;
        for y in y0..y1 {
            for x in (x_base + x0 as i32)..(x_base + x1 as i32) {
                if let Some(p) = pixels.pixel(x as u32, y as u32) {
                    if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 200 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let pre_zone = ink_in(px3, px3 + pre_w); // 组字区
    let i_new = ink_in(px8 + pre_w, px8 + pre_w + ch_w); // i 新位（修前该区为空）
    let j_new = ink_in(px9 + pre_w, px9 + pre_w + ch_w); // j 新位
    let tail_after = ink_in(px9 + pre_w + ch_w, px9 + pre_w + ch_w + 4.0); // 行尾后
    let d_new = ink_in(px3 + pre_w, px3 + pre_w + ch_w); // 后文首字新位
    eprintln!(
        "[P115] px3={px3:.1} px8={px8:.1} px9={px9:.1} ch_w={ch_w:.1} pre_w={pre_w:.1} \
         组字区墨px={pre_zone} i新位墨px={i_new} j新位墨px={j_new} 行尾后墨px={tail_after} d新位墨px={d_new}"
    );
    assert!(pre_zone > 0, "组字未渲染（IME 态未生效？）");
    assert!(d_new > 0, "后文首字未移到组字之后（组字后有空隙/吞字）");
    assert!(i_new > 0, "后文中段未右移组字宽（仍画在未被推开的位置）");
    assert!(j_new > 0, "行尾字符未移到组字之后（后文未右移）");
    assert_eq!(tail_after, 0, "行尾字符右缘之后出现墨迹（右移量异常）");
}

/// P115 勘误回归：空行组字必须渲染（headless 像素级）。三段式把组字
/// 嵌进行绘制后，空行（`text.is_empty()` / `seg_start >= lens`）被整行
/// 跳过——新文档/空白行输入中文时组字整条消失（用户复报「组字直接
/// 没了」；老浮层实现不依赖行循环，空行照常显示）。折行开/关两态各
/// 验：空文档单行、光标 (0,0)、组字 `zhongguo`——断言行首组字区有墨。
#[test]
fn headless_preedit_on_empty_line_renders() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 120u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 360.0f32, 100.0f32);
    let pre_w = shape_row_xs(font, 16.0, "zhongguo")
        .expect("shape 失败")
        .last()
        .copied()
        .unwrap();
    for wrap in [false, true] {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(""));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.set_word_wrap(wrap);
            c.cursor = CursorPos { line: 0, col: 0 };
            assert!(c.ime_preedit("zhongguo".to_owned()));
        }
        let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(font, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        let gutter = core.borrow().gutter_width();
        let x0 = (ex + gutter) as i32;
        let mut ink = 0u32;
        for y in ey as i32..(ey + 22.0) as i32 {
            for x in x0..(x0 + pre_w as i32) {
                if let Some(p) = pixels.pixel(x as u32, y as u32) {
                    if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                        ink += 1;
                    }
                }
            }
        }
        eprintln!(
            "[P115勘误] wrap={wrap} 空行组字区墨px={ink} (pre_w={pre_w:.1})"
        );
        assert!(ink > 0, "空行组字未渲染（wrap={wrap}）：修前整行跳过必挂");
    }
}

/// P115 勘误回归：折行开态**行尾**组字必须渲染（headless 像素级）。
/// 段条件 `col_p < seg_end` 把行尾（末段 col==seg_end==lens）排除，
/// 光标推进到行尾后组字消失。行 `中`×20（折 2 段）、光标行尾
/// col=20、组字 `zhongguo`——末段 [15,20) 内命中：B 画在段内
/// 80px 处，可显示宽 = min(pre_w, 预算 − rel)。断言第二视觉行
/// 段内 [预算 区间] 有组字墨。
#[test]
fn headless_preedit_at_wrapped_line_end_renders() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 140u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 300.0f32, 120.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&"中".repeat(20)));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.set_word_wrap(true);
        c.cursor = CursorPos { line: 0, col: 20 };
        assert!(c.ime_preedit("zhongguo".to_owned()));
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let gutter = core.borrow().gutter_width();
    let pre_w = shape_row_xs(font, 16.0, "zhongguo")
        .expect("shape 失败")
        .last()
        .copied()
        .unwrap();
    // 末段 [15,20)：段内 rel = px(20)-px(15) = 80；B 起点段内 80px；
    // 可显示 = min(pre_w, 预算 − 80)。第二视觉行（y = ey + 22）
    let rel = 80.0f32;
    let budget = {
        let c = core.borrow();
        c.wrap_max_px()
    };
    let vis = pre_w.min(budget - rel);
    assert!(vis > 20.0, "测试前提失效：可显示宽太小（{vis:.1}）");
    let x0 = (ex + gutter) as i32;
    let mut ink = 0u32;
    for y in (ey + 22.0) as i32..(ey + 44.0) as i32 {
        for x in (x0 + rel as i32)..(x0 + (rel + vis) as i32) {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                    ink += 1;
                }
            }
        }
    }
    eprintln!(
        "[P115勘误] 行尾组字：rel={rel:.0} budget={budget:.1} vis={vis:.1} 墨px={ink}"
    );
    assert!(ink > 0, "折行开态行尾组字未渲染（修前段条件排除行尾必挂）");
}

/// P115 续：组字行「插入重排」回归（headless 像素级）——被挤出的后文
/// **换行**到下段（修前在折行边界截断 → 行尾大片空白且缺字，用户复报
/// 「后方让位让出一大片空白，而且没有做到换行」）。折行开态：行
/// `中`×20（原 2 段）、光标 col 8、24 字母组字——合成串按预算重断
/// ≥3 段：断言①第三视觉行（超过原段数）有组字/后文墨迹且末段右缘
/// 与合成流一致（后文折行显示、不截断）；②后续逻辑行（`AB`）整体
/// 下移 k 段（修前在原位必挂）。
#[test]
fn headless_preedit_reflow_wraps_tail_and_shifts_following_lines() {
    use super::super::wrap::pixel_breaks;
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (400u32, 200u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 300.0f32, 180.0f32);
    let preedit = "zhongguo".repeat(6); // 48 字母 ≈384px，跨 ≥2 重排段
    let doc = format!("{}\nAB", "中".repeat(20));
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.set_word_wrap(true);
        c.cursor = CursorPos { line: 0, col: 8 };
        assert!(c.ime_preedit(preedit.to_owned()));
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    // 合成串与断点（与实现同口径，动态）
    let s: String = "中".repeat(8)
        .chars()
        .chain(preedit.chars())
        .chain("中".repeat(12).chars())
        .collect();
    let s_xs = shape_row_xs(font, 16.0, &s).expect("shape 失败");
    let budget = core.borrow().wrap_max_px();
    let breaks = pixel_breaks(&s_xs, budget, &s);
    eprintln!(
        "[P115续] budget={budget:.1} 合成段数={}（原 2 段）s_xs尾={:.1}",
        breaks.len(),
        s_xs.last().copied().unwrap_or(0.0)
    );
    assert!(
        breaks.len() >= 4,
        "测试前提失效：合成串应 ≥4 段（48 字母跨段），实际 {}",
        breaks.len()
    );
    let gutter = core.borrow().gutter_width();
    let x0 = (ex + gutter) as i32;
    let ink_in = |y0: f32, y1: f32, x0r: i32, x1r: i32| -> u32 {
        let mut n = 0u32;
        for y in y0 as i32..y1 as i32 {
            for x in x0r..x1r {
                if let Some(p) = pixels.pixel(x as u32, y as u32) {
                    if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    // ① 第三视觉行（重排新增）有后文墨迹
    let row2 = ink_in(ey + 44.0, ey + 66.0, x0, x0 + 300);
    assert!(row2 > 0, "合成流第三段未渲染（后文未换行到下段）");
    // ② 末段右缘 ≈ 合成流（后文全量显示、未截断缺字）
    let last = breaks.len() - 1;
    let last_right = s_xs[s.chars().count()] - s_xs[breaks[last]];
    let last_y = ey + last as f32 * 22.0;
    let mut ink_right = x0;
    for y in last_y as i32..(last_y + 22.0) as i32 {
        for x in x0..(x0 + 320) {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                    ink_right = ink_right.max(x);
                }
            }
        }
    }
    eprintln!(
        "[P115续] 末段右缘期望 {last_right:.1}px（x0 起），实测 {}({:.1}px)",
        ink_right,
        (ink_right - x0) as f32
    );
    assert!(
        (ink_right - x0) as f32 >= last_right - 8.0,
        "后文未全量折行显示（截断缺字）：末段墨迹右缘 {}px < {}px",
        (ink_right - x0) as f32,
        last_right
    );
    // ③ 后续逻辑行下移 k = 合成段数 − 原段数（原 2 段 → 4 段 → k=2：
    // AB 从 ey+44 移到 ey+88）
    let mut ab_shifted = 0u32;
    for y in (ey + 88.0) as i32..(ey + 110.0) as i32 {
        for x in x0..(x0 + 40) {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                    ab_shifted += 1;
                }
            }
        }
    }
    assert!(
        ab_shifted > 0,
        "后续逻辑行未随重排下移（k 偏移缺失；修前 AB 画在 ey+44 原位必挂）"
    );
    // ④ 组字跨段：**每段**的组字部分下方都应有下划线（用户复报
    // 「下一行的换行没有下划线」——修前只在组字起点段画）。组字串
    // 从段 1 起（词边界断点提前在「中↔拼音」交界），跨段 1/2
    let ul_seg1 = ink_in(ey + 41.0, ey + 43.0, x0, x0 + 300);
    let ul_seg2 = ink_in(ey + 63.0, ey + 65.0, x0, x0 + 300);
    eprintln!("[P115续] 段1下划线墨px={ul_seg1} 段2下划线墨px={ul_seg2}");
    assert!(ul_seg1 > 0, "段 1 组字部分缺下划线");
    assert!(
        ul_seg2 > 0,
        "组字折行到段 2 后该段缺下划线（修前只画起点段必挂）"
    );
}


    /// P46 根治验证（headless 像素级）：完整绘制链路（renderer.fill_text →
/// tiny-skia 光栅化）下，41 汉字行 + 水平滚动（scroll_left=80），
/// 「第 40/41 字」区域必须有墨迹。视口宽 bounds（旧版形态）与
/// INFINITY bounds（新版）对照，定位「41 字封顶」的真实截断点：
/// bounds 只影响 shape 的宽度断言（段落 shape 本身全量，见
/// `paragraph_shapes_full_row_when_bounds_unbounded`），最终可见性
/// 以像素墨迹为准。
#[test]
fn headless_render_shows_characters_beyond_old_viewport_bound() {
    let (w, h) = (630u32, 200u32);
    let text: String = "字".repeat(41);
    let gutter = 60.0f32;
    let scroll_left = 80.0f32;
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let damage = vec![viewport_rect];
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);

    for (label, bounds_w) in [("旧视口宽570", 570.0f32), ("新INFINITY", f32::INFINITY)] {
        let mut renderer = iced::Renderer::new(Font::MONOSPACE, Pixels(16.0));
        renderer.fill_text(
            core_text::Text {
                content: text.clone(),
                bounds: Size::new(bounds_w, 22.0),
                size: Pixels(16.0),
                line_height: core_text::LineHeight::Absolute(Pixels(22.0)),
                font: Font::MONOSPACE,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: core_text::Shaping::Advanced,
                wrapping: core_text::Wrapping::None,
            },
            Point::new(gutter - scroll_left, 10.0),
            Color::BLACK,
            viewport_rect,
        );
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        // 文本起点 x = gutter - scroll_left = -20；第 40 字逻辑 x 600..616，
        // 第 41 字 616..656 → 屏幕 640..656 区间在 [620,640) 采样第 41 字
        let (x0, x1) = (620i32, 640i32);
        let mut ink = 0u32;
        for y in 4..24 {
            for x in x0..x1 {
                if let Some(px) = pixels.pixel(x as u32, y as u32) {
                    if px.alpha() > 0 {
                        ink += 1;
                    }
                }
            }
        }
        eprintln!("[{label}] 第41字区域墨迹像素 = {ink}");
        assert!(
            ink > 20,
            "[{label}] 行尾第41字不可见（墨迹 {ink}px）——字形截断仍存在"
        );
    }
}

/// P59 诊断：小数 scroll_top（平滑滚动/触控板增量的等价物）下渲染真实
/// EditorView，控件边界外不得有任何墨迹——越界绘制会压到标签条/状态栏，
/// 且越界区域不在损伤区内、部分重绘从不清除，逐帧叠加即用户截图的
/// 「缓慢滚动花屏」。本测试先复现（若红）后钉住修复（若绿）。
#[test]
fn headless_fractional_scroll_paints_no_ink_outside_bounds() {
    let (w, h) = (700u32, 500u32);
    // 编辑器摆在 (50, 60)、尺寸 600×300——四周留出可检测的越界带
    let (ex, ey, ew, eh) = (50.0f32, 60.0f32, 600.0f32, 300.0f32);

    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let doc_text: String =
            (1..=20).map(|i| format!("第{i}行内容\n")).collect();
        c.reset_document(editpad_core::Document::from_str(&doc_text));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        // 平滑滚动到小数行位：首行半可见（顶部越界带）+ 末行半可见（底部）
        c.scroll_by_lines(-2.5);
    }
    let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };

    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let mut tree = Tree::empty();
    // 控件尺寸 = 600×300（limits 收紧到目标尺寸，Fill 才解析成 600×300
    // 而非整个画布——首版脚手架此处给错，"越界"多为合法绘制）
    let limits = layout::Limits::new(
        Size::new(ew, eh),
        Size::new(ew, eh),
    );
    let node = view.layout(&mut tree, &renderer, &limits);
    // 平移到 (50, 60)：四周留出可检测的越界带（原点渲染无法检测上方越界）
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);

    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    let damage = vec![viewport_rect];
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );

    // 统计控件边界外的墨迹（暗像素；底色已填白），按方位带细分定位
    let (x0, y0) = (ex as u32, ey as u32);
    let (mut top, mut bottom, mut left, mut right) = (0u32, 0u32, 0u32, 0u32);
    let mut top_x = (u32::MAX, 0u32);
    let mut bottom_box: Option<(u32, u32, u32, u32)> = None;
    let mut left_box: Option<(u32, u32, u32, u32)> = None;
    let grow = |box_: &mut Option<(u32, u32, u32, u32)>, x: u32, y: u32| {
        *box_ = match *box_ {
            None => Some((x, y, x, y)),
            Some((x_min, y_min, x_max, y_max)) => Some((
                x_min.min(x),
                y_min.min(y),
                x_max.max(x),
                y_max.max(y),
            )),
        };
    };
    for y in 0..h {
        for x in 0..w {
            let inside = x >= x0
                && x < x0 + ew as u32
                && y >= y0
                && y < y0 + eh as u32;
            if inside {
                continue;
            }
            if let Some(px) = pixels.pixel(x, y) {
                if px.red() < 200 {
                    if y < y0 {
                        top += 1;
                        top_x = (top_x.0.min(x), top_x.1.max(x));
                    } else if y >= y0 + eh as u32 {
                        bottom += 1;
                        grow(&mut bottom_box, x, y);
                    } else if x < x0 {
                        left += 1;
                        grow(&mut left_box, x, y);
                    } else {
                        right += 1;
                    }
                }
            }
        }
    }
    let escaped = top + bottom + left + right;
    // 调试产物：整帧渲染结果存 PNG，人工核对越界内容
    let _ = pixels.save_png("target/p59-debug.png");
    {
        let c = view.core.borrow();
        eprintln!(
            "[P59] scroll_top={} visible_range={:?} gutter_w={} lh={} viewport={}x{}",
            c.scroll_top,
            c.visible_range(),
            c.gutter_width(),
            c.line_height(),
            c.viewport_w,
            c.viewport_h,
        );
    }
    // 逃逸像素的包围盒（按带），定位泄漏源
    let mut bbox: Option<(u32, u32, u32, u32)> = None; // (min_x, min_y, max_x, max_y)
    for y in 0..h {
        for x in 0..w {
            let inside = x >= x0
                && x < x0 + ew as u32
                && y >= y0
                && y < y0 + eh as u32;
            if inside {
                continue;
            }
            if let Some(px) = pixels.pixel(x, y) {
                if px.red() < 200 {
                    bbox = match bbox {
                        None => Some((x, y, x, y)),
                        Some((x_min, y_min, x_max, y_max)) => Some((
                            x_min.min(x),
                            y_min.min(y),
                            x_max.max(x),
                            y_max.max(y),
                        )),
                    };
                }
            }
        }
    }
    eprintln!(
        "[P59] 越界墨迹 = {escaped}（上 {top} / 下 {bottom} / 左 {left} / 右 {right}），上带x={top_x:?}，下带盒={bottom_box:?}，左带盒={left_box:?}，控件 = ({x0},{y0},{ew},{eh})"
    );
    assert_eq!(
        escaped, 0,
        "小数滚动下控件边界外不得有墨迹（压标签条/状态栏/重影）"
    );
}

fn p66_render_frame(core: &EditorHandle, scroll_top: f32) -> tiny_skia::Pixmap {
    let (w, h) = (700u32, 500u32);
    {
        let mut c = core.borrow_mut();
        c.scroll_top = scroll_top;
        c.clamp_scroll();
    }
    let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };

    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(
        Size::new(600.0, 300.0),
        Size::new(600.0, 300.0),
    );
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(50.0, 60.0));
    let lyt = Layout::new(&node);

    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    let damage = vec![viewport_rect];
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
    pixels
}

#[test]
fn p66_gutter_ink_moves_with_smooth_scroll() {
    // 用户报告「行号不跟随内容移动」的像素级对拍：scroll_top=0 与
    // 2.5 两帧，行号栏（gutter 列）的墨迹必须不同——数字随视口连续
    // 更新，而不是钉在固定槽位
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let doc_text: String =
            (1..=40).map(|i| format!("第{i}行内容\n")).collect();
        c.reset_document(editpad_core::Document::from_str(&doc_text));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
    }

    let frame_a = p66_render_frame(&core, 0.0);
    let frame_b = p66_render_frame(&core, 2.5);

    // 对拍有效性：滚动必须真实改变画面（否则下面的比较无意义）
    let mut total_diff = 0u32;
    for y in 0..500u32 {
        for x in 0..700u32 {
            let da = frame_a.pixel(x, y).map(|p| p.red()).unwrap_or(255);
            let db = frame_b.pixel(x, y).map(|p| p.red()).unwrap_or(255);
            if da != db {
                total_diff += 1;
            }
        }
    }
    eprintln!("[P66] 全帧差异像素 = {total_diff}");
    assert!(total_diff > 1000, "两帧画面几乎相同，滚动未生效");

    let (gx0, gx1) = (50u32, 50u32 + core.borrow().gutter_width() as u32);
    let dark = |px: tiny_skia::PremultipliedColorU8| px.red() < 200;
    let gutter_ink_rows = |frame: &tiny_skia::Pixmap| -> Vec<bool> {
        (0..500u32)
            .map(|y| {
                (gx0..gx1).any(|x| {
                    frame.pixel(x, y).map(dark).unwrap_or(false)
                })
            })
            .collect()
    };
    let ga = gutter_ink_rows(&frame_a);
    let gb = gutter_ink_rows(&frame_b);
    let ink_a = ga.iter().filter(|v| **v).count();
    let ink_b = gb.iter().filter(|v| **v).count();
    eprintln!("[P66] gutter 墨迹行 A={ink_a} B={ink_b}");
    let diff_rows = ga.iter().zip(&gb).filter(|(a, b)| a != b).count();
    eprintln!("[P66] gutter 行墨迹差异行数 = {diff_rows}");
    assert!(diff_rows >= 10, "两帧行号栏墨迹几乎相同？行号没有随滚动移动");

    // P66附 护栏：行号右缘必须贴在「gutter 右缘 − GUTTER_MIN」处（±3px）
    // ——左对齐位置由 num_w 估算，估算漂移会在这里暴露（损伤矩形依赖
    // position/size 与真实字形一致，漂移即重蹈 Right 对齐的错位覆辙）
    let mut x_max = 0u32;
    for x in gx0..gx1 {
        let mut col_has_ink = false;
        for y in 0..500u32 {
            if frame_a.pixel(x, y).map(dark).unwrap_or(false) {
                col_has_ink = true;
                break;
            }
        }
        if col_has_ink {
            x_max = x;
        }
    }
    let expect_right = gx1 as f32 - GUTTER_MIN;
    eprintln!("[P66] 行号最右墨迹列 = {x_max}，期望 ≈ {expect_right}");
    assert!(
        (x_max as f32 - expect_right).abs() <= 3.0,
        "行号右缘漂移：实际 {x_max}，期望 ≈ {expect_right}（num_w 估算失准）"
    );
}

/// P59 微实验（钉住上游缺陷）：iced 0.14 tiny-skia 的 `fill_text` 第 4 参
/// clip_bounds 对 Cached 文本**不裁剪**——flush 掩码快路径
/// `physical_bounds.is_within(clip)` 的 physical 就是 clip 自身，恒真 →
/// 整段无掩码绘制。本测试断言逃逸**存在**：升级 iced 后若此断言翻转
/// （clip 生效、逃逸归零），可重新依赖裁剪并简化规避手段。
/// P66 起的现行规避 = 三兄弟图层（见 draw）：文本层四周内缩迫使快路径
/// 判假、强制走掩码路径；整行对齐/越界行跳过已随像素平滑滚动退役。
#[test]
fn fill_text_clip_bounds_is_not_reliable_upstream() {
    let (w, h) = (400u32, 300u32);
    let clip = Rectangle::new(Point::new(0.0, 100.0), Size::new(400.0, 200.0));
    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    renderer.fill_text(
        core_text::Text {
            content: "逃逸测试".to_owned(),
            bounds: Size::new(f32::INFINITY, 22.0),
            size: Pixels(16.0),
            line_height: core_text::LineHeight::Absolute(Pixels(22.0)),
            font: BODY_FONT,
            align_x: core_text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: core_text::Shaping::Advanced,
            wrapping: core_text::Wrapping::None,
        },
        Point::new(50.0, 89.0), // 11px 在 clip 上方之外
        Color::BLACK,
        clip,
    );
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![Rectangle::with_size(Size::new(w as f32, h as f32))];
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);

    // 裁剪区外（y < 100）的墨迹
    let mut escaped = 0u32;
    for y in 0..100u32 {
        for x in 0..w {
            if let Some(px) = pixels.pixel(x, y) {
                if px.red() < 200 {
                    escaped += 1;
                }
            }
        }
    }
    eprintln!("[P59-微] clip 外墨迹 = {escaped}（上游缺陷钉住：>0 即未裁剪）");
    assert!(
        escaped > 0,
        "iced 已修复 fill_text 裁剪？请复核 P59 的源头规避是否可以简化"
    );
}

// ---------- P42 实测注入（P68 自 core_tests 迁入：横跨 view/core 两层） ----------

#[test]
fn ensure_measured_char_width_measures_and_dedups() {
    // 控件层全链路：layout 首帧实测注入（走全局 font_system，
    // P33 钉字生效）；本机等宽字体 advance 必然落在合法区间
    let view = EditorView {
        core: EditorHandle::default(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };
    view.ensure_measured_char_width();
    let core = view.core.borrow();
    assert_eq!(core.metric_key, Some((BODY_FONT, 16.0)));
    let w = core
        .measured_char_w
        .expect("系统字体可用时实测不应失败");
    assert!(
        (0.3..=0.9).contains(&(w / core.font_size())),
        "实测列宽 {w} 超出合理倍率"
    );
    drop(core);

    // 同键二次调用去重（值稳定不抖动）
    view.ensure_measured_char_width();
    let core = view.core.borrow();
    assert_eq!(core.metric_key, Some((BODY_FONT, 16.0)));
    assert_eq!(core.measured_char_w, Some(w));

    // 换字号 → 换键重测：倍率保持（advance ∝ 字号）
    drop(core);
    view.core.borrow_mut().set_font_size(20.0);
    view.ensure_measured_char_width();
    let core = view.core.borrow();
    assert_eq!(core.metric_key, Some((BODY_FONT, 20.0)));
    let w20 = core.measured_char_w.expect("换字号重测不应失败");
    assert!((w20 / 20.0 - w / 16.0).abs() < 0.05, "倍率应守恒 {w20}");
}

// ---------- P69 渲染帧成本基线（此前无任何帧耗时数据） ----------

#[test]
fn render_frame_cost_is_bounded_on_large_document() {
    // 虚拟化契约的帧成本体检（P69）：10 万行文档上**稳态帧**（同一
    // Renderer/Tree 连续滚动渲染，字形缓存温热——与真实应用渲染循环
    // 同构）应只随**可见行数**（~20 行）变化，与文档总大小无关。
    // P42 行级布局注入（每帧对可见行 shaping）与 P66 三层裁剪都作用
    // 在这条路径上，此处建立首个帧耗时基线。冷启动帧（全新 Renderer、
    // 字形缓存全冷）单测另测不设上限，只打印供参考。
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let doc: String = (0..100_000)
            .map(|i| format!("第{i}行 内容若干 中英混排 token\n"))
            .collect();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
    }

    let (w, h) = (700u32, 500u32);
    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));

    // 冷启动帧：全新 Renderer（字形缓存全冷），只打印不设限
    {
        let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };
        let mut tree = Tree::empty();
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let t = std::time::Instant::now();
        view.draw(&tree, &mut renderer, &Theme::Light,
            &iced::advanced::renderer::Style::default(), lyt,
            mouse::Cursor::Unavailable, &viewport_rect);
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &[viewport_rect], Color::WHITE);
        eprintln!(
            "[P69] 冷启动帧（全新 Renderer）= {:.2} ms（参考值，不设限）",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }

    // 稳态帧：同一 Renderer/Tree/视图连续滚动渲染（真实渲染循环同构）
    let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };
    let mut tree = Tree::empty();
    let node = view.layout(&mut tree, &renderer, &limits);
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];

    // 预热 2 帧让字形缓存温热（每帧先 reset 清层栈——真实渲染循环同款）
    for k in 0..2 {
        renderer.reset(viewport_rect);
        core.borrow_mut().scroll_top = 50_000.0 + k as f32;
        view.draw(&tree, &mut renderer, &Theme::Light,
            &iced::advanced::renderer::Style::default(), lyt,
            mouse::Cursor::Unavailable, &viewport_rect);
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    }

    let mut samples = Vec::new();
    let mut draw_times = Vec::new();
    let mut raster_times = Vec::new();
    for k in 0..5 {
        renderer.reset(viewport_rect);
        core.borrow_mut().scroll_top = 50_100.0 + k as f32 * 7.5;
        let t = std::time::Instant::now();
        view.draw(&tree, &mut renderer, &Theme::Light,
            &iced::advanced::renderer::Style::default(), lyt,
            mouse::Cursor::Unavailable, &viewport_rect);
        let draw = t.elapsed();
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        draw_times.push(draw);
        raster_times.push(t.elapsed() - draw);
        samples.push(t.elapsed());
    }
    samples.sort();
    let median = samples[2];
    eprintln!(
        "[P69] 10 万行文档稳态帧中位数 = {:.2} ms（样本 {:?}）",
        median.as_secs_f64() * 1000.0,
        samples
    );
    eprintln!(
        "[P69] 分解：view.draw 中位 {:.2} ms / 光栅中位 {:.2} ms",
        draw_times.iter().map(|d| d.as_secs_f64() * 1000.0).fold(f64::MAX, f64::min),
        raster_times.iter().map(|d| d.as_secs_f64() * 1000.0).fold(f64::MAX, f64::min),
    );
    // 虚拟化契约钉住：我们控制的 view.draw 必须 O(可见行)——10 万行文档
    // 上仍应为亚毫秒级。光栅（renderer.draw）成本在上游 tiny-skia/swash
    // （忙机器实测 ~6ms/可见行、空帧 ~11ms，与第 52 轮 highlight 2× 同源
    // 的机器状态敏感），只打印观测不设硬上限，建议安静机器复测建立
    // 真实基线（P41 CPU 软渲染取舍的量化数据）。
    let draw_median = {
        let mut d = draw_times.clone();
        d.sort();
        d[2]
    };
    eprintln!(
        "[P69] view.draw 中位 = {:.2} ms（虚拟化契约：10 万行文档上应 <5ms）",
        draw_median.as_secs_f64() * 1000.0
    );
    assert!(
        draw_median < std::time::Duration::from_millis(5),
        "view.draw {:?} 超出虚拟化契约（帧成本随文档规模增长？）",
        draw_median
    );

    // 判别实验：空文档（1 空行）同管线光栅成本——若与 10 万行相近，
    // 则成本为每帧结构开销（层/掩码/背景）而非文档规模
    let tiny = EditorHandle::default();
    let mut tiny_view = EditorView { core: tiny.clone(), font: BODY_FONT, zoom_accum: 0.0 };
    let mut tiny_tree = Tree::empty();
    let tiny_node = tiny_view.layout(&mut tiny_tree, &renderer, &limits);
    let tiny_lyt = Layout::new(&tiny_node);
    let mut tiny_raster = Vec::new();
    for _ in 0..5 {
        renderer.reset(viewport_rect);
        let t = std::time::Instant::now();
        tiny_view.draw(&tiny_tree, &mut renderer, &Theme::Light,
            &iced::advanced::renderer::Style::default(), tiny_lyt,
            mouse::Cursor::Unavailable, &viewport_rect);
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        tiny_raster.push(t.elapsed());
    }
    tiny_raster.sort();
    eprintln!(
        "[P69] 空文档同管线帧中位 = {:.2} ms（判别：接近大文档 → 结构开销主导；远小 → 字形/文档规模主导）",
        tiny_raster[2].as_secs_f64() * 1000.0
    );
}

/// 第 60 轮（headless 像素级）：书签圆点必须画在行号栏左侧条带内——
/// 标记行在条带采样区出现琥珀墨迹，摘除后同区归零；且圆点不得污染
/// 条带右侧的行号数字区（越界即条带几何漂移）。
#[test]
fn headless_bookmark_dot_ink_lives_in_gutter_strip() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    // gutter 公式与 draw 同源（10 行 → 1 位数 → max(3) 位；未注入实测
    // 列宽 = 默认字号 × 0.5625 固定假设）
    let char_w = 16.0f32 * 0.5625;
    let gutter_w = BOOKMARK_STRIP + GUTTER_MIN + 3.0 * char_w;

    let render = |marked: bool| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            let doc_text: String = (1..=10).map(|i| format!("第{i}行\n")).collect();
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 2, col: 0 }; // 第 3 行
            if marked {
                c.toggle_bookmark();
            }
        }
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    // 琥珀墨迹判定：接近 BOOKMARK_COLOR(0xE0,0x96,0x2E) 的暖色像素。
    // ⚠️ 无头 tiny_skia::Pixmap 的 pixel() 通道序为 BGRA（第 60 轮探针
    // 实测：琥珀圆点读出 (46,150,224)），按倒置通道判定。
    let amber_ink = |px: &tiny_skia::Pixmap, x0: u32, x1: u32| -> u32 {
        let mut ink = 0u32;
        for y in 0..h {
            for x in x0..x1 {
                if let Some(p) = px.pixel(x, y) {
                    if p.blue() > 180 && p.green() > 100 && p.red() < 120 {
                        ink += 1;
                    }
                }
            }
        }
        ink
    };

    let frame = render(true);
    let (ex_u, strip_end) = (ex as u32, (ex + BOOKMARK_STRIP) as u32);
    let gutter_end = (ex + gutter_w) as u32;
    let strip_ink = amber_ink(&frame, ex_u, strip_end);
    eprintln!("[P60] 书签条带琥珀墨迹 = {strip_ink}px");
    assert!(
        strip_ink >= 8,
        "标记行的书签圆点未出现在条带采样区（墨迹 {strip_ink}px）"
    );
    // 圆点不得越出条带：数字区（条带右缘 → gutter 右缘）无琥珀墨迹
    let number_zone_ink = amber_ink(&frame, strip_end, gutter_end);
    assert_eq!(
        number_zone_ink, 0,
        "书签圆点墨迹污染行号数字区 {number_zone_ink}px（条带几何漂移）"
    );
    // 摘除后条带归零
    let clean = render(false);
    assert_eq!(
        amber_ink(&clean, ex_u, strip_end),
        0,
        "无书签时条带不应有任何琥珀墨迹"
    );
}

/// 第 61 轮（headless 像素级）：括号匹配下划线必须落在两侧配对括号所
/// 在行的底部条带内；光标离开括号后全画布零蓝墨迹（无幽灵下划线）。
/// 采样口径：BRACKET_LIGHT(0x3366CC@0.85) 叠白底真彩 ≈ (82,125,212)；
/// ⚠️ 无头 tiny_skia::Pixmap 的 pixel() 通道序为 BGRA（第 60 轮探针
/// 实测先例），故 red() 读到的是 B 通道——判 red()>180 && blue()<120
/// && 100<green()<180。正文黑/灰抗锯齿三通道相近不可能同时满足，
/// 书签琥珀（B 通道≈46）亦不误入。
#[test]
fn headless_bracket_underline_ink_at_matched_pair() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);

    let render = |cursor: CursorPos| -> (tiny_skia::Pixmap, f32) {
        let core = EditorHandle::default();
        let lh = {
            let mut c = core.borrow_mut();
            let mut doc_text = String::from("(x)\n");
            doc_text.extend((2..=10).map(|i| format!("第{i}行\n")));
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = cursor;
            c.line_height()
        };
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        (pixels, lh)
    };

    // 蓝墨迹计数（通道序注意见上）：[y0,y1) 条带 × 全宽
    let blue_ink = |px: &tiny_skia::Pixmap, y0: u32, y1: u32| -> u32 {
        let mut ink = 0u32;
        for y in y0..y1.min(h) {
            for x in 0..w {
                if let Some(p) = px.pixel(x, y) {
                    if p.red() > 180 && p.blue() < 120 && p.green() > 100 && p.green() < 180 {
                        ink += 1;
                    }
                }
            }
        }
        ink
    };

    // 光标在 '(' 前：两条 2px 下划线分别落在第 1 行底部的 '(' 与 ')' 下方
    let (frame, lh) = render(CursorPos { line: 0, col: 0 });
    let band_y0 = (ey + lh - 4.0) as u32;
    let band_y1 = (ey + lh + 2.0) as u32;
    let ink = blue_ink(&frame, band_y0, band_y1);
    eprintln!("[P81] 括号下划线蓝墨迹 = {ink}px");
    assert!(ink >= 8, "配对括号行底未见下划线墨迹（仅 {ink}px）");

    // 光标移到无括号邻接的第 2 行：全画布不得有任何蓝色墨迹
    let (clean, _) = render(CursorPos { line: 1, col: 0 });
    assert_eq!(
        blue_ink(&clean, 0, h),
        0,
        "无括号邻接时画布出现蓝色墨迹（幽灵下划线/其他越界墨迹）"
    );
}

/// 第 64 轮（headless 像素级）：不可见字符标记开关——同一份文档在
/// 关/开两次渲染间，除标记 quad 外应逐像素一致；开启后必须产生足量
/// 差异墨迹（第 1 行的空格点 + 制表符横杠 + 各行行尾短竖标）。
/// 用「帧间差分」而非颜色判据：淡蓝标记叠白底后与灰阶抗锯齿难以
/// 单色区分，差分天然免疫。
#[test]
fn headless_invisibles_marks_toggle_frame_diff() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);

    let render = |show: bool| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            let mut doc_text = String::from("a b\tc\n");
            doc_text.extend((2..=10).map(|i| format!("第{i}行\n")));
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(360.0);
            c.set_viewport_height(260.0);
            c.cursor = CursorPos { line: 5, col: 0 }; // 远离首行，光标不参与差分
            c.set_invisibles(show, show);
        }
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(360.0, 260.0), Size::new(360.0, 260.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(20.0, 20.0));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let off = render(false);
    let on = render(true);
    let mut diff = 0u32;
    for y in 0..h {
        for x in 0..w {
            if let (Some(a), Some(b)) = (off.pixel(x, y), on.pixel(x, y)) {
                if (a.red() as i32 - b.red() as i32).abs() > 8
                    || (a.green() as i32 - b.green() as i32).abs() > 8
                    || (a.blue() as i32 - b.blue() as i32).abs() > 8
                {
                    diff += 1;
                }
            }
        }
    }
    eprintln!("[P84] 不可见标记差异墨迹 = {diff}px");
    assert!(diff >= 12, "开/关两帧差异墨迹不足（仅 {diff}px），标记未画出");
}

/// 第 66 轮 主线 A 手段 4：滚动×字号×主题 组合批——任意组合下，
/// 控件矩形之外的画布必须保持纯背景色（P59/P66 越界墨迹历史病灶的
/// 参数化回归网）。文档含长行（水平滚动活动）、书签（gutter 墨迹）、
/// CJK+emoji 与末行无换行形态。
#[test]
fn headless_combo_scroll_size_theme_ink_stays_in_bounds() {
    use super::super::CursorPos;
    let (w, h) = (420u32, 320u32);
    let (ex, ey, ew, eh) = (30.0f32, 24.0f32, 340.0f32, 240.0f32);

    let build = |font_px: f32, dark: bool, scroll: f32| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            let mut doc = String::from("alpha beta gamma\n\tindent 中文 🚀\n");
            for i in 0..40 {
                doc.push_str(&format!("row{i} lorem ipsum dolor sit amet\n"));
            }
            doc.push_str("wide-末行-without-newline");
            c.reset_document(editpad_core::Document::from_str(&doc));
            c.set_font_size(font_px);
            c.toggle_bookmark(); // 行 0：gutter 琥珀圆点参与越界检查
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 20, col: 6 };
            c.scroll_top = scroll;
            c.clamp_scroll();
        }
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(font_px));
        let mut tree = Tree::empty();
        let limits =
            layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            if dark { &Theme::Dark } else { &Theme::Light },
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let mut frames = 0u32;
    for font_px in [12.0f32, 16.0, 28.0] {
        for dark in [false, true] {
            for scroll in [0.0f32, 3.5, 80.0] {
                let px = build(font_px, dark, scroll);
                let mut out_of_bounds_ink = 0u32;
                for y in 0..h {
                    for x in 0..w {
                        let inside = x >= ex as u32
                            && x < (ex + ew) as u32
                            && y >= ey as u32
                            && y < (ey + eh) as u32;
                        if inside {
                            continue;
                        }
                        if let Some(p) = px.pixel(x, y) {
                            // 画布底为纯白：控件外任何非白像素都是越界墨迹
                            if (p.red() as i32 - 255).abs() > 2
                                || (p.green() as i32 - 255).abs() > 2
                                || (p.blue() as i32 - 255).abs() > 2
                            {
                                out_of_bounds_ink += 1;
                            }
                        }
                    }
                }
                frames += 1;
                assert_eq!(
                    out_of_bounds_ink, 0,
                    "fs={font_px} dark={dark} scroll={scroll}: 越界墨迹 \
                     {out_of_bounds_ink}px"
                );
            }
        }
    }
    eprintln!("[P86] 组合批帧数 = {frames}");
}

/// 第 67 轮 ⑮（headless 像素级）：列块选区高亮——同一文档关/开块两帧
/// 差分必须出现足量墨迹（2 行 × 2 列宽的选区 quad），且墨迹全部落在
/// 控件矩形内。
#[test]
fn headless_block_selection_highlight_frame_diff() {
    use super::super::{BlockSel, CursorPos};
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);

    let render = |block: bool| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            let mut doc_text = String::from("abcdef\nghijkl\n");
            doc_text.extend((3..=10).map(|i| format!("第{i}行\n")));
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 5, col: 0 };
            if block {
                c.block_sel = Some(BlockSel {
                    anchor: CursorPos { line: 0, col: 1 },
                    head: CursorPos { line: 1, col: 3 },
                });
            }
        }
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let off = render(false);
    let on = render(true);
    let mut diff_in = 0u32;
    let mut diff_out = 0u32;
    for y in 0..h {
        for x in 0..w {
            if let (Some(a), Some(b)) = (off.pixel(x, y), on.pixel(x, y)) {
                let d = (a.red() as i32 - b.red() as i32).abs()
                    + (a.green() as i32 - b.green() as i32).abs()
                    + (a.blue() as i32 - b.blue() as i32).abs();
                if d > 8 {
                    let inside =
                        x >= ex as u32 && x < (ex + ew) as u32 && y >= ey as u32 && y < (ey + eh) as u32;
                    if inside {
                        diff_in += 1;
                    } else {
                        diff_out += 1;
                    }
                }
            }
        }
    }
    eprintln!("[P87] 列块高亮差分：块内 {diff_in}px / 越界 {diff_out}px");
    assert!(diff_in >= 40, "块选区高亮墨迹不足（仅 {diff_in}px）");
    assert_eq!(diff_out, 0, "高亮不得越出控件矩形");
}

/// 第 73 轮 ⑯（headless 像素级）：软换行开/关两帧差分——长行开启后
/// 折行段必须出现在**后续视觉行**（关态该处为空），且全部墨迹落在控件
/// 矩形内（续行必须从文本区左缘起排，禁止绝对列叠加）。
#[test]
fn headless_wrap_on_produces_segment_ink_in_lower_visual_rows() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    // 行 0 = 80 个 'a'（折 ~3 段），行 1 = 空幻影
    let doc = format!("{}\n", "a".repeat(80));

    let render = |wrap: bool| -> (tiny_skia::Pixmap, f32) {
        let core = EditorHandle::default();
        let lh = {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(&doc));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 0, col: 0 };
            c.set_word_wrap(wrap);
            c.line_height()
        };
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        (pixels, lh)
    };

    let band_ink = |px: &tiny_skia::Pixmap, band: u32, lh: f32, gutter: f32| -> u32 {
        let y0 = (ey + band as f32 * lh).max(0.0) as u32;
        let y1 = (ey + (band + 1) as f32 * lh).min(h as f32) as u32;
        let x0 = (ex + gutter).max(0.0) as u32;
        let mut ink = 0u32;
        for y in y0..y1 {
            for x in x0..w {
                if let Some(p) = px.pixel(x, y) {
                    // 正文墨迹 = 非背景色（行号栏 x0 之前已排除）
                    if p.red() < 245 || p.green() < 245 || p.blue() < 245 {
                        ink += 1;
                    }
                }
            }
        }
        ink
    };

    let (off, lh) = render(false);
    let (on, _) = render(true);
    let gutter = 49.0f32; // 行号栏宽随行数/列宽（3 位数字 × 9px + 常量）
    // 关态：行 0 只占视觉行 0；行 1 = 空幻影 → 第 1 段带无墨迹
    assert_eq!(band_ink(&off, 1, lh, gutter), 0, "关态首行下方不应有正文墨迹");
    // 开态：折行段出现在视觉行 1、2（80 字符 ≈ 3 段）；段 3 起始
    // x 必须从文本区左缘起（左缘 ~= gutter 处有墨迹）
    assert!(band_ink(&on, 1, lh, gutter) > 100, "开态视觉行 1 缺折行段墨迹");
    assert!(band_ink(&on, 2, lh, gutter) > 100, "开态视觉行 2 缺折行段墨迹");
    // 开态越界检查：控件矩形之外零墨迹
    let mut oob = 0u32;
    for y in 0..h {
        for x in 0..w {
            let inside = x >= ex as u32
                && x < (ex + ew) as u32
                && y >= ey as u32
                && y < (ey + eh) as u32;
            if inside {
                continue;
            }
            if let Some(p) = on.pixel(x, y) {
                if (p.red() as i32 - 255).abs() > 2
                    || (p.green() as i32 - 255).abs() > 2
                    || (p.blue() as i32 - 255).abs() > 2
                {
                    oob += 1;
                }
            }
        }
    }
    eprintln!("[P93] 折行段墨迹带 1={} 带 2={} 越界={oob}", band_ink(&on, 1, lh, gutter), band_ink(&on, 2, lh, gutter));
    assert_eq!(oob, 0, "开态折行墨迹越出控件矩形 {oob}px");
}

/// 第 73 轮 ⑯：开态组合批（font × theme × scroll）——任意组合下控件
/// 矩形之外必须保持纯背景（P86 组合批的 word_wrap 扩维，设计 §5 验收）。
#[test]
fn headless_wrap_on_combo_ink_stays_in_bounds() {
    use super::super::CursorPos;
    let (w, h) = (420u32, 320u32);
    let (ex, ey, ew, eh) = (30.0f32, 24.0f32, 340.0f32, 240.0f32);

    let build = |font_px: f32, dark: bool, scroll: f32| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            let mut doc = String::from("alpha beta gamma\n\tindent 中文 🚀\n");
            for i in 0..30 {
                doc.push_str(&format!("row{i} lorem ipsum dolor sit amet\n"));
            }
            doc.push_str("wide-末行-without-newline");
            c.reset_document(editpad_core::Document::from_str(&doc));
            c.set_font_size(font_px);
            c.toggle_bookmark(); // 行 0 书签圆点参与越界检查
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 20, col: 6 };
            c.set_word_wrap(true);
            c.scroll_top = scroll;
            c.clamp_scroll();
        }
        let mut view = EditorView { core, font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(font_px));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            if dark { &Theme::Dark } else { &Theme::Light },
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let mut frames = 0u32;
    for font_px in [12.0f32, 16.0, 28.0] {
        for dark in [false, true] {
            for scroll in [0.0f32, 3.5, 80.0] {
                let px = build(font_px, dark, scroll);
                let mut out_of_bounds_ink = 0u32;
                for y in 0..h {
                    for x in 0..w {
                        let inside = x >= ex as u32
                            && x < (ex + ew) as u32
                            && y >= ey as u32
                            && y < (ey + eh) as u32;
                        if inside {
                            continue;
                        }
                        if let Some(p) = px.pixel(x, y) {
                            if (p.red() as i32 - 255).abs() > 2
                                || (p.green() as i32 - 255).abs() > 2
                                || (p.blue() as i32 - 255).abs() > 2
                            {
                                out_of_bounds_ink += 1;
                            }
                        }
                    }
                }
                frames += 1;
                assert_eq!(
                    out_of_bounds_ink, 0,
                    "wrap fs={font_px} dark={dark} scroll={scroll}: 越界墨迹 \
                     {out_of_bounds_ink}px"
                );
            }
        }
    }
    eprintln!("[P93] 折行开态组合批帧数 = {frames}");
}

/// 第 75 轮 用户点单（P96 排查）：窗口缩放后折行必须重新适配——先以
/// 800 宽渲染，再把视口扩到 1200 重新渲染：新帧墨迹右缘必须贴住新右缘
/// （旧实现若断点不重算，拉宽后文字停在旧右缘留下整条空白）。
#[test]
fn headless_wrap_reaments_after_viewport_grow() {
    use super::super::CursorPos;
    let (w, h) = (1280u32, 600u32);
    let (ex, ey, ew1, eh) = (0.0f32, 0.0f32, 800.0f32, 600.0f32);
    let doc = format!("{}\n", "a".repeat(220));
    // 220 字符：800 宽 ~2 段；1200 宽 ~1 段——段数变化证明断点重算
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 0 };
    }

    let render = |ew: f32| -> tiny_skia::Pixmap {
        core.borrow_mut().set_viewport_width(ew);
        core.borrow_mut().set_word_wrap(true);
        let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let ink_right = |px: &tiny_skia::Pixmap| -> u32 {
        let mut max_x = 0u32;
        for y in 0..h {
            for x in 0..w {
                if let Some(p) = px.pixel(x, y) {
                    if (p.red() < 245 || p.green() < 245 || p.blue() < 245)
                        && x > max_x {
                            max_x = x;
                        }
                }
            }
        }
        max_x
    };

    let p800 = render(ew1);
    // 折行段数必须随宽度变化（800 宽 mc≈85 → 3 段；1200 宽 mc≈130 → 2 段）
    {
        let c = core.borrow();
        assert_eq!(c.line_visual_segments(0), 3, "800 宽 220 字符应折 3 段");
    }
    let p1200 = render(1200.0);
    {
        let c = core.borrow();
        assert_eq!(c.line_visual_segments(0), 2, "1200 宽应折 2 段——断点已随宽度重算");
    }
    let gap1 = (ew1 as u32).saturating_sub(ink_right(&p800) + 1);
    let gap2 = (1200u32).saturating_sub(ink_right(&p1200) + 1);
    eprintln!("[P96/P115] 800 宽右gap={gap1}px  1200 宽右gap={gap2}px");
    // P115 用户点单：行尾与文本区右缘恒留一个汉字宽（= 正文字号）——
    // 右缘余量 ∈ [16, 16+字符宽]，不再贴死右缘（修前 ≤12px 贴边口径）
    assert!(
        (14..=40).contains(&gap1),
        "800 宽行尾距右缘应 ≈ 一个汉字宽（P115 余量），实测 {gap1}px"
    );
    // 拉宽后必须按新预算重新贴余量（断点随宽度重算）
    assert!(
        (14..=40).contains(&gap2),
        "扩宽后行尾距右缘 ≈ 一个汉字宽，实测 {gap2}px（断点停在旧宽度？）"
    );
}

/// P99 用户点单（headless 像素级）：折行文本贴满右缘后，行尾字符被
/// 最右侧垂直滚动条盖住/显得截断——内容超出视口（滚动条 needed）时，
/// 折行预算必须按滚动条可视带宽让位：滑块带内只允许滚动条自身的
/// 半透明墨迹（灰 ≈178），不得出现正文纯黑墨迹（旧行为：正文画满到
/// 控件右缘，带内是纯黑正文与滑块混叠）。两帧渲染：首帧惰性收敛
/// 行程（v1 模型），次帧 needed 判定生效。
#[test]
fn headless_wrap_reserves_scrollbar_band_no_text_ink_under_thumb() {
    use super::super::CursorPos;
    use super::super::scrollbars::VERTICAL_SCROLLBAR_RESERVE;
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    // 800 字符：折 ~23 段 ≈ 506px > 260px 视口 → 垂直滚动条需要出现
    let doc = format!("{}\n", "a".repeat(800));

    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 0 };
        c.set_word_wrap(true);
    }

    let render = || -> tiny_skia::Pixmap {
        let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };

    let _ = render(); // 首帧：惰性收敛可见行行程（v1 模型）
    let px = render(); // 次帧：needed 判定 → 预留生效

    // 滚动条可视带宽内：最暗像素 ≥100（纯滑块 ≈178；正文纯黑混叠 <100）
    let band_x0 = (ex + ew - VERTICAL_SCROLLBAR_RESERVE).round() as u32;
    let mut darkest = 255u8;
    for y in 0..h {
        for x in band_x0..w {
            if let Some(p) = px.pixel(x, y) {
                darkest = darkest.min(p.red().min(p.green()).min(p.blue()));
            }
        }
    }
    eprintln!("[P99] 滚动条带宽 [{band_x0},{w}) 最暗像素 = {darkest}");
    assert!(
        darkest >= 100,
        "滚动条带宽内出现正文墨迹（最暗 {darkest}）——行尾仍被滑块盖住"
    );
    // 对照：预留线左侧必须仍有正文墨迹（折行整体在滑块左侧收尾）
    let mut text_ink = 0u32;
    for y in 0..h {
        for x in (band_x0 - 60)..band_x0 {
            if let Some(p) = px.pixel(x, y) {
                if p.red() < 100 && p.green() < 100 && p.blue() < 100 {
                    text_ink += 1;
                }
            }
        }
    }
    assert!(text_ink > 15, "预留线左侧应仍有正文墨迹（仅 {text_ink}px）");
}

/// P116 勘误探测：**同状态连续两帧像素必须一致**（用户复报「字号调到
/// 24 时字闪来闪去、段落不停出现消失」——帧间布局振荡的嫌疑）。遍历
/// 字号 × 视口宽 × 多行混合文本，同一 EditorView 连续渲染两帧比对；
/// 滚动条 alpha 用系统时钟（两帧有毫秒差，可能进入淡出）——置
/// sb_activity=None 排除，只留正文/组字墨迹的帧差。任何 diff 即振荡
/// 组合（修后此测试恒绿，作为缩放稳定护栏）。
#[test]
fn headless_no_frame_oscillation_after_font_zoom() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    // 多行混合文本：中文长句（折多段）+ 英文短行交错，覆盖段落出现/
    // 消失观感
    let lines: Vec<String> = (0..36)
        .map(|i| {
            if i % 3 == 0 {
                format!("中文长文本段落第{ }行{}", i + 1, "abcdefghijklmnopqrstuvwxyz".repeat(4))
            } else {
                format!("line {} with some english {}", i + 1, "word ".repeat(8))
            }
        })
        .collect();
    let doc = lines.join("\n");
    for size in [16.0f32, 20.0, 22.0, 24.0, 26.0, 28.0, 32.0, 40.0, 48.0] {
        for ew in [600.0f32, 900.0, 1200.0] {
            let core = EditorHandle::default();
            {
                let mut c = core.borrow_mut();
                c.reset_document(editpad_core::Document::from_str(&doc));
                c.set_viewport_width(ew);
                c.set_viewport_height(600.0);
                c.set_font_size(size);
                c.set_word_wrap(true);
                c.cursor = CursorPos { line: 0, col: 0 };
                c.sb_activity = None; // 滚动条 alpha 有时钟差，排除
            }
            let (w, h) = (1300u32, 640u32);
            let (ex, ey) = (10.0f32, 10.0f32);
            let render = || -> tiny_skia::Pixmap {
                let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
                let mut renderer = iced::Renderer::new(font, Pixels(size));
                let mut tree = Tree::empty();
                let limits = layout::Limits::new(Size::new(ew, 600.0), Size::new(ew, 600.0));
                let node = view.layout(&mut tree, &renderer, &limits);
                let node = node.translate(iced::Vector::new(ex, ey));
                let lyt = Layout::new(&node);
                let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
                pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
                let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
                let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
                let viewport =
                    iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
                let damage = vec![viewport_rect];
                view.draw(
                    &tree,
                    &mut renderer,
                    &Theme::Light,
                    &iced::advanced::renderer::Style::default(),
                    lyt,
                    mouse::Cursor::Unavailable,
                    &viewport_rect,
                );
                renderer
                    .draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
                pixels
            };
            let a = render();
            let b = render(); // 同一状态第二帧（真实 GUI 每帧同态重绘）
            let mut diff = 0u32;
            for y in 0..h {
                for x in 0..w {
                    if let (Some(pa), Some(pb)) = (a.pixel(x, y), b.pixel(x, y)) {
                        let d = (pa.red() as i32 - pb.red() as i32).abs()
                            + (pa.green() as i32 - pb.green() as i32).abs()
                            + (pa.blue() as i32 - pb.blue() as i32).abs();
                        if d > 8 {
                            diff += 1;
                        }
                    }
                }
            }
            assert_eq!(
                diff, 0,
                "同状态两帧像素不一致（帧间振荡）：字号 {size} 视口宽 {ew} diff={diff}px"
            );
        }
    }
}

/// P116 勘误探测 2：**内容高度贴视口边缘**时垂直滚动条 needed ↔
/// 折行预算（±VERTICAL_SCROLLBAR_RESERVE）不得逐帧翻转（P99 自持
/// 证明的稳态；翻转即「段落出现/消失」闪烁，用户复报）。连续 8 帧
/// 打印 needed/预算/段数，任何两帧翻转即失败。
#[test]
fn probe_wrap_sb_reserve_no_flip_at_viewport_edge() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    // 24px（lh=33）：视口 600px = 18.18 行；构造单行长文折 ~18 段
    // （内容高度贴 600 边缘），needed 判定处临界
    let scenarios: Vec<(&str, String)> = vec![
        (
            "单行长文",
            "中文长文本段落内容内容内容".repeat(60),
        ),
        (
            "两行长文",
            format!(
                "{}\n{}",
                "英文englishword".repeat(50),
                "中文段落中文段落".repeat(45)
            ),
        ),
        (
            "17短行",
            (0..17).map(|_| "short line xxxx".to_string()).collect::<Vec<_>>().join("\n"),
        ),
        (
            "19短行",
            (0..19).map(|_| "short line xxxx".to_string()).collect::<Vec<_>>().join("\n"),
        ),
        // 翻转带探索：总行数略超视口（18 行 @33px = 594 < 600）+ 长行
        // （不可见行按 1 段低估行程）+ 混合
        (
            "18行+长行尾",
            (0..17)
                .map(|_| "short line xxxx".to_string())
                .chain(std::iter::once(
                    "中文长篇尾部段落".repeat(30),
                ))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "20行+长首行",
            std::iter::once("中文首行长段落".repeat(28))
                .chain((1..20).map(|_| "short line yyyy".to_string()))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "25行混合",
            (0..25)
                .map(|i| {
                    if i % 4 == 0 {
                        "长段落英文englishword".repeat(20)
                    } else {
                        "short sh".to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ];
    for (label, doc) in &scenarios {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(doc));
            c.set_viewport_width(760.0);
            c.set_viewport_height(600.0);
            c.set_font_size(24.0);
            c.set_word_wrap(true);
            c.cursor = CursorPos { line: 0, col: 0 };
        }
        let mut history: Vec<(bool, f32, u32)> = Vec::new();
        for _ in 0..8 {
            // 完整 layout+draw（P99 的 needed 判定与 set_wrap_sb_reserve
            // 在 draw 内执行——layout 只注入度量，不触发翻转）
            let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
            let mut renderer = iced::Renderer::new(font, Pixels(24.0));
            let mut tree = Tree::empty();
            let limits =
                layout::Limits::new(Size::new(760.0, 600.0), Size::new(760.0, 600.0));
            let node = view.layout(&mut tree, &renderer, &limits);
            let node = node.translate(iced::Vector::new(0.0, 0.0));
            let lyt = Layout::new(&node);
            let mut pixels = tiny_skia::Pixmap::new(760, 640).expect("pixmap");
            pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
            let mut mask = tiny_skia::Mask::new(760, 640).expect("mask");
            let viewport_rect = Rectangle::with_size(Size::new(760.0f32, 640.0f32));
            let viewport = iced_graphics::Viewport::with_physical_size(
                Size::new(760, 640),
                1.0,
            );
            let damage = vec![viewport_rect];
            view.draw(
                &tree,
                &mut renderer,
                &Theme::Light,
                &iced::advanced::renderer::Style::default(),
                lyt,
                mouse::Cursor::Unavailable,
                &viewport_rect,
            );
            renderer
                .draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
            let c = core.borrow();
            let (reserve, mpx, segs) = (
                c.wrap_sb_reserve,
                c.wrap_max_px(),
                c.line_visual_segments(0),
            );
            history.push((reserve, mpx, segs));
            eprintln!(
                "[P116探测2] {label} 帧{}: reserve={reserve} 预算={mpx:.1} 行0段数={segs}",
                history.len()
            );
        }
        // 允许帧1→2 的一次性收敛（P99 设计：needed 判定基于上一帧
        // 行程，翻转滞后一帧）；帧2 起必须全部稳定（持续翻转 = 闪烁）
        for w in history.windows(2).skip(1) {
            assert!(
                w[0].0 == w[1].0 && (w[0].1 - w[1].1).abs() < 0.01 && w[0].2 == w[1].2,
                "边界场景 reserve/预算/段数持续翻转（{label}）：{:?} → {:?}",
                w[0],
                w[1]
            );
        }
    }
}

/// P116 窗口缩放适配（headless 像素级）：**拉窄**后折行必须立即按新
/// 预算重排（段数变多、行尾余量保持一个汉字宽）——既有 P96 测试只
/// 覆盖「拉宽」方向，用户复报「窗口缩放时不适配」。220 ASCII 字符：
/// 800 宽 3 段 → 450 宽 6 段；首帧渲染即应生效（无陈旧断点残留）。
#[test]
fn headless_wrap_reflows_after_viewport_shrink() {
    use super::super::CursorPos;
    let (w, h) = (900u32, 400u32);
    let (ex, ey, ew, eh) = (0.0f32, 0.0f32, 800.0f32, 400.0f32);
    let doc = format!("{}\n", "a".repeat(220));
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_height(eh);
        c.cursor = CursorPos { line: 0, col: 0 };
    }
    let render = |ew: f32| -> tiny_skia::Pixmap {
        core.borrow_mut().set_viewport_width(ew);
        core.borrow_mut().set_word_wrap(true);
        let mut view = EditorView { core: core.clone(), font: BODY_FONT, zoom_accum: 0.0 };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let damage = vec![viewport_rect];
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
        pixels
    };
    let _ = render(800.0);
    {
        let c = core.borrow();
        eprintln!("[P116] 800 宽段数 = {}", c.line_visual_segments(0));
    }
    let px = render(450.0);
    let segs = core.borrow().line_visual_segments(0);
    let budget = core.borrow().wrap_max_px();
    let gutter = core.borrow().gutter_width();
    eprintln!("[P116] 450 宽段数 = {segs}（预算 {budget:.1}）");
    assert!(segs >= 6, "拉窄后折行未按新预算重排（段数 {segs}，应 ≥6 段）");
    // 行尾余量 = 一个汉字宽：最右正文墨迹 ≤ 控件右缘 − 14px（文本区
    // 右缘 = 控件右缘，P95 贴边口径；预算已内收余量）
    let x0 = (ex + gutter) as u32;
    let text_right = (ex + ew) as u32;
    let mut max_x = 0u32;
    for y in 0..h {
        for x in x0..w {
            if let Some(p) = px.pixel(x, y) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                    max_x = max_x.max(x);
                }
            }
        }
    }
    eprintln!("[P116] 最右正文墨 x={max_x}（控件右缘 {text_right}）");
    assert!(max_x <= text_right, "折行墨迹越过右缘（max_x={max_x} > {text_right}）");
    assert!(
        max_x >= x0 && text_right.saturating_sub(max_x) >= 14,
        "行尾距右缘不足一个汉字宽（余 {}px）",
        text_right.saturating_sub(max_x)
    );
}

/// P116 字号上限 48 的渲染边界（headless 像素级）：大字号 + 折行 +
/// 右缘固定余量——20 汉字应折多段、全部墨迹不越文本区右缘（P42 列宽、
/// P88 墨迹盒、P96 断行均按字号线性换算，放大只观感变化）。
#[test]
fn headless_large_font_48_wrap_keeps_right_margin() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (700u32, 400u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 620.0f32, 380.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&"中".repeat(20)));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.set_font_size(48.0);
        c.set_word_wrap(true);
        c.cursor = CursorPos { line: 0, col: 0 };
    }
    let mut view = EditorView { core: core.clone(), font, zoom_accum: 0.0 };
    let mut renderer = iced::Renderer::new(font, Pixels(48.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
    let node = view.layout(&mut tree, &renderer, &limits);
    let node = node.translate(iced::Vector::new(ex, ey));
    let lyt = Layout::new(&node);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let damage = vec![viewport_rect];
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &damage, Color::WHITE);
    let segs = core.borrow().line_visual_segments(0);
    let budget = core.borrow().wrap_max_px();
    let gutter = core.borrow().gutter_width();
    eprintln!("[P116] 48px 字号：段数={segs} 预算={budget:.1}");
    assert!(segs >= 2, "48px 字号 20 汉字应折 ≥2 段，实际 {segs}");
    let x0 = (ex + gutter) as u32;
    let text_right = (ex + ew) as u32;
    let mut max_x = 0u32;
    let mut min_x = u32::MAX;
    for y in ey as u32..(ey + eh) as u32 {
        for x in x0..(ex + ew) as u32 {
            if let Some(p) = pixels.pixel(x, y) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 230 {
                    max_x = max_x.max(x);
                    min_x = min_x.min(x);
                }
            }
        }
    }
    eprintln!("[P116] 墨迹 x ∈ [{min_x}, {max_x}]（控件右缘 {text_right}）");
    assert!(min_x != u32::MAX, "48px 大字未渲染");
    assert!(max_x <= text_right, "大字号墨迹越过右缘");
    assert!(
        text_right.saturating_sub(max_x) >= 14,
        "大字号行尾距右缘不足一个汉字宽（余 {}px）",
        text_right.saturating_sub(max_x)
    );
}
