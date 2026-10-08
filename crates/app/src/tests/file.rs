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
    dispatch(
        &mut md,
        Message::FileDropped(PathBuf::from("C:/doc/readme.md")),
    );
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

/// O-6：预览的**解析结果**必须按内容签名缓存。
///
/// 两条断言各挡一头：
/// - 签名不变 → 复用同一份块表。用 `Rc::ptr_eq` 观测「确实没有重解析」，
///   而不是靠耗时断言去猜（耗时在忙机器假红、温热假绿，本仓已两次因此
///   把成本契约改成可观测的计数/同一性）。
/// - 内容一变 → 立即重建，且新块表就是新文档的解析结果（预览陈本是用户
///   看得见的错误）。
///
/// ⚠️ 未覆盖的岔路（据代码推定，非实测）：**等长**改动靠键里的
/// `Tab::version` 分量兜住——真实编辑路径都经 `Tab::note_mutation` 推进
/// 版本（`tab.rs:142` 是该字段唯一写入点）。本用例只改文档长度，没有
/// 构造「等长且版本未动」的场景。
#[test]
fn markdown_preview_cache_is_reused_until_content_changes() {
    fn md_app(text: &str) -> Editpad {
        let mut app = Editpad::default();
        dispatch(
            &mut app,
            Message::FileDropped(PathBuf::from("C:/x/notes.md")),
        );
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str(text);
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        dispatch(&mut app, Message::PreviewToggled);
        assert!(app.preview_visible, ".md 页应能开预览");
        app
    }
    let cached = |app: &Editpad| -> std::rc::Rc<Vec<editpad_core::markdown::MdBlock>> {
        app.md_preview_cache
            .borrow()
            .as_ref()
            .expect("预览开态走过一次 view() 后应建立解析缓存")
            .1
            .clone()
    };

    let src = "# 标题\n\n正文 alpha\n";
    let app = md_app(src);
    let _ = app.view();
    let first = cached(&app);
    assert_eq!(
        &first[..],
        &editpad_core::markdown::parse_markdown(src)[..],
        "缓存内容必须就是本页文档的解析结果"
    );

    // 内容未变：再画 10 帧也不得重解析
    for _ in 0..10 {
        let _ = app.view();
    }
    assert!(
        std::rc::Rc::ptr_eq(&first, &cached(&app)),
        "签名未变却重建了块表：预览缓存在逐帧重解析"
    );

    // 内容变了：必须换新块表，且换的是新文档的结果
    app.cur_handle
        .borrow_mut()
        .insert_str("\n## 追加章节\n\n尾部内容 beta\n");
    let _ = app.view();
    let second = cached(&app);
    assert!(
        !std::rc::Rc::ptr_eq(&first, &second),
        "内容已变仍复用旧块表：预览会陈旧"
    );
    let now = app.cur_handle.borrow().doc.to_text();
    assert_eq!(
        &second[..],
        &editpad_core::markdown::parse_markdown(&now)[..],
        "重建后的块表必须对应改动后的正文"
    );
    assert!(
        second
            .iter()
            .any(|b| matches!(b, editpad_core::markdown::MdBlock::Heading { level: 2, .. })),
        "追加的二级标题应出现在新块表里"
    );
}

// ---------- P22 第二批：格式化 JSON ----------

/// 构造一个已按 .json 加载完成的应用。
fn json_app(text: &str) -> Editpad {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/x/data.json")),
    );
    let seq = app.job_seq;
    let doc = editpad_core::Document::from_str(text);
    dispatch(
        &mut app,
        Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
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
fn format_json_keeps_the_document_line_ending_instead_of_forcing_lf() {
    // P384：`format_json` 的输出只发 `"\n"`，而调用方把那份文本直接 `from_str` 回写
    // ⇒ 一份 CRLF 的 .json（Windows 上最常见形态）格式化一次，行尾整篇被改成 LF：
    // 状态栏行尾徽章跳档、整文件被记成已改并排一次自动保存＝磁盘上整篇翻动。
    // 〔行尾转换〕是一条**显式命令**，用户从没要求过它。兄弟入口
    // `replace_all_document` 恰恰刻意把替换文本归一到文档的主导行尾＝同一件事两份口径。
    const LF_BODY: &str = "{\n  \"a\": 1\n}"; // 格式化后的**唯一**正确形状（LF 版）
    for target in [
        editpad_core::LineEnding::CrLf,
        editpad_core::LineEnding::Cr,
        editpad_core::LineEnding::Lf,
    ] {
        // 输入用该目标行尾写成一份"已经带换行"的 JSON ⇒ detect 一定认得出主导行尾
        // （`{"a":1}` 那种单行输入根本不含换行，那样造出来的夹具看不见这一格）
        let input = target.normalize("{\n\"a\": 1\n}");
        let mut app = json_app(&input);
        assert_eq!(
            app.cur_handle.borrow().doc.line_ending(),
            target,
            "夹具自证：格式化之前这一页的主导行尾就是 {target:?}"
        );
        let expected = target.normalize(LF_BODY);

        dispatch(&mut app, Message::FormatJson);

        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            expected,
            "格式化只该改缩进与换行位置，不该顺手改行尾（{target:?}）"
        );
        assert_eq!(
            app.cur_handle.borrow().doc.line_ending(),
            target,
            "格式化之后行尾必须还是原来那一种（{target:?}）"
        );
    }
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
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/notes/plain.txt")),
    );
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
        handle_key_defaults(
            keyboard::Key::Character("f".into()),
            keyboard::Modifiers::CTRL
        ),
        Some(Message::FindToggled)
    ));
}

#[test]
fn whole_document_write_entries_refuse_read_only_pages() {
    // 只读锁原本只挡 `apply_edit` 里的 `edit_op_mutates` 那道闸，而 `FormatJson`
    // 与 `ConvertEol` 是直接调 `replace_whole_document` 换整份文档的旁路（定案
    // R-4 说的是同一件事，当时补的是替换四条入口，这两条是同族余下的）。
    // 这里断的不止「正文没被改」：这两条原本还会置脏，而 `tab_autosave_ready`
    // 只看 `dirty`——开着自动保存时，两秒后磁盘上那份被用户明确锁定不许动的
    // 文件就被覆写了，而只读页上撤销本身又被 `edit_op_mutates` 拒收，
    // 不先解锁就反悔不了。故 `autosave_inflight` 必须是 false。
    let locked = Editpad::default()
        .t(editpad_core::Key::StDocReadOnlyLocked)
        .to_owned();

    let eol_app = || {
        let app = loaded_txt_app();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("a\nb\nc\n"));
        app
    };
    // (入口名, 夹具, 要派发的消息)
    type Entry = (&'static str, fn() -> Editpad, fn() -> Message);
    let cases: [Entry; 2] = [
        (
            "FormatJson",
            || json_app("{\"b\":1}"),
            || Message::FormatJson,
        ),
        ("ConvertEol", eol_app, || {
            Message::ConvertEol(editpad_core::LineEnding::CrLf)
        }),
    ];

    for (what, build, msg) in cases {
        // 两趟：加锁必须被拒，未加锁必须照改——后者是防空转的正对照（否则
        // 「正文不变／不置脏／不排任务」四格在「夹具压根没走到写入」时也会全绿）
        for lock in [true, false] {
            let mut app = build();
            app.settings.autosave_enabled = true;
            if lock {
                app.cur_handle.borrow_mut().read_only = true;
            }
            let before = app.cur_handle.borrow().doc.to_text();
            let v0 = app.tab().version;
            app.status = "哨兵".into();

            let _ = app.update(msg());

            if lock {
                assert_eq!(
                    app.cur_handle.borrow().doc.to_text(),
                    before,
                    "{what} 不得改动只读页正文"
                );
                assert!(!app.tab().dirty, "{what} 不得在只读页置脏");
                assert_eq!(app.tab().version, v0, "{what} 不得推进版本");
                assert_eq!(
                    app.status, locked,
                    "{what} 应给出与 `Edit` 拒收分支同一句话，实得 {:?}",
                    app.status
                );
                assert!(
                    !app.tabs[0].autosave_inflight,
                    "{what} 不得给只读页排自动保存任务（那就是覆写磁盘）"
                );
            } else {
                assert_ne!(
                    app.cur_handle.borrow().doc.to_text(),
                    before,
                    "未加锁的 {what} 必须改写正文（否则加锁那趟是空断言）"
                );
                assert!(app.tab().dirty, "未加锁的 {what} 必须置脏");
                assert_eq!(app.tab().version, v0 + 1, "未加锁的 {what} 必须推进版本");
                assert!(
                    app.tabs[0].autosave_inflight,
                    "未加锁的 {what} 必须排上自动保存"
                );
            }
        }
    }
}

// ---------- P18 即时保存 ----------

/// P374（池 ⑪）：直接换整份文档的旁路入口，**忙时**一律不许动手。
///
/// 兄弟 `ConvertEol` 早就带着 `busy || active_load` 那道闸，`FormatJson` 在 P334
/// 补只读闸时没补忙闸＝定案 R-4 那族剩下的半格。可复现的形状＝**当前这一页正被一次
/// 在途重载换掉**（〔重载〕按页 id 落回同一页、不切页）：改前那一下格式化会写到"即将
/// 被整体替换"的旧正文上、置脏并推进版本，状态栏还写着「已格式化 JSON」；到货之后
/// `reset_document` 把这一切抹掉＝用户按的那一下什么都没留下，只留下一句假话。
#[test]
fn whole_document_entries_refuse_while_a_load_is_in_flight() {
    let json_doc = || json_app("{\n\"b\":1\n}");
    // (入口名, 夹具, 要派发的消息)
    type Entry = (&'static str, fn() -> Editpad, fn() -> Message);
    let cases: [Entry; 2] = [
        ("FormatJson", json_doc, || Message::FormatJson),
        ("ConvertEol", json_doc, || {
            Message::ConvertEol(editpad_core::LineEnding::CrLf)
        }),
    ];

    for (what, build, msg) in cases {
        let before = build().cur_handle.borrow().doc.to_text();

        // 对照趟：不忙时必须照改——否则下面那三格在"夹具压根没走到写入"时也会全绿
        let mut idle = build();
        let v0 = idle.tab().version;
        dispatch(&mut idle, msg());
        assert_ne!(
            idle.cur_handle.borrow().doc.to_text(),
            before,
            "对照组：不忙的 {what} 必须改写正文"
        );
        assert_eq!(idle.tab().version, v0 + 1, "对照组：{what} 应推进版本");

        // 被测趟：这一页正有一次在途加载要把它整体换掉
        let mut app = build();
        let tab_id = app.tab().id;
        dispatch(&mut app, Message::ConfirmExternalReload(tab_id));
        let job = app
            .active_load
            .clone()
            .expect("夹具：应有一次落在当前页的在途加载");
        assert!(app.busy, "夹具：加载在途即 busy");
        assert_eq!(job.tab_id, tab_id, "夹具：那一页就是要被替换的当前页");

        let _ = app.update(msg());
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            before,
            "{what} 不得往「正被整体替换」的那一页写（改前实测：旧正文被格式化并置脏）"
        );
        assert!(!app.tab().dirty, "{what} 不得在途期间置脏");
        assert_eq!(app.tab().version, v0, "{what} 不得在途期间推进版本");

        // 后果留档：那次在途加载照常到货，改前的写入在这一步被抹干净
        dispatch(
            &mut app,
            Message::Loaded(
                job.id,
                Ok((
                    editpad_core::Document::from_str("{\n  \"b\": 2\n}\n"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{\n  \"b\": 2\n}\n",
            "夹具：到货确实整体替换了这一页"
        );
    }
}

#[test]
fn edit_schedules_single_inflight_autosave_and_success_clears_dirty() {
    let mut app = loaded_txt_app();
    // P63：即时保存改为显式选择——本测试验证的是开启后的机制
    assert!(!app.settings.autosave_enabled);
    app.settings.autosave_enabled = true;

    // 编辑置脏并派发防抖任务（版本号在页上推进）
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert!(app.tab().dirty);
    assert!(app.tabs[0].autosave_inflight, "首次编辑应排队本页防抖任务");
    let scheduled_version = app.tab().version;
    let tid = app.tabs[0].id;

    // 连续再编辑：inflight 去重不重复排队；版本继续推进
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
    assert!(app.tabs[0].autosave_inflight);
    assert_eq!(app.tab().version, scheduled_version + 1);

    // 任务回报且版本一致 → 清脏解除挂起
    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            scheduled_version + 1,
            PathBuf::from("C:/doc/note.txt"),
            AutosaveOutcome::Written(editpad_core::SaveEncoding::Utf8),
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
    let tid = app.tabs[0].id;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            stale,
            PathBuf::from("C:/doc/note.txt"),
            AutosaveOutcome::Written(editpad_core::SaveEncoding::Utf8),
        ),
    );

    assert!(
        app.tab().dirty,
        "版本不符应保持置脏（迟到的成功不得清脏标记）"
    );
    // 契约更新（P183）：账目未清时必须重排一轮防抖，所以这里是
    // `inflight == true` 而不是旧的「解除挂起」——停在未挂起就等于
    // 「用户不再敲字就永远差最后一笔」。见
    // autosave_written_with_stale_version_rearms_debounce。
    assert!(
        app.tabs[0].autosave_inflight,
        "保持置脏的同时必须重新排队，不是停摆"
    );
}

/// 迟到的「写成功」+ 版本不符 = 磁盘上少最后一次编辑。旧实现在这里
/// 只是「保持置脏」就结束，**不再重排**：用户停手后没有新的触发点，
/// 自动保存静默停摆（同一函数里 `Superseded` 分支早就有重排先例）。
#[test]
fn autosave_written_with_stale_version_rearms_debounce() {
    let (mut app, path) = loaded_real_file_app("autosave-rearm");
    app.settings.autosave_enabled = true;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let stale = app.tabs[0].version;
    let tid = app.tabs[0].id;
    // 写盘窗口内又敲一个字（代次作废在途快照，但内容版本继续推进）
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
    app.tabs[0].autosave_inflight = true;

    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            stale,
            path,
            AutosaveOutcome::Written(editpad_core::SaveEncoding::Utf8),
        ),
    );

    assert!(app.tabs[0].dirty, "版本不符应保持置脏");
    assert!(
        app.tabs[0].autosave_inflight,
        "账目未清必须重新排上防抖，否则最后一笔永不落盘"
    );
}

/// 守卫拦下时**不得**自我重排（否则每 2s 撞同一面墙）；但用户〔忽略〕
/// 表态之后，仍置脏的页必须重新排上防抖。
#[test]
fn acknowledging_external_change_rearms_autosave() {
    let (mut app, path) = loaded_real_file_app("autosave-ack-rearm");
    app.settings.autosave_enabled = true;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let (tid, version) = (app.tabs[0].id, app.tabs[0].version);
    std::fs::write(&path, "externally replaced").unwrap();

    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            version,
            path.clone(),
            AutosaveOutcome::SkippedExternalChange,
        ),
    );
    assert_eq!(
        app.external_change,
        prompt_ids(&app, &[0]),
        "应交给提示条裁决"
    );
    assert!(!app.tabs[0].autosave_inflight, "被拦下的一轮不该自我重排");

    let id0 = app.tabs[0].id;
    dispatch(&mut app, Message::IgnoreExternalChange(id0));
    assert!(app.external_change.is_none());
    assert!(
        app.tabs[0].autosave_inflight,
        "裁决之后自动保存要重新生效，否则磁盘永远差这一笔"
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
    let outcome = write_to_disk(
        &path,
        &doc,
        editpad_core::SaveEncoding::Gbk,
        editpad_core::Lang::default(),
    );
    assert!(
        matches!(outcome, AutosaveOutcome::Written(_)),
        "落盘应成功，实际 {outcome:?}"
    );
    let bytes = std::fs::read(&path).unwrap();
    assert_ne!(bytes, text.as_bytes(), "磁盘不得是 UTF-8 形态");
    let loaded = editpad_core::load_document_streaming(&path, |_| {}).unwrap();
    assert_eq!(loaded.doc.to_text(), text, "按 GBK 嗅探读回应无损");
    assert_eq!(loaded.encoding, "GBK");
}

/// P263：**调度点**必须走 `autosave_encoding`（只测纯函数与写盘组合的那两条，
/// 挡不住有人把这里改回 `unwrap_or(Utf8)`）。
///
/// 判据取"跳过"这一侧，因为**改前的实现从不跳过**：标签指不到可写编码
/// （UTF-16LE/BE 等）的页应当保持置脏、等用户显式保存；而 GBK 标签的页照旧
/// 要排得上（否则就成了"自动保存悄悄对所有非 UTF-8 文件失效"）。
#[test]
fn autosave_scheduling_uses_the_encoding_rule_and_skips_unwritable_labels() {
    // ① 不可写标签 ⇒ 不排自动保存，且保持置脏
    let mut app = loaded_txt_app();
    app.settings.autosave_enabled = true;
    app.tabs[0].encoding_label = "UTF-16LE".to_owned();
    app.tabs[0].save_encoding = None;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert!(
        !app.tabs[0].autosave_inflight,
        "UTF-16 的文件不该被自动保存按 UTF-8 覆写——应当跳过这一页"
    );
    assert!(app.tab().dirty, "跳过必须保持置脏，等用户显式保存");

    // ② 正对照：GBK 标签照旧排得上（改前它排得上、但会写成 UTF-8）
    let mut app2 = loaded_txt_app();
    app2.settings.autosave_enabled = true;
    app2.tabs[0].encoding_label = "GBK".to_owned();
    app2.tabs[0].save_encoding = None;
    dispatch(&mut app2, Message::Edit(EditOp::InsertText("x".into())));
    assert!(
        app2.tabs[0].autosave_inflight,
        "GBK 页必须仍然享受自动保存，否则这条修复就变成了功能倒退"
    );
}

/// P271（原 P264）：自动保存之后，编码标签必须前进到**实际写出的那个编码**。
///
/// 走完整链路而不手搓结局：调度点的选码规则 → 真实写盘线程落盘 → 回报进
/// `update` → 标签。改前的形状是「落盘用调度时刻的编码、标签原地不动」——
/// 用户在防抖窗内改过编码菜单（那条动作只写 `save_encoding`，不碰标签）之后，
/// 状态栏报着一个文件里根本没有的编码，要等下一次手动保存才纠正。
#[test]
fn autosave_label_follows_the_encoding_actually_written() {
    // ① 本轮落盘 UTF-8：标签必须跟着走（改前停在 GBK —— 这一格是缺陷本身）
    let (mut app, path) = loaded_real_file_app("autosave-label-enc");
    app.settings.autosave_enabled = true;
    app.tabs[0].encoding_label = "GBK".to_owned();
    app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Utf8);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("中".into())));
    let text = app.tabs[0].editor.borrow().doc.to_text();
    let msg = drive_reported_autosave(&mut app, &path);
    dispatch(&mut app, msg);
    assert_eq!(
        app.tabs[0].encoding_label,
        editpad_core::SaveEncoding::Utf8.label(),
        "落盘的是 UTF-8，标签就不许停在 GBK"
    );
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        on_disk.trim_start_matches('\u{feff}'),
        text,
        "磁盘字节必须是 UTF-8 形态"
    );

    // ② 反向对照：本轮真的按 GBK 落盘 ⇒ 标签必须停在 GBK。
    // 缺了这一格，「一律写成 UTF-8」也能通过 ①。
    let (mut app2, path2) = loaded_real_file_app("autosave-label-enc2");
    app2.settings.autosave_enabled = true;
    app2.tabs[0].encoding_label = "GBK".to_owned();
    app2.tabs[0].save_encoding = None;
    dispatch(&mut app2, Message::Edit(EditOp::InsertText("中".into())));
    let text2 = app2.tabs[0].editor.borrow().doc.to_text();
    let msg2 = drive_reported_autosave(&mut app2, &path2);
    dispatch(&mut app2, msg2);
    assert_eq!(
        app2.tabs[0].encoding_label,
        editpad_core::SaveEncoding::Gbk.label(),
        "按 GBK 落盘时标签不得被改写"
    );
    let bytes2 = std::fs::read(&path2).unwrap();
    assert_ne!(
        bytes2,
        text2.as_bytes().to_vec(),
        "磁盘不得是 UTF-8 形态（GBK 落盘才是这一格的语义）"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path2);
}

/// 用**调度点同一套**选码规则造任务、交给真实写盘线程跑完，返回它将要回报的消息。
fn drive_reported_autosave(app: &mut Editpad, path: &std::path::Path) -> Message {
    use std::sync::atomic::Ordering;
    let (tid, version) = (app.tabs[0].id, app.tabs[0].version);
    let doc = app.tabs[0].editor.borrow().doc.clone();
    let enc =
        crate::autosave::autosave_encoding(app.tabs[0].save_encoding, &app.tabs[0].encoding_label)
            .expect("夹具的标签必须指得到可写编码");
    let gen = app.tabs[0].autosave_gen.clone();
    let my_gen = gen.load(Ordering::Relaxed);
    let task = AutosaveTask {
        encoding: enc,
        expected_stamp: None,
        delay: std::time::Duration::from_millis(20),
        lang: editpad_core::Lang::default(),
    };
    block_on(drive_autosave_once(
        tid,
        path.to_path_buf(),
        doc,
        version,
        gen,
        my_gen,
        task,
    ))
}

#[test]
fn reload_voids_the_autosave_still_in_flight_from_the_discarded_buffer() {
    // P381：〔重新加载〕的语义＝"丢掉我这一份、取磁盘那一份"，而它没作废在途的
    // 自动保存 ⇒ 那次写盘醒来时把**刚被用户丢掉的旧缓冲**写回磁盘＝磁盘上的内容
    // 被一份"已经不存在于任何界面"的文本覆写。兄弟场景（改路径 P146／手动保存
    // 接管／关页）都调了 `invalidate_autosave`，唯独重载这一条漏了。
    // 两格共用同一段"调度→醒来"，只有中不中间发生重载不同 ⇒ 反向那一格
    // 是反空转自证：不重载时同一份在途写盘必须**真的**落盘。
    use std::sync::atomic::Ordering;
    for reload in [false, true] {
        let (mut app, path) = loaded_real_file_app(&format!("reload-voids-autosave-{reload}"));
        app.settings.autosave_enabled = true;
        // 用户在磁盘版本后面敲了两个字：页面置脏，调度时刻＝现在
        // （刚加载完光标在文首，不先移到文末就成了"插在 base 前面"＝夹具自证逮到）
        dispatch(
            &mut app,
            Message::Edit(EditOp::Motion(crate::editor::Motion::DocEnd, false)),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("XX".into())));
        let stale = app.tabs[0].editor.borrow().doc.clone();
        assert_eq!(
            stale.to_text(),
            "baseXX",
            "夹具自证：手里那份是被丢弃的旧缓冲"
        );
        let (tid, version) = (app.tabs[0].id, app.tabs[0].version);
        // 与生产调度点同一套取法：带走页 id、代次 Arc 与调度时刻的代次值
        let gen = app.tabs[0].autosave_gen.clone();
        let my_gen = gen.load(Ordering::Relaxed);
        let enc = crate::autosave::autosave_encoding(
            app.tabs[0].save_encoding,
            &app.tabs[0].encoding_label,
        )
        .expect("夹具的标签必须指得到可写编码");

        if reload {
            // 提示条上那颗〔重新加载〕（按发起页 id 落地）
            dispatch(&mut app, Message::ConfirmExternalReload(tid));
        }

        // 那一份在途写盘此刻醒来（真跑写盘线程，与 `drive_reported_autosave` 同款）
        let msg = block_on(drive_autosave_once(
            tid,
            path.clone(),
            stale,
            version,
            gen,
            my_gen,
            AutosaveTask {
                encoding: enc,
                expected_stamp: None,
                delay: std::time::Duration::from_millis(20),
                lang: editpad_core::Lang::default(),
            },
        ));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        if reload {
            assert_eq!(on_disk, "base", "重载之后，被丢弃的旧缓冲不得写回磁盘");
            assert!(
                matches!(
                    msg,
                    Message::TabAutosaved(_, _, _, AutosaveOutcome::Superseded)
                ),
                "回报必须是\"已被作废\"，而不是写完了还说没写"
            );
        } else {
            assert_eq!(
                on_disk, "baseXX",
                "反空转自证：不重载时同一份在途写盘要真的落盘"
            );
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn autosave_failure_traces_status_keeps_dirty_and_allows_requeue() {
    let mut app = loaded_txt_app();
    app.settings.autosave_enabled = true;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert!(app.tabs[0].autosave_inflight);

    let version = app.tab().version;
    let tid = app.tabs[0].id;
    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
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
    assert!(
        !busy_app.tabs[0].autosave_inflight,
        "busy 时不得排队自动保存"
    );
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
    let tid = app.tabs[0].id;

    fs::write(&path, "external edit made this longer").unwrap();

    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            version,
            path.clone(),
            AutosaveOutcome::SkippedExternalChange,
        ),
    );
    assert!(app.tab().dirty, "拒写必须保持置脏");
    assert!(!app.tabs[0].autosave_inflight, "挂起照常解除，可重新排队");
    assert_eq!(
        app.external_change,
        prompt_ids(&app, &[0]),
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
    let tid = app.tabs[0].id;

    // 先来一条路径不符的回报（另存为/改名竞态）——账目整条丢弃，
    // 不得清脏；但 inflight 必须解除（P146：曾提前 return 漏清，
    // 该页本会话永久失去自动保存）；随后正确路径的同版本回报照常生效
    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            v,
            PathBuf::from("C:/other/renamed.txt"),
            AutosaveOutcome::Written(editpad_core::SaveEncoding::Utf8),
        ),
    );
    assert!(
        app.tab().dirty && !app.tabs[0].autosave_inflight,
        "路径不符的回报丢弃账目但必须解除挂起"
    );

    dispatch(
        &mut app,
        Message::TabAutosaved(
            tid,
            v,
            path,
            AutosaveOutcome::Written(editpad_core::SaveEncoding::Utf8),
        ),
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
    assert_eq!(
        app.external_change,
        prompt_ids(&app, &[0]),
        "应弹 P52 提示条交裁决"
    );
    assert!(app.status.contains("外部修改"), "实际 {:?}", app.status);

    // 〔忽略〕= 按磁盘现状重记戳并收条；随后 Ctrl+S 守卫放行进入保存管线
    let id0 = app.tabs[0].id;
    dispatch(&mut app, Message::IgnoreExternalChange(id0));
    assert!(app.external_change.is_none());
    dispatch(&mut app, Message::SaveRequested);
    assert!(app.busy, "有意覆盖的第二步应正常走保存");
}

#[test]
fn p340_save_as_report_lands_on_the_originating_tab() {
    // 缺陷形状：A 页起「另存为」对话框（busy=true）→ 期间 Ctrl+Tab 切到 B
    // （切页臂不看 busy）→ 选名字 ⇒ 旧实现用 `self.tab_mut()`＝**回报时刻的
    // 活动页**：B 被改名、被重写编码偏好与比对戳，落盘落的还是 B 的内容。
    let target = scratch_dir("p340-saveas-origin").join("a-was-here.txt");
    let mut app = app_with_tabs(2); // 页 0 置脏（"d0"），页 1 干净
    let origin = app.tabs[0].id;
    assert_ne!(origin, app.tabs[1].id, "夹具前提：两页 id 不同");
    let other_path_before = app.tabs[1].path.clone();
    let other_dirty_before = app.tabs[1].dirty;
    let other_text_before = app.tabs[1].editor.borrow().doc.to_text();

    app.set_active_tab(1);
    assert_eq!(app.active_tab, 1, "夹具前提：回报时活动页已不是发起页");
    dispatch(
        &mut app,
        Message::SaveTargetChosen(origin, Some(target.clone())),
    );

    // P349：归属改到落盘成功那一步才提交 ⇒ 在途期间这一页还不该认领目标
    // （旧实现在这里就已经把 path 写上了，失败后留下一个不存在的文件名）
    assert!(app.tabs[0].path.is_none(), "在途期间不得预先认领目标");
    assert_eq!(
        app.tabs[1].path, other_path_before,
        "别页路径不许被这次回报改写"
    );
    assert_eq!(
        app.tabs[1].dirty, other_dirty_before,
        "别页的脏账目不许被动"
    );
    assert_eq!(
        app.tabs[1].editor.borrow().doc.to_text(),
        other_text_before,
        "别页正文不许被动"
    );
    assert!(app.busy, "夹具前提：另存为之后确实起了一次写盘");
    // 落盘回报按发起页 id 归账：清的是 A 的脏，B 的账目一个字都不许动
    let (tid, v) = (app.tabs[0].id, app.tabs[0].version);
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(!app.tabs[0].dirty, "发起页保存成功应清脏");
    assert_eq!(
        app.tabs[0].path.as_deref(),
        Some(target.as_path()),
        "落盘成功才把归属落在**发起页**上（本轮判据的落点）"
    );
    assert_eq!(
        app.tabs[1].dirty, other_dirty_before,
        "别页账目不许被这次回报改写"
    );
    std::fs::remove_dir_all(target.parent().unwrap()).ok();
}

#[test]
fn p340_save_as_report_for_a_closed_tab_touches_nothing() {
    // 同一条回报的另一头：发起页在对话框开着时被关掉了。旧实现没有"页还在不在"
    // 这一问，会把幸存页当成发起页改名。
    let target = scratch_dir("p340-saveas-gone").join("ghost.txt");
    let mut app = app_with_tabs(2);
    let origin = app.tabs[0].id;
    dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
    assert_eq!(app.tabs.len(), 1, "夹具前提：发起页已关");
    assert_ne!(app.tabs[0].id, origin, "夹具前提：幸存页不是发起页");
    let survivor_path = app.tabs[0].path.clone();

    app.busy = true; // 生产里对话框在途必然占着 busy
    dispatch(
        &mut app,
        Message::SaveTargetChosen(origin, Some(target.clone())),
    );

    assert!(!app.busy, "无人认领的回报也必须收口 busy");
    assert_eq!(app.tabs[0].path, survivor_path, "不许把幸存页当发起页改名");
    assert!(!target.exists(), "更不该往那个路径写东西");
    std::fs::remove_dir_all(target.parent().unwrap()).ok();
}

#[test]
fn p63_save_as_target_is_restamped_not_blocked() {
    // 另存为：对话框里显式选中的目标（可能已存在且内容不同）不该被
    // 自家外部修改守卫拦下——选定目标即按其磁盘现状记戳
    let target = scratch_dir("p63-saveas").join("existing-target.txt");
    std::fs::write(&target, "old content on disk").unwrap();

    let mut app = Editpad::default(); // 未命名页，file_stamp = None
    dispatch(&mut app, Message::Edit(EditOp::InsertText("draft".into())));
    let origin = app.tab().id;
    dispatch(
        &mut app,
        Message::SaveTargetChosen(origin, Some(target.clone())),
    );

    assert!(app.busy, "另存为应直接进入保存管线而不被拦");
    // P349：归属改到**落盘成功**那一步才提交（失败就不该认领），所以这两条
    // 断言挪到回报之后——判据本身不变：目标不被 P63 拦下、记的是目标的戳
    let v = app.tab().version;
    dispatch(
        &mut app,
        Message::Saved(origin, v, Ok(editpad_core::EncodeNotice::default())),
    );
    assert_eq!(app.tabs[0].path, Some(target.clone()));
    assert_eq!(
        app.tabs[0].file_stamp,
        file_stamp(&target),
        "选定目标即按其磁盘现状重记戳"
    );
}

/// 池 ⑤：另存为的归属（路径／链接基准目录／未命名序号／该页的编码偏好）曾
/// 全部在**落盘之前**生效 ⇒ 写盘失败后这一页认领着一个磁盘上不存在的文件，
/// 用户为该页显式选过的编码偏好也一起没了（而状态栏那句「保存失败」看着像
/// 「什么都没发生」）。
#[test]
fn failed_save_as_leaves_the_page_ownership_untouched() {
    let dir = scratch_dir("p349-saveas-fail");
    // 父目录不存在 ⇒ 这次写盘必然失败（无需模拟权限或磁盘满）
    let bad = dir.join("没这个目录").join("draft.txt");
    let mut app = Editpad::default();
    app.tabs[0].untitled_num = Some(7);
    app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("稿".into())));
    let tid = app.tabs[0].id;
    let v = app.tab().version;

    dispatch(&mut app, Message::SaveTargetChosen(tid, Some(bad.clone())));
    assert!(app.busy, "夹具前提：确实起了一次写盘");
    assert!(app.tabs[0].path.is_none(), "在途期间不许先认领目标");
    assert_eq!(app.tabs[0].untitled_num, Some(7), "在途期间序号仍在");

    dispatch(&mut app, Message::Saved(tid, v, Err("拒绝访问".to_owned())));

    assert!(!app.busy, "失败照常收口 busy");
    assert!(app.status.contains("保存失败"), "实际 {:?}", app.status);
    assert!(
        app.tabs[0].path.is_none(),
        "失败之后仍不许认领那个不存在的文件"
    );
    assert_eq!(app.tabs[0].untitled_num, Some(7), "未命名序号不得被抹");
    assert_eq!(
        app.tabs[0].save_encoding,
        Some(editpad_core::SaveEncoding::Gbk),
        "该页显式选过的编码偏好不得被抹"
    );
    assert!(!bad.exists(), "夹具自证：目标位置什么都没写出来");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 上一轮 ④/⑩ 记过的那类「残值」形状：登记了却没消费干净的意图，会被之后
/// 一次**无关**的落盘回报认领。失败的那次另存为必须当场作废，否则下一次
/// Ctrl+S 成功时会把这一页改名成一个从没写过的路径。
#[test]
fn a_failed_save_as_cannot_hijack_the_next_plain_save() {
    let (mut app, path) = loaded_real_file_app("p349-stale-saveas");
    let dir = scratch_dir("p349-stale-target");
    let bad = dir.join("没这个目录").join("hijack.txt");
    let tid = app.tabs[0].id;

    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let v1 = app.tab().version;
    dispatch(&mut app, Message::SaveTargetChosen(tid, Some(bad.clone())));
    assert!(app.busy, "夹具前提：另存为已起一次写盘");
    dispatch(
        &mut app,
        Message::Saved(tid, v1, Err("拒绝访问".to_owned())),
    );

    // 之后一次普通 Ctrl+S（走原路径）成功：归属必须是**原路径**
    dispatch(&mut app, Message::SaveRequested);
    assert!(app.busy, "夹具前提：Ctrl+S 应起一次写盘");
    let v2 = app.tab().version;
    dispatch(
        &mut app,
        Message::Saved(tid, v2, Ok(editpad_core::EncodeNotice::default())),
    );
    assert_eq!(
        app.tabs[0].path.as_deref(),
        Some(path.as_path()),
        "失败的那次另存为不得留下残值"
    );
    assert_eq!(app.tabs[0].untitled_num, None, "命名页本就没有序号");
    assert!(!bad.exists(), "更不该往那个路径写东西");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- P67 编码与行尾控制 ----------

#[test]
fn p67_status_menus_toggle_mutually_and_esc_closes() {
    let mut app = loaded_txt_app();

    dispatch(&mut app, Message::ToggleEncodingMenu);
    eprintln!(
        "[P67] after enc-toggle: enc={} eol={}",
        app.encoding_menu, app.eol_menu
    );
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
fn eol_convert_items_are_disabled_for_locked_pages_and_matching_eol() {
    // 只读页上「转换行尾」由 `update/file.rs` 的闸拒收（见
    // `whole_document_write_entries_refuse_read_only_pages`）；这一格盯的是**面上的
    // 诚实**：拒收的动作不该看着能点。判据取 [`Editpad::eol_item_disabled`]
    // 的三态表格——`ViewTree::click` 打不到叠层里的按钮（同
    // `tests/bookmarks_panel.rs` 申报的那条设施边界），所以逐条点击的接线留手测。
    use editpad_core::LineEnding as LE;
    let mut app = loaded_txt_app();
    assert_eq!(
        app.cur_handle.borrow().doc.line_ending(),
        LE::Lf,
        "夹具前提：`base` 这种无换行文档落到 LF 档"
    );

    // (页锁了吗, busy 吗, 目标行尾, 该不该灰)
    let cells = [
        // 正对照：可写页上「转换为 CRLF」必须**能点**——否则下面三格会在
        // 「恒灰」的实现下也全绿（灰掉一切同样"没有能点的项"）
        (false, false, LE::CrLf, false),
        // 已是目标：转换无处可转，灰（既有口径，本轮不许改）
        (false, false, LE::Lf, true),
        // 只读页：动作会被闸拒收 ⇒ 面上也不许看着能点（本轮补的那一半）
        (true, false, LE::CrLf, true),
        // busy：这一族动作在忙碌期间一律灰（与正文右键菜单同一把尺子）
        (false, true, LE::CrLf, true),
        // 只读 + busy 同时成立
        (true, true, LE::CrLf, true),
    ];
    for (locked, busy, target, disabled) in cells {
        app.cur_handle.borrow_mut().read_only = locked;
        app.busy = busy;
        assert_eq!(
            app.eol_item_disabled(target),
            disabled,
            "read_only={locked} busy={busy} target={target:?} 的置灰判据错位"
        );
    }
}

#[test]
fn p67_save_with_encoding_sets_pref_and_enters_save_pipeline() {
    let (mut app, _path) = loaded_real_file_app("p67-enc-pref");

    dispatch(
        &mut app,
        Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk),
    );
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
    dispatch(
        &mut app,
        Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk),
    );
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
    let tid = app.tabs[0].id;

    // 同编码保存：无转码提示
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
    );
    assert_eq!(app.tabs[0].encoding_label, "GBK", "标签反映实际落盘编码");
    assert!(
        app.status.is_empty(),
        "同编码不得提示转码，实际 {:?}",
        app.status
    );

    // 不可映射：优先告警
    app.tabs[0].encoding_label = "GBK".to_owned();
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice { unmappable: true })),
    );
    assert!(
        app.status.contains("&#"),
        "应提示数值实体写入：{:?}",
        app.status
    );

    // 默认（无偏好）保存 GBK 载入的文件 → 转码提示（P6 语义保持）
    app.tabs[0].save_encoding = None;
    app.tabs[0].encoding_label = "GBK".to_owned();
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
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
    let tid = app.tabs[0].id;
    app.pending_backup_notice = Some("已备份旧版 → x.bak".to_owned());
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
    );
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
    let tid = app.tabs[0].id;
    dispatch(
        &mut app,
        Message::Saved(tid, v, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(
        !app.status.contains("已备份"),
        "转码知情权优先，实际 {:?}",
        app.status
    );
    assert!(app.pending_backup_notice.is_none());
}

#[test]
fn backup_notice_does_not_outlive_the_page_it_was_stashed_for() {
    // 提示暂存的取走必须发生在「这一页还在不在」的判定**之前**：晚到的成功
    // 回报找不到发起页时只收 busy，那份提示便留下来，被之后某一次**根本没
    // 有备份**的保存补显出来（报的是另一个文件的旧版）。
    let (mut app, path) = loaded_real_file_app("backup-notice-stale");
    let dir = path.parent().unwrap().to_path_buf();
    // 第二页：发起页关掉后幸存，用来接住后一次保存
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::SwitchTab(0));
    app.settings.backup_mode = editpad_core::settings::BACKUP_MODE_SIMPLE.to_owned();
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let id0 = app.tabs[0].id;
    let v0 = app.tabs[0].version;

    // 走生产入口（Ctrl+S）：写前备份与提示暂存都在 `start_save` 里同步发生
    dispatch(&mut app, Message::SaveRequested);
    assert!(
        dir.join("note.txt.bak").is_file(),
        "夹具自证：这次保存真的做了写前备份"
    );
    assert!(
        app.pending_backup_notice.is_some(),
        "夹具自证：备份提示由生产路径暂存（非夹具手填）"
    );

    // 落盘在途期间用户关掉发起页（Ctrl+W → 确认放弃更改），再让回报到货
    dispatch(&mut app, Message::CloseTabRequest);
    dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
    assert!(
        app.tabs.iter().all(|t| t.id != id0),
        "夹具自证：发起页确已不在"
    );
    dispatch(
        &mut app,
        Message::Saved(id0, v0, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(
        app.pending_backup_notice.is_none(),
        "发起页已关 ⇒ 那份提示当场弃置，不许留给下一次保存"
    );

    // 后果格：目标文件磁盘上本不存在 ⇒ 这次保存不会产生任何备份，
    // 状态栏就该是空的（陈旧提示若留住，正是从这里冒出来的）
    app.tabs[0].path = Some(dir.join("fresh.txt"));
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
    let id1 = app.tabs[0].id;
    let v1 = app.tabs[0].version;
    dispatch(&mut app, Message::SaveRequested);
    assert!(app.busy, "夹具自证：第二次保存确实启动了写盘");
    assert!(
        !dir.join("fresh.txt.bak").is_file(),
        "夹具自证：这次保存没有备份可留（目标文件磁盘上本不存在）"
    );
    dispatch(
        &mut app,
        Message::Saved(id1, v1, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(
        app.status.is_empty(),
        "这次保存既没转码也没备份，状态栏该被清空，实际 {:?}",
        app.status
    );
}

#[test]
fn error_status_survives_edit_noise() {
    // 「保存失败」等错误必须持久：编辑噪声不清除，直到下一条信息
    // 状态让位。旧实现打一个字就把错误抹掉且无日志可查。
    let (mut app, _path) = loaded_real_file_app("error-sticky");
    let v = app.tab().version;
    let tid = app.tabs[0].id;
    let err = Err("无法写入文件 x: disk full".to_owned());
    dispatch(&mut app, Message::Saved(tid, v, err));
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
    assert!(
        app.status.is_empty(),
        "普通信息照旧被编辑清除，实际 {:?}",
        app.status
    );
}

#[test]
fn recent_selected_prechecks_existence() {
    // 已删除/移动的最近文件条目：点击直接明示缺失，不进加载管线，
    // 条目保留（可能是暂时移动）；存在的条目照常走打开管线
    let (mut app, path) = loaded_real_file_app("recent-precheck");
    let missing = scratch_dir("recent-missing").join("gone.txt");
    app.settings
        .recent_files
        .push(missing.display().to_string());

    dispatch(
        &mut app,
        Message::RecentSelected(missing.display().to_string()),
    );
    assert!(
        app.status.contains("不存在"),
        "应明示文件缺失，实际 {:?}",
        app.status
    );
    assert!(!app.busy, "缺失文件不得进入加载管线");
    assert_eq!(
        app.settings.recent_files.len(),
        2,
        "缺失条目保留，不做静默剔除"
    );

    // 存在的条目照常打开（进入加载管线）
    dispatch(
        &mut app,
        Message::RecentSelected(path.display().to_string()),
    );
    assert!(app.busy, "存在的文件应正常进入打开管线");
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
    dispatch(
        &mut app,
        Message::ConvertEol(editpad_core::LineEnding::CrLf),
    );
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
    dispatch(
        &mut app,
        Message::ConvertEol(editpad_core::LineEnding::CrLf),
    );
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
    assert!(app.tab().save_encoding.is_none(), "重载必须重置编码偏好");

    // 另存为新路径：同样重置（旧偏好属于旧路径）——先给活动页设偏好
    app.tab_mut().save_encoding = Some(editpad_core::SaveEncoding::Utf8Bom);
    let target = scratch_dir("p67-pref-reset2").join("new.txt");
    let origin = app.tab().id;
    dispatch(&mut app, Message::SaveTargetChosen(origin, Some(target)));
    // P349：偏好归到「落盘成功」那一步才重置（失败时一个字段都不许动，
    // 那条判据见 `failed_save_as_leaves_the_page_ownership_untouched`）
    assert_eq!(
        app.tab().save_encoding,
        Some(editpad_core::SaveEncoding::Utf8Bom),
        "在途期间不得抢先重置该页的编码偏好"
    );
    let v = app.tab().version;
    dispatch(
        &mut app,
        Message::Saved(origin, v, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(app.tab().save_encoding.is_none(), "另存为必须重置编码偏好");
}

// ---------- P6 编码知情权 ----------

#[test]
fn transcode_notice_covers_all_encoding_labels() {
    use editpad_core::SaveEncoding;
    // P67 口径：提示按「原标签 vs 实际目标」判定
    // 纯 UTF-8 → UTF-8 / 未打开：无需提示
    assert_eq!(
        transcode_notice(editpad_core::Lang::ZhCn, "UTF-8", "UTF-8", false),
        None
    );
    assert_eq!(
        transcode_notice(editpad_core::Lang::ZhCn, "", "UTF-8", false),
        None
    );

    // 用户显式选择 GBK 且原文件就是 GBK：不提示（非意外转码）
    assert_eq!(
        transcode_notice(
            editpad_core::Lang::ZhCn,
            "GBK",
            SaveEncoding::Gbk.label(),
            false
        ),
        None
    );

    // 不可映射字符优先告警
    let m = transcode_notice(editpad_core::Lang::ZhCn, "UTF-8", "GBK", true).expect("应有告警");
    assert!(m.contains("&#"), "应说明数值实体写入：{m}");

    // BOM 丢失要提示
    let bom = transcode_notice(editpad_core::Lang::ZhCn, "UTF-8(BOM)", "UTF-8", false)
        .expect("BOM 丢失应有提示");
    assert!(bom.contains("BOM"));

    // 转码要提示且带出原编码名与目标
    for label in ["GBK", "UTF-16LE", "UTF-16BE"] {
        let notice = transcode_notice(editpad_core::Lang::ZhCn, label, "UTF-8", false)
            .expect("转码应有提示");
        assert!(notice.contains(label), "提示需含原编码 {label}: {notice}");
        assert!(notice.contains("UTF-8"));
    }

    // 反向：UTF-8 → GBK 同样提示
    let back = transcode_notice(
        editpad_core::Lang::ZhCn,
        "UTF-8",
        SaveEncoding::Gbk.label(),
        false,
    )
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
            editpad_core::Lang::default(),
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
    assert!(
        progress_count >= 2,
        "应有多条进度消息，实际 {progress_count}"
    );

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
            editpad_core::Lang::default(),
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
    assert_eq!(
        failures.len(),
        1,
        "panic 后应恰好一条失败 Loaded，实际 {messages:?}"
    );
    assert!(
        failures[0].contains("加载线程崩溃"),
        "错误需含兜底前缀: {}",
        failures[0]
    );
    assert!(
        failures[0].contains("模拟加载线程崩溃"),
        "错误需含 panic 信息: {}",
        failures[0]
    );

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

/// `--help` / `--version` 识别：修前这两个选项被当未知选项静默忽略，
/// `editpad --help` 的结果是打开一个空白窗口。
#[test]
fn cli_help_and_version_options_are_recognized() {
    use std::ffi::OsString;
    let args = |xs: &[&str]| -> Vec<OsString> {
        std::iter::once("editpad.exe")
            .chain(xs.iter().copied())
            .map(OsString::from)
            .collect()
    };
    assert_eq!(parse_cli_option(args(&["--help"])), Some(CliOption::Help));
    assert_eq!(parse_cli_option(args(&["-h"])), Some(CliOption::Help));
    assert_eq!(
        parse_cli_option(args(&["--version"])),
        Some(CliOption::Version)
    );
    assert_eq!(parse_cli_option(args(&["-V"])), Some(CliOption::Version));
    // 文件与选项混排：选项优先（此刻用户要看帮助，不是打开文件）
    assert_eq!(
        parse_cli_option(args(&["C:/a.txt", "--help"])),
        Some(CliOption::Help)
    );
    // 无选项 = 照旧打开文件 / 恢复会话
    assert_eq!(parse_cli_option(args(&[])), None);
    assert_eq!(parse_cli_option(args(&["C:/a.txt"])), None);
    assert_eq!(parse_cli_option(args(&["-fullscreen"])), None);
}

/// `--` 之后一律按文件路径处理：`editpad -- --help` 要打开一个叫
/// `--help` 的文件，而不是打印帮助。
#[test]
fn cli_option_scan_stops_after_double_dash() {
    use std::ffi::OsString;
    let args: Vec<OsString> = ["editpad.exe", "--", "--help", "-V"]
        .iter()
        .map(OsString::from)
        .collect();
    assert_eq!(
        parse_cli_option(args.clone()),
        None,
        "`--` 之后的 --help / -V 是文件名，不得被当选项"
    );
    assert_eq!(
        parse_cli_file_args(args),
        vec![PathBuf::from("--help"), PathBuf::from("-V")],
        "同一份参数的另一半口径：两个都当文件"
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
    app.pending_cli = vec![PathBuf::from("C:/cli/a.txt"), PathBuf::from("C:/cli/b.md")].into();

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
    dispatch(
        &mut app,
        Message::Loaded(seq1, Err("文件不存在".to_owned())),
    );
    assert_eq!(app.job_seq, 2, "单个文件打开失败不得阻断队列里后续文件");
    assert!(
        app.pending_cli.is_empty(),
        "失败结算后队列应继续弹出下一个文件"
    );
    assert!(app.busy, "下一个文件已开始加载（失败不中断排队链）");
}

// ---------- P298（第 207 轮，场景①「打开 50MB 单行日志」）：打开路径不许物化整行 ----------
//
// 两处无条件整行物化改前都在打开路径上：
// ① `recompute_max_line_cols` 为量一行宽度先 `line_text(line)`（单行文档里那就是整份文件）；
// ② `.LOG` 判定先 `doc.line_str(0)` 再 `trim_end_matches`（同样：第一行＝整份文件）。
// 判据用**次数**（`line_text_calls`），不用耗时；峰值内存少一份整文件副本才是这一档的正题。

/// 用真实打开链（FileDropped → Loaded）装一份文档，返回这一帧打开路径上的
/// **整行物化次数**；同时断"该算出来的宽度真算出来了"（防空转：一条都不物化
/// 也可能是什么都没算）。
fn open_doc_and_count_materializations(doc: editpad_core::Document) -> usize {
    let expect_chars = doc.text_len();
    let expect_lines = doc.line_count();
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/logs/app.log")),
    );
    let seq = app.job_seq;
    let handle = app.cur_handle.clone();
    handle.borrow_mut().take_line_text_calls();
    dispatch(
        &mut app,
        Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
    );
    let ed = handle.borrow();
    assert_eq!(ed.doc.text_len(), expect_chars, "夹具自检：文档确实装载了");
    assert_eq!(ed.doc.line_count(), expect_lines, "夹具自检：行数");
    // 高水位宽度必须等于最长行的字符数（纯 ASCII 夹具里 1 字符 = 1 列）：
    // 这条同时证明"零拷贝那条字符流"走通了、且结果与旧的整行物化口径一致。
    let widest = (0..expect_lines)
        .map(|l| ed.doc.line_body_len_chars(l))
        .max()
        .unwrap_or(0);
    assert_eq!(
        ed.max_line_cols, widest,
        "最大列数没算对（零拷贝那条流可能少喂了字符）"
    );
    ed.take_line_text_calls()
}

#[test]
fn p298_opening_never_materializes_a_whole_line() {
    // 形状 A＝场景①本体：一行、20 万字符（真实文件里这一行可以是 50MB）
    let single = editpad_core::Document::from_str(&"x".repeat(200_000));
    let calls_single = open_doc_and_count_materializations(single);
    // 形状 B＝常规多行文档：2 000 行 ×100 字符
    let many: String = (0..2_000)
        .map(|_| format!("{}\n", "y".repeat(100)))
        .collect();
    let calls_many = open_doc_and_count_materializations(editpad_core::Document::from_str(&many));
    eprintln!(
        "[P298] 打开路径上的整行物化次数：单行 20 万字符 {calls_single} / 2 千行 {calls_many}"
    );
    assert_eq!(
        calls_single, 0,
        "打开单行超长文档物化了 {calls_single} 次整行：其中任意一次都是整份文件的副本"
    );
    assert_eq!(calls_many, 0, "打开多行文档物化了 {calls_many} 次整行");
}

/// `.LOG` 判定的**旧算法 oracle**：逐字照抄改前那一行写法。
fn log_marker_oracle(doc: &editpad_core::Document) -> bool {
    doc.line_str(0).trim_end_matches(char::is_control) == ".LOG"
}

#[test]
fn p298_log_marker_decision_matches_the_old_trim_rule() {
    for text in [
        ".LOG",
        ".LOG\n",
        ".LOG\nentry",
        ".LOG\t",
        ".LOG\x00\x01",
        ".log",
        ".LOGX",
        "xLOG",
        "LOG",
        "",
        ".",
        ".LOG and then some printable text",
        &format!(".LOG{}", "z".repeat(100_000)),
        &"x".repeat(100_000),
    ] {
        let doc = editpad_core::Document::from_str(text);
        let app = Editpad::default();
        app.cur_handle.borrow_mut().reset_document(doc.clone());
        let got = app.cur_handle.borrow().first_line_is_log_marker();
        assert_eq!(
            got,
            log_marker_oracle(&doc),
            "首行 {text:?} 的 .LOG 判定与改前写法不一致"
        );
    }
}

// ---------- P126：.LOG 首行自动时间戳 ----------

#[test]
fn log_file_open_appends_timestamp_at_end() {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/log/app.LOG")),
    );
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str(".LOG\nentry"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    let text = app.cur_handle.borrow().doc.to_text();
    assert!(text.starts_with(".LOG\nentry\n"), "无行尾先补一行");
    assert!(text.len() > ".LOG\nentry\n".len(), "时间戳已追加");
    assert!(app.tab().dirty, "追加是普通编辑：置脏（默认不自动写盘）");
}

#[test]
fn non_log_file_open_is_untouched() {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/doc/note.txt")),
    );
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("hello"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "hello",
        "非 .LOG 不追加"
    );
}

// ---------- P130：文件监视（tail 跟随） ----------

/// P377（池 ⑩ 的加载链那一半）：加载失败的文案按**界面语言**取文。
///
/// 改前＝后台线程直接把 `CoreError` 的 `Display`（恒中文）`to_string()` 进
/// `Loaded(Err(..))`，界面只剩前缀本地化 ⇒ 英文界面看到「Failed to open: 」＋一句
/// 中文；两条兜底串（加载线程崩溃／加载流意外终止）同样硬编中文，而 lang.rs 里
/// `ErrRead`／`ErrWrite`／`ErrBinaryDetected`／`StLoadThreadCrashed`／`StLoadStreamEnded`
/// 五条键早已备好、零引用。
#[test]
fn load_failure_text_follows_the_ui_language() {
    let dir = scratch_dir("load-lang");
    let path = dir.join("bin.dat");
    std::fs::write(&path, [0u8, 1, 2, 3, 0, 7]).unwrap(); // NUL ⇒ 二进制防护拒开

    // 生产入口：drive_load 就是那句文案的成形处（loader 失败/panic 两支都走它）
    let failure = |lang, crash: bool| {
        let path = path.clone();
        block_on(async move {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            if crash {
                drive_load(
                    11,
                    lang,
                    path,
                    |_p, _cb| -> Result<editpad_core::LoadedDocument, editpad_core::CoreError> {
                        panic!("模拟崩溃");
                    },
                    &mut tx,
                )
                .await;
            } else {
                drive_load(
                    12,
                    lang,
                    path,
                    |p, cb| editpad_core::load_document_streaming(p, cb),
                    &mut tx,
                )
                .await;
            }
            drop(tx);
            let mut out: Option<String> = None;
            while let Some(message) = rx.next().await {
                if let Message::Loaded(_, Err(text)) = message {
                    out = Some(text);
                }
            }
            out.unwrap_or_default()
        })
    };

    // 拒开二进制：CoreError → 键表
    let zh = failure(editpad_core::Lang::ZhCn, false);
    let en = failure(editpad_core::Lang::En, false);
    assert!(
        zh.contains("疑似二进制文件"),
        "夹具：中文界面该走 `ErrBinaryDetected` 的中文文，实得 {zh:?}"
    );
    assert!(
        en.contains("binary file") && !en.contains('疑'),
        "英文界面不许夹中文，实得 {en:?}"
    );
    assert!(en.contains("bin.dat"), "路径还得带着，实得 {en:?}");

    // 兜底前缀同一条规矩（panic 支）。⚠️ 只断**前缀**：panic 载荷是抛出方写的原文，
    // 不在键表覆盖范围内（把它翻译了才是假话）。
    let zh2 = failure(editpad_core::Lang::ZhCn, true);
    let en2 = failure(editpad_core::Lang::En, true);
    assert!(
        zh2.contains("加载线程崩溃") && zh2.contains("模拟崩溃"),
        "中文兜底：{zh2:?}"
    );
    assert!(
        en2.starts_with("Load thread crashed: ") && en2.contains("模拟崩溃"),
        "英文界面的兜底前缀须是英文、载荷原样保留：{en2:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn monitor_toggle_and_tick_reload_with_tail_follow() {
    let dir = std::env::temp_dir().join("editpad-p130-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("monitor.log");
    std::fs::write(
        &path,
        "line1
line2
",
    )
    .unwrap();
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(path.clone()));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str(
                    "line1
line2
",
                ),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    // 开启监视
    dispatch(&mut app, Message::ToggleMonitorFile);
    assert!(app.tab().monitor);
    // 外部改写文件（size+mtime 均变）
    std::fs::write(
        &path,
        "line1
line2
line3
line4
",
    )
    .unwrap();
    // 用户在底部 → tail 跟随：巡检发起重载，归页后光标落文末
    dispatch(&mut app, Message::MonitorTick);
    assert!(app.active_load.is_some(), "巡检应发现变化并发起重载");
    let job = app.active_load.clone().unwrap();
    dispatch(
        &mut app,
        Message::Loaded(
            job.id,
            Ok((
                editpad_core::Document::from_str(
                    "line1
line2
line3
line4
",
                ),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    {
        let ed = app.cur_handle.borrow();
        assert_eq!(
            ed.doc.to_text(),
            "line1
line2
line3
line4
"
        );
        assert_eq!(
            ed.cursor.line, 4,
            "tail 跟随：光标落文末（幻影行首=最后换行之后）"
        );
    }
    // 关闭监视后巡检不再触发
    dispatch(&mut app, Message::ToggleMonitorFile);
    std::fs::write(
        &path, "line1
",
    )
    .unwrap();
    dispatch(&mut app, Message::MonitorTick);
    assert!(app.active_load.is_none(), "监视关闭后巡检不动");
    let _ = std::fs::remove_file(&path);
}

/// P221：tail 跟随是**一次性**意图，加载失败/结果被丢弃时必须作废，
/// 不得留给下一份文档。
#[test]
fn monitor_tail_follow_does_not_leak_into_the_next_load() {
    let dir = std::env::temp_dir().join("editpad-p221-test");
    let _ = std::fs::create_dir_all(&dir);
    let watched = dir.join("watch.log");
    let other = dir.join("other.txt");
    std::fs::write(&watched, "line1\nline2\n").unwrap();
    std::fs::write(&other, "a\nb\nc\nd\n").unwrap();

    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(watched.clone()));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("line1\nline2\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    dispatch(&mut app, Message::ToggleMonitorFile);
    std::fs::write(&watched, "line1\nline2\nline3\nline4\n").unwrap();
    dispatch(&mut app, Message::MonitorTick);
    let job = app.active_load.clone().expect("巡检应发起监视重载");
    assert!(
        app.monitor_pending.is_some(),
        "夹具：应已记下待归页的 tail 跟随"
    );

    // 重载失败：日志被轮转/删除正是 tail 跟随场景的日常
    dispatch(
        &mut app,
        Message::Loaded(job.id, Err("文件不存在".to_owned())),
    );
    assert!(
        app.monitor_pending.is_none(),
        "失败出口必须作废这份一次性意图（旧实现留着它）"
    );

    // 关掉那一页 → 空净无名页补回同一槽位 → 再打开另一个文件
    dispatch(&mut app, Message::CloseTabAt(0));
    assert_eq!(app.tabs.len(), 1, "关最后一页会补一个空页（不变式）");
    dispatch(&mut app, Message::FileDropped(other.clone()));
    let s2 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s2,
            Ok((
                editpad_core::Document::from_str("a\nb\nc\nd\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    {
        let ed = app.cur_handle.borrow();
        assert_eq!(ed.doc.to_text(), "a\nb\nc\nd\n", "夹具：新文件落在同一槽位");
        assert_eq!(
            ed.cursor.line, 0,
            "陈旧的 tail 跟随不得把刚打开文档的光标打到文末（旧实现在这里是 4）"
        );
        assert_eq!(ed.scroll_top, 0.0, "同理不得顺手滚动这份无关文档");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// P371（池 ⑦）：被内存护栏拒掉的监视重载，不得把归页意图留给这一页。
///
/// 意图写在 `start_loading` 之前，而拒载出口原本只按 P215 作废了链接/FIF 两份
///  goto。拒掉的那次重载根本没开始，比「加载失败」更空——它留下的视图意图要等
/// **下一次落到同一页的装载**才兑现，而巡检只给监视页重写这份意图，用户关掉
/// 监视之后那次重写就不发生了。
#[test]
fn monitor_reload_refused_by_mem_guard_leaves_no_pending_intent() {
    let dir = scratch_dir("p371-monitor-refused");
    let path = dir.join("tail.log");
    std::fs::write(&path, "line1\nline2\nline3\n").unwrap();

    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(path.clone()));
    let s1 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s1,
            Ok((
                editpad_core::Document::from_str("line1\nline2\nline3\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    dispatch(&mut app, Message::ToggleMonitorFile);
    assert!(app.tab().monitor, "夹具：监视已开");

    // 外部把文件撑成 300 MB 稀疏文件（护栏读的是元数据长度）⇒ 这次重载被拒
    std::fs::File::create(&path)
        .expect("撑开待巡检的文件")
        .set_len(300_000_000)
        .expect("置长度");
    dispatch(&mut app, Message::MonitorTick);
    assert!(
        app.active_load.is_none(),
        "夹具：超上限的重载必须被拒（不登记任务）"
    );
    let leaked = app.monitor_pending.is_some();

    // 用户关掉监视，文件恢复正常大小；聚焦巡检仍会重载这一页（它照常与磁盘不符），
    // 但不再记新意图——于是陈旧的 tail 跟随被带进这次装载
    dispatch(&mut app, Message::ToggleMonitorFile);
    std::fs::write(&path, "line1\nline2\nline3\nline4\n").unwrap();
    dispatch(&mut app, Message::WindowFocused);
    let job = app
        .active_load
        .clone()
        .expect("夹具：关掉监视后聚焦巡检仍重载这一页");
    dispatch(
        &mut app,
        Message::Loaded(
            job.id,
            Ok((
                editpad_core::Document::from_str("line1\nline2\nline3\nline4\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );

    let ed = app.cur_handle.borrow();
    assert_eq!(
        ed.doc.to_text(),
        "line1\nline2\nline3\nline4\n",
        "夹具：重载确实落进了这一页"
    );
    assert_eq!(
        ed.cursor.line, 0,
        "陈旧 tail 跟随不得把这次重载的光标打到文末（旧实现在这里是 4）"
    );
    assert_eq!(ed.scroll_top, 0.0, "同理不得顺手滚动这份文档");
    assert!(
        !leaked,
        "拒载＝这次重载根本没开始，它的归页意图必须当场作废（旧实现把它留了下来）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn monitor_skips_dirty_page() {
    let dir = std::env::temp_dir().join("editpad-p130-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("dirty.log");
    std::fs::write(
        &path, "a
",
    )
    .unwrap();
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(path.clone()));
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str(
                    "a
",
                ),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    dispatch(&mut app, Message::ToggleMonitorFile);
    // 置脏 + 外部修改：巡检绝不能静默重载丢用户工作
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("user edit".into())),
    );
    assert!(app.tab().dirty);
    std::fs::write(
        &path,
        "a
external
",
    )
    .unwrap();
    dispatch(&mut app, Message::MonitorTick);
    assert!(app.active_load.is_none(), "置脏页不得被监视重载");
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "user edita
",
    );
    let _ = std::fs::remove_file(&path);
}

// ---------- P133：链接 Ctrl+点击（路线图 E2） ----------

#[test]
fn link_clicked_url_reports_and_keeps_tabs() {
    let mut app = loaded_txt_app();
    let tabs = app.tabs.len();
    dispatch(
        &mut app,
        Message::LinkClicked(crate::editor::LinkTarget::Url(
            "https://example.com/a".to_string(),
        )),
    );
    assert_eq!(app.tabs.len(), tabs, "URL 外开不改标签页");
    assert!(
        app.status.contains("example.com"),
        "状态栏应反馈打开动作，实际：{}",
        app.status
    );
}

#[test]
fn link_clicked_file_opens_then_jumps_to_line() {
    let path = scratch_dir("p133-link").join("target.txt");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let mut app = loaded_txt_app();
    dispatch(
        &mut app,
        Message::LinkClicked(crate::editor::LinkTarget::File {
            path: path.clone(),
            line: Some(3),
        }),
    );
    assert_eq!(app.pending_link_goto, Some(3), "行号应暂存待装载结算");
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("first\nsecond\nthird\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert_eq!(app.pending_link_goto, None, "装载结算后应一次性消费");
    let cursor = app.cur_handle.borrow().cursor;
    assert_eq!(
        (cursor.line, cursor.col),
        (2, 0),
        "应跳到第 3 行行首（1 起）"
    );
    // base_dir 随装载下发 = 文件所在目录（相对路径链接的解析基准）
    assert_eq!(
        app.cur_handle.borrow().base_dir,
        path.parent().map(|p| p.to_path_buf())
    );
    let _ = std::fs::remove_file(&path);
}

// ---------- P146：自动保存代次作废 / Saved 按页 id 归账 ----------

#[test]
fn autosave_superseded_by_generation_bump_skips_write() {
    // 调度后页被编辑（代次推进）→ 睡醒的任务必须作废：曾只认磁盘戳，
    // 已作废快照照写不误（撤销回基线/「放弃更改并关闭」后磁盘与 UI
    // 双向失真）
    let dir = scratch_dir("autosave-superseded");
    let path = dir.join("note.txt");
    let gen = Arc::new(std::sync::atomic::AtomicU64::new(7));
    let task = AutosaveTask {
        encoding: editpad_core::SaveEncoding::Utf8,
        expected_stamp: None,
        delay: std::time::Duration::from_millis(50),
        lang: editpad_core::Lang::default(),
    };
    let fut = drive_autosave_once(
        1,
        path.clone(),
        editpad_core::Document::from_str("x"),
        1,
        gen.clone(),
        7,
        task,
    );
    gen.store(8, std::sync::atomic::Ordering::Relaxed);
    match block_on(fut) {
        Message::TabAutosaved(_, _, _, AutosaveOutcome::Superseded) => {}
        other => panic!("应回报 Superseded，实际 {other:?}"),
    }
    assert!(!path.exists(), "作废任务不得写盘");

    // 对照：代次未变 → 照常落盘
    let task = AutosaveTask {
        encoding: editpad_core::SaveEncoding::Utf8,
        expected_stamp: None,
        delay: std::time::Duration::from_millis(50),
        lang: editpad_core::Lang::default(),
    };
    let fut = drive_autosave_once(
        1,
        path.clone(),
        editpad_core::Document::from_str("x"),
        1,
        gen.clone(),
        8,
        task,
    );
    match block_on(fut) {
        Message::TabAutosaved(_, _, _, AutosaveOutcome::Written(_)) => {}
        other => panic!("代次一致应照常落盘，实际 {other:?}"),
    }
    assert!(path.exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn saved_report_accounts_to_origin_tab_after_switch() {
    // 保存期间切页（版本号巧合）曾把账目记到「完成时刻的活动页」——
    // 错清别页置脏标记 → 关页不再弹确认 → 未保存内容无声丢失。
    let mut app = Editpad::default();
    dispatch(&mut app, Message::Edit(EditOp::InsertText("A".into())));
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("B".into())));
    let v0 = app.tabs[0].version;
    let id0 = app.tabs[0].id;
    // 页0 发起的保存回报，在活动页已切到页1 之后才到达
    dispatch(&mut app, Message::SwitchTab(0));
    dispatch(&mut app, Message::SwitchTab(1));
    dispatch(
        &mut app,
        Message::Saved(id0, v0, Ok(editpad_core::EncodeNotice::default())),
    );
    // 账目必须落到发起页0：页0 清脏，页1 的置脏标记不得被错清
    assert!(!app.tabs[0].dirty, "发起页应按版本守卫清脏");
    assert!(app.tabs[1].dirty, "不得错清活动页的置脏标记");
}

/// 「保存并关闭」必须与 Ctrl+S 走同一条落盘管线：按页编码偏好、写前备份、
/// 外部改动守卫一个都不能少（旧版直接 `save_document_atomic` 按 UTF-8 写）。
#[test]
fn close_tab_save_backs_up_before_overwrite() {
    let (mut app, path) = loaded_real_file_app("close-tab-backup");
    app.settings.backup_mode = editpad_core::settings::BACKUP_MODE_SIMPLE.to_owned();
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    assert!(app.tabs[0].dirty);

    dispatch(&mut app, Message::CloseTabSave(0));

    let bak = path.with_file_name("note.txt.bak");
    assert!(
        bak.is_file(),
        "保存并关闭必须先做写前备份（磁盘旧内容留档），实际未见 {:?}",
        bak
    );
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "base",
        "备份的必须是覆写前的磁盘旧内容"
    );
}

#[test]
fn close_tab_save_blocked_by_external_change_guard() {
    let (mut app, path) = loaded_real_file_app("close-tab-guard");
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    std::fs::write(&path, "externally replaced").unwrap();

    dispatch(&mut app, Message::CloseTabSave(0));

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "externally replaced",
        "拦截期间原文件绝不能被覆盖"
    );
    assert!(!app.busy, "拦截不是进入保存流程");
    assert_eq!(
        app.external_change,
        prompt_ids(&app, &[0]),
        "应交外部改动提示条裁决"
    );
    assert_eq!(app.pending_close_tab, None, "被拦下时不得登记待关页");
    assert_eq!(app.tabs.len(), 1, "内容没存就不该关页");
}

/// S-4 合并后的唯一维护点 `save_page`：三条拒绝出口一律「未启动」，且
/// 只有真的启动写盘才登记 `pending_close_tab`。
/// 这条用例钉的是**合并本身**：待关标记若在拒绝出口留下残值，之后任意一次
/// 无关的 `Saved` 回报都会被抢成 `TabSaved`（关窗链劫持）；而普通 Ctrl+S
/// 那条链绝不能登记待关页。
#[test]
fn save_page_refuses_cleanly_and_registers_close_chain_only_when_starting() {
    // ① busy：不启动、不登记
    let (mut app, _p) = loaded_real_file_app("gate-busy");
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    app.busy = true;
    let (task, started) = app.save_page(0, true);
    drop(task);
    assert!(!started, "busy 期间不得启动写盘");
    assert_eq!(app.pending_close_tab, None, "未启动不得留待关标记");

    // ② P63 外部改动：不启动、交提示条裁决、不登记
    let (mut app, path) = loaded_real_file_app("gate-external");
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    std::fs::write(&path, "externally replaced").unwrap();
    let (task, started) = app.save_page(0, true);
    drop(task);
    assert!(!started, "守卫拦下时不算启动写盘");
    assert_eq!(app.pending_close_tab, None, "拦下不得留待关标记");
    assert_eq!(app.external_change, prompt_ids(&app, &[0]));

    // ③ 正常路径（保存并关闭那条链）：启动 + 登记 + 进 busy
    let (mut app, _p) = loaded_real_file_app("gate-launch");
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let tid = app.tabs[0].id;
    let (task, started) = app.save_page(0, true);
    drop(task);
    assert!(started);
    assert_eq!(app.pending_close_tab, Some(tid), "启动的那刻才登记待关页");
    assert!(app.busy, "启动写盘必须进 busy");

    // ④ 同一个正常路径经 Ctrl+S 那条链：启动，但**不**登记待关页
    let (mut app, _p) = loaded_real_file_app("gate-plain");
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let (task, started) = app.save_page(0, false);
    drop(task);
    assert!(started);
    assert_eq!(app.pending_close_tab, None, "普通保存不得登记逐页关闭链");
}

/// 逐页关窗链的两个停摆出口：未命名置脏页要交给另存为对话框接管；
/// 被 P63 守卫拦下的页必须作废关窗标记——留着的后果是用户随后任何一次
/// 成功的 Ctrl+S 都会把窗口突然关掉。
#[test]
fn save_and_close_chain_routes_unnamed_dirty_page_to_save_as() {
    let mut app = Editpad::default();
    app.settings.enable_snapshots = false; // 走逐页链而非快照直退
    dispatch(&mut app, Message::Edit(EditOp::InsertText("A".into())));
    app.tabs[0].path = Some(PathBuf::from("C:/w/a.txt"));
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("B".into())));
    assert!(app.tabs[1].path.is_none(), "用例前提：页 1 未命名且置脏");
    let (v0, id0) = (app.tabs[0].version, app.tabs[0].id);

    app.pending_close = true;
    dispatch(
        &mut app,
        Message::Saved(id0, v0, Ok(editpad_core::EncodeNotice::default())),
    );

    assert_eq!(app.active_tab, 1, "链应切到下一个置脏页");
    assert!(app.pending_close, "关窗意图尚未完成，标记该保留");
    assert!(
        app.busy,
        "未命名页必须弹另存为对话框接管本链，不能既不推进也不留提示"
    );
}

#[test]
fn save_and_close_chain_voids_pending_flag_when_guard_blocks() {
    let dir = scratch_dir("close-chain-guard");
    let (pa, pb) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&pa, "a").unwrap();
    std::fs::write(&pb, "b").unwrap();
    let mut app = Editpad::default();
    app.settings.enable_snapshots = false;
    dispatch(&mut app, Message::FileDropped(pa.clone()));
    let s1 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s1,
            Ok((
                editpad_core::Document::from_str("a"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::FileDropped(pb.clone()));
    let s2 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s2,
            Ok((
                editpad_core::Document::from_str("b"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    // 页 0 置脏（它的落盘回报由下面手工构造），页 1 真敲一个字
    app.tabs[0].dirty = true;
    dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
    // 页 1 的磁盘被外部改写 → P63 守卫必须拦下这一页的落盘
    std::fs::write(&pb, "externally replaced").unwrap();
    let (v0, id0) = (app.tabs[0].version, app.tabs[0].id);

    app.pending_close = true;
    dispatch(
        &mut app,
        Message::Saved(id0, v0, Ok(editpad_core::EncodeNotice::default())),
    );

    assert_eq!(app.active_tab, 1, "链切到了页 1");
    assert_eq!(
        app.external_change,
        prompt_ids(&app, &[1]),
        "守卫应把覆写裁决交给外部改动提示条"
    );
    assert!(!app.busy, "拦截不是进入保存流程");
    assert!(
        !app.pending_close,
        "守卫拦下时关窗标记必须作废——否则下一次 Ctrl+S 成功即突然关窗"
    );
}

/// 多选文件转发给已运行实例 → 首个进「放弃更改并打开」确认、其余留在
/// `pending_cli`。用户点「取消」只收走了确认条，队列悬挂 → 之后任意一次
/// 无关的 Loaded 收尾续排会把被放弃的文件接二连三开出来。
#[test]
fn p341_open_confirm_discard_clears_the_originating_page() {
    // 缺陷形状：A 页置脏 → 一次打开弹出「放弃更改并打开」确认条（它**不占 busy**，
    // 切页臂也不看 busy）→ 用户切到 B 页并敲了字 → 点〔放弃并打开〕 ⇒ 旧写法用
    // `self.tab_mut()` 清脏，被标成"已保存"的是 **B**：那页的未存改动此后既不弹
    // 关页确认、也进不了自动保存 = 静默丢失；而真正被放弃的 A 反倒还挂着脏账。
    let mut app = app_with_tabs(2); // 页 0 脏（"d0"），页 1 是新建页
    app.set_active_tab(0); // 确认条问的就是**活动页**的未存改动
    let pending = PathBuf::from("C:/w/next.txt");
    dispatch(&mut app, Message::FileDropped(pending.clone()));
    assert_eq!(
        app.open_confirm.as_ref().map(|(_, p)| p),
        Some(&pending),
        "夹具前提：确认条已弹出且带着待开路径"
    );

    app.set_active_tab(1);
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("B 的未存改动".into())),
    );
    assert!(
        app.tabs[0].dirty && app.tabs[1].dirty,
        "夹具前提：两页都有未存改动（否则分辨不出清错了页）"
    );
    let (a_id, b_id) = (app.tabs[0].id, app.tabs[1].id);
    let b_version = app.tabs[1].version;

    dispatch(&mut app, Message::ConfirmOpenDiscard);

    assert_eq!(app.tabs[0].id, a_id);
    assert!(!app.tabs[0].dirty, "被放弃的应当是发起页 A 的更改");
    assert!(app.tabs[1].dirty, "B 的未存改动不许被这次裁决清掉");
    assert_eq!(app.tabs[1].version, b_version, "B 的账目一字不动");
    assert_ne!(b_id, a_id);
    // 打开本身照常起——这才是用户点〔放弃并打开〕要的结果
    let job = app.active_load.as_ref().expect("确认之后应真的开始装载");
    assert_eq!(job.path, pending, "开的就是那条路径");
}

#[test]
fn p341_open_confirm_discard_after_originating_tab_closed_still_opens() {
    // 同一条裁决的另一头：确认条挂着时发起页被关掉了（关页时它自己那笔改动已经
    // 没了）。旧写法没有"页还在不在"这一问，会把幸存页当成发起页清脏。
    let mut app = app_with_tabs(1); // 只有一页，且已置脏
    let pending = PathBuf::from("C:/w/after.txt");
    dispatch(&mut app, Message::FileDropped(pending.clone()));
    let origin = app.open_confirm.as_ref().expect("确认条应弹出").0;

    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
    assert_eq!(app.tabs.len(), 1, "夹具前提：发起页已关");
    assert_ne!(app.tabs[0].id, origin, "夹具前提：幸存页不是发起页");
    assert!(!app.tabs[0].dirty, "夹具前提：幸存页本来就干净");

    dispatch(&mut app, Message::ConfirmOpenDiscard);

    assert!(
        app.active_load.as_ref().is_some_and(|j| j.path == pending),
        "发起页没了也要把用户要开的文件开出来"
    );
    assert!(!app.tabs[0].dirty, "无人认领的裁决不许改别页账目");
}

#[test]
fn cancelling_open_confirm_voids_remaining_cli_queue() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::Edit(EditOp::InsertText("local".into())));
    app.pending_cli = VecDeque::from(vec![
        PathBuf::from("C:/cli/b.txt"),
        PathBuf::from("C:/cli/c.txt"),
    ]);
    app.open_confirm = Some((app.tab().id, PathBuf::from("C:/cli/a.txt")));

    dispatch(&mut app, Message::ConfirmOpenCancel);

    assert!(app.open_confirm.is_none(), "确认条应收起");
    assert!(
        app.pending_cli.is_empty(),
        "放弃打开 = 整批作废，不得留下悬挂队列：实际 {:?}",
        app.pending_cli
    );
}

#[test]
fn esc_on_open_confirm_also_voids_cli_queue() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::Edit(EditOp::InsertText("local".into())));
    app.pending_cli = VecDeque::from(vec![PathBuf::from("C:/cli/b.txt")]);
    app.open_confirm = Some((app.tab().id, PathBuf::from("C:/cli/a.txt")));

    // Esc 在 BarsDismissed 里本就等价于「放弃打开确认」
    dispatch(&mut app, Message::BarsDismissed);

    assert!(app.open_confirm.is_none());
    assert!(!app.pending_close);
    assert!(
        app.pending_cli.is_empty(),
        "Esc 既然视作取消，队列也要一并作废：实际 {:?}",
        app.pending_cli
    );
}

/// 一次性跳转意图只能由**它所属的那次装载**兑现。旧实现把消费写在
/// `match (result, target)` 之外、且作用在「当时的活动页」上，于是取消一次
/// 打开确认后意图悬挂，下一次无关装载的收尾替它跳了行。
#[test]
fn cancelled_open_voids_link_goto_and_later_loads_do_not_honor_it() {
    let dir = scratch_dir("link-goto-cancel");
    let (cur, target) = (dir.join("cur.txt"), dir.join("t.txt"));
    std::fs::write(&cur, "c1\nc2\nc3\n").unwrap();
    std::fs::write(&target, "a\nb\nc\nd\ne\n").unwrap();
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(cur.clone()));
    let s0 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s0,
            Ok((
                editpad_core::Document::from_str("c1\nc2\nc3\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    // 当前页置脏 → 链接打开要先过「放弃更改并打开」确认
    dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
    dispatch(
        &mut app,
        Message::LinkClicked(crate::editor::LinkTarget::File {
            path: target,
            line: Some(5),
        }),
    );
    assert!(app.open_confirm.is_some(), "用例前提：确认条已弹出");
    assert_eq!(app.pending_link_goto, Some(5), "行号暂存等待裁决");

    dispatch(&mut app, Message::ConfirmOpenCancel);
    assert!(
        app.pending_link_goto.is_none(),
        "用户取消打开 = 该跳转意图一并作废，不得悬挂"
    );

    // 之后用户自己正常打开另一个文件并结算：被取消的那次跳行不得借这次
    // 收尾续排复活（旧实现正是如此——当前文档光标被凭空打跑到第 5 行）
    let later = dir.join("later.txt");
    std::fs::write(&later, "l1\nl2\nl3\nl4\nl5\nl6\n").unwrap();
    dispatch(&mut app, Message::FileDropped(later));
    let s1 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s1,
            Ok((
                editpad_core::Document::from_str("l1\nl2\nl3\nl4\nl5\nl6\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert_eq!(
        app.cur_handle.borrow().cursor.line,
        0,
        "无关装载不得替被取消的点击跳行"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// P215：busy 期间的 Ctrl+点击既不该静默失效，更不该把行号「存」给
/// 下一次装载。
///
/// 旧顺序是无条件写 `pending_link_goto` 再 `request_open`，而后者在 busy
/// 下直接 `Task::none()` 返回：用户点完什么都没发生（无提示、无打开），
/// 那个行号却一直留在状态里，等**下一次任意一次装载**结算时被消费，把
/// 无关文档的光标/滚动打到第 N 行。
#[test]
fn link_click_while_busy_opens_nothing_and_banks_no_goto() {
    let dir = scratch_dir("p215-link-busy");
    let (inflight, target) = (dir.join("inflight.txt"), dir.join("target.txt"));
    std::fs::write(&inflight, "1\n2\n3\n").unwrap();
    std::fs::write(&target, "t\n").unwrap();

    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(inflight.clone()));
    let s1 = app.job_seq;
    assert!(
        app.busy && app.active_load.is_some(),
        "夹具：应有一次在途加载未结算"
    );

    dispatch(
        &mut app,
        Message::LinkClicked(crate::editor::LinkTarget::File {
            path: target.clone(),
            line: Some(7),
        }),
    );
    assert!(app.pending_link_goto.is_none(), "busy 时不得登记跳转意图");
    assert_eq!(app.job_seq, s1, "busy 时不得再登记第二个加载任务");
    assert!(
        !app.status.is_empty(),
        "点击被守卫挡下必须留一句提示，不能静默吞掉"
    );

    // 在途那次装载结算：不得顺手把 7 兑现到它自己的页上
    dispatch(
        &mut app,
        Message::Loaded(
            s1,
            Ok((
                editpad_core::Document::from_str("1\n2\n3\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    let ed = app.tabs[app.active_tab].editor.borrow();
    assert_eq!(
        ed.cursor.line, 0,
        "无关文档的光标不得被陈旧跳转意图打跑（旧实现在这里会跳到末行）"
    );
}

#[test]
fn link_goto_applies_to_the_tab_that_received_the_file() {
    let dir = scratch_dir("link-goto-bg-tab");
    let (first, target) = (dir.join("first.txt"), dir.join("t.txt"));
    std::fs::write(&first, "x\n").unwrap();
    std::fs::write(&target, "a\nb\nc\nd\ne\n").unwrap();
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FileDropped(first));
    let s0 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s0,
            Ok((
                editpad_core::Document::from_str("x\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );

    dispatch(
        &mut app,
        Message::LinkClicked(crate::editor::LinkTarget::File {
            path: target.clone(),
            line: Some(4),
        }),
    );
    let landing = app
        .active_load
        .as_ref()
        .map(|j| j.tab_id)
        .expect("链接目标应已登记加载任务");
    // 加载在途期间用户切回页 0：结算时活动页 ≠ 接收文件的那一页
    dispatch(&mut app, Message::SwitchTab(0));
    let s1 = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            s1,
            Ok((
                editpad_core::Document::from_str("a\nb\nc\nd\ne\n"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );

    assert!(app.pending_link_goto.is_none(), "一次性消费");
    let landed_tab = app
        .tabs
        .iter()
        .position(|t| t.id == landing)
        .expect("目标页应仍在");
    assert_eq!(
        app.tabs[landed_tab].editor.borrow().cursor.line,
        3,
        "跳行必须落在接收该文件的页上"
    );
    assert_eq!(
        app.tabs[0].editor.borrow().cursor.line,
        0,
        "活动页不是接收页时不得被误跳"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn confirm_save_and_close_chains_through_all_dirty_pages() {
    // P147 回归：ASK（非快照直退）模式「保存并关闭」曾只存活动页即
    // 关窗，后台置脏页未存改动无声丢失。现逐页链式存完才关窗。
    let mut app = Editpad::default();
    app.settings.enable_snapshots = false; // session_restore_allowed=false → 走逐页链
    dispatch(&mut app, Message::Edit(EditOp::InsertText("A".into())));
    app.tabs[0].path = Some(PathBuf::from("C:/w/a.txt"));
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("B".into())));
    app.tabs[1].path = Some(PathBuf::from("C:/w/b.txt"));
    let (v0, id0) = (app.tabs[0].version, app.tabs[0].id);

    dispatch(&mut app, Message::ConfirmSaveAndClose);
    assert!(app.pending_close && app.busy);

    // 页0 的保存回报到达：仍有置脏页 → 切到页1 继续存
    dispatch(
        &mut app,
        Message::Saved(id0, v0, Ok(editpad_core::EncodeNotice::default())),
    );
    assert_eq!(app.active_tab, 1, "应切到下一个置脏页继续存");
    assert!(app.pending_close, "关窗意图必须保持到全部存完");
    assert!(!app.tabs[0].dirty);

    // 页1 的保存回报到达：无置脏页 → 消费关窗意图
    let (v1, id1) = (app.tabs[1].version, app.tabs[1].id);
    dispatch(
        &mut app,
        Message::Saved(id1, v1, Ok(editpad_core::EncodeNotice::default())),
    );
    assert!(!app.pending_close, "全部存完后消费关窗意图");
    assert!(!app.tabs[1].dirty);
}

#[test]
fn tick_stream_emits_periodic_message() {
    // P149：节拍订阅桥接语义——桥接线程睡满间隔后经 async channel
    // 投递，首拍按间隔到达（此前 Task 睡眠链占用执行器 worker 整段
    // 睡眠时长，四条链常驻占死 2~3 个 worker）。
    // P210 起改用 Monitor 作探针：转发轮询那条流现在刻意**不产空拍**
    // （见下方 pending_open_stream_* 两条用例），已经不是「周期性出声」
    // 的代表样本了。
    let mut stream = Box::pin(crate::update::tick_stream(
        &crate::update::TickKind::Monitor,
    ));
    let first = block_on(stream.as_mut().next());
    assert!(
        matches!(first, Some(Message::MonitorTick)),
        "首拍应产出节拍消息，实际 {first:?}"
    );
}

/// P210（O-7②）：转发轮询的空拍**不得产出消息**。
///
/// 修前每 400ms 无条件一条 `PendingOpenTick`，每条消息 = 一次全窗
/// tiny-skia 重绘 ≈ 2.5 次/秒的常驻开销（而握手目录绝大多数时候是空的），
/// 且「读目录 → rename 抢占 → 读 → 删」四步跑在 UI 线程上。
///
/// P212 起取货函数由订阅身份注入，故本用例**一次都不碰真实实例目录**
/// （真机若开着另一个 Editpad 实例，往 %APPDATA 写批次文件会被它抢走，
/// 既脏了用户会话也让用例互斥不掉）。判据不用耗时：先起流、跨过 ≥7 拍，
/// 再单趟抽缓冲——空拍若发过消息，此刻必然已在缓冲里（容量 1）。
#[test]
fn pending_open_stream_stays_silent_while_nothing_arrived() {
    use iced::futures::task::{noop_waker, Context};
    use std::task::Poll;
    use std::time::{Duration, Instant};
    let key = crate::update::PendingOpenPoll {
        interval_ms: 20,
        poll: || Vec::new(),
    };
    let mut stream = Box::pin(crate::update::pending_open_stream(&key));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    // ⚠️ 必须先 poll 一次：`stream::channel` 的建流闭包（含拉起桥接线程）
    // 到**首次 poll** 才执行。漏了这一步，下面睡的是「还没开始的线程」，
    // 用例空转恒绿——本条第一次就写成那样，靠「拆掉守卫看会不会转红」
    // 才抓出来。
    let started = iced::futures::Stream::poll_next(stream.as_mut(), &mut cx);
    assert!(matches!(started, Poll::Pending), "刚建流时不该有产出");
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(150) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let polled = iced::futures::Stream::poll_next(stream.as_mut(), &mut cx);
    assert!(
        matches!(polled, Poll::Pending),
        "取货函数一直空时，跑满 150ms（≥7 拍）也不该发一条消息，实际 {polled:?}"
    );
}

/// P212：与上一条对偶——**到货必须发声**。只测「空拍不发声」不够，
/// 「整条流永不产出」的错误实现能同时骗过那一条。
#[test]
fn pending_open_stream_delivers_arrived_paths() {
    use iced::futures::task::{noop_waker, Context};
    use std::task::Poll;
    use std::time::{Duration, Instant};
    let key = crate::update::PendingOpenPoll {
        interval_ms: 20,
        poll: || vec![PathBuf::from("C:/p212/a.txt")],
    };
    let mut stream = Box::pin(crate::update::pending_open_stream(&key));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let _ = iced::futures::Stream::poll_next(stream.as_mut(), &mut cx); // 起线程
    let start = Instant::now();
    loop {
        if let Poll::Ready(Some(msg)) = iced::futures::Stream::poll_next(stream.as_mut(), &mut cx) {
            match msg {
                Message::PendingOpenPaths(p) => assert_eq!(
                    p,
                    vec![PathBuf::from("C:/p212/a.txt")],
                    "取到的路径必须原样带在消息里"
                ),
                other => panic!("应产出 PendingOpenPaths，实际 {other:?}"),
            }
            break;
        }
        assert!(
            start.elapsed() < Duration::from_millis(3000),
            "取货函数一直有货，3 秒内必须出声"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// P210：busy 期间到货的转发路径改存暂存位，不混进 `pending_cli`。
/// 依据是 N-07 的既有口径——`ConfirmOpenCancel` / `BarsDismissed` 会整条
/// 清空 `pending_cli`（那批本就是用户主动取消的对象），而转发路径此刻已经
/// 从磁盘抢走、无处重试：混进去就是「取消一次确认条，第二实例双击的文件
/// 全丢」。旧实现靠「busy 时干脆不读盘」回避，读盘挪到桥接线程后改暂存，
/// 下一拍（400ms）自愈合。
#[test]
fn pending_open_paths_stash_while_busy_and_merge_in_order_when_idle() {
    let mut app = Editpad::default();
    app.busy = true;
    dispatch(
        &mut app,
        Message::PendingOpenPaths(vec![PathBuf::from("C:/pf/a.txt")]),
    );
    assert!(
        app.pending_cli.is_empty(),
        "busy 时不得混进 CLI 队列（会被取消分支连带作废）"
    );
    assert_eq!(app.pending_open_stash, vec![PathBuf::from("C:/pf/a.txt")]);

    // 不忙后的下一拍：暂存按原顺序并入，再走既有的串行续排
    app.busy = false;
    dispatch(
        &mut app,
        Message::PendingOpenPaths(vec![PathBuf::from("C:/pf/b.txt")]),
    );
    assert!(app.pending_open_stash.is_empty(), "暂存应已并入并清空");
    assert_eq!(
        app.active_load.as_ref().map(|j| j.path.clone()),
        Some(PathBuf::from("C:/pf/a.txt")),
        "先到的路径先加载"
    );
    assert_eq!(
        app.pending_cli,
        VecDeque::from(vec![PathBuf::from("C:/pf/b.txt")]),
        "余下按原顺序排队"
    );
}

/// P354：忙时到货的单实例批次，**没有第二批次也必须被打开**。
///
/// 上一轮扫描量出来的形状：`PendingOpenPaths` 是"到货通知"（P210 起空拍**一条
/// 消息都不发**），而暂存那段注释写着"节拍每 400ms 再来一次，自愈合"——那句
/// 与机制不符（本仓第 4 次）。磁盘上的批次早已被桥接线程 rename 抢走，
/// 没有第二次到货就永远没人再驱动它：用户双击要打开的文件**静默丢失**。
/// 上面那条既有用例是靠**再投一条** `PendingOpenPaths` 才让 a.txt 被并走的，
/// 所以它恰好掩盖了这一格。
#[test]
fn a_stashed_pending_open_opens_without_a_second_arrival() {
    let mut app = Editpad::default();
    // 场景：打开/另存为对话框在途（busy），第二实例把用户双击的文件转发进来
    app.busy = true;
    dispatch(
        &mut app,
        Message::PendingOpenPaths(vec![PathBuf::from("C:/pf/a.txt")]),
    );
    assert_eq!(app.pending_open_stash, vec![PathBuf::from("C:/pf/a.txt")]);
    assert!(app.active_load.is_none(), "busy 期间不许起装载");
    assert!(
        app.pending_cli.is_empty(),
        "也不许混进 CLI 队列（取消分支会连带作废整条队列）"
    );

    // 对话框取消＝busy 归零；此后再没有任何到货通知。
    // ⚠️ 别把这一步断成 `!app.busy`：批次被接上之后 busy 会因**这次真的开始装载**
    // 而重新为真，那正是本用例要的结果。自证落在「装载作业指向的就是那一批路径」。
    dispatch(&mut app, Message::FileChosen(None));
    assert!(
        app.pending_open_stash.is_empty(),
        "批次不许继续躺在暂存里——路径已从磁盘抢走，不会有人再送来"
    );
    assert_eq!(
        app.active_load.as_ref().map(|j| j.path.clone()),
        Some(PathBuf::from("C:/pf/a.txt")),
        "用户双击的那个文件必须真的开始打开"
    );
}

/// P210：抢占读取内核的首批用例。P148 那段「rename 原子抢占 + 读后即删」
/// 守着的是「用户双击的文件会不会被静默吞掉」，此前**一条用例都没有**。
#[test]
fn pending_open_claim_reads_and_removes_each_batch() {
    let dir = scratch_dir("p210-claim");
    // 两批（pid 命名，P148 的并发口径）+ 一堆非批次名
    std::fs::write(
        dir.join("pending_open.111.txt"),
        "C:/a.txt\n  C:/b.txt  \n\n",
    )
    .unwrap();
    std::fs::write(dir.join("pending_open.222.txt"), "C:/c.txt").unwrap();
    std::fs::write(dir.join("session.toml"), "C:/noise.txt").unwrap();
    std::fs::write(dir.join("pending_open.333.bak"), "C:/noise2.txt").unwrap();

    let mut got = crate::single_instance::take_pending_open_in(&dir);
    got.sort();
    assert_eq!(
        got,
        vec![
            PathBuf::from("C:/a.txt"),
            PathBuf::from("C:/b.txt"),
            PathBuf::from("C:/c.txt")
        ],
        "批次路径应逐行取出（两端空白与空行剔除），非批次名不得认领"
    );
    // 取后即删：同目录再取必须是空表——这正是「空拍不发消息」的判据源
    assert!(
        crate::single_instance::take_pending_open_in(&dir).is_empty(),
        "批次文件应已被认领并删除，且噪音文件不得被吞"
    );
    assert!(dir.join("session.toml").exists(), "非批次文件一律不碰");
    let _ = std::fs::remove_dir_all(&dir);
}

/// P210：上一轮认领后崩溃留下的 `.taking-` 半途文件，下一轮照常读（P148
/// 注释里承诺的自愈路径）。它同样匹配 `pending_open.*.txt`，靠的是命名
/// 前缀而非运气——本用例把这条承诺钉住。
#[test]
fn pending_open_claim_recovers_leftover_taking_file() {
    let dir = scratch_dir("p210-taking");
    std::fs::write(dir.join("pending_open.taking-999.txt"), "C:/x.txt").unwrap();
    assert_eq!(
        crate::single_instance::take_pending_open_in(&dir),
        vec![PathBuf::from("C:/x.txt")],
        "半途文件应被下一轮认领"
    );
    assert!(
        crate::single_instance::take_pending_open_in(&dir).is_empty(),
        "认领后不留残骸"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// P345 夹具：走到「〔保存并关闭〕已落盘成功，但期间用户又编辑了一笔 ⇒
/// 页没关掉、仍置脏」这一步，返回 `(app, 文件路径, 落盘时刻的版本)`。
///
/// 无头 `dispatch` 不执行 Task，所以磁盘由本夹具代写＝模型化那次成功落盘
/// （写的是**调度时刻**的内容，与真实线程一致）。这一步之后 `file_stamp`
/// 与 `encoding_label` 该不该跟着磁盘走，就是下面两条用例的判据。
///
/// 编码偏好在落盘前置成 GBK：这样标签那一半也有真实形状可断（否则标签
/// 本来就等于目标编码，改了也不动）。
fn close_chain_survivor(tag: &str) -> (Editpad, PathBuf, u64) {
    let (mut app, path) = loaded_real_file_app(tag);
    app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);
    dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
    let tid = app.tabs[0].id;
    let scheduled_version = app.tab().version;
    let scheduled_text = app.cur_handle.borrow().doc.to_text();

    dispatch(&mut app, Message::CloseTabSave(0));
    assert!(app.busy, "夹具：写盘应在途");
    assert_eq!(app.pending_close_tab, Some(tid), "夹具：应已登记待关页");

    // 页仍是活动页，用户照常打字 ⇒ 版本推进
    dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
    assert_ne!(
        app.tab().version,
        scheduled_version,
        "夹具：回报必须赶不上这次编辑"
    );

    std::fs::write(&path, &scheduled_text).unwrap();
    dispatch(&mut app, Message::TabSaved(tid, scheduled_version, Ok(())));
    assert_eq!(app.tabs.len(), 1, "版本不符 → 取消自动关闭，页留着");
    assert!(app.tab().dirty, "留着必须保持置脏");
    assert!(!app.busy, "回报已收口");
    (app, path, scheduled_version)
}

#[test]
fn own_close_chain_write_is_not_an_external_change_for_the_next_save() {
    // P50/P63 那条口径（自家落盘之后按磁盘现状重记戳）在 `Saved` 臂上有
    // session.rs 的用例守着；兄弟臂 `TabSaved` 漏了这一步 ⇒ 页留着时，
    // 下一次 Ctrl+S 把自己那次写盘当成「文件被外部修改」拦下来，让用户
    // 去裁决一个自己造成的冲突
    let (mut app, _path, _) = close_chain_survivor("p345-stamp");

    dispatch(&mut app, Message::SaveRequested);

    assert_eq!(
        app.external_change, None,
        "自家那次落盘不得触发外部修改提示条"
    );
    assert!(app.busy, "应真的起了一次写盘，实际状态 {:?}", app.status);
}

#[test]
fn own_close_chain_write_moves_the_encoding_label_to_the_disk_truth() {
    // 同一处漏口的另一半：状态栏标签的口径是「磁盘上真实的编码」（P271），
    // 与本页是否仍置脏无关。漏了这一步，标签停在载入时刻的 UTF-8 而文件
    // 已是 GBK ⇒ 下一次改编码时 `transcode_notice` 拿错的 prev_label 比
    let (app, _path, _) = close_chain_survivor("p345-label");

    assert_eq!(
        app.tabs[0].encoding_label, "GBK",
        "标签应跟上磁盘上真实的编码"
    );
}
