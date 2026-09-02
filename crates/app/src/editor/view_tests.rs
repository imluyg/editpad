//! 控件层无头渲染测试（P68 自 editor.rs tests 拆出；经 #[path] 挂为
//! view 子模块）。全部只走公开 API 构造状态。
use super::*;

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
        renderer.draw(&mut pixels.as_mut(), &mut mask, &viewport, &vec![viewport_rect], Color::WHITE);
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