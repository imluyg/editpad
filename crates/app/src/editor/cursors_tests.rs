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
