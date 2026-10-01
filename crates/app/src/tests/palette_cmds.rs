//! P316：无默认键的命令也要「在面板里看得见、点了要能动」。
//!
//! 三条判据各挡一种"注册了但坏掉"：
//! ① 每个注册 id 都必须有 dispatch 分支（既有的
//!    `hotkey_actions_all_dispatch_through_handle_key` 是**逐 default_combos**
//!    驱动的，空组合的 action 被它静默跳过 ⇒ 光靠那条，"面板里点不动"会一路绿）；
//! ② 无键动作要进面板且键位列显示 `—`；
//! ③ 三条翻转包装消息必须与菜单项等价（取反只有一处实现）。

use super::*;
use crate::hotkeys::{dispatch_action, palette_commands, HOTKEY_ACTIONS};
use iced::keyboard::Modifiers;

#[test]
fn every_registered_action_has_a_dispatch_arm() {
    // 含 13 条 `default_combos: &[]` 的新动作——不依赖组合串，直接按 id 打
    for action in HOTKEY_ACTIONS {
        assert!(
            dispatch_action(action.id, Modifiers::empty()).is_some(),
            "动作 {} 注册了但 dispatch_action 没有分支（面板里会点了没反应）",
            action.id
        );
    }
}

#[test]
fn unassigned_actions_are_visible_in_the_palette() {
    let cmds = palette_commands(editpad_core::Lang::ZhCn);
    let ids: Vec<&str> = cmds.iter().map(|c| c.id).collect();
    for id in [
        "save_as",
        "recents",
        "toggle_word_wrap",
        "toggle_whitespace",
        "toggle_line_endings",
        "toggle_theme",
        "markdown_preview",
        "open_settings",
        "toggle_backup_mode",
        "tab_wrap_override",
        "tab_font_reset",
        "mark_hit_lines",
        "copy_hit_lines",
        // P318 新增的无默认键动作，同一条契约
        "bookmark_panel",
        // P319 同等待遇：剪贴板历史先进面板，键位由用户自行赋
        "clip_history",
        // A7 同等待遇：跨标签全部替换（查找栏另有按钮，面板里也要看得见）
        "replace_all_in_tabs",
        // B10 二期首批（P327）：拆行多选。26 个 Ctrl+Shift 字母已占尽 ⇒ 先无键入表
        "split_selection_by_lines",
    ] {
        assert!(ids.contains(&id), "{id} 应出现在命令面板里");
    }
    // 无默认键 ⇒ 键位列给一个明确的占位，而不是空串。
    // P320 改判：这一列现在由 app 侧按**生效**键位算（数据源只出 id＋标题），
    // 所以判据也走真正渲染出来的条目，不再走 `palette_commands` 的中间结构。
    let app = Editpad::default();
    let entries = app.palette_all_entries();
    let wrap = entries
        .iter()
        .find(|e| e.command_id == Some("toggle_word_wrap"))
        .expect("面板应有「自动换行」");
    assert_eq!(wrap.detail, "—", "实得 {:?}", wrap.detail);
    assert!(!wrap.title.trim().is_empty(), "标题取自既有 Menu* 键");
}

/// P320：面板的键位列要跟着用户的重映射走。
///
/// 改前这一列来自 `default_combo_of`（只读注册表默认值），于是把 `Ctrl+S`
/// 改成 `F10` 之后面板仍写 `Ctrl+S`——而面板正是「忘了这招按什么」时看的地方。
#[test]
fn palette_key_column_shows_the_effective_combo_not_the_default() {
    let combo_of = |app: &Editpad, id: &'static str| {
        app.palette_all_entries()
            .into_iter()
            .find(|e| e.command_id == Some(id))
            .unwrap_or_else(|| panic!("命令面板里找不到动作 {id}"))
            .detail
    };
    let mut app = Editpad::default();
    assert_eq!(combo_of(&app, "save"), "Ctrl+S", "前提：未重映射时显默认键");
    // 同义默认键在窄列里只显首个（与设置页那一行的整串口径有意不同：
    // 显示密度不同，但「谁覆盖谁」只有一份实现）
    assert_eq!(
        combo_of(&app, "redo"),
        "Ctrl+Y",
        "实得 {:?}",
        combo_of(&app, "redo")
    );
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert_eq!(
        combo_of(&app, "save"),
        "F10",
        "重映射后面板必须说生效的那个键"
    );
    // 反向一格：非法组合提交失败 ⇒ 列上留住的仍是上一条生效值
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
    dispatch(&mut app, Message::HotkeyCaptureKey("alt+f4".into()));
    assert_eq!(
        combo_of(&app, "save"),
        "F10",
        "非法组合不该把键位列改回默认串"
    );
}

/// P321：面板里选中一条命令按 F4 = 直接开录键态（不必绕设置页）。
#[test]
fn f4_in_the_palette_starts_capturing_a_key_for_the_selected_command() {
    use iced::keyboard::{self, key::Named};
    let f4 = || {
        (
            keyboard::Key::Named(Named::F4),
            keyboard::Modifiers::empty(),
        )
    };
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::PaletteToggled(crate::state::PaletteMode::Commands),
    );
    assert!(app.palette_visible, "前提：面板开着");
    // 选中「书签列表」（P318 那条无默认键的动作）：按标题过滤出唯一一条
    dispatch(&mut app, Message::PaletteInputChanged("书签列表".into()));
    let picked = app
        .palette_selected_command_id()
        .expect("过滤后应选中一条命令");
    assert_eq!(picked, "bookmark_panel", "选中的应是这一条");
    dispatch(&mut app, Message::KeyPressed(f4().0, f4().1));
    assert_eq!(
        app.hotkey_capture,
        Some("bookmark_panel"),
        "F4 要把录键态开在这一条上"
    );
    assert!(!app.palette_visible, "面板让位给录键态");
    assert!(
        !app.status.is_empty(),
        "状态栏要说一句在等按键（复用设置页那条提示，别默默等）"
    );
    // 赋上 F10（注册表未占用的裸功能键）⇒ 三处一起变：映射、面板列、注册表默认键不变
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert_eq!(
        app.settings
            .hotkeys
            .get("bookmark_panel")
            .map(String::as_str),
        Some("F10")
    );
    assert!(app.hotkey_capture.is_none(), "提交后退出捕获态");
    dispatch(&mut app, Message::PaletteInputChanged("书签列表".into()));
    let entries = app.palette_all_entries();
    let row = entries
        .iter()
        .find(|e| e.command_id == Some("bookmark_panel"))
        .expect("面板里那条命令还在");
    assert_eq!(row.detail, "F10", "赋完键面板那一列要立刻跟上来");
}

/// P321：底部那行手势提示只在命令模式出现。
///
/// 视图树夹具读得到几何、读不到文本，所以钉的是这句裁决本身（与 P319 的
/// `palette_empty_label` 同一手法）——"哪个模式说哪句话"没有地方被盯，
/// 就会在某个模式下变成假提示。
#[test]
fn the_gesture_hint_line_appears_only_in_command_mode() {
    let mut app = Editpad::default();
    app.palette_mode = crate::state::PaletteMode::Commands;
    let hint = app.palette_hint_line().expect("命令模式要给一行手势提示");
    assert!(
        hint.contains("F4"),
        "提示里必须点出 F4，否则这个动作等于不存在：{hint}"
    );
    for mode in [
        crate::state::PaletteMode::Tabs,
        crate::state::PaletteMode::Clipboard,
    ] {
        app.palette_mode = mode;
        assert!(
            app.palette_hint_line().is_none(),
            "{mode:?} 模式没有「给命令赋键」这件事，不许写假提示"
        );
    }
}
/// P321 的另一头：选中的不是命令（标签行／空列表）时，F4 不许吞掉、也不许收起面板。
#[test]
fn f4_on_a_non_command_row_is_a_no_op_and_keeps_the_palette_open() {
    use iced::keyboard::{self, key::Named};
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::PaletteToggled(crate::state::PaletteMode::Tabs),
    );
    assert!(app.palette_visible);
    assert_eq!(
        app.palette_selected_command_id(),
        None,
        "标签模式没有命令 id（有页也无命令）"
    );
    dispatch(
        &mut app,
        Message::KeyPressed(
            keyboard::Key::Named(Named::F4),
            keyboard::Modifiers::empty(),
        ),
    );
    assert!(app.hotkey_capture.is_none(), "标签行不该进录键态");
    assert!(app.palette_visible, "无操作时面板留着，别让人觉得键坏了");

    // 空列表（查询串打到什么都不剩）同样是无操作
    let mut app2 = Editpad::default();
    dispatch(
        &mut app2,
        Message::PaletteToggled(crate::state::PaletteMode::Commands),
    );
    dispatch(
        &mut app2,
        Message::PaletteInputChanged("zzzzz不存在的查询".into()),
    );
    assert_eq!(
        app2.palette_selected_command_id(),
        None,
        "前提：过滤后一条不剩"
    );
    dispatch(
        &mut app2,
        Message::KeyPressed(
            keyboard::Key::Named(Named::F4),
            keyboard::Modifiers::empty(),
        ),
    );
    assert!(app2.hotkey_capture.is_none());
    assert!(app2.palette_visible);
}

#[test]
fn toggle_word_wrap_wrapper_flips_both_ways() {
    let mut app = Editpad::default();
    let before = app.settings.word_wrap;
    dispatch(&mut app, Message::ToggleWordWrap);
    assert_ne!(app.settings.word_wrap, before, "应翻转现值");
    dispatch(&mut app, Message::ToggleWordWrap);
    assert_eq!(app.settings.word_wrap, before, "再翻一次回到原值");
}

#[test]
fn whitespace_and_line_ending_wrappers_flip_their_own_flag() {
    let mut app = Editpad::default();
    let (ws, eol) = (app.settings.show_whitespace, app.settings.show_line_endings);
    // 两条各自只翻转自己那一项——共用的取反包装若写错字段，这里当场对不上
    dispatch(&mut app, Message::ToggleWhitespace);
    assert_ne!(app.settings.show_whitespace, ws);
    assert_eq!(app.settings.show_line_endings, eol, "不该连带改动另一项");
    dispatch(&mut app, Message::ToggleLineEndings);
    assert_ne!(app.settings.show_line_endings, eol);
    assert_ne!(app.settings.show_whitespace, ws, "另一项要停在已翻转的态");
}
