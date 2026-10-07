//! P308：正文右键菜单，以及 P307 连击选择在**真实视图树 + 真实事件流**上的
//! 接线核对。
//!
//! core 层的行为已在 `editor::click_tests` 钉住（20 条）；这里只补那条覆盖
//! 不到的东西：控件层按下分支的顺序（连击裁决先于拖拽候选）与右键入口。
//! 判据刻意取「选中了整词」而非「发过某条消息」——分支顺序错一次，
//! 双击就会被拖拽候选吃掉，本文件即红。
//! B10 二期（Alt+双击带选区）同样只在这里核对接线：core 层盯算式，这里盯
//! 修饰键状态真能穿过按下分支。

use super::*;

/// 正文里按生产入口写入一段文本（不直接改 doc，走应用层同一条链路）。
fn type_into(app: &mut Editpad, text: &str) {
    dispatch(
        app,
        Message::Edit(crate::editor::EditOp::InsertText(text.to_owned())),
    );
}

#[test]
fn headless_single_click_selects_nothing() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.click(at);
    }
    assert_eq!(app.cur_handle.borrow().anchor, None, "单击仍是定位光标");
    assert_eq!(app.cur_handle.borrow().selected_text(), None);
}

#[test]
fn headless_double_click_in_body_selects_a_whole_word() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let msgs = ui.double_click(at);
        assert!(
            msgs.iter().any(|m| matches!(m, Message::EditorNavChanged)),
            "双击应发导航变更消息（状态栏与查找联动）；实收 {msgs:?}"
        );
    }
    let sel = app.cur_handle.borrow().selected_text();
    assert!(
        matches!(sel.as_deref(), Some("alpha") | Some("beta")),
        "双击应选中整词，实得 {sel:?}"
    );
}

#[test]
fn headless_triple_click_in_body_selects_the_line() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta\ngamma");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.click(at);
        let _ = ui.click(at);
        let _ = ui.click(at);
    }
    let sel = app.cur_handle.borrow().selected_text();
    assert_eq!(sel.as_deref(), Some("alpha beta\n"), "三击 = 整行含换行");
}

#[test]
fn headless_right_click_in_body_opens_menu_at_pointer() {
    let app = Editpad::default();
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.4, body.y + 20.0)
    };
    let msgs = {
        let mut ui = ViewTree::layout_default(&app);
        ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Right,
            )),
            at,
        )
    };
    assert!(
        msgs.iter()
            .any(|m| matches!(m, Message::EditorContextMenu(x, y) if (*x, *y) == (at.x, at.y))),
        "右键应按指针位置弹正文菜单；实收 {msgs:?}"
    );
}

#[test]
fn right_click_outside_body_opens_nothing() {
    let app = Editpad::default();
    let msgs = {
        let mut ui = ViewTree::layout_default(&app);
        ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                iced::mouse::Button::Right,
            )),
            Point::new(4.0, 4.0),
        )
    };
    assert!(
        !msgs
            .iter()
            .any(|m| matches!(m, Message::EditorContextMenu(..))),
        "菜单栏条带上的右键不该被正文菜单接管；实收 {msgs:?}"
    );
}

#[test]
fn opening_the_menu_is_pure_navigation() {
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    let dirty_before = app.tab().dirty;
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    assert!(app.editor_context_menu, "菜单已开");
    assert_eq!(app.menu_anchor, (300.0, 240.0));
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "one\ntwo\n");
    assert_eq!(
        app.tab().dirty,
        dirty_before,
        "开菜单本身不改内容、不额外置脏"
    );
}

#[test]
fn editor_and_tab_context_menus_are_mutually_exclusive() {
    let mut app = Editpad::default();
    app.tab_context_menu = Some(0);
    dispatch(&mut app, Message::EditorContextMenu(10.0, 20.0));
    assert_eq!(app.tab_context_menu, None, "两层背板会互相吞事件");
    dispatch(&mut app, Message::TabContextMenu(0));
    assert!(!app.editor_context_menu, "反向同理");
}

#[test]
fn menu_item_runs_the_action_and_closes() {
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    // 插入后光标在文末（幻影末行），先把要删的那一行变成首行
    dispatch(&mut app, Message::Edit(crate::editor::EditOp::SelectAll));
    dispatch(
        &mut app,
        Message::Edit(crate::editor::EditOp::Motion(
            crate::editor::Motion::DocStart,
            false,
        )),
    );
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::DeleteLines))),
    );
    assert!(!app.editor_context_menu, "菜单项执行完即收起");
    assert_eq!(app.cur_handle.borrow().doc.to_text(), "two\n");
    assert!(app.tab().dirty, "经菜单删行同样置脏");
}

#[test]
fn menu_is_not_a_new_bypass_for_read_only_pages() {
    let mut app = Editpad::default();
    type_into(&mut app, "keep me\n");
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "用例前提：已切只读");
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::DeleteLines))),
    );
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "keep me\n",
        "只读页仍由 apply_edit 总闸拒收——菜单只是入口，不是放行口"
    );
}

#[test]
fn stray_command_after_close_does_not_reopen_menu() {
    // 「点菜单外」与「点某项」同一帧只会命中其一；这里断的是状态自洽——
    // 转交仍会发生（收起菜单不等于拒收动作），但绝不把菜单再打开
    let mut app = Editpad::default();
    type_into(&mut app, "one\ntwo\n");
    dispatch(&mut app, Message::EditorContextMenuClosed);
    assert!(!app.editor_context_menu);
    dispatch(
        &mut app,
        Message::EditorCtxCommand(Box::new(Message::Edit(crate::editor::EditOp::SelectAll))),
    );
    assert_eq!(
        app.cur_handle.borrow().selected_text().as_deref(),
        Some("one\ntwo\n"),
        "转交照常执行"
    );
    assert!(!app.editor_context_menu, "执行菜单项不该把菜单再打开");
}

/// B10 二期「选区镜像同步」的**接线**核对：core 层那五格盯的是算式，这一格盯
/// 鼠标事件真能走到 Alt+双击那条分支。变异＝视图恒传 `ExtraTap::Point`
/// ⇒ core 层全绿、只有这里红（第 226 轮那条"注册了但点不动"的护栏同形）。
#[test]
fn headless_alt_double_click_adds_extra_cursor_owning_a_selection() {
    let mut app = Editpad::default();
    type_into(&mut app, "alpha beta");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 10.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.send(
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
                iced::keyboard::Modifiers::ALT,
            )),
            at,
        );
        // 第一次按下＝点（加一条无选区光标），第二次＝双击（该词连选区进来）
        let msgs = ui.double_click(at);
        assert!(
            msgs.iter().any(|m| matches!(m, Message::EditorNavChanged)),
            "Alt+双击应发导航变更消息（状态栏与查找联动）；实收 {msgs:?}"
        );
    }
    let core = app.cur_handle.borrow();
    assert_eq!(core.extra_cursors.len(), 1, "Alt+双击后应恰有一条附加光标");
    assert!(
        core.extra_cursors[0].anchor.is_some(),
        "附加光标必须持有选区——这正是本轮补的那一格"
    );
    assert_eq!(core.doc.to_text(), "alpha beta", "手势本身一字不改文档");
}

/// 池 ⑨：Alt+Shift 建块那条分支必须**先于** dnd 候选裁决。视图层的注释本来就写着
/// 「Alt+Shift 仍走建块；先于 dnd 候选」，实际次序却排在 dnd 之后 ⇒ 在已选中的文本上
/// 做这个手势时，dnd 先把按下拦成拖拽候选：拉开是真的把那段文本搬走，原地松手则选区
/// 被吞、光标落到按下点。core 层的建块算式有单测，这一格盯的是鼠标事件真能走到那条分支。
#[test]
fn headless_alt_shift_drag_inside_a_selection_builds_a_column_block() {
    let mut app = Editpad::default();
    type_into(
        &mut app,
        "alpha beta\ncharlie delta\necho foxtrot\ngolf hotel\nindia juliet\nkilo lima",
    );
    let press = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + 8.0)
    };
    // 夹具不自填 anchor：先用一次真实双击造出选区
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.double_click(press);
    }
    {
        let core = app.cur_handle.borrow();
        assert!(
            core.selected_text().is_some(),
            "夹具自证：此刻真有一段选区（dnd 候选的前提成立）"
        );
        assert!(core.dnd.is_none(), "夹具自证：双击本身没起拖拽会话");
    }

    // 同一处按 Alt+Shift 按下、往下拖一行以上、松开——应当得到列块
    let lower = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.3, body.y + body.height * 0.6)
    };
    let mods_alt_shift = iced::keyboard::Modifiers::ALT | iced::keyboard::Modifiers::SHIFT;
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.send(
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods_alt_shift)),
            press,
        );
        let _ = ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            press,
        );
        // 裁决就在这一刻分岔：改前按下被 dnd 拦成拖拽候选，建块分支轮不到。
        // 此刻块还是「anchor == head」的空块，`has_block()` 要到拖动之后才成立，
        // 所以这一格盯的是**谁接住了这次按下**。
        {
            let core = app.cur_handle.borrow();
            assert!(
                core.dnd.is_none(),
                "已选文本上的 Alt+Shift 按下不该被 dnd 候选拦下（改前正是这一格）"
            );
            assert!(
                core.block_dragging,
                "这次按下应交给列块拖拽建块（`begin_block_select`）"
            );
        }
        let _ = ui.send(
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: lower }),
            lower,
        );
        let _ = ui.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
            lower,
        );
    }
    let core = app.cur_handle.borrow();
    let block = core
        .active_block()
        .expect("向下拖动并松开后应留下有效的列块");
    assert_eq!(block.0, 0, "块上缘=按下那一行（读到 {block:?}）");
    assert!(
        block.1 > block.0,
        "块应跨到下面那行（本次读到行 {block:?}）"
    );
    assert_eq!(
        core.doc.to_text(),
        "alpha beta\ncharlie delta\necho foxtrot\ngolf hotel\nindia juliet\nkilo lima",
        "列块是选区态，一个字都不该动"
    );
    assert!(core.dnd.is_none(), "收尾也不该留拖拽会话");
}

/// 池 ⑨ 的兄弟格：本轮删掉了 P307 那块里 `if core.mods.alt() { click_last = None }`
/// 这一 arm（改后 Alt 与 Alt+Shift 都在更前面就返回，它永不可达）。这一格钉住**它曾经
/// 排在外面的那条 Shift+点击**没被误伤——修饰键裁决的顺序改动最容易被邻居带伤。
#[test]
fn headless_shift_click_still_extends_the_selection() {
    let mut app = Editpad::default();
    // 一行足够长的正文：0.9 宽度的落点仍在行内（行短的话会被钳到行尾，
    // 扩展量读成 0、这格就变成假红）
    type_into(&mut app, "alpha beta charlie delta echo foxtrot golf hotel");
    let at = {
        let body = ViewTree::layout_default(&app).editor_body();
        // 落在第一个词上（0.3 那一档在这台机器的度量下正好命中间隔空格，
        // 自证就退化成"选到一个空白"）
        Point::new(body.x + body.width * 0.12, body.y + 8.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.double_click(at);
    }
    let word = app.cur_handle.borrow().selected_text().unwrap_or_default();
    assert!(
        !word.is_empty() && word.chars().all(|c| c.is_alphabetic()),
        "夹具自证：双击先造出一段**词**选区（读到 {word:?}）"
    );

    let far = {
        let body = ViewTree::layout_default(&app).editor_body();
        Point::new(body.x + body.width * 0.9, body.y + 8.0)
    };
    {
        let mut ui = ViewTree::layout_default(&app);
        let _ = ui.send(
            iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(
                iced::keyboard::Modifiers::SHIFT,
            )),
            far,
        );
        let _ = ui.click(far);
    }
    let core = app.cur_handle.borrow();
    let sel = core.selected_text().unwrap_or_default();
    assert!(
        sel.starts_with(word.as_str()) && sel.chars().count() > word.chars().count(),
        "Shift+点击应把那段词选区向右扩展到按下点，实得 {sel:?}（原本 {word:?}）"
    );
    assert!(!core.has_block(), "Shift+点击不建列块");
}

/// P336：两类右键菜单都绑在「打开它那一刻的那一页」上——正文菜单锚在旧页坐标、
/// 菜单项动的却是新页；标签菜单带的是旧页下标。作废原本由三条切页路径各手写一句
/// （只收标签菜单），`FifGoto` 跳页与装载那条路连这一句都没有，正文菜单则全程没人
/// 收 ⇒ 现收进 `set_active_tab` 一处，两条入口都钉住。
#[test]
fn switching_pages_closes_both_context_menus() {
    use crate::find_scan::{FileHit, FileHits};

    let load = |app: &mut Editpad, name: &'static str, body: &'static str| {
        dispatch(app, Message::FileDropped(PathBuf::from(name)));
        let seq = app.job_seq;
        dispatch(
            app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str(body),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
    };

    let mut app = Editpad::default();
    load(&mut app, "C:/doc/a.txt", "a1\na2\n");
    load(&mut app, "C:/doc/b.txt", "b1\n");
    assert_eq!(app.tabs.len(), 2, "夹具自证：两页都带上了路径");
    assert_eq!(app.active_tab, 1, "夹具自证：装载完停在 B 页");

    // 回 A 页，两类菜单都在场
    app.set_active_tab(0);
    dispatch(&mut app, Message::EditorContextMenu(300.0, 240.0));
    app.tab_context_menu = Some(0);
    assert!(app.editor_context_menu, "夹具自证：正文菜单已开");

    // 入口一：键盘切页（Ctrl+Tab → SwitchTabNext）
    dispatch(&mut app, Message::SwitchTabNext);
    assert_eq!(app.active_tab, 1, "夹具自证：切到了 B 页");
    assert!(
        !app.editor_context_menu,
        "正文菜单锚在 A 页坐标上，切页必须收起"
    );
    assert_eq!(
        app.tab_context_menu, None,
        "标签菜单带的是 A 页下标，切页必须收起"
    );

    // 入口二：从「在文件中查找」结果面板跳进另一个已开页（原先不收菜单）
    dispatch(&mut app, Message::EditorContextMenu(10.0, 20.0));
    app.tab_context_menu = Some(1);
    app.fif_results = vec![FileHits {
        path: PathBuf::from("C:/doc/a.txt"),
        hits: vec![FileHit {
            pos: editpad_core::MatchPos {
                line: 0,
                col: 0,
                len_chars: 2,
            },
            excerpt: "a1".into(),
        }],
    }];
    dispatch(&mut app, Message::FifGoto(0, 0));
    assert_eq!(app.active_tab, 0, "夹具自证：跳页生效，回到 A 页");
    assert!(
        !app.editor_context_menu,
        "跳页同样要收起在 B 页开着的正文菜单"
    );
    assert_eq!(
        app.tab_context_menu, None,
        "跳页这条路径原先连标签菜单都不收"
    );
}

/// P356（第 261 轮扫描池 ⑤）：**每一类**带整窗背板的浮层，在**每一个**"必须让路"的
/// 入口都得收起。
///
/// 形状＝"清单只写了一半"：`enter_busy` 收三类、唯独漏后来才加的正文右键菜单（P308）
/// ⇒ 菜单挂着时按 `Ctrl+O`，busy 的透明背板与菜单背板叠着挡整窗点击，busy 结束后菜单
/// 还悬在**旧坐标**上、内容还是旧页那组命令。这条收口的存在理由（P39）是**背板挡点击**，
/// 不是"数据属于哪一页"——所以判据是"有没有整窗背板"，四类全中。
/// 两个入口都钉：`enter_busy` 与 Esc 的统一收起出口（`dismiss_all_prompts`）。
/// ⚠️ **切页那一列故意不钉**：P336 已裁决状态栏编码／行尾菜单**不带页专属数据**
/// （目标就是"当下活动页"），切页时不收＝既定语义，不是漏做；两类右键菜单在切页那
/// 一列由既有用例 `switching_pages_closes_both_context_menus` 钉着。
#[test]
fn every_dismissal_entry_closes_every_full_window_backdrop_overlay() {
    // 表驱动的三份类型别名（clippy 的 `type_complexity` 在提醒：这条矩阵的类型确实长）
    type Opener = fn(&mut Editpad);
    type Probe = fn(&Editpad) -> bool;
    type Overlay = (&'static str, Opener, Probe);
    type Entry = (&'static str, Opener);

    // 四个浮层：(名字, 打开它的探针, 读它的探针)
    let overlays: Vec<Overlay> = vec![
        (
            "正文右键菜单",
            |a| {
                dispatch(a, Message::EditorContextMenu(120.0, 90.0));
            },
            |a| a.editor_context_menu,
        ),
        (
            "标签右键菜单",
            |a| {
                dispatch(a, Message::TabContextMenu(0));
            },
            |a| a.tab_context_menu.is_some(),
        ),
        (
            "状态栏编码菜单",
            |a| {
                dispatch(a, Message::ToggleEncodingMenu);
            },
            |a| a.encoding_menu,
        ),
        (
            "状态栏行尾菜单",
            |a| {
                dispatch(a, Message::ToggleEolMenu);
            },
            |a| a.eol_menu,
        ),
    ];
    // 两个"必须让路"的入口：进 busy / Esc 统一收起
    let entries: Vec<Entry> = vec![
        ("进 busy（打开对话框）", |a| {
            dispatch(a, Message::OpenRequested);
        }),
        ("Esc（统一收起出口）", |a| {
            dispatch(a, Message::BarsDismissed);
        }),
    ];

    for (oname, open, visible) in &overlays {
        for (ename, entry) in &entries {
            let mut app = Editpad::default();
            open(&mut app);
            assert!(
                visible(&app),
                "夹具自证：{oname} 本来就开得起来（入口＝{ename}）"
            );
            entry(&mut app);
            assert!(
                !visible(&app),
                "入口「{ename}」之后 {oname}必须收起——它的透明背板会挡住整窗点击，\
                 而菜单还悬在打开它那一刻的坐标上"
            );
        }
    }
    // 反空转自证：进 busy 那条入口确实把 busy 置起来了（否则上面 4 格红的是别的原因）
    let mut probe = Editpad::default();
    dispatch(&mut probe, Message::OpenRequested);
    assert!(probe.busy, "夹具自证：OpenRequested 真的走 enter_busy");
}
