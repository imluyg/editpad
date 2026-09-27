//! P322（功能队列①）：多行粘贴按光标所在行的缩进对齐。
//!
//! 算式在 core（`align_paste_indent`，表测在那一侧），这一档钉的是**接线**：
//! - 默认必须关——用户点单的原话理由是"日志正文被改缩进是真实伤害"，
//!   所以"关着时逐字节等于剪贴板"是这项功能最重要的一格，不是次要格；
//! - 目标缩进取自**光标所在行**（不是选区、不是文件首行）；
//! - 列块态与多光标**故意不动**：那两种情形"以哪一处为准"没有定义；
//! - 从剪贴板历史取用（P319 的 `ClipPick`）走同一条预处理 ⇒ 只有一份粘贴行为。
//!
//! 空目标行也是合法的一格（块被整体剥到列首），别把"没缩进"当成"不处理"。

use super::*;
use crate::editor::{BlockSel, CursorPos, EditOp};

/// 造一页：正文是一行 4 空格缩进的内容，光标停在**该行行首之后的缩进末尾**。
fn app_with_indented_caret() -> Editpad {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("    ".to_owned())),
    );
    app
}

fn paste(app: &mut Editpad, text: &str) -> String {
    dispatch(app, Message::Pasted(text.to_owned()));
    app.cur_handle.borrow().doc.to_text()
}

#[test]
fn the_setting_defaults_off_and_a_paste_is_then_byte_for_byte_the_clipboard() {
    let app = Editpad::default();
    assert!(
        !app.settings.paste_align_indent,
        "用例前提：默认必须关（这是用户点单定的默认值）"
    );
    let mut app = app_with_indented_caret();
    // 关着：粘进去的就是剪贴板那一份——首行接在已有缩进之后，其余行原样在列首
    assert_eq!(
        paste(&mut app, "aaa\nbbb\n"),
        "    aaa\nbbb\n",
        "开关关着却改了缩进＝把用户没要求的行为塞进默认路径"
    );
}

#[test]
fn turning_it_on_shifts_the_block_onto_the_caret_line_indent() {
    let mut app = app_with_indented_caret();
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    assert!(app.settings.paste_align_indent, "前提：开关已开");
    // 目标缩进 = 光标行（第 0 行）的 "    "；块基准 = 空 ⇒ 每条非空行补 4 空格。
    // 首行那 8 个空格不是 bug：粘贴点本身已在 4 空格之后，块又各自带了 4 空格。
    assert_eq!(
        paste(&mut app, "aaa\nbbb\n"),
        "        aaa\n    bbb\n",
        "实得 {:?}",
        app.cur_handle.borrow().doc.to_text()
    );
}

#[test]
fn a_caret_line_with_no_indent_dedents_the_block_to_the_margin() {
    // 空目标缩进是合法一格：把带缩进的块整体剥到列首，而不是"没缩进就不处理"
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("x\n".to_owned())),
    );
    app.cur_handle.borrow_mut().cursor = CursorPos { line: 1, col: 0 };
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    let after = paste(&mut app, "    deep\n      deeper\n");
    assert_eq!(
        after, "x\ndeep\n  deeper\n",
        "相对层次要留着（4 与 6 的差还在），只是整块剥到列首"
    );
}

#[test]
fn a_column_block_paste_is_left_alone_even_when_the_setting_is_on() {
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("    abcdef\n".to_owned())),
    );
    {
        let mut core = app.cur_handle.borrow_mut();
        core.cursor = CursorPos { line: 0, col: 8 };
        core.block_sel = Some(BlockSel {
            anchor: CursorPos { line: 0, col: 5 },
            head: CursorPos { line: 0, col: 8 },
        });
    }
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    let after = paste(&mut app, "q\nr\n");
    assert!(
        !after.contains("    q"),
        "列块态平移行首空白会把块拧歪 ⇒ 该原样，实得 {after:?}"
    );
    assert!(
        after.contains('q') && after.contains('r'),
        "前提：文本确实粘进去了"
    );
}

#[test]
fn a_multi_cursor_paste_is_left_alone_even_when_the_setting_is_on() {
    // 两条光标落在不同缩进的行上，"以哪一条为准"没有定义 ⇒ 一律不改
    let mut app = Editpad::default();
    dispatch(
        &mut app,
        Message::Edit(EditOp::InsertText("    head\ntail\n".to_owned())),
    );
    {
        let mut core = app.cur_handle.borrow_mut();
        core.cursor = CursorPos { line: 0, col: 6 };
        assert!(
            core.toggle_extra_cursor(CursorPos { line: 1, col: 4 }),
            "用例前提：第二条光标要真的加上（同行列/守卫会拒绝）"
        );
        assert!(core.has_multi());
    }
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    let after = paste(&mut app, "x\ny\n");
    assert!(
        !after.contains("    x") && !after.contains("        x"),
        "多光标下不许按任一光标的缩进平移，实得 {after:?}"
    );
}

#[test]
fn a_paste_from_the_clipboard_history_takes_the_same_path() {
    // P319 的取用与 Ctrl+V 共用一条预处理 ⇒ 不会出现"两种粘贴两种缩进行为"
    let mut app = app_with_indented_caret();
    let _ = app.clip_write("aaa\nbbb\n".to_owned());
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    dispatch(&mut app, Message::ClipPick(0));
    assert_eq!(
        app.cur_handle.borrow().doc.to_text(),
        "        aaa\n    bbb\n",
        "从历史取用也该按光标行对齐（同一份行为，不是第二个入口）"
    );
}

#[test]
fn toggling_the_setting_persists_it_like_the_other_preferences() {
    // 落盘这一格照 `font_size_delta_clamps_and_persists_to_injected_path` 的形状走
    let dir = scratch_dir("p322-paste-indent");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(true));
    assert!(app.settings.paste_align_indent);
    assert!(
        editpad_core::Settings::load_from(&config).paste_align_indent,
        "开了不落实盘＝重启就丢，用户会觉得这个开关没用"
    );
    dispatch(&mut app, Message::SettingsPasteAlignIndentToggled(false));
    assert!(
        !editpad_core::Settings::load_from(&config).paste_align_indent,
        "关回去也要落盘（两向都要能存）"
    );
}

#[test]
fn the_row_appears_on_the_appearance_page_with_a_control() {
    let app = Editpad::default();
    let row = app
        .rows_for(SettingsPage::Appearance)
        .into_iter()
        .find(|r| r.key == editpad_core::lang::ROW_PASTE_ALIGN_INDENT)
        .expect("设置页外观分类要有这一行");
    assert!(!row.title.trim().is_empty(), "标题取自本语言文案");
    assert!(
        row.desc.contains("默认关") || row.desc.contains("Off by default"),
        "文案要把「默认关」写出来——它改的是粘贴内容，实得 {:?}",
        row.desc
    );
    assert!(
        app.settings_row_control(editpad_core::lang::ROW_PASTE_ALIGN_INDENT)
            .is_some(),
        "这一行必须有控件（复选框），否则开关只能在配置文件里改"
    );
}
