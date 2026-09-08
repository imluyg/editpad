use super::tests::*;
use super::*;

// ---------- B10 多光标一期：附加光标集 ----------

#[test]
fn extra_cursor_toggle_add_remove_and_invariants() {
    let mut c = core_with("abcdef\nghijkl\nmnopqr");
    assert!(!c.has_multi());

    // 加两个附加光标（乱序注入 → 恒按文档位序升序）
    assert!(c.toggle_extra_cursor(CursorPos { line: 2, col: 1 }));
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 3 }));
    assert!(c.has_multi());
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![
            CursorPos { line: 0, col: 3 },
            CursorPos { line: 2, col: 1 },
        ],
        "附加光标恒按文档位序升序"
    );
    assert_eq!(
        c.all_cursors(),
        vec![
            CursorPos { line: 0, col: 0 }, // 主光标缺省 (0,0) 并入全集
            CursorPos { line: 0, col: 3 },
            CursorPos { line: 2, col: 1 },
        ],
        "全集 = 主光标 + 附加（去重排序）"
    );

    // 再点同一位置 = 移除
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 3 }));
    assert_eq!(c.extra_cursors.len(), 1);

    // 点击主光标位 = no-op（集合无变化）
    let main = c.cursor;
    assert!(!c.toggle_extra_cursor(main));
    assert_eq!(c.extra_cursors.len(), 1);
}

#[test]
fn extra_cursor_guards_and_cap() {
    // 折行开态拒绝
    let mut c = core_with("abc\ndef");
    c.set_word_wrap(true);
    assert!(!c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    c.set_word_wrap(false);

    // 列块态拒绝
    c.begin_block_select(CursorPos { line: 0, col: 0 });
    c.update_block_select(CursorPos { line: 1, col: 1 });
    assert!(!c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    c.clear_block();

    // 组字态拒绝
    c.ime_preedit("拼音".to_owned());
    assert!(!c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    c.ime_preedit(String::new());

    // 封顶：MAX_EXTRA_CURSORS 个之后拒新增（位置跨越多行构造）
    let lines = "x\n".repeat(MAX_EXTRA_CURSORS / 2 + 2);
    let mut big = core_with(&lines);
    for i in 0..=MAX_EXTRA_CURSORS / 2 + 1 {
        for j in 0..2 {
            if big.extra_cursors.len() >= MAX_EXTRA_CURSORS {
                break;
            }
            let pos = CursorPos { line: i, col: j };
            if pos == big.cursor {
                continue; // 主光标位 (0,0) 跳过
            }
            assert!(big.toggle_extra_cursor(pos), "未封顶前必须可加");
        }
    }
    assert_eq!(big.extra_cursors.len(), MAX_EXTRA_CURSORS);
    assert!(
        !big.toggle_extra_cursor(CursorPos {
            line: MAX_EXTRA_CURSORS / 2 + 2,
            col: 0
        }),
        "封顶后拒新增"
    );

    // 折叠：返回是否原本存在；空集再折叠返回 false
    assert!(big.collapse_multi());
    assert!(!big.has_multi());
    assert!(!big.collapse_multi());
}

#[test]
fn multi_motion_horizontal_steps_and_boundary_collapse() {
    let mut c = core_with("abcdef\nxy\nlonger");
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 0 })); // 行首（Left 越界）
    assert!(c.toggle_extra_cursor(CursorPos { line: 2, col: 3 }));

    // Right：主光标 1→2；行 2 光标 3→4；行 1 光标 0→1
    c.apply_motion(Motion::Right, false);
    assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![
            CursorPos { line: 1, col: 1 },
            CursorPos { line: 2, col: 4 },
        ]
    );

    // 行 1 光标步进到行尾再 Right = 该光标折叠；主光标与行 2 光标保持
    for _ in 0..2 {
        c.apply_motion(Motion::Right, false);
    }
    // 行 1：col 1 → 2（"xy" 长 2）→ 再 Right 越界折叠
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![CursorPos { line: 2, col: 6 }],
        "行尾 Right 折叠该附加光标"
    );
    assert!(c.has_multi());

    // Left：主光标回退，行 2 光标回退
    c.apply_motion(Motion::Left, false);
    c.apply_motion(Motion::Left, false);
    assert_eq!(c.cursor, CursorPos { line: 0, col: 2 }, "主光标同步回退");
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![CursorPos { line: 2, col: 4 }]
    );

    // 竖向移动 = 整体折叠为单光标（白名单外动作）
    c.apply_motion(Motion::Down, false);
    assert!(!c.has_multi(), "非行内动作折叠多光标");
}

#[test]
fn multi_collapse_on_select_all_word_wrap_and_undo_path() {
    // select_all（白名单外）折叠
    let mut c = core_with("ab\ncd");
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 1 }));
    c.select_all();
    assert!(!c.has_multi(), "SelectAll 折叠多光标");

    // 折行开启折叠
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    c.set_word_wrap(true);
    assert!(!c.has_multi(), "折行开启折叠多光标");
    c.set_word_wrap(false);

    // 折叠后主光标不被 select_all 破坏前的位置语义由 select_all 自身
    // 决定（doc 末尾）；此处只钉「附加集已清 + 光标落在新行数域内」
    assert!(!c.has_multi());
    assert!(c.cursor.line < c.doc.line_count());
}

#[test]
fn caret_rect_at_matches_relative_for_main_cursor() {
    let mut c = core_with("abcdef\nghijkl");
    c.set_viewport_height(400.0);
    c.set_viewport_width(800.0);
    c.cursor = CursorPos { line: 1, col: 3 };
    assert_eq!(
        c.caret_rect_at(c.cursor),
        c.caret_rect_relative(),
        "caret_rect_relative = caret_rect_at(主光标)"
    );
}

// ---------- B10 Phase 2：同步编辑（InsertText/Backspace/Delete） ----------

fn doc_text(c: &EditorCore) -> String {
    c.doc.slice_text(0, c.doc.text_len())
}

#[test]
fn multi_edit_inserts_at_all_cursors_single_snapshot() {
    let mut c = core_with("abc\nabc\nabc");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 3 }));
    assert!(c.toggle_extra_cursor(CursorPos { line: 2, col: 3 }));
    let snapshots_before = c.undo_stack.len();

    assert_eq!(c.multi_edit(MultiEditKind::Insert("X")), Some(true));
    assert_eq!(doc_text(&c), "abcX\nabcX\nabcX", "三点同步插入");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 4 }, "主光标落插入文本尾");
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![
            CursorPos { line: 1, col: 4 },
            CursorPos { line: 2, col: 4 },
        ],
        "附加光标各自落插入文本尾（位序不变量保持）"
    );
    assert_eq!(
        c.undo_stack.len(),
        snapshots_before + 1,
        "单快照：一次用户操作只入一帧"
    );

    // 一次撤销撤掉整步，且恢复完整多光标态（Snapshot 扩展字段）
    assert!(c.undo());
    assert_eq!(doc_text(&c), "abc\nabc\nabc");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 3 });
    assert_eq!(
        c.extra_cursors.iter().map(|e| e.cursor).collect::<Vec<_>>(),
        vec![
            CursorPos { line: 1, col: 3 },
            CursorPos { line: 2, col: 3 },
        ],
        "undo 恢复附加光标集"
    );
    assert!(c.redo());
    assert_eq!(doc_text(&c), "abcX\nabcX\nabcX", "redo 对称");
    assert_eq!(c.extra_cursors.len(), 2);
}

#[test]
fn multi_edit_selection_replace_backspace_delete_inline() {
    // 带选区插入 = 逐点替换选区（模拟 Ctrl+M 词选区后打字）
    let mut c = core_with("foo bar foo");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 3 };
    c.extra_cursors = vec![ExtraCursor {
        cursor: CursorPos { line: 0, col: 11 },
        anchor: Some(CursorPos { line: 0, col: 8 }),
    }];
    assert_eq!(c.multi_edit(MultiEditKind::Insert("XX")), Some(true));
    assert_eq!(doc_text(&c), "XX bar XX", "主/附光标选区同步替换");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 0, col: 9 });
    assert!(
        c.extra_cursors.iter().all(|e| e.anchor.is_none()),
        "编辑后各光标选区清空"
    );

    // 行内退格：两光标各删前一字符（从后往前应用互不干扰）
    let mut c = core_with("abcdef\nghijkl");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 3 }));
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), Some(true));
    assert_eq!(doc_text(&c), "abdef\nghjkl");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 1, col: 2 });

    // 行内前向删除
    let mut c = core_with("abcdef\nghijkl");
    c.cursor = CursorPos { line: 0, col: 0 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 0 }));
    assert_eq!(c.multi_edit(MultiEditKind::Delete), Some(true));
    assert_eq!(doc_text(&c), "bcdef\nhijkl");
}

#[test]
fn multi_edit_noop_variants_produce_no_snapshot() {
    // 主光标在文档原点退格 = no-op；其余光标照常参与
    let mut c = core_with("ab\ncd");
    c.cursor = CursorPos { line: 0, col: 0 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 2 }));
    let snapshots_before = c.undo_stack.len();
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), Some(true));
    assert_eq!(doc_text(&c), "ab\nc");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 0 }, "原点光标不动");
    assert_eq!(c.undo_stack.len(), snapshots_before + 1);

    // 空文本插入 = Some(false)，不产快照
    let mut c = core_with("ab\ncd");
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 1 }));
    let snapshots_before = c.undo_stack.len();
    assert_eq!(c.multi_edit(MultiEditKind::Insert("")), Some(false));
    assert_eq!(doc_text(&c), "ab\ncd");
    assert_eq!(c.undo_stack.len(), snapshots_before, "空文本 no-op 无快照");
}

#[test]
fn multi_edit_crossline_falls_back_to_single_cursor() {
    // 附加光标行首退格 = 跨行删除，一期约束 → 整批折叠返回 None
    let mut c = core_with("abcdef\nghijkl");
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 0 }));
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), None, "跨行回退");
    assert!(!c.has_multi(), "回退前折叠全部附加光标");

    // 主光标行尾 Delete（吞换行）同样触发回退
    let mut c = core_with("abcdef\nghijkl");
    c.cursor = CursorPos { line: 0, col: 6 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 2 }));
    assert_eq!(c.multi_edit(MultiEditKind::Delete), None);
    assert!(!c.has_multi());

    // 跨行选区插入同样回退（主光标选区跨行 + 附加光标在场）
    let mut c = core_with("abcdef\nghijkl");
    c.anchor = Some(CursorPos { line: 0, col: 2 });
    c.cursor = CursorPos { line: 1, col: 2 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 0 }));
    assert_eq!(c.multi_edit(MultiEditKind::Insert("X")), None);
    assert!(!c.has_multi());

    // 无多光标 = None（调用方走普通路径的信号）
    let mut c = core_with("abc");
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), None);
    assert!(!c.has_multi());
}

#[test]
fn multi_edit_multi_line_insert_shifts_bookmarks_below() {
    // 粘贴（多行文本）走 InsertText 同步管线：跨行插入平移下方书签
    let mut c = core_with("aa\nbb\ncc\ndd");
    c.bookmarks.insert(3); // "dd" 行书签
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 2 }));
    assert_eq!(c.multi_edit(MultiEditKind::Insert("\n")), Some(true));
    assert_eq!(doc_text(&c), "aa\n\nbb\n\ncc\ndd", "两点各插一个换行");
    assert!(
        c.bookmarks.contains(&5),
        "dd 行书签随两次插入累计下移 3→5"
    );
}

// ---------- B10 Phase 2：添加下一匹配（Ctrl+M） ----------

#[test]
fn add_next_match_word_cycle_skips_occupied() {
    let mut c = core_with("foo bar foo baz foo");
    c.cursor = CursorPos { line: 0, col: 0 }; // 光标在词 "foo" 上

    // 第 1 次：加 offset 8 的实例（词选区 anchor=8, cursor=11）
    assert_eq!(c.add_next_match(), Ok(true));
    assert_eq!(c.extra_cursors.len(), 1);
    assert_eq!(c.extra_cursors[0].anchor, Some(CursorPos { line: 0, col: 8 }));
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 0, col: 11 });

    // 第 2 次：offset 8 已被占用 → 跳过，加 offset 16
    assert_eq!(c.add_next_match(), Ok(true));
    assert_eq!(c.extra_cursors.len(), 2);
    assert_eq!(c.extra_cursors[1].anchor, Some(CursorPos { line: 0, col: 16 }));
    assert_eq!(c.extra_cursors[1].cursor, CursorPos { line: 0, col: 19 });

    // 第 3 次：无更多匹配 → Err 提示，集合不动
    assert_eq!(
        c.add_next_match(),
        Err("没有更多匹配".to_owned()),
        "环形一圈无匹配 = Err 提示"
    );
    assert_eq!(c.extra_cursors.len(), 2);
}

#[test]
fn add_next_match_selection_text_and_word_end() {
    // 有选区 = 以选区文本检索
    let mut c = core_with("ab cd ab cd");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 2 }; // 选区 "ab"
    assert_eq!(c.add_next_match(), Ok(true));
    assert_eq!(c.extra_cursors[0].anchor, Some(CursorPos { line: 0, col: 6 }));
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 0, col: 8 });

    // 光标在词尾（无选区）= 向左扩展取词；下一实例在后面
    let mut c = core_with("xx yy xx");
    c.cursor = CursorPos { line: 0, col: 2 }; // 词 "xx" 尾
    assert_eq!(c.add_next_match(), Ok(true));
    assert_eq!(c.extra_cursors[0].anchor, Some(CursorPos { line: 0, col: 6 }));
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 0, col: 8 });
}

#[test]
fn add_next_match_guards() {
    // 光标不在词上 → Err
    let mut c = core_with("foo , bar");
    c.cursor = CursorPos { line: 0, col: 5 }; // 空白处
    assert!(c.add_next_match().is_err_and(|m| m.contains("词")));

    // 折行开态拒绝（Ok(false) 静默）
    let mut c = core_with("foo foo");
    c.set_word_wrap(true);
    assert_eq!(c.add_next_match(), Ok(false));
    c.set_word_wrap(false);

    // 列块态拒绝
    c.begin_block_select(CursorPos { line: 0, col: 0 });
    c.update_block_select(CursorPos { line: 0, col: 1 });
    assert_eq!(c.add_next_match(), Ok(false));
    c.clear_block();

    // 组字态拒绝
    c.ime_preedit("拼音".to_owned());
    assert_eq!(c.add_next_match(), Ok(false));
}
