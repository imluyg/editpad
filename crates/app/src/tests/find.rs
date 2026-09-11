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

        // P148：替换本体移后台——dispatch 丢弃 Task，手动回报计算结果
        dispatch(&mut app, Message::ReplaceAll);
        assert!(app.busy, "正则替换应进入 busy 包裹");
        let (new_contents, count) = editpad_core::replace_all_regex(
            "a1 b2\na3 c\nend a4",
            r"a(\d)",
            "x$1",
            true,
        )
        .expect("参照替换应成功");
        dispatch(
            &mut app,
            Message::ReplaceAllRegexDone(Ok((new_contents, count))),
        );
        assert!(!app.busy);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "x1 b2\nx3 c\nend x4",
            "正则替换应展开 $1 组引用"
        );
        assert!(app.tab().dirty);
        assert!(app.status.contains("3"), "应报告替换 3 处：{:?}", app.status);

        // 后台计算失败（如回溯超限）：文档原封不动、busy 收口、错误留痕
        let before = app.cur_handle.borrow().doc.clone();
        dispatch(&mut app, Message::ReplaceAllRegexDone(Err("回溯超限".into())));
        assert!(!app.busy);
        assert_eq!(app.cur_handle.borrow().doc.to_text(), before.to_text());
        assert!(
            app.status.contains("正则替换失败"),
            "实际 {:?}",
            app.status
        );
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

// ---------- A8：在文件中查找（Phase 2 接线） ----------

/// 与 find_scan.rs 的 collect_excerpts 同口径的直测：tempdir 树上跑
/// fif_scan_dir（真文件系统），钉住遍历排除、二进制跳过、摘录预计算
/// 与封顶截断。
#[test]
fn fif_scan_dir_walks_tree_skips_binary_and_truncates() {
    use crate::find_scan::{fif_scan_dir, FifScanPayload};
    let dir = scratch_dir("fif-scan");
    fs_create_dir_all(&dir);
    std::fs::write(dir.join("a.txt"), "needle here\nno match\nneedle two").unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join(".hidden/h.txt"), "needle hidden").unwrap();
    std::fs::create_dir_all(dir.join("node_modules")).unwrap();
    std::fs::write(dir.join("node_modules/n.txt"), "needle noise").unwrap();
    // 二进制文件（NUL）：load_file 拒绝 → 跳过不中断
    std::fs::write(dir.join("bin.dat"), b"ne\x00edle").unwrap();

    let mk = |max_total: usize, max_per: usize| FifScanPayload {
        seq: 1,
        dir: dir.clone(),
        query: "needle".into(),
        case_sensitive: true,
        regex: false,
        whole_word: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        progress: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_files: 100,
        max_total_hits: max_total,
        max_hits_per_file: max_per,
    };

    let (results, truncated) = fif_scan_dir(&mk(100, 100));
    assert!(!truncated);
    assert_eq!(results.len(), 1, "只有 a.txt 命中（隐藏/噪音目录/二进制全跳过）");
    assert!(results[0].path.ends_with("a.txt"));
    assert_eq!(results[0].hits.len(), 2);
    assert!(results[0].hits[0].excerpt.contains("needle"), "摘录预计算");

    // 总命中封顶：截断明示
    let (results, truncated) = fif_scan_dir(&mk(1, 100));
    assert!(truncated, "总命中达封顶即截断");
    assert!(!results.is_empty());

    // 单文件封顶：截断到 max_hits_per_file
    let (_, _) = fif_scan_dir(&mk(100, 1));
    let (results, _) = fif_scan_dir(&mk(100, 1));
    assert!(results.iter().all(|f| f.hits.len() <= 1));

    // 取消标志：直接返回空
    let cancelled = mk(100, 100);
    cancelled
        .cancelled
        .store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(fif_scan_dir(&cancelled), (Vec::new(), false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_in_files_toggle_scan_done_and_seq_guard() {
    let mut app = Editpad::default();
    // 未命名页：无目录锚点，开启后面板可见但目录为 None
    dispatch(&mut app, Message::FindInFilesToggled);
    assert!(app.fif_visible && app.find_visible, "开启即带开查找栏");
    assert!(!app.find_all_visible, "同槽互斥");
    assert!(app.fif_dir.is_none(), "未命名页无目录锚点");
    // 再按 = 关闭
    dispatch(&mut app, Message::FindInFilesToggled);
    assert!(!app.fif_visible);

    // 完成回填 + 过期序号丢弃
    dispatch(&mut app, Message::FindInFilesToggled);
    app.fif_scan = Some(7);
    let fh = crate::find_scan::FileHits {
        path: PathBuf::from("C:/doc/a.txt"),
        hits: vec![crate::find_scan::FileHit {
            pos: editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
            excerpt: "abc".into(),
        }],
    };
    dispatch(&mut app, Message::FifScanDone(8, vec![fh.clone()], true));
    assert!(app.fif_results.is_empty(), "过期序号的结果丢弃");
    dispatch(&mut app, Message::FifScanDone(7, vec![fh], true));
    assert_eq!(app.fif_results.len(), 1, "当前代结果回填");
    assert!(app.fif_truncated);
    assert!(app.status.contains("封顶"), "截断明示上状态栏");
}

#[test]
fn find_in_files_hotkey_f12_and_goto_open_tab() {
    use crate::find_scan::{FileHit, FileHits};
    // F12 默认分发
    let msg = handle_key_defaults(
        keyboard::Key::Named(keyboard::key::Named::F12),
        keyboard::Modifiers::empty(),
    );
    assert!(
        matches!(msg, Some(Message::FindInFilesToggled)),
        "F12 默认分发 find_in_files"
    );

    // 已开页命中：切换 + select_span 选中命中跨度
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/a.txt")));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("xx ab yy"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    app.fif_results = vec![FileHits {
        path: PathBuf::from("C:/doc/a.txt"),
        hits: vec![FileHit {
            pos: editpad_core::MatchPos { line: 0, col: 3, len_chars: 2 },
            excerpt: "ab".into(),
        }],
    }];
    dispatch(&mut app, Message::FifGoto(0, 0));
    {
        let h = app.cur_handle.borrow();
        assert_eq!(h.ordered_selection().map(|(s, e)| (s.line, s.col, e.line, e.col)), Some((0, 3, 0, 5)), "命中跨度被选中");
    }

    // 未开页：登记待跳转 + 走打开管线，Loaded 结算后选中命中
    app.fif_results = vec![FileHits {
        path: PathBuf::from("C:/doc/other.txt"),
        hits: vec![FileHit {
            pos: editpad_core::MatchPos { line: 2, col: 4, len_chars: 3 },
            excerpt: "hit".into(),
        }],
    }];
    dispatch(&mut app, Message::FifGoto(0, 0));
    assert!(app.active_load.is_some(), "未开页触发打开管线");
    assert_eq!(app.pending_fif_goto, Some((2, 4, 3)));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("aa\nbb\nxx hit yy"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert!(app.pending_fif_goto.is_none(), "Loaded 结算一次性消费");
    let h = app.cur_handle.borrow();
    assert_eq!(
        h.ordered_selection().map(|(s, e)| (s.line, s.col, e.line, e.col)),
        Some((2, 4, 2, 7)),
        "装载完成后选区落在命中上"
    );
}

// ---------- A8 Phase 3：边界批 ----------

/// 空目录 / 全被排除 / 不存在的根：扫描空结果且不误报截断；
/// GBK 文件经编码嗅探正常命中；正则模式与 CRLF 文件口径正确。
#[test]
fn fif_scan_dir_boundary_dirs_encodings_and_regex() {
    use crate::find_scan::{fif_scan_dir, FifScanPayload};
    let mk = |dir, regex| FifScanPayload {
        seq: 1,
        dir,
        query: "needle".into(),
        case_sensitive: true,
        regex,
        whole_word: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        progress: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_files: 100,
        max_total_hits: 100,
        max_hits_per_file: 100,
    };

    // 空目录：空结果、无截断
    let empty = scratch_dir("fif-empty");
    fs_create_dir_all(&empty);
    let (results, truncated) = fif_scan_dir(&mk(empty.clone(), false));
    assert!(results.is_empty() && !truncated, "空目录 = 空结果不误报截断");
    let _ = std::fs::remove_dir_all(&empty);

    // 全被排除（只有隐藏目录 + 噪音目录）：同样空结果
    let excluded = scratch_dir("fif-excluded");
    fs_create_dir_all(&excluded.join(".hidden"));
    fs_create_dir_all(&excluded.join("node_modules"));
    std::fs::write(excluded.join(".hidden/x.txt"), "needle").unwrap();
    std::fs::write(excluded.join("node_modules/x.txt"), "needle").unwrap();
    let (results, truncated) = fif_scan_dir(&mk(excluded.clone(), false));
    assert!(results.is_empty() && !truncated);
    let _ = std::fs::remove_dir_all(&excluded);

    // 不存在的根：静默空结果
    let (results, truncated) = fif_scan_dir(&mk(std::path::PathBuf::from("Z:/no/such"), false));
    assert!(results.is_empty() && !truncated);

    // GBK 编码文件：load_file 兜底解码 → 中文查询命中（编码嗅探继承）
    let gbk = scratch_dir("fif-gbk");
    // "中文 needle" 的 GBK 字节：中=D6D0 文=C4FA，其余 ASCII 原样
    std::fs::write(gbk.join("gbk.txt"), b"\xD6\xD0\xCE\xC4 needle").unwrap();
    let mut payload = mk(gbk.clone(), false);
    payload.query = "needle".into();
    let (results, _) = fif_scan_dir(&payload);
    assert_eq!(results.len(), 1, "GBK 文件参与扫描");
    payload.query = "中".into();
    let (results, _) = fif_scan_dir(&payload);
    assert_eq!(results.len(), 1, "GBK 兜底解码后中文查询命中");
    let _ = std::fs::remove_dir_all(&gbk);

    // 正则模式 + CRLF 文件：命中行号按 \r\n 行界计。
    // "needle \w+" 命中行 1 的 "needle two"（行 2 的 needle 在行尾，
    // 后随行界无 \w+ 可配——正好钉住跨行正则不误吞行界）
    let crlf = scratch_dir("fif-crlf");
    std::fs::write(crlf.join("c.txt"), "one\r\nneedle two\r\nthree needle").unwrap();
    let mut payload = mk(crlf.clone(), true);
    payload.query = r"needle \w+".into();
    let (results, _) = fif_scan_dir(&payload);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].hits.len(), 1);
    assert_eq!(results[0].hits[0].pos.line, 1, "CRLF 行界口径");
    let _ = std::fs::remove_dir_all(&crlf);
}

/// 处理器边界：选择目录被取消（None）= busy 清零但目录不动；
/// 查找栏关闭 = FIF 面板随栏隐藏 + 目录扫描取消；查询为空时开启
/// FIF = 不排扫描（面板提示先输入查询）。
#[test]
fn fif_handlers_dir_pick_cancel_and_bar_close_teardown() {
    let mut app = Editpad::default();

    // FifDirPicked(None)：busy 清零、目录保持
    app.busy = true;
    app.fif_dir = Some(PathBuf::from("C:/keep"));
    dispatch(&mut app, Message::FifDirPicked(None));
    assert!(!app.busy, "对话框取消也要清 busy");
    assert_eq!(app.fif_dir.as_deref(), Some(Path::new("C:/keep")));

    // 查询为空时开启 FIF：不排队扫描（面板给提示）
    app.find_query.clear();
    dispatch(&mut app, Message::FindInFilesToggled);
    assert!(app.fif_visible && app.fif_scan.is_none(), "空查询不排扫描");

    // 关查找栏 = FIF 随栏隐藏 + 在途扫描取消
    app.fif_scan = Some(3);
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.find_visible && !app.fif_visible, "面板随栏隐藏");
    assert!(app.fif_scan.is_none(), "在途目录扫描取消");
}

/// 附加光标集与 FIF 互不干扰（只读搜索特性不触碰文档/光标/多光标态）。
#[test]
fn fif_scan_touches_neither_document_nor_cursors() {
    use crate::editor::{CursorPos, ExtraCursor};
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/a.txt")));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("content"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    app.cur_handle.borrow_mut().extra_cursors = vec![ExtraCursor {
        cursor: CursorPos { line: 0, col: 3 },
        anchor: None,
    }];
    app.fif_visible = true;
    app.fif_dir = Some(PathBuf::from("C:/no/such"));
    dispatch(&mut app, Message::FindQueryChanged("x".into()));
    let h = app.cur_handle.borrow();
    assert!(h.has_multi(), "FIF 查询触发不折叠多光标");
    assert_eq!(h.doc.to_text(), "content", "FIF 不触碰文档");
    assert_eq!(h.cursor, CursorPos { line: 0, col: 0 }, "FIF 不动主光标");
}

// ---------- P150：查找轻浮层（居中 + 不挤占正文） ----------

/// 收集应用视图的全部布局节点 bounds（深度封顶，够覆盖 Stack/正文树）。
fn collect_layout_nodes(
    layout: iced::advanced::Layout<'_>,
    depth: usize,
    out: &mut Vec<iced::Rectangle>,
) {
    out.push(layout.bounds());
    if depth >= 7 {
        return;
    }
    for i in 0..layout.children().count() {
        collect_layout_nodes(layout.child(i), depth + 1, out);
    }
}

/// 渲染一次应用视图并返回全部节点 bounds。
fn app_layout_nodes(app: &Editpad) -> Vec<iced::Rectangle> {
    use iced::advanced::{
        layout::{self, Layout},
        widget::Tree,
    };
    use iced::{Font, Pixels, Size};

    let mut element = app.view();
    let mut tree = Tree::new(element.as_widget());
    let renderer = iced::Renderer::new(Font::MONOSPACE, Pixels(16.0));
    let limits = layout::Limits::new(
        Size::new(app.viewport_size.0, app.viewport_size.1),
        Size::new(app.viewport_size.0, app.viewport_size.1),
    );
    let node = element.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);
    let mut nodes = Vec::new();
    collect_layout_nodes(root, 0, &mut nodes);
    nodes
}

/// 正文编辑器节点 = 全宽节点里 y > 40（菜单/标签条之下）且最高的那个。
fn editor_node(nodes: &[iced::Rectangle]) -> iced::Rectangle {
    *nodes
        .iter()
        .filter(|b| b.width > 1000.0 && b.y > 40.0 && b.height > 150.0)
        .max_by(|a, b| a.height.partial_cmp(&b.height).unwrap())
        .expect("应能找到正文编辑器节点")
}

fn app_with_lines(lines: usize) -> Editpad {
    let mut app = Editpad::default();
    app.viewport_size = (1024.0, 768.0); // 与真实窗口同尺寸（卡片宽度钳制要用）
    dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/p150.txt")));
    let seq = app.job_seq;
    let text: String = "1\n".repeat(lines);
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str(&text),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    app
}

/// P150：查找 UI 从「底部整宽停靠行」改为「窗口中间定宽轻浮层」——
/// 正文高度不随查找栏开关变化（修前查找行会把正文挤矮），且卡片水平居中。
#[test]
fn p150_find_overlay_centered_and_does_not_shrink_editor() {
    let mut app = app_with_lines(12);
    let off = editor_node(&app_layout_nodes(&app));

    dispatch(&mut app, Message::FindToggled);
    assert!(app.find_visible, "Ctrl+F 应打开查找");
    let nodes_on = app_layout_nodes(&app);
    let on = editor_node(&nodes_on);
    assert_eq!(
        (off.y, off.height),
        (on.y, on.height),
        "查找浮层不得挤占正文（修前为底部停靠行，正文被压矮）"
    );

    // 卡片：宽度贴近 FIND_CARD_W、水平居中于窗口
    let card = nodes_on
        .iter()
        .filter(|b| (480.0..=600.0).contains(&b.width) && b.height > 80.0)
        .max_by(|a, b| a.height.partial_cmp(&b.height).unwrap())
        .copied()
        .expect("应能找到查找浮层卡片");
    let center_x = card.x + card.width * 0.5;
    let center_y = card.y + card.height * 0.5;
    eprintln!("[P150] 卡片 = {card:?} 中心 = ({center_x:.1},{center_y:.1})");
    assert!(
        (center_x - 512.0).abs() < 2.0,
        "查找浮层应水平居中，实际中心 x = {center_x:.1}"
    );
    assert!(
        (center_y - 384.0).abs() < 60.0,
        "查找浮层应出现在窗口中间，实际中心 y = {center_y:.1}"
    );

    // 结果面板开态同样不占正文（面板在卡片内滚动）
    app.find_all_visible = true;
    let panel = editor_node(&app_layout_nodes(&app));
    assert_eq!(
        (off.y, off.height),
        (panel.y, panel.height),
        "结果面板开态也必须留在浮层内，不得挤占正文"
    );

    // 关栏后浮层消失（节点回到基线）
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.find_visible);
    let after = editor_node(&app_layout_nodes(&app));
    assert_eq!((off.y, off.height), (after.y, after.height));
}

/// P151：程序化聚焦应用内文本框（查找框/命令面板/重命名）时，**正文编辑器
/// 必须交出输入焦点**——编辑器核心的 `focused` 只在鼠标按下时更新，程序化
/// 聚焦不经过鼠标，于是同一个 IME 组字事件被两处消费：预编辑串在文本框与
/// 正文各画一份（用户复现「怎么有两个」），**上屏文本还会被编辑器吃进文档**
/// （在查找框里打中文 = 改正文）。
#[test]
fn p151_programmatic_text_field_focus_releases_editor_ime() {
    use crate::editor::ImeCommit;
    use crate::state::PaletteMode;

    let mut app = app_with_lines(3);

    // 查找栏：打开 → 正文交出焦点（不吃预编辑、不吃上屏）
    dispatch(&mut app, Message::FindToggled);
    assert!(app.find_visible);
    assert!(
        !app.cur_handle.borrow().focused,
        "查找框接管焦点后正文不得再持有 IME 焦点"
    );
    assert!(
        !app.cur_handle.borrow_mut().ime_preedit("ni".to_owned()),
        "正文不得消费预编辑串（否则查找框与正文各画一份）"
    );
    assert_eq!(
        app.cur_handle.borrow_mut().ime_commit("你好"),
        ImeCommit::Ignored,
        "正文不得消费上屏事件"
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "1\n1\n1\n",
        "上屏文本绝不能落进文档"
    );

    // 关闭（× 同路径）→ 焦点还给正文，IME 恢复可用
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.find_visible);
    assert!(app.cur_handle.borrow().focused, "关栏后正文应重新持有焦点");
    assert!(app.cur_handle.borrow_mut().ime_preedit("ni".to_owned()));

    // Esc 收起路径同口径
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.cur_handle.borrow().focused);
    dispatch(&mut app, Message::BarsDismissed);
    assert!(!app.find_visible);
    assert!(app.cur_handle.borrow().focused, "Esc 收起后焦点应回正文");

    // 命令面板同口径（P129 也是程序化聚焦）
    dispatch(&mut app, Message::PaletteToggled(PaletteMode::Commands));
    assert!(app.palette_visible);
    assert!(
        !app.cur_handle.borrow().focused,
        "命令面板接管焦点后正文应让出"
    );
    dispatch(&mut app, Message::PaletteToggled(PaletteMode::Commands));
    assert!(!app.palette_visible);
    assert!(app.cur_handle.borrow().focused);
}

/// 用户复报二连：①查找浮层必须能**拖开**（固定居中会遮住正文目标行，
/// 只看得到浮层底下看不到内容）；②关栏后状态栏左下的「第 N/M 处匹配」
/// 必须一起消失（修前关栏、点空白都不消失）。
#[test]
fn find_overlay_drags_and_clears_its_status_on_close() {
    let mut app = app_with_lines(3);
    app.viewport_size = (1024.0, 768.0);
    dispatch(&mut app, Message::FindToggled);
    assert!(app.find_visible);
    assert!(app.find_pos.is_none(), "未拖动时应走默认位置");
    let default = app.default_find_pos();

    // 按下拖动条（锚点 = 最近一次 on_move 位置）→ 逐帧增量平移
    dispatch(&mut app, Message::FindCursorMoved(Point::new(600.0, 300.0)));
    dispatch(&mut app, Message::FindDragStart);
    dispatch(&mut app, Message::FindCursorMoved(Point::new(700.0, 350.0)));
    let moved = app.find_pos.expect("拖动后应持有显式位置");
    assert!((moved.x - (default.x + 100.0)).abs() < 0.01, "x 未按增量平移：{moved:?}");
    assert!((moved.y - (default.y + 50.0)).abs() < 0.01, "y 未按增量平移：{moved:?}");
    dispatch(&mut app, Message::FindDragEnd);

    // 拖出视野 → 钳制在窗口内；双击拖动条 → 复位默认
    dispatch(&mut app, Message::FindCursorMoved(Point::new(5000.0, 5000.0)));
    dispatch(&mut app, Message::FindDragStart);
    dispatch(&mut app, Message::FindCursorMoved(Point::new(9000.0, 9000.0)));
    assert_eq!(
        app.find_pos,
        Some(app.clamp_find_pos(Point::new(9000.0, 9000.0))),
        "拖动必须钳制在窗口内"
    );
    dispatch(&mut app, Message::FindDragEnd);
    dispatch(&mut app, Message::FindDragReset);
    assert!(app.find_pos.is_none(), "双击拖动条应复位到默认位置");

    // 结构核对：卡片左上角 ≡ find_pos（拖到左上角区域）
    dispatch(&mut app, Message::FindCursorMoved(Point::new(600.0, 300.0)));
    dispatch(&mut app, Message::FindDragStart);
    dispatch(&mut app, Message::FindCursorMoved(Point::new(200.0, 120.0)));
    dispatch(&mut app, Message::FindDragEnd);
    let pos = app.find_pos.expect("应持有位置");
    let nodes = app_layout_nodes(&app);
    let card = nodes
        .iter()
        .filter(|b| (480.0..=600.0).contains(&b.width) && b.height > 80.0)
        .max_by(|a, b| a.height.partial_cmp(&b.height).unwrap())
        .copied()
        .expect("应能找到查找浮层卡片");
    assert!(
        (card.x - pos.x).abs() < 1.0 && (card.y - pos.y).abs() < 1.0,
        "卡片位置应等于 find_pos：卡片 {card:?}，pos {pos:?}"
    );

    // 状态清理：× / Ctrl+F 关栏
    app.set_find_status("第 1/23 处匹配");
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.find_visible);
    assert!(app.status.is_empty(), "关栏后查找进度应清除，实际 {:?}", app.status);
    // Esc 关栏同样清除
    dispatch(&mut app, Message::FindToggled);
    app.set_find_status("第 1/23 处匹配");
    dispatch(&mut app, Message::BarsDismissed);
    assert!(app.status.is_empty(), "Esc 关栏后查找进度应清除，实际 {:?}", app.status);
    // 换过别的状态则不得误清
    dispatch(&mut app, Message::FindToggled);
    app.set_find_status("第 1/23 处匹配");
    app.set_status("已保存");
    dispatch(&mut app, Message::FindToggled);
    assert_eq!(app.status, "已保存", "非查找状态不得被关栏误清");
}

// ---------- P153：点正文 → 查找框淡出 ----------

/// 点正文内容让查找框转半透明（用户点单「开启查找框之后点具体内容，
/// 查找框进入半透明」）：仅在栏开着时有意义；点回卡片 / 改查询 / 关栏
/// 都要复位，且淡出态不影响浮层布局（卡片框体不变）。
#[test]
fn p153_editor_click_dims_find_overlay_and_card_click_restores() {
    let mut app = app_with_lines(12);
    assert!(!app.find_dimmed, "初始应不透明");

    // 未开栏：点正文不进入淡出态（无框可淡）
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(!app.find_dimmed, "未开栏时点正文不应置淡出位");

    dispatch(&mut app, Message::FindToggled);
    assert!(app.find_visible && !app.find_dimmed, "开栏即不透明");

    // 点正文 → 淡出；重复点击幂等
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(app.find_dimmed, "栏开着时点正文应淡出");
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(app.find_dimmed, "连续点正文保持淡出（幂等）");

    // 淡出态下浮层布局不变：卡片仍是那一个定宽高卡片
    //（宽度口径与 `view::FIND_CARD_W` 一致 = 560；常量是 view 模块私有，
    //  测试里按 480..=600 区间认卡片，与 p150 的位置核对同口径）
    let nodes = app_layout_nodes(&app);
    assert!(
        nodes
            .iter()
            .any(|b| (480.0..=600.0).contains(&b.width) && b.height > 100.0),
        "淡出态下卡片框体应保持不变（仅背板透明度变化）"
    );

    // 点回卡片 → 复位
    dispatch(&mut app, Message::FindBoxPressed);
    assert!(!app.find_dimmed, "点回查找卡片应恢复不透明");

    // 在查询框里打字（焦点已回查找框）同样复位
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(app.find_dimmed);
    dispatch(&mut app, Message::FindQueryChanged("1".into()));
    assert!(!app.find_dimmed, "改查询即视为回到查找框操作");

    // 关栏清标记（重开恒为不透明）
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(app.find_dimmed);
    dispatch(&mut app, Message::FindToggled); // 关
    assert!(!app.find_dimmed, "关栏应清掉淡出标记");
    dispatch(&mut app, Message::FindToggled); // 开
    assert!(!app.find_dimmed, "重开查找框应为不透明");
}

/// P153 第四轮复报：**开查找框 → 打进数据 → 点正文中间 → 回查找框打字**
/// 之后，组字串在正文与查找框各画一份（上屏还可能落进文档）。
///
/// 序列化到状态上就是「正文控件的 `focused`（= 是否请求 IME 并内联画组字串）
/// 在回到查找框后仍为 true」：
/// * 点正文 → 正文控件 `pointer_focus(true)`；
/// * 点查找框 → 按下被 `PressObserver` 消费（必须消费，否则事件下传正文
///   再次夺焦），正文**收不到**「区外按下 = 交出焦点」的信号 → 保持 true。
///
/// 故 `FindBoxPressed` 必须显式把正文焦点让出去。本用例钉住这条——去掉
/// `pointer_focus(false)` 即变红。
#[test]
fn p153_returning_to_find_box_surrenders_editor_ime_focus() {
    let mut app = app_with_lines(12);
    dispatch(&mut app, Message::FindToggled);
    assert!(!app.cur_handle.borrow().focused, "开栏即让出正文焦点（P151）");

    // 点正文：正文控件接管焦点（IME 归属正文）
    app.cur_handle.borrow_mut().pointer_focus(true);
    assert!(app.cur_handle.borrow().focused);

    // 点查找框（输入框/按钮/空白任意一处都发这条）：必须让出正文焦点
    dispatch(&mut app, Message::FindBoxPressed);
    assert!(
        !app.cur_handle.borrow().focused,
        "回到查找框后正文不得再持焦——否则组字串在正文与查找框各画一份，\
         上屏文本还可能被正文吃进文档"
    );

    // 反复来回仍成立（幂等）
    app.cur_handle.borrow_mut().pointer_focus(true);
    dispatch(&mut app, Message::FindBoxPressed);
    assert!(!app.cur_handle.borrow().focused);

    // 反向路径不受影响：点正文仍由控件自己接管焦点
    dispatch(&mut app, Message::EditorBodyPressed);
    assert!(
        app.find_dimmed,
        "点正文照旧淡出（View 层的淡出与焦点让出互不影响）"
    );
}
