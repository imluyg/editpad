use super::*;

    /// 外部修改提示条的布局契约。
    ///
    /// 两种失败都严重：条**渲染不出来** = 用户永远看不到外部改动（静默失败）；
    /// 条**覆盖在正文上** = 挡住正在编辑的内容。故同时钉住：
    /// ① 条确实存在于**正文之下**（正文底与视口底之间）；
    /// ② 存在可点控件（重载 / 忽略两个按钮）——条不是一段死文字。
    ///
    /// 走生产视图树；此前本文件的三处只写 `let _ = app.view()`（仅验「不 panic」）。
    fn assert_external_change_bar(app: &Editpad) {
        let ui = ViewTree::layout_default(app);
        let body = ui.editor_body();
        let below_body = |b: &iced::Rectangle| b.y >= body.y + body.height - 0.5;
        let bar = ui
            .find(|b| (20.0..=60.0).contains(&b.height) && below_body(&b))
            .unwrap_or_else(|| panic!("外部修改提示条应渲染在正文之下；\n树形：\n{}", ui.dump()));
        assert!(
            bar.y + bar.height <= ui.viewport().height,
            "提示条不得溢出视口底部：{bar:?}"
        );
        assert!(
            ui.find(|b| (40.0..=110.0).contains(&b.width)
                && (18.0..=36.0).contains(&b.height)
                && b.y >= body.y + body.height - 0.5)
                .is_some(),
            "提示条内应有可点按钮（重载 / 忽略）；\n树形：\n{}",
            ui.dump()
        );
    }

    // ---------- P32 最近文件光标/滚动记忆 ----------

    #[test]
    fn reopened_recent_file_restores_cursor_and_scroll() {
        let mut app = Editpad::default();
        // 文档要足够大（默认视口 ~400px 高可容纳全部行时，
        // restore_view 的行程钳制会把小滚动值顶到 max+1——P30 设计语义）
        let mut text = String::new();
        for i in 1..=60 {
            text.push_str(&format!("line{i}\n"));
        }
        let doc = || editpad_core::Document::from_str(&text);

        // 首开 C:/a.txt：进最近列表（尚无记忆）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq1, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.settings.recent_view(Path::new("C:/a.txt")).is_none());

        // 种入上次退出时的位置（模拟历史记忆）
        assert!(app.settings.set_recent_view(
            Path::new("C:/a.txt"),
            editpad_core::RecentView { line: 2, col: 1, scroll_top: 5.0 },
        ));

        // 重开同一文件（当前页已命名非空 → 落新页并聚焦）→ 光标滚动就位
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq2, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        let ed = app.cur_handle.borrow();
        assert_eq!(
            ed.cursor,
            crate::editor::CursorPos { line: 2, col: 1 },
            "光标应回到记忆位置"
        );
        assert_eq!(ed.scroll_top, 5.0, "垂直滚动应回到记忆位置");
    }

    #[test]
    fn closing_named_tab_writes_back_cursor_memory_to_disk() {
        let dir = scratch_dir("p32-writeback");
        let mut app = Editpad::default();
        // 注入配置路径：验证回写真的落盘，且绝不碰真实 %APPDATA%
        app.settings_path_override = Some(dir.join("config.toml"));

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("r0\nr1\nr2\nr3"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 光标挪到第 3 行（jump_to_line 1 基 → 0 基 line=2），页面干净可直接关
        app.cur_handle.borrow_mut().jump_to_line(3);
        assert!(!app.tab().dirty);
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.tabs.len(), 1, "干净命名页应被关闭");

        let recorded = app.settings.recent_view(Path::new("C:/b.txt"));
        assert_eq!(
            recorded.map(|v| v.line),
            Some(2),
            "关页必须把光标行（0 基）写回记忆"
        );
        // 注入路径端到端：重载 config.toml 记忆仍在
        let persisted = editpad_core::Settings::load_from(&dir.join("config.toml"));
        assert_eq!(persisted.recent_view(Path::new("C:/b.txt")).map(|v| v.line), Some(2));

        fs_remove_dir_all(&dir);
    }

    #[test]
    fn window_close_writes_back_all_named_tabs_views() {
        let mut app = Editpad::default();
        let dir = scratch_dir("p32-windowclose");
        app.settings_path_override = Some(dir.join("config.toml"));

        // 两页各开一个文件，光标停在不同行
        for (name, jump) in [("C:/w1.txt", 1usize), ("C:/w2.txt", 3usize)] {
            dispatch(&mut app, Message::FileDropped(PathBuf::from(name)));
            let seq = app.job_seq;
            dispatch(
                &mut app,
                Message::Loaded(
                    seq,
                    Ok((
                        editpad_core::Document::from_str("a\nb\nc\nd\ne"),
                        String::new(),
                        "UTF-8".to_owned(),
                    )),
                ),
            );
            if jump > 1 {
                app.cur_handle.borrow_mut().jump_to_line(jump);
            }
        }
        assert_eq!(app.tabs.len(), 2);

        // 关窗请求（快照直退默认路径；目录传 None 走降级不影响断言）：
        // 入口处必须先把两页的光标都记账
        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, None);

        assert_eq!(
            app.settings.recent_view(Path::new("C:/w1.txt")).map(|v| v.line),
            Some(0),
            "未跳转的页记文首"
        );
        assert_eq!(
            app.settings.recent_view(Path::new("C:/w2.txt")).map(|v| v.line),
            Some(2),
            "跳转过光标的页按 0 基记账"
        );

        fs_remove_dir_all(&dir);
    }

    /// std::fs::remove_dir_all 的薄封装（测试尾部清理）。
    fn fs_remove_dir_all(dir: &Path) {
        std::fs::remove_dir_all(dir).ok();
    }

    // ---------- P29 会话快照直退（关窗状态机） ----------

    /// 项目内落盘目录：快照测试绝不触碰真实 %APPDATA%。
    fn snapshot_scratch_dir(tag: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("app-p29-{tag}-{}", std::process::id()))
    }

    #[test]
    fn snapshot_exit_writes_session_and_closes_without_confirm() {
        let dir = snapshot_scratch_dir("exit");
        let mut app = Editpad::default();

        // 页 0：未命名页，已编辑置脏（未命名页正是 auto-save 的盲区）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿内容\r\n第二行".into())));
        // 页 1：命名干净页——只记元数据不写内容文件
        let mut named = Tab::empty();
        named.path = Some(PathBuf::from("C:/work/notes.md"));
        app.tabs.push(named);

        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, Some(dir.clone()));

        assert!(!app.confirm_visible, "快照直退零询问");
        assert_eq!(app.main_window, Some(id));

        // 清单完整落盘且可解析
        let manifest = editpad_core::snapshot::read_manifest(&dir)
            .expect("关窗后必须有可解析的会话清单");
        assert!(manifest.clean_exit, "正常退出必须带收尾标记");
        assert_eq!(manifest.tabs.len(), 2);
        assert_eq!(manifest.active, 0);
        assert_eq!(manifest.next_untitled, app.untitled_next);

        // 置脏未命名页：序号延续 + 内容逐字回来
        // （插入文本经 P9 归一为文档主导行尾 LF;CRLF 保真已由 core 测试钉住）
        let t0 = &manifest.tabs[0];
        assert!(t0.dirty);
        assert_eq!(t0.path, None);
        assert_eq!(t0.untitled_num, Some(1));
        let doc = editpad_core::snapshot::read_page(&dir, t0).expect("置脏页必须有快照");
        assert_eq!(doc.to_text(), "草稿内容\n第二行");

        // 干净命名页：路径记住、无内容文件
        let t1 = &manifest.tabs[1];
        assert!(!t1.dirty);
        assert_eq!(t1.path.as_deref(), Some("C:/work/notes.md"));
        assert_eq!(t1.file, None);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn snapshot_exit_captures_cursor_and_scroll_positions() {
        let dir = snapshot_scratch_dir("cursor");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("a\nb\nc\n".into())));
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.jump_to_line(3); // 跳到末行
            ed.scroll_top = 7.5;
            ed.scroll_left = 2.0;
        }
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        let t0 = &manifest.tabs[0];
        assert_eq!(t0.cursor_line, 2, "光标行（0 基）必须被记录");
        assert!((t0.scroll_top - 7.5).abs() < f32::EPSILON);
        assert!((t0.scroll_left - 2.0).abs() < f32::EPSILON);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn disabled_or_ask_mode_falls_back_to_confirm_bar_without_writes() {
        let dir = snapshot_scratch_dir("fallback");
        fs::create_dir_all(&dir).unwrap();

        // 开关关闭 + 置脏 → 旧确认条，且磁盘上什么都没写
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        dispatch(&mut off, Message::Edit(EditOp::InsertText("x".into())));
        let _ = off.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(off.confirm_visible, "开关关闭必须回退旧确认条");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "降级路径不得写任何快照"
        );

        // 模式为「每次询问」+ 置脏 → 同样回退
        let mut ask = Editpad::default();
        ask.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut ask, Message::Edit(EditOp::InsertText("y".into())));
        let _ = ask.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(ask.confirm_visible);
        assert!(editpad_core::snapshot::read_manifest(&dir).is_none());

        // 目录不可用（None）→ 功能自动降级，同样回退确认条
        let mut nodir = Editpad::default();
        dispatch(&mut nodir, Message::Edit(EditOp::InsertText("z".into())));
        let _ = nodir.handle_close_request(iced::window::Id::unique(), None);
        assert!(nodir.confirm_visible, "无快照目录时必须降级而不是丢数据");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_failure_degrades_to_confirm_bar_with_status_reason() {
        // 用一个普通文件冒充快照目录 → create_dir_all 必败
        let dir = snapshot_scratch_dir("fail");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, "").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText(" precious ".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        assert!(app.confirm_visible, "快照失败必须降级回确认条保住数据");
        assert!(
            app.status.contains("会话快照失败"),
            "失败原因要留在状态栏:{:?}",
            app.status
        );
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "失败的提交不得产生清单"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discard_and_close_clears_snapshot_area() {
        let dir = snapshot_scratch_dir("discard");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("will discard".into())));
        // 先正常快照一次（模拟上一轮退出留下的会话记录）
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(editpad_core::snapshot::read_manifest(&dir).is_some());

        // 新会话里用户选择「放弃更改」→ 快照区必须一并清空，
        // 否则下次启动会把已放弃的内容当会话恢复回来
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("new session".into())));
        let _ = app2.discard_all_and_close(Some(dir.clone()));
        assert!(!app2.confirm_visible && !app2.any_dirty());
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "放弃语义 = 连快照一起丢"
        );
        assert!(!dir.exists(), "清场应删除整个快照目录");

        // 开关关闭时放弃不清场（本来就没有快照承诺）
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        let _ = off.discard_all_and_close(Some(dir.clone()));
        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P30 启动会话恢复 ----------

    /// 构造一个干净命名页的清单元数据（测试辅助）。
    fn clean_named_tab(path: &str) -> editpad_core::snapshot::SessionTab {
        editpad_core::snapshot::SessionTab {
            path: Some(path.to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        }
    }

    #[test]
    fn session_restore_applies_per_tab_display_overrides() {
        // P134（C7）：换行/字号覆盖随清单恢复并回填编辑器
        let dir = snapshot_scratch_dir("p134-overrides");
        let mut named = clean_named_tab("C:/w/log.txt");
        named.wrap_override = Some(true);
        named.font_size_override = Some(24.0);
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab: named,
                doc: editpad_core::Document::new(),
            }],
            0,
            2,
        )
        .expect("写会话应成功");

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].wrap_override, Some(true));
        assert_eq!(app.tabs[0].font_size_override, Some(24.0));
        assert!(
            app.tabs[0].editor.borrow().wrap_enabled(),
            "覆盖开 → 编辑器折行（区别于全局默认关）"
        );
        assert!((app.tabs[0].editor.borrow().font_size() - 24.0).abs() < 0.01);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_restore_rebuilds_tabs_content_active_and_pending_loads() {
        let dir = snapshot_scratch_dir("p30-restore");

        // 准备上一轮正常退出留下的会话：置脏未命名页（内容+光标）+ 干净命名页
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(4),
            dirty: true,
            file: None,
            cursor_line: 1,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/notes.md".to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 10,
            cursor_col: 3,
            scroll_top: 50.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("第一行\r\n草稿内容"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            7,
        )
        .expect("写会话应成功");
        assert!(manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 0, "激活页按清单还原");
        assert!(app.recover_prompt.is_none(), "clean_exit=true 静默恢复，不弹提示条");

        // 置脏未命名页：内容逐字还原 + 光标就位 + 编号延续
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "第一行\r\n草稿内容");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(4));
        let ed0 = app.tabs[0].editor.borrow();
        assert_eq!((ed0.cursor.line, ed0.cursor.col), (1, 2), "光标回到退出时位置");
        drop(ed0);
        assert_eq!(app.untitled_next, 7, "未命名编号从清单延续单调性");

        // 干净命名页：占位页已建、加载任务已自动开跑（不抢焦点）
        assert!(app.restore_placeholder_ready(1));
        let job = app.active_load.as_ref().expect("命名页应立即开载").clone();
        assert_eq!(job.tab, 1);
        assert!(app.busy);
        assert_eq!(app.active_tab, 0, "恢复链绝不把用户拽到加载页");

        // 加载完成：内容回填 + 视图回填 + 收尾
        let body = editpad_core::Document::from_str(&"line\n".repeat(30));
        dispatch(
            &mut app,
            Message::Loaded(job.id, Ok((body, String::new(), "UTF-8".to_owned()))),
        );
        assert_eq!(app.tabs[1].path.as_deref(), Some(Path::new("C:/w/notes.md")));
        assert!(!app.tabs[1].dirty);
        let ed1 = app.tabs[1].editor.borrow();
        assert_eq!(
            (ed1.cursor.line, ed1.cursor.col),
            (10, 3),
            "加载完成的命名页同样回填光标"
        );
        assert!(
            ed1.scroll_top > 0.0 && ed1.scroll_top <= 50.0,
            "滚动值被应用且经行程钳制，实际 {}",
            ed1.scroll_top
        );
        drop(ed1);
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0, "恢复链排空");
        assert!(app.status.is_empty(), "全部成功的恢复不打扰状态栏");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// P43 回归：新建标签页唯一出口必须套用全局字号设置
    /// （曾有多处直接 `Tab::empty()`，编辑器默认 16px 脱离设置）。
    #[test]
    fn fresh_tab_carries_global_font_size() {
        let mut app = Editpad::default();
        app.settings.font_size = 22.0;
        let tab = app.fresh_tab();
        assert_eq!(tab.editor.borrow().font_size(), 22.0);

        // 归一规则与 boot 同源：非法（超界/负）值收敛到合法区间
        let mut app = Editpad::default();
        app.settings.font_size = 9999.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            crate::editor::normalize_font_size(9999.0)
        );
        let mut app = Editpad::default();
        app.settings.font_size = -5.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            crate::editor::normalize_font_size(-5.0)
        );
    }

    /// P43 回归：会话恢复（P30）创建的标签页必须套用全局字号——
    /// 曾恢复路径整体替换 tabs，boot 只给恢复前默认页应用过字号，
    /// 表现为「字号调到 24 生效，重启后字体回到原大小，设置面板仍 24」。
    #[test]
    fn restored_tabs_carry_global_font_size() {
        let dir = snapshot_scratch_dir("p43-restore-font-size");
        let tab = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("待恢复的草稿"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        app.settings.font_size = 24.0;
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().font_size(),
            24.0,
            "恢复页必须应用全局字号设置"
        );
        assert_eq!(app.settings.font_size, 24.0, "设置本身不受影响");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn dirty_named_tab_restores_snapshot_instead_of_disk_reload() {
        let dir = snapshot_scratch_dir("p30-dirty-named");
        let tab = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/doc.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 4,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("磁盘上没有的未保存内容"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "磁盘上没有的未保存内容",
            "v1 一律信快照（所见即所得），绝不从磁盘重载覆盖未存改动"
        );
        assert!(app.tabs[0].dirty, "恢复出的置脏页保持置脏");
        assert_eq!(app.tabs[0].encoding_label, "UTF-8", "快照恒为 UTF-8 落盘");
        assert!(app.active_load.is_none(), "快照同步还原，无需排队加载");
        assert!(app.status.is_empty());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restored_dirty_pages_stay_dirty_when_undo_revisits_snapshot_state() {
        // P38 × P30 交互回归：恢复出来的置脏页不得把快照内容当落盘基线，
        // 否则「编辑 → 撤销回快照态」会错误清脏 → 零询问退出后下次启动
        // 命名页从磁盘旧内容重载、未命名页直接空白——未存改动静默丢失。
        let dir = snapshot_scratch_dir("p38-restore-baseline");
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/report.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("草稿"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::from_str("磁盘上没有的修改"),
                },
            ],
            0,
            4,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[0].dirty && app.tabs[1].dirty);

        // 未命名页：编辑一步再撤销回快照态——必须保持置脏（无基线保守）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        assert!(app.tabs[0].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(
            app.tabs[0].dirty,
            "撤销回快照态不清脏：内容从未落盘（P30 恢复页基线已清空）"
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "草稿");

        // 置脏命名页同理：磁盘还是旧内容，快照态不算「已保存」
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
        assert!(app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(app.tabs[1].dirty, "命名恢复页撤销回快照态同样保持置脏");
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "磁盘上没有的修改"
        );

        // 真正落盘一次后基线重建，撤销回清能力恢复正常语义
        let v = app.tabs[1].version;
        let tid = app.tabs[1].id;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                tid,
                v,
                PathBuf::from("C:/w/report.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.tabs[1].dirty, "落盘之后撤销回基线应正常回清");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// 在指定目录种一个「异常退出中间态」会话（clean_exit=false，
    /// 即 P31 心跳写的清单形态）：手工落一份可解析 TOML + 真实页文件。
    fn plant_orphan_session(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("s7-t0.snap"), "崩溃前的未保存草稿").unwrap();
        fs::write(
            dir.join(editpad_core::snapshot::MANIFEST_NAME),
            concat!(
                "generation = 7\n",
                "active = 0\n",
                "next_untitled = 3\n",
                "clean_exit = false\n",
                "\n",
                "[[tabs]]\n",
                "untitled_num = 2\n",
                "dirty = true\n",
                "file = \"s7-t0.snap\"\n",
                "cursor_line = 0\n",
                "cursor_col = 3\n",
            ),
        )
        .unwrap();
    }

    #[test]
    fn crash_orphan_manifest_prompts_then_restores_or_discards() {
        // 「恢复」路径：弹一次性提示条 → 接受后全量重建（含置脏内容）
        let dir_a = snapshot_scratch_dir("p30-orphan-a");
        plant_orphan_session(&dir_a);
        let manifest = editpad_core::snapshot::read_manifest(&dir_a).unwrap();
        assert!(!manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_dir(&dir_a);
        assert!(app.recover_prompt.is_some(), "异常退出必须弹一次性恢复条");
        assert_eq!(app.tabs.len(), 1, "未裁决前不得动当前标签");

        let _ = app.accept_session_recover(Some(dir_a.clone()));
        assert!(app.recover_prompt.is_none());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "崩溃前的未保存草稿");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(2), "编号延续");
        assert_eq!(app.untitled_next, 3, "计数器取清单的 next_untitled");
        editpad_core::snapshot::clear_session(&dir_a);

        // 「丢弃」路径：连快照一起丢（P29 放弃语义同族），空白起步
        let dir_b = snapshot_scratch_dir("p30-orphan-b");
        plant_orphan_session(&dir_b);
        let mut app2 = Editpad::default();
        let _ = app2.restore_from_dir(&dir_b);
        assert!(app2.recover_prompt.is_some());
        let _ = app2.discard_session_recover(Some(dir_b.clone()));
        assert!(app2.recover_prompt.is_none());
        assert!(
            editpad_core::snapshot::read_manifest(&dir_b).is_none(),
            "丢弃语义必须清场，防下次启动复活已弃内容"
        );
        fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn remember_session_disabled_means_blank_start_and_ask_style_close() {
        // 总开关判定纯函数：两个开关都开才允许
        assert!(session_restore_allowed(true, true));
        assert!(!session_restore_allowed(false, true), "快照总开关是前提");
        assert!(!session_restore_allowed(true, false), "关闭会话记忆 = 不恢复也不写清单");

        // 退出侧：remember_session 关闭时关窗回退旧确认条，不写任何会话数据
        let dir = snapshot_scratch_dir("p30-gate");
        let mut app = Editpad::default();
        app.settings.remember_session = false;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("keep me".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(app.confirm_visible, "回退旧确认条而不是静默丢改动");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "记住会话关闭后退出不写清单（§3 P30 第 4 条）"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_skips_unloadable_page_and_keeps_chain_going() {
        let dir = snapshot_scratch_dir("p30-fail");
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/gone.txt"),
                    doc: editpad_core::Document::new(),
                },
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/stays.txt"),
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.restore_pending, 2);
        let seq_a = app.active_load.as_ref().unwrap().id;

        // 第一页加载失败（文件已被删等）：占位移除、失败计数、
        // 续排同步完成——B 已顶上载通道，队列排空
        dispatch(&mut app, Message::Loaded(seq_a, Err("文件不存在".to_owned())));
        assert_eq!(app.tabs.len(), 1, "失败占位页应被移除（单页失败不阻断）");
        assert_eq!(app.restore_failed, 1);
        assert!(
            app.restore_queue.is_empty(),
            "续排是同步的：下一页应已登记为在途任务"
        );

        // 第二页自动续排并成功落地
        let job_b = app.active_load.as_ref().expect("失败后必须续排下一页").clone();
        assert_eq!(job_b.tab, 0, "队列下标必须随页面移除整体左移");
        dispatch(
            &mut app,
            Message::Loaded(
                job_b.id,
                Ok((
                    editpad_core::Document::from_str("b body"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "b body");
        assert_eq!(app.tabs[0].path.as_deref(), Some(Path::new("C:/w/stays.txt")));
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0);
        assert!(
            app.status.contains("会话恢复完成") && app.status.contains("1 页未能恢复"),
            "失败页要有状态栏汇总:{:?}",
            app.status
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restore_plan_prioritizes_active_page_and_truncates_over_cap() {
        let dir = snapshot_scratch_dir("p30-plan");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f100.snap"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("f200.snap"), vec![0u8; 200]).unwrap();

        // t0 = 干净命名页（估算读其磁盘文件 100B）；t1 = 置脏页（快照 200B）；
        // t2 = 干净未命名页（空文档，0B）
        let manifest = |active: usize| editpad_core::snapshot::SessionManifest {
            generation: 1,
            tabs: vec![
                clean_named_tab(&dir.join("f100.snap").display().to_string()),
                editpad_core::snapshot::SessionTab {
                    file: Some("f200.snap".to_owned()),
                    dirty: true,
                    ..clean_named_tab("")
                },
                editpad_core::snapshot::SessionTab {
                    untitled_num: Some(5),
                    ..clean_named_tab("")
                },
            ],
            active,
            next_untitled: 6,
            clean_exit: true,
        };

        // 宽松上限：全保
        let (kept, dropped) = plan_restore_order(&manifest(0), &dir, u64::MAX);
        assert_eq!((kept, dropped), (vec![0, 1, 2], 0));

        // 250B 上限、激活页是 200B 快照页：先保激活页 → 100B 页出局
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 250);
        assert_eq!((kept, dropped), (vec![1, 2], 1));

        // 150B 上限、激活页是 200B 快照页：激活页自身超限同样出局
        // （「先保激活页」是装填优先级，不是护栏豁免权），
        // 随后 100B 页与空页照常装填
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 150);
        assert_eq!((kept, dropped), (vec![0, 2], 1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loaded_language_routes_through_alias_and_sniff() {
        // P22：扩展名别名层 —— .log 命中「Editpad Log」内嵌语法
        // （即使内容长得像 JSON，扩展名可信优先）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/logs/app.log")));
        let doc = editpad_core::Document::from_str("{\"level\":1}");
        let sample = "{\"level\":1}";
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, sample.to_owned(), "UTF-8".to_owned()))),
        );
        assert_eq!(
            app.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Editpad Log")
        );

        // 无扩展名 + shebang 样本 → 内容嗅探接管
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/bin/build")));
        let doc2 = editpad_core::Document::from_str("#!/bin/sh\necho hi\n");
        let seq2 = app2.job_seq;
        dispatch(
            &mut app2,
            Message::Loaded(
                seq2,
                Ok((doc2, "#!/bin/sh\necho hi\n".to_owned(), "UTF-8".to_owned())),
            ),
        );
        assert_eq!(
            app2.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Bourne Again Shell (bash)")
        );
    }

    // ---------- P102：窗口几何记忆 + 实例隔离 ----------

    /// P102 回归：窗口移动/拉伸事件 → 设置字段更新并（节流）落盘——
    /// 写盘走注入路径，绝不碰真实 %APPDATA%；icon 测试同款零数据泄漏。
    #[test]
    fn p102_window_geometry_events_update_and_persist_settings() {
        let dir = scratch_dir("p102-geom");
        let mut app = Editpad::default();
        app.settings_path_override = Some(dir.join("config.toml"));

        // 移动 + 拉伸：首事件立即落盘；紧随的拉伸在节流窗内不落盘
        dispatch(&mut app, Message::WindowMoved(iced::Point::new(320.0, 180.0)));
        dispatch(&mut app, Message::ViewportResized(1280.0, 720.0));
        let loaded = editpad_core::Settings::load_from(&dir.join("config.toml"));
        assert_eq!(
            (loaded.window_x, loaded.window_y),
            (Some(320), Some(180)),
            "首个几何事件必须落盘"
        );
        assert_eq!(
            (loaded.window_width, loaded.window_height),
            (None, None),
            "节流窗内的拉伸不得落盘"
        );

        // 节流：紧随其后的第二次移动（同节流窗内）不得再写盘
        let before = std::fs::read(dir.join("config.toml")).unwrap();
        dispatch(&mut app, Message::WindowMoved(iced::Point::new(333.0, 188.0)));
        let after = std::fs::read(dir.join("config.toml")).unwrap();
        assert_eq!(before, after, "节流窗内不得反复写盘");
        // 内存态已更新（关闭路径兜底落盘时带走最后的移动）
        assert_eq!((app.settings.window_x, app.settings.window_y), (Some(333), Some(188)));

        // 关闭路径兜底落盘：handle_close_request 补写最后状态（含节流窗内的尺寸）
        let _ = app.handle_close_request(iced::window::Id::unique(), None);
        let final_loaded = editpad_core::Settings::load_from(&dir.join("config.toml"));
        assert_eq!(
            (final_loaded.window_x, final_loaded.window_y),
            (Some(333), Some(188)),
            "关闭时必须兜底写入节流窗内的最后几何"
        );
        assert_eq!(
            (final_loaded.window_width, final_loaded.window_height),
            (Some(1280.0), Some(720.0)),
            "关闭时必须兜底写入被节流挡下的尺寸"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// P102 契约：restore_window_geometry 把配置换算成建窗参数——
    /// 合法值原样还原、缺省回退默认居中、坏值（越界坐标/极小尺寸）
    /// 回退且不 panic。
    #[test]
    fn p102_restore_window_geometry_clamps_bad_values() {
        use iced::window::Position;

        let mut s = editpad_core::Settings::default();
        let (size, pos) = restore_window_geometry(&s);
        assert_eq!((size.width, size.height), (1024.0, 768.0), "未记录回退默认尺寸");
        assert!(matches!(pos, Position::Centered), "未记录回退居中");

        s.window_x = Some(120);
        s.window_y = Some(-8);
        s.window_width = Some(1280.5);
        s.window_height = Some(720.25);
        let (size, pos) = restore_window_geometry(&s);
        assert_eq!((size.width, size.height), (1280.5, 720.25));
        assert!(
            matches!(pos, Position::Specific(p) if p.x == 120.0 && p.y == -8.0),
            "合法记录必须原样建窗"
        );

        // 坏值：坐标越界、尺寸过小/非有限 → 回退，不得建出不可用窗口
        s.window_x = Some(1 << 30);
        s.window_y = Some(1 << 30);
        s.window_width = Some(10.0);
        s.window_height = Some(10.0);
        let (_, pos) = restore_window_geometry(&s);
        assert!(matches!(pos, Position::Centered));
        let (size, _) = restore_window_geometry(&s);
        assert_eq!((size.width, size.height), (1024.0, 768.0));

        s.window_x = Some(5);
        s.window_y = Some(5);
        s.window_width = Some(f32::NAN);
        s.window_height = Some(f32::INFINITY);
        let (size, _) = restore_window_geometry(&s);
        assert_eq!((size.width, size.height), (1024.0, 768.0), "非有限尺寸必须回退");
    }

    #[test]
    fn p76_window_title_icon_decodes_48px_entry() {
        // P71 自定义 ICO 的 48px 条目必须可解析为窗口图标：48×48、
        // RGBA 长度正确、存在非透明像素（纯透明 = 白图，标题栏无意义）、
        // 且与 boot 下发的为同一实例（OnceLock 缓存语义）
        let Some(icon) = window_title_icon() else {
            panic!("app.ico 48px 条目解析失败（DIB 解码或条目缺失）");
        };
        let (rgba, size) = icon.into_raw();
        assert_eq!((size.width, size.height), (48, 48), "窗口图标应为 48×48");
        assert_eq!(rgba.len(), 48 * 48 * 4);
        assert!(
            rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
            "窗口图标不得为全透明位图"
        );
        // 预乘校验：非透明像素的 RGB ≤ alpha（premultiply 不变量）
        for px in rgba.as_chunks::<4>().0 {
            let a = px[3] as u32;
            if a > 0 {
                assert!(px[0] as u32 <= a && px[1] as u32 <= a && px[2] as u32 <= a);
            }
        }
    }

    #[test]
    fn p64_rename_input_id_is_stable_for_focus_operation() {
        // 聚焦操作按 id 查找控件：每次调用必须产出相等的 id（逐帧稳定），
        // 且与视图侧 text_input 挂载的是同一个值（同源常量函数）
        let a = rename_input_id();
        let b = rename_input_id();
        assert_eq!(a, b, "id 必须逐帧稳定，聚焦操作才能命中输入框");
    }

    #[test]
    fn p63_late_skip_after_own_manual_save_is_silently_dropped() {
        // 误报抑制：编辑→Ctrl+S 手动落盘（磁盘与记录戳同步刷新）→ 在途
        // 防抖线程拿着旧期望戳迟到拒写——这是自己的保存不是外部改动，
        // 必须静默丢弃，不得弹提示条
        let (mut app, path) = loaded_real_file_app("p63-late-skip");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let v = app.tab().version;
        let tid = app.tabs[0].id;

        // 手动保存走完整链路：进入 busy → Saved 回报清脏并按新磁盘重记戳
        dispatch(&mut app, Message::SaveRequested);
        assert!(app.busy);
        std::fs::write(&path, "our own newer content").unwrap();
        dispatch(
            &mut app,
            Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert!(!app.tab().dirty && !app.busy);
        let fresh_stamp = app.tabs[0].file_stamp;

        // 迟到的拒写回报（线程的期望戳仍是载入时刻的旧戳）
        dispatch(
            &mut app,
            Message::TabAutosaved(
                tid,
                v,
                path.clone(),
                AutosaveOutcome::SkippedExternalChange,
            ),
        );
        assert!(
            app.external_change.is_none(),
            "自己的手动保存不得触发外部修改提示条"
        );
        assert!(!app.status.contains("外部修改"), "实际 {:?}", app.status);
        assert_eq!(app.tabs[0].file_stamp, fresh_stamp, "记录戳保持不动");

        // 对照：真外部改动后磁盘 ≠ 记录戳，同样的迟到回报必须入队
        std::fs::write(&path, "genuinely external").unwrap();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        let v2 = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(tid, v2, path, AutosaveOutcome::SkippedExternalChange),
        );
        assert_eq!(app.external_change, Some(vec![0]), "真外部改动必须入队裁决");
    }

    // ---------- P21 内存护栏 ----------

    #[test]
    fn mem_guard_rejects_when_estimate_exceeds_cap() {
        let cap = 1_000u64;
        // 现有 100 字符 ×3 字节估算 = 300；再开 500 字节文件 → 800 ≤ 1000 允许
        assert!(mem_guard_allows(100, 500, cap));
        // 再大一点就超
        assert!(!mem_guard_allows(100, 900, cap));
        // 极端值不 panic（饱和运算）
        assert!(mem_guard_allows(usize::MAX, u64::MAX, u64::MAX));
        assert!(!mem_guard_allows(usize::MAX, u64::MAX, 0));
    }

    #[test]
    fn start_loading_enforces_mem_guard() {
        let mut app = Editpad::default();
        // 预置一个超大字符量的当前页（直接改 doc 以绕过真实大文件）
        let huge = editpad_core::Document::from_str(&"x".repeat(200_000_000));
        app.cur_handle.borrow_mut().reset_document(huge);
        // 当前页非空 → 打开会走新页，但护栏按全页合计判定
        let big_path = PathBuf::from("C:/definitely/too/big.bin");
        let task = app.start_loading(big_path.clone(), app.tabs.len());
        let _ = task;
        assert!(
            app.status.contains("内存保护"),
            "超限打开应被拒绝并提示，实际 {:?}",
            app.status
        );
        assert!(app.active_load.is_none(), "被拒的打开不得登记任务");
        assert_eq!(app.tabs.len(), 1, "拒绝时不得占位新页");
    }

    // ---------- P31 周期快照心跳 ----------

    /// 构造已加载命名文档、注入快照目录的心跳测试应用。
    fn heartbeat_app(dir: &Path) -> Editpad {
        let mut app = loaded_txt_app();
        app.snapshot_dir_override = Some(dir.to_path_buf());
        app
    }

    #[test]
    fn heartbeat_writes_changed_dirty_page_and_marks_intermediate_manifest() {
        let dir = scratch_dir("hb-e2e");
        let mut app = heartbeat_app(&dir);

        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("有变化的置脏页应触发一次提交");
        assert!(outcome.result.is_ok(), "提交应成功，实际 {:?}", outcome.result);

        // 磁盘形态：clean_exit=false 的中间态清单 + 内容文件（未命名页
        // 与命名页同规则收录——P18 的盲区由此获得兜底）
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!manifest.clean_exit, "心跳写的必须是中间态清单");
        assert!(manifest.tabs[0].dirty);
        let doc = editpad_core::snapshot::read_page(&dir, &manifest.tabs[0]).unwrap();
        assert_eq!(doc.to_text(), "xbase", "快照必须反映当前内容（光标在行首插入）");

        // 账目回填：版本一致 → 记录 (版本, 文件名)
        assert_eq!(
            app.tabs[0].heartbeat_snap,
            Some((app.tabs[0].version, manifest.tabs[0].file.clone().unwrap()))
        );
        assert!(!app.heartbeat_inflight, "回报落地后必须解除在途标记");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_commit_entirely_when_nothing_changed() {
        let dir = scratch_dir("hb-dedupe");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));

        let first = app.run_heartbeat_cycle(&dir).expect("首次应有提交");
        let manifest_before = editpad_core::snapshot::read_manifest(&dir).unwrap();

        // 无新编辑：代次去重命中 → 零 IO，磁盘原封不动
        let second = app.run_heartbeat_cycle(&dir);
        assert!(second.is_none(), "无变化且清单不过期时不得产生任何写盘");
        assert_eq!(
            editpad_core::snapshot::read_manifest(&dir),
            Some(manifest_before),
            "清单必须保持原样（含代次号）"
        );
        assert_eq!(
            first.plan,
            vec![(0usize, app.tabs[0].version)],
            "计划只含变化页"
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_refreshes_manifest_after_autosave_cleans_tab() {
        let dir = scratch_dir("hb-stale");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let _ = app.run_heartbeat_cycle(&dir).unwrap();
        let dirty_manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(dirty_manifest.tabs[0].dirty);

        // 模拟 auto-save 成功清脏：内存比已提交清单「更干净」→ 过期标记
        let version = app.tabs[0].version;
        let tid = app.tabs[0].id;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                tid,
                version,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tab().dirty);
        assert!(app.session_manifest_stale, "清脏后下一拍必须重写清单");

        // 下一拍即使无页被选中也要提交：清单转为「干净页纯元数据」，
        // 否则崩溃恢复会把已落盘内容按旧快照复活成置脏页
        app.run_heartbeat_cycle(&dir).expect("过期清单应触发重写");
        let refreshed = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!refreshed.tabs[0].dirty && refreshed.tabs[0].file.is_none());
        assert!(!app.session_manifest_stale, "成功提交后过期标记归零");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_oversized_page_content_but_still_records_it_dirty() {
        let dir = scratch_dir("hb-oversize");
        let mut app = heartbeat_app(&dir);
        // 17MB 文档 > HEARTBEAT_MAX_PAGE_BYTES(16MB)：不进心跳重写，
        // 只在退出时随全量提交落盘（§3 P29 第 5 条的 IO 保护）
        let big = editpad_core::Document::from_str(&"a".repeat(17_000_000));
        {
            let tab = &mut app.tabs[0];
            tab.editor.borrow_mut().reset_document(big);
            tab.dirty = true;
            tab.version += 1;
        }

        // 首拍：Loaded 留下的清单过期标记驱动一次提交——超限页被记账
        // （保持置脏）但绝不产生内容文件
        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("过期清单 + 置脏页应触发一次记账提交");
        assert!(outcome.result.is_ok());
        assert!(
            outcome.plan.is_empty(),
            "超限页不得进入重写计划，实际 {:?}",
            outcome.plan
        );
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(manifest.tabs[0].dirty, "超限页保持置脏记账");
        assert_eq!(manifest.tabs[0].file, None, "超限页不得落内容文件");

        let snap_files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".snap"))
            .collect();
        assert!(snap_files.is_empty(), "磁盘上不得出现任何内容快照");

        // 次拍：过期标记已清、计划仍为空 → 零 IO（节流的常态形态）
        assert!(app.run_heartbeat_cycle(&dir).is_none());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_failure_clears_accounts_traces_status_and_keeps_dirty() {
        let dir = scratch_dir("hb-fail");
        // 注入目录指向一个普通文件 → create_dir_all 必败 → 提交必败
        let blocker = dir.join("occupied-as-file");
        std::fs::write(&blocker, b"not a dir").unwrap();

        let mut app = heartbeat_app(&blocker);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&blocker)
            .expect("有置脏页就应有提交尝试");
        assert!(outcome.result.is_err(), "目录不可用时应失败");

        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(app.tabs[0].heartbeat_snap.is_none(), "失败账目作废");
        assert!(!app.heartbeat_inflight, "失败也解除在途（允许下拍重试）");
        assert!(
            app.status.contains("快照心跳失败"),
            "失败必须留痕不无声吞掉，实际 {:?}",
            app.status
        );

        // 账目作废后下一拍会全量重试（此处仅验证状态复位，不再造真实目录）
        let recovery_dir = scratch_dir("hb-fail-recovery");
        let mut recovered = app.clone();
        recovered.snapshot_dir_override = Some(recovery_dir.clone());
        let retry = recovered.run_heartbeat_cycle(&recovery_dir);
        assert!(retry.is_some_and(|o| o.result.is_ok()), "复位后重试应成功");

        editpad_core::snapshot::clear_session(&dir);
        editpad_core::snapshot::clear_session(&recovery_dir);
    }

    #[test]
    fn heartbeat_result_with_stale_version_does_not_record_account() {
        // 回报落地时的版本守卫：派发后页面又编辑过 → 迟到的快照不得入账
        // （否则账目声称「版本 v 已快照」，实际盘上是更旧的内容）
        let dir = scratch_dir("hb-stale-version");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let planned_version = app.tabs[0].version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        let outcome = HeartbeatOutcome {
            plan: vec![(0, planned_version)],
            rev: app.manifest_rev,
            result: Ok(editpad_core::snapshot::SessionManifest {
                generation: 1,
                tabs: vec![editpad_core::snapshot::SessionTab {
                    path: None,
                    untitled_num: None,
                    dirty: true,
                    file: Some("s1-t0.snap".to_owned()),
                    cursor_line: 0,
                    cursor_col: 0,
                    scroll_top: 0.0,
                    scroll_left: 0.0,
                    wrap_override: None,
                    font_size_override: None,
                }],
                active: 0,
                next_untitled: 2,
                clean_exit: false,
            }),
        };
        app.heartbeat_apply(outcome);

        assert!(
            app.tabs[0].heartbeat_snap.is_none(),
            "版本不符的迟到成果不得记账"
        );
        assert!(!app.session_manifest_stale, "成功路径仍应清过期标记");
        assert!(!app.heartbeat_inflight);

        editpad_core::snapshot::clear_session(&dir);
    }

    // ---------- P50 外部修改检测 ----------

    #[test]
    fn file_changed_externally_contract() {
        let t1 = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(100);
        let t2 = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(200);

        // 记录缺失（未落地页）= 不判定
        assert!(!file_changed_externally(None, Some((t1, 10))));
        // 完全一致 = 未修改
        assert!(!file_changed_externally(Some((t1, 10)), Some((t1, 10))));
        // mtime 变 / size 变（FAT 系 2s 粒度盲区的兜底）/ 双变 = 修改
        assert!(file_changed_externally(Some((t1, 10)), Some((t2, 10))));
        assert!(file_changed_externally(Some((t1, 10)), Some((t1, 11))));
        assert!(file_changed_externally(Some((t1, 10)), Some((t2, 11))));
        // 当前缺失 = 文件被外部删除 = 视为修改（提示用户，重载得明确报错）
        assert!(file_changed_externally(Some((t1, 10)), None));
    }

    #[test]
    fn window_focus_reloads_clean_active_tab_silently() {
        let dir = scratch_dir("p50-focus");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "v1").unwrap();

        let mut app = Editpad::default();
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
        assert!(app.tabs[0].file_stamp.is_some(), "载入成功即记比对戳");

        // 无外部变化：聚焦不动作
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.active_load.is_none());
        assert!(app.external_change.is_none());

        // 外部修改（内容+大小都变）：聚焦 → 干净活动页静默重载
        std::fs::write(&path, "v2-changed").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.active_load.is_some(), "干净活动页应静默重载");
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("v2-changed"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "v2-changed");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn window_focus_prompts_for_dirty_tab_and_respects_decision() {
        let dir = scratch_dir("p50-dirty");
        let path = dir.join("dirty.txt");
        std::fs::write(&path, "disk").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("disk"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 用户编辑置脏；期间文件被外部改
        dispatch(&mut app, Message::Edit(EditOp::InsertText("local edit".into())));
        std::fs::write(&path, "disk changed").unwrap();

        // 聚焦：置脏页绝不静默重载，弹提示条
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        assert_external_change_bar(&app);

        // 忽略 → 以磁盘现状重记戳，再次聚焦不再提示
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none(), "忽略后同状态不再提示");

        // 文件再次变化 → 又提示；这次选重载 → 放弃本地编辑取磁盘内容
        std::fs::write(&path, "disk v3").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        dispatch(&mut app, Message::ConfirmExternalReload(0));
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("disk v3"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "disk v3");
        assert!(!app.tabs[0].dirty, "重载完成后回净");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn external_change_prompt_queues_multiple_tabs() {
        // P52 聚合：两个置脏后台页同被外部修改 → 队列按序聚合；
        // 忽略首页自动切下一页；全部忽略一次清队
        let dir = scratch_dir("p52-queue");
        let (p0, p1) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&p0, "a1").unwrap();
        std::fs::write(&p1, "b1").unwrap();

        let mut app = Editpad::default();
        // 页0：加载后置脏；页1：新开加载（保持干净，但属后台页）
        dispatch(&mut app, Message::FileDropped(p0.clone()));
        let s1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s1,
                Ok((
                    editpad_core::Document::from_str("a1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("local".into())));
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::FileDropped(p1.clone()));
        let s2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s2,
                Ok((
                    editpad_core::Document::from_str("b1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.active_tab, 1, "第二个打开落新页并聚焦");

        // 两页同被外部修改 → 聚焦后队列 [0, 1]（活动页1干净本可静默重载，
        // 但页0置脏在先——巡检按序扫描，页0先命中进队列；页1是干净活动页
        // 走静默重载分支并提前返回，队列只收页0。此处钉住该优先级语义。）
        // ⚠️ 第 71 轮隔离：写盘后跨过 mtime 粒度/缓存窗口（~15-25ms），
        // 否则全量并发下外部写入可能与加载戳同窗，巡检漏检致偶发失败。
        // 第 74 轮（P95）加宽 25→50ms：软换行像素批等新增测试加重并发
        // 调度抖动后 25ms 仍在全量下偶发（隔离单跑恒绿），50ms 留足余量。
        // 第 75 轮（P96）再宽 50→150ms：P96 像素断行测试引入更多行布局
        // 注入与并发压力，50ms 连续三轮全量偶发（隔离单跑恒绿）。
        std::fs::write(&p0, "a2").unwrap();
        std::fs::write(&p1, "b2").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        dispatch(&mut app, Message::WindowFocused);
        assert!(
            app.active_load.is_some(),
            "干净活动页(1)优先静默重载并提前返回"
        );
        let s3 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s3,
                Ok((
                    editpad_core::Document::from_str("b2"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 重载完成后再次聚焦：页1已重记戳不再命中，队列只剩页0
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        assert_external_change_bar(&app);

        // 忽略页0 → 队列清空；再改两页且都置脏 → 聚合 [0,1]
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());

        dispatch(&mut app, Message::Edit(EditOp::InsertText("more".into())));
        std::fs::write(&p0, "a3").unwrap();
        std::fs::write(&p1, "b3").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(
            app.external_change,
            Some(vec![0, 1]),
            "两页同变必须聚合成队列"
        );

        // 忽略首页 → 队列切到下一页（条不消失）
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert_eq!(app.external_change, Some(vec![1]));
        assert_external_change_bar(&app);

        // 全部忽略 → 清队且两页都重记戳（再次聚焦不再提示）
        dispatch(&mut app, Message::IgnoreAllExternalChanges);
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none(), "全部忽略后同状态不再提示");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn focus_check_skips_busy_and_unnamed_tabs() {
        let mut app = Editpad::default();
        // 未命名页（无路径无戳）：聚焦不动作不 panic
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none());

        // busy 中聚焦：跳过巡检（在途任务的完成回调会刷新戳）
        app.busy = true;
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none() && app.active_load.is_none());
        app.busy = false;
    }


    #[test]
    fn heartbeat_ok_does_not_clear_stale_when_tabs_closed_in_flight() {
        // P146 回归：心跳在途期间关页 → 成功回报曾无条件清 stale，磁盘
        // 清单里留着已关页且永不重写——崩溃恢复把已关页连同旧内容复活。
        let dir = scratch_dir("hb-inflight-close");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let payload = app.prepare_heartbeat_commit(&dir).expect("首轮应有提交");
        app.heartbeat_inflight = true;
        // 在途期间关掉置脏页（放弃更改路径）——结构变化推进过期代次
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
        assert!(app.session_manifest_stale, "关页应置位过期标记");
        let result = editpad_core::snapshot::write_heartbeat_session(
            &payload.dir,
            &payload.pages,
            payload.active,
            payload.next_untitled,
        )
        .map_err(|e| e.to_string());
        app.heartbeat_apply(HeartbeatOutcome {
            plan: payload.plan,
            rev: payload.rev,
            result,
        });
        assert!(
            app.session_manifest_stale,
            "在途期间的结构变化必须保持过期标记（派发时的清单已过期）"
        );

        // 对照：无在途变化的正常提交照常清除标记
        app.run_heartbeat_cycle(&dir).unwrap();
        assert!(!app.session_manifest_stale);
        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restore_with_all_pages_over_mem_cap_keeps_app_alive() {
        // P147 回归：全部页超内存护栏被规划放弃 → kept 为空。曾直接
        // tabs[active_tab] 越界 panic（启动即崩，需手删快照目录才能恢复）；
        // 必须补空页保持「tabs 恒非空」不变式。快照文件用 set_len 稀疏
        // 超标（瞬间构造，不写真实字节——规划期只看 metadata 大小）。
        let dir = snapshot_scratch_dir("p147-all-over-cap");
        let tab = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/huge.log".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            wrap_override: None,
            font_size_override: None,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("content"),
            }],
            0,
            2,
        )
        .expect("写会话应成功");

        // 把该页快照「撑」到超护栏（稀疏文件，只改元数据长度）
        let snap = dir.join(manifest.tabs[0].file.as_deref().expect("置脏页有快照"));
        std::fs::File::options()
            .write(true)
            .open(&snap)
            .unwrap()
            .set_len(crate::session::MULTI_TAB_MEM_CAP_BYTES + 1)
            .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        // 收尾汇总会重置计数器，但超护栏事实必须如实进状态栏
        assert!(
            app.status.contains("超出内存护栏"),
            "截断汇总应提示，实际 {:?}",
            app.status
        );
        assert_eq!(app.tabs.len(), 1, "全部放弃时补空页，tabs 恒非空");
        assert!(app.tabs[0].editor.borrow().doc.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
