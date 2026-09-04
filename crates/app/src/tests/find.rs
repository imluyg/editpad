use super::*;

    // ---------- 查找/替换转义解析（\n \r \t \\） ----------

    #[test]
    fn unescape_resolves_known_escapes_and_keeps_unknown_literal() {
        assert_eq!(unescape_query("\\n"), "\n");
        assert_eq!(unescape_query("a\\tb"), "a\tb");
        assert_eq!(unescape_query("\\r"), "\r");
        assert_eq!(unescape_query("a\\\\b"), "a\\b");
        // 未知转义保持原样两个字符；孤立反斜杠保持字面
        assert_eq!(unescape_query("\\q"), "\\q");
        assert_eq!(unescape_query("trailing\\"), "trailing\\");
        // 无转义内容不受影响
        assert_eq!(unescape_query("plain 中文 🚀"), "plain 中文 🚀");
    }

    #[test]
    fn find_and_replace_support_escaped_tab_and_newline() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("a\tb"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 查找 \t 并替换为换行：输入框里的字面反斜杠序列被解析成真实控制字符
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("\\t".into()));
        dispatch(&mut app, Message::ReplaceQueryChanged("\\n".into()));
        let raw_query_kept = app.find_query.clone();
        assert_eq!(raw_query_kept, "\\t", "输入框保留用户原始输入");
        // 模拟在途扫描已完成（测试中任务被丢弃，不会自动清除）
        app.find_scan = None;

        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\nb",
            "替换文本中的 \\n 转义应成为真实换行"
        );
        assert!(app.tab().dirty);
    }

    // ---------- P70 正则查找/替换 ----------

    #[test]
    fn p70_regex_toggle_rescans_and_invalid_pattern_surfaces_error() {
        let mut app = loaded_txt_app();
        dispatch(&mut app, Message::FindToggled); // 打开查找栏（校验在排扫描时进行）
        dispatch(
            &mut app,
            Message::FindQueryChanged("foo".to_owned()),
        );
        dispatch(&mut app, Message::RegexToggled(true));
        assert!(app.regex_enabled);
        assert!(app.status.contains("$1"), "开启正则应提示替换语法：{:?}", app.status);

        // 无效正则：状态栏报错、不排扫描任务
        dispatch(&mut app, Message::FindQueryChanged("(unclosed".to_owned()));
        assert!(
            app.status.contains("正则无效"),
            "非法模式应立即报错：{:?}",
            app.status
        );
        assert!(app.find_scan.is_none(), "无效模式不得排队扫描");

        // 关闭开关恢复字面模式
        dispatch(&mut app, Message::RegexToggled(false));
        assert!(!app.regex_enabled);
    }

    #[test]
    fn p70_regex_replace_all_end_to_end_with_captures() {
        let (mut app, _path) = loaded_real_file_app("p70-replace-all");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str(
                "a1 b2\na3 c\nend a4",
            ));
        }
        app.regex_enabled = true;
        app.find_query = r"a(\d)".to_owned();
        app.replace_query = "x$1".to_owned();

        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "x1 b2\nx3 c\nend x4",
            "正则替换应展开 $1 组引用"
        );
        assert!(app.tab().dirty);
        assert!(app.status.contains("3"), "应报告替换 3 处：{:?}", app.status);

        // 非法模式：文档原封不动
        app.find_query = "[".to_owned();
        dispatch(&mut app, Message::ReplaceAll);
        assert!(
            app.status.contains("正则无效"),
            "非法模式应报错：{:?}",
            app.status
        );
        assert!(app.tab().dirty, "文档保持上次替换后的置脏状态");
    }

    #[test]
    fn p70_regex_replace_current_expands_single_match() {
        let (mut app, _path) = loaded_real_file_app("p70-replace-current");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("a1 b2 a3"));
        }
        app.regex_enabled = true;
        app.find_query = r"a(\d)".to_owned();
        app.replace_query = "<$1>".to_owned();

        // 命中表直接注入（模拟后台扫描完成）：3 处命中
        let hits = vec![
            editpad_core::MatchPos { line: 0, col: 0, len_chars: 2 },
            editpad_core::MatchPos { line: 0, col: 4, len_chars: 2 },
            editpad_core::MatchPos { line: 0, col: 7, len_chars: 2 },
        ];
        let seq = app.find_seq;
        app.find_scan = Some(seq); // 登记「在途扫描」：FindScanDone 的守卫条件
        dispatch(&mut app, Message::FindScanDone(seq, hits));

        // 无当前命中：先跳到第一个（step_match 语义）
        dispatch(&mut app, Message::ReplaceCurrentRegex);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "<1> b2 a3",
            "仅当前命中被展开替换"
        );
        assert!(app.tab().dirty, "替换当前是真实文档编辑");
    }

    #[test]
    fn step_prev_from_selected_match_jumps_back_in_one_press() {
        // 光标/选区正落在命中上时，「查找上一个」必须一步跳到上一处——
        // 旧实现以光标（命中末尾）为原点，第一按会重新选中当前命中
        let (mut app, _path) = loaded_real_file_app("step-prev");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("aXbXc"));
        }
        app.find_visible = true;
        app.find_query = "X".to_owned();
        let hits = vec![
            editpad_core::MatchPos { line: 0, col: 1, len_chars: 1 },
            editpad_core::MatchPos { line: 0, col: 3, len_chars: 1 },
        ];
        let seq = app.find_seq;
        app.find_scan = Some(seq);
        dispatch(&mut app, Message::FindScanDone(seq, hits));

        dispatch(&mut app, Message::FindNext); // 选中第 1 个命中
        dispatch(&mut app, Message::FindNext); // 选中第 2 个命中
        dispatch(&mut app, Message::FindPrev);
        assert_eq!(app.match_idx, Some(0), "从第 2 个命中一步回到第 1 个");

        // 「下一个」语义不变：以命中末尾为原点继续向后
        dispatch(&mut app, Message::FindNext);
        assert_eq!(app.match_idx, Some(1));
    }

    #[test]
    fn whole_word_replace_all_and_scan_filter() {
        // 整词模式：全部替换走整词路径（concat 内部的 cat 不动）；
        // 扫描命中表经 filter_whole_word 过滤词内命中
        let (mut app, _path) = loaded_real_file_app("whole-word");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("cat concat cat"));
        }
        app.find_visible = true;
        app.find_query = "cat".to_owned();
        app.replace_query = "dog".to_owned();
        dispatch(&mut app, Message::WholeWordToggled(true));
        assert!(app.whole_word, "开关应置位");
        // 开关排队的扫描 Task 被 dispatch 丢弃：模拟扫描已完成，
        // 否则「扫描在途禁全部替换」守卫会拦截下面的 ReplaceAll
        app.find_scan = None;

        // 全部替换（整词路径）：首尾两个独立 cat 被替换
        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "dog concat dog",
            "concat 内部的 cat 不得被替换"
        );
        assert!(
            app.status.contains("已替换 2 处"),
            "实际 {:?}",
            app.status
        );

        // 扫描过滤语义：替换后唯一剩余的 cat 在 concat 内部，
        // 整词过滤应将其剔除
        let doc = app.cur_handle.borrow().doc.clone();
        let hits = editpad_core::find_all_document(&doc, "cat", true);
        assert_eq!(hits.len(), 1);
        let filtered = editpad_core::filter_whole_word(&doc, hits);
        assert!(filtered.is_empty(), "词内命中应被过滤");
    }

    #[test]
    fn p70_regex_replace_current_zero_width_expands_not_literal_fallback() {
        // 零宽命中（如 a*/b* 的空匹配）：旧实现 selected_text() 为 None
        // 时静默回落字面 replace_current——拿正则串当字面量匹配。钉死：
        // 按位置展开替换文本（$0 引用）。
        let (mut app, _path) = loaded_real_file_app("p70-zero-width");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("abc"));
        }
        app.regex_enabled = true;
        app.find_query = r"b*".to_owned();
        app.replace_query = "-$0-".to_owned();

        // 命中表直接注入：b* 在 col 1 的零宽匹配（空命中 len_chars=0）
        let hits = vec![editpad_core::MatchPos { line: 0, col: 1, len_chars: 0 }];
        let seq = app.find_seq;
        app.find_scan = Some(seq);
        dispatch(&mut app, Message::FindScanDone(seq, hits));

        dispatch(&mut app, Message::ReplaceCurrentRegex);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a-b-bc",
            "零宽命中应在原位展开替换文本（b 被 $0 引用）"
        );
        assert!(app.tab().dirty);
    }

    #[test]
    fn p63_autosave_must_skip_contract() {
        // 写前判定纯函数契约：期望戳缺失不拦截；期望已知时以差异为准
        let t0 = std::time::SystemTime::UNIX_EPOCH;
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert!(!autosave_must_skip(None, Some((t1, 5))), "从未记录无从比对");
        assert!(!autosave_must_skip(Some((t0, 5)), Some((t0, 5))), "一致放行");
        assert!(autosave_must_skip(Some((t0, 5)), Some((t1, 5))), "mtime 变化拒写");
        assert!(autosave_must_skip(Some((t0, 5)), Some((t0, 6))), "size 变化拒写");
        assert!(
            autosave_must_skip(Some((t0, 5)), None),
            "文件被外部删除同样拒写（提示条会给出明确失败提示）"
        );
    }

    // ---------- P10 后台查找扫描 ----------

    #[test]
    fn find_scan_stream_emits_exactly_one_done_with_matches() {
        let payload = FindScanPayload {
            seq: 42,
            doc: editpad_core::Document::from_str("foo\nbar foo\n"),
            query: "foo".to_owned(),
            case_sensitive: true,
            regex: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        // 恰好一条完成消息（函数直接返回它）
        let message = block_on(drive_find_scan(payload, |doc, q, cs, _rx| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 42);
                assert_eq!(
                    hits,
                    &vec![
                        editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
                        editpad_core::MatchPos { line: 1, col: 4, len_chars: 3 },
                    ]
                );
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_cancelled_task_skips_scanning_and_replies_empty() {
        // 新输入排队时会把上一代的取消标志置位；被作废的任务醒来即退出，
        // 不浪费一次全文扫描，但仍回一条空结果消息保持「恰好一条」语义
        let flag = Arc::new(AtomicBool::new(false));
        flag.store(true, Ordering::Relaxed);
        let payload = FindScanPayload {
            seq: 3,
            doc: editpad_core::Document::from_str("target target"),
            query: "target".to_owned(),
            case_sensitive: true,
            regex: false,
            cancelled: flag,
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(payload, |doc, q, cs, _rx| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(seq, 3);
                assert!(hits.is_empty(), "被取消的任务不得产出命中");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_survives_panic_and_still_replies() {
        // 扫描函数崩溃也必须回消息（空表），否则查找栏永久停在「查找中…」
        let payload = FindScanPayload {
            seq: 7,
            doc: editpad_core::Document::new(),
            query: "x".to_owned(),
            case_sensitive: false,
            regex: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(
            payload,
            |_doc, _q, _cs, _rx| -> Vec<editpad_core::MatchPos> { panic!("模拟扫描崩溃") },
        ));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 7);
                assert!(hits.is_empty(), "panic 兜底应回空命中表");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_results_are_filtered_by_sequence_number() {
        let mut app = Editpad::default();
        // 两份可区分的命中表：过期投递不得覆盖已采纳/待采纳的状态
        let hit_a = || vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 1 }];
        let hit_b = || vec![editpad_core::MatchPos { line: 1, col: 4, len_chars: 1 }];
        /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
        fn dispatch(app: &mut Editpad, message: Message) {
            let _ = app.update(message);
        }

        app.find_visible = true;

        // 排队 → 采纳当前序号的结果
        dispatch(&mut app, Message::FindQueryChanged("a".into()));
        assert_eq!(app.find_scan, Some(1), "查询变化应排队一次后台扫描");
        dispatch(&mut app, Message::FindScanDone(1, hit_a()));
        assert_eq!(app.matches, hit_a());
        assert_eq!(app.find_scan, None, "结果采纳后在途标记应清除");
        assert_eq!(app.match_idx, None, "新结果后跳转游标复位");

        // 已消费的序号再回来（重复投递）不得二次生效
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a());

        // 新输入换新序号；重扫期间保留旧命中（防闪烁），但旧序号的迟到结果不许覆盖
        dispatch(&mut app, Message::FindQueryChanged("ab".into()));
        assert_eq!(app.find_scan, Some(2));
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a(), "过期结果必须被丢弃、不得覆盖");
        dispatch(&mut app, Message::FindScanDone(2, hit_b()));
        assert_eq!(app.matches, hit_b());
        assert_eq!(app.find_scan, None);

        // 大小写切换同样触发重扫
        dispatch(&mut app, Message::CaseToggled(true));
        assert_eq!(app.find_scan, Some(3));

        // 文档编辑（查找栏开着时）触发重扫
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert_eq!(app.find_scan, Some(4));

        // 关闭查找栏 = 取消：清结果且在途结果作废
        dispatch(&mut app, Message::FindToggled);
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(4, hit_a()));
        assert!(app.matches.is_empty(), "取消后的迟到结果必须被丢弃");

        // Esc 关栏同样取消（注意：上一步关栏的取消已把序号推到 5，本次排队为 6）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("abc".into()));
        assert_eq!(app.find_scan, Some(6));
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.find_scan, None);
        dispatch(&mut app, Message::FindScanDone(6, hit_a()));
        assert!(app.matches.is_empty());

        // 清空查询 = 取消而非空表扫描（BarsDismissed 取消后序号为 7，本次排队 8）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("q".into()));
        assert_eq!(app.find_scan, Some(8));
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert_eq!(app.matches, hit_a());
        dispatch(&mut app, Message::FindQueryChanged(String::new()));
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert!(app.matches.is_empty(), "清空查询取消后，同号迟到结果也必须被丢弃");
    }

    #[test]
    fn find_next_while_scanning_reports_progress_not_stale_jump() {
        let mut app = Editpad::default();
        app.find_visible = true;
        app.find_query = "zzz".into();

        // 有结果在途：不基于过期命中表跳转
        app.find_scan = Some(9);
        app.matches = vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 }];
        let _ = app.update(Message::FindNext);
        assert_eq!(app.status, "查找中…", "扫描在途时 Enter 应提示进度");

        // 无结果且无在途扫描：懒补排一次扫描而不是误报「无匹配」
        let mut fresh = Editpad::default();
        fresh.find_visible = true;
        fresh.find_query = "zzz".into();
        let _ = fresh.update(Message::FindNext);
        assert!(fresh.find_scan.is_some(), "Enter 应懒触发一次后台扫描");
    }

    // ---------- P26 跨行查询（CRLF 文档上的选区还原与替换当前） ----------

    /// 构造一个已加载 CRLF 文档的应用，返回 (应用, 文档快照)。
    fn crlf_find_app() -> (Editpad, editpad_core::Document) {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/crlf.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("first\r\nsecond\r\nthird");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc.clone(), String::new(), "UTF-8".to_owned()))),
        );
        (app, doc)
    }

    #[test]
    fn multiline_match_selects_across_lines_on_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        // 用户输入转义序列：\n 解析为真实换行 → 查询含换行、跨行匹配
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        assert!(app.find_scan.is_some(), "查询变化应排队后台扫描");

        // 用与后台线程相同的核心函数产出命中表，再按协议回填
        let query = unescape_query("st\\nse");
        let hits = editpad_core::find_all_document(&doc, &query, true);
        assert_eq!(
            hits,
            vec![editpad_core::MatchPos { line: 0, col: 3, len_chars: 5 }],
            "命中跨度应为显示口径 5（2 字符 + 1 次跨行 + 2 字符），不是原始字符数 6"
        );
        let seq = app.find_scan.unwrap();
        dispatch(&mut app, Message::FindScanDone(seq, hits));
        assert!(app.matches.len() == 1);

        // 跳到该命中：选区必须跨行且恰好覆盖「st\r\nse」（\r\n 只算一格）
        dispatch(&mut app, Message::FindNext);
        assert_eq!(app.status, "第 1/1 处匹配");
        let selected = app.cur_handle.borrow().selected_text();
        assert_eq!(
            selected.as_deref(),
            Some("st\r\nse"),
            "跨行选区应包含文档真实的 \\r\\n，而不是多走/少走一格"
        );
    }

    #[test]
    fn replace_current_replaces_multiline_match_in_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        let query = unescape_query("st\\nse");
        let seq = {
            let hits = editpad_core::find_all_document(&doc, &query, true);
            let s = app.find_scan.unwrap();
            dispatch(&mut app, Message::FindScanDone(s, hits));
            s
        };
        let _ = seq;
        dispatch(&mut app, Message::FindNext);
        dispatch(&mut app, Message::ReplaceQueryChanged("-".into()));

        // 替换当前：选区文本 st\r\nse 与查询 st\nse 行尾归一后判等才可替换。
        // （P26 前的行为：字面比较判不等 → 误跳下一个、文档不动。）
        dispatch(&mut app, Message::ReplaceCurrent);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            // "second" 被吃掉开头 "se" 后剩 "cond"，不是 "ond"
            "fir-cond\r\nthird",
            "跨行命中应被整体替换为替换文本"
        );
        assert!(app.tab().dirty);

        // 可撤销：替换走 insert_str 快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "first\r\nsecond\r\nthird");
    }

    // ---------- P11 全部替换（rope 流式路径） ----------

    #[test]
    fn replace_all_swaps_document_and_sets_dirty() {
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("foo bar foo\nfoo"));
        app.find_visible = true;
        app.find_query = "foo".into();
        app.replace_query = "baz".into();

        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "baz bar baz\nbaz",
            "全部替换应改写文档内容"
        );
        assert!(app.tab().dirty, "全部替换后必须置脏");
        assert_eq!(app.status, "已替换 3 处");

        // 无命中时不改文档也不置脏
        let mut app2 = Editpad::default();
        app2.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("untouched"));
        app2.find_query = "zzz".into();
        app2.replace_query = "x".into();
        let _ = app2.update(Message::ReplaceAll);
        assert_eq!(app2.cur_handle.borrow().doc.to_text(), "untouched");
        assert!(!app2.tab().dirty);
        assert_eq!(app2.status, "已替换 0 处");
    }

    #[test]
    fn replace_all_is_blocked_while_scan_in_flight() {
        // P10 守卫在 P11 新路径上仍然生效：扫描在途时的全文快照可能过期
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("keep me"));
        app.find_query = "me".into();
        app.replace_query = "you".into();
        app.find_scan = Some(11);
        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "keep me",
            "后台扫描在途时不得执行全部替换"
        );
        assert!(!app.tab().dirty);
    }


    // ---------- P123：视口命中高亮与选中带入 ----------

    #[test]
    fn find_scan_done_syncs_viewport_highlights_and_close_clears() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("cat concat cat"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        dispatch(&mut app, Message::FindToggled);
        assert!(app.find_visible);
        dispatch(&mut app, Message::FindQueryChanged("cat".into()));
        let scan_seq = app.find_scan.expect("查询变化应排队扫描");
        dispatch(
            &mut app,
            Message::FindScanDone(
                scan_seq,
                vec![
                    editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
                    editpad_core::MatchPos { line: 0, col: 11, len_chars: 3 },
                ],
            ),
        );
        assert_eq!(
            app.cur_handle.borrow().find_hl.len(),
            2,
            "扫描完成后命中表同步进编辑器高亮层"
        );
        // 关栏清空（cancel_find_scan 收口）
        dispatch(&mut app, Message::FindToggled);
        assert!(app.cur_handle.borrow().find_hl.is_empty());
    }

    #[test]
    fn find_bar_open_prefills_selection_as_query() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("hello world"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        app.cur_handle.borrow_mut().select_span(0, 6, 5); // 选中 "world"
        dispatch(&mut app, Message::FindToggled);
        assert_eq!(app.find_query, "world", "开栏带入选中文本");
        // 清掉选区后重开栏：无选区不覆盖已有查询（关栏不清编辑器选区）
        dispatch(&mut app, Message::FindToggled); // 关
        app.cur_handle.borrow_mut().anchor = None;
        dispatch(&mut app, Message::FindToggled); // 开
        assert_eq!(app.find_query, "world", "无选区开栏保留原查询");
    }
