use super::*;
use super::tests::*;

    // ---------- P33 字体一致性 ----------

    #[test]
    fn cjk_mono_candidates_pick_by_priority_with_loose_family_matching() {
        // 优先级：候选表顺序决定命中（更纱黑体 > Noto > NSimSun > …）
        let installed = ["Noto Sans Mono CJK SC".to_owned(), "Sarasa Mono SC".to_owned()];
        assert_eq!(pick_cjk_mono_family(&installed), Some("Sarasa Mono SC"));

        // 大小写与空白宽松匹配（fontdb 枚举出的族名形态不保证规范）
        let odd = ["  SARASA   MONO  sc ".to_owned(), "Consolas".to_owned()];
        assert_eq!(pick_cjk_mono_family(&odd), Some("Sarasa Mono SC"));

        // Windows 自带链：无用户自装字体时落到 NSimSun（新宋体）
        let builtin = ["Microsoft YaHei UI".to_owned(), "nsimsun".to_owned()];
        assert_eq!(pick_cjk_mono_family(&builtin), Some("NSimSun"));

        // 全部未命中（非 CJK 环境/极简系统）→ None，调用方保持现状零变化
        let none = ["Consolas".to_owned(), "Arial".to_owned()];
        assert_eq!(pick_cjk_mono_family(&none), None);
    }

    #[test]
    fn cjk_candidate_table_entries_are_unique_and_exclude_simsun() {
        let mut seen: Vec<String> = Vec::new();
        for cand in CJK_MONO_CANDIDATES {
            let key = normalize_family(cand);
            assert!(!seen.contains(&key), "候选族名归一后重复：{cand}");
            seen.push(key);
        }
        // 刻意排除 SimSun（宋体）：其 ASCII 半宽非严格等宽，
        // 钉成主字体会破坏 P14 制表位/列映射假设
        assert!(CJK_MONO_CANDIDATES.iter().all(|c| *c != "SimSun"));
    }

    #[test]
    fn ui_typography_constants_contract() {
        // P36 口径：UI 字号固定、不随正文字号缩放——Ctrl+滚轮（P48 落地）
        // 与设置面板步进只调节文件内容；固定基准与 iced 默认文本尺寸
        // (16px) 持平（第 25 轮「UI 14px」记录已勘误）
        assert_eq!(UI_FONT_BASE_PX, 16.0);
        assert!((ui_font_px() - UI_FONT_BASE_PX * UI_FONT_SCALE).abs() < f32::EPSILON);
        // 字形族口径：UI 与正文共用同一换装点（P34 落地时只需改 BODY_FONT）
        assert_eq!(BODY_FONT, Font::MONOSPACE);
        // P48：缩放单步增量单一来源（设置面板步进与 Ctrl+滚轮共用）
        assert_eq!(FONT_ZOOM_STEP, 2.0);
    }

    #[test]
    fn wheel_zoom_step_counts_notches_and_accumulates_touchpad() {
        // 滚轮格：非零即一步，方向取符号；零增量不发
        assert_eq!(wheel_zoom_step(0.0, 1.0, false), (0.0, 1.0));
        assert_eq!(wheel_zoom_step(0.0, -1.0, false), (0.0, -1.0));
        assert_eq!(wheel_zoom_step(0.5, 0.0, false), (0.5, 0.0));
        // 触控板：折算行数累积，满 ±1 行发一步并清零
        //（用 2 的幂避免 f32 字面量精度噪声）
        assert_eq!(wheel_zoom_step(0.0, 0.25, true), (0.25, 0.0));
        assert_eq!(wheel_zoom_step(0.75, 0.25, true), (0.0, 1.0));
        assert_eq!(wheel_zoom_step(-0.75, -0.25, true), (0.0, -1.0));
        // 反向增量先抵消同向累积
        assert_eq!(wheel_zoom_step(0.75, -0.25, true), (0.5, 0.0));
    }

    #[test]
    fn ensure_visible_keeps_cursor_on_fully_visible_row() {
        // P59：视口高非行高整倍数时底缘部分行不绘制——光标贴底必须
        // 收敛到完整可见行，否则贴底打字时光标所在行整行不可见
        let mut c = core_with(&"l\n".repeat(50));
        c.set_viewport_height(100.0); // 100/22 = 4.54 → 完整可见 4 行
        c.scroll_top = 10.0;
        c.cursor = CursorPos { line: 14, col: 0 };
        c.ensure_visible();
        let offset = c.cursor.line as f32 - c.scroll_top;
        assert!(
            offset >= 0.0 && (offset + 1.0) * c.line_height() <= 100.0 + 0.01,
            "光标行必须完整可见：scroll_top={} offset={offset}",
            c.scroll_top
        );
        // 顶侧同理：向上跳到视口上方的行，行首对齐即完整可见
        c.cursor = CursorPos { line: 2, col: 0 };
        c.ensure_visible();
        assert_eq!(c.scroll_top, 2.0, "顶侧跳转行首对齐");
    }

    #[test]
    fn scrollbar_alpha_fades_after_idle_window() {
        // 闲置窗内（含边界）全显
        assert_eq!(scrollbar_alpha(0), 1.0);
        assert_eq!(scrollbar_alpha(SCROLLBAR_IDLE_MS), 1.0);
        // 窗后线性下降：中点 ≈ 0.5，末端归零
        let mid = scrollbar_alpha(SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS / 2);
        assert!((mid - 0.5).abs() < 0.01, "淡出中点应约 0.5，实际 {mid}");
        assert_eq!(
            scrollbar_alpha(SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS),
            0.0
        );
        // 之后恒 0（不回弹）
        assert_eq!(
            scrollbar_alpha(SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS * 10),
            0.0
        );
    }

    #[test]
    fn scrollbar_visibility_touch_fade_and_refade() {
        let mut c = core_with("line1\nline2\n");
        let t0 = std::time::Instant::now();

        // 从未活动 → 恒隐藏
        c.sb_activity = None;
        assert_eq!(c.scrollbar_visibility_at(t0), 0.0);
        assert!(!c.scrollbar_fading_at(t0));

        // 刚活动 → 全显且不在淡出态
        c.sb_activity = Some(t0 - std::time::Duration::from_millis(100));
        assert_eq!(c.scrollbar_visibility_at(t0), 1.0);
        assert!(!c.scrollbar_fading_at(t0));

        // 闲置超窗 → 淡出进行中（alpha 半程 + 动画态上报）
        c.sb_activity = Some(
            t0 - std::time::Duration::from_millis(
                (SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS / 2) as u64,
            ),
        );
        let mid = c.scrollbar_visibility_at(t0);
        assert!((mid - 0.5).abs() < 0.02, "半程 alpha 应约 0.5，实际 {mid}");
        assert!(c.scrollbar_fading_at(t0), "淡出进行中必须报告动画态");

        // 淡出完成 → 隐藏且动画态结束（应用层据此停快拍）
        c.sb_activity = Some(
            t0 - std::time::Duration::from_millis(
                (SCROLLBAR_IDLE_MS + SCROLLBAR_FADE_MS + 50) as u64,
            ),
        );
        assert_eq!(c.scrollbar_visibility_at(t0), 0.0);
        assert!(!c.scrollbar_fading_at(t0));

        // 淡出中再次活动 → 立即回全显（touch 重置计时）
        c.touch_scrollbar_activity();
        assert_eq!(c.scrollbar_visibility(), 1.0);
    }

    #[test]
    fn scrolling_and_view_shifts_light_up_scrollbar() {
        let mut c = core_with(&"x\n".repeat(200));
        c.set_viewport_height(100.0);

        // 滚轮/滚动入口点亮
        c.sb_activity = None;
        c.scroll_by_lines(3.0);
        assert!(c.scrollbar_visibility() > 0.99, "滚动必须点亮滚动条");

        // 光标跳转引发视口移动同样点亮（ensure_visible 的条件触点）
        c.sb_activity = None;
        c.apply_motion(Motion::DocEnd, false);
        assert!(c.scrollbar_visibility() > 0.99, "视口随光标移动应点亮");

        // 行内移动不改变视口 → 不点亮（打字不无谓遮挡行尾）
        c.sb_activity = None;
        c.apply_motion(Motion::Left, false);
        assert_eq!(c.scrollbar_visibility(), 0.0, "视口未移动不得点亮");

        // P54：横向滚动同样点亮（水平条与竖直条共用活动戳）
        c.scroll_by_columns(5.0);
        assert!(c.scrollbar_visibility() > 0.99, "横向滚动应点亮滚动条");
    }

    #[test]
    fn insert_advances_cursor_and_updates_lines() {
        let mut c = core_with("hello\nworld\n");
        c.cursor = CursorPos { line: 0, col: 5 };
        c.insert_str(",\neditpad");
        assert_eq!(c.doc.to_text(), "hello,\neditpad\nworld\n");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 7 });
    }

    #[test]
    fn backspace_joins_lines_and_deletes_selection() {
        let mut c = core_with("ab\ncd\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.backspace(); // 吃掉换行符
        assert_eq!(c.doc.to_text(), "abcd\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });

        c.reset_document(Document::from_str("abcd"));
        c.anchor = Some(CursorPos { line: 0, col: 1 });
        c.cursor = CursorPos { line: 0, col: 3 };
        c.backspace(); // 删除选区 "bc"
        assert_eq!(c.doc.to_text(), "ad");
        assert_eq!(c.anchor, None);
    }

    #[test]
    fn delete_forward_removes_next_char() {
        let mut c = core_with("abc");
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "bc");
    }

    #[test]
    fn motions_respect_logical_lines() {
        let mut c = core_with("abc\nx\nlonger\n");
        c.cursor = CursorPos { line: 0, col: 3 };
        c.apply_motion(Motion::Right, false); // 折到下一行行首
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });

        c.apply_motion(Motion::End, false);
        assert_eq!(c.cursor.col, 1);

        c.apply_motion(Motion::Down, false); // 到 "longer"，列被夹紧
        assert_eq!(c.cursor, CursorPos { line: 2, col: 1 });

        c.apply_motion(Motion::DocEnd, false);
        // 与主流编辑器一致：文档以换行结尾时，末尾停在最后的空行上
        assert_eq!(c.cursor, CursorPos { line: 3, col: 0 });
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut c = core_with("");
        c.insert_str("第一版");
        c.insert_str("+第二版");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "第一版");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "第一版");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "第一版+第二版");
        assert!(!c.redo()); // 到底了
    }

    // ---------- P42 列宽实测 ----------

    #[test]
    fn measured_char_width_overrides_and_rejects_invalid() {
        let mut c = core_with("hello");
        // 未实测：固定假设路径（既有契约）
        assert_eq!(c.char_width(), 9.0);

        // 合法实测值接管
        assert!(c.set_measured_char_width(8.0));
        assert_eq!(c.char_width(), 8.0);

        // 非法值全部拒绝且保持现状：非有限 / 非正 / 超出字号 [0.3,0.9] 倍
        for bad in [0.0, -8.0, f32::NAN, f32::INFINITY, 20.0, 3.0] {
            assert!(!c.set_measured_char_width(bad), "应拒绝 {bad}");
            assert_eq!(c.char_width(), 8.0);
        }
        // 倍率区间两端可接受（16px → [4.8, 14.4]）
        assert!(c.set_measured_char_width(4.8));
        assert!(c.set_measured_char_width(14.4));
    }

    #[test]
    fn measured_char_width_drives_caret_and_hit_test() {
        let mut c = core_with("hello world");
        c.set_viewport_height(200.0);
        c.set_measured_char_width(8.0);
        let gutter = c.gutter_width();

        // 点击第 5 列中点之后 → col 5；光标 x = gutter + 5×8（贴合字形，
        // 旧假设 9px 时会漂到 gutter+45，压进后面的字符）
        let hit = c.hit_test(gutter + 5.0 * 8.0 + 1.0, 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 5 });
        c.cursor = hit;
        let caret_x = c.caret_rect_relative().x;
        assert!((caret_x - (gutter + 40.0)).abs() < 1e-3);

        // CJK 双宽列同样按实测列宽换算：点击第 2 列（第 1 个汉字占 0..16px）
        let mut zh = core_with("中文");
        zh.set_measured_char_width(8.0);
        let gutter = zh.gutter_width();
        let hit = zh.hit_test(gutter + 2.5 * 8.0, 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 1 });
        zh.cursor = hit;
        assert!((zh.caret_rect_relative().x - (gutter + 16.0)).abs() < 1e-3);
    }

    #[test]
    fn set_font_size_rescales_measured_width() {
        let mut c = core_with("hello");
        assert!(c.set_measured_char_width(8.0));
        // 同一字体 advance 与字号线性：16px 实测 8.0 → 20px 应为 10.0
        c.set_font_size(20.0);
        assert_eq!(c.char_width(), 10.0);
        c.set_font_size(16.0);
        assert_eq!(c.char_width(), 8.0);
    }

    /// 量一段文本的真实 advance 宽（像素）：与正文绘制同款 cosmic-text
    /// 段落参数（Shaping::Advanced + Wrapping::None），段落总宽。
    fn measure_text_advance(font: Font, size: f32, text: &str) -> Option<f32> {
        if !(size.is_finite() && size > 0.0) {
            return None;
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
        let total = paragraph.min_bounds().width;
        if !(total.is_finite() && total > 0.0) {
            return None;
        }
        Some(total)
    }

    /// 量单个字符的真实 advance（像素/字符）：8 字符采样取均值。
    fn measure_char_advance(font: Font, size: f32, ch: char) -> Option<f32> {
        if ch.is_control() {
            return None;
        }
        measure_text_advance(font, size, &ch.to_string().repeat(8))
            .map(|w| w / 8.0)
    }

    /// 端到端自洽（第 40 轮根治）：`shape_row_xs` 注入布局必须与真实 shaping
    /// 逐前缀一致——光标/点击/选区按注入布局定位即与绘制同源零误差。
    /// 历史沿革：早期断言对象是「列模型 vs 真实」——实测暴露 ★☆☀☁♠♥
    /// （6 字符累计 49.8px）、✅ emoji（+14px）、▀▄█▌（分数宽度 15.01px）
    /// 等不可静态修复的漂移源，故列模型仅作「未注入行」的回退兜底，
    /// 其偏差在此作为文档输出（不再断言）。
    #[test]
    fn injected_row_layout_matches_real_shaping() {
        let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
            return;
        };
        let families: Vec<String> = font_system
            .raw()
            .db_mut()
            .faces()
            .flat_map(|face| face.families.iter().map(|(n, _)| n.clone()))
            .collect();
        drop(font_system);
        let Some(family) = pick_cjk_mono_family(&families) else {
            return;
        };
        let font = Font {
            family: iced::font::Family::Name(family),
            ..Font::MONOSPACE
        };
        let Some(narrow) = measure_char_advance(font, 16.0, '0') else {
            return;
        };
        // 各行覆盖：纯 ASCII / 纯中文 / 混排 / 分数宽度块元素 / 未覆盖
        // 符号段 / emoji（回退字形）/ Tab / 常见日志行
        let lines: &[&str] = &[
            "abc123",
            "中文中文中文",
            "a中a中a",
            "──分割线──",
            "▀▄█▌",
            "→ 箭头 → ◆",
            "…省略…",
            "hello world 2026-08-25 12:00:00 INFO",
            "执行完毕：成功 ✅ 继续",
            "★☆☀☁♠♥",
            "x·y°C℃№×÷≈≠≤≥",
            "quite a long ascii line with many words to amplify any drift",
            "制表\t位\t\t测试",
        ];
        let mut worst = 0.0f32;
        let mut worst_line = String::new();
        for text in lines {
            let chars: Vec<char> = text.chars().collect();
            let Some(xs) = shape_row_xs(font, 16.0, text) else {
                continue;
            };
            assert_eq!(
                xs.len(),
                chars.len() + 1,
                "行 {text:?} 布局长度 = 字符数+1"
            );
            // xs[k] = 第 k 字符起点 = 前 k 个字符的真实总宽（k=0 → 0）
            for (k, _ch) in chars.iter().enumerate() {
                let prefix: String = chars[..k].iter().collect();
                let Some(real) = measure_text_advance(font, 16.0, &prefix) else {
                    continue;
                };
                let delta = (xs[k] - real).abs();
                if delta > worst {
                    worst = delta;
                    worst_line = text.to_string();
                }
                let k_p1 = k + 1;
                assert!(
                    delta < 0.5,
                    "行 {text:?} 第{k_p1}字符起点 {:.2}px ≠ 前 {k} 字符真实宽 {real:.2}px",
                    xs[k]
                );
            }
            let Some(real_total) = measure_text_advance(font, 16.0, text) else {
                continue;
            };
            let delta_tail = (xs[chars.len()] - real_total).abs();
            if delta_tail > worst {
                worst = delta_tail;
                worst_line = text.to_string();
            }
            assert!(
                delta_tail < 0.5,
                "行 {text:?} 行尾 注入 {:.2}px ≠ 真实 {real_total:.2}px",
                xs[chars.len()]
            );
            // 文档输出：列模型在此行的偏差（说明「未注入回退」的兜底量级）
            let mut model_cols = 0f32;
            for ch in chars.iter() {
                let w = char_cols(*ch, model_cols as usize);
                model_cols += w;
            }
            let model: f32 = model_cols * narrow;
            eprintln!(
                "[文档] 行 {text:?} 列模型行宽 {model:.1}px vs 真实 {real_total:.1}px（偏差 {:.1}px，注入布局后光标不再受其影响）",
                (model - real_total).abs()
            );
        }
        eprintln!(
            "[对拍通过] 族={family}，'0'={narrow:.2}px：注入布局与真实排版逐前缀误差最大 {worst:.3}px（行 {worst_line:?}）"
        );
    }

    /// 第 40 轮：注入行级真实布局后，caret/选区/hit_test 全部按真实字形
    /// 位置定位且互相往返一致（行含分数宽度/全宽/emoji 混合）；清空后
    /// 回退列模型（既有行为不变）。
    #[test]
    fn row_layout_drives_caret_selection_and_hit_test() {
        let text = "★☆☀ 我✅a";
        let Some(xs) = shape_row_xs(BODY_FONT, 16.0, text) else {
            return;
        };
        let mut c = core_with(text);
        c.set_viewport_height(300.0);
        c.set_row_layout(0, xs.clone());
        let gutter = c.gutter_width();

        // caret：每列起点 x == 注入值（字形画在哪光标就在哪）
        for (col, x) in xs.iter().enumerate() {
            c.cursor = CursorPos { line: 0, col };
            let caret = c.caret_rect_relative();
            assert!(
                (caret.x - (gutter + *x)).abs() < 1e-3,
                "caret col={col} x={} 应为 {}",
                caret.x,
                gutter + xs[col]
            );
        }

        // hit_test：字符中点两侧往返（左半→k，右半→k+1）
        for k in 0..(xs.len() - 1) {
            let mid = (xs[k] + xs[k + 1]) * 0.5;
            let hit_left = c.hit_test(gutter + mid - 0.1, 0.0);
            let hit_right = c.hit_test(gutter + mid + 0.1, 0.0);
            assert_eq!(hit_left, CursorPos { line: 0, col: k }, "中点左 k={k}");
            assert_eq!(
                hit_right,
                CursorPos { line: 0, col: k + 1 },
                "中点右 k={k}"
            );
        }
        // 行尾之后点击夹紧到行尾
        let tail = c.hit_test(gutter + xs.last().unwrap() + 40.0, 0.0);
        assert_eq!(tail, CursorPos { line: 0, col: xs.len() - 1 });
        // 行宽高水位（水平行程钳制的真实补充上界）
        assert!((c.max_row_width_px() - xs.last().unwrap()).abs() < 1e-3);
        assert_eq!(c.row_width_px(0), Some(xs.last().copied().unwrap()));

        // 清空 → 全部回退列模型（既有路径行为不变）
        c.clear_row_layouts();
        assert_eq!(c.max_row_width_px(), 0.0);
        assert_eq!(c.row_width_px(0), None);
        let caret = c.caret_rect_relative();
        let col = c.cursor.col.min(text.chars().count());
        let expect = gutter + prefix_width(text, col) * c.char_width();
        assert!((caret.x - expect).abs() < 1e-3, "清空后应回退列模型");
    }

    /// 第 40 轮诊断：列模型分类（[`is_wide`]）与等宽 CJK 字体真实字形宽度
    /// 逐字符对拍——光标压字/漂移的候选根因 = 分类与实际 advance 不符
    /// （如制表绘图/块元素/箭头/数学符号在中文字体内多为全宽，却被判 1 列）。
    /// 失败输出即修复清单；无 CJK 等宽字体环境时跳过（零断言）。
    #[test]
    fn wide_classification_matches_real_glyph_advance() {
        let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
            return;
        };
        let families: Vec<String> = font_system
            .raw()
            .db_mut()
            .faces()
            .flat_map(|face| face.families.iter().map(|(n, _)| n.clone()))
            .collect();
        drop(font_system);
        let Some(family) = pick_cjk_mono_family(&families) else {
            return;
        };
        let font = Font {
            family: iced::font::Family::Name(family),
            ..Font::MONOSPACE
        };
        let Some(narrow) = measure_char_advance(font, 16.0, '0') else {
            return;
        };
        let cases: &[(char, &str)] = &[
            ('A', "ASCII 字母"),
            ('0', "ASCII 数字"),
            (',', "半角逗号"),
            (' ', "半角空格"),
            ('·', "间隔号 U+00B7"),
            ('我', "CJK 汉字"),
            ('，', "全角逗号 U+FF0C"),
            ('：', "全角冒号 U+FF1A"),
            ('！', "全角叹号 U+FF01"),
            ('　', "全角空格 U+3000"),
            ('「', "直角引号 U+300C"),
            ('、', "顿号 U+3001"),
            ('。', "句号 U+3002"),
            ('—', "破折号 U+2014"),
            ('…', "省略号 U+2026"),
            ('─', "制表横线 U+2500"),
            ('│', "制表竖线 U+2502"),
            ('├', "制表交叉 U+251C"),
            ('└', "制表拐角 U+2514"),
            ('█', "块元素 U+2588"),
            ('▀', "块元素 U+2580"),
            ('◆', "几何图形 U+25C6"),
            ('→', "箭头 U+2192"),
            ('←', "箭头 U+2190"),
            ('×', "乘号 U+00D7"),
            ('÷', "除号 U+00F7"),
            ('≈', "约等号 U+2248"),
            ('≠', "不等号 U+2260"),
            ('≤', "小于等于 U+2264"),
            ('≥', "大于等于 U+2265"),
            ('°', "度号 U+00B0"),
            ('№', "编号 U+2116"),
            ('℃', "摄氏度 U+2103"),
        ];
        let mut mismatches = Vec::new();
        for &(ch, label) in cases {
            let Some(w) = measure_char_advance(font, 16.0, ch) else {
                continue;
            };
            let ratio = w / narrow;
            let actual_wide = ratio >= 1.5;
            let expect_wide = is_wide(ch);
            if actual_wide != expect_wide {
                mismatches.push(format!(
                    "U+{:04X} {label:<10} ratio={ratio:.2} 表判 {}列 实际 {}列",
                    ch as u32,
                    if expect_wide { 2 } else { 1 },
                    if actual_wide { 2 } else { 1 },
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "列模型分类与等宽 CJK 字体真实字形不符（族={family}，'0' 宽 {narrow:.2}px）：\n{}",
            mismatches.join("\n")
        );
    }

    // （P68：ensure_measured_char_width_measures_and_dedups 横跨 view/core
    // 两层，随构造方式迁往 view_tests.rs）

    #[test]
    fn ensure_visible_follows_cursor_jump() {
        let mut c = core_with(&(0..500).map(|i| format!("line{i}\n")).collect::<String>());
        c.jump_to_line(480);
        let (first, _) = c.visible_range();
        let cursor_row = 479.0;
        assert!(
            (cursor_row >= first as f32)
                && (cursor_row <= first as f32 + c.viewport_height() / c.line_height())
        );
    }

    #[test]
    fn default_metrics_match_legacy_constants() {
        let c = core_with("");
        assert_eq!(c.font_size(), 16.0);
        // 与旧 LINE_HEIGHT/CHAR_WIDTH 常量完全一致，保证默认观感不变
        assert_eq!(c.line_height(), 22.0);
        assert_eq!(c.char_width(), 9.0);
    }

    // ---------- P45：超宽打字的滚动即时性与滚动条收敛 ----------

    /// P45 回归 1：布局滞后（上一帧旧文本注入）时，光标水平位置必须回退
    /// 列模型实时计算——否则超宽后新字符画在视口外（吞字）。
    #[test]
    fn typing_beyond_viewport_scrolls_immediately_despite_stale_layout() {
        let mut c = core_with("short");
        c.set_viewport_width(260.0); // 视口窄，容纳约 20 字符
        // 模拟「上一帧注入的是短文本布局」：行内容已变长、布局滞后
        c.set_row_layout(0, vec![0.0, 9.0, 18.0]); // 旧布局（2 字符 + 行尾）

        // 光标移到行尾并输入 40 字符（远超视口）
        let text = "x".repeat(40);
        c.cursor = CursorPos { line: 0, col: 5 };
        c.insert_str(&text); // 内部 ensure_visible → px_of 滞后回退列模型

        // 光标必须仍在视口内（行尾光标右缘允许探出内容末端 2px——
        // 通用编辑器语义：贴行尾绘制；clamp 以内容宽为上限截停滚动）
        let caret = c.caret_rect_relative();
        assert!(
            caret.x >= 0.0 && caret.x <= 260.0,
            "光标左缘必须仍在视口内（吞字回归），x={}",
            caret.x
        );
        let view_w = 260.0 - c.gutter_width();
        // 期望滚动 = ensure 目标（cx−view_w+2）与内容宽上限（405−view_w）取小
        let expect_scroll =
            ((405.0 - view_w + CARET_WIDTH).max(0.0)).min((405.0 - view_w).max(0.0));
        assert!(
            (c.scroll_left - expect_scroll).abs() < 1e-3,
            "视口应紧随光标右移：{}/{}",
            c.scroll_left,
            expect_scroll
        );
    }

    /// P45 回归 2：`px_of` 布局新鲜走真实字形位置、滞后回退列模型。
    #[test]
    fn px_of_prefers_fresh_layout_and_falls_back_when_stale() {
        let mut c = core_with("短行abcX");
        c.set_row_layout(0, vec![0.0, 16.0, 24.0, 32.0, 40.0, 48.0, 56.0]);
        // 布局新鲜（覆盖全部 6 字符 + 行尾）：真实字形位置
        assert_eq!(c.px_of(0, "短行abcX", 1), 16.0);
        assert_eq!(c.px_of(0, "短行abcX", 3), 32.0);
        assert_eq!(c.px_of(0, "短行abcX", 6), 56.0, "行尾 = 第 6 字符后");
        // 越界列按行文本长度夹紧（col 5 = 第 5 字符后 = 48）
        assert_eq!(c.px_of(0, "短行abcX", 99), 56.0);
        // 文本变长但布局未刷新（滞后）：回退列模型实时文本
        assert_eq!(
            c.px_of(0, "短行abcXXXX", 10),
            prefix_width("短行abcXXXX", 10) * c.char_width()
        );
    }

    /// P45 回归 3：点击命中在布局滞后时回退列模型（点新字符不落旧行尾）。
    #[test]
    fn hit_test_falls_back_when_layout_stale() {
        // 布局滞后：注入的是旧文本（3 字符）布局，文档当前 6 字符
        let mut c = core_with("abcdef");
        c.set_viewport_height(200.0);
        c.set_row_layout(0, vec![0.0, 9.0, 18.0, 27.0]); // 旧布局（3 字符+行尾）
        let gutter = c.gutter_width();
        // 第 6 字符左半区（x∈[45,49.5)）：滞后回退列模型 → col 5
        let hit = c.hit_test(gutter + 49.0, 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 5 });
        // 行尾之后：夹紧新文本行尾（6）
        let hit_tail = c.hit_test(gutter + 100.0, 0.0);
        assert_eq!(hit_tail, CursorPos { line: 0, col: 6 });
        // 布局新鲜（6 字符布局注入）后同一点走真实路径，结果一致
        c.set_row_layout(0, vec![0.0, 9.0, 18.0, 27.0, 36.0, 45.0, 54.0]);
        let hit2 = c.hit_test(gutter + 49.0, 0.0);
        assert_eq!(hit2, CursorPos { line: 0, col: 5 }, "布局新鲜走真实路径");
    }

    /// P45 回归 4：缩短类编辑后 `max_line_cols` 过冷却窗收敛——水平滚动条
    /// 不会常驻不消（旧取舍：只升不降）。
    #[test]
    fn max_line_cols_converges_after_shrinking_edits() {
        let mut c = core_with(&format!("{}\n短", "x".repeat(100)));
        c.set_viewport_width(400.0);
        assert!(c.max_line_cols >= 100, "长行抬高水位");

        // 删除 1 字符：ensure_visible→clamp 链上 stale 首次触发即收敛
        // （checked=None 视为已过冷却窗）
        c.cursor = CursorPos { line: 0, col: 100 };
        c.backspace();
        assert_eq!(c.max_line_cols, 99, "删除后水位即刻收敛到 99");
        assert!(!c.max_cols_stale, "收敛后清除标记");

        // 同一冷却窗内再删：限流生效，不立即全量重算（50MB 文档防每删
        // 一次 O(n)）——水位暂保持 99（过估 1 列，滚动条宽度误差可忽略）
        c.cursor = CursorPos { line: 0, col: 99 };
        c.backspace();
        assert!(c.max_cols_stale, "冷却窗内仍保持待收敛标记");
        assert!(c.max_line_cols >= 98, "限流期内不重算（旧高水位暂存）");

        // 越过冷却窗后任一钳制路径触发全量重算 → 收敛
        c.max_cols_checked = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        c.clamp_scroll_horizontal();
        assert!(!c.max_cols_stale, "收敛后清除标记");
        assert_eq!(c.max_line_cols, 98, "最宽行 = 收缩后的 98 字符行");

        // 整文档替换为短内容（再经编辑路径）→ 收敛到 2 列
        c.cursor = CursorPos { line: 0, col: 1 };
        c.select_all();
        c.replace_selection("短");
        c.max_cols_checked = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        c.clamp_scroll_horizontal();
        assert_eq!(c.max_line_cols, 2, "「短」= 2 列");

        // 再输入长内容：水位随输入即时抬高（不影响即时滚动行为）
        c.cursor = CursorPos { line: 0, col: 1 };
        c.insert_str(&"y".repeat(50));
        assert!(c.max_line_cols >= 50);
    }

    /// P45 回归 5：水平行程统一口径——真实行宽证据并入行程上限。
    #[test]
    fn content_width_merges_column_highwater_and_real_rows() {
        let mut c = core_with("a");
        c.set_row_layout(0, vec![0.0, 200.0]); // 注入超宽真实行（如全宽符号）
        assert_eq!(c.content_width_px(), 200.0, "真实行宽接管行程");
        c.clear_row_layouts();
        assert_eq!(c.content_width_px(), c.max_line_cols as f32 * c.char_width(), "无注入回退列模型");
    }

    /// P46 结论固化：bounds 宽度**不**影响 glyph 生成数量——cosmic-text 对
    /// 无换行长行全量 shape（视口宽 bounds 与 INFINITY 出的字形一样多）；
    /// 「41 字封顶」的截断不在 shape 层（headless 像素实验
    /// `headless_render_shows_characters_beyond_old_viewport_bound`
    /// 同证绘制层两版均无截断）。
    #[test]
    fn paragraph_shapes_all_glyphs_regardless_of_bounds() {
        let text: String = "字".repeat(41);
        let make = |bounds: Size| {
            <iced::Renderer as core_text::Renderer>::Paragraph::with_text(core_text::Text {
                content: text.as_str(),
                bounds,
                size: Pixels(16.0),
                line_height: core_text::LineHeight::Absolute(Pixels(22.0)),
                font: BODY_FONT,
                align_x: core_text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: core_text::Shaping::Advanced,
                wrapping: core_text::Wrapping::None,
            })
        };
        let glyphs = |p: &<iced::Renderer as core_text::Renderer>::Paragraph| {
            p.buffer().layout_runs().map(|run| run.glyphs.len()).sum::<usize>()
        };
        let limited_count = glyphs(&make(Size::new(570.0, 22.0)));
        let full_count = glyphs(&make(Size::new(f32::INFINITY, 22.0)));
        eprintln!(
            "[诊断] bounds=570px → {limited_count} 字形；bounds=∞ → {full_count} 字形（41 汉字）"
        );
        assert!(limited_count >= 41, "视口宽 bounds 也必须 shape 全部字形");
        assert!(full_count >= 41, "无限宽 bounds 必须 shape 全部字形");
        assert_eq!(limited_count, full_count, "bounds 不改变字形数量");
    }

// ---------- P66 像素级平滑滚动：行号随动 ----------

#[test]
fn p66_fractional_scroll_top_survives_clamp() {
    // 触控板像素平滑的核心契约：小数滚动位不被吸附到整数行
    // （P59 的 round 曾把它钳成整数——用户看到「内容整行瞬跳、
    // 行号钉死在固定槽位」的直接根因）
    let mut c = core_with(&"l\n".repeat(200));
    c.set_viewport_height(220.0);

    c.scroll_by_lines(-2.5);
    assert!(
        (c.scroll_top - 2.5).abs() < 1e-4,
        "小数滚动位必须保留，实际 {}",
        c.scroll_top
    );

    c.scroll_by_lines(-0.4);
    c.clamp_scroll();
    assert!(
        (c.scroll_top - 2.9).abs() < 1e-4,
        "clamp 只钳范围不取整，实际 {}",
        c.scroll_top
    );

    // 头部越界仍归零；尾部超界仍钳到行程内（+1 行余量语义不变）
    c.scroll_top = -7.0;
    c.clamp_scroll();
    assert_eq!(c.scroll_top, 0.0);
}

// 渲染一帧编辑器画布（P66 对拍脚手架）：控件摆在 (50,60) 尺寸 600×300，
// 白色底。（注：原为某脚手架函数的文档，函数已随重构移除，降级为普通
// 注释保留背景信息。）

/// P46 诊断：滚动条出现阈值必须与「当前文档最宽行的真实像素宽」一致——
/// 实测列宽注入后，90 ASCII 字符（720px）在 800px 视口内**不**出现滚动条；
/// 未实测（9px 假设）或文档确有超宽行时出现。此测试钉住判定口径，
/// 防止任何一侧高估导致「文字还没占满右边滚动条就出现」。
    #[test]
    fn hscrollbar_threshold_matches_real_content_width() {
        // 90 ASCII 字符 × 实测 8px = 720px < 视口 800px → 不出现
        let mut c = core_with(&"x".repeat(90));
        c.set_viewport_width(800.0);
        c.set_measured_char_width(8.0);
        c.recompute_max_line_cols();
        assert_eq!(c.max_line_cols, 90);
        assert_eq!(c.content_width_px(), 720.0, "实测 8px 下行程=真实宽");
        let view = c.text_viewport_w();
        assert!(
            !HScrollbar::measure(c.content_width_px(), view, 800.0, 0.0).needed,
            "720px 内容在 800px 视口内不得出现滚动条"
        );
        // 同文档注入真实行宽（此处与列模型一致），口径不变
        c.set_row_layout(0, {
            let mut xs = Vec::with_capacity(91);
            for i in 0..=90 {
                xs.push(i as f32 * 8.0);
            }
            xs
        });
        assert_eq!(c.content_width_px(), 720.0);
        assert!(
            !HScrollbar::measure(c.content_width_px(), view, 800.0, 0.0).needed
        );
        // 超宽行（200 字符 = 1600px > 800）→ 出现
        c.insert_str(&"x".repeat(110)); // 90+110=200 字符
        assert_eq!(c.max_line_cols, 200);
        c.set_measured_char_width(8.0);
        let needed = HScrollbar::measure(c.content_width_px(), view, 800.0, 0.0).needed;
        assert!(needed, "1600px 内容在 800px 视口内必须出现滚动条");
        // 未实测（回退 9px 假设）：90→810px > 800 → 确实会出现（P42 未生效时的
        // 高估留痕——这正是「文字没占满滚动条就出现」的机制面之一）
        let mut c2 = core_with(&"x".repeat(90));
        c2.set_viewport_width(800.0);
        c2.recompute_max_line_cols();
        assert_eq!(c2.content_width_px(), 810.0);
        assert!(
            HScrollbar::measure(c2.content_width_px(), c2.text_viewport_w(), 800.0, 0.0).needed,
            "9px 假设下 810px > 800px 出现——文档化：实测注入后消失"
        );
    }

    #[test]
    fn set_font_size_clamps_and_rescales_metrics() {
        use editpad_core::settings::{MAX_FONT_SIZE, MIN_FONT_SIZE};

        let mut c = core_with("hello\nworld\n");
        c.set_font_size(40.0); // 越上界被夹紧
        assert_eq!(c.font_size(), MAX_FONT_SIZE);
        c.set_font_size(1.0); // 越下界被夹紧
        assert_eq!(c.font_size(), MIN_FONT_SIZE);
        c.set_font_size(f32::NAN); // 非有限值回退默认
        assert_eq!(c.font_size(), 16.0);

        c.set_font_size(20.0);
        // 断言具体值：20 × 1.375 与 20 × 0.5625 均为二进制精确值
        assert_eq!(c.line_height(), 27.5);
        assert_eq!(c.char_width(), 11.25);

        // 字号变大后同样视口可见行数变少（用长文档避免被文档末尾夹平）
        let mut long = core_with(&(0..200).map(|i| format!("l{i}\n")).collect::<String>());
        long.set_viewport_height(220.0);
        long.set_font_size(16.0);
        let (_, last_16) = long.visible_range();
        long.set_font_size(MAX_FONT_SIZE);
        let (_, last_big) = long.visible_range();
        assert!(last_big < last_16, "字号变大后可见行应变少");
    }

    #[test]
    fn set_font_size_keeps_cursor_visible() {
        let mut c = core_with(&(0..500).map(|i| format!("line{i}\n")).collect::<String>());
        c.set_viewport_height(400.0);
        c.set_font_size(24.0);
        c.jump_to_line(480);
        // 字号再调大也不得把光标挤出视口（ensure_visible 的不变量以浮点 scroll_top 表述）
        c.set_font_size(28.0);
        let cursor_row = 479.0;
        let rows_in_view = c.viewport_height() / c.line_height();
        assert!(c.scroll_top <= cursor_row, "光标行应在视口顶之下");
        assert!(
            cursor_row <= c.scroll_top + rows_in_view - 1.0 + 1e-3,
            "光标行应在视口底之上"
        );
    }

    // ---------- P3 滚轮方向契约 ----------

    #[test]
    fn scroll_by_lines_sign_contract_matches_winit() {
        // winit 约定：滚轮上推上报 LineDelta(y=+1)（"positive Y means moving the
        // content down"，即视口向文档头方向走）。控件层把 y×3 直接传给本方法，
        // 因此契约是：正数 → scroll_top 减小（看到更早的内容）；负数反之。
        let mut c = core_with(&(0..200).map(|i| format!("l{i}\n")).collect::<String>());
        c.set_viewport_height(220.0);
        c.scroll_top = 50.0;
        c.clamp_scroll();

        c.scroll_by_lines(3.0); // 滚轮上推一格
        assert!((c.scroll_top - 47.0).abs() < 1e-4, "上推应向文档头滚动，实际 {}", c.scroll_top);

        c.scroll_by_lines(-6.0); // 滚轮下推两格
        assert!((c.scroll_top - 53.0).abs() < 1e-4, "下推应向文档尾滚动，实际 {}", c.scroll_top);

        // 头部钳制：继续上推不得变负
        c.scroll_by_lines(100.0);
        assert_eq!(c.scroll_top, 0.0);
    }

    // ---------- P13 水平滚动 ----------

    fn core_with_wide_line() -> EditorCore {
        // 一条 1000 列的超长行 + 若干短行：内容必然超出任何常规视口宽
        let mut c = core_with(&format!("{}\nshort\n", "x".repeat(1000)));
        c.set_viewport_width(400.0);
        c
    }

    #[test]
    fn hscrollbar_geometry_mirrors_vertical_contract() {
        let char_w = 16.0 * 0.5625;
        let content_px = 1000.0 * char_w;

        // 内容不超宽 → 不需要水平滚动条
        let fit = HScrollbar::measure(100.0, 400.0, 800.0, 0.0);
        assert!(!fit.needed);
        assert!(!HScrollbar::measure(0.0, 400.0, 800.0, 0.0).needed);

        // 超宽 → 出现，滑块宽 ∝ 视口占比（未触底时）
        let sb = HScrollbar::measure(content_px, 400.0, 800.0, 0.0);
        assert!(sb.needed);
        let expect_w = (sb.track_w * 400.0 / content_px).clamp(THUMB_MIN_W, sb.track_w);
        assert!((sb.thumb_w - expect_w).abs() < 1e-3);
        assert!(sb.thumb_w > THUMB_MIN_W, "此比例下不应触底");

        // 极宽内容（20 万列）滑块触底到最小宽度
        let huge = HScrollbar::measure(200_000.0 * char_w, 400.0, 800.0, 0.0);
        assert_eq!(huge.thumb_w, THUMB_MIN_W);

        // 拖拽逆映射恒等；两端夹紧
        for &scroll in &[0.0f32, 100.0, 777.7] {
            let back = sb.scroll_for_thumb_x(sb.thumb_x + scroll / sb.range_px * (sb.track_w - sb.thumb_w));
            assert!(
                (back - scroll.min(sb.range_px)).abs() < 0.01,
                "scroll={scroll} 反解={back}"
            );
        }
        assert_eq!(sb.scroll_for_thumb_x(sb.track_x - 40.0), 0.0);
        assert!((sb.scroll_for_thumb_x(sb.track_x + sb.track_w) - sb.range_px).abs() < 1e-3);

        // 轨道点击把滑块中心对准点击处
        let click = sb.track_x + sb.track_w * 0.8;
        let scrolled = sb.scroll_for_track_click(click);
        let after = HScrollbar::measure(content_px, 400.0, 800.0, scrolled);
        let center_after = after.thumb_x + after.thumb_w / 2.0;
        assert!((center_after - click).abs() < 1.5);

        // 命中区只在下缘窄带
        assert!(!sb.hits(400.0, 700.0, 720.0), "中部不得算命中");
    }

    #[test]
    fn wide_document_tracks_max_cols_and_enables_hscroll() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        assert_eq!(c.max_line_display_cols(), 1000, "reset 时应精确重算最宽行");

        // 高水位只上调：编辑短行不缩水，编辑出更长的行要跟上
        c.cursor = CursorPos { line: 1, col: 5 };
        c.insert_str("yyyyyyyyyy"); // 短行 15 列，仍小于 1000
        assert_eq!(c.max_line_display_cols(), 1000);
        c.cursor = CursorPos { line: 2, col: 0 };
        c.insert_str(&"z".repeat(1200)); // 更长的新行
        assert_eq!(c.max_line_display_cols(), 1200);

        // 整体替换后精确重算（高水位回落）
        c.replace_whole_document(Document::from_str("tiny"));
        assert_eq!(c.max_line_display_cols(), 4);
    }

    #[test]
    fn ensure_visible_follows_cursor_horizontally() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        assert_eq!(c.scroll_left, 0.0);

        // 光标放到超长行末尾 → 视口右移让行尾可见
        c.cursor = CursorPos { line: 0, col: 1000 };
        c.ensure_visible_pub();
        assert!(c.scroll_left > 0.0, "行尾光标必须推动横向滚动");

        // 光标回到行首（Home/左移到 0 列）→ 视口回到最左
        c.cursor = CursorPos { line: 0, col: 0 };
        c.ensure_visible_pub();
        assert_eq!(c.scroll_left, 0.0, "第 0 列必须滚回左缘");

        // 手动横滚到中间再点视口内的位置 → 不打扰当前横向位置
        c.scroll_by_columns(50.0);
        let mid = c.scroll_left;
        c.cursor = CursorPos { line: 0, col: 60 };
        c.ensure_visible_pub();
        assert!((c.scroll_left - mid).abs() < f32::EPSILON.max(mid * 1e-6),
            "可见范围内的光标不得扰动横向滚动");
    }

    #[test]
    fn scroll_by_columns_clamps_to_content_width() {
        let mut c = core_with_wide_line();
        c.clamp_scroll_horizontal();
        assert_eq!(c.scroll_left, 0.0, "空行程时保持 0");

        c.scroll_by_columns(-5000.0);
        assert_eq!(c.scroll_left, 0.0, "左向越界夹到 0");

        c.scroll_by_columns(f32::MAX / 2.0); // 巨量右滚 → 夹到最大行程
        let max_expected =
            (c.max_line_display_cols() as f32 * c.char_width()) - c.text_viewport_w();
        assert!((c.scroll_left - max_expected).abs() < 1.0, "右缘应贴住最宽行");
    }

    #[test]
    fn hit_test_accounts_for_horizontal_scroll_offset() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        // 同一文档坐标的列，在滚动前后用相差 scroll_left 的 x 点击应命中同列
        let gutter = c.gutter_width();
        let char_w = c.char_width();
        let target_col = 20usize;

        c.scroll_left = 0.0;
        let hit_unscrolled = c.hit_test(gutter + target_col as f32 * char_w + 1.0, 10.0);
        c.scroll_by_columns(30.0); // 视口右移 30 列
        let hit_scrolled =
            c.hit_test(gutter + (target_col as f32 - 30.0) * char_w + 1.0, 10.0);
        assert_eq!(hit_unscrolled.col, target_col);
        assert_eq!(hit_scrolled.col, target_col, "横滚后命中测试必须补偿 scroll_left");
    }

    #[test]
    fn caret_rect_shifts_by_scroll_left() {
        let mut c = core_with_wide_line();
        c.set_viewport_height(300.0);
        c.cursor = CursorPos { line: 0, col: 100 };
        c.ensure_visible_pub();
        let before = c.caret_rect_relative().x;

        c.scroll_by_columns(40.0);
        let after = c.caret_rect_relative().x;
        assert!(
            (before - after - 40.0 * c.char_width()).abs() < 1e-3,
            "光标矩形必须随横向滚动平移（IME 候选框与绘制共用此路径）"
        );
    }

    // ---------- 健壮性边界用例批 ----------

    #[test]
    fn oversized_line_number_jump_clamps_to_last_line() {
        // 「跳转到行」输入 usize::MAX / 手改配置等极端值不得 panic，
        // 必须夹紧到文档末行（ropey 越界是 panic 不是错误）
        let mut c = core_with("l1\nl2\nl3");
        c.jump_to_line(usize::MAX);
        assert_eq!(c.cursor.line, 2, "超大行号应落到末行");
        c.jump_to_line(0); // 0 与 1 等价（1 起）
        assert_eq!(c.cursor.line, 0);
        c.jump_to_line(2);
        assert_eq!(c.cursor.line, 1);

        // 带尾换行的文档：末行是换行后的空行，同样不得越界
        let mut trailing = core_with("l1\nl2\n");
        trailing.jump_to_line(usize::MAX);
        assert_eq!(trailing.cursor.line, 2);
    }

    #[test]
    fn restore_view_places_cursor_and_scroll_without_side_effects() {
        // P30：会话恢复定位入口——越界夹紧、非有限滚动消毒、无撤销副作用
        let mut c = core_with("第一行\r\nsecond\r\n第三行");
        c.restore_view(usize::MAX, usize::MAX, f32::NAN, f32::INFINITY);
        assert_eq!(
            c.cursor,
            CursorPos { line: 2, col: 3 },
            "越界行列必须夹紧到文档末行末列（显示口径不计 \\r）"
        );
        assert_eq!(c.scroll_top, 0.0, "NaN/inf 滚动值消毒归零");
        assert_eq!(c.scroll_left, 0.0);

        // 负值被钳回左上角
        c.restore_view(1, 3, -5.0, -5.0);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 3 });
        assert_eq!(c.scroll_top, 0.0);
        assert_eq!(c.scroll_left, 0.0);

        // 无副作用：不进撤销栈、不置脏（undo/redo 栈保持为空）
        assert!(c.undo_stack.is_empty() && c.redo_stack.is_empty());

        // 空文档同样不得 panic（恢复一个空页的光标位置）
        let mut empty = core_with("");
        empty.restore_view(9, 9, 100.0, 100.0);
        assert_eq!(empty.cursor, CursorPos { line: 0, col: 0 });

        // 视口内有余量时合法值原样落位（31 行 + 120 列长行提供双向行程；
        // 视口压到 5 行高让纵向行程覆盖 20px；第 10 行是 3 字符的 "row"，
        // 超出列宽的请求同样按显示口径夹紧）
        let mut wide = core_with(&format!("{}\n{}", "x".repeat(120), "row\n".repeat(30)));
        wide.set_viewport_height(wide.line_height() * 5.0);
        wide.restore_view(10, 2, 20.0, 40.0);
        assert_eq!(wide.cursor, CursorPos { line: 10, col: 2 });
        assert!((wide.scroll_top - 20.0).abs() < 1e-4, "行程内的纵向滚动应原样保留，实际 {}", wide.scroll_top);
        assert!((wide.scroll_left - 40.0).abs() < 1e-4, "行程内的横向滚动应原样保留，实际 {}", wide.scroll_left);
    }

    #[test]
    fn single_line_document_edges_are_noops_not_panics() {
        let mut c = core_with("only");
        assert_eq!(c.doc.line_count(), 1);

        // 文档首退格 / 文档尾 Delete：无变化不崩溃
        c.backspace();
        assert_eq!(c.doc.to_text(), "only");
        c.cursor = CursorPos { line: 0, col: 4 };
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "only");

        // 行内移动到边界后继续同向移动被吸收
        c.apply_motion(Motion::DocEnd, false);
        c.apply_motion(Motion::Right, false);
        assert_eq!(c.cursor, CursorPos { line: 0, col: 4 });
        c.apply_motion(Motion::DocStart, false);
        c.apply_motion(Motion::Left, false);
        assert_eq!(c.cursor, CursorPos { line: 0, col: 0 });
    }

    #[test]
    fn four_byte_emoji_survives_edit_roundtrip() {
        // 4 字节字符（U+1F680）在插入/撤销/保存口径下都是普通标量
        let mut c = core_with("");
        c.insert_str("发射🚀!");
        assert_eq!(c.doc.to_text(), "发射🚀!");
        assert_eq!(c.doc.text_len(), 4, "emoji 按 1 个 Unicode 标量计");

        c.insert_str("\n更多🛰内容\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "发射🚀!", "撤销必须完整还原含 emoji 的状态");

        // 分块原子保存往返（P19 路径对非 ASCII 同样透明）
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("emoji-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("emoji.txt");
        editpad_core::save_document_atomic(&target, &c.doc).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "发射🚀!");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- 垂直滚动条 ----------

    #[test]
    fn scrollbar_hidden_when_content_fits() {
        // 内容不超视口 → 不需要滚动条（用户要求的核心行为）
        assert!(!VScrollbar::measure(10, 500.0, 22.0, 500.0, 0.0).needed);
        // 空文档 / 单行同样隐藏
        assert!(!VScrollbar::measure(0, 500.0, 22.0, 500.0, 0.0).needed);
        assert!(!VScrollbar::measure(1, 100.0, 22.0, 300.0, 0.0).needed);
    }

    #[test]
    fn scrollbar_appears_on_overflow_with_proportional_thumb() {
        let (lines, lh, vh, wh) = (1000usize, 20.0, 400.0, 400.0);
        let sb = VScrollbar::measure(lines, vh, lh, wh, 0.0);
        assert!(sb.needed);

        // 滑块高度 = 视口占比 × 轨道高（未触底最小值时）
        let expect_h = (sb.track_h * vh / (lines as f32 * lh)).max(THUMB_MIN_H);
        assert!((sb.thumb_h - expect_h).abs() < 1e-3);
        assert!(sb.thumb_y >= sb.track_y);

        // 滚动到 clamp 允许的最大值（行数-可见行+1）→ 滑块应贴到轨道底部
        let max_scroll = lines as f32 - vh / lh + 1.0;
        let bot = VScrollbar::measure(lines, vh, lh, wh, max_scroll + 10.0);
        assert!(
            (bot.thumb_y + bot.thumb_h - (bot.track_y + bot.track_h)).abs() < 1e-2,
            "超出行程的 scroll_top 应被夹到滑块贴底"
        );
    }

    #[test]
    fn thumb_drag_roundtrips_through_inverse_mapping() {
        let (lines, lh, vh, wh) = (2000usize, 22.0, 550.0, 800.0);
        for &scroll in &[0.0f32, 7.5, 123.4, 900.0] {
            let sb = VScrollbar::measure(lines, vh, lh, wh, scroll);
            assert!(sb.needed);
            // 滑块位置反解回 scroll_top 必须是恒等映射
            let back = sb.scroll_for_thumb_y(sb.thumb_y);
            assert!(
                (back - scroll.min(sb.range_lines)).abs() < 0.01,
                "scroll={scroll} 反解={back}"
            );
        }
        // 拖出上下边界都要夹紧
        let sb = VScrollbar::measure(lines, vh, lh, wh, 100.0);
        assert_eq!(sb.scroll_for_thumb_y(sb.track_y - 50.0), 0.0, "拖过头=回到顶部");
        let over = sb.scroll_for_thumb_y(sb.track_y + sb.track_h + 50.0);
        assert!((over - sb.range_lines).abs() < 1e-3, "拖到底=最大行程");
    }

    #[test]
    fn track_click_centers_thumb_on_cursor() {
        let sb = VScrollbar::measure(1500usize, 660.0, 22.0, 660.0, 10.0);
        assert!(sb.needed);
        let click = sb.track_y + sb.track_h * 0.8;
        let scrolled = sb.scroll_for_track_click(click);
        // 点击后滑块中心应落在点击处附近（±1px）
        let after = VScrollbar::measure(1500usize, 660.0, 22.0, 660.0, scrolled);
        let center_after = after.thumb_y + after.thumb_h / 2.0;
        assert!((center_after - click).abs() < 1.5, "点击 {click}，中心停在 {center_after}");
    }

    #[test]
    fn huge_document_keeps_grabbable_thumb_min_height() {
        // 50MB 场景：几十万行 → 视口占比极小，滑块不得小于可抓握的最小高度
        let sb = VScrollbar::measure(600_000usize, 800.0, 22.0, 800.0, 0.0);
        assert_eq!(sb.thumb_h, THUMB_MIN_H);
        // 最小滑块仍能覆盖完整行程
        let bottom = sb.scroll_for_thumb_y(sb.track_y + sb.track_h);
        assert!((bottom - sb.range_lines).abs() < 1e-3);
    }

    #[test]
    fn scrollbar_flows_through_core_state() {
        // 经 EditorCore 的真实链路：设视口 → 推导 → 用逆映射模拟拖拽 → 夹紧
        let mut c = core_with(&"line\n".repeat(500));
        c.set_viewport_height(400.0);
        c.scroll_top = 12345.0; // 故意越界
        c.clamp_scroll();

        let sb = VScrollbar::measure(
            c.doc.line_count(),
            c.viewport_h,
            c.line_height(),
            400.0,
            c.scroll_top,
        );
        assert!(sb.needed);
        c.scroll_top = sb.scroll_for_thumb_y(sb.track_y + (sb.track_h - sb.thumb_h) * 0.5);
        c.clamp_scroll();
        assert!(c.scroll_top > 0.0 && c.scroll_top <= sb.range_lines);

        // 文档缩到视口内后滚动条消失
        c.reset_document(Document::from_str("short"));
        c.set_viewport_height(400.0);
        let sb = VScrollbar::measure(
            c.doc.line_count(),
            c.viewport_h,
            c.line_height(),
            400.0,
            c.scroll_top,
        );
        assert!(!sb.needed, "内容装得下就必须隐藏滚动条");

        // 命中区只在右侧窄带
        assert!(!sb.hits(798.0, 300.0, 800.0) || !sb.needed);
        assert!(!sb.hits(100.0, 300.0, 800.0), "正文区域不得算进滚动条命中区");
    }

    // ---------- 光标闪烁 ----------

    #[test]
    fn caret_blink_phase_toggles_and_activity_forces_visible() {
        let mut c = core_with("hello");
        let t0 = std::time::Instant::now();
        assert!(c.caret_visible(), "初始相位可见");

        // 心跳翻转相位（P53 起按真实间隔门控：注入时钟逐拍推进 ~530ms）
        c.tick_blink_at(t0);
        assert!(!c.caret_visible(), "静止期应按相位隐没");
        c.tick_blink_at(t0 + std::time::Duration::from_millis(CARET_BLINK_MS));
        assert!(c.caret_visible());

        // 间隔不足一拍（P53 淡出动画的 33ms 快拍）：相位不得加速翻转
        c.tick_blink_at(
            t0 + std::time::Duration::from_millis(CARET_BLINK_MS + 33),
        );
        assert!(c.caret_visible(), "快拍不得加速相位翻转");

        // 活动窗口：移动光标后 450ms 内无论相位都常显
        c.poke_caret();
        assert!(c.caret_visible());
        c.tick_blink_at(t0 + std::time::Duration::from_millis(CARET_BLINK_MS * 2)); // 相位翻到隐，但活动窗未过期
        assert!(c.caret_visible(), "活动窗口期内不得隐没");
    }

    // ---------- 拖选边缘自动滚动 ----------

    #[test]
    fn drag_edge_autoscroll_pushes_viewport_toward_pointer() {
        // 语义：行/列增量与 scroll_by_lines/columns 入参同号——
        // 顶缘 dy>0（向文档头）、底缘 dy<0（向文档尾）、左缘 dx<0、右缘 dx>0
        let probe = |x: f32, y: f32| {
            let c = core_with(&"x\n".repeat(50));
            c.edge_scroll_delta(x, y, 800.0, 600.0)
        };

        let (dx, dy) = probe(400.0, 10.0);
        assert!(dy > 0.0 && dx == 0.0, "顶缘应向文档头推进且无横向分量");

        let (dx, dy) = probe(400.0, 590.0);
        assert!(dy < 0.0 && dx == 0.0, "底缘应向文档尾推进");

        assert_eq!(probe(400.0, 300.0), (0.0, 0.0), "安全区中点无滚动");

        let (dx_l, _) = probe(8.0, 300.0);
        assert!(dx_l < 0.0, "左缘应向文档头方向横滚");
        let (dx_r, _) = probe(792.0, 300.0);
        assert!(dx_r > 0.0, "右缘应向文档尾方向横滚");
    }

    // ---------- 第 61 轮：括号匹配 ----------

    #[test]
    fn bracket_match_adjacency_and_none() {
        let mut c = core_with("()\nfoo\n");
        // 光标在 '(' 前：右侧字符优先
        c.cursor = CursorPos { line: 0, col: 0 };
        assert_eq!(c.bracket_match(), Some((0, 1)));
        // 光标在 ')' 后：左侧字符兜底
        c.cursor = CursorPos { line: 0, col: 2 };
        assert_eq!(c.bracket_match(), Some((1, 0)));
        // 光标在两括号之间：右侧 ')' 优先（停在括号前口径）
        c.cursor = CursorPos { line: 0, col: 1 };
        assert_eq!(c.bracket_match(), Some((1, 0)));
        // 无括号邻接
        c.cursor = CursorPos { line: 1, col: 1 };
        assert_eq!(c.bracket_match(), None);
        // 文档尾（光标偏移 == text_len）：左兜底仍可用（']' 配 '['）
        let mut d = core_with("x[]");
        d.cursor = CursorPos { line: 0, col: 3 };
        assert_eq!(d.bracket_match(), Some((2, 1)));
        // 孤立括号无配对 → None（"x{" 的 { 等不到闭括号，勘误初版用例：
        // 该场景正确答案就是 None）
        let mut e = core_with("x{");
        e.cursor = CursorPos { line: 0, col: 2 };
        assert_eq!(e.bracket_match(), None);
        // 空文档
        assert_eq!(core_with("").bracket_match(), None);
    }

    #[test]
    fn bracket_match_multiline_nested_and_crlf() {
        // 跨行嵌套：外层 ( 配对到最后一行的 )
        let mut c = core_with("f(\n  g(1)\n)\n");
        c.cursor = CursorPos { line: 0, col: 2 };
        // 偏移口径：'f'0 '('1 '\n'2 '  '3,4 'g'5 '('6 '1'7 ')'8 '\n'9 ')'10
        assert_eq!(c.bracket_match(), Some((1, 10)));
        // 内层配对不受外层干扰（光标在 ')' 后 → 邻接括号在前：(8,6)）
        c.cursor = CursorPos { line: 1, col: 6 };
        assert_eq!(c.bracket_match(), Some((8, 6)));
        // CRLF 文档：\r 计 1 字符，配对偏移仍精确
        let mut d = core_with("a(\r\nb)\r\n");
        // 偏移：'a'0 '('1 '\r'2 '\n'3 'b'4 ')'5
        d.cursor = CursorPos { line: 0, col: 2 };
        assert_eq!(d.bracket_match(), Some((1, 5)));
        d.cursor = CursorPos { line: 1, col: 1 };
        assert_eq!(d.bracket_match(), Some((5, 1)));
        // 三种括号各自独立计数
        let mut e = core_with("([)]"); // 交叉不配平：'(' 向后扫先遇 ')' 归零
        e.cursor = CursorPos { line: 0, col: 0 };
        assert_eq!(e.bracket_match(), Some((0, 2)), "同对独立计数（主流朴素口径）");
    }

    #[test]
    fn bracket_jump_toggles_sides_and_noop_without_match() {
        let mut c = core_with("ab(cd)\n");
        // 光标在 '(' 前 → 跳到 ')' 后
        c.cursor = CursorPos { line: 0, col: 2 };
        assert!(c.jump_to_matching_bracket());
        assert_eq!(c.cursor, CursorPos { line: 0, col: 6 });
        // 再按：光标在 ')' 后 → 跳回 '(' 前（往返手感）
        assert!(c.jump_to_matching_bracket());
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
        // 无括号邻接：不动
        c.cursor = CursorPos { line: 0, col: 0 };
        assert!(!c.jump_to_matching_bracket());
        assert_eq!(c.cursor, CursorPos { line: 0, col: 0 });
        // 未配平：不动
        let mut d = core_with("(abc\n");
        d.cursor = CursorPos { line: 0, col: 0 };
        assert!(!d.jump_to_matching_bracket());
        // 跳转不产撤销快照（纯光标移动）
        assert!(c.undo_stack.is_empty());
    }

    #[test]
    fn bracket_cache_invalidates_on_content_change_same_cursor() {
        let mut c = core_with("a()");
        c.cursor = CursorPos { line: 0, col: 2 }; // ( 与 ) 之间：右邻 ')' 在前
        assert_eq!(c.bracket_match(), Some((2, 1)));
        // 选区替换 ')' 为 'x'：光标原地不动、内容已变 → 不得吐陈旧缓存
        c.cursor = CursorPos { line: 0, col: 2 };
        c.anchor = Some(CursorPos { line: 0, col: 3 });
        c.insert_str("x");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 3 });
        // 光标回到原查询位（替换后列不变），文档已是 "a(x"
        c.cursor = CursorPos { line: 0, col: 2 };
        assert_eq!(
            c.bracket_match(),
            None,
            "同光标位内容已变，缓存必须已失效（invalidate 汇点清缓存）"
        );
    }

    #[test]
    fn bracket_cache_invalidates_on_undo_redo_and_doc_replacement() {
        // 第 61 轮审查发现：undo/redo/reset_document/replace_whole_document
        // 四条整体换文路径旧实现直接调 highlight 失效、绕过
        // invalidate_highlight_from 汇点——光标键控的括号匹配缓存对
        // 「同位异文」吐陈旧结果（撤销落点恰为缓存键时高亮丢失/跳转失灵）。
        let mut c = core_with("(y)");
        c.cursor = CursorPos { line: 0, col: 1 };
        assert_eq!(c.bracket_match(), Some((0, 2)));
        // 打字改内容（常规路径本就清缓存）→ 移回同位再查（在新内容上重建缓存）
        c.cursor = CursorPos { line: 0, col: 2 };
        c.insert_str("z"); // 文档变 "(yz)"，光标落 col3
        c.cursor = CursorPos { line: 0, col: 2 };
        assert_eq!(c.bracket_match(), None, "新内容 (yz) 同位无匹配");
        // 撤销：内容回 "(y)"、快照光标恰落回缓存键 (0,2)——此时右邻恰是
        // ')' 自身，必须重算出 (2,0) 而非吐缓存里的 None
        assert!(c.undo());
        assert_eq!(
            c.bracket_match(),
            Some((2, 0)),
            "撤销后同光标位必须按恢复后的内容重算（缓存已随汇点清空）"
        );
        // 重做对称：回到 "(yz)" 同位 → None
        assert!(c.redo());
        assert_eq!(c.bracket_match(), None);

        // reset_document（加载文件/静默重载 P50 路径）：光标复位 (0,0)
        // 恰是常见缓存键
        let mut d = core_with("()");
        d.cursor = CursorPos { line: 0, col: 0 };
        assert_eq!(d.bracket_match(), Some((0, 1)));
        d.reset_document(editpad_core::Document::from_str("hello"));
        assert_eq!(d.cursor, CursorPos::default());
        assert_eq!(
            d.bracket_match(),
            None,
            "静默重载后不得吐旧文档的陈旧配对"
        );

        // replace_whole_document（全部替换路径）：同样复位到 (0,0)
        let mut e = core_with("()");
        e.cursor = CursorPos { line: 0, col: 0 };
        assert_eq!(e.bracket_match(), Some((0, 1)));
        e.replace_whole_document(editpad_core::Document::from_str("world"));
        assert_eq!(e.bracket_match(), None, "全部替换后不得吐陈旧配对");
    }

    // ---------- 第 73 轮 ⑯：软换行（自动换行）接线 ----------
    //
    // 视口宽 300 → 列预算 = (300 − gutter(≈49))/9 ≈ 27 列（P95 起不预留
    // 滚动条槽位）；行高 = 16×1.375 = 22px。开态后所有断言以
    // wrap_max_cols() 实时取值推导，不硬编码字体度量。

    /// 折行开态 core：两行各 50 字符 + 一行 70 字符（未尾行）。
    fn wrap_core(text: &str) -> EditorCore {
        let mut c = core_with(text);
        c.set_viewport_width(300.0);
        c.set_viewport_height(220.0);
        c.set_word_wrap(true);
        c
    }

    /// 惰性收敛（v1 模型披露：enable/reset 后 BIT 暂记「每行一段」，
    /// 行被查询才差值更新；生产里 draw 每帧遍历可见行即收敛）。
    /// 本助手把全部行查询一遍，令 `visual_rows_total` 反映真实总值。
    fn wrap_converge(c: &mut EditorCore) {
        let n = c.doc.line_count();
        for l in 0..n {
            let t = c.line_text(l);
            c.segments_of_line(l, &t);
        }
    }

    #[test]
    fn wrap_off_identity_toggle_and_visual_mapping_roundtrip() {
        let mut c = core_with("aa\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\ncc\n");
        c.set_viewport_width(300.0);
        c.set_viewport_height(220.0);
        // 关态恒等退化：视觉行 = 逻辑行（ropey 行数含尾部幻影行）
        assert!(!c.wrap_enabled());
        assert_eq!(c.visual_rows_total() as usize, c.doc.line_count());
        assert_eq!(c.visual_row_of(1, 40), 1);
        let (line, seg, s0, s1) = c.locate_visual(2);
        assert_eq!((line, seg, s0, s1), (2, 0, 0, 2));
        // 开启：长行折行
        c.set_word_wrap(true);
        wrap_converge(&mut c);
        let mc = c.wrap_max_cols();
        assert!((20..=30).contains(&mc), "300 宽列预算应≈25，实际 {mc}");
        let segs_line1 = c.line_visual_segments(1);
        assert!(segs_line1 == 2, "48 字符行应折 2 段，实际 {segs_line1}");
        // 1 + 2 + 1 + 幻影 1 = 5
        assert_eq!(c.visual_rows_total(), 1 + segs_line1 + 2);
        // 视觉行 → 逻辑行双向映射一致
        for v in 0..c.visual_rows_total() {
            let (line, _seg, s0, s1) = c.locate_visual(v);
            assert!(line < c.doc.line_count(), "幻影行也是合法视觉行");
            assert!(s0 <= s1);
            assert_eq!(c.visual_row_of(line, s0), v, "段首列映射回自身视觉行");
            if s1 > s0 {
                assert_eq!(c.visual_row_of(line, s1 - 1), v, "段末字符在同一视觉行");
            }
        }
        // 折行边界列属于下一段
        assert_eq!(c.visual_row_of(1, mc), 1 + segs_line1 - 1, "段 2 起点在行 1 末段");
        // 关闭恒等退化（含幻影行 4 逻辑行）
        c.set_word_wrap(false);
        assert_eq!(c.visual_rows_total(), 4);
        assert_eq!(c.visual_row_of(1, 40), 1);
    }

    #[test]
    fn wrap_cjk_segments_follow_double_width_columns() {
        // 中×30 = 60 显示列；预算 mc → 双宽段断点 = mc/2 字符处。
        // 60/mc ≤ 3 → 恰 3 段（mc ∈ 20..=30 恒成立）
        let mut c = wrap_core("中中中中中中中中中中中中中中中中中中中中中中中中中中中中中中\n");
        wrap_converge(&mut c);
        let mc = c.wrap_max_cols();
        assert!((20..=30).contains(&mc));
        assert_eq!(c.line_visual_segments(0), 3, "30 个双宽字符应折 3 段");
        assert_eq!(c.visual_rows_total(), 3 + 1, "3 段 + 幻影行 1 段");
        assert_eq!(c.visual_row_of(0, 0), 0);
        assert_eq!(c.visual_row_of(0, mc / 2 + 1), 1, "跨过段 1 起点（更宽列）");
        let (line, seg, s0, s1) = c.locate_visual(1);
        assert_eq!((line, seg), (0, 1));
        assert_eq!((s0, s1), (mc / 2, 2 * (mc / 2)), "段 1 覆盖双宽断点区间");
    }

    #[test]
    fn wrap_max_cols_fills_viewport_without_hscroll_reservation() {
        // P95 用户点单：开态**不预留水平滚动条槽位**——列预算 = 正文区
        // 可视宽 ÷ 列宽（无 SCROLLBAR_ZONE_W+2 扣除），折行文本贴窗右缘，
        // 不再留出竖直空白带（水平条在软换行下恒隐藏）
        let mut c = wrap_core("1234567890\n");
        wrap_converge(&mut c);
        let mc = c.wrap_max_cols();
        let expect = (c.text_viewport_w() / c.char_width()).floor().max(1.0) as usize;
        assert_eq!(mc, expect, "开态列预算不得预留滚动条槽位");
        assert!(mc >= 20, "300 宽视窗应有充足列数，实际 {mc}");
    }

    #[test]
    fn wrap_pixel_breaks_fill_right_edge_on_nonuniform_advances() {
        // P96 用户点单根治：真实场景（探针日志：28px 字号、char_w=15.6）
        // 的 CJK 文本在列模型下段尾系统性留白 ~10%（汉字真实 28px 但
        // 列模型按 2×15.6=31.2px 计）——真实字形布局注入后必须按像素
        // 断行贴满右缘。本测试注入与真实场景同形的 xs（每汉字 28px）。
        let mut c = core_with(&"汉".repeat(40));
        c.set_viewport_width(1024.0);
        c.set_viewport_height(600.0);
        c.set_word_wrap(true);
        // 注入「真实字形」行布局：40 汉字 = 1120px，每字 28px
        let xs: Vec<f32> = (0..=40).map(|i| i as f32 * 28.0).collect();
        c.set_row_layout(0, xs.clone());
        let max_px = c.text_viewport_w(); // 1024 − gutter(49) = 975
        let breaks = c.segments_of_line(0, &c.line_text(0));
        // 段 0 恰容纳 floor(975/28)=34 个字（952 ≤ 975）
        assert_eq!(*breaks, vec![0, 34], "像素断行段 0 应到第 34 字符");
        let seg0_end_px = xs[34] - xs[0];
        assert!(
            max_px - seg0_end_px < 28.0,
            "段尾缺 {}px ≥ 一字符宽——未贴满右缘",
            max_px - seg0_end_px
        );
        // 断点全部尊重像素预算：任一段右缘 ≤ max_px（列模型对 28px 汉字
        // 按 2×char_w 计会超/欠预算，像素路径不受影响）
        let mut prev = 0usize;
        for &b in breaks.iter().skip(1) {
            assert!(xs[b] - xs[prev] <= max_px);
            prev = b;
        }
        assert!(xs[40] - xs[prev] <= max_px, "末段右缘不得超预算");
        // 视觉映射与像素断点自洽：40 字 → 2 段（无尾随换行 → 无幻影行）
        assert_eq!(c.line_visual_segments(0), 2);
        assert_eq!(c.visual_rows_total(), 2);
        // 光标/命中按段内真实 x 工作（段 1 起点 = 字符 34 → 像素 x 952）
        assert_eq!(c.visual_row_of(0, 35), 1);
        let gutter = c.gutter_width();
        let hit = c.hit_test(gutter + 28.0 * 5.0, c.line_height() * 1.5);
        assert_eq!((hit.line, hit.col), (0, 39), "段 1 内命中第 5 个字 = 字符 39");
    }

    #[test]
    fn wrap_sb_reserve_shrinks_budget_only_when_set() {
        // P99 用户点单：折行文本贴满右缘后行尾字符被垂直滚动条盖住。
        // 修复 = 内容超出视口（滚动条 needed）时折行预算扣除滚动条
        // 可视带宽；放得下（无滚动条）时零预留全宽贴边（P95 口径
        // 不回归）。
        let mut c = wrap_core("1234567890\n");
        wrap_converge(&mut c);
        assert!(!c.wrap_sb_reserve, "默认零预留（P95 贴边口径）");
        let full = c.text_viewport_w();
        assert!((c.wrap_max_px() - full).abs() < 0.01);
        assert_eq!(
            c.wrap_max_cols(),
            (full / c.char_width()).floor().max(1.0) as usize
        );

        // 需要滚动条：预算 = 可视宽 − 滚动条可视带宽（≥4px 防御不变）
        c.set_wrap_sb_reserve(true);
        assert!(c.wrap_sb_reserve);
        let reserved = (full - VERTICAL_SCROLLBAR_RESERVE).max(4.0);
        assert!(
            (c.wrap_max_px() - reserved).abs() < 0.01,
            "预留后像素预算必须扣除滚动条带宽"
        );
        assert_eq!(
            c.wrap_max_cols(),
            (reserved / c.char_width()).floor().max(1.0) as usize,
            "列模型回退与像素预算同口径"
        );
    }

    #[test]
    fn wrap_sb_reserve_rebreaks_at_smaller_budget() {
        // P99 像素口径：预留后断点按更小预算重算——每段右缘 ≤
        // 「文本区宽 − 滚动条带」，行尾字符整体在滑块左侧收尾。
        // （真实字形场景：15.6px 半宽 → 975px 全宽 62 字符/段、
        // 962px 预留 61 字符/段。）
        let mut c = core_with(&"a".repeat(100));
        c.set_viewport_width(1024.0);
        c.set_viewport_height(600.0);
        c.set_word_wrap(true);
        let xs: Vec<f32> = (0..=100).map(|i| i as f32 * 15.6).collect();
        c.set_row_layout(0, xs.clone());
        let full = c.text_viewport_w();
        let breaks_full = c.segments_of_line(0, &c.line_text(0));
        assert_eq!(breaks_full[1], 62, "全宽预算 62 字符/段（967.2 ≤ 975）");
        c.set_wrap_sb_reserve(true);
        let breaks = c.segments_of_line(0, &c.line_text(0));
        assert!(
            breaks.len() >= breaks_full.len(),
            "预留后段宽更小：断点数不得少于全宽"
        );
        assert!(breaks[1] <= breaks_full[1], "首段断点不晚于全宽");
        assert_eq!(breaks[1], 61, "预留预算 61 字符/段（951.6 ≤ 962）");
        // 每段右缘（含末段）都不越出预留预算
        let mut prev = 0usize;
        for &b in breaks.iter().skip(1) {
            assert!(
                xs[b] - xs[prev] <= full - VERTICAL_SCROLLBAR_RESERVE + 0.01,
                "段右缘不得越出预留预算"
            );
            prev = b;
        }
        assert!(
            xs[100] - xs[prev] <= full - VERTICAL_SCROLLBAR_RESERVE + 0.01,
            "末段右缘不得越出预留预算"
        );
    }

    #[test]
    fn wrap_stale_row_layout_on_document_swap_must_not_pin_memo() {
        // boot 折行事故（2026-08）：带文件启动时 CJK 长行整行不折、
        // 滚动不自愈（编辑/最大化才自愈）。链路 = 换文档帧的 layout
        // 先经 visible_range→locate_visual 用**上一帧布局**（此处手工
        // 注入 1 项脏 xs 模拟空文档残留）调 set_line 写 memo；原
        // locate_visual 不做长度校验，1 项 xs 被 pixel_breaks 截成
        // 「整行一段」固化——之后 xs 注入恢复也不自愈（memo 只认
        // gen）。修复 = 调用点统一 trusted_xs 对齐校验 + memo 记录
        // 断行路径（列模型↔像素互切即重算）。
        let mut c = core_with(&"汉".repeat(360));
        c.set_viewport_width(1024.0);
        c.set_viewport_height(600.0);
        c.set_word_wrap(true);
        // 换文档帧的脏布局：上一帧空文档的 line 0 布局 xs = [0.0]（1 项）
        c.set_row_layout(0, vec![0.0]);
        // layout 阶段的反解（visible_range 内部走 locate_visual）
        let _ = c.visible_range();
        // 本帧注入恢复正确布局：360 字 × 16px = 5760px ≫ 预算 975
        let xs: Vec<f32> = (0..=360).map(|i| i as f32 * 16.0).collect();
        c.set_row_layout(0, xs);
        assert!(
            c.line_visual_segments(0) > 1,
            "脏 xs 固化 memo：换文档帧后 CJK 长行整行不折"
        );
        assert!(c.visual_rows_total() > 1, "总视觉行数未随真实布局收敛");
    }

    #[test]
    fn wrap_motion_vertical_uses_visual_rows_and_goal_column() {
        // 行 0：50 字符 → 2 段；行 1：70 字符 → 3 段。总视觉行 5。
        let mut c = wrap_core(&format!("{}\n{}\n", "a".repeat(50), "a".repeat(70)));
        wrap_converge(&mut c);
        let mc = c.wrap_max_cols();
        assert!((20..=30).contains(&mc));
        assert_eq!(c.visual_rows_total(), 2 + 3 + 1, "行 0 两段 + 行 1 三段 + 幻影");
        c.cursor = CursorPos { line: 0, col: 40 };
        // Down：视觉行 1 → 2（行 1 段 0）。goaI = 40 列像素，远超段尾
        // 25 列 → 钳到段末字符格（mc−1，不跳到下一视觉段）
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: mc - 1 });
        // goal 保持：再 Down → 行 1 段 1 [mc, 2mc)，goaI 落在列 40
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 40 });
        // goal 越段 2 起点 → 段起点 (2mc)
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 2 * mc });
        // 再 Down → 幻影行（与关态同口径：末行之后还有可导航的空行）
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 2, col: 0 });
        // 末视觉行再 Down 不动
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 2, col: 0 });
        // 原路返回：goal 保持 → (1,2mc) → (1,40) → (1,mc−1) → (0,40)
        c.apply_motion(Motion::Up, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 2 * mc });
        c.apply_motion(Motion::Up, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 40 });
        c.apply_motion(Motion::Up, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: mc - 1 });
        c.apply_motion(Motion::Up, false);
        assert_eq!(c.cursor, CursorPos { line: 0, col: 40 });
        // 往返闭环：再 Down → (1, seg0) 段尾钳制（goal 仍 40 列像素）
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: mc - 1 });
        // 非竖向操作清 goal：Left 立即清（断言在 Down 之前）；随后 Down 的
// 新 goal（23 列像素）落在目标段起点（25 列）之前 → 吸附段首
        c.apply_motion(Motion::Left, false);
        assert_eq!(c.cursor.col, mc - 2);
        assert_eq!(c.goal_px, None, "Left 后 goal 已清");
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: mc });
        // 关态竖向移动仍是逻辑行口径（回归）
        c.set_word_wrap(false);
        c.cursor = CursorPos { line: 0, col: 40 };
        c.apply_motion(Motion::Down, false);
        assert_eq!(c.cursor, CursorPos { line: 1, col: 40 });
    }

    #[test]
    fn wrap_hit_test_resolves_visual_row_to_segment_col() {
        let c = wrap_core(&format!("{}\nbb\n", "a".repeat(50)));
        let mc = c.wrap_max_cols();
        let char_w = c.char_width();
        let gutter = c.gutter_width();
        let lh = c.line_height();
        // 视觉行 0（逻辑行 0 段 0）段首
        let hit = c.hit_test(gutter, lh * 0.5);
        assert_eq!((hit.line, hit.col), (0, 0));
        // 视觉行 1 = 逻辑行 0 段 1：续行从文本区**左缘**起排——段首
        // 点击在 x=gutter（P96 左缘模型），段内第 5 字符在 +5 列
        let hit = c.hit_test(gutter, lh * 1.5);
        assert_eq!((hit.line, hit.col), (0, mc), "段 1 首字符（左缘起排）");
        let hit = c.hit_test(gutter + 5.0 * char_w, lh * 1.5);
        assert_eq!((hit.line, hit.col), (0, mc + 5), "段 1 中段第 5 字符");
        // 视觉行 2 = 逻辑行 1
        let hit = c.hit_test(gutter, lh * 2.5);
        assert_eq!((hit.line, hit.col), (1, 0));
        // 段外 x 越右 → 钳到段末（行 50 字符 = 段 1 末列 50）
        let hit = c.hit_test(gutter + 9999.0, lh * 1.5);
        assert_eq!((hit.line, hit.col), (0, 50), "段尾钳制应为行末");
    }

    #[test]
    fn wrap_scroll_clamp_and_ensure_visible_use_visual_rows() {
        // 12 行 × 各 50 字符 → 24 视觉行；视口 3 行
        let doc: String = (0..12)
            .map(|_| "a".repeat(50))
            .collect::<Vec<_>>()
            .join("\n");
        let mut c = core_with(&doc);
        c.set_viewport_width(300.0);
        c.set_viewport_height(3.0 * 22.0);
        c.set_word_wrap(true);
        wrap_converge(&mut c);
        // 12 行 × 2 段（join 无尾随换行 → 无幻影）= 24 视觉行
        assert_eq!(c.visual_rows_total(), 24);
        // clamp 上限 = 24 − 3 = 21（+1 恰与关态「末行留白一行」口径一致）
        c.scroll_top = 9999.0;
        c.clamp_scroll();
        assert_eq!(c.scroll_top, 22.0);
        // ensure_visible：光标 (5,0) 视觉行 10 → scroll_top 收敛 8
        c.cursor = CursorPos { line: 5, col: 0 };
        c.scroll_top = 0.0;
        c.ensure_visible_pub();
        assert_eq!(c.scroll_top, 8.0);
        // visible_range 映射为覆盖视口的逻辑行（first+rows 边距窗口，
        // last=12 → 行 6，与关态口径一致）
        assert_eq!(c.visible_range(), (4, 6));
        // 关态恢复逻辑口径
        c.set_word_wrap(false);
        c.scroll_top = 9999.0;
        c.clamp_scroll();
        assert_eq!(c.scroll_top, 12.0 - 3.0 + 1.0);
    }

    #[test]
    fn wrap_toggle_clears_block_locks_hscroll_and_rejects_block_select() {
        let mut c = core_with("aaaa\nbbbb\n");
        c.set_viewport_width(300.0);
        c.begin_block_select(CursorPos { line: 0, col: 1 });
        c.update_block_select(CursorPos { line: 1, col: 3 });
        assert!(c.active_block().is_some());
        assert!(c.block_dragging);
        // 开启瞬间：清块 + 锁水平滚动
        c.set_word_wrap(true);
        assert!(c.active_block().is_none());
        assert!(!c.block_dragging);
        assert_eq!(c.scroll_left, 0.0);
        c.scroll_left = 100.0;
        c.clamp_scroll_horizontal();
        assert_eq!(c.scroll_left, 0.0, "开态横向钳制恒锁 0");
        // 开态拒绝列块选择（设计 §4.10）
        c.begin_block_select(CursorPos { line: 0, col: 0 });
        assert!(c.active_block().is_none());
        assert!(!c.block_dragging);
        // 关态恢复可用
        c.set_word_wrap(false);
        c.begin_block_select(CursorPos { line: 0, col: 0 });
        assert!(c.block_dragging);
        c.finish_block_select();
    }

    #[test]
    fn wrap_index_survives_edits_via_shared_invalidation() {
        let mut c = wrap_core(&format!("{}\n{}\n", "a".repeat(50), "a".repeat(50)));
        wrap_converge(&mut c);
        let mc = c.wrap_max_cols();
        assert!((20..=30).contains(&mc));
        assert_eq!(c.visual_rows_total(), 2 + 2 + 1, "两行各 2 段 + 幻影 1 段");
        // 行 1 拉长：50 → 56 字符（BIT 差值更新，段数随 mc 派生）
        c.cursor = CursorPos { line: 1, col: 46 };
        c.insert_str("cccccc");
        let segs1 = 56u32.div_ceil(mc as u32);
        assert!(segs1 > 2, "56 字符在 mc=27 时应折 3 段");
        assert_eq!(c.line_visual_segments(1), segs1);
        assert_eq!(c.visual_rows_total(), 2 + segs1 + 1);
        // 撤销（整体换文档，行数不变 → 代次失效；被查询行重算收敛）
        c.undo();
        let line1 = c.line_text(1);
        c.segments_of_line(1, &line1); // draw 同款懒惰收敛
        assert_eq!(c.visual_rows_total(), 2 + 2 + 1);
        // 插入换行 → 行数变化 → after_edit 整表重置，逐行收敛
        c.cursor = CursorPos { line: 1, col: 50 };
        c.insert_str("\n");
        assert_eq!(c.doc.line_count(), 4);
        for l in 0..4 {
            let text = c.line_text(l);
            c.segments_of_line(l, &text);
        }
        assert_eq!(
            c.visual_rows_total(),
            2 + 2 + 1 + 1,
            "行 0/1 各 2 段 + 空行 1 段 + 幻影 1 段"
        );
        // 编辑后的光标可见性按视觉行口径：视口 2 行 → (2,0) 视觉行 4
        // 收敛 scroll_top = 4 − 2 + 1 = 3
        c.set_viewport_height(2.0 * 22.0);
        c.scroll_top = 0.0;
        c.cursor = CursorPos { line: 2, col: 0 };
        c.ensure_visible_pub();
        assert_eq!(c.scroll_top, 3.0);
    }

