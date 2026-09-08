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

// ---------- B10 Phase 3：边界批 ----------

#[test]
fn multi_edit_boundary_extra_cursor_inside_main_selection_folds() {
    // 附加光标落在主选区内部 = 区间交叠 → 整批折叠回退
    let mut c = core_with("abcdef");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    assert_eq!(c.multi_edit(MultiEditKind::Insert("X")), None);
    assert!(!c.has_multi(), "交叠区间折叠回退");

    // 同起点一宽一零（附加光标恰在选区起点）：退化情形同样回退
    let mut c = core_with("abcdef");
    c.anchor = Some(CursorPos { line: 0, col: 0 });
    c.cursor = CursorPos { line: 0, col: 3 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 0 }));
    assert_eq!(c.multi_edit(MultiEditKind::Delete), None);
    assert!(!c.has_multi());
}

#[test]
fn multi_edit_boundary_doc_end_and_empty_document() {
    // 文末光标 Delete = no-op，其余光标照常
    let mut c = core_with("abc\ndefg");
    c.cursor = CursorPos { line: 1, col: 4 }; // 文档末尾
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    assert_eq!(c.multi_edit(MultiEditKind::Delete), Some(true));
    assert_eq!(doc_text(&c), "ac\ndefg");
    assert_eq!(c.cursor, CursorPos { line: 1, col: 4 }, "文末光标不动");

    // 空文档：附加光标无处可加（主光标位拒），多光标不可达；
    // multi_edit 短路 None、add_next_match Err
    let mut c = core_with("");
    assert!(!c.toggle_extra_cursor(CursorPos { line: 0, col: 0 }));
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), None);
    assert!(c.add_next_match().is_err());

    // 空文档插入（无多光标恒等）：单光标路径不受影响
    c.insert_str("hi");
    assert_eq!(doc_text(&c), "hi");
}

#[test]
fn multi_edit_boundary_single_line_document() {
    // 单行文档双光标：插入/退格/删除全行内语义
    let mut c = core_with("abcdef");
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 5 }));
    assert_eq!(c.multi_edit(MultiEditKind::Insert("-")), Some(true));
    assert_eq!(doc_text(&c), "ab-cde-f");
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), Some(true));
    assert_eq!(doc_text(&c), "abcdef", "两处 '-' 各删一个");
    assert_eq!(c.multi_edit(MultiEditKind::Delete), Some(true));
    assert_eq!(doc_text(&c), "abde", "两点各删一个（f 与 c）");
}

#[test]
fn multi_edit_boundary_crlf_normalization_and_inline_delete() {
    // CRLF 文档：插入 payload 按主导行尾归一；行内退格不吃行尾 \r
    let mut c = core_with("ab\r\ncd");
    assert_eq!(c.doc.line_ending(), LineEnding::CrLf);
    c.cursor = CursorPos { line: 0, col: 1 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 1 }));
    assert_eq!(c.multi_edit(MultiEditKind::Insert("X\nY")), Some(true));
    assert_eq!(
        doc_text(&c),
        "aX\r\nYb\r\ncX\r\nYd",
        "payload 的 \\n 归一为 CRLF，两点同步插入"
    );
    // 光标落在插入文本尾（跨 CRLF 计量）：主光标 (1,1)、附加 (3,1)
    // ——payload "X\r\nY" 自身跨两行，附加点在 line2 中部插入后其尾
    // 落在新增的 line3
    assert_eq!(c.cursor, CursorPos { line: 1, col: 1 });
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 3, col: 1 });

    // 行内退格：删 'b'/'d'，不触碰行尾 \r\n
    let mut c = core_with("ab\r\ncd");
    c.cursor = CursorPos { line: 0, col: 2 };
    assert!(c.toggle_extra_cursor(CursorPos { line: 1, col: 2 }));
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), Some(true));
    assert_eq!(doc_text(&c), "a\r\nc", "行内删除不吞换行单元");
}

#[test]
fn multi_edit_boundary_phantom_last_line_backspace_folds() {
    // 尾随换行的幻影末行：主光标 (1,0) 退格 = 跨行并行 → 折叠回退
    let mut c = core_with("ab\r\n");
    c.cursor = CursorPos { line: 1, col: 0 }; // 幻影行
    assert!(c.toggle_extra_cursor(CursorPos { line: 0, col: 1 }));
    assert_eq!(c.multi_edit(MultiEditKind::Backspace), None);
    assert!(!c.has_multi());
    assert_eq!(doc_text(&c), "ab\r\n", "回退不动文档");
}

#[test]
fn multi_edit_boundary_same_start_zero_width_inserts() {
    // 双零宽插入同点（同步编辑落点并位的退化态）：合法、落点按应用
    // 次序修正——后应用的 payload 在下、光标落其尾
    let mut c = core_with("ab");
    c.cursor = CursorPos { line: 0, col: 1 };
    c.extra_cursors = vec![ExtraCursor {
        cursor: CursorPos { line: 0, col: 1 },
        anchor: None,
    }];
    assert_eq!(c.multi_edit(MultiEditKind::Insert("XY")), Some(true));
    assert_eq!(doc_text(&c), "aXYXYb", "同点两次插入：后应用者在下");
    assert_eq!(c.cursor, CursorPos { line: 0, col: 5 }, "主光标（先应用）落上方 payload 尾");
    assert_eq!(c.extra_cursors[0].cursor, CursorPos { line: 0, col: 3 });
}

// ---------- B10 Phase 3：随机对拍（多光标同步编辑 vs 字符模型 oracle） ----------

/// 字符向量 + 光标偏移的独立模型（不经 multi_edit 任何代码路径）：
/// 同一操作序列在模型上按「偏移降序逐点 splice」重放，逐步与 core
/// 逐字节对拍，收尾撤销到底回初始 + 重放终态一致（P86/P138 骨架）。
#[test]
fn random_multi_cursor_sync_edit_matches_char_model_oracle() {
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
    const CHARS: &[char] = &['a', 'b', 'c', 'd', 'e', ' '];
    // ropey 行界口径切行（本地副本：LF 文档，不含 \r 分支）
    fn lf_line_bodies(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for ch in s.chars() {
            if ch == '\n' {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(ch);
            }
        }
        out.push(cur);
        out
    }
    // 模型偏移 → (行, 列)：行 = 前置 '\n' 数，列 = 行内偏移
    let to_pos = |text: &[char], off: usize| -> CursorPos {
        let line = text[..off].iter().filter(|&&c| c == '\n').count();
        let line_start = text[..off].iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1);
        CursorPos { line, col: off - line_start }
    };
    for seed in [7u64, 0xB10_C0DE, 0x5EED_9999] {
        let mut rng = XorShift64(seed);
        // LF 纯净文档（P86 先例）：3~8 行，每行 0~10 个字母/空格
        let rows = rng.below(6) + 3;
        let mut init = String::new();
        for r in 0..rows {
            for _ in 0..rng.below(11) {
                init.push(CHARS[rng.below(CHARS.len())]);
            }
            if r + 1 < rows {
                init.push('\n');
            }
        }
        let initial = init.clone();
        let mut c = core_with(&init);
        let mut text: Vec<char> = init.chars().collect();

        // 放置主光标 + 2~3 附加：各占一行、列 ∈ [1, 行长]（退格合法），
        // 位置两两不同。行数不足可用（全空行）时放满即止。
        let usable: Vec<usize> =
            lf_line_bodies(&init).iter().enumerate().filter(|(_, b)| !b.is_empty()).map(|(i, _)| i).collect();
        assert!(usable.len() >= 2, "种子 {seed} 初始文档应有可用行");
        let bodies = lf_line_bodies(&init);
        let mut placed: Vec<(usize, usize)> = Vec::new();
        let main_line = usable[rng.below(usable.len())];
        let main_col = rng.below(bodies[main_line].len()) + 1;
        placed.push((main_line, main_col));
        c.cursor = CursorPos { line: main_line, col: main_col };
        for _ in 0..2 + rng.below(2) {
            let line = usable[rng.below(usable.len())];
            let col = rng.below(bodies[line].len()) + 1;
            if placed.contains(&(line, col)) {
                continue;
            }
            if c.toggle_extra_cursor(CursorPos { line, col }) {
                placed.push((line, col));
            }
        }
        let mut init_extras: Vec<(usize, usize)> = placed[1..].to_vec();
        init_extras.sort_unstable(); // 附加集恒按 (line,col) 位序
        let init_main = placed[0];
        // 模型光标偏移（主在首位）；初始全序与 (line,col) 序一致
        let line_starts: Vec<usize> = {
            let mut v = vec![0usize];
            for (i, ch) in text.iter().enumerate() {
                if *ch == '\n' {
                    v.push(i + 1);
                }
            }
            v
        };
        let off_of = |p: (usize, usize)| line_starts[p.0] + p.1;
        let mut cursors: Vec<usize> = placed.iter().map(|p| off_of(*p)).collect();
        cursors.sort_unstable(); // 降序应用用；主光标身份由 init_cursors 对齐

        let steps = 120;
        for step in 0..steps {
            // 操作选择（门控不满足时退化为插入）：
            // - 退格要求全体光标列 ≥ 1（模型：前驱字符非 '\n' 且非文档头）
            // - 删除要求全体光标处有非换行字符（模型：off < len 且非 '\n'）
            // - 两者都要求无相邻光标：相邻双退格/双删除会并位（core 侧
            //   附加光标去重、模型两侧数量失配），属构造外退化，门控避开
            let occupied: std::collections::HashSet<usize> =
                cursors.iter().copied().collect();
            let non_adjacent = !cursors.iter().any(|&o| occupied.contains(&(o - 1)));
            let back_ok = non_adjacent
                && cursors.iter().all(|&o| o > 0 && text[o - 1] != '\n');
            let del_ok = non_adjacent
                && cursors
                    .iter()
                    .all(|&o| o < text.len() && text[o] != '\n');
            let pick = rng.below(100);
            let mut payload = String::new();
            payload.push(CHARS[rng.below(CHARS.len())]);
            if rng.below(5) == 0 {
                payload.push('\n'); // 偶发跨行插入（书签不存在，专注行结构）
            }
            let (is_ins, is_back) = if pick < 50 || (!back_ok && !del_ok) {
                (true, false)
            } else if pick < 75 && back_ok {
                (false, true)
            } else if del_ok {
                (false, false)
            } else {
                (false, true)
            };

            // core 执行（multi_edit 按值收 kind，此处为最后一次使用）
            let kind = if is_ins {
                MultiEditKind::Insert(payload.as_str())
            } else if is_back {
                MultiEditKind::Backspace
            } else {
                MultiEditKind::Delete
            };
            let r = c.multi_edit(kind);
            assert!(matches!(r, Some(true)), "seed={seed} step={step} 应为实编辑");

            // oracle 重放：偏移降序逐点 splice。门控已保证全体行内合法；
            // payload 归一在 LF 文档为恒等
            let pchars: Vec<char> = if is_ins {
                payload.chars().collect()
            } else {
                Vec::new()
            };
            let mut ops: Vec<usize> = cursors.clone();
            ops.sort_unstable_by_key(|o| std::cmp::Reverse(*o));
            for &o in &ops {
                if is_ins {
                    text.splice(o..o, pchars.iter().copied());
                } else if is_back {
                    text.remove(o - 1);
                } else {
                    text.remove(o);
                }
            }
            // 光标落点：自身调整（插入落 payload 尾 / 退格左移一）+
            // 更小偏移操作的净变化平移（同 kind ⇒ delta 一致：
            // 插入 +len，退格/删除各 -1——删除平移的是删除点之后的
            // 偏移，含自身不变、他点 -1，与 core 落点修正同式）
            let own: isize = if is_ins {
                pchars.len() as isize
            } else if is_back {
                -1
            } else {
                0
            };
            let delta: isize = if is_ins { pchars.len() as isize } else { -1 };
            let old: Vec<usize> = cursors.clone();
            for (i, o) in cursors.iter_mut().enumerate() {
                let shift: isize =
                    old.iter().filter(|&&o2| o2 < old[i]).count() as isize * delta;
                *o = (*o as isize + own + shift) as usize;
            }

            // 逐步对拍：文档逐字节 + 光标全集中（主 + 附加，按位序）
            let core_text = c.doc.to_text();
            let oracle_text: String = text.iter().collect();
            assert_eq!(core_text, oracle_text, "seed={seed} step={step} 文本发散");
            let mut core_pos: Vec<CursorPos> = c
                .extra_cursors
                .iter()
                .map(|e| e.cursor)
                .chain(std::iter::once(c.cursor))
                .collect();
            core_pos.sort_by_key(|p| (p.line, p.col));
            let mut oracle_pos: Vec<CursorPos> =
                cursors.iter().map(|o| to_pos(&text, *o)).collect();
            oracle_pos.sort_by_key(|p| (p.line, p.col));
            assert_eq!(core_pos, oracle_pos, "seed={seed} step={step} 光标发散");
        }

        // 撤销到底回初始（含完整多光标态），重放回终态
        let final_text = c.doc.to_text();
        let final_main = c.cursor;
        let final_extra = c.extra_cursors.clone();
        for _ in 0..steps {
            assert!(c.undo(), "seed={seed} 撤销栈应足够");
        }
        assert_eq!(c.doc.to_text(), initial, "seed={seed} 撤销到底未回初始");
        assert_eq!(
            c.extra_cursors
                .iter()
                .map(|e| (e.cursor.line, e.cursor.col))
                .collect::<Vec<_>>(),
            init_extras.clone(),
            "seed={seed} 撤销到底附加光标未回初始"
        );
        assert_eq!(
            (c.cursor.line, c.cursor.col),
            init_main,
            "seed={seed} 撤销到底主光标未回初始"
        );
        for _ in 0..steps {
            assert!(c.redo(), "seed={seed} 重放栈应足够");
        }
        assert_eq!(c.doc.to_text(), final_text, "seed={seed} 重放终态发散");
        assert_eq!(c.cursor, final_main);
        assert_eq!(c.extra_cursors, final_extra);
    }
}
