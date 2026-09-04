use super::*;
use super::tests::*;

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
    fn backspace_and_delete_forward_report_whether_anything_was_deleted() {
        // 返回值口径：空操作（原点退格 / 末尾前删）= false，真实删除 =
        // true——应用层据此判定置脏与自动保存，空操作不得当编辑。
        let mut c = core_with("abc");
        assert!(!c.backspace(), "文档原点退格是 no-op");
        assert_eq!(c.doc.to_text(), "abc");
        c.cursor = CursorPos { line: 0, col: 3 };
        assert!(!c.delete_forward(), "文档末尾前删是 no-op");
        assert_eq!(c.doc.to_text(), "abc");

        assert!(c.backspace(), "行内退格真实删除");
        assert_eq!(c.doc.to_text(), "ab");
        c.cursor = CursorPos { line: 0, col: 0 };
        assert!(c.delete_forward(), "行内前删真实删除");
        assert_eq!(c.doc.to_text(), "b");

        // 选区删除同样算真实编辑
        let mut s = core_with("abc");
        s.anchor = Some(CursorPos { line: 0, col: 0 });
        s.cursor = CursorPos { line: 0, col: 2 };
        assert!(s.backspace(), "选区删除真实生效");
        assert_eq!(s.doc.to_text(), "c");
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

