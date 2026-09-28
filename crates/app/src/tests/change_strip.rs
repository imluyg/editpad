//! B12（功能队列⑤）：变更历史行边条的应用层格子。
//!
//! 数据侧（谁是"改过的行"、缓存怎么失效、成本上限）在
//! [`crate::editor::change_strip`] 里钉；绘制侧在 `editor::view::tests` 的
//! `headless_change_strip_ink_sits_on_changed_rows_only` 里钉。这一份管的是
//! **接线**：开关能不能真的落到每一页、新开的页有没有跟上、设置页有没有行有控件、
//! 两向落不落盘。这几格本仓都各踩过一次（"重启后参考线失效但设置仍显示开启"）。

use super::*;

/// 三页应用，每页内容相同（基线＝加载态，之后各自改动互不串味）。
fn app_with_three_pages() -> Editpad {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::NewTab);
    dispatch(&mut app, Message::NewTab);
    app
}

#[test]
fn toggling_the_strip_reaches_every_open_page() {
    // 与缩进参考线同一条契约：全标签页即时生效，不是"只动当前页"
    let mut app = app_with_three_pages();
    assert_eq!(app.tabs.len(), 3, "夹具自证：三页");
    dispatch(&mut app, Message::SettingsChangeStripToggled(false));
    for (i, tab) in app.tabs.iter().enumerate() {
        assert!(
            !tab.editor.borrow().change_strip,
            "第 {i} 页没跟着关（当前页之外的页最容易漏）"
        );
    }
    dispatch(&mut app, Message::SettingsChangeStripToggled(true));
    for (i, tab) in app.tabs.iter().enumerate() {
        assert!(tab.editor.borrow().change_strip, "第 {i} 页没跟着开");
    }
}

#[test]
fn a_new_tab_inherits_the_current_switch() {
    // `fresh_tab` 漏下发是这个仓反复出现的形状（参考线/标尺都记过账）
    let mut app = Editpad::default();
    dispatch(&mut app, Message::SettingsChangeStripToggled(false));
    dispatch(&mut app, Message::NewTab);
    let last = app.tabs.len() - 1;
    assert!(
        !app.tabs[last].editor.borrow().change_strip,
        "新标签页必须按当前设置起步，不能默认开"
    );
    dispatch(&mut app, Message::SettingsChangeStripToggled(true));
    dispatch(&mut app, Message::NewTab);
    let last = app.tabs.len() - 1;
    assert!(app.tabs[last].editor.borrow().change_strip);
}

#[test]
fn the_row_appears_on_the_appearance_page_with_a_control() {
    let app = Editpad::default();
    let row = app
        .rows_for(SettingsPage::Appearance)
        .into_iter()
        .find(|r| r.key == editpad_core::lang::ROW_CHANGE_STRIP)
        .expect("设置页外观分类要有「变更历史行边条」这一行");
    assert!(!row.title.trim().is_empty(), "标题取自本语言文案");
    assert!(
        row.desc.contains("行号栏") || row.desc.contains("gutter"),
        "文案要说清边条画在哪，用户才知道自己在关什么，实得 {:?}",
        row.desc
    );
    assert!(
        app.settings_row_control(editpad_core::lang::ROW_CHANGE_STRIP)
            .is_some(),
        "这一行必须有控件（复选框），否则开关只能在配置文件里改"
    );
}

#[test]
fn toggling_the_setting_persists_it_both_ways() {
    let dir = scratch_dir("b12-change-strip");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    dispatch(&mut app, Message::SettingsChangeStripToggled(false));
    assert!(
        !editpad_core::Settings::load_from(&config).change_history_strip,
        "关了不落实盘＝重启又亮回来"
    );
    dispatch(&mut app, Message::SettingsChangeStripToggled(true));
    assert!(
        editpad_core::Settings::load_from(&config).change_history_strip,
        "开回去也要落盘（两向都要能存）"
    );
}

#[test]
fn edited_pages_light_the_baseline_and_a_save_clears_it_end_to_end() {
    // 端到端一格：打字 → 当前页有边条；基线抬上去（保存成功那一步）→ 边条收回。
    // ⚠️ 夹具必须先显式下发一次开关：`Editpad::default()` 的首屏**不走**
    // `fresh_tab`/`boot`（下发是异步初始化做的），裸 core 的开关是关的——
    // 与生产无关，但夹具要按同一条口径摆状态，否则这一格会假红。
    let mut app = app_with_three_pages();
    dispatch(&mut app, Message::SettingsChangeStripToggled(true));
    dispatch(&mut app, Message::SwitchTab(0));
    assert!(
        app.cur_handle.borrow().change_strip,
        "用例前提：当前页开关已开"
    );
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("hello\n".into())),
    );
    let has_mark = (0..3).any(|l| app.cur_handle.borrow().change_mark_at(l).is_some());
    assert!(has_mark, "刚打了一行，当前页该有边条");
    // 另一页一字未动 ⇒ 不该有任何标记（边条是每页各自的状态，不是全局一份）
    let other = app.tabs[1].editor.clone();
    let other_clean = (0..3).all(|l| other.borrow().change_mark_at(l).is_none());
    assert!(other_clean, "改动不该串到别的标签页");
    // `mark_saved` 是保存成功路径（`update/file.rs`）唯一调的那一步：基线抬上去、
    // 正文没动 ⇒ 缓存必须重算（单键缓存在这里会漏，这条也是它的端到端读数）
    app.cur_handle.borrow_mut().mark_saved();
    let after_save = (0..3).any(|l| app.cur_handle.borrow().change_mark_at(l).is_some());
    assert!(!after_save, "保存后该行不再算「未保存改动」，边条要收回去");
}

#[test]
fn the_setting_defaults_on_while_a_bare_core_defaults_off() {
    // 两档默认各有理由：Settings 开（被动显示、不参与置脏与撤销，与参考线同款）；
    // 裸 core 关（`indent_guides` 记过账：无头夹具造的 core 不该凭空多出墨迹，
    // 否则全部像素用例的读数被动改变）
    assert!(
        editpad_core::Settings::default().change_history_strip,
        "默认值语义：开"
    );
    assert!(
        !crate::editor::EditorCore::default().change_strip,
        "裸 core 默认关，避免既有像素用例被动改读数"
    );
}
