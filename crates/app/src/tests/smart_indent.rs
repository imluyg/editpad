//! P458（语言适配档5）：回车补一档——**接线**这一档。
//!
//! 判据本身住在 `editpad_core::indent`（17 门封闭名单、四档动作、文档步长，用例在
//! core 那一侧）；落到文档的形状在 `editor::edit_tests::enter_smart_*`。这里只钉
//! 接线才照得到的四格：
//!
//! - **默认关** ⇒ 回车与引入本功能前逐字节相同（这不是"次要路径"，是用户装完新版本
//!   没动设置时看到的形状，也是 B11 立下的口径：写没按下的字符必须显式开启）；
//! - 开着才走新入口（走生产消息路 `Message::Edit(EditOp::Enter)`，与热键那把 Enter
//!   同一个 op ⇒ 钉的是"设置 ⇒ 换回车行为"那格线）；
//! - **次序**：Markdown 页永远先走它自己的续写判据，不被花括号推断抢走；
//! - 一次回车＝一发撤销：补的那一档不许单独成为一组（否则撤一次留下半截空行）。
//!
//! 光标在新行上的闪烁位置、真实文件里阶梯的观感——只能开真窗口看的东西记在
//! `docs/手测清单.md`，这里不冒充覆盖。

use super::*;
use crate::editor::{CursorPos, EditOp, Motion};

/// 打开关。
///
/// ⚠️ 必须先钉 `settings_path_override`：这一路会 `persist_settings`，而没钉住时它写的是
/// **真实实例 config**（`%APPDATA%\editpad\instances\<键>\config.toml`）。这条地基账不是
/// 本轮引入的（`update/file.rs` 的装载成功路径同样会写），本仓口径＝凡走到
/// `persist_settings` 的用例先设 override。
fn turn_on(app: &mut Editpad) {
    let dir = scratch_dir("p458-smart-indent");
    app.settings_path_override = Some(dir.join("config.toml"));
    dispatch(app, Message::SettingsSmartIndentToggled(true));
    assert!(
        app.settings.smart_indent,
        "用例前提：开关已开（默认关，所以每格都得自己开）"
    );
}

fn doc(app: &Editpad) -> String {
    app.cur_handle.borrow().doc.to_text()
}

fn caret(app: &Editpad) -> CursorPos {
    app.cur_handle.borrow().cursor
}

/// 给当前页指定语言（测试辅助；生产是 `resolve_language` → `set_language_by_name`）。
fn set_lang(app: &mut Editpad, extension: &str) {
    app.cur_handle.borrow_mut().set_language(Some(extension));
}

/// 先打字把文本放进去，再把光标定到行尾（走的是生产编辑路，不是直接改文档）。
fn type_text(app: &mut Editpad, text: &str) {
    dispatch(app, Message::Edit(EditOp::InsertText(text.to_owned())));
}

fn press_enter(app: &mut Editpad) {
    dispatch(app, Message::Edit(EditOp::Enter));
}

/// 第 1 格（最重要）：默认关——`def f():` 后按回车就是一个空行，没有补出来的四格。
#[test]
fn the_setting_defaults_off_and_enter_is_then_byte_identical() {
    let app = Editpad::default();
    assert!(
        !app.settings.smart_indent,
        "用例前提：默认必须关（它写入的空白不是用户按下的）"
    );
    let mut app = app;
    set_lang(&mut app, "py");
    type_text(&mut app, "def f():");
    press_enter(&mut app);
    assert_eq!(
        doc(&app),
        "def f():\n",
        "关着却补了缩进＝把行为塞进默认路径"
    );
    assert_eq!(caret(&app), CursorPos { line: 1, col: 0 });
}

/// 第 2 格：开着之后，Python 块头后面补一档；一档宽度来自这份文档自己（这里是 2 格）。
#[test]
fn smart_enter_indents_after_a_python_header_when_on() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    set_lang(&mut app, "py");
    type_text(&mut app, "def f():\n  pass");
    // 光标此刻在文档末尾（第 1 行 "  pass" 之后），先回到块头那一行的行尾
    dispatch(
        &mut app,
        Message::Edit(EditOp::Motion(Motion::DocStart, false)),
    );
    dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::End, false)));
    press_enter(&mut app);
    assert_eq!(
        doc(&app),
        "def f():\n  \n  pass",
        "块头后回车没补档＝设置到回车行为的接线断了"
    );
    // ⚠️ 补的是**两格**不是四格：这份文档自己就是 2 格一档（`  pass` 那行是证据），
    // 而 `dominant_indent` 取文档最外层那一档。硬补 4 会把整份文件的阶梯推歪——
    // 这一格同时钉住"随当前行风格"那条用户点单的口径。
    assert_eq!(caret(&app), CursorPos { line: 1, col: 2 });
}

/// 第 3 格：一次撤销同时收回「换行 + 补的那一档」，不许留下半截空行。
#[test]
fn smart_enter_is_a_single_undo_step() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    set_lang(&mut app, "java");
    type_text(&mut app, "if (x) {");
    press_enter(&mut app);
    assert_eq!(doc(&app), "if (x) {\n    ", "前提：已补上一档");
    dispatch(&mut app, Message::Edit(EditOp::Undo));
    assert_eq!(
        doc(&app),
        "if (x) {",
        "撤一次要同时收回换行与那一档，否则留下半截空行"
    );
}

/// 第 4 格（次序）：Markdown 页永远先走自己的续写判据。
/// 这一格存在的理由是 `E::Enter` 那一臂里两个分支的**顺序**：反过来的话，
/// `- 甲` 后面接的会是缩进而不是 `- `，而列表续写是早就交付的行为。
#[test]
fn markdown_page_keeps_its_own_enter_rule_even_when_smart_indent_is_on() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    set_lang(&mut app, "md");
    type_text(&mut app, "- 甲");
    press_enter(&mut app);
    assert_eq!(
        doc(&app),
        "- 甲\n- ",
        "Markdown 页的回车必须接列表，不是补一档"
    );
    // 再来一格：Markdown 行尾正好是 `{` 时也不许被花括号判据抢走
    let mut app2 = Editpad::default();
    turn_on(&mut app2);
    set_lang(&mut app2, "md");
    type_text(&mut app2, "code {");
    press_enter(&mut app2);
    assert_eq!(
        doc(&app2),
        "code {\n",
        "Markdown 不在缩进名单里，且那一页的回车归它自己的判据"
    );
}

/// 第 5 格：名单外的语言开着也一样不动（这一格与 core 的"封闭名单"用例成对——
/// 那边钉判据，这边钉"设置开了也没绕过名单"）。
#[test]
fn smart_enter_on_a_plain_text_page_still_just_breaks_the_line() {
    let mut app = Editpad::default();
    turn_on(&mut app);
    // 未命名页没判出语言＝纯文本路径
    type_text(&mut app, "notes {");
    press_enter(&mut app);
    assert_eq!(doc(&app), "notes {\n", "纯文本页被补了空白＝名单被绕过了");
}

/// 第 6 格：设置页那一行有控件，且开关**两向**都落盘（与其余偏好同一条持久化路）。
#[test]
fn the_settings_row_exists_and_persists_both_ways() {
    let dir = scratch_dir("p458-smart-indent-persist");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    assert!(
        app.settings_row_control(editpad_core::lang::ROW_SMART_INDENT)
            .is_some(),
        "这一行必须有控件（复选框），否则开关只能在配置文件里改"
    );
    dispatch(&mut app, Message::SettingsSmartIndentToggled(true));
    assert!(
        editpad_core::Settings::load_from(&config).smart_indent,
        "开了不落实盘＝重启就丢"
    );
    dispatch(&mut app, Message::SettingsSmartIndentToggled(false));
    assert!(
        !editpad_core::Settings::load_from(&config).smart_indent,
        "关回去也要落盘（两向都要能存）"
    );
}
