use super::*;

// ---------- P8 AltGr ----------

#[test]
fn altgr_character_falls_through_to_text_insert() {
    use iced::keyboard::{self, key::Named};
    let altgr = keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT;

    // 德语布局 @ = AltGr+Q：winit 上报 Ctrl+Alt + 字符 q/@，
    // 不得落入 Ctrl 快捷键分支被吞，必须按普通文本插入
    let at = keyboard::Key::Character("@".into());
    assert!(matches!(
        handle_key_defaults(at, altgr),
        Some(Message::Edit(EditOp::InsertText(t))) if t == "@"
    ));

    let q = keyboard::Key::Character("q".into());
    assert!(matches!(
        handle_key_defaults(q, altgr),
        Some(Message::Edit(EditOp::InsertText(t))) if t == "q"
    ));

    // 纯 Ctrl 的快捷键行为不受影响
    let ctrl = keyboard::Modifiers::CTRL;
    let o = keyboard::Key::Character("O".into());
    assert!(matches!(
        handle_key_defaults(o, ctrl),
        Some(Message::OpenRequested)
    ));

    // Ctrl+未绑定字母仍返回 None（不插入；注意 x 已是 P4 剪切键、
    // q 已是第 64 轮行注释键、v 是粘贴——探针用至今无主的 e）
    let e = keyboard::Key::Character("e".into());
    assert!(handle_key_defaults(e, ctrl).is_none());

    // Shift+字符（无 Ctrl）照常插入
    let bang = keyboard::Key::Character("!".into());
    assert!(matches!(
        handle_key_defaults(bang, keyboard::Modifiers::SHIFT),
        Some(Message::Edit(EditOp::InsertText(t))) if t == "!"
    ));
    let _ = Named::Tab; // 保持 import 使用
}

// ---------- P4 剪贴板 ----------

#[test]
fn ctrl_c_x_v_map_to_clipboard_messages() {
    use iced::keyboard::{self, key::Named};
    let ctrl = keyboard::Modifiers::CTRL;
    let _ = Named::Tab;

    for (letter, expected) in [
        ("c", "copy"),
        ("x", "cut"),
        ("v", "paste"),
        ("C", "copy"), // 大写（Shift 同按）也走同一快捷键
    ] {
        let key = keyboard::Key::Character(letter.into());
        let message = handle_key_defaults(key, ctrl).expect("应产生消息");
        let ok = matches!(
            (&message, expected),
            (Message::CopyRequested, "copy")
                | (Message::CutRequested, "cut")
                | (Message::PasteRequested, "paste")
        );
        assert!(ok, "{letter} 应映射到剪贴板消息，实际 {message:?}");
    }
}

#[test]
fn pasted_text_enters_document_as_dominant_eol() {
    // P9：粘贴不再在 app 层预归一；insert_str 按文档主导行尾统一改写。
    // CRLF 文档进来什么行尾都落成 \r\n，LF 文档落成 \n（与旧版行为一致）。
    let mut crlf = crate::editor::EditorCore::default();
    crlf.reset_document(editpad_core::Document::from_str("a\r\nb"));
    crlf.cursor = crate::editor::CursorPos { line: 1, col: 1 }; // 文末
    crlf.insert_str("x\r\ny\rz");
    assert_eq!(crlf.doc.to_text(), "a\r\nbx\r\ny\r\nz");

    let mut lf = crate::editor::EditorCore::default();
    lf.insert_str("x\r\ny\rz");
    assert_eq!(lf.doc.to_text(), "x\ny\nz");
}

// ---------- P24/P25 手测反馈修复 ----------

#[test]
fn untitled_tabs_get_unique_sequential_names() {
    let mut app = Editpad::default();
    // 初始页即「未命名1」
    assert_eq!(app.tab().base_name(), "未命名1");
    assert_eq!(app.title(), "未命名1 - Editpad");

    // 连开两页：未命名2、未命名3，全局唯一
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::NewTab);
    let names: Vec<String> = app.tabs.iter().map(|t| t.base_name()).collect();
    assert_eq!(names, vec!["未命名1", "未命名2", "未命名3"]);

    // 置脏前缀进标签名但不进 title 的基础名判断之外重复
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert!(app.tab().display_name().starts_with("● 未命名"));
}

#[test]
fn closed_untitled_numbers_are_never_reused_and_saving_clears_them() {
    let mut app = Editpad::default();

    // 关闭「未命名2」（干净直接关）后新建：号码不复用，拿「未命名3」
    dispatch(&mut app, Message::NewTab);
    assert_eq!(app.tabs[1].base_name(), "未命名2");
    app.set_active_tab(1);
    dispatch(&mut app, Message::CloseTabRequest);
    assert_eq!(app.tabs.len(), 1);

    dispatch(&mut app, Message::NewTab);
    assert_eq!(app.tabs[1].base_name(), "未命名3", "单调分配杜绝重名");

    // 另存为转正后序号清除，标签显示真实文件名
    dispatch(
        &mut app,
        Message::SaveTargetChosen(Some(PathBuf::from("C:/x/real.txt"))),
    );
    assert_eq!(
        app.tabs[1].path.as_deref(),
        Some(Path::new("C:/x/real.txt"))
    );
    assert_eq!(app.tabs[1].untitled_num, None);
    assert_eq!(app.tabs[1].base_name(), "real.txt");

    // 加载真实文件同样清除序号（Loaded 路径）
    let mut app2 = Editpad::default();
    dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/r/a.md")));
    let seq = app2.job_seq;
    dispatch(
        &mut app2,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("x"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert_eq!(app2.tabs[0].untitled_num, None);
    assert_eq!(app2.tabs[0].base_name(), "a.md");
}

#[test]
fn tab_keymap_new_next_close() {
    use iced::keyboard::{self};
    let ctrl = keyboard::Modifiers::CTRL;

    assert!(matches!(
        handle_key_defaults(keyboard::Key::Character("t".into()), ctrl),
        Some(Message::NewTab)
    ));
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Character("w".into()), ctrl),
        Some(Message::CloseTabRequest)
    ));
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::Tab), ctrl),
        Some(Message::SwitchTabNext)
    ));
    // 上一标签页：Ctrl+Shift+Tab（与 next 互逆）
    let ctrl_shift = ctrl | keyboard::Modifiers::SHIFT;
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::Tab), ctrl_shift),
        Some(Message::SwitchTabPrev)
    ));
}

#[test]
fn find_navigation_f3_family_and_prev_tab_cycle() {
    use iced::keyboard::{self};
    let none = keyboard::Modifiers::empty();
    let shift = keyboard::Modifiers::SHIFT;

    // F3 家族（裸功能键通道）映射查找导航
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::F3), none),
        Some(Message::FindNext)
    ));
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::F3), shift),
        Some(Message::FindPrev)
    ));

    // SwitchTabPrev 循环切换：3 页时从页 0 反向切到最后一页
    //（app_with_tabs 创建后活动页在最后一页，先切回页 0）
    let mut app = app_with_tabs(3);
    dispatch(&mut app, Message::SwitchTab(0));
    assert_eq!(app.active_tab, 0);
    dispatch(&mut app, Message::SwitchTabPrev);
    assert_eq!(app.active_tab, 2, "从页 0 反向应回绕到最后一页");
    dispatch(&mut app, Message::SwitchTabNext);
    assert_eq!(app.active_tab, 0, "正向切回页 0");
}

// ---------- P38 撤销回基线清脏 ----------

#[test]
fn undo_back_to_saved_content_clears_dirty_and_redo_restores() {
    let mut app = loaded_txt_app();
    assert!(!app.tab().dirty);

    // 基线之后编辑置脏 → 自动保存成功落盘 → 当前内容成为磁盘内容
    dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
    assert!(app.tab().dirty);
    let v = app.tab().version;
    dispatch(
        &mut app,
        Message::TabAutosaved(
            0,
            v,
            PathBuf::from("C:/doc/note.txt"),
            AutosaveOutcome::Written,
        ),
    );
    assert!(!app.tab().dirty);

    // 再编辑后撤销：内容恰好回到磁盘版本 → dirty 如实回清。
    // 注意先移动光标打断 P37 打字组——否则「?」并入上一组，
    // 一次撤销会直接退回基线之前（那时保持置脏才是正确行为）
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::Right, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
    assert!(app.tab().dirty);
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert!(
        !app.tab().dirty,
        "撤销回已保存内容必须回清 dirty（旧实现保持置脏）"
    );
    assert!(!app.any_dirty(), "单页应用：聚合口径同样干净");

    // 重做离开基线 → 重新置脏；再撤又回清
    dispatch(&mut app, Message::Edit(EditOp::Redo));
    assert!(app.tab().dirty, "重做到未保存状态必须重新置脏");
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert!(!app.tab().dirty);
}

#[test]
fn untitled_page_undo_to_blank_clears_dirty() {
    let mut app = Editpad::default();
    // 未命名页的基线 = 初始空白：打字置脏，退净即安全可关
    dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿".into())));
    assert!(app.any_dirty());
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert!(!app.any_dirty(), "撤销回空白的未命名页不应再拦截关窗");
    assert_eq!(app.tabs.len(), 1);

    // 部分撤销仍视为有未保存内容：两步输入只退一步
    let mut app2 = Editpad::default();
    dispatch(&mut app2, Message::Edit(EditOp::InsertText("a".into())));
    dispatch(&mut app2, Message::Edit(EditOp::InsertText("\n".into()))); // 换行独立成组
    dispatch(&mut app2, Message::Edit(EditOp::Undo));
    assert_eq!(app2.cur_handle.borrow().doc.to_text(), "a");
    assert!(app2.any_dirty(), "\"a\" 从未落盘，必须保持置脏");
}

#[test]
fn noop_backspace_delete_do_not_dirty_clean_tabs() {
    // 空操作的退格/前向删除不算编辑：干净文件在原点按 Backspace、
    // 末尾按 Delete 不得置脏、不得排队自动保存重写磁盘。
    let mut app = loaded_txt_app();
    app.settings.autosave_enabled = true;
    assert!(!app.tab().dirty);
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocStart, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert!(!app.tab().dirty, "原点 Backspace 是空操作");
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocEnd, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::Delete));
    assert!(!app.tab().dirty, "文档末尾 Delete 是空操作");
    assert!(!app.tabs[0].autosave_inflight, "空操作不得排队自动保存");
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "base");

    // 真删除仍照常置脏并排队自动保存（退格吃掉末尾字符）
    dispatch(&mut app, Message::Edit(EditOp::Backspace));
    assert!(app.tab().dirty, "真实删除必须置脏");
    assert!(app.tabs[0].autosave_inflight, "真实编辑应排队自动保存");
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "bas");
}

#[test]
fn tab_key_maps_to_smart_indent_op() {
    // P121：无选区 Tab 仍插入真实制表符（语义收进 EditOp::TabKey，
    // 由 indent_touched_lines 兜底；显示层展开见 editor.rs）
    use iced::keyboard::{self, key::Named};
    let message = handle_key_defaults(
        keyboard::Key::Named(Named::Tab),
        keyboard::Modifiers::empty(),
    )
    .expect("Tab 应产生编辑消息");
    assert!(matches!(message, Message::Edit(EditOp::TabKey(false))));
}

#[test]
fn line_op_hotkeys_dispatch_to_edit_ops() {
    // 第 57 轮行操作：组合键 → EditOp 映射抽样（全量覆盖见注册表
    // 分发一致性测试 hotkey_actions_all_dispatch_through_handle_key）
    use iced::keyboard::{self, Modifiers};
    let msg = handle_key_defaults(keyboard::Key::Character("d".into()), Modifiers::CTRL)
        .expect("Ctrl+D 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::DuplicateLines)));
    let msg = handle_key_defaults(keyboard::Key::Character("l".into()), Modifiers::CTRL)
        .expect("Ctrl+L 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::DeleteLines)));
    let msg = handle_key_defaults(
        keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+Up 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::MoveLinesUp)));
    let msg = handle_key_defaults(
        keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+Down 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::MoveLinesDown)));
    // 第 58 轮：大小写转换与行首尾清理（默认键与主流编辑器对齐的 U 系 +
    // 助记 T/L/B；全量覆盖仍由 hotkey_actions_all_dispatch_through_handle_key）
    let msg = handle_key_defaults(
        keyboard::Key::Character("u".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+U 应产生编辑消息");
    assert!(matches!(
        msg,
        Message::Edit(EditOp::ConvertCase(CaseKind::Upper))
    ));
    let msg = handle_key_defaults(keyboard::Key::Character("u".into()), Modifiers::CTRL)
        .expect("Ctrl+U 应产生编辑消息");
    assert!(matches!(
        msg,
        Message::Edit(EditOp::ConvertCase(CaseKind::Lower))
    ));
    for (ch, mode, combo) in [
        ("t", TrimMode::Trailing, "Ctrl+Shift+T"),
        ("l", TrimMode::Leading, "Ctrl+Shift+L"),
        ("b", TrimMode::Both, "Ctrl+Shift+B"),
    ] {
        let msg = handle_key_defaults(
            keyboard::Key::Character(ch.into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .unwrap_or_else(|| panic!("{combo} 应产生编辑消息"));
        assert!(
            matches!(msg, Message::Edit(EditOp::TrimLines(m)) if m == mode),
            "{combo} 映射到错误的清理模式"
        );
    }
    // 第 59 轮：行排序与去重（S/D=升/降序助记，K=去重；全量覆盖仍由
    // hotkey_actions_all_dispatch_through_handle_key）
    for (ch, order, combo) in [
        ("s", SortOrder::Ascending, "Ctrl+Shift+S"),
        ("d", SortOrder::Descending, "Ctrl+Shift+D"),
    ] {
        let msg = handle_key_defaults(
            keyboard::Key::Character(ch.into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .unwrap_or_else(|| panic!("{combo} 应产生编辑消息"));
        assert!(
            matches!(msg, Message::Edit(EditOp::SortLines(o)) if o == order),
            "{combo} 映射到错误的排序方向"
        );
    }
    let msg = handle_key_defaults(
        keyboard::Key::Character("k".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+K 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::RemoveDuplicateLines)));
    // 第 60 轮：书签套件（F2 家族 = 主流编辑器同默认键；裸功能键经
    // 第 60 轮扩约的 combo_string 进入热键系统；全量覆盖仍由
    // hotkey_actions_all_dispatch_through_handle_key）
    use iced::keyboard::key::Named as N;
    let msg = handle_key_defaults(keyboard::Key::Named(N::F2), Modifiers::CTRL)
        .expect("Ctrl+F2 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::ToggleBookmark)));
    let msg = handle_key_defaults(keyboard::Key::Named(N::F2), Modifiers::empty())
        .expect("裸 F2 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::BookmarkNext)));
    let msg = handle_key_defaults(keyboard::Key::Named(N::F2), Modifiers::SHIFT)
        .expect("Shift+F2 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::BookmarkPrev)));
    let msg = handle_key_defaults(
        keyboard::Key::Named(N::F2),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+F2 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::BookmarksClearAll)));
    let msg = handle_key_defaults(
        keyboard::Key::Character("c".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+C 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::CopyBookmarkedLines)));
    let msg = handle_key_defaults(
        keyboard::Key::Character("x".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+X 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::RemoveBookmarkedLines)));
    // 第 61 轮：括号匹配跳转（M=Matching 助记）
    let msg = handle_key_defaults(
        keyboard::Key::Character("m".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
    )
    .expect("Ctrl+Shift+M 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::JumpToMatchingBracket)));
    // 第 62 轮：行操作扩充七编辑键 + 一面板键（均可在设置页重映射）
    let key = |ch: &str| {
        handle_key_defaults(
            keyboard::Key::Character(ch.into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
    };
    for (ch, want) in [
        ("i", TabSpaceKind::LeadingTabsToSpaces),
        ("o", TabSpaceKind::AllTabsToSpaces),
        ("p", TabSpaceKind::LeadingSpacesToTabs),
    ] {
        let msg = key(ch).unwrap_or_else(|| panic!("Ctrl+Shift+{} 应产生消息", ch.to_uppercase()));
        let Message::Edit(EditOp::ConvertTabsSpaces(got)) = msg else {
            panic!("Ctrl+Shift+{ch} 应分发 ConvertTabsSpaces");
        };
        assert_eq!(got, want, "Ctrl+Shift+{ch} 转换方向不符");
    }
    for (ch, want) in [("j", EditOp::MergeLines), ("h", EditOp::SplitLine)] {
        let msg = key(ch).unwrap_or_else(|| panic!("Ctrl+Shift+{} 应产生消息", ch.to_uppercase()));
        let Message::Edit(got) = msg else {
            panic!("Ctrl+Shift+{ch} 应分发编辑消息");
        };
        assert_eq!(got, want, "Ctrl+Shift+{ch} 分发不符");
    }
    for (ch, want) in [("n", BlankKind::Empty), ("r", BlankKind::Whitespace)] {
        let msg = key(ch).unwrap_or_else(|| panic!("Ctrl+Shift+{} 应产生消息", ch.to_uppercase()));
        let Message::Edit(EditOp::DeleteEmptyLines(got)) = msg else {
            panic!("Ctrl+Shift+{ch} 应分发 DeleteEmptyLines");
        };
        assert_eq!(got, want, "Ctrl+Shift+{ch} 空行口径不符");
    }
    let msg = key("a").expect("Ctrl+Shift+A 应产生消息");
    assert!(matches!(msg, Message::FindAllToggled));
    // 第 63 轮：F5 插入日期时间 + G/Q 复制路径/文件名
    let msg = key_f5().expect("裸 F5 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::InsertDateTime)));
    let msg = key("g").expect("Ctrl+Shift+G 应产生消息");
    assert!(
        matches!(msg, Message::CopyFilePath(None)),
        "热键以活动页为目标"
    );
    let msg = key("q").expect("Ctrl+Shift+Q 应产生消息");
    assert!(matches!(msg, Message::CopyFileName(None)));
    // 第 64 轮：Ctrl+Q 行注释 + Ctrl+Shift+W 恢复上次关闭
    let msg = handle_key_defaults(keyboard::Key::Character("q".into()), Modifiers::CTRL)
        .expect("Ctrl+Q 应产生编辑消息");
    assert!(matches!(msg, Message::Edit(EditOp::ToggleLineComment)));
    let (mods, k) = parse_combo_for_test("Ctrl+Shift+W");
    let msg = handle_key_defaults(k, mods).expect("Ctrl+Shift+W 应产生消息");
    assert!(matches!(msg, Message::ReopenLastClosedFile));
}

#[test]
fn reopen_last_closed_pops_stack_and_guards_empty() {
    // 空栈：给提示、不进入加载
    let mut app = Editpad::default();
    dispatch(&mut app, Message::ReopenLastClosedFile);
    assert!(app.status.contains("没有可恢复"), "空栈应有状态栏提示");
    assert!(app.active_load.is_none());
    // 栈非空：出栈并走打开管线（Task 被 dispatch 丢弃，仅查状态）
    app.closed_stack = vec![std::path::PathBuf::from(r"C:\tmp\gone.txt")];
    app.recents_visible = true;
    dispatch(&mut app, Message::ReopenLastClosedFile);
    assert!(app.closed_stack.is_empty(), "恢复即出栈");
    assert!(app.active_load.is_some(), "复用打开管线进入 busy");
    assert!(!app.recents_visible, "面板随动作收起");
}

#[test]
fn close_tab_pushes_named_path_into_closed_stack() {
    let mut app = Editpad::default();
    app.tabs[0].path = Some(std::path::PathBuf::from(r"C:\tmp\a.txt"));
    dispatch(&mut app, Message::CloseTabAt(0));
    assert_eq!(
        app.closed_stack,
        vec![std::path::PathBuf::from(r"C:\tmp\a.txt")]
    );
    // 同一文件反复开关只占一位（去重保首现=最近期）
    app.tabs[0].path = Some(std::path::PathBuf::from(r"C:\tmp\a.txt"));
    dispatch(&mut app, Message::CloseTabAt(0));
    assert_eq!(app.closed_stack.len(), 1);
    // 未命名页不入栈
    dispatch(&mut app, Message::CloseTabAt(0));
    assert_eq!(app.closed_stack.len(), 1);
}

#[test]
fn invisibles_settings_propagate_to_all_tabs() {
    let mut app = Editpad::default();
    // 造第二页，两页都应收到下发
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::SettingsShowWhitespaceToggled(true));
    dispatch(&mut app, Message::SettingsShowLineEndingsToggled(true));
    assert!(app.settings.show_whitespace && app.settings.show_line_endings);
    for tab in &app.tabs {
        let ed = tab.editor.borrow();
        assert!(ed.show_whitespace && ed.show_line_endings);
    }
    // 关闭单边只动一边
    dispatch(&mut app, Message::SettingsShowWhitespaceToggled(false));
    for tab in &app.tabs {
        let ed = tab.editor.borrow();
        assert!(!ed.show_whitespace && ed.show_line_endings);
    }
}

#[test]
fn perform_backup_modes_and_guards() {
    use editpad_core::settings::{BACKUP_MODE_NONE, BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED};
    let dir = scratch_dir("p64-backup");
    let target = dir.join("note.txt");
    std::fs::write(&target, "OLD-VERSION").unwrap();

    // none 模式：不产生任何文件、无提示
    assert!(perform_backup_before_overwrite(&target, BACKUP_MODE_NONE).is_none());
    // 目标不存在（另存新路径场景）：无旧版可备份
    let ghost = dir.join("ghost.txt");
    assert!(perform_backup_before_overwrite(&ghost, BACKUP_MODE_SIMPLE).is_none());

    // simple：同名 .bak 覆盖式，内容 = 磁盘旧版
    let note = perform_backup_before_overwrite(&target, BACKUP_MODE_SIMPLE).unwrap();
    assert!(note.starts_with("已备份"), "{note}");
    assert_eq!(
        std::fs::read_to_string(dir.join("note.txt.bak")).unwrap(),
        "OLD-VERSION"
    );

    // timestamped：name.bak.d 目录内 name.YYYYMMDD-HHMMSS.bak
    // （.bak.d 与 simple 的 .bak 文件不同名——模式切换互不污染，
    // 本用例顺序即先 simple 后 timestamped 的实证）
    let note = perform_backup_before_overwrite(&target, BACKUP_MODE_TIMESTAMPED).unwrap();
    assert!(note.starts_with("已备份"), "{note}");
    let bakdir = dir.join("note.txt.bak.d");
    assert!(bakdir.is_dir(), "时间戳模式建目录留存");
    let entries: Vec<_> = std::fs::read_dir(&bakdir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 1, "同秒两次调用覆盖同一名，不累积");
    let fname = &entries[0];
    assert!(
        fname.starts_with("note.txt.") && fname.ends_with(".bak"),
        "文件名形如 note.txt.YYYYMMDD-HHMMSS.bak，实际 {fname}"
    );
    assert_eq!(
        std::fs::read_to_string(bakdir.join(fname)).unwrap(),
        "OLD-VERSION"
    );

    // 大文件豁免：set_len 秒建稀疏文件验证阈值短路
    let big = dir.join("big.log");
    let f = std::fs::File::create(&big).unwrap();
    f.set_len(MAX_BACKUP_SOURCE_BYTES + 1).unwrap();
    drop(f);
    let note = perform_backup_before_overwrite(&big, BACKUP_MODE_SIMPLE).unwrap();
    assert!(note.contains("跳过备份"), "{note}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn menubar_toggle_and_hover_flow() {
    let mut app = Editpad::default();
    // 悬停存位（浮层锚点数据源）
    dispatch(
        &mut app,
        Message::MenubarHovered(iced::Point::new(64.0, 10.0)),
    );
    assert_eq!(app.menubar_pos, (64.0, 10.0));
    // 开 → 同项关 → 异项切换（互斥展开）
    dispatch(&mut app, Message::MenuToggled(0));
    assert_eq!(app.menu_bar_open, Some(0));
    // 第 70 轮：打开瞬间冻结锚点——此后悬停变化不影响已展开浮层
    assert_eq!(app.menubar_anchor, (64.0, 10.0));
    dispatch(
        &mut app,
        Message::MenubarHovered(iced::Point::new(200.0, 10.0)),
    );
    assert_eq!(app.menubar_pos, (200.0, 10.0));
    assert_eq!(app.menubar_anchor, (64.0, 10.0), "展开期间锚点冻结");
    dispatch(&mut app, Message::MenuToggled(0));
    assert_eq!(app.menu_bar_open, None);
    dispatch(&mut app, Message::MenuToggled(1));
    dispatch(&mut app, Message::MenuToggled(2));
    assert_eq!(app.menu_bar_open, Some(2), "异项直接切换");
    // 背板点击落在菜单栏条带内（y < 36）→ 横移切换到目标槽位：
    // 当前开 2，点槽位 0（异项）→ 切到 0；再点槽位 0（同项）→ 关闭
    dispatch(
        &mut app,
        Message::MenubarHovered(iced::Point::new(16.0, 10.0)),
    );
    dispatch(&mut app, Message::MenubarPressed);
    assert_eq!(app.menu_bar_open, Some(0), "条带内异项=横移切换");
    dispatch(
        &mut app,
        Message::MenubarHovered(iced::Point::new(16.0, 10.0)),
    );
    dispatch(&mut app, Message::MenubarPressed);
    assert_eq!(app.menu_bar_open, None, "条带内同项=关闭");
    // 背板点击落在条带外（y ≥ 36）→ 收起
    dispatch(&mut app, Message::MenuToggled(3));
    dispatch(
        &mut app,
        Message::MenubarHovered(iced::Point::new(120.0, 200.0)),
    );
    dispatch(&mut app, Message::MenubarPressed);
    assert_eq!(app.menu_bar_open, None, "条带外=收起");
    // Esc（BarsDismissed）同样收起
    dispatch(&mut app, Message::MenuToggled(3));
    dispatch(&mut app, Message::BarsDismissed);
    assert_eq!(app.menu_bar_open, None);
}

#[test]
fn menubar_slot_idx_math() {
    // 槽位反推：左缘/中部/越左越右钳制
    assert_eq!(menubar_slot_idx(MENU_BAR_LEFT), 0);
    assert_eq!(menubar_slot_idx(MENU_BAR_LEFT + MENU_SLOT_W * 2.5), 2);
    assert_eq!(menubar_slot_idx(0.0), 0, "越左钳到槽 0");
    assert_eq!(menubar_slot_idx(9999.0), 4, "越右钳到槽 4");
}

#[test]
fn backup_mode_setting_cycles() {
    let mut app = Editpad::default();
    // 三态循环 none→simple→timestamped→none
    assert_eq!(app.settings.backup_mode, "none", "默认关闭");
    dispatch(&mut app, Message::SettingsBackupModeToggled);
    assert_eq!(app.settings.backup_mode, "simple");
    dispatch(&mut app, Message::SettingsBackupModeToggled);
    assert_eq!(app.settings.backup_mode, "timestamped");
    dispatch(&mut app, Message::SettingsBackupModeToggled);
    assert_eq!(app.settings.backup_mode, "none");
}

#[test]
fn block_selection_edit_and_esc_flow() {
    use crate::editor::BlockSel;
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("abcdef\nghijkl\n".into())),
    );
    // 直接构造块态（生产路径经 Alt+Shift 拖拽，core 生命周期单测覆盖）
    app.cur_handle.borrow_mut().block_sel = Some(BlockSel {
        anchor: crate::editor::CursorPos { line: 0, col: 1 },
        head: crate::editor::CursorPos { line: 1, col: 3 },
    });
    // Esc → CancelBlock 编辑臂：清块、不置脏
    let dirty_before = app.any_dirty();
    dispatch(
        &mut app,
        Message::KeyPressed(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::empty(),
        ),
    );
    assert!(!app.cur_handle.borrow().has_block(), "Esc 清块");
    assert_eq!(app.any_dirty(), dirty_before, "取消块不改变置脏");

    // 块态下输入：逐行替换块内容
    app.cur_handle.borrow_mut().block_sel = Some(BlockSel {
        anchor: crate::editor::CursorPos { line: 0, col: 1 },
        head: crate::editor::CursorPos { line: 1, col: 3 },
    });
    dispatch(&mut app, Message::Edit(EditOp::InsertText("Z".into())));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "aZdef\ngZjkl\n",
        "块内逐行替换 [c0,c1)"
    );

    // 块态下剪切流：Delete 臂删块（剪贴板 Task 被 dispatch 丢弃）。
    // 行0 此时为 "aZdef"，块 [1..4)="Zde" 被删 → "af"
    app.cur_handle.borrow_mut().block_sel = Some(BlockSel {
        anchor: crate::editor::CursorPos { line: 0, col: 1 },
        head: crate::editor::CursorPos { line: 0, col: 4 },
    });
    dispatch(&mut app, Message::CutRequested);
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "af\ngZjkl\n",
        "剪切删除块内容并清块"
    );
    assert!(!app.cur_handle.borrow().has_block());
}

/// 裸功能键便捷构造（第 60 轮热键契约放宽后 F 键可作默认键）。
fn key_f5() -> Option<Message> {
    use iced::keyboard::{self, key::Named};
    handle_key_defaults(
        keyboard::Key::Named(Named::F5),
        keyboard::Modifiers::empty(),
    )
}

#[test]
fn copy_path_and_name_flow_and_unnamed_guard() {
    // 未命名页：不写剪贴板，状态栏提示
    let mut app = Editpad::default();
    dispatch(&mut app, Message::CopyFilePath(None));
    assert!(app.status.contains("没有路径"), "未命名页应提示而非静默");
    dispatch(&mut app, Message::CopyFileName(Some(0)));
    assert!(app.status.contains("没有路径"));
    // 命名页：状态栏反馈复制内容（Task 由 dispatch 丢弃，无副作用）
    app.tabs[0].path = Some(std::path::PathBuf::from(r"C:\tmp\note.txt"));
    dispatch(&mut app, Message::CopyFilePath(Some(0)));
    assert_eq!(app.status, r"已复制 C:\tmp\note.txt");
    dispatch(&mut app, Message::CopyFileName(Some(0)));
    assert_eq!(app.status, "已复制 note.txt");
}

#[test]
fn find_all_panel_toggles_and_goto_selects_hit() {
    // 面板开关只在查找栏可见时有意义；跳转按索引直选命中
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FindAllToggled);
    assert!(!app.find_all_visible, "查找栏未开时面板开关是 no-op");

    dispatch(&mut app, Message::FindToggled);
    assert!(app.find_visible);
    dispatch(&mut app, Message::FindAllToggled);
    assert!(app.find_all_visible);
    dispatch(&mut app, Message::FindAllToggled);
    assert!(!app.find_all_visible);

    // 跳转流：命中表 + 编辑器文档就位后，Goto(i) 选中该命中跨度
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("foo bar\nbaz foo\n".into())),
    );
    app.matches = vec![
        editpad_core::MatchPos {
            line: 0,
            col: 0,
            len_chars: 3,
        },
        editpad_core::MatchPos {
            line: 1,
            col: 4,
            len_chars: 3,
        },
    ];
    dispatch(&mut app, Message::FindAllGoto(1));
    assert_eq!(app.match_idx, Some(1));
    {
        let ed = app.cur_handle.borrow();
        assert_eq!(
            ed.selected_text(),
            Some("foo".to_owned()),
            "Goto 按命中跨度还原选区"
        );
    }
    // 越界索引安全无操作
    dispatch(&mut app, Message::FindAllGoto(99));
    assert_eq!(app.match_idx, Some(1), "越界索引不得改变当前命中");
}

#[test]
fn match_excerpt_windows_around_hit_column() {
    assert_eq!(
        view::match_excerpt("hello world\n", 6, 96),
        "hello world",
        "短行剥行尾原样返回"
    );
    let long: String = {
        let mut s = String::new();
        for i in 0..200 {
            s.push((b'a' + (i % 26) as u8) as char);
        }
        s
    };
    // 命中在正中：内窗 19 字符 + 两端省略号 = 总长 21
    let out = view::match_excerpt(&long, 100, 21);
    assert!(out.starts_with('…') && out.ends_with('…'));
    assert_eq!(out.chars().count(), 21, "双端截断总长 = max_cols");
    // 命中贴近行首：只截尾部，头部不占位（总长 max_cols-1）
    let head = view::match_excerpt(&long, 2, 11);
    assert!(!head.starts_with('…') && head.ends_with('…'));
    assert_eq!(head.chars().count(), 10, "单端截断省一位给内容");
    // 列越界钳制不 panic
    let _ = view::match_excerpt("abc", 999, 10);
}

#[test]
fn line_ops_flow_through_app_update_marking_dirty() {
    // 行操作经统一编辑入口：置脏 + 可撤销语义与普通编辑完全一致
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("甲\n乙\n丙丁".into())),
    );
    assert!(app.any_dirty());
    // 当前行（第 3 行）上移
    dispatch(&mut app, Message::Edit(EditOp::MoveLinesUp));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n乙");
    // 复制当前行（第 2 行）到其下方
    dispatch(&mut app, Message::Edit(EditOp::DuplicateLines));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n丙丁\n乙");
    // 删除当前行（副本）
    dispatch(&mut app, Message::Edit(EditOp::DeleteLines));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n乙");
    // 行删除可经 Ctrl+Z 撤销
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n丙丁\n乙");
}

#[test]
fn case_convert_and_trim_flow_through_app_update_marking_dirty() {
    // 第 58 轮：大小写/清理经统一编辑入口——置脏、可撤销与普通编辑一致
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("  hello world  ".into())),
    );
    dispatch(
        &mut app,
        Message::Edit(EditOp::ConvertCase(CaseKind::Upper)),
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "  HELLO WORLD  ",
        "无选区=整个文档转换"
    );
    assert!(app.any_dirty());
    dispatch(&mut app, Message::Edit(EditOp::TrimLines(TrimMode::Both)));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "HELLO WORLD");
    // 每步都可撤销，且回退精确
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "  HELLO WORLD  ");
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "  hello world  ");
}

#[test]
fn sort_and_dedupe_flow_through_app_update_marking_dirty() {
    // 第 59 轮：排序/去重经统一编辑入口——置脏、可撤销与普通编辑一致
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("梨\n苹果\nApple\n苹果".into())),
    );
    assert!(app.any_dirty());
    // 码点序：'A'(0x41) < 梨(U+68A8) < 苹(U+82F9)
    dispatch(
        &mut app,
        Message::Edit(EditOp::SortLines(SortOrder::Ascending)),
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "Apple\n梨\n苹果\n苹果",
        "无选区=全文档排序"
    );
    dispatch(&mut app, Message::Edit(EditOp::RemoveDuplicateLines));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "Apple\n梨\n苹果");
    assert!(app.any_dirty(), "去重改动必须保持置脏");
    // 逐步撤销，回退精确
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "Apple\n梨\n苹果\n苹果"
    );
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "梨\n苹果\nApple\n苹果"
    );
}

#[test]
fn bookmark_ops_flow_through_app_update_without_dirtying() {
    // 第 60 轮：书签开关/跳转/清除是纯状态操作——不置脏、不排自动保存；
    // 删除标记行是真编辑——置脏 + 可撤销（文本与书签一并找回）
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("甲\n乙\n丙".into())),
    );
    let dirty_after_typing = app.any_dirty();
    assert!(dirty_after_typing);

    // 开关当前行（第 3 行）：不置脏
    dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark));
    assert_eq!(app.cur_handle.borrow().bookmarked_lines(), vec![2]);
    // 唯一书签就是当前行：跳转原地不动（主流口径）
    dispatch(&mut app, Message::Edit(EditOp::BookmarkPrev));
    assert_eq!(app.cur_handle.borrow().cursor.line, 2);
    // 移到第 1 行再标记，从第 3 行向上跳 → 落第 1 行
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocStart, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行0
    dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::Down, false)));
    dispatch(&mut app, Message::Edit(EditOp::BookmarkPrev));
    assert_eq!(app.cur_handle.borrow().cursor.line, 0);
    assert_eq!(
        app.any_dirty(),
        dirty_after_typing,
        "书签操作不得改变置脏态"
    );
    // 清除全部：同样不置脏
    dispatch(&mut app, Message::Edit(EditOp::BookmarksClearAll));
    assert!(app.cur_handle.borrow().bookmarked_lines().is_empty());

    // 标记首尾两行后删除标记行：置脏 + 撤销找回
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocStart, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行0
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocEnd, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行2
    dispatch(&mut app, Message::Edit(EditOp::RemoveBookmarkedLines));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "乙\n",
        "只删标记行，中间行连同其行尾保留"
    );
    assert!(app.any_dirty(), "删除标记行是真编辑");
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    {
        let ed = app.cur_handle.borrow();
        assert_eq!(ed.doc.to_text(), "甲\n乙\n丙");
        assert_eq!(ed.bookmarked_lines(), vec![0, 2], "撤销一并找回书签");
    }
}

#[test]
fn combo_string_contract_includes_bare_function_keys() {
    // 第 60 轮扩约：无 Ctrl 仅放行 F1~F12（至多带 Shift）；Alt 恒拒；
    // 裸字符键绝不参与热键（打字正文保护）
    use iced::keyboard::{self, key::Named, Modifiers};
    let f2 = || keyboard::Key::Named(Named::F2);
    assert_eq!(
        combo_string(Modifiers::empty(), &f2()),
        Some("F2".to_owned())
    );
    assert_eq!(
        combo_string(Modifiers::SHIFT, &f2()),
        Some("Shift+F2".to_owned())
    );
    assert_eq!(
        combo_string(Modifiers::CTRL, &f2()),
        Some("Ctrl+F2".to_owned())
    );
    assert_eq!(
        combo_string(Modifiers::CTRL | Modifiers::SHIFT, &f2()),
        Some("Ctrl+Shift+F2".to_owned())
    );
    // Alt 恒拒（AltGr 保护）
    assert_eq!(combo_string(Modifiers::ALT, &f2()), None);
    assert_eq!(combo_string(Modifiers::CTRL | Modifiers::ALT, &f2()), None);
    // 无 Ctrl 的非功能键一律拒绝
    let home = keyboard::Key::Named(Named::Home);
    assert_eq!(combo_string(Modifiers::empty(), &home), None);
    let letter = keyboard::Key::Character("a".into());
    assert_eq!(combo_string(Modifiers::empty(), &letter), None);
    assert_eq!(combo_string(Modifiers::SHIFT, &letter), None);
}

// ---------- P12 高亮后台分批补建 ----------

/// 构造一个启用了 Rust 语法高亮的 N 行文档应用，返回 (状态, 当时代次)。
fn app_with_rs_doc(lines: usize) -> (Editpad, u64) {
    let app = Editpad::default();
    let text = "fn f(x: f64) -> f64 { x /* 注释 */ }\n".repeat(lines);
    {
        let mut ed = app.cur_handle.borrow_mut();
        ed.reset_document(editpad_core::Document::from_str(&text));
        ed.set_language(Some("rs"));
    }
    let gen = app
        .cur_handle
        .borrow()
        .highlight_generation()
        .expect("已启用高亮");
    (app, gen)
}

fn pave_payload(app: &Editpad, batch: usize) -> (HlPavePayload, u64) {
    let (gen, highlighter) = app
        .cur_handle
        .borrow()
        .highlight_pave_snapshot()
        .expect("已启用高亮");
    let doc = app.cur_handle.borrow().doc.clone();
    (
        HlPavePayload {
            gen,
            doc,
            highlighter,
            total_lines: app.cur_handle.borrow().doc.line_count(),
            cancelled: Arc::new(AtomicBool::new(false)),
            batch_strides: batch,
        },
        gen,
    )
}

#[test]
fn hl_pave_stream_reports_progress_then_installs_final_state() {
    let (mut app, _) = app_with_rs_doc(600);
    let (payload, gen) = pave_payload(&app, 1);

    // 调度登记：HighlightPaveNeeded 应把任务记入在途（幂等性另有专测）
    dispatch(&mut app, Message::HighlightPaveNeeded);
    assert!(app.hl_paving.is_some(), "应登记在途铺建任务");

    // 直接驱动一次完整流并逐条喂给 update（模拟运行时投递）
    let messages = block_on(async {
        let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
        drive_hl_pave(payload, pave_run, &mut tx).await;
        drop(tx);
        let mut collected = Vec::new();
        while let Some(message) = rx.next().await {
            collected.push(message);
        }
        collected
    });

    // 600 行 → 3 个完整档位，batch=1 → 3 条进度 + 恰好 1 条完成
    let progresses: Vec<u64> = messages
        .iter()
        .filter_map(|m| match m {
            Message::HlPaveProgress(g, done) => {
                assert_eq!(*g, gen, "进度消息必须携带调度代次");
                Some(*done)
            }
            _ => None,
        })
        .collect();
    assert_eq!(progresses, vec![1, 2, 3], "进度应为累计档位数");
    let dones: Vec<_> = messages
        .iter()
        .filter_map(|m| match m {
            Message::HlPaved(g, _) => Some(*g),
            _ => None,
        })
        .collect();
    assert_eq!(dones, vec![gen], "恰好一条完成消息且代次一致");

    // 进度消息刷新状态栏；完成消息安装成果并解除登记
    for message in messages {
        dispatch(&mut app, message);
    }
    assert!(
        matches!(app.status.as_str(), "语法分析中…100%（后台）" | ""),
        "铺建完成后状态应归位，实际 {:?}",
        app.status
    );
    assert_eq!(app.hl_paving, None, "完成后必须解除在途登记");
    assert_eq!(
        app.cur_handle.borrow().highlight_checkpoints_len(),
        Some(1 + 3),
        "初始检查点 + 3 个后台档位"
    );
}

#[test]
fn stale_hl_pave_result_is_dropped_after_generation_change() {
    let (mut app, gen0) = app_with_rs_doc(600);
    dispatch(&mut app, Message::HighlightPaveNeeded);
    assert_eq!(app.hl_paving, Some(gen0));

    // 编辑换代：真实编辑触发 invalidate_from → 换代
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert_ne!(
        app.cur_handle.borrow().highlight_generation(),
        Some(gen0),
        "编辑必须换代"
    );
    let len_now = app.cur_handle.borrow().highlight_checkpoints_len();

    // 迟到的旧代成果（内容无关紧要，代次闸门负责拒收）
    let (stale_payload, _) = pave_payload(&app, 32);
    let mut enriched_hl = stale_payload.highlighter.clone();
    enriched_hl.advance_checkpoints(4, 600, &mut |i| format!("let e{i} = {i};"));
    dispatch(&mut app, Message::HlPaved(gen0, enriched_hl));

    assert_eq!(app.hl_paving, None, "过期任务的登记必须解除");
    assert_eq!(
        app.cur_handle.borrow().highlight_checkpoints_len(),
        len_now,
        "换代后的迟到成果不得覆盖当前高亮器"
    );
}

#[test]
fn cancelled_hl_pave_skips_work_but_still_replies_done() {
    let (app, _) = app_with_rs_doc(600);
    let (payload, _gen) = pave_payload(&app, 1);
    payload.cancelled.store(true, Ordering::Relaxed);
    let base_len = payload.highlighter.checkpoints_len();

    let messages = block_on(async {
        let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
        drive_hl_pave(payload, pave_run, &mut tx).await;
        drop(tx);
        let mut collected = Vec::new();
        while let Some(message) = rx.next().await {
            collected.push(message);
        }
        collected
    });

    assert!(
        messages
            .iter()
            .all(|m| !matches!(m, Message::HlPaveProgress(..))),
        "被取消的任务不得产出任何推进进度"
    );
    let dones = messages
        .iter()
        .filter(|m| matches!(m, Message::HlPaved(..)))
        .count();
    assert_eq!(dones, 1, "取消也必须回一条完成消息保持「恰好一条」语义");

    // 兜底/取消路径送回的是起点克隆：安装它等于无变化，UI 不受损
    if let Some(Message::HlPaved(_, hl)) = messages.into_iter().next() {
        assert_eq!(hl.checkpoints_len(), base_len);
    }
}

#[test]
fn hl_pave_runner_panic_still_replies_done() {
    // 推进函数崩溃也必须回完成消息（起点克隆兜底），
    // 否则「语法分析中…」永不解除——P5/P10 同款契约
    let (mut app, _) = app_with_rs_doc(600);
    let (payload, gen) = pave_payload(&app, 1);
    let base_len = payload.highlighter.checkpoints_len();

    let messages = block_on(async {
        let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
        drive_hl_pave(
            payload,
            |_payload, _report| -> editpad_core::LazyHighlighter { panic!("模拟铺建崩溃") },
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

    assert_eq!(messages.len(), 1, "panic 后只应有兜底完成消息");
    match &messages[0] {
        Message::HlPaved(g, hl) => {
            assert_eq!(*g, gen);
            assert_eq!(hl.checkpoints_len(), base_len, "兜底必须是未推进的起点");
        }
        other => panic!("应为 HlPaved，实际 {other:?}"),
    }

    // 喂给 update 后登记解除、状态归零（不卡死）
    dispatch(&mut app, messages.into_iter().next().unwrap());
    assert_eq!(app.hl_paving, None);
}

#[test]
fn schedule_highlight_pave_is_idempotent_until_generation_changes() {
    let (mut app, _) = app_with_rs_doc(600);

    // 首次调度 + 同代重发：不得作废旧任务（取消标志不变、代次不变）
    dispatch(&mut app, Message::HighlightPaveNeeded);
    let flag_first = app.hl_pave_cancel.clone();
    let slot_first = app.hl_paving;
    dispatch(&mut app, Message::HighlightPaveNeeded);
    assert!(
        Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
        "同代重复请求必须幂等跳过"
    );
    assert_eq!(app.hl_paving, slot_first);

    // 编辑换代后再发：旧任务作废（换新取消标志）、新代次重新出发
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
    dispatch(&mut app, Message::HighlightPaveNeeded);
    assert!(
        !Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
        "换代后必须重排新任务"
    );
    assert_ne!(app.hl_paving, slot_first, "新任务应携带换代后的代次");

    // 纯文本文档（无高亮器）：请求直接清空登记不派发
    let mut plain = Editpad::default();
    plain
        .cur_handle
        .borrow_mut()
        .reset_document(editpad_core::Document::from_str("plain text only\n"));
    dispatch(&mut plain, Message::HighlightPaveNeeded);
    assert_eq!(plain.hl_paving, None);
}

#[test]
fn enter_and_tab_map_to_smart_indent_ops() {
    use iced::keyboard::{self, key::Named};
    let none = keyboard::Modifiers::empty();
    let shift = keyboard::Modifiers::SHIFT;
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::Enter), none),
        Some(Message::Edit(EditOp::Enter))
    ));
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::Tab), none),
        Some(Message::Edit(EditOp::TabKey(false)))
    ));
    assert!(matches!(
        handle_key_defaults(keyboard::Key::Named(Named::Tab), shift),
        Some(Message::Edit(EditOp::TabKey(true)))
    ));
}

    #[test]
    fn word_motion_and_delete_word_hotkeys_dispatch() {
        // P122：Ctrl+←/→ 词导航、Ctrl+Backspace/Delete 删词（Backspace
        // 新入组合键白名单）
        use iced::keyboard::{self, key::Named};
        let ctrl = keyboard::Modifiers::CTRL;
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Named(Named::ArrowLeft), ctrl),
            Some(Message::Edit(EditOp::Motion(Motion::WordLeft, false)))
        ));
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Named(Named::ArrowRight), ctrl),
            Some(Message::Edit(EditOp::Motion(Motion::WordRight, false)))
        ));
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Named(Named::Backspace), ctrl),
            Some(Message::Edit(EditOp::DeleteWordLeft))
        ));
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Named(Named::Delete), ctrl),
            Some(Message::Edit(EditOp::DeleteWordRight))
        ));
    }
