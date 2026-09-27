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
    ] {
        assert!(ids.contains(&id), "{id} 应出现在命令面板里");
    }
    // 无默认键 ⇒ 键位列给一个明确的占位，而不是空串
    let wrap = cmds
        .iter()
        .find(|c| c.id == "toggle_word_wrap")
        .expect("面板应有「自动换行」");
    assert_eq!(wrap.detail, "—", "实得 {:?}", wrap.detail);
    assert!(!wrap.title.trim().is_empty(), "标题取自既有 Menu* 键");
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
