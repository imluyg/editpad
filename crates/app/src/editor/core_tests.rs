//! EditorCore 单测（P68 自 editor.rs tests 拆出；经 #[path] 挂为 core
//! 子模块，保留对私有字段的直接访问）。
use super::*;
// P9 断言用；非测试代码只经 Document::line_ending() 间接接触该类型
use editpad_core::LineEnding;
// 横层引用（P33 字体候选/P59 诊断构造等）经 editor 模块再导出取用
use super::super::*;
use iced::advanced::text as core_text;
use iced::advanced::text::Paragraph as _;
use iced::alignment;
use iced::{Pixels, Size};
use super::super::metrics::{is_wide, shape_row_xs, TAB_STOP_COLS};
use super::super::scrollbars::{
    HScrollbar, THUMB_MIN_H, THUMB_MIN_W, VScrollbar,
};

    fn core_with(text: &str) -> EditorCore {
        let mut c = EditorCore::default();
        c.reset_document(Document::from_str(text));
        c
    }

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

    // ---------- P37 打字成组撤销 ----------

    #[test]
    fn typing_run_merges_consecutive_single_chars_into_one_undo() {
        let mut c = core_with("");
        for ch in ["a", "b", "c"] {
            c.insert_str(ch);
        }
        assert_eq!(c.doc.to_text(), "abc");
        // 三次连续单字符输入 = 一个撤销组：一步退回输入前
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");
        assert!(!c.undo(), "组内只有一份快照，一步即到输入前");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "abc", "重做整体恢复该组");
    }

    #[test]
    fn typing_run_breaks_on_cursor_motion() {
        let mut c = core_with("");
        c.insert_str("a");
        c.insert_str("b");
        c.apply_motion(Motion::Left, false); // 光标移动打断成组
        c.insert_str("c");
        assert_eq!(c.doc.to_text(), "acb");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "ab", "移动后的输入独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "", "移动前的连续输入是另一组");
    }

    #[test]
    fn typing_run_breaks_on_newline_paste_and_selection_replace() {
        // 换行：归一后即使只有 \n 也不并入组
        let mut c = core_with("");
        c.insert_str("a");
        c.insert_str("\n");
        c.insert_str("b");
        assert_eq!(c.doc.to_text(), "a\nb");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a\n", "b 独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a");

        // 粘贴（多字符）不参与成组；其后紧邻的单字符输入开新组
        let mut c = core_with("");
        c.insert_str("hello");
        c.insert_str("!");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "hello", "! 是独立组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");

        // 选区替换：消费选区的插入永远开新组。
        // 状态序列 xyz → x-z → x-!z：撤一次只撤掉 !（独立成组），
        // 再撤才回到原始 xyz
        let mut c = core_with("xyz");
        c.select_span(0, 1, 1); // 选中 y —— 选区变更本身打断组
        c.replace_selection("-");
        c.insert_str("!");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "x-z", "! 独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "xyz");
    }

    #[test]
    fn typing_run_survives_scrolling_but_not_focus_loss_or_undo() {
        let mut c = core_with("seed\n");
        c.cursor = CursorPos { line: 0, col: 4 }; // 行尾起打
        c.insert_str("a");
        c.scroll_by_lines(2.0); // 滚动不动光标与文档，不打断
        c.insert_str("b");
        assert_eq!(c.doc.to_text(), "seedab\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seed\n", "滚动不打断成组：ab 一起撤销");

        // 失焦打断
        c.insert_str("c");
        c.pointer_focus(false);
        c.insert_str("d");
        assert_eq!(c.doc.to_text(), "seedcd\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "失焦后的输入独立成组");

        // 撤销本身打断：ef 连续输入并作一组，一步退净且不混入更早历史；
        // undo 之后的新输入是全新一组——重做链被新快照清空，再退只撤掉自己
        c.insert_str("e");
        c.insert_str("f");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "ef 同组一起撤销");
        c.insert_str("g");
        assert_eq!(c.doc.to_text(), "seedcg\n", "undo 后的输入开新组");
        assert!(
            c.redo_stack.is_empty(),
            "undo 后的新编辑必须作废被撤销的重做链"
        );
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "只撤掉 g 本身");
    }

    #[test]
    fn undo_stack_capacity_bumps_to_512_groups() {
        let mut c = core_with("");
        for i in 0..600 {
            c.insert_str(&format!("{}", i % 10));
            // 每次移动打断成组 → 每个字符各自成组，撑爆容量上限
            c.apply_motion(Motion::Left, false);
            c.apply_motion(Motion::Right, false);
        }
        assert_eq!(c.undo_stack.len(), 512, "容量上限提升到 512 组");
        for _ in 0..512 {
            assert!(c.undo());
        }
        // 容量淘汰的已知取舍：最旧的 88 组（第 1~88 次插入）已被挤出，
        // 能退到的最早状态 = 第 89 次插入前的快照（88 字符），不是初始空文档
        assert_eq!(
            c.doc.to_text(),
            "0123456789".repeat(8) + "01234567",
            "淘汰后最早退到第 89 组快照，而非空文档"
        );
        assert!(!c.undo(), "栈已耗尽");
    }

    // ---------- P38 落盘基线：dirty 如实反映与磁盘的差异 ----------

    #[test]
    fn saved_baseline_tracks_save_and_revert_cycle() {
        let mut c = core_with("");
        // 初始基线 = 空文档（Default 即登记）
        assert!(c.is_at_saved_content(), "初始空内容应在基线上");

        c.insert_str("hello");
        assert!(!c.is_at_saved_content());
        // 模拟落盘成功：当前内容成为新基线
        c.mark_saved();
        assert!(c.is_at_saved_content());

        // 基线之后继续编辑 → 偏离；撤销回基线 → 回到与磁盘一致
        c.insert_str("!");
        assert!(!c.is_at_saved_content());
        assert!(c.undo());
        assert!(c.is_at_saved_content(), "撤销回基线应判定与磁盘一致");
        assert!(c.redo());
        assert!(!c.is_at_saved_content(), "重做离开基线");
    }

    #[test]
    fn reset_document_installs_new_baseline() {
        let mut c = core_with("");
        c.insert_str("stale");
        c.mark_saved();
        // 换文档（加载/放弃重置）：基线必须跟着换，不能沿用旧文档
        c.reset_document(editpad_core::Document::from_str("fresh\r\ndoc"));
        assert!(c.is_at_saved_content());
        c.insert_str("x");
        assert!(!c.is_at_saved_content());
        assert!(c.undo());
        assert!(c.is_at_saved_content());
        // 旧基线不得复活：撤销后内容是新文档原文，不是 "stale"
        assert_eq!(c.doc.to_text(), "fresh\r\ndoc");
    }

    #[test]
    fn cleared_baseline_is_conservative_until_marked_saved_again() {
        let mut c = core_with("");
        c.insert_str("快照恢复出的草稿");
        // P30 恢复路径：快照内容 ≠ 磁盘内容，基线必须清空
        c.clear_saved_baseline();
        assert!(!c.is_at_saved_content(), "无基线时必须保守判定为偏离");

        // 清空基线后撤销/重做往返：一律保守置脏，绝不误判「已与磁盘一致」
        c.insert_str("!");
        assert!(c.undo());
        assert!(!c.is_at_saved_content());
        assert!(c.redo());
        assert!(!c.is_at_saved_content());

        // 真正落盘一次后基线重建，回清能力恢复
        c.mark_saved();
        assert!(c.is_at_saved_content());
        c.insert_str("x");
        assert!(c.undo());
        assert!(c.is_at_saved_content());
    }

    #[test]
    fn select_span_and_replace_selection() {
        let mut c = core_with("你好世界");
        c.select_span(0, 1, 2); // 选中 “好世”
        assert_eq!(c.selected_text().as_deref(), Some("好世"));
        c.replace_selection("-");
        assert_eq!(c.doc.to_text(), "你-界");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    }

    #[test]
    fn hit_test_maps_to_visible_columns() {
        let mut c = core_with("abcdef\nxy\n");
        c.set_viewport_height(200.0);
        let gutter = c.gutter_width();

        let hit = c.hit_test(gutter + 1.0, 1.0 * c.line_height());
        assert_eq!(hit, CursorPos { line: 1, col: 0 });

        // 超出该行尾的点击被夹紧到行尾
        let hit = c.hit_test(gutter + 100.0 * c.char_width(), 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 6 });
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
                let w = char_cols(*ch, model_cols as usize) as f32;
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
        for col in 0..xs.len() {
            c.cursor = CursorPos { line: 0, col };
            let caret = c.caret_rect_relative();
            assert!(
                (caret.x - (gutter + xs[col])).abs() < 1e-3,
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

/// 渲染一帧编辑器画布（P66 对拍脚手架）：全新 Renderer/Tree，
/// 控件摆在 (50,60) 尺寸 600×300，白色底。

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
        assert_eq!(
            HScrollbar::measure(c.content_width_px(), view, 800.0, 0.0).needed,
            false,
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
        assert_eq!(
            HScrollbar::measure(c.content_width_px(), view, 800.0, 0.0).needed,
            false
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
        assert_eq!(
            HScrollbar::measure(c2.content_width_px(), c2.text_viewport_w(), 800.0, 0.0).needed,
            true,
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

    // ---------- P2 输入法焦点过滤 ----------
    #[test]
    fn ime_events_are_gated_by_focus() {
        let mut c = core_with("");
        assert!(c.focused, "默认持有焦点（正文是初始焦点控件）");

        // 有焦点：组字串进入正文显示，上屏文本交由应用层插入
        assert!(c.ime_preedit("nihao".to_owned()));
        assert_eq!(c.preedit.as_deref(), Some("nihao"));
        assert_eq!(c.ime_commit("你好"), ImeCommit::Consumed(Some("你好".into())));
        assert_eq!(c.preedit, None, "上屏后预编辑串必须清掉");

        // 点击查找框（区外）→ 交出焦点；此后 IME 事件一律放行不消费
        c.pointer_focus(false);
        assert!(!c.focused);
        assert!(!c.ime_preedit("pinyin".to_owned()), "未聚焦不得消费预编辑");
        assert_eq!(c.preedit, None);
        assert_eq!(c.ime_commit("字"), ImeCommit::Ignored, "未聚焦时上屏不得进正文");

        // 点回编辑区 → 重新接管焦点，IME 恢复消费
        c.pointer_focus(true);
        assert!(c.ime_preedit("z".to_owned()));
    }

    #[test]
    fn focus_loss_discards_stale_preedit_and_empty_commit_only_clears() {
        let mut c = core_with("");
        c.ime_preedit("wei".to_owned());

        // 组字中途点去别的控件：残留预编辑串不能悬在正文里
        c.pointer_focus(false);
        assert_eq!(c.preedit, None);

        // 空提交（如输入法取消）：只清预编辑、无文本插入，但事件算已消费
        c.pointer_focus(true);
        c.ime_preedit("x".to_owned());
        assert_eq!(c.ime_commit(""), ImeCommit::Consumed(None));
        assert_eq!(c.preedit, None);
        assert_eq!(c.doc.to_text(), "", "空提交不得改动文档");
    }

    // ---------- P9 CRLF 按 EOL 单元处理 ----------

    #[test]
    fn backspace_joins_crlf_lines_in_one_press() {
        let mut c = core_with("ab\r\ncd");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.backspace();
        // 旧实现第一下只删 \r：两行没并上，看起来像按键无反应
        assert_eq!(c.doc.to_text(), "abcd", "一次退格必须删掉整个 \\r\\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
        assert_eq!(c.line_display_len(0), 4, "并行后不得残留孤立 \\r");

        // 撤销一步回到并行前（快照语义不受 EOL 单元影响）
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "ab\r\ncd");
    }

    #[test]
    fn delete_forward_removes_crlf_in_one_press() {
        let mut c = core_with("ab\r\ncd");
        c.cursor = CursorPos { line: 0, col: 2 }; // 行尾
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "abcd", "一次 Delete 必须删掉整个 \\r\\n");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });

        // 文档末尾的单个字符照常单删；越界探测不得 panic
        c.reset_document(Document::from_str("ab\r"));
        c.cursor = CursorPos { line: 0, col: 2 };
        c.delete_forward();
        assert_eq!(c.doc.to_text(), "ab", "孤立尾部 \\r 单字符删除");
    }

    #[test]
    fn backspace_still_deletes_single_char_within_line() {
        let mut c = core_with("abc\r\ndef");
        c.cursor = CursorPos { line: 0, col: 3 }; // 行尾 'c' 之后
        c.backspace();
        assert_eq!(c.doc.to_text(), "ab\r\ndef", "行内退格不得误删行尾 CRLF");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    }

    #[test]
    fn enter_and_paste_follow_dominant_line_ending() {
        // CRLF 文档：回车（插 \n）与粘贴的混合行尾都归一为 \r\n
        let mut c = core_with("a\r\nb");
        c.cursor = CursorPos { line: 0, col: 1 }; // 'a' 之后
        c.insert_str("\n");
        assert_eq!(c.doc.to_text(), "a\r\n\r\nb", "回车应插入主导行尾 CRLF");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf);

        // 此时光标在第 1 行行首；粘贴混合行尾文本统一改写为主导行尾
        c.insert_str("剪贴板\r\n来了\n多行");
        assert_eq!(
            c.doc.to_text(),
            "a\r\n剪贴板\r\n来了\r\n多行\r\nb",
            "粘贴的 LF/CRLF 应统一改写为主导行尾"
        );
        assert_eq!(c.cursor, CursorPos { line: 3, col: 2 }, "光标推进按 EOL 单元计列");

        // LF 文档：CRLF 粘贴归一为 \n，行为与旧版一致
        let mut lf = core_with("");
        lf.insert_str("x\r\ny\rz");
        assert_eq!(lf.doc.to_text(), "x\ny\nz", "LF 文档把 CRLF/孤立 CR 归一为 LF");
        assert_eq!(lf.cursor, CursorPos { line: 2, col: 1 });

        // 无换行内容不产生任何改动（光标先移到文末再插入）
        let mut plain = core_with("plain");
        plain.cursor = CursorPos { line: 0, col: 5 };
        plain.insert_str("中文🚀");
        assert_eq!(plain.doc.to_text(), "plain中文🚀");
    }

    #[test]
    fn crlf_document_keeps_eol_through_undo() {
        let mut c = core_with("l1\r\nl2\r\n");
        c.cursor = CursorPos { line: 1, col: 2 }; // 行尾
        c.insert_str("\n");
        assert_eq!(c.doc.to_text(), "l1\r\nl2\r\n\r\n", "行尾回车追加一个 CRLF");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "l1\r\nl2\r\n", "撤销完整还原");
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "快照携带同一行尾元数据");
    }

    // ---------- P12 高亮后台分批补建 ----------

    #[test]
    fn needs_paving_tracks_visible_deficit() {
        let mut c = core_with(&(0..600).map(|i| format!("fn f{i}() {{}}\n")).collect::<String>());
        c.set_language(Some("rs"));
        c.set_viewport_height(400.0);
        c.clamp_scroll();
        assert!(
            !c.needs_paving(),
            "顶部可见区落在初始检查点预算内，无需后台铺建"
        );

        // 跳到文末：可见区末行 599 需 5 个档位、只有初始 1 个 → 必须请求铺建
        c.scroll_top = 100_000.0;
        c.clamp_scroll();
        assert!(c.needs_paving(), "大跳转后的可见区缺档必须被识别");

        // 纯文本路径（无高亮器）恒不需要
        let plain = core_with(&"x\n".repeat(600));
        assert!(!plain.needs_paving());
    }

    #[test]
    fn pave_snapshot_and_conditional_install_flow() {
        let mut c = core_with(&(0..600).map(|i| format!("let a{i} = {i};\n")).collect::<String>());
        c.set_language(Some("rs"));

        // 起点快照 + 后台式推进
        let (gen, snapshot) = c.highlight_pave_snapshot().expect("已启用高亮");
        assert_eq!(c.highlight_generation(), Some(gen));
        let mut worker = snapshot.clone();
        let built = worker.advance_checkpoints(32, 600, &mut |i| format!("let w{i} = {i};"));
        assert_eq!(built, 3, "600 行的完整档位起点为 128/256/384，共 3 个");
        assert!(worker.checkpoints_len() > snapshot.checkpoints_len());

        // 代次一致 → 安装成功且状态生效
        assert!(c.install_highlighter_if_current(gen, worker.clone()));
        assert_eq!(
            c.highlight_checkpoints_len(),
            Some(worker.checkpoints_len()),
            "安装后检查点应与成果一致"
        );

        // 代次不符（模拟安装前发生过编辑）→ 整体拒绝
        c.cursor = CursorPos { line: 599, col: 0 };
        c.insert_str("x"); // 真实编辑：内部 invalidate_from → 换代
        let stale_gen = gen;
        assert_ne!(c.highlight_generation(), Some(stale_gen), "编辑必须换代");
        assert!(
            !c.install_highlighter_if_current(stale_gen, worker),
            "换代后的迟到成果不得覆盖当前高亮器"
        );
    }

    // ---------- P14 Tab 输入与制表位列宽 ----------

    #[test]
    fn tab_widths_advance_to_next_tab_stop() {
        // 'a'=1 列；Tab 在第 1 列推进到第 4 列（占 3）；'b'=1 → 共 5
        assert_eq!(display_cols("a\tb"), 5.0);
        // Tab 恰在制表位上：至少仍占 1 列（"abc\t" = 3+1）
        assert_eq!(display_cols("abc\td"), 5.0);
        assert_eq!(display_cols("\t"), TAB_STOP_COLS as f32);

        // prefix_width 按字符索引取前缀宽：过 Tab 处发生跳跃
        assert_eq!(prefix_width("a\tb", 1), 1.0, "只有 'a'");
        assert_eq!(prefix_width("a\tb", 2), 4.0, "'a'+Tab 应到第 4 制表位");
        assert_eq!(prefix_width("a\tb", 3), 5.0, "再加 'b'");

        // 与像素换算单调一致（caret/选区/着色段共用此路径）
        let cw = EditorCore::default().char_width();
        for i in 0..=4 {
            let a = prefix_width("a\tb", i) * cw;
            let b = prefix_width("a\tb", (i + 1).min(3)) * cw;
            assert!(b >= a, "前缀宽必须随索引单调不减");
        }
    }

    #[test]
    fn tab_inserts_as_literal_char_with_tab_aware_caret() {
        // Tab 是真实字符进文档：保存往返不失真，光标按字符推进 1
        let mut c = core_with("");
        c.insert_str("\t");
        assert_eq!(c.doc.to_text(), "\t", "文档存真实 \\t 字符");
        assert_eq!(c.cursor.col, 1);

        // 光标矩形按制表位宽度落位（第 1 个字符之后 = 第 4 显示列）
        let expect_x = c.gutter_width() + prefix_width("\t", 1) * c.char_width();
        assert!((c.caret_rect_relative().x - expect_x).abs() < 1e-4);

        // 行首 Tab 后再打字：后续列宽从制表位起算（hit_test 往返验证）
        c.cursor = CursorPos { line: 0, col: 1 };
        c.insert_str("xy");
        assert_eq!(c.doc.to_text(), "\txy");
        let hit = c.hit_test(
            c.gutter_width() + prefix_width("\txy", 3) * c.char_width() + 1.0,
            0.0,
        );
        assert_eq!(hit.col, 3, "像素位置反查字符索引必须与正向宽度一致");
    }

    #[test]
    fn wide_line_cols_count_tab_expansion_for_hscroll_range() {
        // 含 Tab 的行：水平行程按展开后的显示列计（P13×P14 协同）
        let mut c = core_with(&format!("\t{}\n", "x".repeat(100)));
        c.set_viewport_width(400.0);
        assert_eq!(
            c.max_line_display_cols(),
            TAB_STOP_COLS + 100,
            "行首 Tab 占 4 列，高水位应按展开后计"
        );
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
        assert!(HScrollbar::measure(0.0, 400.0, 800.0, 0.0).needed == false);

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
        assert!(sb.hits(798.0, 300.0, 800.0) == false || !sb.needed);
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

    // ---------- 第 57 轮：行操作套件 ----------

    #[test]
    fn delete_current_line_removes_whole_line_and_is_undoable() {
        let mut c = core_with("alpha\nbeta\ngamma");
        c.cursor = CursorPos { line: 1, col: 2 }; // 行中任意列都删整行
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "alpha\ngamma");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 }, "光标落到删除起点");
        assert!(c.undo(), "行删除必须可撤销");
        assert_eq!(c.doc.to_text(), "alpha\nbeta\ngamma");
    }

    #[test]
    fn delete_current_line_handles_crlf_tail_and_phantom_last_line() {
        // CRLF 文档按整行（含 \r\n 单元）删除，不留孤立 \r
        let mut c = core_with("a\r\nb\r\nc");
        c.cursor = CursorPos { line: 0, col: 0 };
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "b\r\nc");
        // 尾行无行尾：删除后前一行行尾保持原样
        c.cursor = CursorPos { line: 1, col: 1 };
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "b\r\n");
        // 幻影末行（尾随行尾产生的空行）：退化为吃掉前面的换行单元
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "b");
        // 只剩一行内容：删成空文档（主流编辑器同款手感）
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "");
    }

    #[test]
    fn delete_on_empty_document_is_noop() {
        let mut c = core_with("");
        assert!(!c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "");
    }

    #[test]
    fn delete_selection_spans_lines_and_col0_end_excludes_last_line() {
        let mut c = core_with("l1\nl2\nl3\nl4");
        // 选区从 l1 中间拉到 l3 行首：l3 不算触及（选区没盖到它的字符）
        c.anchor = Some(CursorPos { line: 0, col: 2 });
        c.cursor = CursorPos { line: 2, col: 0 };
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "l3\nl4");
    }

    #[test]
    fn duplicate_line_inserts_copy_below_and_is_undoable() {
        let mut c = core_with("one\ntwo");
        c.cursor = CursorPos { line: 0, col: 3 };
        assert!(c.duplicate_current_lines());
        assert_eq!(c.doc.to_text(), "one\none\ntwo");
        assert_eq!(c.cursor.line, 1, "光标落到副本首行");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "one\ntwo");
    }

    #[test]
    fn duplicate_tail_line_without_newline_terminates_copy_properly() {
        // 尾块本身无行尾：副本必须独立成行（先补主导行尾）
        let mut c = core_with("one\ntail");
        c.cursor = CursorPos { line: 1, col: 1 };
        assert!(c.duplicate_current_lines());
        assert_eq!(c.doc.to_text(), "one\ntail\ntail");
        // 空文档 / 幻影末行：无内容可复制
        let mut empty = core_with("");
        assert!(!empty.duplicate_current_lines());
    }

    #[test]
    fn duplicate_last_real_line_of_trailing_newline_doc_no_blank_line() {
        // P80 勘误（第 60 轮书签测试揪出）：文档以换行收尾时，末真实行
        // 的块文本自带行尾，旧判定 end<len 误判「块无行尾」误补行尾，
        // 副本前凭空多出一个空行。钉死正确形态防回退。
        let mut c = core_with("one\ntwo\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        assert!(c.duplicate_current_lines());
        assert_eq!(c.doc.to_text(), "one\ntwo\ntwo\n");
        assert_eq!(c.cursor.line, 2, "光标落到副本首行");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "one\ntwo\n");
        // 末真实行为空行时同样成立（复制出独立空行，而非两个）
        let mut d = core_with("x\n\n");
        d.cursor = CursorPos { line: 1, col: 0 };
        assert!(d.duplicate_current_lines());
        assert_eq!(d.doc.to_text(), "x\n\n\n");
    }

    #[test]
    fn move_line_swaps_with_neighbor_respects_boundaries_and_undo() {
        let mut c = core_with("a\nb\nc");
        c.cursor = CursorPos { line: 1, col: 0 };
        assert!(c.move_current_lines(true));
        assert_eq!(c.doc.to_text(), "b\na\nc");
        assert_eq!(c.cursor.line, 0);
        assert!(!c.move_current_lines(true), "已在顶行不得再动");
        assert!(c.move_current_lines(false));
        assert_eq!(c.doc.to_text(), "a\nb\nc");
        assert!(c.move_current_lines(false));
        assert_eq!(c.doc.to_text(), "a\nc\nb");
        assert!(!c.move_current_lines(false), "已在底行不得再动");
        // 边界 no-op 不产生快照：一步撤销恰好回到三行换位前
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a\nb\nc");
    }

    #[test]
    fn move_multi_line_block_rotates_as_a_unit_preserving_order() {
        let mut c = core_with("p\nq1\nq2\nr");
        c.anchor = Some(CursorPos { line: 1, col: 0 });
        c.cursor = CursorPos { line: 2, col: 1 };
        assert!(c.move_current_lines(true));
        assert_eq!(c.doc.to_text(), "q1\nq2\np\nr", "上移=整块与上一行换位");
        assert!(c.undo());
        assert!(c.move_current_lines(false));
        assert_eq!(c.doc.to_text(), "p\nr\nq1\nq2", "下移=整块与下一行换位");
    }

    #[test]
    fn move_line_rebuilds_with_dominant_crlf_and_keeps_col() {
        let mut c = core_with("abcdef\r\nsecond\r\nc");
        c.cursor = CursorPos { line: 0, col: 3 };
        assert!(c.move_current_lines(false));
        assert_eq!(c.doc.to_text(), "second\r\nabcdef\r\nc", "CRLF 文档换位不产生混合行尾");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 3 }, "光标列尽量保持");
    }

    // ---------- 大小写转换与行首尾清理（第 58 轮） ----------

    #[test]
    fn convert_case_selection_scoped_and_crlf_preserved() {
        let mut c = core_with("abc\r\nDef G");
        // 跨行选区：第一行 col2 起盖到第二行 col3（"c\r\nDef"）
        c.anchor = Some(CursorPos { line: 0, col: 2 });
        c.cursor = CursorPos { line: 1, col: 3 };
        assert!(c.convert_case(CaseKind::Upper));
        // 只有选区内字符变大写；换行单元字节原样保留
        assert_eq!(c.doc.to_text(), "abC\r\nDEF G");
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "主导行尾元数据不变");
        // 转换结果保持选中：起点不动、终点按新偏移落位
        assert_eq!(c.anchor, Some(CursorPos { line: 0, col: 2 }));
        assert_eq!(c.cursor, CursorPos { line: 1, col: 3 }, "终点=新文本结束处（'DEF'后）");
        // 撤销逐字节还原
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "abc\r\nDef G");
    }

    #[test]
    fn convert_case_without_selection_transforms_whole_document_idempotent_no_snapshot() {
        let mut c = core_with("abc\n123 中文\nx");
        c.cursor = CursorPos { line: 1, col: 1 };
        let depth = c.undo_stack.len();
        assert!(c.convert_case(CaseKind::Upper));
        assert_eq!(c.doc.to_text(), "ABC\n123 中文\nX", "数字/CJK 不参与大小写映射");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 1 }, "光标行列尽量保持");
        // 幂等 no-op：再转一次不动、不追加快照
        assert!(!c.convert_case(CaseKind::Upper));
        assert_eq!(c.undo_stack.len(), depth + 1, "幂等调用不产生撤销快照");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "abc\n123 中文\nx");
    }

    #[test]
    fn convert_case_expanding_mapping_keeps_selection_over_result_and_undo_exact() {
        // ß→SS 是 Unicode 全量映射的加长案例：选区终点必须按新长度重算
        let mut c = core_with("aß b");
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 0, col: 2 }; // 选区 = "aß"
        assert!(c.convert_case(CaseKind::Upper));
        assert_eq!(c.doc.to_text(), "ASS b");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 3 }, "终点按 SS 新长度落位");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "aß b", "加长映射撤销后逐字节还原");
        // İ→i̇（i+U+0307 组合点）同样加长，且绝不产生换行
        let mut t = core_with("İX");
        assert!(t.convert_case(CaseKind::Lower));
        assert_eq!(t.doc.to_text(), "i\u{307}x");
        assert_eq!(t.doc.line_count(), 1);
    }

    #[test]
    fn trim_modes_strip_unicode_whitespace_on_whole_document() {
        // 全角空格 U+3000 / NBSP / Tab 都属 Unicode White_Space 口径
        let mut c = core_with("\u{3000}a b\u{a0}\r\n\t中 文  \r\n  尾无行尾 ");
        assert!(c.trim_touched_lines(TrimMode::Trailing));
        assert_eq!(
            c.doc.to_text(),
            "\u{3000}a b\r\n\t中 文\r\n  尾无行尾",
            "去尾不动行首；文档末行无行尾的形态保持"
        );
        // 去首：LF 主导文档重建后统一主导行尾（P73 行操作同哲学）
        let mut c2 = core_with("  x\ty\r\n\t\tz  \n W");
        assert!(c2.trim_touched_lines(TrimMode::Leading));
        assert_eq!(c2.doc.to_text(), "x\ty\nz  \nW");
        // 两端都去
        let mut c3 = core_with("  a  \r\n\tb\t");
        assert!(c3.trim_touched_lines(TrimMode::Both));
        assert_eq!(c3.doc.to_text(), "a\r\nb");
    }

    #[test]
    fn trim_selection_scope_only_touched_lines() {
        let mut c = core_with("  a\r\n  b\r\n  c");
        // 选区从第 1 行行首拉到第 2 行中间：触及行=0..=1，第 3 行不得波及
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 1, col: 2 };
        assert!(c.trim_touched_lines(TrimMode::Leading));
        assert_eq!(c.doc.to_text(), "a\r\nb\r\n  c");
    }

    #[test]
    fn trim_noop_empty_and_phantom_edge_cases() {
        // 空文档：无可清理
        let mut c = core_with("");
        assert!(!c.trim_touched_lines(TrimMode::Both));
        // 全干净文档：no-op 不产快照
        let mut d = core_with("clean\r\ntoo");
        let depth = d.undo_stack.len();
        assert!(!d.trim_touched_lines(TrimMode::Both));
        assert_eq!(d.undo_stack.len(), depth, "幂等调用不产生撤销快照");
        // 幻影末行参与清理但「尾随换行」形态保持
        let mut e = core_with("x  \r\n");
        assert!(e.trim_touched_lines(TrimMode::Trailing));
        assert_eq!(e.doc.to_text(), "x\r\n");
        assert!(e.undo());
        assert_eq!(e.doc.to_text(), "x  \r\n");
    }

    // ---------- 第 59 轮：行排序与去重 ----------

    #[test]
    fn sort_lines_whole_document_both_directions_and_undo() {
        // 码点序大小写敏感：'B'(0x42) < 'a'(0x61)
        let mut c = core_with("pear\napple\nBanana");
        let depth = c.undo_stack.len();
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "Banana\napple\npear");
        assert_eq!(c.doc.line_count(), 3, "行数不变");
        c.cursor = CursorPos { line: 0, col: 1 };
        assert!(c.sort_lines(SortOrder::Descending));
        assert_eq!(c.doc.to_text(), "pear\napple\nBanana", "对已升序文档降序=精确倒转");
        assert_eq!(
            c.cursor,
            CursorPos { line: 0, col: 1 },
            "光标按索引钳回触及块内、列尽量保持"
        );
        // 撤销逐步逐字节还原
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "Banana\napple\npear");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "pear\napple\nBanana");
        assert_eq!(c.undo_stack.len(), depth);
    }

    #[test]
    fn sort_lines_already_sorted_is_idempotent_no_snapshot() {
        let mut c = core_with("a\nb\nc");
        let depth = c.undo_stack.len();
        assert!(!c.sort_lines(SortOrder::Ascending), "已升序不得重复产快照");
        assert_eq!(c.undo_stack.len(), depth);
        // 单行 / 空文档 / 纯幻影：无从排序
        let mut one = core_with("solo");
        assert!(!one.sort_lines(SortOrder::Descending));
        let mut empty = core_with("");
        assert!(!empty.sort_lines(SortOrder::Ascending));
        let mut blank = core_with("\n");
        assert!(!blank.sort_lines(SortOrder::Ascending));
        assert_eq!(blank.doc.to_text(), "\n", "纯幻影文档原样保留");
    }

    #[test]
    fn sort_lines_selection_scope_excludes_outside_lines() {
        let mut c = core_with("zeta\nalpha\nmid\nbeta\nlast");
        // 选区盖到第 2~4 行（0 起 1..=3）：首尾两行不得波及
        c.anchor = Some(CursorPos { line: 1, col: 0 });
        c.cursor = CursorPos { line: 3, col: 2 };
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "zeta\nalpha\nbeta\nmid\nlast");
    }

    #[test]
    fn sort_lines_preserves_trailing_shape_and_rebuilds_dominant_eol() {
        // 文档以换行收尾：幻影末行不参与排序，尾随换行仍留在文档末尾
        let mut c = core_with("b\r\n\r\na\r\n");
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(
            c.doc.to_text(),
            "\r\na\r\nb\r\n",
            "空行为真实内容排到最前，尾随换行形态保持"
        );
        // 无尾随换行的文档末行：排序后仍不以换行收尾
        let mut e = core_with("a\nzz\nb");
        assert!(e.sort_lines(SortOrder::Descending));
        assert_eq!(e.doc.to_text(), "zz\nb\na", "末行无换行的形态保持");
        // 混合行尾经重建归一到主导行尾（P9 同哲学）
        let mut f = core_with("b\r\nA\na\r\n");
        assert!(f.sort_lines(SortOrder::Ascending));
        assert_eq!(f.doc.to_text(), "A\r\na\r\nb\r\n", "混合行尾统一为主导 CRLF");
        assert_eq!(f.doc.line_ending(), LineEnding::CrLf);
    }

    #[test]
    fn sort_orders_by_codepoint_including_emoji_and_cjk() {
        // 4 字节 emoji（U+1F600）> CJK（U+4E2D）> ASCII，按码点一字节不乱
        let mut c = core_with("😀\n中\nA\na");
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "A\na\n中\n😀");
        assert!(c.sort_lines(SortOrder::Descending));
        assert_eq!(c.doc.to_text(), "😀\n中\na\nA");
    }

    #[test]
    fn dedupe_keeps_first_occurrence_preserving_order_and_undo() {
        let mut c = core_with("b\na\nb\nc\na\nb");
        let depth = c.undo_stack.len();
        assert!(c.remove_duplicate_lines());
        assert_eq!(c.doc.to_text(), "b\na\nc", "保留首次出现，其余相对次序不变");
        assert_eq!(c.cursor.line, 0, "光标钳回块内");
        assert_eq!(c.undo_stack.len(), depth + 1, "单次去重只产一个快照");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "b\na\nb\nc\na\nb");
        assert_eq!(c.undo_stack.len(), depth, "撤销后栈深回落");
        // 文档末尾无换行且重复发生在末行：收尾形态保持
        let mut d = core_with("x\nx");
        assert!(d.remove_duplicate_lines());
        assert_eq!(d.doc.to_text(), "x");
    }

    #[test]
    fn dedupe_selection_scope_and_crlf_rebuild() {
        let mut c = core_with("keep\r\nDUP\r\nDUP\r\nDUP\r\nkeep2");
        // 选区只盖中间三行 DUP（1..=3）
        c.anchor = Some(CursorPos { line: 1, col: 1 });
        c.cursor = CursorPos { line: 3, col: 1 };
        assert!(c.remove_duplicate_lines());
        assert_eq!(c.doc.to_text(), "keep\r\nDUP\r\nkeep2", "范围外行不波及，CRLF 整单元重建");
        assert_eq!(c.doc.line_ending(), LineEnding::CrLf, "主导行尾元数据不变");
    }

    #[test]
    fn dedupe_noop_unique_or_single_line_no_snapshot() {
        let mut c = core_with("one\ntwo\nthree");
        let depth = c.undo_stack.len();
        assert!(!c.remove_duplicate_lines(), "无重复不得产快照");
        assert_eq!(c.undo_stack.len(), depth);
        let mut one = core_with("only");
        assert!(!one.remove_duplicate_lines());
        let mut empty = core_with("");
        assert!(!empty.remove_duplicate_lines());
    }

    #[test]
    fn sort_dedupe_on_phantom_line_selection_is_safe_noop() {
        // 幻影排除的守卫路径：选区恰好只有幻影末行 / 实线+幻影行首时
        // 不得整数下溢，一律安全 no-op
        let mut c = core_with("a\n");
        c.cursor = CursorPos { line: 1, col: 0 }; // 光标停在幻影行
        assert!(!c.sort_lines(SortOrder::Ascending));
        assert!(!c.remove_duplicate_lines());
        assert_eq!(c.doc.to_text(), "a\n", "文档原样保留");
        let mut d = core_with("x\ny\n");
        d.anchor = Some(CursorPos { line: 1, col: 0 });
        d.cursor = CursorPos { line: 2, col: 0 }; // 触及块=y 行（幻影行首不算触及）
        assert!(!d.sort_lines(SortOrder::Descending));
        assert!(!d.remove_duplicate_lines());
        assert_eq!(d.doc.to_text(), "x\ny\n");
    }

    #[test]
    fn lone_carriage_return_is_a_line_break_in_ropey_terms() {
        // 主线 A（第 59 轮）口径钉子：审查中曾假设「孤立 \r 是内容、行操作
        // 吞它是缺陷」，实测证伪——ropey 把 `\r\n` / `\n` / 孤立 `\r` 都
        // 视为换行单元，P73/P75 剥离口径正确。本测试钉死该口径防回退。
        // ① 文档以孤立 \r 收尾：多出一个幻影空行，排序不得把它当真实
        //    内容搬到最前；尾随换行形态保持（归一到主导行尾；本文档
        //    CRLF×2 > 孤立\r×1，主导=CRLF）
        let mut c = core_with("c\r\nb\r\na\r");
        assert_eq!(c.doc.line_count(), 4, "孤立 \\r 是换行：其后是幻影空行");
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "a\r\nb\r\nc\r\n", "幻影不参与排序，尾随换行保持");
        // ② 行移动沿用同一剥离口径（P73 原行为不变）
        let mut d = core_with("p\r\nq\r\nz\r");
        d.cursor = CursorPos { line: 2, col: 0 };
        assert!(d.move_current_lines(true));
        assert_eq!(d.doc.to_text(), "p\r\nz\r\nq", "末行以孤立 \\r 收尾=有行尾的普通行");
        // ③ 清理：去首只动行首；混合行尾块经主导行尾重建（P9 口径）
        let mut e = core_with(" x\nabc\r");
        assert!(e.trim_touched_lines(TrimMode::Leading));
        assert_eq!(e.doc.to_text(), "x\nabc\n");
        // ④ 去重逐字节比对行内容（不含换行单元）
        let mut f = core_with("dup\r\nx\r\ndup");
        assert!(f.remove_duplicate_lines());
        assert_eq!(f.doc.to_text(), "dup\r\nx", "去重同样逐字节保留行内容");
    }

    // ---------- 第 60 轮：书签套件 ----------

    #[test]
    fn bookmark_toggle_and_query_basics() {
        let mut c = core_with("alpha\nbeta\ngamma\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        assert!(c.toggle_bookmark());
        assert!(c.is_bookmarked(1));
        assert_eq!(c.bookmarked_lines(), vec![1]);
        // 再按一次 = 摘除
        assert!(!c.toggle_bookmark());
        assert!(!c.is_bookmarked(1));
        assert!(c.bookmarked_lines().is_empty());
        // 多行标记仍按升序返回
        c.cursor = CursorPos { line: 2, col: 0 };
        c.toggle_bookmark();
        c.cursor = CursorPos { line: 0, col: 0 };
        c.toggle_bookmark();
        assert_eq!(c.bookmarked_lines(), vec![0, 2]);
    }

    #[test]
    fn bookmark_next_prev_wrap_and_stay() {
        let mut c = core_with("l0\nl1\nl2\nl3\nl4\n");
        // 无书签：跳转 no-op
        assert!(!c.next_bookmark(true));
        assert_eq!(c.cursor.line, 0);
        c.cursor = CursorPos { line: 1, col: 3 };
        c.toggle_bookmark(); // bm {1}
        c.cursor = CursorPos { line: 3, col: 0 };
        c.toggle_bookmark(); // bm {1,3}
        // 向后：3 → 回绕 1，落行首、清选区
        assert!(c.next_bookmark(true));
        assert_eq!(c.cursor.line, 1);
        assert_eq!(c.cursor.col, 0, "跳转落行首");
        assert!(c.anchor.is_none());
        // 再向后：1 → 3（正常前进）
        assert!(c.next_bookmark(true));
        assert_eq!(c.cursor.line, 3);
        // 向前：3 → 1；再向前：1 → 无更前 → 回绕 3
        assert!(c.next_bookmark(false));
        assert_eq!(c.cursor.line, 1);
        assert!(c.next_bookmark(false));
        assert_eq!(c.cursor.line, 3);
        // 唯一书签恰为当前行：两个方向都原地不动
        c.clear_bookmarks();
        c.cursor = CursorPos { line: 2, col: 1 };
        c.toggle_bookmark(); // bm {2}，光标就在行 2
        assert!(!c.next_bookmark(true));
        assert_eq!(c.cursor.line, 2);
        assert!(!c.next_bookmark(false));
        assert_eq!(c.cursor.line, 2);
        // 清空后再跳 no-op；空集合清除幂等 false
        assert!(c.clear_bookmarks());
        assert!(!c.next_bookmark(true));
        assert!(!c.clear_bookmarks());
    }

    #[test]
    fn bookmark_toggle_enters_undo_history() {
        let mut c = core_with("a\nb\n");
        c.cursor = CursorPos { line: 0, col: 0 };
        c.toggle_bookmark(); // 快照1 {bm:{}} → bm {0}
        c.insert_str("x"); // 快照2 {bm:{0}} → "xa\nb\n"
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark(); // 快照3 {bm:{0}} → bm {0,1}
        assert_eq!(c.bookmarked_lines(), vec![0, 1]);
        // 撤销书签开关：bm 回到 {0}（开关本身可撤销）
        c.undo();
        assert_eq!(c.bookmarked_lines(), vec![0]);
        // 撤销打字：打字快照同样携带书签，bm 保持 {0}
        c.undo();
        assert_eq!(c.bookmarked_lines(), vec![0]);
        assert_eq!(c.doc.to_text(), "a\nb\n");
        // 撤销第一个开关：bm 回到空
        c.undo();
        assert!(c.bookmarked_lines().is_empty());
        assert!(!c.undo(), "撤销栈已空");
    }

    #[test]
    fn copy_marked_lines_order_dominant_eol_and_trailing_newline() {
        let mut c = core_with("one\r\ntwo\r\nthree\r\n"); // CRLF 主导
        c.cursor = CursorPos { line: 2, col: 0 };
        c.toggle_bookmark();
        c.cursor = CursorPos { line: 0, col: 0 };
        c.toggle_bookmark();
        // 升序输出、主导行尾、每行带行尾（粘贴到他处保持整行语义）
        assert_eq!(c.copy_bookmarked_lines().unwrap(), "one\r\nthree\r\n");
        // LF 主导 + emoji/CJK 行正文原样
        let mut d = core_with("🦀\n中\nx\n");
        d.cursor = CursorPos { line: 1, col: 0 };
        d.toggle_bookmark();
        assert_eq!(d.copy_bookmarked_lines().unwrap(), "中\n");
        // 无书签 None；空文档唯一空行被标 = 空正文行仍产行尾
        assert!(core_with("a\n").copy_bookmarked_lines().is_none());
        let mut e = core_with("");
        e.toggle_bookmark();
        assert_eq!(e.copy_bookmarked_lines().unwrap(), "\n");
    }

    #[test]
    fn remove_marked_lines_disjoint_runs_and_undo_restores() {
        let mut c = core_with("a\nb\nc\nd\ne\nf\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark(); // 1
        c.cursor = CursorPos { line: 3, col: 0 };
        c.toggle_bookmark(); // 3（与 4 连续成段）
        c.cursor = CursorPos { line: 4, col: 0 };
        c.toggle_bookmark(); // bm {1,3,4}
        assert!(c.remove_bookmarked_lines());
        assert_eq!(c.doc.to_text(), "a\nc\nf\n", "不连续段整段删除");
        assert!(c.bookmarked_lines().is_empty(), "被删的正是全部书签行");
        assert_eq!(c.cursor.line, 1, "光标落在首个被删段起点");
        // 撤销：文本与书签一并找回
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a\nb\nc\nd\ne\nf\n");
        assert_eq!(c.bookmarked_lines(), vec![1, 3, 4]);
        // 无书签 no-op 不产快照
        assert!(c.clear_bookmarks());
        let depth = c.undo_stack.len();
        assert!(!c.remove_bookmarked_lines());
        assert_eq!(c.undo_stack.len(), depth, "no-op 不得入栈");
    }

    #[test]
    fn remove_marked_lines_crlf_last_real_line_and_phantom() {
        // 末真实行被标：块区间含其行尾，删除后前文的尾随换行保持
        //（与 Ctrl+L 同区间口径）
        let mut c = core_with("1\r\n2\r\n3\r\n");
        c.cursor = CursorPos { line: 2, col: 0 };
        c.toggle_bookmark();
        assert!(c.remove_bookmarked_lines());
        assert_eq!(c.doc.to_text(), "1\r\n2\r\n");
        // 幻影末行单独被标：退化为吃掉前面的换行单元
        let mut d = core_with("x\ny\n");
        d.cursor = CursorPos { line: 2, col: 0 };
        d.toggle_bookmark();
        assert!(d.remove_bookmarked_lines());
        assert_eq!(d.doc.to_text(), "x\ny");
        // 空文档唯一空行被标：无可删
        let mut e = core_with("");
        e.toggle_bookmark();
        assert!(!e.remove_bookmarked_lines());
        assert_eq!(e.doc.to_text(), "");
    }

    #[test]
    fn bookmark_remap_on_typing_newlines_and_selection_replace() {
        // 上方插行：书签下移
        let mut c = core_with("a\nb\nc\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark(); // bm{1}
        c.cursor = CursorPos { line: 0, col: 0 };
        c.insert_str("x\n");
        assert_eq!(c.doc.to_text(), "x\na\nb\nc\n");
        assert_eq!(c.bookmarked_lines(), vec![2]);
        // 下方插行：不动
        c.cursor = CursorPos { line: 4, col: 0 };
        c.insert_str("\n");
        assert_eq!(c.bookmarked_lines(), vec![2]);
        // 跨行选区替换变体 A：选区含终点行首字符（r3 后缀幸存）
        // → 终点行书签并入结果行，被替换内容（r1）的书签消失
        let mut d = core_with("r0\nr1\nr2\nr3\nr4\n");
        d.cursor = CursorPos { line: 1, col: 0 };
        d.toggle_bookmark(); // r1（将被替换掉的内容）
        d.cursor = CursorPos { line: 3, col: 0 };
        d.toggle_bookmark(); // r3（后缀幸存）
        d.cursor = CursorPos { line: 1, col: 0 };
        d.anchor = Some(CursorPos { line: 3, col: 1 });
        d.insert_str("Z");
        assert_eq!(d.doc.to_text(), "r0\nZ3\nr4\n");
        assert_eq!(d.bookmarked_lines(), vec![1], "r3 的书签并入结果行，r1 的随内容消失");
        // 变体 B：选区止于终点行行首（该行不算触及、整行并入结果行）
        // → 其书签同样并入结果行；起点行整行被吃 → 自身书签丢弃
        let mut e = core_with("r0\nr1\nr2\nr3\nr4\n");
        e.cursor = CursorPos { line: 1, col: 0 };
        e.toggle_bookmark(); // r1
        e.cursor = CursorPos { line: 3, col: 0 };
        e.toggle_bookmark(); // r3
        e.cursor = CursorPos { line: 1, col: 0 };
        e.anchor = Some(CursorPos { line: 3, col: 0 });
        e.insert_str("Z");
        assert_eq!(e.doc.to_text(), "r0\nZr3\nr4\n");
        assert_eq!(e.bookmarked_lines(), vec![1], "r3 整行并入结果行 → 书签并入行1；r1 随内容消失");
    }

    #[test]
    fn bookmark_remap_on_line_suite_ops() {
        // 删行（选区跨行）：区间内书签消失，下方上移
        let mut c = core_with("0\n1\n2\n3\n4\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark();
        c.cursor = CursorPos { line: 4, col: 0 };
        c.toggle_bookmark(); // bm{1,4}
        c.cursor = CursorPos { line: 0, col: 0 };
        c.anchor = Some(CursorPos { line: 2, col: 1 });
        assert!(c.delete_current_lines());
        assert_eq!(c.doc.to_text(), "3\n4\n");
        assert_eq!(c.bookmarked_lines(), vec![1], "行1 书签随行消失，行4 上移到行1");
        // 复制行：下方书签下推，副本不带书签
        let mut d = core_with("a\nb\nc\n");
        d.cursor = CursorPos { line: 0, col: 0 };
        d.toggle_bookmark();
        d.cursor = CursorPos { line: 2, col: 0 };
        d.toggle_bookmark(); // bm{0,2}
        assert!(d.duplicate_current_lines());
        assert_eq!(d.doc.to_text(), "a\nb\nc\nc\n");
        assert_eq!(d.bookmarked_lines(), vec![0, 2], "副本不带书签");
        // 移行：块内书签随内容轮转
        let mut e = core_with("p\nq\nr\ns\n");
        e.cursor = CursorPos { line: 2, col: 0 };
        e.toggle_bookmark(); // bm{2}（r）
        e.cursor = CursorPos { line: 1, col: 0 };
        assert!(e.move_current_lines(false)); // q 下移：p r q s
        assert_eq!(e.doc.to_text(), "p\nr\nq\ns\n");
        assert_eq!(e.bookmarked_lines(), vec![1], "r 的书签随内容上移");
        assert!(e.move_current_lines(true)); // r 上移回：p q r s
        assert_eq!(e.doc.to_text(), "p\nq\nr\ns\n");
        assert_eq!(e.bookmarked_lines(), vec![2]);
        // 相邻行书签换入块尾（上移时上行书签落到块尾位置）
        let mut f = core_with("p\nq\nr\n");
        f.cursor = CursorPos { line: 0, col: 0 };
        f.toggle_bookmark(); // bm{0}（p）
        f.cursor = CursorPos { line: 1, col: 0 };
        assert!(f.move_current_lines(true)); // q 上移：q p r
        assert_eq!(f.doc.to_text(), "q\np\nr\n");
        assert_eq!(f.bookmarked_lines(), vec![1], "p 的书签随内容下移");
    }

    #[test]
    fn bookmark_remap_on_sort_and_dedup() {
        // 排序：书签跟随行内容（稳定排序排列反演映射）
        let mut c = core_with("c\na\nb\n");
        c.cursor = CursorPos { line: 0, col: 0 };
        c.toggle_bookmark(); // c 行
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "a\nb\nc\n");
        assert_eq!(c.bookmarked_lines(), vec![2], "书签跟着 c 行走");
        assert!(c.sort_lines(SortOrder::Descending));
        assert_eq!(c.doc.to_text(), "c\nb\na\n");
        assert_eq!(c.bookmarked_lines(), vec![0]);
        // 去重：重复项的书签随行丢弃；首现保留项书签原位
        let mut d = core_with("x\ny\nx\nx\n");
        d.cursor = CursorPos { line: 2, col: 0 };
        d.toggle_bookmark(); // 第二个 x（重复项）
        assert!(d.remove_duplicate_lines());
        assert_eq!(d.doc.to_text(), "x\ny\n");
        assert!(d.bookmarked_lines().is_empty(), "重复项书签随行丢弃");
        let mut e = core_with("x\ny\nx\n");
        e.cursor = CursorPos { line: 0, col: 0 };
        e.toggle_bookmark(); // 首个 x（保留项）
        assert!(e.remove_duplicate_lines());
        assert_eq!(e.doc.to_text(), "x\ny\n");
        assert_eq!(e.bookmarked_lines(), vec![0], "保留项书签原位");
        // 幻影末行（块外）书签不受排序影响
        let mut f = core_with("b\na\n");
        f.cursor = CursorPos { line: 2, col: 0 };
        f.toggle_bookmark(); // 幻影行
        assert!(f.sort_lines(SortOrder::Ascending));
        assert_eq!(f.doc.to_text(), "a\nb\n");
        assert_eq!(f.bookmarked_lines(), vec![2], "幻影行号恒不变");
    }

    #[test]
    fn bookmark_remap_on_merge_ops_and_replace_all() {
        // 回退并行：两行书签取并集落在幸存行
        let mut c = core_with("a\nb\n");
        c.cursor = CursorPos { line: 0, col: 0 };
        c.toggle_bookmark();
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark(); // bm{0,1}
        c.cursor = CursorPos { line: 1, col: 0 };
        c.backspace();
        assert_eq!(c.doc.to_text(), "ab\n", "b 并上行 a，幻影末行保持");
        assert_eq!(c.bookmarked_lines(), vec![0], "并集落在幸存行");
        // 行尾 Delete 吞换行：下一行书签并入当前行
        let mut d = core_with("a\nb\n");
        d.cursor = CursorPos { line: 0, col: 1 };
        d.toggle_bookmark();
        d.delete_forward();
        assert_eq!(d.doc.to_text(), "ab\n");
        assert_eq!(d.bookmarked_lines(), vec![0]);
        // 行中 Delete 不改结构：书签不动
        let mut e = core_with("ab\ncd\n");
        e.cursor = CursorPos { line: 1, col: 0 };
        e.toggle_bookmark();
        e.cursor = CursorPos { line: 0, col: 0 };
        e.delete_forward();
        assert_eq!(e.doc.to_text(), "b\ncd\n");
        assert_eq!(e.bookmarked_lines(), vec![1]);
        // 全部替换：书签清空，撤销经快照找回
        let mut f = core_with("a\nb\n");
        f.cursor = CursorPos { line: 1, col: 0 };
        f.toggle_bookmark();
        f.replace_whole_document(editpad_core::Document::from_str("zz\n"));
        assert!(f.bookmarked_lines().is_empty());
        assert!(f.undo());
        assert_eq!(f.bookmarked_lines(), vec![1], "撤销找回整体替换前的书签");
        // 加载新文档（reset）：书签清空且撤销链已清
        let mut g = core_with("a\nb\n");
        g.cursor = CursorPos { line: 1, col: 0 };
        g.toggle_bookmark();
        g.reset_document(editpad_core::Document::from_str("q\n"));
        assert!(g.bookmarked_lines().is_empty());
    }

    #[test]
    fn bookmark_gutter_reserves_strip_without_moving_line_numbers() {
        // 第 60 轮：gutter = 书签条带 + 间距 + 数字宽。条带加在左侧，
        // 行号右缘 = gutter − GUTTER_MIN 的 P66附 对齐契约不受影响
        //（数字盒右缘相对 gutter 右缘的偏移公式未动）。
        let c = core_with("a\nb\nc\n"); // 行数 1 位 → 取 max(3) 位
        let expect = BOOKMARK_STRIP + GUTTER_MIN + 3.0 * c.char_width();
        assert!((c.gutter_width() - expect).abs() < 1e-3, "gutter 公式漂移");
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

    // ---------- 第 62 轮：行操作扩充（Tab↔空格 / 合并拆分 / 删空行） ----------

    #[test]
    fn tab_space_pure_helpers_match_render_tab_stops() {
        // detab：与渲染同源（TAB_STOP_COLS=4、宽字符计 2 显示列）
        assert_eq!(expand_tabs_in("\t\tX", true), "        X");
        assert_eq!(
            expand_tabs_in(" \tX", true),
            "    X",
            "悬置空格后的 Tab 补齐到下一制表位"
        );
        assert_eq!(expand_tabs_in("a\tb", true), "a\tb", "行首模式不碰行中 Tab");
        assert_eq!(expand_tabs_in("a\tb", false), "a   b", "全部模式按显示列展开");
        assert_eq!(
            expand_tabs_in("中\tX", false),
            "中  X",
            "宽字符占 2 列，Tab 只需补 2 格到列 4"
        );
        // entab：只有「恰好到制表位且攒够 ≥2 格」才收拢成 Tab
        assert_eq!(entab_leading_ws("    X"), "\tX");
        assert_eq!(entab_leading_ws("   X"), "   X", "不足一档保持原样（幂等）");
        assert_eq!(entab_leading_ws("      X"), "\t  X", "收一档、余两格悬置保留");
        assert_eq!(
            entab_leading_ws(" \t X"),
            " \t X",
            "既有 Tab 前后的零星空格不收拢（不破坏既有对齐）"
        );
        assert_eq!(
            entab_leading_ws("\u{3000}X"),
            "\u{3000}X",
            "全角空格等其他空白不参与收拢"
        );
        assert_eq!(entab_leading_ws("  "), "  ", "整行短空白原样");
    }

    #[test]
    fn convert_tabs_spaces_scopes_noop_and_bookmarks() {
        // 有选区只转触及行
        let mut c = core_with("\tX\nY\tZ\n");
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 0, col: 2 };
        assert!(c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
        assert_eq!(c.doc.to_text(), "    X\nY\tZ\n", "范围外行不被波及");
        // 无选区转全文
        assert!(c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
        assert_eq!(c.doc.to_text(), "    X\nY   Z\n");
        // 幂等 no-op 不产快照（第二次已无 Tab）
        let snaps = c.undo_stack.len();
        assert!(!c.convert_tabs_spaces(TabSpaceKind::AllTabsToSpaces));
        assert_eq!(c.undo_stack.len(), snaps, "no-op 不得产快照");
        // 行数不变的恒等映射：书签原位保留
        let mut d = core_with("\ta\nb\n");
        d.cursor = CursorPos { line: 1, col: 0 };
        d.toggle_bookmark();
        assert!(d.convert_tabs_spaces(TabSpaceKind::LeadingTabsToSpaces));
        assert_eq!(d.doc.to_text(), "    a\nb\n");
        assert_eq!(d.bookmarked_lines(), vec![1], "恒等映射书签不动");
        // 空格→制表符：整档收拢、不足一档保持
        let mut e = core_with("    a\n  b\n");
        assert!(e.convert_tabs_spaces(TabSpaceKind::LeadingSpacesToTabs));
        assert_eq!(e.doc.to_text(), "\ta\n  b\n");
    }

    #[test]
    fn merge_lines_block_next_line_and_phantom() {
        // 触及块合成一行：各段 trim 后单空格连接，空白行消失
        let mut c = core_with("alpha\n beta \n\ngamma\ndelta\n");
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 2, col: 1 }; // 触及 0..=2 行
        assert!(c.merge_lines());
        assert_eq!(c.doc.to_text(), "alpha beta\ngamma\ndelta\n");
        // 无选区 = 当前行并入下一行；末行无从并 → no-op
        let mut d = core_with("one\ntwo\nthree");
        d.cursor = CursorPos { line: 1, col: 1 };
        assert!(d.merge_lines());
        assert_eq!(d.doc.to_text(), "one\ntwo three");
        assert!(!d.merge_lines(), "末行没有下一行可并");
        // 幻影末行不参与合并，尾随换行形态保持（光标落行 2 行首才触及
        // 行 0..=1——选区字节半开口径）
        let mut e = core_with("x\ny\n");
        e.anchor = Some(CursorPos { line: 0, col: 0 });
        e.cursor = CursorPos { line: 2, col: 0 };
        assert!(e.merge_lines());
        assert_eq!(e.doc.to_text(), "x y\n");
        // CRLF 文档：主导行尾重建不产混合尾
        let mut f = core_with("a\r\nb\r\n");
        f.cursor = CursorPos { line: 0, col: 0 };
        assert!(f.merge_lines());
        assert_eq!(f.doc.to_text(), "a b\r\n");
    }

    #[test]
    fn merge_lines_bookmark_collapse_and_below_shift() {
        // 并入行的书签随内容消失；撤销经快照整体找回
        let mut c = core_with("a\nbb\nccc\nlow\n");
        c.cursor = CursorPos { line: 2, col: 0 };
        c.toggle_bookmark(); // 将被并入的行
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 2, col: 3 };
        assert!(c.merge_lines());
        assert_eq!(c.doc.to_text(), "a bb ccc\nlow\n");
        assert!(c.bookmarked_lines().is_empty(), "并入行书签丢弃");
        // 块外下方行书签随净减行数上移：low 原行 3 → 行 1
        let mut d = core_with("p\nq\nr\nlow\n");
        d.cursor = CursorPos { line: 3, col: 0 };
        d.toggle_bookmark();
        // 触及块 0..=2（光标列 >0 才「触及」第 2 行——选区字节半开口径）
        d.anchor = Some(CursorPos { line: 0, col: 0 });
        d.cursor = CursorPos { line: 2, col: 1 };
        assert!(d.merge_lines());
        assert_eq!(d.doc.to_text(), "p q r\nlow\n");
        assert_eq!(d.bookmarked_lines(), vec![1], "块外书签按行数增量平移");
        assert!(d.undo());
        assert_eq!(d.bookmarked_lines(), vec![3], "撤销整体回滚");
    }

    #[test]
    fn split_line_cursor_selection_and_noop() {
        // 无选区：光标处断行，光标落后半段行首
        let mut c = core_with("abcd\n");
        c.cursor = CursorPos { line: 0, col: 2 };
        assert!(c.split_line());
        assert_eq!(c.doc.to_text(), "ab\ncd\n");
        assert_eq!(c.cursor, CursorPos { line: 1, col: 0 });
        // 空行正中断行：与回车同款——凭空多出一个空行（不是 no-op）
        let mut d = core_with("a\n\nb\n");
        d.cursor = CursorPos { line: 1, col: 0 };
        assert!(d.split_line());
        assert_eq!(d.doc.to_text(), "a\n\n\nb\n", "原尾随换行保持");
        // 选区独立成行：前后残文各留原行
        let mut e = core_with("one two three\n");
        e.anchor = Some(CursorPos { line: 0, col: 4 });
        e.cursor = CursorPos { line: 0, col: 7 };
        assert!(e.split_line());
        assert_eq!(e.doc.to_text(), "one \ntwo\n three\n");
        assert_eq!(e.cursor, CursorPos { line: 1, col: 0 });
        // CRLF 文档用主导行尾断行
        let mut f = core_with("ab\r\ncd\r\n");
        f.cursor = CursorPos { line: 0, col: 1 };
        assert!(f.split_line());
        assert_eq!(f.doc.to_text(), "a\r\nb\r\ncd\r\n");
    }

    #[test]
    fn split_line_bookmark_shifts_below_block() {
        // 摘出行首片段：块 [0..=0] 由 1 行变 3 行，块外 low 书签 +2
        let mut c = core_with("aXb\nlow\n");
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark();
        c.anchor = Some(CursorPos { line: 0, col: 1 });
        c.cursor = CursorPos { line: 0, col: 2 };
        assert!(c.split_line());
        assert_eq!(c.doc.to_text(), "a\nX\nb\nlow\n");
        assert_eq!(c.bookmarked_lines(), vec![3], "块外书签随行数增量下移");
        assert!(c.undo());
        assert_eq!(c.bookmarked_lines(), vec![1]);
    }

    #[test]
    fn delete_empty_lines_modes_scope_and_bookmarks() {
        // 严格口径只删零字符行；幻影末行不参与、尾随换行保持
        let mut c = core_with("a\n\nb\n \n\n");
        assert!(c.delete_empty_lines(BlankKind::Empty));
        assert_eq!(c.doc.to_text(), "a\nb\n \n", "\" \" 行在严格口径下幸存");
        // 含空白口径把纯空白行一并删掉（删到文档尾：保留末行行尾形态）
        let mut d = core_with("a\n\nb\n \n\n");
        assert!(d.delete_empty_lines(BlankKind::Whitespace));
        assert_eq!(d.doc.to_text(), "a\nb\n");
        // 块夹在内容中间且全删空：必须补一个衔接换行，前后不得粘连
        let mut g = core_with("a\n\n\nb");
        assert!(g.delete_empty_lines(BlankKind::Empty));
        assert_eq!(g.doc.to_text(), "a\nb", "中段全删空补一个衔接符");
        // 选区限定作用域（光标落行 2 行首=字节止于行 1 末，触及 0..=1；
        // 半开口径：光标在行首不「触及」该行本身）
        let mut e = core_with("\n\nkeep\n\n\n");
        e.anchor = Some(CursorPos { line: 0, col: 0 });
        e.cursor = CursorPos { line: 2, col: 0 };
        assert!(e.delete_empty_lines(BlankKind::Empty));
        assert_eq!(e.doc.to_text(), "keep\n\n\n", "文档头空块全删不补衔接符");
        // 书签跟随幸存行压缩上移；无可删时 no-op
        let mut f = core_with("x\n\ny\nz\n");
        f.cursor = CursorPos { line: 2, col: 0 };
        f.toggle_bookmark(); // y 行
        assert!(f.delete_empty_lines(BlankKind::Empty));
        assert_eq!(f.doc.to_text(), "x\ny\nz\n");
        assert_eq!(f.bookmarked_lines(), vec![1], "幸存行书签随压缩上移");
        let snaps = f.undo_stack.len();
        assert!(!f.delete_empty_lines(BlankKind::Whitespace));
        assert_eq!(f.undo_stack.len(), snaps);
    }

    #[test]
    fn selection_display_len_matches_len_chars_semantics() {
        // 单行：纯列差
        let mut c = core_with("hello world\n");
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 0, col: 5 };
        assert_eq!(c.selection_display_len(), Some(5));
        // 跨行 LF：换行计 1——「b+\n + cd+\n + e」= 6
        let mut d = core_with("ab\ncd\nef\n");
        d.anchor = Some(CursorPos { line: 0, col: 1 });
        d.cursor = CursorPos { line: 2, col: 1 };
        assert_eq!(
            d.selection_display_len(),
            Some(6),
            "与查找命中 len_chars 同口径"
        );
        // CRLF：\r\n 整体计 1——「b + \r\n + c」= 3
        let mut e = core_with("ab\r\ncd\r\n");
        e.anchor = Some(CursorPos { line: 0, col: 1 });
        e.cursor = CursorPos { line: 1, col: 1 };
        assert_eq!(e.selection_display_len(), Some(3));
        // 无选区 → None
        let g = core_with("abc");
        assert_eq!(g.selection_display_len(), None);
    }

    #[test]
    fn insert_date_time_format_and_edit_pipeline() {
        let mut c = core_with("log: \n");
        c.cursor = CursorPos { line: 0, col: 5 };
        let stamp = c.insert_date_time();
        // 格式钉子：YYYY-MM-DD HH:MM（16 字符、分隔符位固定）
        assert_eq!(stamp.len(), 16, "{stamp}");
        let b = stamp.as_bytes();
        assert_eq!((b[4], b[7], b[10], b[13]), (b'-', b'-', b' ', b':'));
        assert!(stamp.as_bytes()[..16].iter().all(|&x| x.is_ascii_digit() || x == b'-' || x == b' ' || x == b':'));
        assert_eq!(c.doc.to_text(), format!("log: {stamp}\n"), "插在光标处");
        assert!(!c.undo_stack.is_empty(), "插入是真实编辑，产快照");
        // 撤销完整移除时间戳
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "log: \n");
    }

    #[test]
    fn toggle_line_comment_wrap_unwrap_roundtrip() {
        // 默认语法（未设置语言）→ `//`；空行不参与
        let mut c = core_with("alpha\nbeta\n\ngamma");
        assert!(c.toggle_line_comment());
        assert_eq!(c.doc.to_text(), "// alpha\n// beta\n\n// gamma");
        // 全部已注释 → 去一层（往返幂等）
        assert!(c.toggle_line_comment());
        assert_eq!(c.doc.to_text(), "alpha\nbeta\n\ngamma");
        // 缩进保留、插在首个非空白前；混合块按「非全注释」统一加
        let mut d = core_with("  indented\nplain\n");
        assert!(d.toggle_line_comment());
        assert_eq!(d.doc.to_text(), "  // indented\n// plain\n");
        // 剥层时连紧随的一个空格一起剥，多余空格保留
        let mut e = core_with("// nospace\n//  keeps double\n");
        assert!(e.toggle_line_comment());
        assert_eq!(e.doc.to_text(), "nospace\n keeps double\n");
        // 已是注释态的判定看缩进后前缀：整块已注释才剥
        let mut f = core_with("// a\nb\n");
        assert!(f.toggle_line_comment());
        assert_eq!(f.doc.to_text(), "// // a\n// b\n");
        // 无变化 no-op 不产快照（空文档）
        let mut g = core_with("");
        assert!(!g.toggle_line_comment());
    }

    #[test]
    fn toggle_line_comment_prefix_follows_syntax() {
        // Python → #；行数不变 → 书签原位
        let mut c = core_with("x = 1\ny = 2\n");
        c.set_language_by_name(Some("Python"));
        assert_eq!(c.highlight_syntax_name().as_deref(), Some("Python"));
        c.cursor = CursorPos { line: 1, col: 0 };
        c.toggle_bookmark();
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 1, col: 5 };
        assert!(c.toggle_line_comment());
        assert_eq!(c.doc.to_text(), "# x = 1\n# y = 2\n");
        assert_eq!(c.bookmarked_lines(), vec![1], "恒等映射书签不动");
    }

    #[test]
    fn collect_block_keeps_last_real_row_when_selection_reaches_eof() {        // 第 64 轮勘误回归（🟠 既有缺陷，本测试初版当场暴露）：选区触及
        // 「最后一个真实行」且文档以换行收尾时，该行曾被幻影排除误剔——
        // 排序把 a 行整行吞掉、注释切换只处理了首行。守卫 = 幻影只可能
        // 是 b == line_count()-1 那一行。
        let mut c = core_with("c\nb\na\n");
        c.anchor = Some(CursorPos { line: 0, col: 0 });
        c.cursor = CursorPos { line: 1, col: 1 }; // 触及行 0..=1（a 行未选）
        assert!(c.sort_lines(SortOrder::Ascending));
        assert_eq!(c.doc.to_text(), "b\nc\na\n", "块外行 a 不得被吞");
        // 同一选区跑注释切换：两行都必须加上前缀
        let mut d = core_with("x = 1\ny = 2\n");
        d.set_language_by_name(Some("Python"));
        d.anchor = Some(CursorPos { line: 0, col: 0 });
        d.cursor = CursorPos { line: 1, col: 5 };
        assert!(d.toggle_line_comment());
        assert_eq!(d.doc.to_text(), "# x = 1\n# y = 2\n");
    }

    #[test]
    fn apply_line_block_clamps_cursor_into_new_row_count() {
        // 第 66 轮勘误回归（随机对拍当场抓住）：块被压缩后旧光标行可能
        // 超出新行数域——全空白文档删空后只剩 1 行，旧光标行号必须收敛
        let mut c = core_with("\n\n\n\n");
        c.cursor = CursorPos { line: 3, col: 0 };
        assert!(c.delete_empty_lines(BlankKind::Empty));
        assert_eq!(c.doc.to_text(), "", "全空文档删空行 → 空文档");
        assert_eq!(
            c.cursor,
            CursorPos { line: 0, col: 0 },
            "光标必须收敛进新行数域"
        );
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "\n\n\n\n", "撤销完整还原");
    }

    // ---------- 第 67 轮 ⑮：列块编辑 ----------

    #[test]
    fn block_select_lifecycle_and_geometry() {
        let mut c = core_with("abcdef\ngh\nijklm\n");
        assert!(!c.has_block());
        // 拖拽起点 = 空块
        c.begin_block_select(CursorPos { line: 0, col: 2 });
        assert!(!c.has_block(), "单击不应形成有效块");
        // 正向拖到 (2,4)：矩形 0..2 行 × 2..4 列
        c.update_block_select(CursorPos { line: 2, col: 4 });
        assert!(c.has_block());
        assert_eq!(c.active_block(), Some((0, 2, 2, 4)));
        // 反向拖拽归一化：head(1,1) → 行 0..1 × 列 1..2
        c.begin_block_select(CursorPos { line: 0, col: 2 });
        c.update_block_select(CursorPos { line: 1, col: 1 });
        assert_eq!(c.active_block(), Some((0, 1, 1, 2)));
        // 松开保留有效块；clear 后归零；空块 finish 自动清除
        assert!(c.finish_block_select());
        assert!(c.clear_block());
        assert!(!c.has_block());
        c.begin_block_select(CursorPos { line: 1, col: 1 });
        assert!(!c.finish_block_select(), "空块松开自动清除");
        assert!(!c.has_block());
    }

    #[test]
    fn block_copy_delete_insert_oracles() {
        let doc = "abcdef\ngh\nij";
        // 复制：行0[1..3)="bc"、行1[1..3) 截到行尾="h"
        let mut c = core_with(doc);
        c.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 1 },
            head: CursorPos { line: 1, col: 3 },
        });
        assert_eq!(c.active_block(), Some((0, 1, 1, 3)));
        assert_eq!(c.block_copy_text(), Some("bc\nh".to_owned()));

        // 删除：行0 去 [1..3)="bc"；行1 长 2，c1 截到行尾 → 只去 "h"
        assert!(c.delete_block_content());
        assert_eq!(c.doc.to_text(), "adef\ng\nij");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 1 });
        // 全区间越行（短于 c0）→ no-op 不产快照
        let snaps = c.undo_stack.len();
        c.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 9 },
            head: CursorPos { line: 1, col: 9 },
        });
        assert!(!c.delete_block_content());
        assert_eq!(c.undo_stack.len(), snaps);

        // 插入：逐行把 [c0,c1) **替换**为「Z」（行1 长 2，c1 截到行尾）
        let mut e = core_with("abc\nXY\nW");
        e.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 1 },
            head: CursorPos { line: 1, col: 2 },
        });
        assert!(e.insert_into_block("Z"));
        assert_eq!(e.doc.to_text(), "aZc\nXZ\nW");
        assert_eq!(e.cursor, CursorPos { line: 0, col: 2 });
        assert!(e.undo());
        assert_eq!(e.doc.to_text(), "abc\nXY\nW", "撤销完整还原");

        // 书签不受影响（行数不变）
        let mut f = core_with("ab\ncd\n");
        f.toggle_bookmark(); // 行 0
        f.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 0 },
            head: CursorPos { line: 1, col: 1 },
        });
        assert!(f.delete_block_content());
        assert_eq!(f.bookmarked_lines(), vec![0], "恒等结构书签原位");
    }

    #[test]
    fn block_cleared_by_nav_undo_and_esc_op() {
        let mk = || {
            let mut c = core_with("abcd\n");
            c.block_sel = Some(BlockSel {
                anchor: CursorPos { line: 0, col: 1 },
                head: CursorPos { line: 0, col: 3 },
            });
            c
        };
        // 键盘移动 / 全选 / select_span（查找跳转）/ undo 各自清块
        let mut c = mk();
        c.apply_motion(Motion::Right, false);
        assert!(!c.has_block(), "键盘移动退出块态");
        let mut d = mk();
        d.select_all();
        assert!(!d.has_block());
        let mut e = mk();
        e.select_span(0, 0, 2);
        assert!(!e.has_block());
        let mut g = mk();
        g.insert_str("x"); // 普通插入前 apply_edit 已清（此处直测 core 口径）
        g.block_sel = mk().block_sel;
        assert!(g.undo());
        assert!(!g.has_block(), "撤销清块");
        // clear_block 返回「原本是否有块」（调用方据此决定是否触发刷新）
        let mut h = mk();
        assert!(h.clear_block());
        assert!(!h.clear_block(), "二次清除返回 false");
    }

    #[test]
    fn cursor_offset_is_full_document_char_index() {
        // 第 69 轮状态栏「位置」：全文字符偏移（CRLF 的 \r 计 1）
        let mut c = core_with("ab\ncd\r\nef");
        c.cursor = CursorPos { line: 0, col: 1 };
        assert_eq!(c.cursor_offset(), 1);
        c.cursor = CursorPos { line: 1, col: 2 };
        // 行0 "ab\n"=3 + 行1 "cd" 前 2 = 5
        assert_eq!(c.cursor_offset(), 5);
        c.cursor = CursorPos { line: 2, col: 2 };
        // 3 + ("cd\r\n"=4) + 2 = 9
        assert_eq!(c.cursor_offset(), 9);
        // 文档尾 = 总字符数
        assert_eq!(c.cursor_offset(), c.doc.text_len());
    }

    // ---------- 第 58 轮 主线 A 扩容：随机混合操作不变量 + 撤销重放对拍 ----------

    /// XorShift64（与 crates/core/tests/edit_sequence_fuzz.rs 同款零依赖 PRNG，
    /// 固定种子失败可精确复现）。
    struct XorShift64(u64);

    impl XorShift64 {
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                return 0;
            }
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            (x % n as u64) as usize
        }
    }

    /// 剥掉全部换行字符——大小写对拍口径（换行不在大小写映射表内）。
    fn strip_newlines(s: &str) -> String {
        s.chars().filter(|c| *c != '\r' && *c != '\n').collect()
    }

    /// ropey 行界口径切行，返回不含行尾的各行内容。
    fn rope_line_bodies(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\n' => out.push(std::mem::take(&mut cur)),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(std::mem::take(&mut cur));
                }
                other => cur.push(other),
            }
        }
        out.push(cur);
        out
    }

    /// 每步必跑的结构不变量：光标/锚点在界内、滚动值有限非负、
    /// 书签行号全部落在文档行数域内（第 60 轮）。
    fn assert_structural_invariants(c: &EditorCore) {
        assert!(
            c.cursor.col <= c.line_display_len(c.cursor.line),
            "光标列越界：{:?} 行长 {}",
            c.cursor,
            c.line_display_len(c.cursor.line)
        );
        assert!(c.cursor.line < c.doc.line_count(), "光标行越界");
        if let Some(a) = c.anchor {
            assert!(a.col <= c.line_display_len(a.line), "锚点列越界");
            assert!(a.line < c.doc.line_count(), "锚点行越界");
        }
        assert!(c.scroll_top.is_finite() && c.scroll_top >= 0.0, "垂直滚动非法");
        // 第 60 轮：书签行号界内（任何再映射漂移当场暴露）
        for &l in &c.bookmarks {
            assert!(
                l < c.doc.line_count(),
                "书签行越界：{l} / 行数 {}",
                c.doc.line_count()
            );
        }
    }

    #[test]
    fn random_mixed_ops_preserve_invariants_undo_replays_exactly() {
        // 混入三种行尾、CJK、emoji 与加长映射字符（ß），覆盖新旧行为交互
        const TOKENS: &[&str] = &[
            "a", "Z", "9", "中", "文", "🚀", "ß", " ", "\t", "  ", "\n", "\r\n",
        ];
        let motions = [
            Motion::Left,
            Motion::Right,
            Motion::Up,
            Motion::Down,
            Motion::Home,
            Motion::End,
        ];
        for seed in [1u64, 0xDEAD_BEEF, 0x5EED_1234] {
            let mut rng = XorShift64(seed);
            let mut init = String::new();
            for _ in 0..rng.below(20) + 5 {
                init.push_str(TOKENS[rng.below(TOKENS.len())]);
            }
            let initial = init.clone();
            let mut c = core_with(&init);

            for step in 0..240 {
                let old_text = c.doc.to_text();
                match rng.below(100) {
                    // 插入随机 token（含 CRLF/LF/CJK/emoji/ß）
                    0..=34 => {
                        let text: String = (0..rng.below(3) + 1)
                            .map(|_| TOKENS[rng.below(TOKENS.len())])
                            .collect();
                        c.insert_str(&text);
                    }
                    35..=49 => c.backspace(),
                    50..=59 => c.delete_forward(),
                    60..=74 => {
                        let m = motions[rng.below(motions.len())];
                        c.apply_motion(m, rng.below(4) == 0); // 偶尔带 Shift 成选区
                    }
                    75..=79 => c.select_all(),
                    // 大小写转换：先收拢选区 → 全文档口径，oracle 才有全局闭式解
                    80..=87 => {
                        if c.anchor.is_some() {
                            c.apply_motion(Motion::Left, false);
                        }
                        let kind =
                            if rng.below(2) == 0 { CaseKind::Upper } else { CaseKind::Lower };
                        let expected = match kind {
                            CaseKind::Upper => strip_newlines(&old_text).to_uppercase(),
                            CaseKind::Lower => strip_newlines(&old_text).to_lowercase(),
                        };
                        let _changed = c.convert_case(kind);
                        assert_eq!(
                            strip_newlines(&c.doc.to_text()),
                            expected,
                            "seed={seed} step={step} 大小写对拍发散"
                        );
                        assert_eq!(
                            c.doc.line_count(),
                            rope_line_bodies(&old_text).len(),
                            "seed={seed} step={step} 大小写转换改变行数"
                        );
                    }
                    // 行首尾清理：同上收拢为全文档口径，逐行闭式对拍
                    88..=94 => {
                        if c.anchor.is_some() {
                            c.apply_motion(Motion::Left, false);
                        }
                        let mode = [TrimMode::Leading, TrimMode::Trailing, TrimMode::Both]
                            [rng.below(3)];
                        let before = rope_line_bodies(&old_text);
                        let _changed = c.trim_touched_lines(mode);
                        let after = rope_line_bodies(&c.doc.to_text());
                        assert_eq!(
                            after.len(),
                            before.len(),
                            "seed={seed} step={step} 清理改变行数"
                        );
                        for (i, (o, n)) in before.iter().zip(after.iter()).enumerate() {
                            let want = match mode {
                                TrimMode::Leading => o.trim_start(),
                                TrimMode::Trailing => o.trim_end(),
                                TrimMode::Both => o.trim(),
                            };
                            assert_eq!(n, want, "seed={seed} step={step} 第 {i} 行清理结果不符");
                        }
                    }
                    // 撤销/重做交错（空栈返回 false 合法）
                    95..=96 => {
                        let _ = c.undo();
                    }
                    // 书签套件混入（第 60 轮）：开关/跳转/清除随机三选一，
                    // 行号界内由结构不变量把关，回滚一致性由收尾对拍把关。
                    // 第 66 轮起并入括号跳转噪声臂（纯光标移动，零文本变化）。
                    97..=98 => match rng.below(4) {
                        0 => {
                            c.toggle_bookmark();
                        }
                        1 => {
                            let _ = c.next_bookmark(rng.below(2) == 0);
                        }
                        2 => {
                            let _ = c.clear_bookmarks();
                        }
                        _ => {
                            let t0 = c.doc.to_text();
                            let _ = c.jump_to_matching_bracket();
                            assert_eq!(
                                c.doc.to_text(),
                                t0,
                                "seed={seed} step={step} 括号跳转改变了文本"
                            );
                        }
                    },
                    99 => {
                        let _ = c.redo();
                    }
                    _ => {
                        let _ = c.undo();
                    }
                }
                assert_structural_invariants(&c);
            }

            // 撤销到底必须逐字节回到初始文本；重做推进到顶终态一致（可复演性）
            // 第 60 轮起书签同拍对拍：初始无书签 → 撤销到底必空 → 重放到
            // 顶与终态书签集一致
            let final_text = c.doc.to_text();
            let final_marks = c.bookmarked_lines();
            while c.undo() {}
            assert_eq!(c.doc.to_text(), initial, "seed={seed} 撤销到底未回初始态");
            assert!(
                c.bookmarked_lines().is_empty(),
                "seed={seed} 撤销到底书签未随快照清空"
            );
            while c.redo() {}
            assert_eq!(c.doc.to_text(), final_text, "seed={seed} 重放终态发散");
            assert_eq!(
                c.bookmarked_lines(),
                final_marks,
                "seed={seed} 重放终态书签发散"
            );
        }
    }

    /// 第 66 轮 主线 A 手段 2 扩容：P82/P84 行块操作（合并/拆分/删空行/
    /// 注释切换）纳入随机对拍。
    ///
    /// 文档口径 = **LF 纯净**（不含 CRLF/孤立 \r token）：主导行尾恒
    /// `\n`，四个操作的 oracle 均有闭式解；CRLF 混排下这些操作的结构
    /// 不变量已由上方主循环把关。每步先按当前光标行构造期望文本再调
    /// 用实现，逐字节对拍；收尾同样做「撤销到底回初始 / 重放终态一致」。
    #[test]
    fn random_line_block_ops_lf_docs_match_oracles() {
        const TOK: &[&str] = &["a", "Z", "中", " ", "  ", "// x", "# y"];
        for seed in [7u64, 0xBEEF_5EED, 42] {
            let mut rng = XorShift64(seed);
            let rows = rng.below(6) + 2; // 2..=7 行
            let mut init: Vec<String> = Vec::new();
            for _ in 0..rows {
                let mut s = String::new();
                for _ in 0..rng.below(4) {
                    s.push_str(TOK[rng.below(TOK.len())]);
                }
                init.push(s);
            }
            let trailing_nl = rng.below(2) == 0;
            let mut text = init.join("\n");
            if trailing_nl {
                text.push('\n');
            }
            let initial = text.clone();
            let mut c = core_with(&text);

            for step in 0..160 {
                let cur_text = c.doc.to_text();
                let bodies = rope_line_bodies(&cur_text);
                // 幻影末行：以换行收尾时 bodies 多出的最后一个空元素
                let ends_nl = cur_text.ends_with('\n');
                let real: &[String] =
                    if ends_nl && bodies.last().is_some_and(|s| s.is_empty()) {
                        &bodies[..bodies.len() - 1]
                    } else {
                        &bodies[..]
                    };
                let cur = c.cursor.line.min(real.len().saturating_sub(1));
                c.anchor = None; // 本批全部走「无选区」口径，oracle 才闭式
                c.cursor = CursorPos { line: cur, col: 0 };
                let join = |b: &[String]| -> String {
                    let mut t = b.join("\n");
                    if ends_nl && !t.is_empty() {
                        t.push('\n');
                    }
                    t
                };
                match rng.below(100) {
                    0..=24 => {
                        // 合并行：当前行并入下一行（末真实行无从并 → 不变）
                        let expected = if cur + 1 < real.len() {
                            let merged = [real[cur].trim(), real[cur + 1].trim()]
                                .iter()
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                                .collect::<Vec<_>>()
                                .join(" ");
                            let mut nb = real[..cur].to_vec();
                            nb.push(merged);
                            nb.extend_from_slice(&real[cur + 2..]);
                            join(&nb)
                        } else {
                            cur_text.clone()
                        };
                        let _changed = c.merge_lines();
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 合并发散"
                        );
                    }
                    25..=49 => {
                        // 拆分行：光标处断行（列随机）
                        let col = rng.below(real[cur].chars().count() + 1);
                        c.cursor.col = col;
                        let pre: String = real[cur].chars().take(col).collect();
                        let post: String = real[cur].chars().skip(col).collect();
                        let mut nb = real[..cur].to_vec();
                        nb.push(pre);
                        nb.push(post);
                        nb.extend_from_slice(&real[cur + 1..]);
                        let expected = join(&nb);
                        assert!(c.split_line());
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 拆分发散"
                        );
                    }
                    50..=74 => {
                        // 删空行：Empty / Whitespace 两口径（全文档）
                        let kind = if rng.below(2) == 0 {
                            BlankKind::Empty
                        } else {
                            BlankKind::Whitespace
                        };
                        let blank =
                            |s: &str| kind == BlankKind::Whitespace && s.trim().is_empty()
                                || s.is_empty();
                        let kept: Vec<String> = real
                            .iter()
                            .filter(|s| !blank(s))
                            .cloned()
                            .collect();
                        let expected = if kept.len() == real.len() {
                            cur_text.clone() // 无可删 no-op
                        } else if kept.is_empty() {
                            String::new() // 全删空且块在文档头 → 衔接符规则不加尾
                        } else {
                            join(&kept)
                        };
                        let _changed = c.delete_empty_lines(kind);
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 删空行发散（kind={kind:?}）"
                        );
                    }
                    75..=94 => {
                        // 行注释切换（无语法 → `//`）：独立按规范重算
                        let nonblank: Vec<bool> =
                            real.iter().map(|s| !s.trim().is_empty()).collect();
                        let all_commented = real
                            .iter()
                            .zip(nonblank.iter())
                            .all(|(s, &b)| !b || s.trim_start().starts_with("//"));
                        let expected_bodies: Vec<String> = real
                            .iter()
                            .enumerate()
                            .map(|(i, s)| {
                                if !nonblank[i] {
                                    return s.clone();
                                }
                                let t = s.trim_start();
                                let ind = &s[..s.len() - t.len()];
                                if all_commented {
                                    let rest = &t["//".len()..];
                                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                                    format!("{ind}{rest}")
                                } else {
                                    format!("{ind}// {t}")
                                }
                            })
                            .collect();
                        let changed_expected = expected_bodies != real;
                        let changed = c.toggle_line_comment();
                        assert_eq!(changed, changed_expected, "seed={seed} step={step} 返回值不符");
                        assert_eq!(
                            c.doc.to_text(),
                            join(&expected_bodies),
                            "seed={seed} step={step} 注释切换发散"
                        );
                    }
                    _ => {
                        // 光标噪声：End/Left 移动，仅不变量把关
                        let m = if rng.below(2) == 0 { Motion::End } else { Motion::Left };
                        c.apply_motion(m, false);
                    }
                }
                assert_structural_invariants(&c);
            }

            let final_text = c.doc.to_text();
            while c.undo() {}
            assert_eq!(c.doc.to_text(), initial, "seed={seed} 撤销到底未回初始态");
            while c.redo() {}
            assert_eq!(c.doc.to_text(), final_text, "seed={seed} 重放终态发散");
        }
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
        // 视觉行 1 = 逻辑行 0 段 1：x 落在段首与段中
        let hit = c.hit_test(gutter + mc as f32 * char_w, lh * 1.5);
        assert_eq!((hit.line, hit.col), (0, mc), "段 1 首字符");
        let hit = c.hit_test(gutter + (mc + 5) as f32 * char_w, lh * 1.5);
        assert_eq!((hit.line, hit.col), (0, mc + 5), "段 1 中段");
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
