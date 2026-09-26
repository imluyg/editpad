//! 控件层无头渲染测试（P68 自 editor.rs tests 拆出；经 #[path] 挂为
//! view 子模块）。全部只走公开 API 构造状态。
use super::*;

/// P154：行号栏字体独立下发后——换行号族**必须重测**行号字宽（否则用旧族
/// 量出的宽度画新族字形，就是 P150 的老坑「末位字形被丢」），且正文列宽
/// 不受影响（正文字体未变，不该重算）。
#[test]
fn p154_gutter_font_change_remeasures_gutter_width_only() {
    let core = EditorHandle::default();
    core.borrow_mut().set_font_size(24.0);
    // 行号族：显式具名族（本机 Win 自带，等价 GUI 里解析出的 Consolas 类）
    let gutter_font = Font {
        family: iced::font::Family::Name("Courier New"),
        ..Font::DEFAULT
    };
    core.borrow_mut().set_gutter_font(Some(gutter_font));

    let view = EditorView {
        core: core.clone(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };
    view.ensure_measured_char_width();
    {
        let c = core.borrow();
        assert_eq!(
            c.gutter_font(),
            Some(gutter_font),
            "下发的行号族应被控件层读到"
        );
        assert_eq!(
            c.metric_key,
            Some((BODY_FONT, 24.0, Some(gutter_font))),
            "度量键必须含行号字体（只换行号字体也要重测）"
        );
        let gw = c.gutter_char_w.expect("行号字宽应按行号族实测注入");
        // 行号字号 = 正文字号 × GUTTER_FONT_SCALE（19.5px）
        let expect = super::super::metrics::measure_char_width(
            gutter_font,
            c.font_size() * GUTTER_FONT_SCALE,
        )
        .expect("行号族字宽应可测");
        assert!(
            (gw - expect).abs() < 0.01,
            "行号字宽必须按**行号族**实测：注入 {gw} ≠ 实测 {expect}"
        );
    }

    // 再下发同名族：键不变 → 不重测（幂等，避免每帧空转）
    let before = core.borrow().metric_key;
    core.borrow_mut().set_gutter_font(Some(gutter_font));
    assert_eq!(
        core.borrow().metric_key,
        before,
        "同族重复下发不得作废度量键"
    );

    // 清空行号族（回落正文字体）：换键 + 字宽按正文字体重测
    core.borrow_mut().set_gutter_font(None);
    assert!(
        core.borrow().metric_key.is_none(),
        "行号族变化应作废度量键，下一帧重测"
    );
    view.ensure_measured_char_width();
    let c = core.borrow();
    assert_eq!(c.metric_key, Some((BODY_FONT, 24.0, None)), "回落正文字体");
    let body_only = super::super::metrics::measure_char_width(BODY_FONT, 24.0 * GUTTER_FONT_SCALE)
        .expect("正文字体字宽应可测");
    let gw = c.gutter_char_w.expect("应已重测");
    assert!(
        (gw - body_only).abs() < 0.01,
        "回落正文字体后行号字宽应按正文字体重测：{gw} ≠ {body_only}"
    );
}

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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
    renderer2.draw(
        &mut pixels2.as_mut(),
        &mut mask2,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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

/// P118 组字行首段滚出视口回归（headless 像素级）：软换行开态 + 长行
/// 折成多段 + 下滚让行号（首段 v0）离开视口顶 + 行尾组字——修前 B 层
/// 把组字行的重排绘制挂在段 0 上，段 0 滚出视口后循环只遇到段 ≥1、
/// 逐段 continue，该行视口内正文整体空白（用户截图：行号不可见时
/// 打字，上方文字全部消失）；修后 = 首个被迭代到的段即全量重排绘制。
/// 断言：视口顶部可见段带有正文墨迹、总墨行覆盖大半视口（修前 = 0，
/// 下划线/光标随组字尾落在视口外，正文区全白）。
#[test]
fn headless_preedit_reflow_paints_line_when_first_segment_scrolled_out() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (500u32, 260u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 460.0f32, 230.0f32);
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&"中文折行测试".repeat(90)));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        // 光标落行尾（与用户场景一致：组字发生在行尾）
        let end = c.line_display_len(0);
        c.cursor = CursorPos { line: 0, col: end };
        c.set_word_wrap(true);
        c.ensure_visible();
        assert!(c.visual_rows_total() > 3, "长行应折成多段（测试前提失效）");
        // 下滚一行：行号（首段 v0 = 0）离开视口顶——「看不到行号」态
        c.scroll_top = 1.0;
        // 滚动条禁画（黑色 thumb 会污染墨迹判据，同 P114 口径）
        c.sb_activity = None;
        // 行尾组字（等价 Preedit 事件；focused 默认真）
        assert!(c.ime_preedit("此时".to_owned()));
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
    let (gutter, lh) = {
        let c = core.borrow();
        (c.gutter_width(), c.line_height())
    };
    let mut ink_rows = 0u32;
    let mut first_ink = i32::MAX;
    for y in ey as i32..(ey + eh) as i32 {
        let mut row_ink = 0u32;
        for x in (ex + gutter) as i32..(ex + ew) as i32 {
            if let Some(p) = pixels.pixel(x as u32, y as u32) {
                let avg = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                if avg < 200 {
                    row_ink += 1;
                }
            }
        }
        if row_ink > 0 {
            ink_rows += 1;
            first_ink = first_ink.min(y);
        }
    }
    eprintln!("[P118] 墨行 {ink_rows} 首墨行 {first_ink}（ey={ey} lh={lh:.1}）");
    assert!(
        ink_rows as f32 >= 3.0 * lh,
        "组字行首段滚出视口后正文消失（修前视口内整行空白，墨行≈0）"
    );
    assert!(
        first_ink != i32::MAX && (first_ink as f32 - ey) < lh,
        "首个可见段（视口顶行）应有正文墨迹"
    );
}

/// 第 173 轮：P118 场景的**分辨力**加强版（headless 像素级）。
///
/// 为什么另立一条而不是改上面那条：`..._paints_line_when_first_segment_scrolled_out`
/// 断的是"视口内有墨 + 首墨行落在顶行"，而第 170 轮第①步实测——把组字重排的
/// 预计算短路成恒 `None` 跑全量，796 条里**只红另一条**、这条不红。原因就在
/// 那两句断言上：回退到 `pre_slot` 三段式后正文照画、首段照滚出视口 ⇒ 它分辨
/// 不出"整行重排"与"同一行右移 + 折行边界硬裁"（P115 口径）。
///
/// 本条改断**只有重排路径能给出的现象**：组字把后文挤出的那一个**新视觉段**
/// （视觉行索引 = 未组字时的段数 `seg_old`）确有正文墨迹、且**从行首左缘起排**。
/// 回退分支给不出这一行——重排关闭时 `visual_rows_total` 仍是 `seg_old`，绘制循环
/// `last_v = (first_v + rows_v + k_vis).min(total - 1)` 根本迭代不到它，而开态那
/// 一行是 `reflow_painted` 全量重排逐段画出来的（P118 的机制本身）。
///
/// 判据拿 `preedit_reflow_off` 当**同一份文档、同一次滚动的同帧 oracle**（做法同
/// 第 168 轮的 `preedit_ul_off`）；并自带夹具自证：回退帧必须在自己的旧段里有
/// 大量墨迹，否则"新行无墨"可能只是整帧啥都没画。
#[test]
fn headless_p118_reflow_shifts_tail_into_a_new_segment() {
    use super::super::wrap::pixel_breaks;
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let (w, h) = (500u32, 260u32);
    let (ex, ey, ew, eh) = (10.0f32, 10.0f32, 460.0f32, 230.0f32);
    let core = EditorHandle::default();
    // ── 夹具参数按**实测**折行预算倒推（换字体／换 DPI 时前提自证，不硬编码字符数）
    let (body, preedit, seg_old, seg_new) = {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str("中"));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        c.set_word_wrap(true);
        let budget = c.wrap_max_px();
        let probe = shape_row_xs(font, 16.0, "中中").expect("shape 失败");
        let char_w = (probe[1] - probe[0]).max(1.0);
        let per_seg = ((budget / char_w).floor() as usize).max(4);
        let n = per_seg + per_seg / 2; // 1.5 段 ⇒ 未组字时占 2 段
        let body = "中".repeat(n);
        let preedit = "时".repeat(per_seg); // ≈ 一整段 ⇒ 合成串必多出一段
        c.reset_document(editpad_core::Document::from_str(&body));
        let old = c.segments_of_line(0, &body).len();
        let s: String = body.chars().chain(preedit.chars()).collect();
        let s_xs = shape_row_xs(font, 16.0, &s).expect("shape 失败");
        let new = pixel_breaks(&s_xs, budget, &s).len();
        (body, preedit, old, new)
    };
    assert!(
        seg_old >= 2,
        "测试前提失效：未组字的长行应已折成 ≥2 段（P118 要的就是首段滚出视口），实际 {seg_old}"
    );
    assert!(
        seg_new > seg_old,
        "测试前提失效：组字后应多出一个视觉段（重排独有现象），实际 {seg_old}→{seg_new}"
    );
    let frame = |reflow_off: bool| -> tiny_skia::Pixmap {
        {
            let mut c = core.borrow_mut();
            // 顺序要紧：reset_document 会清组字与光标
            c.reset_document(editpad_core::Document::from_str(&body));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.set_word_wrap(true);
            let end = c.line_display_len(0);
            c.cursor = CursorPos { line: 0, col: end };
            c.ensure_visible();
            // 首段（行号段）滚出视口顶 = P118 场景；下滚一行即可
            c.scroll_top = 1.0;
            c.sb_activity = None; // 滚动条黑块会污染墨迹判据（P114 口径）
            assert!(c.ime_preedit(preedit.clone()));
            c.preedit_reflow_off = reflow_off;
        }
        let mut view = EditorView {
            core: core.clone(),
            font,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(font, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
        let node = view.layout(&mut tree, &renderer, &limits);
        let node = node.translate(iced::Vector::new(ex, ey));
        let lyt = Layout::new(&node);
        let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let style = iced::advanced::renderer::Style::default();
        // 预热帧：冷帧量宽失败会让整版平移（P189 的像素护栏教训）
        for _ in 0..3 {
            view.draw(
                &tree,
                &mut renderer,
                &Theme::Light,
                &style,
                lyt,
                mouse::Cursor::Unavailable,
                &rect,
            );
        }
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        pixels
    };
    let on = frame(false);
    let off = frame(true);
    let (gutter, lh) = {
        let c = core.borrow();
        (c.gutter_width(), c.line_height())
    };
    // 视觉行 v 在 [x_lo, x_hi) 内的正文墨迹数；scroll_top 恒 1.0，故
    // y = bounds.y + (v − 1)·lh（与实现同一公式）
    let band = |px: &tiny_skia::Pixmap, v: usize, x_lo: f32, x_hi: f32| -> u32 {
        let y0 = ey + (v as f32 - 1.0) * lh;
        let mut n = 0u32;
        for y in (y0 + 3.0) as i32..(y0 + lh - 2.0) as i32 {
            for x in x_lo as i32..x_hi as i32 {
                if y < 0 || x < 0 {
                    continue;
                }
                if let Some(p) = px.pixel(x as u32, y as u32) {
                    let avg = (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3;
                    if avg < 200 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let (x0, xr) = (ex + gutter, ex + ew);
    let new_row_on = band(&on, seg_old, x0, xr);
    let new_row_off = band(&off, seg_old, x0, xr);
    // 新段是**重排后的新一行**，文字必从行首左缘起排：只看左缘 4 字宽
    let char_w = core.borrow().char_width();
    let left_on = band(&on, seg_old, x0, x0 + 4.0 * char_w);
    let off_body = band(&off, 1, x0, xr) + band(&off, seg_old - 1, x0, xr);
    eprintln!(
        "[第 173 轮] 新段行 v={seg_old}：重排 {new_row_on}px / 回退 {new_row_off}px；\
         左缘 {left_on}px；回退帧旧段自证 {off_body}px（段数 {seg_old}→{seg_new}）"
    );
    assert!(
        off_body > 300,
        "夹具自证失败：回退帧自己的段里几乎没墨（{off_body}px），\
         两帧差分说明不了任何事"
    );
    assert!(
        new_row_on >= 40,
        "重排没把后文挤进新视觉段（v={seg_old} 只有 {new_row_on}px）"
    );
    assert!(
        new_row_off * 5 <= new_row_on,
        "回退分支也在同一行画出 {new_row_off}px（重排 {new_row_on}px）⇒ 判据分辨不出\
         重排与 pre_slot 三段式"
    );
    assert!(
        left_on >= 20,
        "新段文字未从行首左缘起排（左缘带只有 {left_on}px）⇒ 不是折行新段的样子"
    );
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
        let mut view = EditorView {
            core: core.clone(),
            font,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        eprintln!("[P115勘误] wrap={wrap} 空行组字区墨px={ink} (pre_w={pre_w:.1})");
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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
    eprintln!("[P115勘误] 行尾组字：rel={rel:.0} budget={budget:.1} vis={vis:.1} 墨px={ink}");
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
    // 合成串与断点（与实现同口径，动态）
    let s: String = "中"
        .repeat(8)
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
        let doc_text: String = (1..=20).map(|i| format!("第{i}行内容\n")).collect();
        c.reset_document(editpad_core::Document::from_str(&doc_text));
        c.set_viewport_width(ew);
        c.set_viewport_height(eh);
        // 平滑滚动到小数行位：首行半可见（顶部越界带）+ 末行半可见（底部）
        c.scroll_by_lines(-2.5);
    }
    let mut view = EditorView {
        core,
        font: BODY_FONT,
        zoom_accum: 0.0,
    };

    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let mut tree = Tree::empty();
    // 控件尺寸 = 600×300（limits 收紧到目标尺寸，Fill 才解析成 600×300
    // 而非整个画布——首版脚手架此处给错，"越界"多为合法绘制）
    let limits = layout::Limits::new(Size::new(ew, eh), Size::new(ew, eh));
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
            Some((x_min, y_min, x_max, y_max)) => {
                Some((x_min.min(x), y_min.min(y), x_max.max(x), y_max.max(y)))
            }
        };
    };
    for y in 0..h {
        for x in 0..w {
            let inside = x >= x0 && x < x0 + ew as u32 && y >= y0 && y < y0 + eh as u32;
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
            let inside = x >= x0 && x < x0 + ew as u32 && y >= y0 && y < y0 + eh as u32;
            if inside {
                continue;
            }
            if let Some(px) = pixels.pixel(x, y) {
                if px.red() < 200 {
                    bbox = match bbox {
                        None => Some((x, y, x, y)),
                        Some((x_min, y_min, x_max, y_max)) => {
                            Some((x_min.min(x), y_min.min(y), x_max.max(x), y_max.max(y)))
                        }
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
    let mut view = EditorView {
        core: core.clone(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };

    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
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
        let doc_text: String = (1..=40).map(|i| format!("第{i}行内容\n")).collect();
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
            .map(|y| (gx0..gx1).any(|x| frame.pixel(x, y).map(dark).unwrap_or(false)))
            .collect()
    };
    let ga = gutter_ink_rows(&frame_a);
    let gb = gutter_ink_rows(&frame_b);
    let ink_a = ga.iter().filter(|v| **v).count();
    let ink_b = gb.iter().filter(|v| **v).count();
    eprintln!("[P66] gutter 墨迹行 A={ink_a} B={ink_b}");
    let diff_rows = ga.iter().zip(&gb).filter(|(a, b)| a != b).count();
    eprintln!("[P66] gutter 行墨迹差异行数 = {diff_rows}");
    assert!(
        diff_rows >= 10,
        "两帧行号栏墨迹几乎相同？行号没有随滚动移动"
    );

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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );

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
    assert_eq!(core.metric_key, Some((BODY_FONT, 16.0, None)));
    let w = core.measured_char_w.expect("系统字体可用时实测不应失败");
    assert!(
        (0.3..=0.9).contains(&(w / core.font_size())),
        "实测列宽 {w} 超出合理倍率"
    );
    drop(core);

    // 同键二次调用去重（值稳定不抖动）
    view.ensure_measured_char_width();
    let core = view.core.borrow();
    assert_eq!(core.metric_key, Some((BODY_FONT, 16.0, None)));
    assert_eq!(core.measured_char_w, Some(w));

    // 换字号 → 换键重测：倍率保持（advance ∝ 字号）
    drop(core);
    view.core.borrow_mut().set_font_size(20.0);
    view.ensure_measured_char_width();
    let core = view.core.borrow();
    assert_eq!(core.metric_key, Some((BODY_FONT, 20.0, None)));
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
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
        let mut tree = Tree::empty();
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let t = std::time::Instant::now();
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[viewport_rect],
            Color::WHITE,
        );
        eprintln!(
            "[P69] 冷启动帧（全新 Renderer）= {:.2} ms（参考值，不设限）",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }

    // 稳态帧：同一 Renderer/Tree/视图连续滚动渲染（真实渲染循环同构）
    let mut view = EditorView {
        core: core.clone(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };
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
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
    }

    let mut samples = Vec::new();
    let mut draw_times = Vec::new();
    let mut raster_times = Vec::new();
    for k in 0..5 {
        renderer.reset(viewport_rect);
        core.borrow_mut().scroll_top = 50_100.0 + k as f32 * 7.5;
        let t = std::time::Instant::now();
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        let draw = t.elapsed();
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        draw_times
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .fold(f64::MAX, f64::min),
        raster_times
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .fold(f64::MAX, f64::min),
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
    let mut tiny_view = EditorView {
        core: tiny.clone(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };
    let mut tiny_tree = Tree::empty();
    let tiny_node = tiny_view.layout(&mut tiny_tree, &renderer, &limits);
    let tiny_lyt = Layout::new(&tiny_node);
    let mut tiny_raster = Vec::new();
    for _ in 0..5 {
        renderer.reset(viewport_rect);
        let t = std::time::Instant::now();
        tiny_view.draw(
            &tiny_tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            tiny_lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        tiny_raster.push(t.elapsed());
    }
    tiny_raster.sort();
    eprintln!(
        "[P69] 空文档同管线帧中位 = {:.2} ms（判别：接近大文档 → 结构开销主导；远小 → 字形/文档规模主导）",
        tiny_raster[2].as_secs_f64() * 1000.0
    );
}

// ---------- O-1 绘制循环的视口剔除（次数护栏 + 逐像素护栏） ----------
//
// 上方 P69 的 `<5ms` 护栏测的是**无选区、无命中**的帧，所以选区循环与命中
// 循环各自「按文档规模」跑的那条路径它一次都没挡到（体检报告 §6 原话：
// 恒绿是因为那个用例没有选区）。这里改用**次数**结算同一契约——耗时断言在
// 忙机器上假红、缓存温热时假绿，而「成本按视口规模而非文档规模」本质是与
// 机器负载无关的次数命题。

/// 与真实渲染循环同构的最小无头管线：画一帧，返回该帧 `line_text()` 的
/// 实际取串次数。视口固定在文档中部，`select_all` 后选区向两端各伸出
/// 半个文档——改前即每帧「全文档行数 × 整行取串」。
fn draw_once_and_count_line_text(lines: usize, select_all: bool, hits: usize) -> usize {
    draw_frame_and_count_line_text(lines, 0, select_all, hits, false, false)
}

/// 同款管线，多三个旋钮：`line_len` 把每行正文补到至少这么长（0 = 不补，
/// 即原夹具），`indent` 决定夹具是否带行首缩进，`guides` 决定缩进参考线开关。
/// 给「某个绘制块不许整行取串」这类次数契约用——同一份夹具只切一个开关，
/// 两帧的取串次数必须相同（该块一旦自己取串就会多出来）。
/// `line_len` 这一档是给**长行**夹具开的：整行物化 memo 按字节封顶，可见行
/// 总字节超过封顶就缓存不住，代价只在长行上显形（短行夹具看不见它）。
#[allow(clippy::too_many_arguments)]
fn draw_frame_and_count_line_text(
    lines: usize,
    line_len: usize,
    select_all: bool,
    hits: usize,
    indent: bool,
    guides: bool,
) -> usize {
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let pad = if indent { "    " } else { "" };
        let doc: String = (0..lines)
            .map(|i| {
                let head = format!("{pad}row-{i:03} abc");
                let fill = line_len.saturating_sub(head.len());
                format!("{head}{}\n", "x".repeat(fill))
            })
            .collect();

        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.indent_guides = guides;
        if select_all {
            c.select_all();
        }
        // 命中表按行均匀铺满全文档（后台扫描快照的形状，也是改前最贵的分布）
        c.find_hl = (0..hits)
            .map(|k| editpad_core::MatchPos {
                line: (k * lines / hits.max(1)).min(lines.saturating_sub(1)),
                col: 0,
                len_chars: 3,
            })
            .collect();
        // select_all 会把滚动推到文末，视口位置必须在它之后设定
        c.scroll_top = (lines / 2) as f32;
        c.take_line_text_calls();
    }
    let mut view = EditorView {
        core: core.clone(),
        font: BODY_FONT,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let node = view.layout(&mut tree, &renderer, &limits);
    let lyt = Layout::new(&node);
    let (w, h) = (700u32, 500u32);
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &[viewport_rect],
        Color::WHITE,
    );
    let n = core.borrow().take_line_text_calls();
    assert!(n > 0, "探针失效：整帧一次整行取串都没有");
    n
}

/// O-1 主护栏：Ctrl+A 大选区下，一帧的整行取串次数只随**视口行数**变，
/// 不随文档行数变（文档翻 10 倍，次数不得跟着翻）。
#[test]
fn o1_selection_draw_cost_scales_with_viewport_not_document() {
    let small = draw_once_and_count_line_text(2_000, true, 0);
    let large = draw_once_and_count_line_text(20_000, true, 0);
    eprintln!("[O-1] Ctrl+A 单帧取串次数：2 万行 {large} / 2 千行 {small}");
    // 绝对上界：视口 ~15 行，正文 + 选区 + 行号各项合计应为百次量级。
    // 改前该值是 20_000 以上（选区循环一行一次取串）。
    assert!(
        large < 500,
        "2 万行文档单帧取串 {large} 次：绘制成本仍按文档规模结算"
    );
    assert!(
        large <= small * 3 + 50,
        "文档 ×10 使单帧取串次数 ×{large} vs ×{small}：剔除未与视口挂钩"
    );
}

/// O-1 次护栏：查找命中开态同款——命中表长度不应进入帧成本。
#[test]
fn o1_find_hit_draw_cost_scales_with_viewport_not_document() {
    let small = draw_once_and_count_line_text(2_000, false, 2_000);
    let large = draw_once_and_count_line_text(20_000, false, 20_000);
    eprintln!("[O-1] 满屏命中单帧取串次数：2 万命中 {large} / 2 千命中 {small}");
    assert!(
        large < 500,
        "2 万条命中单帧取串 {large} 次：命中循环仍按命中总数结算"
    );
    assert!(
        large <= small * 3 + 50,
        "命中表 ×10 使单帧取串次数 {small} → {large}：未按视口粗筛"
    );
}

/// P278 护栏：缩进参考线一圈**不许整行取串**。同一份带行首缩进的夹具、同一
/// 个视口，只切参考线这一个开关，两帧的整行取串次数必须相同。
///
/// 改前那个块为了问「这行缩进几列」，把每个可见行 `line_text()` 物化一遍——
/// 于是「开着参考线」这个纯视觉选项让每帧的整行物化量翻倍（长行文档里是每帧
/// 几万字，而绝大多数行的答案就是 0）。参考线的**画在哪儿**由 P132 那条像素
/// 用例守（`headless_indent_guides_ink_at_tab_stops_and_toggle_off`），本条只
/// 守它的成本：换成 rope 行首字符流之后，开态相对关态应当一颗不多取。
#[test]
fn o3_indent_guides_add_no_whole_line_fetch() {
    let off = draw_frame_and_count_line_text(2_000, 0, false, 0, true, false);
    let on = draw_frame_and_count_line_text(2_000, 0, false, 0, true, true);
    eprintln!("[P278] 参考线开/关的单帧取串次数：关 {off} / 开 {on}");
    assert_eq!(
        on, off,
        "缩进参考线开态比关态多 {on} vs {off} 次整行取串：该块仍在自行取串"
    );
}

/// P283 护栏：一帧之内的**整行物化次数**不该随「需要这行文本的绘制块数」增长。
///
/// 正文、选区带、行布局 memo 各扫一遍可见行，改前每行被完整物化 3.4 遍（实测
/// 14 个可见行取 47 次）。短行无所谓；长行文档（单行日志 / 压缩文件 / 长 CSV，
/// 本仓主用例）里一行几万字符，那就是每帧多抄几百 KB 的分配 + memcpy。
/// 判据是**次数**而非耗时（同 O-1），且双向断言：下界证明这帧真的把每个可见行
/// 都取过（不是"什么都没画"的假绿），上界证明同一行不再被反复物化。
#[test]
fn p283_one_frame_materializes_each_visible_line_once() {
    let calls = draw_frame_and_count_line_text(2_000, 0, true, 0, false, false);
    eprintln!("[P283] 单帧整行物化次数：{calls}（改前 47）");
    // 视口 300px / 行高 ≈22px ⇒ 14 个可见行；每行至少取一次是"这帧画了正文"的证据。
    assert!(
        calls >= 14,
        "单帧只物化 {calls} 行，少于可见行数 14：夹具没画满，上界断言就是假绿"
    );
    assert!(
        calls <= 22,
        "单帧物化 {calls} 次：同一行仍被多个绘制块各自取串（改前 47）"
    );
}

/// P292 护栏：**长行**可见区下单帧整行物化次数不该高于短行基准。
///
/// P283 把"同一行一帧只物化一次"钉成了次数契约，但它用的夹具是短行
/// （`row-042 abc`），而那正是这条契约**看不见**失效的形状：整行 memo 按字节封顶，
/// 旧额度 256 KB 在 20000 字符 ×14 可见行＝280 KB 时就已经越过，越过的策略是
/// **拒绝入表**——于是同一行又被正文、选区带、行布局各抄一遍，长行文档静默退回
/// P283 改前那一档。实测（旧额度）：短行 17 / 2 万字符 21 / 6 万字符 **39**
/// （改前对照是 47）。额度按"本帧要画的行"放宽到 2 MB 后三档一律 17。
///
/// ⚠️ 这条守的是**次数**（与机器无关），不是耗时；也不声明 2 MB 是终点——
/// 越过新额度的形状（约 34 行 × 60000 字符）仍会退回拒绝入表，那一档留在 §2 L-13。
#[test]
fn p292_long_visible_lines_keep_the_one_fetch_per_line_contract() {
    let base = draw_frame_and_count_line_text(80, 40, true, 0, false, false);
    let long = draw_frame_and_count_line_text(80, 20_000, true, 0, false, false);
    let vlong = draw_frame_and_count_line_text(80, 60_000, true, 0, false, false);
    eprintln!("[P292] 单帧整行物化次数：短行 {base} / 2 万 {long} / 6 万 {vlong}");
    assert!(
        base >= 14,
        "夹具自检：可见行没各取过一次（{base} < 14），上界断言就是空断言"
    );
    assert_eq!(
        long, base,
        "2 万字符长行单帧物化 {long} 次，高于短行基准 {base}（旧额度下是 21 次）"
    );
    assert_eq!(
        vlong, base,
        "6 万字符长行单帧物化 {vlong} 次，高于短行基准 {base}（旧额度下是 39 次）"
    );
}

/// P285 护栏：vendor 给"零面积字形（空格）"补的**负缓存**不得改变像素。
///
/// 改前 `allocate()` 对 `glyph_size == 0` 直接 `return None` 而不入表 ⇒ 空格
/// 每帧重跑一次未缓存栅格。本夹具实测（未命中计数，非耗时）：**改前冷帧 70 /
/// 热帧 42，改后 29 / 0** —— 热帧归零即 P285 的全部主张。
///
/// 这条用例守的是什么、不守什么（都写清楚，别让它被误读成"负缓存的正确性证明"）：
/// - **守得住**：① 空 buffer 被当成可画字形交给 `PixmapRef::from_bytes` ⇒ 直接 panic
///   （探针实测：去掉 `!buffer.is_empty()` 过滤后本用例红在 `Create glyph pixel map`）；
///   ② 冷/热两帧像素分叉（同一处绘制，缓存态不同就结果不同）；
///   ③ 热帧的字形栅格未命中数必须为 0 —— 去掉负缓存即红（实测红在"热帧 42 次、
///   冷帧 70 次"，且像素那条断言先过、不红），所以这半条不是装饰，是 P285 的成本
///   契约本身。
/// - **守不住**：一个对两帧**对称生效**的过度吞并（比如把真字形也判成"没什么可画"）
///   在这里 cold==warm 且未命中仍为 0，照样绿。那一头由"空 buffer 只在
///   `get_image_uncached` 取不到或 `w×h == 0` 两种情形写入"的结构保证，加上全量
///   像素护栏（830 条）兜住 —— 真要硬钉得在 vendor 里写单测，而 vendor 不是
///   workspace 成员、CI 不跑它的测试。
///
/// 计数入口：`iced_tiny_skia::take_glyph_probe_misses()`（取走并清零本线程计数）；
/// 要看是哪个字形键在重复，用 `EDITPAD_GLYPH_PROBE=1` 打键。
///
/// 跨帧比较按 P280 的规矩钉掉两处时钟层，字体用显式族名。附带"帧不是空白"自证。
#[test]
fn p285_negative_glyph_cache_does_not_change_pixels() {
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        // 空格密集（每 6 个字符一个）+ 少量其他字形，正是负缓存覆盖的那一类
        let doc: String = (0..20).map(|i| format!("ab {i:02} cd ef\n")).collect();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.sb_activity = None;
        c.last_activity = None;
        c.blink_on = true;
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let (w, h) = (700u32, 500u32);
    let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");

    let mut frame = || {
        renderer.reset(rect);
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let mut px = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        px.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &rect,
        );
        renderer.draw(
            &mut px.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        px.data().to_vec()
    };
    iced_tiny_skia::take_glyph_probe_misses();
    let cold = frame();
    let cold_misses = iced_tiny_skia::take_glyph_probe_misses();
    let warm = frame();
    let warm_misses = iced_tiny_skia::take_glyph_probe_misses();
    let ink = body_ink(&warm);
    assert!(ink > 200, "夹具失效：正文带墨迹仅 {ink} px，比对无从谈起");
    assert!(
        cold_misses > 0,
        "夹具失效：冷帧一次字形栅格都没发生（未命中 0），未命中计数无从比对"
    );
    assert!(
        core.borrow().line_text_ref(0).contains(' '),
        "夹具须含空格（负缓存覆盖的那一类字形）"
    );
    assert_eq!(
        cold, warm,
        "冷缓存帧与热缓存帧像素不同：负缓存吞掉了本该画的字形"
    );
    assert_eq!(
        warm_misses, 0,
        "同一份内容重画一帧仍栅格了 {warm_misses} 个字形（冷帧 {cold_misses} 个）：\
         零面积/取不到图的字形没进负缓存，每帧白跑一趟未缓存栅格"
    );
}

/// P286：横向滚动的**亚像素分箱**行为钉成护栏（第 188 轮为量 L-7 而设；量完
/// 的结论是"不改"，所以把测得的事实留在这里当回归网）。
///
/// 字形缓存键含亚像素分箱（`CacheKey.x_bin`），于是"滚动一帧要不要重栅格"
/// 完全取决于偏移落在哪个箱里。实测（本夹具 20 行 × 81 字符、600×300 视口）：
/// 整数偏移 1px / 64px **未命中 0**（整像素平移与 0 同箱 ⇒ 全命中）；亚像素
/// 偏移 0.5 / 64.25 各重栅格 **37** 次＝一整屏可见字形。⇒ 亚像素滚动确有代价，
/// 但那是"把字形画在两个整像素之间"应付的工：37 个字形掩码 ≈ 4.7k px，对比
/// 整帧 350k px 约 1%。要消除它只能把文本层原点强行取整，那会**改像素位置**
/// ⇒ 为 1% 不值得单方面动视觉口径。复测入口：`take_glyph_probe_misses()`
/// 或 `EDITPAD_GLYPH_PROBE=1` 打键。
///
/// 这条守的是"整数偏移不许产生未命中"，两个方向都可能被将来改坏：缩放系数
/// 改成非整（`transformation.scale_factor()` 一乘就把整数偏移揉成小数）、
/// 或每个字形带独立颜色（`key` 含颜色三元组 ⇒ 同字形不同色＝不同键）。
/// 亚像素那一头只断"确实发生了重栅格 + 次数有上界"，不守任何成本优化。
/// P287：关态长行里"整枚字形都落在目标像素图之外"的落笔要跳过，且**一个像素都不许变**。
///
/// 勘察数据（第 189 轮一次性仪表量出来的，仪表已撤）：普通 60 行关态一帧 28 个文本
/// 图元 / 565 次落笔、真高亮 Rust 60 行 238 图元 / 755 落笔、开态折行 15 图元 / 771 落笔
/// —— 这三种**窗口外落笔都是 0**；只有"单行 4400 字符 + 关态"这一族吃得到：
/// `scroll_left=0` 时 47/135，`scroll_left=3000` 时 **105/205（51%）**。根因是横向裁剪
/// 窗口左右各留 64 字符的 shaping 余量（P160 的口径，故意留的：跨边界的连字/字距要
/// 有上下文），那些字符**必须进 shaping**，但它们那一笔落下去注定什么都不写。
///
/// 判据故意保守：**只比目标像素图的边界**，不看 `clip_mask` —— tiny-skia 0.11 的
/// `Mask` 是视口尺寸、不带原点（掩码矩形在这一层拿不到），所以"在像素图内但被掩码
/// 裁掉"的那些落笔**不在跳过范围内**。⇒ 本用例守得住"落笔判据写宽了把该画的字裁掉"
/// （同帧拿 `set_blit_cull_for_test(false)` 当老口径逐像素对拍），**守不住**掩码那头的
/// 浪费（那一头一行未改）。
/// P288：整数对齐矩形走逐行 memset 快速路径，**必须与上游扫描线填充逐字一致**。
///
/// 为什么要这么 narrow：`adjust_clip_mask` 的"清空整张掩码 + 填一个矩形"在
/// tiny-skia 0.11 里走的是一般路径——clone 路径、按 8K 瓦片切开、每片重建 blitter
/// 再跑扫描线。实测 700×500 掩码一次 1.5ms、一帧 4 次 ⇒ 6.1ms，占合成阶段 25%。
/// 快速路径只在**四条边都是整数、宽高非负、坐标有限**时接管；非整边 / 负宽高 /
/// NaN / 超大坐标全部退回上游同一段代码，连路径都不构建。⇒ 判据边界由本用例
/// 逐格钉住（含"完全在掩码外""单像素贴边""左上越界"这些容易被差一像素的格子）。
///
/// 快速路径真的被走到了吗？由性能读数佐证而不是由本用例断言：同一帧掩码重建
/// 6110µs → 58µs（一次性仪表，仪表已撤）。本用例只管"走了以后一模一样"。
#[test]
fn p288_clip_mask_integral_rect_matches_scanline_fill() {
    let (mw, mh) = (700u32, 500u32);
    let rects: [(f32, f32, f32, f32); 12] = [
        (0.0, 0.0, 700.0, 500.0),     // 整窗
        (12.0, 30.0, 300.0, 100.0),   // 内部整数
        (0.0, 0.0, 0.0, 0.0),         // 零面积（上游：空路径直接 return）
        (350.0, 250.0, 0.0, 60.0),    // 零宽
        (-40.0, -50.0, 200.0, 120.0), // 左上越界
        (650.0, 470.0, 300.0, 200.0), // 右下越界
        (699.0, 499.0, 1.0, 1.0),     // 单像素贴右下角
        (700.0, 10.0, 50.0, 20.0),    // 完全在右外侧
        (-100.0, 10.0, 50.0, 20.0),   // 完全在左外侧
        (0.5, 0.5, 100.0, 50.0),      // 非整边 ⇒ 退回上游
        (2.5, 3.0, 40.0, 20.0),       // 只有左边非整 ⇒ 也退回
        (-0.25, 4.0, 60.0, 30.0),     // 负分数 ⇒ 退回
    ];
    for (x, y, w, h) in rects {
        let bounds = Rectangle::new(Point::new(x, y), Size::new(w, h));
        // 新：走 vendor 的快速路径分派
        let mut fast = tiny_skia::Mask::new(mw, mh).expect("mask");
        iced_tiny_skia::adjust_clip_mask(&mut fast, bounds);
        // 参照：改前那段（clear + 非抗锯齿 EvenOdd 矩形填充）
        let mut slow = tiny_skia::Mask::new(mw, mh).expect("mask");
        slow.clear();
        let path = {
            let mut b = tiny_skia::PathBuilder::new();
            b.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).unwrap());
            b.finish().unwrap()
        };
        slow.fill_path(
            &path,
            tiny_skia::FillRule::EvenOdd,
            false,
            tiny_skia::Transform::default(),
        );
        assert_eq!(
            fast.data(),
            slow.data(),
            "矩形 ({x},{y},{w}x{h}) 的快速路径与上游扫描线不一致"
        );
    }
}

#[test]
fn p287_offscreen_glyph_blits_are_culled_without_changing_pixels() {
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&"abcdefghij ".repeat(400)));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.scroll_left = 3000.0;
        c.sb_activity = None;
        c.last_activity = None;
        c.blink_on = true;
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let (w, h) = (700u32, 500u32);
    let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    // 画一帧（`cull` = 是否允许跳过窗口外落笔），返回像素与被跳过的次数
    let mut frame = |cull: bool| -> (Vec<u8>, usize) {
        iced_tiny_skia::set_blit_cull_for_test(cull);
        let _ = iced_tiny_skia::take_blit_culls();
        renderer.reset(rect);
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let mut px = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        px.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &rect,
        );
        renderer.draw(
            &mut px.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        (px.data().to_vec(), iced_tiny_skia::take_blit_culls())
    };
    let _ = frame(true); // 预热：吃掉冷缓存与 P33 的一次性钉字
    let (on_px, on_culls) = frame(true);
    let (off_px, off_culls) = frame(false);
    iced_tiny_skia::set_blit_cull_for_test(true);
    let ink = body_ink(&off_px);
    assert!(ink > 200, "夹具失效：正文带墨迹仅 {ink} px，对拍无从谈起");
    assert!(
        on_culls > 0,
        "夹具失效：一帧都没跳过窗口外落笔（{on_culls}）⇒ 下面的逐像素对拍是空转"
    );
    assert_eq!(
        off_culls, 0,
        "关掉开关后仍有 {off_culls} 次跳过 ⇒ 老口径没真的退回去"
    );
    assert_eq!(
        on_px, off_px,
        "跳过窗口外落笔改变了像素 ⇒ 判据把该画的字形也裁掉了"
    );
}

#[test]
fn p286_scroll_keeps_glyph_cache_on_integer_offsets() {
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let doc: String = (0..20)
            .map(|i| format!("abcdefgh{i:02} ijklmnop qrstuvwxyz\n"))
            .collect();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.sb_activity = None;
        c.last_activity = None;
        c.blink_on = true;
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let (w, h) = (700u32, 500u32);
    let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let mut misses_at = |sl: f32| -> usize {
        core.borrow_mut().scroll_left = sl;
        renderer.reset(rect);
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let mut px = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        px.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &rect,
        );
        renderer.draw(
            &mut px.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        iced_tiny_skia::take_glyph_probe_misses()
    };
    let _ = misses_at(0.0); // 预热：吃掉冷缓存与 P33 一次性钉字
    let repeat = misses_at(0.0);
    assert_eq!(
        repeat, 0,
        "同一偏移重画一帧仍栅格 {repeat} 个字形（应为 0）"
    );
    for int_off in [1.0f32, 64.0] {
        let m = misses_at(int_off);
        assert_eq!(
            m, 0,
            "整数偏移 {int_off}px 重栅格了 {m} 个字形：整像素平移应与 0 同箱、全命中"
        );
    }
    let back = misses_at(0.0);
    assert_eq!(
        back, 0,
        "滚回用过的偏移仍重栅格 {back} 个字形：缓存跨帧不该被清"
    );
    let sub = misses_at(0.5);
    assert!(
        sub > 0,
        "亚像素偏移 0.5px 未命中为 0 ⇒ 分箱根本没起作用，下面那条上界断言就成了空断言"
    );
    assert!(
        sub <= 60,
        "亚像素偏移一次重栅格 {sub} 个字形，远超一屏可见字形数：分箱粒度或缓存边界出了问题"
    );
}

// ---------- 关态长行的**横向**剔除（O-1 的姊妹命题，第 157 轮勘出） ----------//
// O-1 把绘制循环按视口**行**剔干净了，但一行只要有一列可见就整行进 shaping：
// 视口 600px 装得下 ~60 个字符，4000 字符的行仍完整进 cosmic-text（第 157 轮
// 实测行长 11→4000 时取串次数一动不动、帧耗时 113→861ms）。所以这里的护栏
// 不能用取串**次数**——一次取串可以有几万字符——改断言整帧进 shaping 的
// **字符数**（`EditorCore::shaped_chars` 探针，`paint_text_slice` 入口累加）。

/// 同一份长行夹具的无头管线；core / view / renderer / tree / mask **跨帧共用**，
/// 每帧只切 `h_clip_off` 这一个开关（`frame` 的入参），并把两处**时钟依赖**钉死。
///
/// 钉时钟是这条用例"偶发红"的真因（第 186 轮实测勘定，此前两次归因都不对）：
/// 见 [`LongLineFixture::frame`]。共用 fixture 是同一方向上的收口——被比较的两帧
/// 之间除了被测开关，不该再有"各自新建 core ⇒ 各自向全局 font_system 量一次字宽"
/// 这种差异（虽然本次实测它并不是红的原因）。
struct LongLineFixture {
    core: EditorHandle,
    view: EditorView,
    renderer: iced::Renderer,
    tree: Tree,
    limits: layout::Limits,
    viewport_rect: Rectangle,
    viewport: iced_graphics::Viewport,
    mask: tiny_skia::Mask,
    w: u32,
    h: u32,
}

impl LongLineFixture {
    /// 造 `lines` 行、每行 `line_len` 字符的文档，横向滚动 `scroll_left`。
    fn new(lines: usize, line_len: usize, scroll_left: f32) -> Self {
        // 显式字体名，不用 BODY_FONT：P33 的进程级钉字是**一次性全局变更**，
        // 全量并发时它可能正好落在本用例两帧之间——两帧解析到不同字形族，
        // 字符宽度一变整行文字就集体位移，比出来的差分与本改动毫无关系
        // （同族处置见 `headless_caret_and_selection_never_ink_above_first_row`）。
        let font = Font {
            family: iced::font::Family::Name("NSimSun"),
            ..iced::Font::MONOSPACE
        };
        let doc: String = (0..lines)
            .map(|i| {
                let mut s = String::with_capacity(line_len + 1);
                for k in 0..line_len {
                    // 逐字符变着写，避免整行同字被任何按游程合并的优化掩盖差异
                    s.push((b'a' + ((k + i) % 26) as u8) as char);
                }
                s.push('\n');
                s
            })
            .collect();
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(&doc));
            c.set_viewport_width(600.0);
            c.set_viewport_height(300.0);
            // reset_document 会把滚动量复位，必须在它之后设
            c.scroll_left = scroll_left;
        }
        let (w, h) = (700u32, 500u32);
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        LongLineFixture {
            core: core.clone(),
            view: EditorView {
                core,
                font,
                zoom_accum: 0.0,
            },
            renderer: iced::Renderer::new(font, Pixels(16.0)),
            tree: Tree::empty(),
            limits: layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0)),
            viewport_rect,
            viewport: iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0),
            mask: tiny_skia::Mask::new(w, h).expect("mask"),
            w,
            h,
        }
    }

    /// 画一帧并光栅化：`clip_off` = true 走**改前老口径**（整行进 shaping），
    /// 用于「剔除对像素不可见」的同文档对照。返回（整帧进 shaping 的字符数，
    /// 光栅化像素）。
    fn frame(&mut self, clip_off: bool) -> (usize, Vec<u8>) {
        {
            let mut c = self.core.borrow_mut();
            // 把两处**墙上时钟依赖**钉成常量（本仓既有像素护栏都这么做，见
            // `sb_activity = None; // 滚动条 alpha 有时钟差`）：
            // ① 滚动条淡出——`scrollbar_visibility()` 按 `sb_activity.elapsed()`
            //   算 alpha，静止 900ms 起淡、再 300ms 淡完；两帧之间只要跨过这个
            //   窗口，就一帧画得出滚动条、另一帧画不出，差异正好是一条
            //   10px 宽 × 满高的轨道（复现实测 4966 px，首差在 x=589/y=2 即右缘
            //   轨道顶）。机器越忙、单帧越慢，跨窗概率越大——这条用例红的一直
            //   是这个，不是字形度量、也不是 vendor 丢图元。
            // ② 光标活动窗——`caret_visible()` 在 `last_activity` 起 450ms 内
            //   常显、过期后按 `blink_on` 相位隐没，跨过 450ms 同样会让两帧
            //   在光标处不一致。钉成"无活动记录 + 相位为显"即与耗时无关。
            c.sb_activity = None;
            c.last_activity = None;
            c.blink_on = true;
            c.h_clip_off = clip_off;
            // 清零两个探针（取串次数不参与本批断言，清掉以免被别的计数口径误读）
            let _ = c.take_line_text_calls();
            let _ = c.take_shaped_chars();
        }
        // 夹具自检：三处时钟依赖确已钉死。将来谁把上面的赋值挪走或改条件，
        // 这里当场红——否则退化形式不是"编译不过"，而是"偶发红"，最难查的那种。
        debug_assert_eq!(
            self.core.borrow().scrollbar_visibility(),
            0.0,
            "滚动条 alpha 没被钉住：两帧比较会随机器忙闲在淡出窗两侧各画一次"
        );
        debug_assert!(
            self.core.borrow().caret_visible(),
            "光标可见性仍随 last_activity/blink_on 漂，跨帧比较不成立"
        );
        // 与真实渲染循环同款：每帧先 reset 清层栈（同族做法见稳态帧用例）
        self.renderer.reset(self.viewport_rect);
        let mut pixels = tiny_skia::Pixmap::new(self.w, self.h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let node = self
            .view
            .layout(&mut self.tree, &self.renderer, &self.limits);
        let lyt = Layout::new(&node);
        self.view.draw(
            &self.tree,
            &mut self.renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &self.viewport_rect,
        );
        let damage = vec![self.viewport_rect];
        self.renderer.draw(
            &mut pixels.as_mut(),
            &mut self.mask,
            &self.viewport,
            &damage,
            Color::WHITE,
        );
        let shaped = self.core.borrow().take_shaped_chars();
        (shaped, pixels.data().to_vec())
    }
}

/// 单帧薄封装——只要 shaping 字符数的成本用例用它；**跨帧比像素必须共用一个
/// [`LongLineFixture`]**，理由见那里。换文档内容比是比不得的：行宽是实测字形宽，
/// 尾部字符一换 `max_row_width_px` 就变，水平滚动条跟着改形。
fn draw_long_line_frame(
    lines: usize,
    line_len: usize,
    scroll_left: f32,
    clip_off: bool,
) -> (usize, Vec<u8>) {
    LongLineFixture::new(lines, line_len, scroll_left).frame(clip_off)
}

/// 关态不可见字符标记的横向剔除管线：**4 行**、每行 `line_len` 组「字母+空格」
/// （一半字符是空格 ⇒ 标记密集），横向滚动 `scroll_left` 后画一帧；
/// 返回（本帧递交绘制的标记数，光栅化像素）。`clip_off` = true 走改前老口径
/// （`ws_clip_off` 开关 ⇒ 标记一圈整行递交），当**改前算法的 oracle**。
/// 字体名与 `draw_long_line_frame` 同款显式指定，理由见那里（P33 一次性全局钉字）。
///
/// ⚠️ 行数刻意只留 4、`line_len` 也别放大：oracle 那一帧会把 `line_len` 个 quad
/// **整行**递交，第 183 轮实测大帧会扰动同进程里的其他像素用例（20 行 × 4 千组
/// 的版本让 `o3_horizontal_clipping_*` 与 `s5_caret_layer_*` 在全量并发下连着红，
/// 缩到本尺度后仍有偶发，见 §2 ⭐ 池第 ② 条）。
fn draw_ws_mark_frame(line_len: usize, scroll_left: f32, clip_off: bool) -> (usize, Vec<u8>) {
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let doc: String = (0..4)
        .map(|i| {
            let mut s = String::with_capacity(line_len * 2 + 1);
            for k in 0..line_len {
                s.push((b'a' + ((k + i) % 26) as u8) as char);
                s.push(' ');
            }
            s.push('\n');
            s
        })
        .collect();
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.show_whitespace = true;
        c.scroll_left = scroll_left;
        #[cfg(test)]
        {
            c.ws_clip_off = clip_off;
            let _ = c.take_ws_marks();
            let _ = c.take_shaped_chars();
        }
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let node = view.layout(&mut tree, &renderer, &limits);
    let lyt = Layout::new(&node);
    let (w, h) = (700u32, 500u32);
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &viewport_rect,
    );
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &[viewport_rect],
        Color::WHITE,
    );
    let marks = core.borrow().take_ws_marks();
    (marks, pixels.data().to_vec())
}

/// 两帧墨迹的不对称量：`(只在 a 有墨, 只在 b 有墨)`。
/// 断言失败时只印这两个数与包围盒级别的信息——140 万像素整片进 `assert_eq!`
/// 的消息会把工具输出撑到十几 MB。
fn ws_ink_asymmetry(a: &[u8], b: &[u8]) -> (u32, u32) {
    let ink = |px: &[u8], i: usize| px[i] < 250 || px[i + 1] < 250 || px[i + 2] < 250;
    let (mut only_a, mut only_b) = (0u32, 0u32);
    for i in (0..a.len().min(b.len())).step_by(4) {
        match (ink(a, i), ink(b, i)) {
            (true, false) => only_a += 1,
            (false, true) => only_b += 1,
            _ => {}
        }
    }
    (only_a, only_b)
}

/// ⑧（第 185 轮）：组字期间 IME 候选框的锚点必须落在**这一帧画出来的光标**上，
/// 而不是提交态位置（改前差一整个拼音串的宽，重排时还高一段）。
///
/// 判据不碰像素（跨帧像素比较在本池子里有时序问题，见 §2 ⭐）：绘制点记下的锚点
/// 与提交态几何的差，必须明显是"组字把它推前了"的量级；另有一格反向对照——
/// **非组字帧必须不记锚点**，否则提交态也会被上一帧替代。
#[test]
fn p276_ime_anchor_follows_the_painted_caret_while_composing() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str("abcdefgh\nsecond line\n"));
        c.set_viewport_width(600.0);
        c.set_viewport_height(200.0);
        c.cursor = CursorPos { line: 0, col: 0 };
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 200.0), Size::new(600.0, 200.0));
    let node = view.layout(&mut tree, &renderer, &limits);
    let lyt = Layout::new(&node);
    let (w, h) = (700u32, 300u32);
    let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    let mut frame = |view: &mut EditorView| {
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[viewport_rect],
            Color::WHITE,
        );
    };

    // ① 反向对照：非组字帧的锚点矩形恒等于提交态几何
    frame(&mut view);
    {
        let c = core.borrow();
        assert_eq!(
            c.ime_anchor_rect(),
            c.caret_rect_relative(),
            "非组字帧不该被上一帧的锚点替代"
        );
    }

    // ② 组字帧：锚点须跟着画出来的光标（右移一整个拼音串的可显示宽）
    let bare = {
        let mut c = core.borrow_mut();
        assert!(
            c.ime_preedit("zhongguo".to_owned()),
            "夹具：preedit 应被接受"
        );
        c.caret_rect_relative()
    };
    frame(&mut view);
    let anchor = core.borrow().ime_anchor_rect();
    let dx = anchor.x - bare.x;
    assert!(
        dx > 24.0 && dx < 200.0,
        "IME 锚点应比提交态右移一整个拼音串宽（8 字符），实际 dx={dx:.1}\
         （提交态 x={:.1}、锚点 x={:.1}）",
        bare.x,
        anchor.x
    );
    assert!(
        (anchor.y - bare.y).abs() < 0.5,
        "关态没有重排，y 不该动：{:.1} vs {:.1}",
        anchor.y,
        bare.y
    );

    // ③ 组字结束 ⇒ 锚点必须清空（不能让候选框停在旧位置）
    {
        core.borrow_mut().preedit = None;
    }
    frame(&mut view);
    {
        let c = core.borrow();
        assert_eq!(
            c.ime_anchor_rect(),
            c.caret_rect_relative(),
            "组字结束后必须回到精确的提交态几何，不能让候选框停在旧位置"
        );
    }
}

/// 关态不可见字符标记的横向剔除契约（第 183 轮）：**只断次数，不断像素**。
///
/// 递交绘制的标记量必须与**视口宽**同阶、与行长脱钩（实测 4 千组「字母+空格」
/// 的行整行递交 56000 个 quad，剔除后 938 个；行长 ×5 数字一动不动）。
/// oracle = `ws_clip_off`（只切标记这一圈，不切正文的 `h_clip_window`——两个一起
/// 切就把两件事的差异糊成一条差分）。它同时充当变异探针：把本改动回退成旧算法，
/// 那条敏感性断言自己就红，不必手工短路源码。
///
/// ⚠️ 为什么这条不配像素判据（第 160 轮那套是配的）：实测「一帧递交万级图元」会
/// **偶发性**丢掉屏内内容——同一段代码连跑两次，一次两帧逐位相同、另一次开态帧比
/// 整行帧多 5440 px 墨迹（差异铺满整个视口），机制未定案（§2 ⭐ 第 ① 条）。
/// 另注：第 183 轮曾把"`o3_horizontal_clipping_*` 的偶发红"归因到"本用例的大帧进池"，
/// 第 184 轮四组控制实验**推翻了那条归因**（红只需要"整池并发 + 跨帧比较"两个条件，
/// 见日档第 184 轮段）——成本判据用次数这条结论不变。
#[test]
fn p273_wrap_off_whitespace_marks_are_culled_by_viewport_width() {
    // 夹具刻意做小（4 行；曾经用 20 行 × 4 千组量到 56000 → 938，那版大帧会把
    // sibling 像素用例打红，见函数头）：只留**一帧**老口径，1 200 个 quad 已足够
    // 把下面的比例断言拉开到 4 倍以上，而单帧图元量小一个数量级。
    let near = draw_ws_mark_frame(300, 0.0, false).0;
    let far = draw_ws_mark_frame(1_500, 0.0, false).0;
    let oracle = draw_ws_mark_frame(300, 0.0, true).0;
    assert!(near > 0, "探针失效：整帧一个标记都没递交");
    assert!(
        oracle > near * 4,
        "敏感性：改前 {oracle} 个应远多于改后 {near} 个（否则分不清剔没剔）"
    );
    assert_eq!(
        near, far,
        "标记量必须与行长脱钩：300 组 {near} vs 1500 组 {far}"
    );

    // 滚到行中段：窗口跟着 scroll_left 走（不是只剔右端），量级不得膨胀
    let scrolled = draw_ws_mark_frame(1_500, 3_000.0, false).0;
    assert!(
        scrolled > 0 && scrolled <= near * 2,
        "滚动后仍须只递交窗口内的标记：行首 {near} vs 滚动后 {scrolled}"
    );
}

/// 正文带（x ≥ 120，即行号栏右侧）的墨迹像素数。行号数字全在 x < 120，
/// 所以这个计数**只可能**来自正文——它同时充当「帧不是空白」的夹具自证。
fn body_ink(px: &[u8]) -> u32 {
    let mut n = 0u32;
    for y in 0..500usize {
        for x in 120..700usize {
            let i = (y * 700 + x) * 4;
            if px[i] < 250 || px[i + 1] < 250 || px[i + 2] < 250 {
                n += 1;
            }
        }
    }
    n
}

/// 横向剔除的成本契约：整帧进 shaping 的字符数不得随行长出格。
///
/// 自带敏感性证明：同一份文档用 `h_clip_off` 再画一帧 = **改前老算法的
/// oracle**，两帧之差就是本改动的收益。改前若把这行断言删了才绿，这里
/// 会当场红（不必靠手工短路源码来验）。
#[test]
fn o3_long_line_shaping_is_capped_by_viewport_width_not_line_length() {
    let short = draw_long_line_frame(20, 11, 0.0, false).0;
    let near = draw_long_line_frame(20, 4_000, 0.0, false).0;
    let far = draw_long_line_frame(20, 20_000, 0.0, false).0;
    let oracle = draw_long_line_frame(20, 4_000, 0.0, true).0;
    eprintln!(
        "[O-3续] 关态单帧进 shaping 字符数：行长 11 {short} / 4 千 {near} / 2 万 {far}；\
         同文档改前口径 {oracle}"
    );
    assert!(short > 0, "探针失效：整帧一次 shaping 都没有");
    // 相对判据：行长 ×5 不该让 shaping 量跟着放大
    assert!(
        far <= near * 12 / 10 + 200,
        "行长 4 千→2 万使整帧 shaping 由 {near} 涨到 {far}：横向未剔除"
    );
    // 敏感性判据：改前同一份文档要付 ≈ 可视行 × 行长，剔除必须把它砍到一个
    // 视口宽的量级（实测 64 000 → 1 778，即 ~36 倍）。
    assert!(
        oracle > 40_000,
        "老口径 oracle 只有 {oracle}：探针或开关失效，下面的对比不构成证据"
    );
    assert!(
        near * 10 < oracle,
        "剔除只把 {oracle} 砍到 {near}，不足一个数量级：未按视口结算"
    );
}

/// 横向剔除的像素契约：**同一份文档**只切开关，两帧必须逐像素相同。
///
/// 这是本改动的核心等价性证据——窗口边界落在视口之外，剔掉的那段本来就
/// 不在屏上。`scroll_left = 1500` 那组尤其要紧：此时窗口左端不在 0，
/// 无 runs 分支要靠调用点传的 `dx` 把整片文字搬回正确的绝对位置，
/// `dx` 若算错，这一组会整体错位而 `scroll_left = 0` 那组照样绿。
///
/// ⚠️ 这条曾在全量并发下**偶发红**，第 186 轮勘定为两处时钟依赖所致，判据一字未改
/// （见 [`LongLineFixture::frame`] 的钉死）。同时**撤回两条早先的错误归因**：
/// ① "两帧解析到不同字形度量、整版平移"——若是平移，独有墨迹必然**两侧都有**，
///   而失败信息是 `开态独有 4966 px / 改前独有 0 px`，即一侧完全包含另一侧，
///   只能是"某一层在一帧里画了、另一帧没画"；首差像素 x=589/y=2 正落在右缘
///   10px 宽的滚动条轨道顶（`SCROLLBAR_WIDTH = 10`，10 × 满高 ≈ 4800 px）。
/// ② "vendor 在万级图元帧里偶发丢掉屏内内容"——同一条失败信息，被丢的不是屏内
///   正文而是滚动条层，且它与图元量无关（`h_clip_off` 关掉剔除、图元更多的那一帧
///   反而是少画的那侧）。池 ① 与 ② 自始至终是同一个现象。
#[test]
fn o3_horizontal_clipping_is_pixel_identical_to_full_line_paint() {
    for scroll_left in [0.0f32, 1_500.0, 12_000.0] {
        let mut fx = LongLineFixture::new(20, 4_000, scroll_left);
        // 先画一帧**丢掉**：吃掉字形链/度量的一次性初始化（第 184 轮的处置保留）。
        // 共用 fixture 之后这帧也是唯一走「memo 冷路径」的一帧，所以顺手把它的
        // shaping 量也断一下——否则改后两帧都 memo 温热，冷路径的剔除就没人看了。
        let (shaped_cold, _) = fx.frame(false);
        let (shaped_on, px_on) = fx.frame(false);
        let (shaped_off, px_off) = fx.frame(true);
        assert!(
            shaped_cold < shaped_off,
            "scroll_left={scroll_left}：首帧（memo 冷）就该只 shape {shaped_cold} 字符，\
             却比改前 {shaped_off} 不少——横向剔除没进冷路径"
        );
        assert!(
            shaped_on < shaped_off,
            "scroll_left={scroll_left}：开态 shaping {shaped_on} 不少于改前 {shaped_off}，剔除没生效"
        );
        let (only_on, only_off) = ws_ink_asymmetry(&px_on, &px_off);
        assert!(
            px_on == px_off,
            "scroll_left={scroll_left}：横向剔除改变了像素（开态 {shaped_on} 字符 / \
             改前 {shaped_off} 字符）；首个差异在第 {} 字节（开态独有 {only_on} px / \
             改前独有 {only_off} px 墨迹）",
            px_on
                .iter()
                .zip(px_off.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(0)
        );
        let ink = body_ink(&px_on);
        assert!(
            ink > 500,
            "scroll_left={scroll_left}：正文带墨迹仅 {ink} px，剔除把该显示的一段剔没了"
        );
        eprintln!(
            "[O-3续] scroll_left={scroll_left}：shaping {shaped_off} → {shaped_on} 字符，像素逐点相同，正文墨迹 {ink} px"
        );
    }
}

/// O-1 的反证护栏：**单帧判据**——剔除只许砍掉视口外的行，可见行一行都不许漏。
///
/// 一帧之内直接问「每一可视行的选区带在不在」，与渲染次数无关。
/// ⚠️ 这条原先写成「两份文档（视口内逐字符相同、视口外行数不同）成帧逐像素
/// 相等」，实测**被否证**：跨帧比较把字体测量的不确定性当成了被测对象——两次
/// 独立渲染的 `char_w` 可以不同（cosmic-text 走全局 font_system，冷热不一、
/// 缓存受压淘汰），整版平移几像素就让断言随机失败（同一份代码先后报 6048、
/// 11963 个差异像素，且加预热帧也压不住）。像素级判据只能用**同帧内**的颜色
/// 事实，不能跨帧比。
#[test]
fn o1_every_visible_line_keeps_its_selection_band() {
    let render = |total: usize| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            // 前 100 行两份完全一致；多出来的行也在视口之外（视口中部 15 行）
            let doc: String = (0..total).map(|i| format!("row-{i:03} abc\n")).collect();
            c.reset_document(editpad_core::Document::from_str(&doc));
            c.set_viewport_width(600.0);
            c.set_viewport_height(300.0);
            c.select_all();
            c.scroll_top = 50.0;
        }
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let (w, h) = (700u32, 500u32);
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[viewport_rect],
            Color::WHITE,
        );
        pixels
    };
    let px = render(500);
    // 行高是纯字号派生量（不经字体测量），在一份裸 handle 上取即可
    let handle = EditorHandle::default();
    let lh = handle.borrow().line_height();
    let rows = (300.0 / lh).floor() as usize;
    let mid = (lh / 2.0) as u32;
    // 逐可视行的选区带墨迹量。⚠️ 本编辑器只把选区带画到**行内容宽**（记事本
    // 同款，不是整幅视口宽），所以一条带的墨迹只有几十字像素，且会被带上的
    // 灰阶字形挖掉一部分——绝对阈值没有意义，判据用「行与行相当且都非零」。
    let mut inks: Vec<u32> = Vec::new();
    for row in 0..rows {
        // scroll_top 恰为整数 50 → 屏幕第 row 行即逻辑行 50+row，占据
        // y ∈ [row·lh, (row+1)·lh)，取行心扫描线
        let y = row as u32 * lh as u32 + mid;
        let mut ink = 0u32;
        for x in 0..560u32 {
            if let Some(p) = px.pixel(x, y) {
                // 选区带 = SELECTION_COLOR(0x33,0x66,0xCC,0.25) 叠白底，是画面里
                // 唯一带蓝调的东西（正文/行号/底纹全是灰阶）。按 R 与 B 的通道
                // 张量判定，与 Pixmap 的通道序无关（第 60 轮实测其为 BGRA）。
                if ((p.red() as i32) - (p.blue() as i32)).abs() >= 25 {
                    ink += 1;
                }
            }
        }
        inks.push(ink);
    }
    let max_ink = *inks.iter().max().expect("至少一行");
    assert!(max_ink > 20, "整帧找不到任何选区带（最大 {max_ink} px）");
    for (row, ink) in inks.iter().enumerate() {
        assert!(
            *ink >= max_ink / 2,
            "第 {row} 可视行选区带墨迹 {ink} px，远低于同帧最宽行 {max_ink} px：\
             该行被视口剔除吃掉了"
        );
    }
    assert!(rows >= 8, "可视行数过少（{rows}），护栏形同虚设");
}

/// 第 174 轮：补上 S-5 第 163 轮拆 `draw_selections` 时留下的**残留缺口**——
/// 附加光标（Ctrl+M `add_next_match`）的词选区上屏从来没有专门判据。
///
/// 缺口是真的：把 `draw_selections` 里收集附加光标区间那一圈 `for ec in
/// core.extra_cursors` 摘掉跑全量，797 条**一条都不红**（本轮第①步现证，读数
/// 记在日档）。既有两条选区护栏都只喂主选区（`select_all` / 单字符选区），
/// 多光标那一支结构上有代码、判据上没人管。
///
/// 判据形状：主光标**无**选区，只让两个附加光标各持一段词选区，于是
/// 「行 1／行 2 有蓝调带、行 0／行 3／行 4 没有」是**同帧内的空间归属**——
/// 蓝带不可能来自主选区（它压根不存在），也别想靠"整幅泛蓝"蒙过负向断言。
/// 不跨帧比像素（P33 全局钉字与字体测量冷热会让整版平移，见上条 `o1_...` 的
/// 被否证记录）。
#[test]
fn s5_extra_cursor_word_selection_inks_on_its_own_row() {
    use super::super::CursorPos;
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let core = EditorHandle::default();
    let lh = core.borrow().line_height();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(
            "foo 111\nfoo 222\nfoo 333\nbar 444\nbaz 555",
        ));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.set_word_wrap(false);
        c.cursor = CursorPos { line: 0, col: 1 };
        c.anchor = None; // 主光标不带选区：蓝带只能来自附加光标
        assert!(
            c.ordered_selection().is_none(),
            "测试前提失效：主光标应无选区"
        );
        assert!(
            c.add_next_match().unwrap_or(false),
            "测试前提失效：应能加到第 2 个实例"
        );
        assert!(
            c.add_next_match().unwrap_or(false),
            "测试前提失效：应能加到第 3 个实例"
        );
        assert_eq!(c.extra_cursors.len(), 2, "应有两个附加光标");
        for ec in c.extra_cursors.iter() {
            assert!(ec.anchor.is_some(), "附加光标应各持一段词选区");
        }
        c.sb_activity = None; // 滚动条黑块会污染墨迹判据（P114 口径）
    }
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
    let mut renderer = iced::Renderer::new(font, Pixels(16.0));
    let mut tree = Tree::empty();
    let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
    let node = view.layout(&mut tree, &renderer, &limits);
    let lyt = Layout::new(&node);
    let (w, h) = (700u32, 500u32);
    let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
    let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
    let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
    let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
    view.draw(
        &tree,
        &mut renderer,
        &Theme::Light,
        &iced::advanced::renderer::Style::default(),
        lyt,
        mouse::Cursor::Unavailable,
        &rect,
    );
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &[rect],
        Color::WHITE,
    );
    // 逐逻辑行量蓝调像素。SELECTION_COLOR(0x33,0x66,0xCC,0.25) 叠白底是本画面
    // 里唯一带蓝调的东西（正文/行号/底纹全灰阶）；|R−B| 张量判法与 Pixmap 的
    // BGRA 通道序无关（第 60 轮实测）
    let blue = |line: usize| -> u32 {
        let y0 = (line as f32 * lh) as i32;
        let y1 = ((line + 1) as f32 * lh) as i32;
        let mut n = 0u32;
        for y in y0..y1 {
            for x in 0..560i32 {
                if let Some(p) = pixels.pixel(x as u32, y as u32) {
                    if ((p.red() as i32) - (p.blue() as i32)).abs() >= 25 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let (b0, b1, b2, b3, b4) = (blue(0), blue(1), blue(2), blue(3), blue(4));
    eprintln!("[第 174 轮] 蓝调墨迹：主光标行 {b0} / 附加光标行 {b1},{b2} / 无光标行 {b3},{b4}");
    assert!(
        b1 >= 20 && b2 >= 20,
        "附加光标的词选区没上屏（行 1={b1}px、行 2={b2}px）⇒ `draw_selections` 里\
         extra_cursors 那一圈无人看守（本用例的存在理由）"
    );
    assert!(
        b0 <= 1,
        "主光标不该有选区，却在行 0 量到 {b0}px 蓝调 ⇒ 夹具被改坏，归属不再唯一"
    );
    assert!(
        b3 <= 1 && b4 <= 1,
        "无光标的行出现蓝调（{b3}/{b4}px）⇒ 判据把别的东西当成了选区带"
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
    assert!(
        diff >= 12,
        "开/关两帧差异墨迹不足（仅 {diff}px），标记未画出"
    );
}

/// P268（headless）：**组字串以一个"发不出字形"的字符结尾时不得越界 panic**。
///
/// 病灶：`compute_reflow` 只挡 `s_xs.len() < 2`，而三个读者都按
/// "合成流字符数"那个位置去索引（`s_xs[(col_p+pel).min(chars)]` 定光标、
/// `s_xs[ul_lo]/s_xs[ul_hi]` 画组字下划线）。`shape_row_xs` 的表长是
/// "最后一个**有字形**的字符 + 2"，所以行尾字符发不出字形时表**短一格**，
/// 那个索引直接越界 ⇒ 输入法组字过程中整个编辑器崩掉。
/// 短表不是假想：本轮实测 U+200D（零宽连接符）、U+0301（组合符）、
/// U+FE0F（变体选符）、U+1D165（音乐符号）四种结尾都产出 `len == chars`
/// ——字体栈没覆盖到的码位不留条目（emoji 🚀 反而正常，因为有 fallback）。
///
/// 判据两半：①渲染不 panic（本用例能跑完就是主断言）；②**回退路径仍然把
/// 组字画出来了**（墨迹严格多于无组字基线），防"修成什么都不画"的假绿。
#[test]
fn headless_reflow_survives_preedit_ending_in_an_unshaped_char() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);

    let render = |preedit: &str| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str("abc"));
            c.set_viewport_width(360.0);
            c.set_viewport_height(260.0);
            c.set_word_wrap(true);
            c.focused = true;
            c.cursor = CursorPos { line: 0, col: 3 };
            // 组字本身：一个有字形的 'X' + 一个发不出字形的 U+1D165
            // ⇒ 合成流以"无字形字符"结尾 ⇒ 表短一格
            if !c.ime_preedit(preedit.to_owned()) {
                panic!("ime_preedit 必须被消费（本用例前置：focused = true）");
            }
        }
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        // ← 改前这一行就 panic（index out of bounds: the length is 5 but an
        //   index of 5 was attempted）
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };
    let ink = |p: &tiny_skia::Pixmap| -> u32 {
        (0..p.height()).fold(0u32, |acc, y| {
            acc + (0..p.width())
                .filter(|&x| match p.pixel(x, y) {
                    Some(v) => !(v.red() > 245 && v.green() > 245 && v.blue() > 245),
                    None => false,
                })
                .count() as u32
        })
    };

    let base = ink(&render(""));
    let unshaped = ink(&render("X\u{1D165}"));
    let shaped = ink(&render("XY"));
    eprintln!("[P268] 无组字={base} 无字形结尾={unshaped} 正常结尾={shaped}");
    assert!(base > 0, "连正文都没画出来，本用例的前置已经不成立");
    assert!(
        unshaped > base,
        "回退路径下组字部分一个像素都没画（{unshaped} <= {base}）——\
         说明 `compute_reflow` 返回 None 之后正文/组字被一起丢了，不是真修复"
    );
    // 两种结尾应当同量级（各只多出一个可见字符），防"回退把整行吞了"
    assert!(
        unshaped * 2 >= shaped,
        "无字形结尾 ({unshaped}) 比正常结尾 ({shaped}) 墨迹少一半以上，回退画少了"
    );
}

/// P267（headless 像素级）：**空格标记必须落在它自己那一格里**。
///
/// 病灶：`draw_invisibles` 的两份循环用**显示列**去查按**字符**索引的
/// `row_x`（`xs[col.min(last)]`）。CJK 每字进 2 列、制表符跳到制表位 ⇒
/// 只要行内出现过宽字符，其后的每个标记都一路右偏，最远被钳到行末——
/// 用户看到的就是"点画在行尾、空格底下什么都没有"。
/// 既有那条用例的夹具是 `a b\tc`：Tab 恰好落在制表位上，显示列 == 字符序，
/// 所以这个偏移**从来没有被暴露**（同一条用例本轮只数差异像素，不看位置）。
///
/// 判据方向：开/关两帧的差异墨迹必须整体落在正文右缘**之内**。
/// 改前那个点被画到 `xs[4]`（正文最后一个字符 'x' 的右缘之外）⇒ 红；
/// 改后落在「中中」与「x」之间的空格格内 ⇒ 绿。
/// 开态与关态**各测一遍**——病灶本来就在两份副本里（本仓第 6 次"分叉只守一边"）。
#[test]
fn headless_invisible_space_mark_stays_inside_its_own_cell() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);

    let render = |wrap_on: bool, show: bool| -> tiny_skia::Pixmap {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            // 「中中 x」：空格是第 3 个字符（idx 2），显示列却是 4
            c.reset_document(editpad_core::Document::from_str("中中 x"));
            c.set_viewport_width(360.0);
            c.set_viewport_height(260.0);
            // 光标钉在行首：两帧都有 ⇒ 在差分里互相抵消，也不参与"正文右缘"
            c.cursor = CursorPos { line: 0, col: 0 };
            c.set_word_wrap(wrap_on);
            // 只开空格标记：行尾短竖标本就该画在正文右缘之外，会污染本判据
            c.set_invisibles(show, false);
        }
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    for wrap_on in [false, true] {
        let off = render(wrap_on, false);
        let on = render(wrap_on, true);
        // 正文右缘 = 关态里最靠右的一列非白像素（'x' 的字面右缘）
        let body_right = (0..w).rev().find(|&x| {
            (0..h).any(|y| match off.pixel(x, y) {
                Some(p) => !(p.red() > 245 && p.green() > 245 && p.blue() > 245),
                None => false,
            })
        });
        let body_right = body_right.expect("关态必须画出正文");
        // 标记墨迹的最右列：开/关两帧差分出的一切像素
        let mut mark_right = None::<u32>;
        let mut mark_px = 0u32;
        for y in 0..h {
            for x in 0..w {
                if let (Some(a), Some(b)) = (off.pixel(x, y), on.pixel(x, y)) {
                    if (a.red() as i32 - b.red() as i32).abs() > 8
                        || (a.green() as i32 - b.green() as i32).abs() > 8
                        || (a.blue() as i32 - b.blue() as i32).abs() > 8
                    {
                        mark_px += 1;
                        mark_right = Some(mark_right.map_or(x, |m: u32| m.max(x)));
                    }
                }
            }
        }
        let mark_right = mark_right.unwrap_or_else(|| {
            panic!("wrap_on={wrap_on}：开/关两帧毫无差异，标记根本没画出来，本判据是空转")
        });
        eprintln!(
            "[P267] wrap_on={wrap_on} 正文右缘 x={body_right}，标记最右 x={mark_right}，差异 {mark_px}px"
        );
        assert!(
            mark_right <= body_right,
            "wrap_on={wrap_on}：空格标记画到了 x={mark_right}，已在正文右缘 x={body_right} 之外\
             ——它在按显示列查按字符索引的 x 表（P267 的病灶）"
        );
        // 再钉一格方向性：标记必须明显偏左（落在「中中」与「x」之间），
        // 而不是"刚好贴着右缘"——防判据被一次整体位移糊过去
        assert!(
            (body_right as i32 - mark_right as i32) >= 4,
            "wrap_on={wrap_on}：标记最右 x={mark_right} 离正文右缘 x={body_right} 太近，\
             不像落在自己那格里"
        );
    }
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
                    let inside = x >= ex as u32
                        && x < (ex + ew) as u32
                        && y >= ey as u32
                        && y < (ey + eh) as u32;
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
    assert_eq!(
        band_ink(&off, 1, lh, gutter),
        0,
        "关态首行下方不应有正文墨迹"
    );
    // 开态：折行段出现在视觉行 1、2（80 字符 ≈ 3 段）；段 3 起始
    // x 必须从文本区左缘起（左缘 ~= gutter 处有墨迹）
    assert!(
        band_ink(&on, 1, lh, gutter) > 100,
        "开态视觉行 1 缺折行段墨迹"
    );
    assert!(
        band_ink(&on, 2, lh, gutter) > 100,
        "开态视觉行 2 缺折行段墨迹"
    );
    // 开态越界检查：控件矩形之外零墨迹
    let mut oob = 0u32;
    for y in 0..h {
        for x in 0..w {
            let inside =
                x >= ex as u32 && x < (ex + ew) as u32 && y >= ey as u32 && y < (ey + eh) as u32;
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
    eprintln!(
        "[P93] 折行段墨迹带 1={} 带 2={} 越界={oob}",
        band_ink(&on, 1, lh, gutter),
        band_ink(&on, 2, lh, gutter)
    );
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
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    let ink_right = |px: &tiny_skia::Pixmap| -> u32 {
        let mut max_x = 0u32;
        for y in 0..h {
            for x in 0..w {
                if let Some(p) = px.pixel(x, y) {
                    if (p.red() < 245 || p.green() < 245 || p.blue() < 245) && x > max_x {
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
        assert_eq!(
            c.line_visual_segments(0),
            2,
            "1200 宽应折 2 段——断点已随宽度重算"
        );
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
    use super::super::scrollbars::VERTICAL_SCROLLBAR_RESERVE;
    use super::super::CursorPos;
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
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
                format!(
                    "中文长文本段落第{ }行{}",
                    i + 1,
                    "abcdefghijklmnopqrstuvwxyz".repeat(4)
                )
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
                let mut view = EditorView {
                    core: core.clone(),
                    font,
                    zoom_accum: 0.0,
                };
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
                renderer.draw(
                    &mut pixels.as_mut(),
                    &mut mask,
                    &viewport,
                    &damage,
                    Color::WHITE,
                );
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
        ("单行长文", "中文长文本段落内容内容内容".repeat(60)),
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
            (0..17)
                .map(|_| "short line xxxx".to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "19短行",
            (0..19)
                .map(|_| "short line xxxx".to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        // 翻转带探索：总行数略超视口（18 行 @33px = 594 < 600）+ 长行
        // （不可见行按 1 段低估行程）+ 混合
        (
            "18行+长行尾",
            (0..17)
                .map(|_| "short line xxxx".to_string())
                .chain(std::iter::once("中文长篇尾部段落".repeat(30)))
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
            let mut view = EditorView {
                core: core.clone(),
                font,
                zoom_accum: 0.0,
            };
            let mut renderer = iced::Renderer::new(font, Pixels(24.0));
            let mut tree = Tree::empty();
            let limits = layout::Limits::new(Size::new(760.0, 600.0), Size::new(760.0, 600.0));
            let node = view.layout(&mut tree, &renderer, &limits);
            let node = node.translate(iced::Vector::new(0.0, 0.0));
            let lyt = Layout::new(&node);
            let mut pixels = tiny_skia::Pixmap::new(760, 640).expect("pixmap");
            pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
            let mut mask = tiny_skia::Mask::new(760, 640).expect("mask");
            let viewport_rect = Rectangle::with_size(Size::new(760.0f32, 640.0f32));
            let viewport = iced_graphics::Viewport::with_physical_size(Size::new(760, 640), 1.0);
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
            renderer.draw(
                &mut pixels.as_mut(),
                &mut mask,
                &viewport,
                &damage,
                Color::WHITE,
            );
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
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
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
    assert!(
        segs >= 6,
        "拉窄后折行未按新预算重排（段数 {segs}，应 ≥6 段）"
    );
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
    assert!(
        max_x <= text_right,
        "折行墨迹越过右缘（max_x={max_x} > {text_right}）"
    );
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
    let mut view = EditorView {
        core: core.clone(),
        font,
        zoom_accum: 0.0,
    };
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
    renderer.draw(
        &mut pixels.as_mut(),
        &mut mask,
        &viewport,
        &damage,
        Color::WHITE,
    );
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

// ---------- P131：滚动条标记条 ----------

/// P131 纯函数：刻度 y 换算单调、钳到轨道内、首行贴轨道顶。
#[test]
fn mark_track_y_monotonic_and_clamped() {
    let sb = VScrollbar::measure(1000, 200.0, 20.0, 400.0, 0.0);
    assert!(sb.needed, "测试前提：滚动条需要出现");
    let y0 = mark_y_for_row(0, &sb);
    assert!((y0 - sb.track_y).abs() < 0.01, "首行刻度应贴轨道顶");
    let mut prev = y0;
    for row in (0..=1000u32).step_by(37) {
        let y = mark_y_for_row(row, &sb);
        assert!(y >= prev - 1e-3, "刻度 y 应随视觉行单调不减");
        assert!(y >= sb.track_y - 1e-3 && y <= sb.track_y + sb.track_h + 1e-3);
        prev = y;
    }
    // 超出行程的行钳到 ratio=1（= 滑块顶位 track_y + travel，非轨道底：
    // 口径与滑块一致，末行滚到视口顶即行程尽头）
    let y_end = mark_y_for_row(50_000, &sb);
    let travel = (sb.track_h - sb.thumb_h).max(0.0);
    assert!((y_end - (sb.track_y + travel)).abs() < 0.01);
}

/// P131 纯函数：点击解析取容差内最近者；同距命中优先；容差外拒绝。
#[test]
fn resolve_mark_click_nearest_within_tolerance_hit_wins_ties() {
    let hits = [(100.0f32, 3usize), (200.0, 7)];
    let bms = [(102.0f32, 5usize)];
    // 命中更近
    assert_eq!(
        resolve_mark_click(&hits, &bms, 100.5),
        Some(MarkTarget::Hit(3))
    );
    // 书签更近
    assert_eq!(
        resolve_mark_click(&hits, &bms, 102.0),
        Some(MarkTarget::Bookmark(5))
    );
    // 同距（两刻度中点）→ 命中优先
    assert_eq!(
        resolve_mark_click(&hits, &bms, 101.0),
        Some(MarkTarget::Hit(3))
    );
    // 容差外 → None
    assert_eq!(resolve_mark_click(&hits, &bms, 120.0), None);
    // 空表恒 None
    assert_eq!(resolve_mark_click(&[], &[], 100.0), None);
}

/// P131（headless 像素级）：命中（橙）与书签（琥珀）刻度画在竖直滚动条
/// 轨道上，y = 行的视觉行号经 mark_y_for_row 换算；同一视觉行多条命中
/// 只出一枚刻度；滚动条淡出（无活动）时不画。
/// 采样口径：刻度带中心列逐行扫描；⚠️ 无头 tiny_skia::Pixmap 的 pixel()
/// 通道序为 BGRA（第 60 轮先例）——橙 (0xE0,0x5A,0x1E) 读出
/// blue()≈224 / green()≈90 / red()≈30，琥珀 (0xE0,0x96,0x2E) 读出
/// blue()≈224 / green()≈150 / red()≈46，按 green() 分界两类。
#[test]
fn headless_find_and_bookmark_marks_ink_on_scrollbar_track() {
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    let handle = EditorHandle::default();
    let render = |core: &EditorHandle, fade_out: bool| -> tiny_skia::Pixmap {
        {
            let mut c = core.borrow_mut();
            let doc_text: String = (0..100).map(|i| format!("line {i} abc\n")).collect();
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            // 命中：行 10 两条（同视觉行 → 一枚刻度）+ 行 40 一条
            c.set_find_highlights(vec![
                editpad_core::MatchPos {
                    line: 10,
                    col: 0,
                    len_chars: 4,
                },
                editpad_core::MatchPos {
                    line: 10,
                    col: 6,
                    len_chars: 3,
                },
                editpad_core::MatchPos {
                    line: 40,
                    col: 0,
                    len_chars: 4,
                },
            ]);
            c.bookmarks.insert(3);
            c.bookmarks.insert(60);
            if fade_out {
                // 活动戳拨回远超淡出窗口（set_viewport_* 会点亮滚动条，
                // 必须在其后覆盖；新 handle 初始即点亮态，不存在
                // 「从未活动」，None 断言不可测）
                c.sb_activity =
                    Some(std::time::Instant::now() - std::time::Duration::from_secs(60));
            }
        }
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    let frame = render(&handle, false);
    // 期望 y 与 draw 同源换算：视觉行 → mark_y_for_row
    let sb = {
        let c = handle.borrow();
        VScrollbar::measure(
            c.scroll_content_lines(),
            c.viewport_h,
            c.line_height(),
            eh,
            c.scroll_top,
        )
    };
    assert!(sb.needed, "测试前提：100 行文档在 260px 视口下滚动条出现");
    let expected_y = |line: usize| -> f32 {
        let c = handle.borrow();
        ey + mark_y_for_row(c.visual_row_of(line, 0), &sb)
    };
    // 刻度带中心列逐行分类
    let mark_cx = (ex + ew - SCROLLBAR_EDGE_INSET - MARK_WIDTH / 2.0) as u32;
    let mut orange_groups: Vec<(i32, i32)> = Vec::new();
    let mut amber_groups: Vec<(i32, i32)> = Vec::new();
    for y in (ey as u32)..(ey + eh) as u32 {
        let Some(p) = frame.pixel(mark_cx, y) else {
            continue;
        };
        let (bch, gch, rch) = (p.blue(), p.green(), p.red());
        // 全不透明像素判定；抗锯齿半透边缘混白底后 green 上浮——橙 ≈90
        // 的边缘会落入 115~135 中性带之外，故意留空挡防两类互串
        let kind = if bch > 180 && (60..115).contains(&gch) && rch < 80 {
            Some(0) // 橙（命中）
        } else if bch > 180 && (135..180).contains(&gch) && rch < 80 {
            Some(1) // 琥珀（书签）
        } else {
            None
        };
        let groups = match kind {
            Some(0) => &mut orange_groups,
            Some(1) => &mut amber_groups,
            _ => continue,
        };
        match groups.last_mut() {
            Some(g) if y as i32 - g.1 <= 2 => g.1 = y as i32,
            _ => groups.push((y as i32, y as i32)),
        }
    }
    let center = |g: (i32, i32)| (g.0 + g.1) as f32 / 2.0;
    assert_eq!(
        orange_groups.len(),
        2,
        "橙色命中刻度应有 2 枚（行 10 同视觉行去重为一枚）"
    );
    for (g, line) in orange_groups.iter().zip([10usize, 40]) {
        assert!(
            (center(*g) - expected_y(line)).abs() <= 3.0,
            "命中刻度 y 偏离换算位（行 {line}）：{} vs {}",
            center(*g),
            expected_y(line)
        );
    }
    assert_eq!(amber_groups.len(), 2, "琥珀书签刻度应有 2 枚");
    for (g, line) in amber_groups.iter().zip([3usize, 60]) {
        assert!(
            (center(*g) - expected_y(line)).abs() <= 3.0,
            "书签刻度 y 偏离换算位（行 {line}）"
        );
    }
    // 淡出隐藏时不画刻度——活动戳在渲染闭包内拨回（见 fade_out 分支）
    let idle_handle = EditorHandle::default();
    let idle = render(&idle_handle, true);
    let mut idle_ink = 0u32;
    for y in (ey as u32)..(ey + eh) as u32 {
        if let Some(p) = idle.pixel(mark_cx, y) {
            if p.blue() > 180 && p.red() < 100 {
                idle_ink += 1;
            }
        }
    }
    assert_eq!(idle_ink, 0, "滚动条淡出时不应绘制任何刻度墨迹");
}

// ---------- P132：缩进参考线 + 右缘标尺 ----------

#[test]
fn leading_indent_cols_of_counts_display_columns() {
    use super::super::metrics::leading_indent_cols_of;
    let cols = |s: &str| leading_indent_cols_of(s.chars());
    assert_eq!(cols(""), 0);
    assert_eq!(cols("abc"), 0);
    assert_eq!(cols("    code"), 4);
    // Tab 补齐到制表位：1 空格 + Tab = 4 列；两个 Tab = 8 列
    assert_eq!(cols("\tcode"), TAB_STOP_COLS);
    assert_eq!(cols(" \tcode"), TAB_STOP_COLS);
    assert_eq!(cols("\t\tcode"), 2 * TAB_STOP_COLS);
    // 行中 Tab 不算缩进
    assert_eq!(cols("a\tb"), 0);
    // 宽字符属内容非缩进
    assert_eq!(cols("中文"), 0);
    // P278：绘制层喂的是 rope 的**行首之后整条字符流**（会越过行尾），
    // 故换行必须终止扫描——纯空白行不许把下一行的缩进吃进来。
    assert_eq!(cols("   \n    x"), 3, "换行须终止扫描");
    assert_eq!(cols("\t\n\t"), TAB_STOP_COLS, "换行须终止扫描");
    assert_eq!(cols("\r\n  y"), 0, "CRLF 的 \\r 亦终止");
}

/// P132（headless 像素级）：缩进参考线——8 空格缩进行的制表位 4/8 列
/// 处应有贯穿行盒高的淡竖线，关闭后同列零墨迹（正文「deep」从列 8 起，
/// 与采样列 4 无字形重叠；空白不产生墨迹，判定干净）。
#[test]
fn headless_indent_guides_ink_at_tab_stops_and_toggle_off() {
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    let handle = EditorHandle::default();
    let render = |guides: bool| -> tiny_skia::Pixmap {
        {
            let mut c = handle.borrow_mut();
            c.reset_document(editpad_core::Document::from_str("        deep\nx\n"));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.set_indent_guides(guides);
        }
        let mut view = EditorView {
            core: handle.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    // 先渲染一帧让 layout 注入实测列宽（未注入前 char_width 是固定假设，
    // draw 实际用实测值），再取度量算期望 x
    let on = render(true);
    let (gutter_w, char_w) = {
        let c = handle.borrow();
        (c.gutter_width(), c.char_width())
    };
    // 制表位 4 列的参考线 x（与 draw 同源换算）
    let gx = (ex + gutter_w + 4.0 * char_w).round() as u32;
    let line0_top = ey as u32; // 行 0 首段 y（scroll_top=0）
    let ink = |px: &tiny_skia::Pixmap, x0: u32, y0: u32, y1: u32| -> u32 {
        let mut n = 0u32;
        for y in y0..y1 {
            for x in x0.saturating_sub(1)..=(x0 + 1) {
                if let Some(p) = px.pixel(x, y) {
                    // 参考线 = 14% 前景叠白底 ≈ 219 灰；白底 255
                    if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 245 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let g_ink = ink(&on, gx, line0_top, line0_top + 22);
    eprintln!("[P132] 参考线墨迹 {g_ink}px @x={gx}");
    assert!(
        g_ink >= 30,
        "8 空格缩进行在制表位 4 列处应有贯穿行高的参考线（墨迹 {g_ink}px）"
    );
    let off = render(false);
    assert_eq!(
        ink(&off, gx, line0_top, line0_top + 22),
        0,
        "关闭参考线后该列不应有任何墨迹（空白行首无字形）"
    );
}

/// P132（headless 像素级）：右缘标尺——edge_column=8 时在列 8 处有贯穿
/// 正文区高度的 1px 竖线，0（关）后同列零墨迹（文档行都很短，无字形重叠）。
#[test]
fn headless_edge_ruler_ink_at_column_and_toggle_off() {
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    let handle = EditorHandle::default();
    let render = |col: u32| -> tiny_skia::Pixmap {
        {
            let mut c = handle.borrow_mut();
            c.reset_document(editpad_core::Document::from_str("abc\ndef\n"));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.set_edge_column(col);
        }
        let mut view = EditorView {
            core: handle.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    // 先渲染一帧注入实测列宽再取度量（同参考线测试）
    let on = render(8);
    let (gutter_w, char_w) = {
        let c = handle.borrow();
        (c.gutter_width(), c.char_width())
    };
    let rx = (ex + gutter_w + 8.0 * char_w).round() as u32;
    let ink = |px: &tiny_skia::Pixmap, x0: u32| -> u32 {
        let mut n = 0u32;
        for y in ey as u32..(ey + eh) as u32 {
            for x in x0.saturating_sub(1)..=(x0 + 1) {
                if let Some(p) = px.pixel(x, y) {
                    // 标尺 = 22% 前景叠白底 ≈ 199 灰
                    if (p.red() as i32 + p.green() as i32 + p.blue() as i32) / 3 < 245 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let r_ink = ink(&on, rx);
    eprintln!("[P132] 标尺墨迹 {r_ink}px @x={rx}");
    assert!(
        r_ink >= ((eh as u32) - 8) * 2,
        "edge_column=8 应有贯穿正文区高度的标尺线（墨迹 {r_ink}px）"
    );
    let off = render(0);
    assert_eq!(
        ink(&off, rx),
        0,
        "标尺关闭后该列不应有任何墨迹（短行无字形）"
    );
}

// ---------- P133：链接悬停下划线（headless 像素级） ----------

/// 悬停 token 下方应有 1.5px 下划线（BRACKET_LIGHT 同族蓝，BGRA 判色
/// 先例：red()=我们的 B 通道 ≈204）；清空悬停后同区域零蓝墨。span 字符
/// 区间直接注入 link_hover（探测逻辑已由 links 纯函数测试覆盖）。
#[test]
fn headless_link_hover_underline_ink_under_token() {
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    let handle = EditorHandle::default();
    let render = |hover: Option<(usize, usize, usize)>| -> tiny_skia::Pixmap {
        {
            let mut c = handle.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(
                "go to https://x.io/a ok\n",
            ));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.link_hover = hover;
        }
        let mut view = EditorView {
            core: handle.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
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
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &damage,
            Color::WHITE,
        );
        pixels
    };

    // 首帧热身注入实测列宽（P132 教训），再取 px_of 换算 token 的 x 区间
    let _ = render(None);
    let text = "go to https://x.io/a ok";
    let (c0, c1) = (6usize, 19usize); // "https://x.io/a"
    let (gutter_w, x0, x1) = {
        let c = handle.borrow();
        (c.gutter_width(), c.px_of(0, text, c0), c.px_of(0, text, c1))
    };
    assert!(x1 > x0, "测试前提：URL 区间有像素宽度");
    let blue_ink = |px: &tiny_skia::Pixmap| -> u32 {
        let mut n = 0u32;
        // 下划线 y = 行盒底 − 3px（lh=22）±3 容差；x 与 draw 同源
        //（正文原点 = 控件左缘 + 行号栏宽）
        for y in (ey as u32 + 16)..=(ey as u32 + 24) {
            for x in
                (ex as u32 + gutter_w as u32 + x0 as u32)..(ex as u32 + gutter_w as u32 + x1 as u32)
            {
                if let Some(p) = px.pixel(x, y) {
                    if p.red() > 180 && (80..140).contains(&p.green()) && p.blue() < 120 {
                        n += 1;
                    }
                }
            }
        }
        n
    };
    let on = render(Some((0, c0, c1)));
    let ink = blue_ink(&on);
    eprintln!("[P133] 链接下划线墨迹 {ink}px");
    assert!(ink >= 12, "悬停 URL 下方应有下划线墨迹（{ink}px）");
    let off = render(None);
    assert_eq!(blue_ink(&off), 0, "无悬停时该区域不应有下划线蓝墨");
}

// ---------- P150：行号末位字形被裁根治 ----------

/// 契约：行号文本盒宽度必须**严格大于**文本真实宽度。
///
/// 上游 `fill_text` 以 `bounds.width` 为排版界，盒宽 ≤ 文本宽度时末位
/// 字形会被整段丢弃（用户复现：霞鹜臻楷 GB + 24px，`10/11/12` 只画出
/// 首位）。本测试按控件层同源实测注入字宽后核对每个字号/位数的余量。
#[test]
fn p150_gutter_number_box_leaves_room_for_last_glyph() {
    use super::super::metrics::{measure_char_width, measure_text_width};
    use crate::editor::GUTTER_NUM_SLACK;
    let font = BODY_FONT;
    for size in [10.0f32, 13.0, 16.0, 20.0, 24.0, 32.0, 48.0] {
        let gsize = size * GUTTER_FONT_SCALE;
        let Some(gw) = measure_char_width(font, gsize) else {
            continue;
        };
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.set_font_size(size);
            assert!(
                c.set_gutter_char_width(gw),
                "字号 {size}：行号字宽注入应通过校验"
            );
        }
        let c = core.borrow();
        for digits in 1..=4usize {
            let real =
                measure_text_width(font, gsize, &"0".repeat(digits)).expect("文本宽度应可测");
            let box_w = c.gutter_number_box_w(digits);
            assert!(
                box_w > real,
                "字号 {size} 位数 {digits}：盒宽 {box_w:.3} 必须严格大于文本宽度 \
                 {real:.3}（相等即丢末位字形）"
            );
            // 余量恰为 GUTTER_NUM_SLACK：右对齐左缘仍按「位数 × 字宽」算
            assert!(
                (box_w - (digits as f32 * gw + GUTTER_NUM_SLACK)).abs() < 1e-3,
                "盒宽公式漂移：{box_w:.3}"
            );
        }
    }
}

/// 像素级回归：估算略偏窄（比例字体的常见情形，0.25px/字）时，两位数
/// 行号的**末位字形**仍须上墨——修前盒宽 = 位数 × 估算值 < 真实宽度，
/// 上游丢掉末位（用户截图：第 10~12 行只剩「1」）。开/关自动换行两条
/// 绘制路径同验。
#[test]
fn p150_two_digit_line_number_keeps_last_glyph() {
    for wrap in [false, true] {
        let core = EditorHandle::default();
        let (gw_real, gsize) = {
            let c = core.borrow();
            let gsize = c.font_size() * GUTTER_FONT_SCALE;
            (
                super::super::metrics::measure_char_width(BODY_FONT, gsize)
                    .expect("行号字宽应可测"),
                gsize,
            )
        };
        {
            let mut c = core.borrow_mut();
            let doc_text: String = "1\n".repeat(12);
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(600.0);
            c.set_viewport_height(600.0);
            c.set_word_wrap(wrap);
            // 钉住度量键：layout 不再重测覆盖注入值（模拟控件层注入结果）
            c.metric_key = Some((BODY_FONT, c.font_size(), None));
            assert!(c.set_gutter_char_width(gw_real - 0.25), "偏窄估算仍须合法");
            let body_w = super::super::metrics::measure_char_width(BODY_FONT, c.font_size())
                .expect("正文字宽应可测");
            assert!(c.set_measured_char_width(body_w));
        }

        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 600.0), Size::new(600.0, 600.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let (w, h) = (600u32, 600u32);
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

        let (gutter, lh) = {
            let c = core.borrow();
            (c.gutter_width(), c.line_height())
        };
        // 行号栏墨迹的水平跨度：单位数 < 1×字宽；两位数须 ≥ 1.2×字宽
        let ink_span = |line: usize| -> (u32, u32, u32) {
            let y0 = (line as f32 * lh) as u32;
            let y1 = ((line as f32 + 1.0) * lh) as u32;
            let (mut min_x, mut max_x) = (u32::MAX, 0u32);
            for x in 0..gutter as u32 {
                let has =
                    (y0..y1).any(|y| pixels.pixel(x, y).map(|p| p.red() < 200).unwrap_or(false));
                if has {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
            if min_x == u32::MAX {
                (0, 0, 0)
            } else {
                (min_x, max_x, max_x - min_x)
            }
        };
        let (_, _, span1) = ink_span(0); // 第 1 行「1」
        let (_, _, span2) = ink_span(9); // 第 10 行「10」
        eprintln!("[P150] wrap={wrap} gsize={gsize:.2} 单位数跨度={span1} 两位数跨度={span2}");
        assert!(span1 > 0, "wrap={wrap}：第 1 行行号必须有墨迹");
        assert!(
            span2 as f32 > span1 as f32 * 1.2,
            "wrap={wrap}：第 10 行行号跨度 {span2} 应明显大于单位数 {span1}\
             （末位字形被裁的典型症状）"
        );
    }
}

/// 软换行 Phase 2「查找联动」：开态下查找命中底色必须落在**该命中所属视觉段**那一行，
/// 且续行的带从段首（正文左缘）起排、不是绝对列偏移。
///
/// 这片区域的两次修复（P94 段相对绘制、P96 顺带揪出的「hit_test 续行命中恒落段首」）
/// 都改在 `segments_of_line` 与 `line_visual_base` 的口径上，而 P123 的命中高亮
/// 此前**只在关态被测过**——即"选区画对了"并不蕴含"命中底色画对了"，两条路径各自
/// 有一份拆段循环。本用例用两帧差分把底色单独钉出来。
#[test]
fn wrap_find_highlight_paints_on_the_hit_visual_segment() {
    let text: String = "abcdefghij ".repeat(18);
    let total_chars = text.chars().count();
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        c.reset_document(editpad_core::Document::from_str(&text));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.set_word_wrap(true);
        c.reconcile_wrap_index();
        c.scroll_top = 0.0;
    }
    // 夹具自证：这一行必须真的折出 ≥3 段，且记下末段的起始列
    let segs = core.borrow().line_visual_segments(0);
    assert!(
        segs >= 3,
        "夹具失效：{total_chars} 字符在 600px 预算下只折出 {segs} 段，测不到续行"
    );
    let last_seg_start = {
        let c = core.borrow();
        let t = c.line_text(0);
        let breaks = c.segments_of_line(0, &t);
        breaks.last().copied().unwrap_or(0)
    };
    assert!(
        last_seg_start > 0,
        "夹具失效：末段起始列为 {last_seg_start}，与首段无法区分"
    );

    // 画一帧（含两次前置预热帧），返回像素缓冲
    let frame = |hits: Vec<editpad_core::MatchPos>| -> Vec<u8> {
        core.borrow_mut().find_hl = hits;
        let mut view = EditorView {
            core: core.clone(),
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let (w, h) = (620u32, 340u32);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let style = iced::advanced::renderer::Style::default();
        // 前置预热帧：冷帧量宽失败会让整版平移（P189 的像素护栏教训）
        for _ in 0..3 {
            view.draw(
                &tree,
                &mut renderer,
                &Theme::Light,
                &style,
                lyt,
                mouse::Cursor::Unavailable,
                &rect,
            );
        }
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        pixels.data().to_vec()
    };

    // 与真实渲染循环同构的最小无头管线：底色差 = 两次帧像素之差（正文两帧完全相同）
    let (w, h) = (620usize, 340usize);
    let base = frame(Vec::new());
    let ink_rows = |a: &[u8], b: &[u8]| -> (Vec<usize>, usize) {
        let mut rows: Vec<usize> = Vec::new();
        let mut min_x = usize::MAX;
        for y in 0..h {
            let mut hit_row = false;
            for x in 0..w {
                let i = (y * w + x) * 4;
                if a[i..i + 4] != b[i..i + 4] {
                    hit_row = true;
                    if x < min_x {
                        min_x = x;
                    }
                }
            }
            if hit_row {
                rows.push(y);
            }
        }
        (rows, min_x)
    };
    let head = frame(vec![editpad_core::MatchPos {
        line: 0,
        col: 0,
        len_chars: 3,
    }]);
    let tail = frame(vec![editpad_core::MatchPos {
        line: 0,
        col: last_seg_start,
        len_chars: 3,
    }]);
    let (rows_head, minx_head) = ink_rows(&base, &head);
    let (rows_tail, minx_tail) = ink_rows(&base, &tail);
    assert!(
        !rows_head.is_empty() && !rows_tail.is_empty(),
        "两帧无差异＝命中底色根本没画出来（夹具或绘制路径失效）"
    );
    let lh = core.borrow().line_height();
    assert!(
        rows_head.len() as f32 <= lh * 1.9 && rows_tail.len() as f32 <= lh * 1.9,
        "底色跨了多行：首段 {} 行 / 续段 {} 行（一条命中只应染一个视觉行，行高 {lh:.1}）",
        rows_head.len(),
        rows_tail.len()
    );
    let overlap = rows_head.iter().filter(|y| rows_tail.contains(y)).count();
    assert_eq!(
        overlap, 0,
        "首段与续段的命中墨迹行重叠 ⇒ 底色没有按视觉段定位（回到按逻辑行画一条带）"
    );
    assert!(
        (minx_head as i64 - minx_tail as i64).abs() <= 3,
        "续段命中左缘 x={minx_tail} 与首段 x={minx_head} 相差 >3px ⇒ 续行仍按绝对列偏移起排"
    );
}

/// P123 命中底色的**关态**守卫（S-5 第五步的前置护栏）。
///
/// 为什么非补不可：`wrap_find_highlight_paints_on_the_hit_visual_segment` 只跑
/// `set_word_wrap(true)`，而这段绘制有**两份**拆段循环（开态逐视觉段／关态逐逻辑行）。
/// 第 162 轮按 S-5 四步法第①步实测：把整块短路成"一颗都不画"，全量 793 条里**只有
/// 开态那一条红**——关态分支此前无人看守（`o1_find_hit_draw_cost_...` 断的是次数
/// 上界，短路只会让次数更小、恒不红）。外提前先补齐，否则搬完"全绿"不构成等价证据。
#[test]
fn find_highlight_paints_only_the_hit_span_when_wrap_off() {
    let core = EditorHandle::default();
    {
        let mut c = core.borrow_mut();
        let doc: String = (0..20)
            .map(|i| format!("row-{i:02} abcdefgh ijklmnop\n"))
            .collect();
        c.reset_document(editpad_core::Document::from_str(&doc));
        c.set_viewport_width(600.0);
        c.set_viewport_height(300.0);
        c.scroll_top = 0.0;
        // 关态：软换行不开（默认即关，这里显式写出来表意）
        c.set_word_wrap(false);
    }
    let lh = core.borrow().line_height();
    let hit_line = 5usize;

    let frame = |hits: Vec<editpad_core::MatchPos>| -> Vec<u8> {
        core.borrow_mut().find_hl = hits;
        let mut view = EditorView {
            core: core.clone(),
            // 显式字体名：底色差分是跨两帧比对，P33 的一次性全局钉字若落在两帧
            // 之间会让整版平移（同文件里 s5_caret 那处的教训），钉住字形族才可比。
            font: Font {
                family: iced::font::Family::Name("NSimSun"),
                ..iced::Font::MONOSPACE
            },
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(view.font, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let (w, h) = (620u32, 340u32);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let style = iced::advanced::renderer::Style::default();
        // 前置预热帧：冷帧量宽失败会让整版平移（P189 的像素护栏教训）
        for _ in 0..3 {
            view.draw(
                &tree,
                &mut renderer,
                &Theme::Light,
                &style,
                lyt,
                mouse::Cursor::Unavailable,
                &rect,
            );
        }
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        pixels.data().to_vec()
    };

    let (w, h) = (620usize, 340usize);
    // 只扫正文区：`find_hl` 同时喂给 P131 的滚动条刻度条（那条有它自己的用例
    // `headless_find_and_bookmark_marks_ink_on_scrollbar_track` 看守），刻度标记
    // 落在控件右缘 x≈590 的窄条上、纵向按行号比例摆——不排掉它，下面的"行带/跨度"
    // 两条判据会被一个正确的东西搞红。
    const TEXT_SCAN_END: usize = 580;
    // 差分 = 「有命中」减「无命中」：正文/行号/滚动条两帧完全相同，剩下的只有底色
    let diff_box = |a: &[u8], b: &[u8]| -> (usize, usize, usize, usize, usize) {
        let (mut ymin, mut ymax) = (usize::MAX, 0usize);
        let (mut xmin, mut xmax) = (usize::MAX, 0usize);
        let mut n = 0usize;
        for y in 0..h {
            for x in 0..TEXT_SCAN_END {
                let i = (y * w + x) * 4;
                if a[i..i + 4] != b[i..i + 4] {
                    n += 1;
                    ymin = ymin.min(y);
                    ymax = ymax.max(y);
                    xmin = xmin.min(x);
                    xmax = xmax.max(x);
                }
            }
        }
        (n, ymin, ymax, xmin, xmax)
    };
    let base = frame(Vec::new());
    let hit = frame(vec![editpad_core::MatchPos {
        line: hit_line,
        col: 3,
        len_chars: 4,
    }]);
    let (n, ymin, ymax, xmin, xmax) = diff_box(&base, &hit);
    eprintln!(
        "[P123关态] 命中底色差分 {n}px，y=[{ymin}..{ymax}] x=[{xmin}..{xmax}]（行高 {lh:.1}）"
    );

    // ① 存在性 + 夹具自证：短路整块 / 底色根本没画 ⇒ 这里是 0px
    assert!(
        n > 20,
        "关态命中底色没有上屏（差分仅 {n}px）：夹具或绘制路径失效"
    );
    // ② 落在**命中那一行**：行带只能在 hit_line±1 行内（decoration_inset 会下移几 px）
    assert!(
        ymin as f32 >= (hit_line - 1) as f32 * lh && ymax as f32 <= (hit_line + 2) as f32 * lh,
        "底色落在 y=[{ymin}..{ymax}]，而命中在第 {hit_line} 行（行高 {lh:.1}）：行带错位"
    );
    // ③ 只涂命中跨度，不是整行：4 字符宽 ≈ 30px 量级，右端不越过 x=120
    assert!(
        xmax - xmin <= 120,
        "底色横向跨度 {}px：只该涂 4 个字符的命中，不该整行铺满",
        xmax - xmin
    );
    // ④ 在正文区，不在行号栏里
    assert!(xmin > 20, "底色左缘 x={xmin} 压进了行号栏");
}

/// S-5 第七步护栏（第②步补，本块此前**无人看守**）：背景 quad 与行号栏底 quad
/// 都必须真的上屏。
///
/// 为什么非补不可：第 164 轮按四步法第①步把这两枚 quad 整块短路掉跑全量，
/// **574 条全绿**——一块画满整个控件的东西居然没人管。两层原因：
/// * 既有像素用例全部**铺白底 + 白清屏**，而浅色主题的背景本来就是白
///   ⇒ 浅色下「不画背景」与「画白背景」像素相同，这一半**结构上测不到**。
///   本用例把清屏色换成品红补上（见 `render` 里那条注释）；
/// * 行号栏的 0xF2 浅灰在浅色下是可辨差异，但过去所有用例只量带上的字/圆点/
///   选区，没有一条去量「栏底本身是什么颜色」。
///
/// ⇒ 判据取**同帧内的颜色事实**（不做跨帧比对，避开 P33 钉字与量宽冷热那族
/// 不确定性），且**深浅两主题各测一半**。期望值一律现取自 `EditorColors::resolve`／
/// `theme.palette()`，不写死十六进制——配色改了用例跟着走，不会变成第二条陈旧断言。
///
/// ⚠️ 通道序按本仓实测是 **BGRA**（期望 (43,45,49) 读出来是 (49,45,43)），
/// 判等写成「正序或反序任一命中」，第 60 轮为同样的事踩过一次。
#[test]
fn headless_backdrop_paints_background_and_gutter_strip() {
    let doc = "ab\ncd\n";
    let (w, h) = (700u32, 500u32);
    // 采样区：文档只有 2 行，视口 300px 高 ⇒ y ∈ [120,280) 既无正文也无行号数字，
    // 两枚底 quad 在该区是**唯一**的着色来源（无滚动条：内容远小于一屏）。
    let (scan_y0, scan_y1) = (120usize, 280usize);

    let render = |theme: &Theme| -> (tiny_skia::Pixmap, f32) {
        let core = EditorHandle::default();
        {
            let mut c = core.borrow_mut();
            c.reset_document(editpad_core::Document::from_str(doc));
            c.set_viewport_width(600.0);
            c.set_viewport_height(300.0);
            c.scroll_top = 0.0;
        }
        let gutter_w = core.borrow().gutter_width();
        let mut view = EditorView {
            core,
            font: BODY_FONT,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(BODY_FONT, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let viewport_rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        // 光栅底铺**品红**而不是白：白底会让浅色主题「不画背景」与「画白背景」像素
        // 相同（夹具自身致盲）。品红不等于任何主题的中性底色，两主题的「没画」都可见。
        pixels.fill(tiny_skia::Color::from_rgba8(255, 0, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        view.draw(
            &tree,
            &mut renderer,
            theme,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[viewport_rect],
            // 清屏色＝品红（不是白）：iced 的 tiny-skia `draw` 会先用这个颜色清掉
            // 损伤区，铺在下面的白/品红底都会被它盖掉——所以「quad 没画」在画面上的
            // 表现是这个清屏色。用白就会让浅色主题（背景也是白）测不出东西。
            Color::from_rgb8(255, 0, 255),
        );
        (pixels, gutter_w)
    };

    // 一条扫描线上「与期望底色相符（±2 取整容差）」的像素占比
    let match_ratio = |px: &tiny_skia::Pixmap, x0: u32, x1: u32, want: (f32, f32, f32)| -> f32 {
        let (wr, wg, wb) = (want.0 * 255.0, want.1 * 255.0, want.2 * 255.0);
        let (mut hit, mut total) = (0u32, 0u32);
        for y in scan_y0..scan_y1 {
            for x in x0..x1 {
                let Some(p) = px.pixel(x, y as u32) else {
                    continue;
                };
                total += 1;
                // 通道序按本仓实测为 **BGRA**（第 60 轮就为此踩过一次）：期望
                // (43,45,49) 的深色底在这里读出来是 (49,45,43)。故按「正序或反序
                // 任一命中」判等——两序只差通道排列，颜色本身仍要逐通道对上，
                // 白/品红一类完全不同的底依旧不命中。
                let (pr, pg, pb) = (p.red() as f32, p.green() as f32, p.blue() as f32);
                let near = |a: f32, b: f32| (a - b).abs() <= 2.0;
                if (near(pr, wr) && near(pg, wg) && near(pb, wb))
                    || (near(pr, wb) && near(pg, wg) && near(pb, wr))
                {
                    hit += 1;
                }
            }
        }
        hit as f32 / total.max(1) as f32
    };
    let rgb = |c: Color| (c.r, c.g, c.b);

    // ---------- 深色主题：背景 quad ----------
    let dark = Theme::Dark;
    let dark_bg = rgb(dark.palette().background);
    // 夹具自证：深色底必须与光栅白底明显不同，否则本主题这一半是空断言
    assert!(
        dark_bg.0 < 0.9 && dark_bg.1 < 0.9 && dark_bg.2 < 0.9,
        "夹具失效：深色主题 background 为 {dark_bg:?}，与白底无法区分"
    );
    let (px_dark, gw_dark) = render(&dark);
    let body_dark = match_ratio(&px_dark, gw_dark.ceil() as u32 + 2, 560, dark_bg);
    assert!(
        body_dark > 0.9,
        "深色主题正文区只有 {:.0}% 是主题背景色：背景 quad 没上屏",
        body_dark * 100.0
    );
    // 行号栏在深色下由 palette 派生（lighten），必须与正文区**不同**，
    // 且与 resolve() 给的 gutter_bg 同色
    let dark_gutter = rgb(super::colors::EditorColors::resolve(&dark).gutter_bg);
    let strip_dark = match_ratio(&px_dark, 2, (gw_dark - 2.0).max(3.0) as u32, dark_gutter);
    assert!(
        strip_dark > 0.9,
        "深色主题行号栏只有 {:.0}% 命中 gutter_bg {dark_gutter:?}：栏底 quad 没上屏",
        strip_dark * 100.0
    );

    // ---------- 浅色主题：行号栏 quad（0xF2 浅灰，与白底可分）----------
    let light = Theme::Light;
    let light_gutter = rgb(super::colors::EditorColors::resolve(&light).gutter_bg);
    assert!(
        light_gutter.0 < 0.99,
        "夹具失效：浅色 gutter_bg {light_gutter:?} 与白底同色，这一半测不出东西"
    );
    let (px_light, gw_light) = render(&light);
    let strip_light = match_ratio(&px_light, 2, (gw_light - 2.0).max(3.0) as u32, light_gutter);
    assert!(
        strip_light > 0.9,
        "浅色主题行号栏只有 {:.0}% 命中 gutter_bg {light_gutter:?}：栏底 quad 没上屏",
        strip_light * 100.0
    );
    // 浅色主题的背景 quad：清屏色是品红，所以「白底没画上去」现在也测得出来
    // （旧写法铺白底时这一半是空断言，正是它让整块 quad 无人看守）。
    let light_body = match_ratio(
        &px_light,
        gw_light.ceil() as u32 + 2,
        560,
        rgb(light.palette().background),
    );
    assert!(
        light_body > 0.9,
        "浅色主题正文区只有 {:.0}% 是主题背景色：背景 quad 没上屏",
        light_body * 100.0
    );
    eprintln!(
        "[S-5⑦] 浅色正文背景命中 {:.0}%／浅色栏底 {:.0}%／深色正文背景 {:.0}%／深色栏底 {:.0}%（gutter_w 浅={gw_light:.1} 深={gw_dark:.1}）",
        light_body * 100.0,
        strip_light * 100.0,
        body_dark * 100.0,
        strip_dark * 100.0
    );
}

/// S-5 第十一步护栏（第②步补）：**关态（非重排）** 的输入法下划线必须真的上屏。
///
/// 为什么非补不可：C 层这块和命中底色那块一样有**两份**分支——组字重排存在时
/// 逐段画（开态），否则按原段口径画一条。第 168 轮第①步把整块短路跑全量，
/// 只红一条 `headless_preedit_reflow_wraps_tail_and_shifts_following_lines`
/// （它测的是重排那条分支）⇒ 原段口径那条分支**无人看守**。
///
/// 判据用 `preedit_ul_off` 开关当**同一份文档的"画 vs 不画"oracle**：两帧只差
/// 这一个布尔量，差分里剩下的就只有那 2px 下划线本身——不必去猜字形墨迹与
/// 下划线各自的 y 范围，也不会被组字串正文、光标竖线一起带进差分。
#[test]
fn headless_preedit_underline_inks_in_wrap_off_state() {
    use super::super::CursorPos;
    let (w, h) = (620u32, 340u32);
    let core = EditorHandle::default();
    // 显式字形族：本用例跨两帧逐像素比，P33 的一次性全局钉字若落在两帧之间
    // 会让整版平移（同文件里 s5_caret 与 O-3续 那两处的教训）。
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    let frame = |ul_off: bool| -> Vec<u8> {
        {
            let mut c = core.borrow_mut();
            // 顺序要紧：reset_document 会清组字与光标
            c.reset_document(editpad_core::Document::from_str("ab"));
            c.set_viewport_width(600.0);
            c.set_viewport_height(300.0);
            c.set_word_wrap(false);
            c.scroll_top = 0.0;
            c.cursor = CursorPos { line: 0, col: 2 };
            c.ime_preedit("xyz".to_owned());
            c.preedit_ul_off = ul_off;
        }
        let mut view = EditorView {
            core: core.clone(),
            font,
            zoom_accum: 0.0,
        };
        let mut renderer = iced::Renderer::new(font, Pixels(16.0));
        let mut tree = Tree::empty();
        let limits = layout::Limits::new(Size::new(600.0, 300.0), Size::new(600.0, 300.0));
        let node = view.layout(&mut tree, &renderer, &limits);
        let lyt = Layout::new(&node);
        let rect = Rectangle::with_size(Size::new(w as f32, h as f32));
        let viewport = iced_graphics::Viewport::with_physical_size(Size::new(w, h), 1.0);
        let mut pixels = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixels.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
        let mut mask = tiny_skia::Mask::new(w, h).expect("mask");
        let style = iced::advanced::renderer::Style::default();
        // 前置预热帧：冷帧量宽失败会让整版平移（P189 的像素护栏教训）
        for _ in 0..3 {
            view.draw(
                &tree,
                &mut renderer,
                &Theme::Light,
                &style,
                lyt,
                mouse::Cursor::Unavailable,
                &rect,
            );
        }
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[rect],
            Color::WHITE,
        );
        pixels.data().to_vec()
    };

    let on = frame(false);
    let off = frame(true);
    let (w, h) = (620usize, 340usize);
    let (mut n, mut ymin, mut ymax, mut xmin, mut xmax) =
        (0usize, usize::MAX, 0usize, usize::MAX, 0usize);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if on[i..i + 4] != off[i..i + 4] {
                n += 1;
                ymin = ymin.min(y);
                ymax = ymax.max(y);
                xmin = xmin.min(x);
                xmax = xmax.max(x);
            }
        }
    }
    let lh = core.borrow().line_height();
    eprintln!(
        "[S-5⑪] 关态预编辑下划线差分 {n}px，y=[{ymin}..{ymax}] x=[{xmin}..{xmax}]（行高 {lh:.1}）"
    );

    // ① 存在性：摘掉下划线就没差分 ⇒ 关态分支根本没画
    assert!(n > 10, "关态预编辑下划线没有上屏（差分仅 {n}px）");
    // ② 形状：2px 横线 ⇒ 纵向只该跨几行，跨多了就是别的东西在动
    assert!(
        ymax - ymin <= 4,
        "差分纵向跨了 {}px（y=[{ymin}..{ymax}]），不像一条 2px 下划线",
        ymax - ymin
    );
    // ③ 宽度量级：3 个字符的下划线
    assert!(
        xmax - xmin >= 10,
        "差分横向只有 {}px，太窄不像 3 字符下划线",
        xmax - xmin
    );
    // ④ 落在第 0 行的行盒里（文档只有一行，画到别处即错位）
    assert!(
        ymax as f32 <= lh * 1.5,
        "差分出现在 y={ymax}，超出首行行盒（行高 {lh:.1}）"
    );
    assert!(xmin > 20, "下划线左缘 x={xmin} 压进了行号栏");
}

/// S-5 护栏（外提光标层的前置）：**光标竖线必须真的上屏，且只占光标那一列**。
///
/// 为什么非补不可：既有像素用例覆盖了滚动条刻度、选区带、组字、折行段与书签
/// 圆点，却**没有一条断言光标墨迹的存在**（`headless_caret_and_selection_
/// never_ink_above_first_row` 只管「不越界」）。实测把外提后的 `draw_carets`
/// 调用整行摘掉，787 条全绿——那一层是无人看守的；无人看守时「全绿」不能当作
/// 外提等价的证据，所以先补判据再搬。
#[test]
fn s5_caret_layer_inks_on_the_caret_column() {
    use super::super::CursorPos;
    let (w, h) = (400u32, 300u32);
    let (ex, ey, ew, eh) = (20.0f32, 20.0f32, 360.0f32, 260.0f32);
    // 显式字体名：本用例判的是「blink 开/关两帧差分」，而 P33 的进程级钉字是
    // **一次性全局变更**——全量并发下它可能正好落在两帧之间，两帧解析到不同
    // 字形族 → 整行文字集体位移 → 差分带上一堆正文，"不像一条竖线"假红。
    let font = Font {
        family: iced::font::Family::Name("NSimSun"),
        ..iced::Font::MONOSPACE
    };
    // P281（承 P280）：core 挪到闭包外，两帧共用同一份——否则每帧新建 core 就
    // 每帧各向全局 font_system 量一次字宽，两帧量到不同值时整版平移、差分里
    // 混进一堆正文。顺带把 `sb_activity` 钉掉：`EditorCore::default()` 里它是
    // `Some(now)`，滚动条整层的 alpha 随时钟淡出，也是跨帧差分的噪声源。
    let core = EditorHandle::default();
    let render = |blink_on: bool| -> tiny_skia::Pixmap {
        {
            let mut c = core.borrow_mut();
            let doc_text: String = (0..20).map(|i| format!("line {i} abcdef\n")).collect();
            c.reset_document(editpad_core::Document::from_str(&doc_text));
            c.set_viewport_width(ew);
            c.set_viewport_height(eh);
            c.cursor = CursorPos { line: 3, col: 6 };
            c.scroll_top = 0.0;
            // 让 blink_on 单独决定可见性：活动期常显那条分支在这里必须关掉
            c.last_activity = None;
            c.sb_activity = None;
            c.blink_on = blink_on;
        }
        let mut view = EditorView {
            core: core.clone(),
            font,
            zoom_accum: 0.0,
        };
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
        view.draw(
            &tree,
            &mut renderer,
            &Theme::Light,
            &iced::advanced::renderer::Style::default(),
            lyt,
            mouse::Cursor::Unavailable,
            &viewport_rect,
        );
        renderer.draw(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[viewport_rect],
            Color::WHITE,
        );
        pixels
    };

    let off = render(false);
    let on = render(true);
    let mut xs: Vec<u32> = Vec::new();
    let mut ys: Vec<u32> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if let (Some(a), Some(b)) = (off.pixel(x, y), on.pixel(x, y)) {
                let d = (a.red() as i32 - b.red() as i32).abs()
                    + (a.green() as i32 - b.green() as i32).abs()
                    + (a.blue() as i32 - b.blue() as i32).abs();
                if d > 8 {
                    xs.push(x);
                    ys.push(y);
                }
            }
        }
    }
    let n = xs.len();
    let (xmin, xmax) = (xs.iter().min().unwrap(), xs.iter().max().unwrap());
    let (ymin, ymax) = (ys.iter().min().unwrap(), ys.iter().max().unwrap());
    eprintln!("[S-5] 光标开/关差分：{n}px，x=[{xmin}..{xmax}] y=[{ymin}..{ymax}]");

    // ① 存在性——整层摘掉时这里先红（差分应为 0 像素）
    assert!(n >= 12, "光标开/关两帧差异仅 {n}px：光标竖线没有上屏");
    // ② 形状——竖线 = x 跨度窄、y 跨度至少一行
    assert!(
        xmax - xmin <= 6,
        "光标 x 跨度 {}px，不像一条竖线",
        xmax - xmin
    );
    assert!(
        ymax - ymin >= 8,
        "光标 y 跨度 {}px，不足一行高",
        ymax - ymin
    );
    // ③ 落点——差异集中在光标所在行带内，且不越出控件矩形
    assert!(
        ymax - ymin <= 44,
        "光标差分跨 {}px 高，超出单行行带（不得跨行留墨）",
        ymax - ymin
    );
    assert!(
        *ymin >= ey as u32 && *ymax < (ey + eh) as u32,
        "光标墨迹越出控件矩形：y=[{ymin}..{ymax}]"
    );
    // ④ 夹具自证——关帧本身得有别处墨迹，否则"零差异"可能只是整帧没画东西
    let mut body_ink = 0u32;
    for y in 0..h {
        for x in 0..w {
            if let Some(p) = off.pixel(x, y) {
                if (p.red() as i32 + p.green() as i32 + p.blue() as i32) < 700 {
                    body_ink += 1;
                }
            }
        }
    }
    assert!(
        body_ink > 200,
        "关帧整幅仅 {body_ink}px 墨迹：夹具失效，差分断言无从谈起"
    );
}
