//! 档3：手动指定本页的语法高亮（状态栏那个语言位 + 命令面板的第四种模式）。
//!
//! 这一档存在的理由来自档1 的实测：内置索引会把 `.h` 判给 Objective-C、`.s` 判给 R，
//! 而「借道」这条路上有些语言借不得（`tsx`/`jsx` 会被 JS 语法推进正则态、跨行串色）。
//! 只要还有一格判不到、或判错后没法纠正，用户就只有一个地方可去——**自己选**。
//! 所以这里钉的是四件事：面板列得出全部语言、选下去真的换了配色、
//! 这件事**不写正文**（只读页也该能用）、以及状态栏那句话在未知语言时显示什么。

use super::*;

/// 打开语法模式面板（走的是命令注册表那一条路，与用户从面板里点进来同形）。
fn open_syntax_palette(app: &mut Editpad) {
    let msg = crate::hotkeys::dispatch_action("switch_syntax", keyboard::Modifiers::empty())
        .expect("switch_syntax 得在派发表里");
    dispatch(app, msg);
}

/// 某门语言在面板数据源里的下标（面板条目带的是下标，测试也得按下标取）。
fn syntax_index_of(name: &str) -> usize {
    editpad_core::available_syntax_names()
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("语法表里没有 {name}"))
}

#[test]
fn the_switch_syntax_command_opens_a_fourth_palette_mode() {
    let mut app = Editpad::default();
    open_syntax_palette(&mut app);
    assert!(app.palette_visible, "命令面板里的这条得真的把面板打开");
    assert_eq!(
        app.palette_mode,
        PaletteMode::Syntax,
        "打开的必须是语法模式"
    );
    // 语法模式说"搜得到/搜不到"那句，不该冒出来命令模式的键位提示
    // （P321 的口径：键位提示只在命令模式给，别的模式给就是假提示）
    assert!(
        app.palette_hint_line().is_none(),
        "语法模式不该借用命令模式的键位提示"
    );
    // 同模式再按一次 = 关闭（与命令/标签/剪贴板三种模式共用一条口径）
    open_syntax_palette(&mut app);
    assert!(!app.palette_visible, "语法模式也得遵守「再按一次即关闭」");
}

#[test]
fn the_command_is_registered_without_a_default_key_in_both_languages() {
    // 无默认键＝它只能从命令面板进来、或在设置 → 快捷键里被赋键（与
    // clip_history／split_selection_by_lines 同等待遇）。命令面板的数据源就是
    // 注册表全量，所以"在表里"一旦断了，面板与设置页同时没有这一行。
    let zh = crate::hotkeys::palette_commands(editpad_core::Lang::ZhCn);
    let found = zh
        .iter()
        .find(|c| c.id == "switch_syntax")
        .expect("命令面板里要有「切换语法」这条");
    assert!(!found.title.is_empty(), "标题不能是空串");
    let action = crate::hotkeys::HOTKEY_ACTIONS
        .iter()
        .find(|a| a.id == "switch_syntax")
        .expect("注册表里要有这条");
    assert!(
        action.default_combos.is_empty(),
        "这一条刻意不占键位（26 个 Ctrl+Shift 字母早已占尽，正确手势是搜语言名）"
    );
    // 英文界面侧也得有文案（新键漏了英文侧就是汉字漏进英文界面）
    let en = crate::hotkeys::palette_commands(editpad_core::Lang::En);
    let en_title = en
        .iter()
        .find(|c| c.id == "switch_syntax")
        .map(|c| c.title.clone())
        .expect("英文侧也要有这条");
    assert!(
        !en_title
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "英文界面那条不该带汉字，实际 {en_title:?}"
    );
}

#[test]
fn the_syntax_panel_lists_every_language_in_a_stable_order() {
    let mut app = Editpad::default();
    open_syntax_palette(&mut app);
    let names = editpad_core::available_syntax_names();
    let entries = app.palette_all_entries();
    assert_eq!(
        entries.len(),
        names.len(),
        "面板条目数必须等于语法表长度（下标就是执行参数，两边不同形就选错语言）"
    );
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.title, names[i], "第 {i} 条标题与语法表不符");
        assert_eq!(e.syntax_index, Some(i), "第 {i} 条必须带自己的下标");
        assert!(e.command_id.is_none() && e.tab_index.is_none() && e.clip_index.is_none());
    }
    // 下标跨帧稳定＝每帧重建的条目序列一模一样（面板每帧重建这个 Vec）
    let again = app.palette_all_entries();
    assert_eq!(
        again.iter().map(|e| e.title.clone()).collect::<Vec<_>>(),
        entries.iter().map(|e| e.title.clone()).collect::<Vec<_>>(),
        "两帧之间顺序变了，PalettePick 的下标就会指向两门不同的语言"
    );
}

#[test]
fn picking_a_language_rehighlights_without_writing_a_single_character() {
    // 夹具：一页**没有行注释前缀**的内容也能被切成 Rust —— 这一格要的不是嗅探，
    // 而是"手动指定"这条路本身。
    let mut app = loaded_txt_app();
    let before = app.cur_handle.borrow().doc.to_text().to_owned();
    assert_eq!(
        app.cur_handle.borrow().highlight_syntax_name().as_deref(),
        Some("Plain Text"),
        "夹具前提：.txt 走的是 Plain Text"
    );

    open_syntax_palette(&mut app);
    let idx = syntax_index_of("Rust");
    dispatch(&mut app, Message::PalettePick(idx));

    let ed = app.cur_handle.borrow();
    assert_eq!(ed.highlight_syntax_name().as_deref(), Some("Rust"));
    assert_eq!(ed.doc.to_text(), before, "换语言一个字都不该写进正文");
    drop(ed);
    assert!(
        !app.tab().dirty,
        "换语言不许把页面置脏（它不是编辑动作，关掉程序不该因此问要不要保存）"
    );
    // 反馈要说清作用范围：重开那个文件仍回到自动判定
    let expect = format!(
        "{} Rust（{}）",
        app.t(editpad_core::Key::HintSyntaxSet),
        app.t(editpad_core::Key::HintReloadAutoDetect)
    );
    assert_eq!(app.status, expect, "状态栏那句得报出切成了哪门语言");
}

#[test]
fn picking_a_language_is_allowed_on_a_read_only_page() {
    // 只读锁挡的是**写正文**的手势。换高亮器不动文档 ⇒ 拦它就是拦掉用户在
    // 只读文件里唯一能纠正误判的手段。
    let mut app = loaded_txt_app();
    dispatch(&mut app, Message::ToggleReadOnly);
    assert!(app.cur_handle.borrow().read_only, "夹具前提：这页已锁只读");

    open_syntax_palette(&mut app);
    dispatch(&mut app, Message::PalettePick(syntax_index_of("Python")));
    assert_eq!(
        app.cur_handle.borrow().highlight_syntax_name().as_deref(),
        Some("Python"),
        "只读页照样要能手动指定语言"
    );
}

#[test]
fn picking_out_of_range_is_a_silent_no_op() {
    let mut app = loaded_txt_app();
    let before = app.cur_handle.borrow().highlight_syntax_name().clone();
    dispatch(&mut app, Message::SyntaxPick(9999));
    assert_eq!(
        app.cur_handle.borrow().highlight_syntax_name(),
        before,
        "越界下标不许改语言，也不许 panic"
    );
}

#[test]
fn the_status_bar_names_the_language_and_falls_back_to_a_dash() {
    // 未知语言（未命名新页、内容也嗅不出来）＝纯文本路径 ⇒ 状态栏显 `—`，
    // 与编码那一格的空态同形。这一句抽成方法就是为了让它能被钉住。
    let app = Editpad::default();
    assert_eq!(app.status_syntax_label(), "—", "没有高亮器时必须显 —");

    // 手动指定之后显的是**这一页**选定的名字
    let mut app = Editpad::default();
    open_syntax_palette(&mut app);
    dispatch(&mut app, Message::PalettePick(syntax_index_of("Markdown")));
    assert_eq!(app.status_syntax_label(), "Markdown");

    // 选择是**每页**的属性，不是应用的属性。⚠️ 两页都得有内容：空净页会被
    // FileDropped 复用成同一个标签页（第一版夹具就在这里踩空——它以为开了两页）。
    let mut app = loaded_txt_app();
    assert_eq!(
        app.status_syntax_label(),
        "Plain Text",
        "夹具前提：.txt 判成 Plain Text"
    );
    dispatch(
        &mut app,
        Message::FileDropped(PathBuf::from("C:/doc/other.rs")),
    );
    let seq = app.job_seq;
    dispatch(
        &mut app,
        Message::Loaded(
            seq,
            Ok((
                editpad_core::Document::from_str("fn main() {}"),
                String::new(),
                "UTF-8".to_owned(),
            )),
        ),
    );
    assert_eq!(app.tabs.len(), 2, "夹具自证：这才真的是两页");
    assert_eq!(app.status_syntax_label(), "Rust", "新页按扩展名判");

    dispatch(&mut app, Message::SwitchTab(0));
    dispatch(&mut app, Message::SyntaxPick(syntax_index_of("Markdown")));
    dispatch(&mut app, Message::SwitchTab(1));
    assert_eq!(
        app.status_syntax_label(),
        "Rust",
        "另一页选了语言，不许把这页的语言一起换掉"
    );
    dispatch(&mut app, Message::SwitchTab(0));
    assert_eq!(
        app.status_syntax_label(),
        "Markdown",
        "回到第一页必须还是那页自己选定的语言"
    );
}

#[test]
fn the_syntax_panel_filters_by_name_and_marks_the_current_one() {
    let mut app = Editpad::default();
    open_syntax_palette(&mut app);
    app.palette_input = "mark".to_owned();
    let hits = app.palette_filtered();
    assert!(
        hits.iter().any(|e| e.title == "Markdown"),
        "搜 mark 要能搜到 Markdown，实际 {:?}",
        hits.iter().map(|e| e.title.clone()).collect::<Vec<_>>()
    );
    // 本页当前那门语言在 detail 上标一句「当前」，其余留空
    dispatch(&mut app, Message::SyntaxPick(syntax_index_of("Perl")));
    let marked: Vec<String> = app
        .palette_all_entries()
        .into_iter()
        .filter(|e| !e.detail.is_empty())
        .map(|e| e.title)
        .collect();
    assert_eq!(marked, vec!["Perl".to_owned()], "detail 只许标住当前那一门");
}
