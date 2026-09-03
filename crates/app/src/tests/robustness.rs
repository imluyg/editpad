use super::*;

    // ---------- 健壮性边界用例批 ----------

    #[test]
    fn duplicate_open_while_loading_is_guards_and_reload_after_done() {
        // 加载进行中再次拖入同一文件：busy 守卫必须拒绝重入（防双任务竞态）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert!(app.active_load.is_some(), "首次拖入应登记加载任务");
        let seq_first = app.job_seq;

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(app.job_seq, seq_first, "加载中不得排队第二个任务");

        // 加载完成后允许重复打开同一文件（每次新 job，无路径去重）
        let doc = editpad_core::Document::from_str("reloaded\n");
        dispatch(
            &mut app,
            Message::Loaded(seq_first, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none(), "完成后任务应解除");
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(
            app.job_seq,
            seq_first + 1,
            "完成后的重复打开必须作为新任务重新加载"
        );
    }

    #[test]
    fn close_during_load_exits_without_confirm_bar_or_stuck_state() {
        // 加载中关窗：dirty 必为 false（打开确认已清），应走直接关窗路径，
        // 不弹未保存确认条、不残留 pending 状态。
        // P29：快照直退路径有专门测试（注入目录），此处关掉开关钉住降级行为，
        // 同时避免测试触碰真实 %APPDATA%。
        let mut app = Editpad::default();
        app.settings.enable_snapshots = false;
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/big.log")));
        assert!(app.active_load.is_some());
        assert!(!app.tab().dirty);

        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert_eq!(app.main_window, Some(id), "窗口 id 应被捕获供 close 使用");
        assert!(
            !app.confirm_visible && !app.pending_close,
            "加载中关窗不得触发确认条"
        );
        // 关窗后迟到的加载完成消息照常按 job 过滤消化，不 panic
        let doc = editpad_core::Document::from_str("late arrival");
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none());
    }

    // ---------- P49 健壮性边界输入（app 层；core 层见 boundary_inputs.rs） ----------

    #[test]
    fn goto_line_beyond_document_and_invalid_input_are_clamped() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("a\nb\nc".into())));
        assert_eq!(app.cur_handle.borrow().doc.line_count(), 3);

        // 超大行号：夹到末行行首，不 panic
        dispatch(&mut app, Message::GotoInputChanged("999999999".into()));
        dispatch(&mut app, Message::GotoSubmit);
        assert_eq!(app.cur_handle.borrow().cursor.line, 2);
        assert_eq!(app.cur_handle.borrow().cursor.col, 0);
        assert!(!app.goto_visible, "有效跳转后收起输入条");

        // 前导/尾随空白容忍（输入框手滑）
        dispatch(&mut app, Message::GotoInputChanged("  2  ".into()));
        dispatch(&mut app, Message::GotoSubmit);
        assert_eq!(app.cur_handle.borrow().cursor.line, 1);

        // 0 / 非数字 / 负数形态：报错提示、输入条保持、光标不动
        //（先重开输入条——上面的有效跳转已把它收起）
        dispatch(&mut app, Message::GotoToggled);
        assert!(app.goto_visible);
        for bad in ["0", "abc", "-3"] {
            dispatch(&mut app, Message::GotoInputChanged(bad.to_owned()));
            dispatch(&mut app, Message::GotoSubmit);
            assert!(!app.status.is_empty(), "非法行号 {bad} 必须有提示");
            assert!(app.goto_visible, "非法行号 {bad} 不收起输入条");
        }
        assert_eq!(app.cur_handle.borrow().cursor.line, 1, "非法输入不动光标");
    }

    #[test]
    fn reopen_same_file_twice_lands_consistently() {
        let mut app = Editpad::default();
        let path = PathBuf::from("C:/same.txt");

        // 第一次打开：落入初始空净页
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("v1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].path.as_ref(), Some(&path));

        // 重复打开同一路径：当前页非空 → 新开一页承接，不覆盖、不 panic
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq2 = app.job_seq;
        assert_eq!(app.active_tab, 1, "重复打开落新页");
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("v1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        assert_eq!(app.tabs.len(), 2);
        for tab in &app.tabs {
            assert_eq!(tab.path.as_ref(), Some(&path));
            assert_eq!(tab.editor.borrow().doc.to_text(), "v1");
        }
    }

    #[test]
    fn close_window_during_active_load_degrades_safely() {
        // 「每次询问」模式避免触碰真实 %APPDATA%（既有关窗测试同款守则）
        let mut app = Editpad::default();
        app.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();

        // 加载进行中：busy + 在途任务 + 占位页干净
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/loading.txt")));
        assert!(app.busy && app.active_load.is_some());

        // 此刻关窗：占位页干净 → 直接关窗路径，不 panic、不弹确认
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(!app.confirm_visible, "加载中关窗（干净页）不弹确认");
        assert_eq!(app.main_window, Some(id), "关窗流必须捕获窗口 id");

        // 变体：加载中且后台有置脏页 → 确认条兜底（数据不丢）。
        // 布局 = 页0 置脏、页1 干净且活动（加载落活动页，脏页在后台）
        let mut app2 = Editpad::default();
        app2.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("dirty".into())));
        dispatch(&mut app2, Message::NewTab);
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/loading2.txt")));
        assert!(app2.active_load.is_some());
        let id2 = iced::window::Id::unique();
        let _ = app2.update(Message::CloseRequested(id2));
        assert!(app2.confirm_visible, "加载中关窗（置脏页）必须弹确认");

        // 快照直退模式 + 注入目录：加载中关窗照走快照直退，不卡死
        let dir = scratch_dir("p49-close-during-load");
        let mut app3 = Editpad::default();
        dispatch(&mut app3, Message::FileDropped(PathBuf::from("C:/loading3.txt")));
        let id3 = iced::window::Id::unique();
        let _ = app3.handle_close_request(id3, Some(dir.clone()));
        assert!(!app3.confirm_visible);
        // 占位页（干净未命名）也入清单——下次启动恢复为空页，无数据丢失
        assert!(editpad_core::read_manifest(&dir).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

