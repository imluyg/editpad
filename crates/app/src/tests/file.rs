use super::*;

    // ---------- P35 打开落新页的别名失步（P32 本轮发现） ----------

    #[test]
    fn opening_into_new_tab_keeps_editor_alias_in_sync() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq_a = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq_a,
                Ok((
                    editpad_core::Document::from_str("content A"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 打开 B：当前页已命名 → 落新页 idx1 并「聚焦」（active_tab 切过去）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq_b = app.job_seq;
        assert_eq!(app.active_tab, 1, "打开即聚焦新页");
        dispatch(
            &mut app,
            Message::Loaded(
                seq_b,
                Ok((
                    editpad_core::Document::from_str("content B"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 加载完成后直接输入（无任何显式切换）：必须落在聚焦的新页。
        // 失步缺陷下 cur_handle 仍指页 0——敲字打进上一个文档。
        // （打开文件后光标在文首 0:0，插入落在开头是既有正确语义；
        //  本测试钉住的是「落进哪一页」，不是行内位置。）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("+X".into())));
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "+Xcontent B",
            "输入必须落在聚焦的新页"
        );
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "content A",
            "旧页不得被误改"
        );
        // 渲染源与活动页一致
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "+Xcontent B");
    }

    #[test]
    fn preview_toggle_only_flips_for_markdown_documents() {
        // .md 扩展名经别名层得到 Markdown 语法 → 开关生效
        let mut md = Editpad::default();
        dispatch(&mut md, Message::FileDropped(PathBuf::from("C:/doc/readme.md")));
        let seq = md.job_seq;
        dispatch(
            &mut md,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("# 标题\n正文"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert!(!md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(!md.preview_visible);

        // 非 Markdown 页：不翻转并提示
        let mut app = json_app("{}");
        dispatch(&mut app, Message::PreviewToggled);
        assert!(!app.preview_visible);
        assert!(app.status.contains("仅支持 Markdown"), "{:?}", app.status);
    }

    // ---------- P22 第二批：格式化 JSON ----------

    /// 构造一个已按 .json 加载完成的应用。
    fn json_app(text: &str) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/x/data.json")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str(text);
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((doc, String::new(), "UTF-8".to_owned())),
            ),
        );
        app
    }

    #[test]
    fn format_json_pretty_prints_and_is_revertible() {
        let mut app = json_app("{\"b\":1,\"a\":[2,3]}");
        dispatch(&mut app, Message::FormatJson);
        let expected = "{\n  \"b\": 1,\n  \"a\": [\n    2,\n    3\n  ]\n}";
        assert_eq!(app.cur_handle.borrow().doc.to_text(), expected);
        assert!(app.tab().dirty, "格式化属于内容修改，必须置脏");
        assert_eq!(app.status, "已格式化 JSON");

        // 可撤销：replace_whole_document 走快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{\"b\":1,\"a\":[2,3]}",
            "撤销应还原到格式化前"
        );
    }

    #[test]
    fn format_json_reports_error_position_and_keeps_document() {
        let bad = "{\"a\": 1,,}";
        let mut app = json_app(bad);
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            bad,
            "校验失败不得改动文档"
        );
        assert!(
            app.status.contains("JSON 格式化失败") && app.status.contains("第"),
            "状态栏应带出错误行列，实际 {:?}",
            app.status
        );
    }

    #[test]
    fn format_json_is_noop_for_non_json_documents() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/notes/plain.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("{not:json,but:plain txt}");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{not:json,but:plain txt}",
            "非 JSON 文档不得被改动"
        );
        assert!(app.status.contains("仅对 JSON"), "应提示语法不匹配");
    }

    #[test]
    fn ctrl_shift_f_maps_to_format_json_but_ctrl_f_stays_find() {
        use iced::keyboard::{self};
        let shift_ctrl = keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("F".into()), shift_ctrl),
            Some(Message::FormatJson)
        ));
        // 普通 Ctrl+F 不受影响
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("f".into()), keyboard::Modifiers::CTRL),
            Some(Message::FindToggled)
        ));
    }

    // ---------- P18 即时保存 ----------

    #[test]
    fn edit_schedules_single_inflight_autosave_and_success_clears_dirty() {
        let mut app = loaded_txt_app();
        // P63：即时保存改为显式选择——本测试验证的是开启后的机制
        assert!(!app.settings.autosave_enabled);
        app.settings.autosave_enabled = true;

        // 编辑置脏并派发防抖任务（版本号在页上推进）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(
            app.tabs[0].autosave_inflight,
            "首次编辑应排队本页防抖任务"
        );
        let scheduled_version = app.tab().version;

        // 连续再编辑：inflight 去重不重复排队；版本继续推进
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        assert!(app.tabs[0].autosave_inflight);
        assert_eq!(app.tab().version, scheduled_version + 1);

        // 任务回报且版本一致 → 清脏解除挂起
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                scheduled_version + 1,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tab().dirty, "版本一致时落盘应清脏");
        assert!(!app.tabs[0].autosave_inflight);
    }

    #[test]
    fn autosave_stale_version_keeps_dirty() {
        // 快照之后又有编辑：迟到的「保存成功」不得清脏（否则丢改动标记）
        let mut app = loaded_txt_app();
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let stale = app.tab().version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        dispatch(
            &mut app,
            Message::TabAutosaved(0, stale, PathBuf::from("C:/doc/note.txt"), AutosaveOutcome::Written),
        );

        assert!(
            app.tab().dirty && !app.tabs[0].autosave_inflight,
            "版本不符应保持置脏并解除挂起"
        );
    }

    #[test]
    fn autosave_write_honors_tab_save_encoding_not_always_utf8() {
        // 自动保存落盘必须按标签页的保存编码：旧实现恒 UTF-8，会把用户
        // 选定 GBK 的文件静默转码覆写回磁盘。直击防抖线程体的落盘动作，
        // 钉死 GBK 形态（磁盘字节 ≠ UTF-8，且能被嗅探加载回原文本）。
        let dir = scratch_dir("autosave-encoding");
        let path = dir.join("gbk.txt");
        let text = "中文内容";
        let doc = editpad_core::Document::from_str(text);
        let outcome = write_to_disk(&path, &doc, editpad_core::SaveEncoding::Gbk, "off");
        assert!(
            matches!(outcome, AutosaveOutcome::Written),
            "落盘应成功，实际 {outcome:?}"
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_ne!(bytes, text.as_bytes(), "磁盘不得是 UTF-8 形态");
        let loaded = editpad_core::load_document_streaming(&path, |_| {}).unwrap();
        assert_eq!(loaded.doc.to_text(), text, "按 GBK 嗅探读回应无损");
        assert_eq!(loaded.encoding, "GBK");
    }

    #[test]
    fn autosave_failure_traces_status_keeps_dirty_and_allows_requeue() {
        let mut app = loaded_txt_app();
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tabs[0].autosave_inflight);

        let version = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                version,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Failed("disk full".into()),
            ),
        );

        assert!(!app.tabs[0].autosave_inflight, "失败也要解除挂起");
        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(
            app.status.contains("自动保存失败") && app.status.contains("disk full"),
            "失败必须留痕不能无声吞掉，实际 {:?}",
            app.status
        );

        // 失败解除挂起后，下一次编辑仍可重新排队
        dispatch(&mut app, Message::Edit(EditOp::InsertText("z".into())));
        assert!(app.tabs[0].autosave_inflight, "新编辑应重新排队");
    }

    #[test]
    fn autosave_skipped_for_untitled_or_disabled() {
        // 未命名文档（path == None）：绝不自动落盘
        let mut untitled = Editpad::default();
        dispatch(&mut untitled, Message::Edit(EditOp::InsertText("x".into())));
        assert!(untitled.tab().dirty);
        assert!(
            !untitled.tabs[0].autosave_inflight,
            "未命名文档不参与自动保存"
        );

        // 开关关闭（P63 起即默认状态）：已命名页同样不排队
        let mut app = loaded_txt_app();
        assert!(!app.settings.autosave_enabled, "P63：默认关闭");
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(!app.tabs[0].autosave_inflight, "开关关闭时不排队");

        // busy（手动 IO 进行中）时也跳过——显式开启后守卫仍生效
        let mut busy_app = loaded_txt_app();
        busy_app.settings.autosave_enabled = true;
        busy_app.busy = true;
        dispatch(&mut busy_app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(!busy_app.tabs[0].autosave_inflight, "busy 时不得排队自动保存");
    }

    // ---------- P63 保存策略对标主流编辑器 ----------

    #[test]
    fn p63_default_settings_never_autowrite_existing_files() {
        // 用户点名（对标主流编辑器）：默认设置下编辑已有文件只置脏，
        // 绝不悄悄排队写盘；● 标记与关窗确认照旧兜底
        let (mut app, path) = loaded_real_file_app("p63-default-off");

        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(
            !app.tabs[0].autosave_inflight,
            "默认关闭时编辑不得触发写盘任务"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "base",
            "默认设置下磁盘必须原封不动"
        );
    }

    #[test]
    fn p63_autosave_skip_on_external_change_queues_prompt_and_keeps_dirty() {
        // 防抖睡眠期间文件被外部改动 → 写前校验拒写：保持置脏、送入
        // P52 提示条队列、留痕状态栏，未裁决前不重记戳
        let (mut app, path) = loaded_real_file_app("p63-skip-ext");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tabs[0].autosave_inflight);
        let old_stamp = app.tabs[0].file_stamp;
        let version = app.tab().version;

        fs::write(&path, "external edit made this longer").unwrap();

        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                version,
                path.clone(),
                AutosaveOutcome::SkippedExternalChange,
            ),
        );
        assert!(app.tab().dirty, "拒写必须保持置脏");
        assert!(!app.tabs[0].autosave_inflight, "挂起照常解除，可重新排队");
        assert_eq!(
            app.external_change,
            Some(vec![0]),
            "拒写应把页送进 P52 提示条队列"
        );
        assert!(app.status.contains("外部修改"), "实际 {:?}", app.status);
        assert_eq!(
            app.tabs[0].file_stamp, old_stamp,
            "用户裁决前不得按磁盘现状重记戳"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "external edit made this longer",
            "拒写后磁盘内容必须还是外部版本"
        );
    }

    #[test]
    fn p63_written_outcome_clears_dirty_and_mismatched_path_report_is_dropped() {
        // 正常路径：路径匹配 + 版本一致 → 清脏（新载荷下的既有语义）
        let (mut app, path) = loaded_real_file_app("p63-written");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let v = app.tab().version;

        // 先来一条路径不符的回报（下标漂移竞态）——必须整条丢弃，
        // 不得清脏/解除挂起；随后正确路径的同版本回报照常生效
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                v,
                PathBuf::from("C:/other/renamed.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(
            app.tab().dirty && app.tabs[0].autosave_inflight,
            "路径不符的回报必须整条丢弃"
        );

        dispatch(
            &mut app,
            Message::TabAutosaved(0, v, path, AutosaveOutcome::Written),
        );
        assert!(!app.tab().dirty && !app.tabs[0].autosave_inflight);
    }

    #[test]
    fn p63_manual_save_blocked_on_external_change_until_acknowledged() {
        // 应用聚焦期间文件被外部改（无焦点事件 → P50 巡检不触发），
        // Ctrl+S 必须拦截而不是无声覆盖；〔忽略〕重记戳后再存 = 有意覆盖
        let (mut app, path) = loaded_real_file_app("p63-manual-guard");
        std::fs::write(&path, "externally replaced").unwrap();

        dispatch(&mut app, Message::SaveRequested);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "externally replaced",
            "拦截期间原文件绝不能被覆盖"
        );
        assert!(!app.busy, "拦截不是进入保存流程");
        assert_eq!(app.external_change, Some(vec![0]), "应弹 P52 提示条交裁决");
        assert!(app.status.contains("外部修改"), "实际 {:?}", app.status);

        // 〔忽略〕= 按磁盘现状重记戳并收条；随后 Ctrl+S 守卫放行进入保存管线
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::SaveRequested);
        assert!(app.busy, "有意覆盖的第二步应正常走保存");
    }

    #[test]
    fn p63_save_as_target_is_restamped_not_blocked() {
        // 另存为：对话框里显式选中的目标（可能已存在且内容不同）不该被
        // 自家外部修改守卫拦下——选定目标即按其磁盘现状记戳
        let target = scratch_dir("p63-saveas").join("existing-target.txt");
        std::fs::write(&target, "old content on disk").unwrap();

        let mut app = Editpad::default(); // 未命名页，file_stamp = None
        dispatch(&mut app, Message::Edit(EditOp::InsertText("draft".into())));
        dispatch(&mut app, Message::SaveTargetChosen(Some(target.clone())));

        assert_eq!(app.tabs[0].path, Some(target.clone()));
        assert_eq!(
            app.tabs[0].file_stamp,
            file_stamp(&target),
            "选定目标即按其磁盘现状重记戳"
        );
        assert!(app.busy, "另存为应直接进入保存管线而不被拦");
    }

    // ---------- P67 编码与行尾控制 ----------

    #[test]
    fn p67_status_menus_toggle_mutually_and_esc_closes() {
        let mut app = loaded_txt_app();

        dispatch(&mut app, Message::ToggleEncodingMenu);
        eprintln!("[P67] after enc-toggle: enc={} eol={}", app.encoding_menu, app.eol_menu);
        assert!(app.encoding_menu && !app.eol_menu);

        // 互斥：开行尾关编码
        dispatch(&mut app, Message::ToggleEolMenu);
        assert!(!app.encoding_menu && app.eol_menu);

        // busy 时不得「打开」菜单（已开的菜单由各项自身守卫兜底）
        dispatch(&mut app, Message::ToggleEolMenu); // 再 toggle 一次 = 收起
        assert!(!app.encoding_menu && !app.eol_menu, "两菜单此时应全关");
        app.busy = true;
        dispatch(&mut app, Message::ToggleEncodingMenu);
        assert!(!app.encoding_menu, "busy 时不得开菜单");
        app.busy = false;

        // Esc（BarsDismissed）一并收起
        dispatch(&mut app, Message::ToggleEncodingMenu);
        assert!(app.encoding_menu);
        dispatch(&mut app, Message::BarsDismissed);
        assert!(!app.encoding_menu && !app.eol_menu);
    }

    #[test]
    fn p67_save_with_encoding_sets_pref_and_enters_save_pipeline() {
        let (mut app, _path) = loaded_real_file_app("p67-enc-pref");

        dispatch(&mut app, Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk));
        assert_eq!(
            app.tabs[0].save_encoding,
            Some(editpad_core::SaveEncoding::Gbk),
            "偏好必须记住（此后每次保存沿用）"
        );
        assert!(app.busy, "选择编码后应立即走保存管线");
    }

    #[test]
    fn p67_save_with_encoding_rejected_for_untitled() {
        let mut app = Editpad::default(); // 未命名页
        dispatch(&mut app, Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk));
        assert!(app.tabs[0].save_encoding.is_none(), "无路径不得记偏好");
        assert!(!app.busy);
        assert!(app.status.contains("另存为"), "应提示先另存为");
    }

    #[test]
    fn p67_saved_reflects_target_label_and_unmappable_warning() {
        let (mut app, _path) = loaded_real_file_app("p67-saved-label");
        app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);
        // 模拟载入自 GBK 文件（标签为 GBK）
        app.tabs[0].encoding_label = "GBK".to_owned();
        let v = app.tab().version;

        // 同编码保存：无转码提示
        dispatch(
            &mut app,
            Message::Saved(v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert_eq!(app.tabs[0].encoding_label, "GBK", "标签反映实际落盘编码");
        assert!(app.status.is_empty(), "同编码不得提示转码，实际 {:?}", app.status);

        // 不可映射：优先告警
        app.tabs[0].encoding_label = "GBK".to_owned();
        dispatch(
            &mut app,
            Message::Saved(
                v,
                Ok(editpad_core::EncodeNotice { unmappable: true }),
            ),
        );
        assert!(app.status.contains("&#"), "应提示数值实体写入：{:?}", app.status);

        // 默认（无偏好）保存 GBK 载入的文件 → 转码提示（P6 语义保持）
        app.tabs[0].save_encoding = None;
        app.tabs[0].encoding_label = "GBK".to_owned();
        dispatch(
            &mut app,
            Message::Saved(v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert_eq!(app.tabs[0].encoding_label, "UTF-8");
        assert!(app.status.contains("GBK"), "应提示原编码：{:?}", app.status);
    }

    #[test]
    fn saved_ok_surfaces_stashed_backup_notice() {
        // 备份提示写在异步落盘完成之前，会被 Saved 分支立即覆盖/抹掉；
        // 暂存补显机制钉死：无转码补显、转码让位、失败弃置。
        let (mut app, _path) = loaded_real_file_app("backup-notice");
        let v = app.tab().version;
        app.pending_backup_notice = Some("已备份旧版 → x.bak".to_owned());
        dispatch(&mut app, Message::Saved(v, Ok(editpad_core::EncodeNotice::default())));
        assert!(
            app.status.contains("已备份"),
            "落盘成功后补显备份提示，实际 {:?}",
            app.status
        );
        assert!(app.pending_backup_notice.is_none(), "补显即取走");

        // 转码提示优先：备份提示让位且不留存
        app.pending_backup_notice = Some("已备份旧版 → x.bak".to_owned());
        app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);
        app.tabs[0].encoding_label = "UTF-8".to_owned();
        let v = app.tab().version;
        dispatch(&mut app, Message::Saved(v, Ok(editpad_core::EncodeNotice::default())));
        assert!(!app.status.contains("已备份"), "转码知情权优先，实际 {:?}", app.status);
        assert!(app.pending_backup_notice.is_none());
    }

    #[test]
    fn error_status_survives_edit_noise() {
        // 「保存失败」等错误必须持久：编辑噪声不清除，直到下一条信息
        // 状态让位。旧实现打一个字就把错误抹掉且无日志可查。
        let (mut app, _path) = loaded_real_file_app("error-sticky");
        let v = app.tab().version;
        let err = Err("无法写入文件 x: disk full".to_owned());
        dispatch(&mut app, Message::Saved(v, err));
        assert!(app.status.contains("保存失败"));
        assert!(app.status_is_error, "错误必须带类型标记");

        // 编辑一次：错误仍在（旧实现此处被 status.clear() 抹掉）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(
            app.status.contains("保存失败"),
            "错误不得被编辑噪声清除，实际 {:?}",
            app.status
        );
        assert!(app.status_is_error);

        // 下一条信息状态正常让位并复位类型
        app.set_status("已替换 1 处");
        assert!(!app.status_is_error);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        assert!(app.status.is_empty(), "普通信息照旧被编辑清除，实际 {:?}", app.status);
    }

    #[test]
    fn p67_convert_eol_rewrites_document_undoably() {
        let (mut app, _path) = loaded_real_file_app("p67-eol");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("a\nb\nc\n"));
        }
        let v0 = app.tab().version;

        // LF → CRLF：内容改写、置脏、版本推进
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::CrLf));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\r\nb\r\nc\r\n",
            "行尾应统一为 CRLF"
        );
        assert!(app.tab().dirty, "行尾转换是真实文档编辑");
        assert_eq!(app.tab().version, v0 + 1, "版本应推进（自动保存触发依据）");
        assert_eq!(
            app.cur_handle.borrow().doc.line_ending(),
            editpad_core::LineEnding::CrLf
        );

        // 已是目标：提示且不动文档
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::CrLf));
        assert!(app.status.contains("已是"), "实际 {:?}", app.status);
        assert_eq!(app.tab().version, v0 + 1, "无变化不得推进版本");

        // CRLF → LF：可撤销（撤销回 LF 原文）
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::Lf));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "a\nb\nc\n");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\r\nb\r\nc\r\n",
            "行尾转换必须可撤销"
        );
    }

    #[test]
    fn p67_loaded_and_save_as_reset_encoding_preference() {
        // 重新载入同一文件：编码偏好重置为默认 UTF-8
        // （先派发 FileDropped 建新加载任务，否则 Loaded 会被过期守卫丢弃；
        // 当前页非空净 → 打开落在新页，断言按路径定位而非固定下标）
        let (mut app, path) = loaded_real_file_app("p67-pref-reset");
        app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);

        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("base"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        // start_loading「打开即聚焦」：活动页 = 刚重载完成的那一页
        assert_eq!(
            app.tab().path.as_deref(),
            Some(path.as_path()),
            "活动页应为重载的文件"
        );
        assert!(
            app.tab().save_encoding.is_none(),
            "重载必须重置编码偏好"
        );

        // 另存为新路径：同样重置（旧偏好属于旧路径）——先给活动页设偏好
        app.tab_mut().save_encoding = Some(editpad_core::SaveEncoding::Utf8Bom);
        let target = scratch_dir("p67-pref-reset2").join("new.txt");
        dispatch(&mut app, Message::SaveTargetChosen(Some(target)));
        assert!(
            app.tab().save_encoding.is_none(),
            "另存为必须重置编码偏好"
        );
    }

    // ---------- P6 编码知情权 ----------

    #[test]
    fn transcode_notice_covers_all_encoding_labels() {
        use editpad_core::SaveEncoding;
        // P67 口径：提示按「原标签 vs 实际目标」判定
        // 纯 UTF-8 → UTF-8 / 未打开：无需提示
        assert_eq!(transcode_notice("UTF-8", "UTF-8", false), None);
        assert_eq!(transcode_notice("", "UTF-8", false), None);

        // 用户显式选择 GBK 且原文件就是 GBK：不提示（非意外转码）
        assert_eq!(
            transcode_notice("GBK", SaveEncoding::Gbk.label(), false),
            None
        );

        // 不可映射字符优先告警
        let m = transcode_notice("UTF-8", "GBK", true).expect("应有告警");
        assert!(m.contains("&#"), "应说明数值实体写入：{m}");

        // BOM 丢失要提示
        let bom = transcode_notice("UTF-8(BOM)", "UTF-8", false).expect("BOM 丢失应有提示");
        assert!(bom.contains("BOM"));

        // 转码要提示且带出原编码名与目标
        for label in ["GBK", "UTF-16LE", "UTF-16BE"] {
            let notice =
                transcode_notice(label, "UTF-8", false).expect("转码应有提示");
            assert!(notice.contains(label), "提示需含原编码 {label}: {notice}");
            assert!(notice.contains("UTF-8"));
        }

        // 反向：UTF-8 → GBK 同样提示
        let back = transcode_notice("UTF-8", SaveEncoding::Gbk.label(), false)
            .expect("反向转码应有提示");
        assert!(back.contains("GBK") && back.contains("UTF-8"));
    }

    #[test]
    fn load_stream_happy_path_emits_progress_then_done() {
        let dir = scratch_dir("happy");
        let path = dir.join("note.txt");
        // 超过单块（64KB）以触发多次进度回调
        let content = "Editpad 加载流测试\n".repeat(8_000);
        std::fs::write(&path, &content).unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                7,
                path.clone(),
                |p, cb| editpad_core::load_document_streaming(p, cb),
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let progress_count = messages
            .iter()
            .filter(|m| matches!(m, Message::LoadProgress(7, _, _)))
            .count();
        assert!(progress_count >= 2, "应有多条进度消息，实际 {progress_count}");

        let dones: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(id, result) => Some((*id, result)),
                _ => None,
            })
            .collect();
        assert_eq!(dones.len(), 1, "恰好一条 Loaded");
        let (id, Ok((doc, _sample, encoding))) = dones[0] else {
            panic!("应为成功加载，实际 {:?}", dones[0]);
        };
        assert_eq!(id, 7);
        // P19：消息携带的是 rope 文档本体
        assert_eq!(doc.to_text(), content);
        assert_eq!(encoding.as_str(), "UTF-8");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_stream_reports_failure_when_loader_panics() {
        // P5 回归：加载线程崩溃也必须让 UI 收到 Loaded(Err)，busy 才能解除
        let dir = scratch_dir("panic");
        let path = dir.join("boom.txt");
        std::fs::write(&path, b"data").unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                9,
                path.clone(),
                |_p, _cb| -> Result<editpad_core::LoadedDocument, editpad_core::CoreError> {
                    panic!("模拟加载线程崩溃");
                },
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let failures: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(_, Err(text)) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1, "panic 后应恰好一条失败 Loaded，实际 {messages:?}");
        assert!(failures[0].contains("加载线程崩溃"), "错误需含兜底前缀: {}", failures[0]);
        assert!(failures[0].contains("模拟加载线程崩溃"), "错误需含 panic 信息: {}", failures[0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P103：命令行参数打开（双击文件 / 「打开方式」） ----------

    #[test]
    fn cli_file_args_skips_exe_and_options_honors_dashdash() {
        use std::ffi::OsString;
        // 模拟资源管理器调用：exe 路径 + 两个真实文件 + 一个未知选项 +
        // `--` 后以 `-` 开头的合法文件名
        let args: Vec<OsString> = [
            "E:\\Tools\\editpad.exe",
            "C:/notes/a.txt",
            "-fullscreen",
            "--",
            "-dash-name.md",
            "D:/logs/app.log",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            parse_cli_file_args(args),
            vec![
                PathBuf::from("C:/notes/a.txt"),
                PathBuf::from("-dash-name.md"),
                PathBuf::from("D:/logs/app.log"),
            ]
        );
    }

    #[test]
    fn cli_file_args_without_files_is_empty() {
        use std::ffi::OsString;
        // 无参数（直接双击 exe）：空清单 = 走会话恢复原行为
        assert!(
            parse_cli_file_args(vec![OsString::from("editpad.exe")]).is_empty(),
            "仅有程序自身参数时应无可打开文件"
        );
        // 纯选项启动（未来预留）：同样不开文件
        assert!(
            parse_cli_file_args(vec![
                OsString::from("editpad.exe"),
                OsString::from("--help"),
            ])
            .is_empty(),
            "未知选项一律忽略"
        );
    }

    #[test]
    fn boot_cli_kickoff_registers_first_file_and_queues_rest() {
        let mut app = Editpad::default();
        // boot 分支语义：首个文件同步登记加载任务（状态变更），其余排队
        app.boot_cli_kickoff(vec![
            PathBuf::from("C:/cli/a.txt"),
            PathBuf::from("C:/cli/b.txt"),
            PathBuf::from("C:/cli/c.txt"),
        ]);
        assert_eq!(app.job_seq, 1, "首个文件应立即登记加载任务");
        assert_eq!(app.active_tab, 0, "首个文件落入初始空净页（打开即聚焦）");
        assert!(app.busy, "登记后应置 busy（单任务承接）");
        assert_eq!(
            app.active_load.as_ref().map(|j| j.path.clone()),
            Some(PathBuf::from("C:/cli/a.txt")),
            "在途任务必须是首个文件"
        );
        assert_eq!(app.pending_cli.len(), 2, "其余文件应留在队列串行续排");
        assert_eq!(
            app.pending_cli,
            VecDeque::from(vec![
                PathBuf::from("C:/cli/b.txt"),
                PathBuf::from("C:/cli/c.txt"),
            ]),
            "队列顺序必须保持命令行顺序"
        );
    }

    #[test]
    fn cli_files_open_sequentially_via_pending_queue() {
        let mut app = Editpad::default();
        // 模拟 boot 注入的待开文件清单（无参数时该队列为空）
        app.pending_cli =
            vec![PathBuf::from("C:/cli/a.txt"), PathBuf::from("C:/cli/b.md")].into();

        // 首发（boot 的 OpenNextCliFile 任务）：弹出第一个 → 登记加载任务
        dispatch(&mut app, Message::OpenNextCliFile);
        assert_eq!(app.job_seq, 1, "首发应登记第一个文件的加载任务");
        assert_eq!(app.pending_cli.len(), 1, "队列应弹出第一个文件");
        assert!(app.busy, "加载进行中应置 busy，防止并发任务");

        // 第一个加载完成 → busy 结算、队列非空 → 自动续排第二个
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("first"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.job_seq, 2, "Loaded 结算后应自动弹出下一个 CLI 文件");
        assert_eq!(app.active_tab, 1, "第二个文件应落新页并聚焦（打开即聚焦）");
        assert!(
            app.pending_cli.is_empty(),
            "第二个文件已弹出在途，队列应清空"
        );
        assert!(app.busy, "第二个文件加载中");

        // 第二个加载完成：两页内容各自就位，队列清空
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("second"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert!(app.pending_cli.is_empty(), "队列应随最后一个文件清空");
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "first");
        assert_eq!(app.tabs[1].editor.borrow().doc.to_text(), "second");
    }

    #[test]
    fn cli_open_failure_does_not_block_the_rest_of_queue() {
        let mut app = Editpad::default();
        // 第一个文件不存在：加载失败应只提示，不阻断后续文件
        app.pending_cli = vec![
            PathBuf::from("C:/cli/missing.txt"),
            PathBuf::from("C:/cli/ok.txt"),
        ]
        .into();
        dispatch(&mut app, Message::OpenNextCliFile);
        let seq1 = app.job_seq;
        dispatch(&mut app, Message::Loaded(seq1, Err("文件不存在".to_owned())));
        assert_eq!(
            app.job_seq, 2,
            "单个文件打开失败不得阻断队列里后续文件"
        );
        assert!(
            app.pending_cli.is_empty(),
            "失败结算后队列应继续弹出下一个文件"
        );
        assert!(app.busy, "下一个文件已开始加载（失败不中断排队链）");
    }

