use super::*;

/// 设置弹窗的几何契约（P47 卡片尺寸纯函数的落地结果）：
/// **宽 720、高 vh−32、且水平垂直都居中**。
///
/// 走生产视图树——卡片尺寸算错或叠层位置跑偏，这里就红；
/// 此前本文件的弹窗用例只写 `let _ = app.view()`（仅验「不 panic」）。
fn assert_settings_card_centered(app: &Editpad) {
    let ui = ViewTree::layout_default(app);
    let card = ui
        .find(|b| (700.0..=740.0).contains(&b.width) && b.height > 300.0)
        .unwrap_or_else(|| panic!("应能找到设置弹窗卡片；\n树形：\n{}", ui.dump()));
    let vp = ui.viewport();
    let c = card.center();
    assert!(
        (c.x - vp.width / 2.0).abs() < 1.0,
        "弹窗应水平居中：中心 x = {:.1}，视口宽 {:.1}",
        c.x,
        vp.width
    );
    assert!(
        (c.y - vp.height / 2.0).abs() < 1.0,
        "弹窗应垂直居中：中心 y = {:.1}，视口高 {:.1}",
        c.y,
        vp.height
    );
}

// ---------- P33 字体一致性 ----------

#[test]
fn font_pin_is_idempotent_and_headless_tests_share_the_products_face() {
    // P33 冒烟（公开路径在测试环境真实可用、二次调用短路）＋ P329 升级。
    //
    // ⚠️ 本用例的原注释写的是「副作用仅为 fontdb 解析目标被改，测试无渲染断言、
    // 不受影响」——**那句是错的**，而且是这条线索的根因：它改的是**进程级**的
    // `Family::Monospace` 解析目标，本用例先跑与否决定同一二进制里所有按
    // `BODY_FONT` 渲染的用例读到哪一套字（本机 24px 实测：未钉 Cascadia Mono
    // 14.0625 vs 钉了 NSimSun 12.0）。
    //
    // 所以这里**故意不先调用钉字**：第一步就走生产量度入口，判"入口自己有没有把
    // 解析目标钉到位"（P329 把钉字从"启动顺序约定"变成 `metrics`/`draw` 的结构
    // 保证）。第二条断言才用到幂等。将来谁改候选表、或把 `set_monospace_family`
    // 那一步弄丢，红在这里——不是红在某条看不懂的像素差上。
    let available = crate::fonts::enumerate_available_families();
    assert!(
        !available.is_empty(),
        "全局 font_system 在测试环境必须可枚举（空清单＝量度与解析全都不可信）"
    );
    let via_funnel = crate::editor::metrics::measure_char_width(crate::editor::BODY_FONT, 24.0);
    apply_default_cjk_mono_pin();
    apply_default_cjk_mono_pin(); // 二次调用不得改变已生效的解析目标
    let mono_w = crate::editor::metrics::measure_char_width(crate::editor::BODY_FONT, 24.0);
    assert_eq!(
        via_funnel, mono_w,
        "生产量度入口解析出的 MONOSPACE 必须已经等于钉好之后的那一套（不等＝入口漏钉，\
         用例读到的字形族取决于谁先跑）"
    );
    let Some(family) = crate::editor::pick_cjk_mono_family(&available) else {
        // 无候选（非 CJK 环境）⇒ 钉字按设计什么都不做，只保证上面两条不炸
        return;
    };
    let named = iced::Font {
        family: iced::font::Family::Name(family),
        ..iced::Font::DEFAULT
    };
    let named_w = crate::editor::metrics::measure_char_width(named, 24.0);
    assert_eq!(
        mono_w, named_w,
        "MONOSPACE 解析出的列宽必须等于产品钉选族按名字量的宽度：不等＝用例与产品两套字形"
    );
    // 反空转（正对照）：若"任何族都量出同一个宽度"，上面那条相等就是瞎的
    let others: Vec<f32> = [
        "Arial",
        "Times New Roman",
        "Verdana",
        "Courier New",
        "Segoe UI",
    ]
    .iter()
    .filter_map(|name| {
        crate::editor::metrics::measure_char_width(
            iced::Font {
                family: iced::font::Family::Name(name),
                ..iced::Font::DEFAULT
            },
            24.0,
        )
    })
    .collect();
    assert!(
        others.len() >= 2,
        "正对照前提：本机至少要能量到两个别的族的宽度，实得 {} 个",
        others.len()
    );
    assert!(
        others.iter().any(|w| Some(*w) != named_w),
        "正对照：别的族量出与钉选族**不同**的宽度，上面那条相等断言才有分辨力"
    );
}

// ---------- P27 设置按钮 + 设置弹窗 + P62 热键系统 ----------

#[test]
fn hotkey_actions_are_well_formed() {
    assert!(!HOTKEY_ACTIONS.is_empty(), "热键动作注册表不得为空");
    // id 唯一且非空（持久化主键）
    let mut ids: Vec<_> = HOTKEY_ACTIONS.iter().map(|a| a.id).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "动作 id 存在重复");
    // 默认组合全部可归一（格式合法）且互不冲突（多默认逐个检查）
    let mut combos: Vec<_> = HOTKEY_ACTIONS
        .iter()
        .flat_map(|a| a.default_combos)
        .map(|c| editpad_core::normalize_combo(c).expect("默认组合必须合法"))
        .collect();
    let combo_total = combos.len();
    combos.sort_unstable();
    combos.dedup();
    assert_eq!(combos.len(), combo_total, "默认组合存在冲突");
    for a in HOTKEY_ACTIONS {
        assert!(
            !a.desc.text(editpad_core::Lang::ZhCn).trim().is_empty()
                && !a.desc.text(editpad_core::Lang::En).trim().is_empty(),
            "动作 {} 说明不得为空（两种语言）",
            a.id
        );
    }
}

#[test]
fn hotkey_actions_all_dispatch_through_handle_key() {
    // 注册表的每个默认组合（含多默认同义键位）都必须经 handle_key
    // （默认表）产生消息——注册表与分发器任何一侧改动漏同步，
    // 本测试当场暴露（防漂移）
    for action in HOTKEY_ACTIONS {
        for combo in action.default_combos {
            let (mods, key) = parse_combo_for_test(combo);
            let dispatched = handle_key_defaults(key, mods);
            assert!(
                dispatched.is_some(),
                "动作 {}（{combo}）在 handle_key 中无对应分支",
                action.id
            );
        }
    }
}

#[test]
fn handle_key_remap_overrides_and_frees_default() {
    // 把「保存」重映射到 Ctrl+Shift+S：新组合生效、旧组合让出
    let mut remap = std::collections::HashMap::new();
    remap.insert("save".to_owned(), "Ctrl+Shift+S".to_owned());

    let (mods, key) = parse_combo_for_test("Ctrl+Shift+S");
    assert!(matches!(
        handle_key(key, mods, &remap),
        Some(Message::SaveRequested)
    ));
    let (mods, key) = parse_combo_for_test("Ctrl+S");
    assert!(
        handle_key(key, mods, &remap).is_none(),
        "被挪走的默认组合必须让出（用户显式放弃）"
    );
    // 未重映射的动作不受影响
    let (mods, key) = parse_combo_for_test("Ctrl+O");
    assert!(matches!(
        handle_key(key, mods, &remap),
        Some(Message::OpenRequested)
    ));
}

#[test]
fn redo_has_two_default_combos_and_remap_frees_both() {
    // 多默认同义键位：重做同时支持 Ctrl+Y 与主流的 Ctrl+Shift+Z；
    // 用户重映射后两个默认组合一并让出（显式放弃 = 全部放弃）
    let (mods, key) = parse_combo_for_test("Ctrl+Y");
    assert!(matches!(
        handle_key_defaults(key, mods),
        Some(Message::Edit(EditOp::Redo))
    ));
    let (mods, key) = parse_combo_for_test("Ctrl+Shift+Z");
    assert!(matches!(
        handle_key_defaults(key, mods),
        Some(Message::Edit(EditOp::Redo))
    ));

    // 重映射 redo → Ctrl+R：两个默认组合都让出
    let mut remap = std::collections::HashMap::new();
    remap.insert("redo".to_owned(), "Ctrl+R".to_owned());
    let (mods, key) = parse_combo_for_test("Ctrl+Y");
    assert!(
        handle_key(key, mods, &remap).is_none(),
        "重映射后主默认让出"
    );
    let (mods, key) = parse_combo_for_test("Ctrl+Shift+Z");
    assert!(
        handle_key(key, mods, &remap).is_none(),
        "重映射后附加默认一并让出"
    );
    let (mods, key) = parse_combo_for_test("Ctrl+R");
    assert!(matches!(
        handle_key(key, mods, &remap),
        Some(Message::Edit(EditOp::Redo))
    ));
}

#[test]
fn settings_modal_open_close_and_busy_guard() {
    let mut app = Editpad::default();
    assert!(!app.settings_visible);

    // 工具栏「设置」开 ↔ 再点关
    dispatch(&mut app, Message::SettingsToggled);
    assert!(app.settings_visible);
    dispatch(&mut app, Message::SettingsToggled);
    assert!(!app.settings_visible);

    // busy（加载/保存中）禁开——update 层守卫，与工具栏按钮禁用双保险
    app.busy = true;
    dispatch(&mut app, Message::SettingsToggled);
    assert!(!app.settings_visible, "busy 时不得打开设置弹窗");
    app.busy = false;

    // Esc 统一收口所有面板：设置弹窗一并关闭
    dispatch(&mut app, Message::SettingsToggled);
    assert!(app.settings_visible);
    dispatch(&mut app, Message::BarsDismissed);
    assert!(!app.settings_visible, "Esc 必须关闭设置弹窗");
}

#[test]
fn settings_modal_actions_write_back_and_persist_to_injected_path() {
    use editpad_core::settings::{
        EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT, MAX_AUTOSAVE_DELAY_SECS, MAX_SNAPSHOT_INTERVAL_SECS,
        MIN_AUTOSAVE_DELAY_SECS, MIN_SNAPSHOT_INTERVAL_SECS,
    };
    let dir = scratch_dir("p27-settings");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    // 注入配置路径：弹窗的每次改动都落到这里，绝不碰真实 %APPDATA%
    app.settings_path_override = Some(config.clone());

    // 即时保存开关写回
    dispatch(&mut app, Message::SettingsAutosaveToggled(false));
    assert!(!app.settings.autosave_enabled);

    // 防抖秒数 ±1 步进，clamp [1,60]
    dispatch(&mut app, Message::SettingsAutosaveDelayDelta(-1));
    assert_eq!(app.settings.autosave_delay_secs, 1, "默认 2s 减 1 → 1s");
    dispatch(&mut app, Message::SettingsAutosaveDelayDelta(-10));
    assert_eq!(app.settings.autosave_delay_secs, MIN_AUTOSAVE_DELAY_SECS);
    dispatch(&mut app, Message::SettingsAutosaveDelayDelta(500));
    assert_eq!(app.settings.autosave_delay_secs, MAX_AUTOSAVE_DELAY_SECS);

    // P20 口径随开关收编：关闭「记住最近文件」→ 存量列表一并清空
    app.settings.recent_files.push("C:/old.txt".to_owned());
    dispatch(&mut app, Message::SettingsRememberRecentToggled(false));
    assert!(
        app.settings.recent_files.is_empty(),
        "只关开关不清数据等于没关"
    );

    // 快照与会话两个开关
    dispatch(&mut app, Message::SettingsSnapshotsToggled(false));
    assert!(!app.settings.enable_snapshots);
    dispatch(&mut app, Message::SettingsRememberSessionToggled(false));
    assert!(!app.settings.remember_session);

    // 关窗行为快照直退 ↔ 每次询问往返切换
    dispatch(&mut app, Message::SettingsExitModeToggled);
    assert_eq!(app.settings.exit_mode, EXIT_MODE_ASK);
    dispatch(&mut app, Message::SettingsExitModeToggled);
    assert_eq!(app.settings.exit_mode, EXIT_MODE_SNAPSHOT);

    // 心跳间隔 ±5 步进，clamp [5,120]：越界方向被夹在边界上
    dispatch(&mut app, Message::SettingsIntervalDelta(-50));
    assert_eq!(
        app.settings.snapshot_interval_secs,
        MIN_SNAPSHOT_INTERVAL_SECS
    );
    dispatch(&mut app, Message::SettingsIntervalDelta(500));
    assert_eq!(
        app.settings.snapshot_interval_secs,
        MAX_SNAPSHOT_INTERVAL_SECS
    );

    // 汇总断言：以上每次改动都已即时持久化到注入路径
    assert_eq!(
        editpad_core::Settings::load_from(&config),
        app.settings,
        "弹窗改动必须即时写回 config.toml"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------- P47 设置弹窗分类导航（侧栏分类风格） ----------

#[test]
fn settings_page_nav_selects_and_clears_search() {
    let mut app = app_with_tabs(2);
    dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
    assert_eq!(
        app.settings_page,
        SettingsPage::default(),
        "默认落在第一分类"
    );

    // 打开弹窗 → 每个分类都可选中，且选中后视图树可构造
    dispatch(&mut app, Message::SettingsToggled);
    for page in SettingsPage::ALL {
        dispatch(&mut app, Message::SettingsPageSelected(page));
        assert_eq!(app.settings_page, page);
        assert_settings_card_centered(&app);
    }

    // 搜索态：输入词入账、视图可构造（跨分类过滤分支）
    dispatch(&mut app, Message::SettingsSearchChanged("快照".to_owned()));
    assert_eq!(app.settings_search, "快照");
    assert_settings_card_centered(&app);

    // 点导航 = 离开搜索态（同款语义：搜索词被清空）
    dispatch(&mut app, Message::SettingsPageSelected(SettingsPage::Save));
    assert!(app.settings_search.is_empty(), "点分类必须清空搜索词");
    assert_eq!(app.settings_page, SettingsPage::Save);

    // 关闭弹窗：搜索词清空；重开 = 回到第一分类（第 64 轮用户点单：
    // 不再记忆上次浏览位置，P51 撤销）
    dispatch(&mut app, Message::SettingsToggled);
    assert!(app.settings_search.is_empty(), "关弹窗必须清空搜索词");
    dispatch(&mut app, Message::SettingsToggled);
    assert_eq!(
        app.settings_page,
        SettingsPage::default(),
        "每次进入设置都默认第一分类"
    );
}

// ---------- P51 分类页记忆持久化 ----------

#[test]
fn settings_nav_keys_match_core_registry() {
    // app 侧键映射与 core 注册表逐一对上（防两套字符串漂移）：
    // 每个分类的 key 必须经 core 归一后原样返回（= 注册键合法）
    for page in SettingsPage::ALL {
        assert_eq!(
            editpad_core::normalize_settings_page(page.key()),
            page.key(),
            "分类 {} 的持久化键不在 core 注册表中",
            page.title(editpad_core::Lang::ZhCn)
        );
    }
    // 顺序一致：ALL 的展示顺序 = 注册表顺序
    for (page, key) in SettingsPage::ALL.iter().zip(editpad_core::SETTINGS_PAGES) {
        assert_eq!(page.key(), key);
    }
}

#[test]
fn settings_page_selection_is_not_persisted_anymore() {
    // 第 64 轮用户点单回归钉子：选分类只改 UI 态——配置里不再出现
    // settings_page 键（旧文件里的遗留键由 serde 忽略、下次保存消失）
    let dir = scratch_dir("p64-no-page-memory");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    dispatch(
        &mut app,
        Message::SettingsPageSelected(SettingsPage::Hotkeys),
    );
    assert_eq!(app.settings_page, SettingsPage::Hotkeys, "会话内导航照常");
    dispatch(&mut app, Message::SettingsToggled); // 触发一次落盘
    let raw = std::fs::read_to_string(&config).unwrap_or_default();
    assert!(
        !raw.contains("settings_page"),
        "config.toml 不得再写入分类页键"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn settings_search_hit_rules() {
    // 空词 / 纯空白 = 不过滤（全部命中）
    assert!(settings_search_hit("", "任意", "任意"));
    assert!(settings_search_hit("   ", "任意", "任意"));

    // 大小写不敏感：desc 里的 JSON 命中 "json" 查询（热键行描述）
    let zh = editpad_core::Lang::ZhCn;
    let (_, json_desc) = HOTKEY_ACTIONS
        .iter()
        .find(|a| a.desc.text(zh).contains("JSON"))
        .map(|a| (a.id, a.desc.text(zh)))
        .expect("热键动作必有 JSON 格式化条目");
    assert!(settings_search_hit("json", "Ctrl+Shift+F", json_desc));

    // 英文文案同样参与过滤（切到英文后按英文关键词命中）
    let en = editpad_core::Lang::En;
    let (_, json_desc_en) = HOTKEY_ACTIONS
        .iter()
        .find(|a| a.desc.text(en).contains("JSON"))
        .map(|a| (a.id, a.desc.text(en)))
        .expect("英文表同样有 JSON 条目");
    assert!(settings_search_hit("json", "Ctrl+Shift+F", json_desc_en));

    // 中文子串：标题命中（会话快照）与描述命中（关窗行为含「快照直退」）
    assert!(settings_search_hit(
        "快照",
        "会话快照",
        "关窗时自动保存未存内容"
    ));
    assert!(settings_search_hit(
        "快照",
        "关窗行为",
        "「快照直退」不打断"
    ));

    // 未命中
    assert!(!settings_search_hit(
        "打印机",
        "主题",
        "切换深色 / 浅色主题"
    ));
}

#[test]
fn settings_row_catalog_covers_pages_and_controls() {
    let zh = editpad_core::Lang::ZhCn;
    let rows: Vec<SettingsRow> = settings_rows(zh).collect();

    // 行元数据完整：键/标题/描述非空，键全清单唯一（控件匹配的依据）
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    let total = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), total, "设置行键存在重复：{keys:?}");
    for r in &rows {
        assert!(!r.key.trim().is_empty() && !r.title.trim().is_empty());
        assert!(!r.desc.trim().is_empty(), "行 {} 描述不得为空", r.key);
    }

    // 静态目录覆盖非热键分类页（P62 起热键页为动态行，由 rows_for 构建）
    for page in SettingsPage::ALL {
        if page == SettingsPage::Hotkeys {
            continue;
        }
        assert!(
            rows.iter().any(|r| r.page == page),
            "分类 {} 在行清单中没有条目",
            page.title(zh)
        );
    }
    assert!(
        !rows.iter().any(|r| r.page == SettingsPage::Hotkeys),
        "热键行已改为动态构建，静态目录不得再包含"
    );

    // 动态热键行：每个注册动作一行，标题 = 说明、描述 = 生效组合
    let app = Editpad::default();
    let hotkey_rows = app.rows_for(SettingsPage::Hotkeys);
    assert_eq!(hotkey_rows.len(), HOTKEY_ACTIONS.len());
    for (action, r) in HOTKEY_ACTIONS.iter().zip(&hotkey_rows) {
        assert_eq!(r.key, action.id, "热键行键必须与动作 id 一致");
        assert_eq!(r.title, action.desc.text(zh));
        // P320 改判：这里原先无条件断 `desc == default_combos.join(" / ")`，
        // 而 `default_combos: &[]` 的 15 条动作 join 出来就是**空串**——
        // 那条断言等于把"设置页一片空白"钉成了契约。现在两格分开钉。
        let defaults = action.default_combos.join(" / ");
        if defaults.trim().is_empty() {
            assert_eq!(
                r.desc,
                app.t(editpad_core::Key::HotkeyUnassigned),
                "没有默认键的动作也要说一句，不能留空行"
            );
        } else {
            assert_eq!(r.desc, defaults, "未重映射时描述 = 全部默认组合");
        }
    }

    // 控件覆盖：功能行必有控件；热键行有「修改」控件、关于为纯展示行
    for r in &rows {
        let control = app.settings_row_control(&r.key);
        match r.page {
            SettingsPage::About => {
                assert!(control.is_none(), "展示行 {} 不应有控件", r.key);
            }
            _ => {
                assert!(control.is_some(), "设置行 {} 缺右侧控件", r.key);
            }
        }
    }
    for r in &hotkey_rows {
        assert!(
            app.settings_row_control(&r.key).is_some(),
            "热键行 {} 缺「修改」控件",
            r.key
        );
    }

    // 字体挑选块挂载点：FONT_ROW_KEY 行存在于字体页（键语言无关、
    // 标题为该语言下「正文字体」的文案）
    assert!(rows.iter().any(|r| {
        r.page == SettingsPage::Font
            && r.key == FONT_ROW_KEY
            && r.title == editpad_core::Key::RowFontFamily.text(zh)
    }));
}

#[test]
fn hotkey_capture_flow_persists_rejects_conflict_and_resets() {
    let dir = scratch_dir("p62-capture");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());

    // 进入捕获 → 提交无冲突组合 → 写映射 + 持久化 + 退出捕获
    // （第 59 轮注：Ctrl+Shift+S/D/K 已成为排序/去重默认键，本测试
    //   改用仍无主的功能键演练重映射流程（P126 后 Ctrl+Shift+V
    //   已成为打开所在文件夹默认键）；P124 后 Ctrl+Shift+E
    //   已成为行序反转默认键；B9 后 F6 已成为列编辑器默认键；
    //   A8 后 F12 已成为在文件中查找默认键，演练键挪 F10）
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
    assert_eq!(app.hotkey_capture, Some("save"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert!(app.hotkey_capture.is_none());
    assert_eq!(
        app.settings.hotkeys.get("save").map(String::as_str),
        Some("F10")
    );
    assert_eq!(
        editpad_core::Settings::load_from(&config)
            .hotkeys
            .get("save")
            .map(String::as_str),
        Some("F10"),
        "重映射必须即时落盘"
    );

    // 重映射后分发走新组合（save → Ctrl+Shift+E；分发读活重映射表）
    let (mods, key) = parse_combo_for_test("F10");
    assert!(matches!(
        handle_key(key, mods, &app.settings.hotkeys),
        Some(Message::SaveRequested)
    ));
    let (mods, key) = parse_combo_for_test("Ctrl+S");
    assert!(
        handle_key(key, mods, &app.settings.hotkeys).is_none(),
        "旧组合已让出"
    );

    // 冲突：把「打开」绑到已被 save 占用的组合 → 拒绝并保持捕获态
    dispatch(&mut app, Message::HotkeyCaptureStarted("open"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert_eq!(app.hotkey_capture, Some("open"), "冲突保持捕获态");
    assert!(
        app.status.contains("占用"),
        "冲突提示应说明被谁占用：{}",
        app.status
    );
    assert!(
        !app.settings.hotkeys.contains_key("open"),
        "被拒动作不得写入映射"
    );
    dispatch(&mut app, Message::HotkeyCaptureCancel);
    assert!(app.hotkey_capture.is_none());

    // 全部恢复默认：清空映射 + 持久化（旧组合回归默认、新组合失效）
    dispatch(&mut app, Message::HotkeysResetAll);
    assert!(app.settings.hotkeys.is_empty());
    assert!(editpad_core::Settings::load_from(&config)
        .hotkeys
        .is_empty());
    let (mods, key) = parse_combo_for_test("Ctrl+S");
    assert!(
        matches!(handle_key_defaults(key, mods), Some(Message::SaveRequested)),
        "恢复默认后 Ctrl+S 回归保存"
    );
    let (mods, key) = parse_combo_for_test("F10");
    assert!(
        handle_key_defaults(key, mods).is_none(),
        "恢复默认后 F10 不再触发保存"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hotkey_capture_rejects_non_hotkey_keys_and_esc_cancels() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));

    // 无 Ctrl 的按键不可作热键：忽略（保持捕获态）
    dispatch(
        &mut app,
        Message::KeyPressed(
            keyboard::Key::Character("a".into()),
            keyboard::Modifiers::empty(),
        ),
    );
    assert_eq!(app.hotkey_capture, Some("save"));

    // Esc 取消
    dispatch(
        &mut app,
        Message::KeyPressed(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::empty(),
        ),
    );
    assert!(app.hotkey_capture.is_none());

    // 非法组合串（core 归一兜底）：捕获态提交非法串 → 不写入
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
    dispatch(&mut app, Message::HotkeyCaptureKey("alt+f4".into()));
    assert!(
        !app.settings.hotkeys.contains_key("save"),
        "非法组合不得写入映射"
    );
    dispatch(&mut app, Message::HotkeyCaptureCancel);
}

/// P320：设置页热键行不许出现空白描述。
///
/// 注册表里有 15 条 `default_combos: &[]` 的动作（P316 那批＋P318/P319 两条面板入口），
/// 改前它们的「当前组合」是 `default_combos.join(" / ")` ＝ **空串**——界面上
/// 既看不出"这条没键"还是"这里渲染坏了"，也不告诉人怎么给它一个键。
#[test]
fn no_hotkey_row_is_blank_and_the_unassigned_are_exactly_the_keyless_ones() {
    let app = Editpad::default();
    let rows = app.rows_for(SettingsPage::Hotkeys);
    let blanks: Vec<&str> = rows
        .iter()
        .filter(|r| r.desc.trim().is_empty())
        .map(|r| r.key.as_str())
        .collect();
    assert!(
        blanks.is_empty(),
        "设置热键页不许有空白行，实得 {} 条：{:?}",
        blanks.len(),
        blanks
    );
    // 非空自证：这 15 条确实存在，否则上面那条断言会因"没有空白行可言"而恒真
    let keyless: Vec<&str> = HOTKEY_ACTIONS
        .iter()
        .filter(|a| a.default_combos.is_empty())
        .map(|a| a.id)
        .collect();
    assert!(
        keyless.len() >= 15,
        "用例前提：注册表里要有无默认键的动作可数，实得 {} 条",
        keyless.len()
    );
    let hint = app.t(editpad_core::Key::HotkeyUnassigned);
    let shown_hint: Vec<&str> = rows
        .iter()
        .filter(|r| r.desc == hint)
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(
        shown_hint, keyless,
        "显式说「未赋值」的那些必须恰好是注册表里没默认键的——多一条＝有键的被说成没键，\
         少一条＝有动作在冒充未赋值"
    );
}

#[test]
fn assigning_a_key_to_a_keyless_action_flips_that_row_off_the_hint() {
    let mut app = Editpad::default();
    let desc = |app: &Editpad| {
        app.rows_for(SettingsPage::Hotkeys)
            .into_iter()
            .find(|r| r.key == "clip_history")
            .expect("clip_history 在热键页有一行")
            .desc
    };
    let hint = app.t(editpad_core::Key::HotkeyUnassigned).to_owned();
    assert_eq!(
        desc(&app),
        hint,
        "用例前提：P319 那条动作没有默认键 ⇒ 行上显示「未赋值」"
    );
    dispatch(&mut app, Message::HotkeyCaptureStarted("clip_history"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert_eq!(
        app.settings.hotkeys.get("clip_history").map(String::as_str),
        Some("F10")
    );
    assert_eq!(
        desc(&app),
        "F10",
        "赋完键行上要立刻显出这个键，不能再停在提示语——否则用户看不出刚按的键生效了"
    );
    // 反向一格：非法组合提交失败时，行上留住的仍是上一条生效值
    dispatch(&mut app, Message::HotkeyCaptureStarted("clip_history"));
    dispatch(&mut app, Message::HotkeyCaptureKey("alt+f4".into()));
    assert_eq!(
        desc(&app),
        "F10",
        "非法组合不该把行改回「未赋值」，也不该留个空串"
    );
}

/// P323：冲突校验必须按**全部生效键位**判，与运行期匹配同一份判据。
///
/// 「重做」有两个同义默认键（`Ctrl+Y` / `Ctrl+Shift+Z`），而改前的校验只比
/// **首个** ⇒ 把 `Ctrl+Shift+Z` 派给别的动作会被放行；按下时
/// `effective_action` 先扫重映射表，新动作赢，重做就此少了这个同义键——
/// 用户拿到的是一次"成功"的赋键加一次静默降级。
#[test]
fn a_second_synonym_default_key_still_belongs_to_its_action() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::HotkeyCaptureStarted("clip_history"));
    dispatch(&mut app, Message::HotkeyCaptureKey("Ctrl+Shift+Z".into()));
    assert!(
        !app.settings.hotkeys.contains_key("clip_history"),
        "Ctrl+Shift+Z 是「重做」的第二个同义默认键 ⇒ 该算冲突（改前只比首个，这一格会被放行）"
    );
    assert!(
        app.status.contains("Ctrl+Shift+Z"),
        "冲突提示要说出抢的是哪个键，实得 {:?}",
        app.status
    );
    assert_eq!(
        app.hotkey_capture,
        Some("clip_history"),
        "被拒后仍留在捕获态（用户可以直接再按一个别的键）"
    );
}

#[test]
fn the_first_synonym_default_key_is_still_a_conflict_too() {
    // 反向对照：改成按集合判之后，本来拦得住的那一头不能被改松
    let mut app = Editpad::default();
    dispatch(&mut app, Message::HotkeyCaptureStarted("clip_history"));
    dispatch(&mut app, Message::HotkeyCaptureKey("Ctrl+Y".into()));
    assert!(
        !app.settings.hotkeys.contains_key("clip_history"),
        "Ctrl+Y 是「重做」的主默认键 ⇒ 照旧拦"
    );
}

#[test]
fn a_remapped_key_conflicts_and_the_vacated_default_frees_up() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert_eq!(
        app.settings.hotkeys.get("save").map(String::as_str),
        Some("F10"),
        "前提：先给「保存」派一个注册表里无主的键"
    );
    dispatch(&mut app, Message::HotkeyCaptureStarted("clip_history"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F10".into()));
    assert!(
        !app.settings.hotkeys.contains_key("clip_history"),
        "F10 已被重映射占用 ⇒ 拦"
    );
    // 同一份判据的另一头：重映射过的动作不再认领自己的默认键
    dispatch(&mut app, Message::HotkeyCaptureKey("Ctrl+S".into()));
    assert_eq!(
        app.settings.hotkeys.get("clip_history").map(String::as_str),
        Some("Ctrl+S"),
        "「保存」已让出 Ctrl+S ⇒ 现在派给别人不该算冲突"
    );
}

#[test]
fn an_unowned_key_is_acceptable_and_rebinding_the_same_key_to_the_same_action_is_not_a_conflict() {
    // ①没有主的键（F4 是面板手势，不入注册表）该放行
    let mut app = Editpad::default();
    dispatch(&mut app, Message::HotkeyCaptureStarted("bookmark_panel"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F4".into()));
    assert_eq!(
        app.settings
            .hotkeys
            .get("bookmark_panel")
            .map(String::as_str),
        Some("F4"),
        "F4 在注册表里无主 ⇒ 不该被拦"
    );
    // ⚠️ 这里不能用 tests/mod.rs 的 `handle_key_defaults`——它喂的是**空**重映射表，
    // 看不见刚赋的键；本格要验的正是"派完键之后按下真的通到那条动作"。
    assert!(
        matches!(
            crate::hotkeys::handle_key(
                keyboard::Key::Named(keyboard::key::Named::F4),
                keyboard::Modifiers::empty(),
                &app.settings.hotkeys
            ),
            Some(Message::BookmarksToggled)
        ),
        "赋完键之后 F4 这条路要真的通到那条动作（注册表→handle_key 全链路）"
    );
    // ②把同一个键再派给同一个动作＝自我覆盖，不该被当成冲突
    dispatch(&mut app, Message::HotkeyCaptureStarted("bookmark_panel"));
    dispatch(&mut app, Message::HotkeyCaptureKey("F4".into()));
    assert_eq!(
        app.settings
            .hotkeys
            .get("bookmark_panel")
            .map(String::as_str),
        Some("F4"),
        "自己不算自己的冲突（判据里 a.id != id 那一格）"
    );
}

#[test]
fn font_size_delta_clamps_and_persists_to_injected_path() {
    // P48：工具栏 A-/A+ 移除后，FontSizeDelta 的入口 = 设置面板步进
    // 与 Ctrl+滚轮（共用 FONT_ZOOM_STEP）——clamp 与落盘契约不变
    let dir = scratch_dir("p48-zoom");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());

    dispatch(
        &mut app,
        Message::FontSizeDelta(crate::editor::FONT_ZOOM_STEP),
    );
    assert_eq!(app.settings.font_size, 18.0);
    assert_eq!(
        app.cur_handle.borrow().font_size(),
        18.0,
        "当前页必须立即生效（P36 口径：只调正文）"
    );

    // 越界方向被夹在边界上（Ctrl+滚轮连滚不越界）
    for _ in 0..80 {
        dispatch(
            &mut app,
            Message::FontSizeDelta(crate::editor::FONT_ZOOM_STEP),
        );
    }
    assert_eq!(
        app.settings.font_size,
        editpad_core::settings::MAX_FONT_SIZE
    );
    for _ in 0..300 {
        dispatch(
            &mut app,
            Message::FontSizeDelta(-crate::editor::FONT_ZOOM_STEP),
        );
    }
    assert_eq!(
        app.settings.font_size,
        editpad_core::settings::MIN_FONT_SIZE
    );

    assert_eq!(
        editpad_core::Settings::load_from(&config).font_size,
        app.settings.font_size,
        "缩放必须即时落盘"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------- P56/P57 全局中性样式 ----------

#[test]
fn chrome_styles_are_neutral_and_status_aware() {
    let bg_of = |s: &button::Style| match s.background {
        Some(Background::Color(c)) => Some(c),
        _ => None,
    };
    for (label, theme) in [("浅色", Theme::Light), ("深色", Theme::Dark)] {
        // 按钮：悬停必须有视觉反馈、禁用必须降灰（不再是实心蓝底白字）
        let active = chrome_button_style(&theme, button::Status::Active);
        let hovered = chrome_button_style(&theme, button::Status::Hovered);
        let disabled = chrome_button_style(&theme, button::Status::Disabled);
        assert_ne!(
            bg_of(&active),
            bg_of(&hovered),
            "{label}: 悬停必须有视觉反馈"
        );
        assert_ne!(
            active.text_color, disabled.text_color,
            "{label}: 禁用必须降灰"
        );
        assert_ne!(
            active.border.color, disabled.border.color,
            "{label}: 禁用描边应弱化"
        );

        // 菜单项：常态透明融入卡片、悬停淡染（不再像全选中的高亮条）
        let item = chrome_menu_item_style(&theme, button::Status::Active);
        let item_hover = chrome_menu_item_style(&theme, button::Status::Hovered);
        assert_eq!(
            bg_of(&item),
            Some(Color::TRANSPARENT),
            "{label}: 菜单项常态透明"
        );
        assert_ne!(
            bg_of(&item),
            bg_of(&item_hover),
            "{label}: 菜单项悬停必须有淡染"
        );
        let item_disabled = chrome_menu_item_style(&theme, button::Status::Disabled);
        assert_ne!(
            item.text_color, item_disabled.text_color,
            "{label}: 菜单项禁用必须降灰"
        );

        // 页签：活动页淡底 + 描边，非活动透明无边
        let tab_active = chrome_nav_button_style(&theme, button::Status::Active, true);
        let tab_idle = chrome_nav_button_style(&theme, button::Status::Active, false);
        assert!(bg_of(&tab_active).is_some(), "{label}: 活动页签应有底色");
        assert!(bg_of(&tab_idle).is_none(), "{label}: 非活动页签应透明");
        assert_ne!(
            tab_active.border.color, tab_idle.border.color,
            "{label}: 活动页签应有描边指示"
        );

        // P112 页签胶囊样式：胶囊底画在容器上（活动底+描边 / 悬停
        // 淡染 / 闲置透明），内部「文字」「×」按钮无底扁平
        let pill_active = tab_pill_style(&theme, true, false);
        let pill_hover = tab_pill_style(&theme, false, true);
        let pill_idle = tab_pill_style(&theme, false, false);
        assert!(
            pill_active.background.is_some(),
            "{label}: 活动胶囊应有底色"
        );
        assert!(pill_hover.background.is_some(), "{label}: 悬停胶囊应有淡染");
        assert!(pill_idle.background.is_none(), "{label}: 闲置胶囊应透明");
        assert_ne!(
            pill_active.border.color, pill_idle.border.color,
            "{label}: 活动胶囊应有描边指示"
        );
        assert_eq!(
            pill_active.border.radius, pill_idle.border.radius,
            "{label}: 胶囊圆角应恒定"
        );
        let tab_label_btn = tab_label_style(&theme, button::Status::Active);
        let tab_label_off = tab_label_style(&theme, button::Status::Disabled);
        assert!(
            bg_of(&tab_label_btn).is_none(),
            "{label}: 页签文字按钮必须无底"
        );
        assert_ne!(
            tab_label_btn.text_color, tab_label_off.text_color,
            "{label}: 页签文字禁用降灰"
        );
        let tab_close = tab_close_style(&theme, button::Status::Active);
        let tab_close_hover = tab_close_style(&theme, button::Status::Hovered);
        let tab_close_off = tab_close_style(&theme, button::Status::Disabled);
        assert_ne!(
            bg_of(&tab_close),
            bg_of(&tab_close_hover),
            "{label}: × 悬停必须有芯片感"
        );
        assert_ne!(
            tab_close.text_color, tab_close_off.text_color,
            "{label}: × 禁用降灰"
        );
    }
}

#[test]
fn settings_card_size_adapts_to_viewport() {
    // 大窗：宽封顶 720，内容高封顶 680
    assert_eq!(settings_card_size(1600.0, 1000.0), (720.0, 680.0));
    // 中窗：高度跟随 vh−96
    assert_eq!(settings_card_size(1000.0, 700.0), (720.0, 604.0));
    // 窄窗：宽度收缩到 vw−32，永不出窗
    assert_eq!(settings_card_size(500.0, 700.0), (468.0, 604.0));
    // 窗口尺寸未知：保守默认
    assert_eq!(settings_card_size(0.0, 0.0), (720.0, 560.0));
    // 极小窗：高度走保守默认，宽度仍收缩
    assert_eq!(settings_card_size(300.0, 100.0), (268.0, 560.0));

    // 不变式：任意尺寸下卡片宽 ≤ 视口宽（已知时）、内容高 ≤ vh−96
    for (vw, vh) in [
        (800.0, 600.0),
        (1024.0, 768.0),
        (1920.0, 1080.0),
        (360.0, 500.0),
    ] {
        let (w, h) = settings_card_size(vw, vh);
        assert!(w <= vw - 32.0 + 0.5, "{vw}x{vh}: 宽 {w} 出窗");
        assert!(h <= vh - 96.0 + 0.5, "{vw}x{vh}: 高 {h} 出窗");
    }
}

// ---------- P34 字体选择 ----------

#[test]
fn effective_font_family_resolution_contract() {
    let available = vec![
        "Microsoft YaHei UI".to_owned(),
        "NSimSun".to_owned(),
        "Sarasa Mono SC".to_owned(),
    ];

    // 未配置 → 默认等宽（None）
    assert_eq!(effective_font_family(None, &available), None);

    // 精确命中 → 原样生效
    assert_eq!(
        effective_font_family(Some("NSimSun"), &available),
        Some("NSimSun".to_owned())
    );

    // 大小写/空白变体 → 宽松命中并矫正为清单规范名
    assert_eq!(
        effective_font_family(Some("nsimsun"), &available),
        Some("NSimSun".to_owned())
    );
    assert_eq!(
        effective_font_family(Some(" SarasaMono sc "), &available),
        Some("Sarasa Mono SC".to_owned())
    );

    // 未安装 → 回退默认，不臆造名字
    assert_eq!(
        effective_font_family(Some("不存在的字体"), &available),
        None
    );
    // 空清单（枚举失败的降级环境）→ 一律默认
    assert_eq!(effective_font_family(Some("NSimSun"), &[]), None);
}

#[test]
fn resolve_startup_font_prompts_only_for_configured_but_missing() {
    let available = vec!["NSimSun".to_owned()];

    // 未配置：无提示
    assert_eq!(resolve_startup_font(None, &available), (None, false));
    // 配置且可用（含变体矫正）：生效但不提示
    assert_eq!(
        resolve_startup_font(Some("nsimsun"), &available),
        (Some("NSimSun".to_owned()), false)
    );
    // 配置了未安装的字体：回退 + 提示一次
    assert_eq!(resolve_startup_font(Some("Foo"), &available), (None, true));
    // 空清单降级环境：配置了就提示
    assert_eq!(resolve_startup_font(Some("Foo"), &[]), (None, true));
}

#[test]
fn font_selection_updates_active_font_and_persists_to_injected_path() {
    let dir = scratch_dir("p34-app");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    app.available_fonts = vec!["Arial".to_owned(), "Sarasa Mono SC".to_owned()];

    // 未选择时 = 默认等宽（P33 钉字语义不变）
    assert_eq!(app.body_font(), crate::editor::BODY_FONT);

    // 选择系统字体：设置与生效族名同步更新并落盘
    dispatch(&mut app, Message::SettingsFontSelected("Arial".to_owned()));
    assert_eq!(app.settings.font_family.as_deref(), Some("Arial"));
    assert_eq!(app.active_font_family, Some("Arial"));
    assert_eq!(
        app.body_font().family,
        iced::font::Family::Name("Arial"),
        "正文与 UI 的换装点必须切到所选族"
    );
    assert_eq!(
        editpad_core::Settings::load_from(&config)
            .font_family
            .as_deref(),
        Some("Arial"),
        "选择必须即时写回注入路径的 config.toml"
    );

    // 回退默认：设置清空、渲染回到 BODY_FONT、落盘为空
    dispatch(&mut app, Message::SettingsFontReset);
    assert_eq!(app.settings.font_family, None);
    assert_eq!(app.active_font_family, None);
    assert_eq!(app.body_font(), crate::editor::BODY_FONT);
    assert_eq!(
        editpad_core::Settings::load_from(&config).font_family,
        None,
        "回退默认后 config.toml 不应再留字体名"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------- P154：UI 字体（按语言）+ 行号字体 + 正文解耦 ----------

/// 候选表按语言命中 + 宽松匹配（大小写/空白）+ 全不命中回落 None。
#[test]
fn p154_ui_and_gutter_font_picking_contract() {
    let zh = vec![
        "Microsoft YaHei".to_owned(),
        "Consolas".to_owned(),
        "Noto Sans".to_owned(),
    ];
    assert_eq!(
        pick_ui_font_family(editpad_core::Lang::ZhCn, &zh),
        Some("Microsoft YaHei"),
        "中文简体应命中雅黑系"
    );
    assert_eq!(
        pick_ui_font_family(editpad_core::Lang::ZhCn, &zh),
        pick_ui_font_family(editpad_core::settings::normalize_language("bogus"), &zh),
        "未知语言回落中文简体（同一候选表）"
    );
    // 英文候选全不在清单里 → None（调用方回落 Font::DEFAULT，不失败）
    assert_eq!(pick_ui_font_family(editpad_core::Lang::En, &zh), None);
    let en = vec!["Segoe UI".to_owned(), "Tahoma".to_owned()];
    assert_eq!(
        pick_ui_font_family(editpad_core::Lang::En, &en),
        Some("Segoe UI"),
        "English 应命中 Segoe UI 系"
    );
    // 宽松匹配：大小写/空白变体也算命中
    let loose = vec!["microsoft  yahei  ui".to_owned()];
    assert_eq!(
        pick_ui_font_family(editpad_core::Lang::ZhCn, &loose),
        Some("Microsoft YaHei UI"),
        "候选匹配应忽略大小写与空白（复用 normalize_family）"
    );

    // 行号位：等宽候选链按顺序命中，全不命中 → None
    assert_eq!(
        pick_gutter_font_family(&["Consolas".to_owned()]),
        Some("Consolas")
    );
    assert_eq!(
        pick_gutter_font_family(&["Cascadia Mono".to_owned()]),
        Some("Cascadia Mono"),
        "首选缺失时顺位下移"
    );
    assert_eq!(
        pick_gutter_font_family(&["Consolas".to_owned(), "Courier New".to_owned()]),
        Some("Consolas"),
        "候选顺序优先 Consolas"
    );
    assert_eq!(pick_gutter_font_family(&["Arial".to_owned()]), None);
    assert_eq!(pick_gutter_font_family(&[]), None);
}

/// 用户点单的核心契约：**改正文只影响正文**（UI 与行号不动），
/// **切语言只影响 UI**（正文与行号不动）。
#[test]
fn p154_body_ui_and_gutter_fonts_are_decoupled() {
    let dir = scratch_dir("p154-decouple");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    app.available_fonts = vec![
        "Arial".to_owned(),
        "Microsoft YaHei".to_owned(),
        "Segoe UI".to_owned(),
        "Consolas".to_owned(),
    ];
    // 启动期解析语义：中文界面 → 雅黑；行号 → Consolas
    app.ui_font_family = pick_ui_font_family(editpad_core::Lang::ZhCn, &app.available_fonts);
    app.gutter_font_family = pick_gutter_font_family(&app.available_fonts);
    assert_eq!(app.ui_font_family, Some("Microsoft YaHei"));
    assert_eq!(app.gutter_font_family, Some("Consolas"));

    // 改正文：正文换族，UI 与行号**不受影响**
    dispatch(&mut app, Message::SettingsFontSelected("Arial".to_owned()));
    assert_eq!(
        app.body_font().family,
        iced::font::Family::Name("Arial"),
        "正文应切到所选族"
    );
    assert_eq!(
        app.ui_font().family,
        iced::font::Family::Name("Microsoft YaHei"),
        "改正文不得影响 UI 字体（P154 解耦契约）"
    );
    assert_eq!(app.gutter_font_family, Some("Consolas"), "行号字体不受影响");

    // 切语言：UI 换族，正文与行号**不受影响**，且即时落盘
    dispatch(&mut app, Message::LanguageToggled);
    assert_eq!(app.settings.language, editpad_core::Lang::En);
    assert_eq!(
        app.ui_font().family,
        iced::font::Family::Name("Segoe UI"),
        "English 应切到 Segoe UI 系"
    );
    assert_eq!(
        app.body_font().family,
        iced::font::Family::Name("Arial"),
        "切语言不得影响正文字体（P154 解耦契约）"
    );
    assert_eq!(
        editpad_core::Settings::load_from(&config).language,
        editpad_core::Lang::En,
        "切语言必须即时写回 config.toml"
    );

    // 再切一次回到中文（循环两态）
    dispatch(&mut app, Message::LanguageToggled);
    assert_eq!(app.settings.language, editpad_core::Lang::ZhCn);
    assert_eq!(
        app.ui_font().family,
        iced::font::Family::Name("Microsoft YaHei")
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------- P155：界面文案 i18n + 语言下拉框 ----------

/// 用户点单的核心行为：设置里选 English → **界面文案**真的变英文
/// （不只是换字体）——菜单名、设置分类、行标题/描述、状态栏提示
/// 全部按新语言取文，且即时落盘。
#[test]
fn p155_language_selection_switches_ui_text_and_persists() {
    let dir = scratch_dir("p155-i18n");
    let config = dir.join("config.toml");
    let mut app = Editpad::default();
    app.settings_path_override = Some(config.clone());
    app.available_fonts = vec![
        "Microsoft YaHei".to_owned(),
        "Segoe UI".to_owned(),
        "Consolas".to_owned(),
    ];
    app.ui_font_family = pick_ui_font_family(editpad_core::Lang::ZhCn, &app.available_fonts);

    // 中文界面（默认）：菜单与设置行都是中文
    assert_eq!(app.lang(), editpad_core::Lang::ZhCn);
    assert_eq!(app.t(editpad_core::Key::MenuFile), "文件");
    assert_eq!(app.t(editpad_core::Key::Settings), "设置");
    let zh_rows: Vec<SettingsRow> = app.rows_for(SettingsPage::Appearance);
    assert!(
        zh_rows.iter().any(|r| r.title == "界面语言"),
        "中文界面下应显示中文行标题：{zh_rows:?}"
    );

    // 下拉框选中 English（pick_list 的选中消息 = LangOption）
    dispatch(
        &mut app,
        Message::LanguageOptionSelected(editpad_core::LangOption {
            lang: editpad_core::Lang::En,
            ui: editpad_core::Lang::ZhCn,
        }),
    );
    assert_eq!(app.lang(), editpad_core::Lang::En, "语言应切到英文");
    assert_eq!(app.t(editpad_core::Key::MenuFile), "File");
    assert_eq!(app.t(editpad_core::Key::Settings), "Settings");
    let en_rows: Vec<SettingsRow> = app.rows_for(SettingsPage::Appearance);
    assert!(
        en_rows.iter().any(|r| r.title == "Interface Language"),
        "英文界面下行标题应换英文：{en_rows:?}"
    );
    // 行键**语言无关**：两种语言下行键集合完全一致（控件匹配不失配）
    let mut zh_keys: Vec<&str> = zh_rows.iter().map(|r| r.key.as_str()).collect();
    let mut en_keys: Vec<&str> = en_rows.iter().map(|r| r.key.as_str()).collect();
    zh_keys.sort_unstable();
    en_keys.sort_unstable();
    assert_eq!(zh_keys, en_keys, "切语言不得改变行键");

    // 热键页同样换语言（标题 = 动作说明）
    let hk_en = app.rows_for(SettingsPage::Hotkeys);
    assert_eq!(hk_en[0].title, "Open a file", "热键说明也要翻译");

    // 状态栏反馈用**切换后**的语言书写（不留中英混排）
    assert_eq!(app.status, "Interface language switched (Segoe UI)");
    assert_eq!(
        editpad_core::Settings::load_from(&config).language,
        editpad_core::Lang::En,
        "语言选择必须即时落盘"
    );

    // 选同一语言 = no-op（不刷状态栏）
    app.status.clear();
    dispatch(
        &mut app,
        Message::LanguageOptionSelected(editpad_core::LangOption {
            lang: editpad_core::Lang::En,
            ui: editpad_core::Lang::En,
        }),
    );
    assert!(app.status.is_empty(), "重复选中不应产生状态栏噪声");

    // 英文界面下弹窗几何不变（换文案不应破坏布局）
    dispatch(&mut app, Message::SettingsToggled);
    assert_settings_card_centered(&app);

    std::fs::remove_dir_all(&dir).ok();
}

/// P155：设置搜索按**当前语言**过滤——切英文后中文关键词失效、
/// 英文关键词命中（数据源与渲染同源，不留两套文案）。
#[test]
fn p155_settings_search_follows_current_language() {
    let zh = editpad_core::Lang::ZhCn;
    let en = editpad_core::Lang::En;
    let zh_hit = settings_rows(zh).any(|r| settings_search_hit("语言", &r.title, &r.desc));
    let en_hit = settings_rows(en).any(|r| settings_search_hit("language", &r.title, &r.desc));
    assert!(zh_hit, "中文关键词应命中中文行");
    assert!(en_hit, "英文关键词应命中英文行");
    assert!(
        !settings_rows(en).any(|r| settings_search_hit("语言", &r.title, &r.desc)),
        "英文界面下不该再被中文关键词命中"
    );
}

/// P155：语言下拉框的选项覆盖全部支持语言（加语言只改 core 的
/// `Lang`，此处零改动即自动跟随）。
#[test]
fn p155_language_dropdown_covers_every_supported_language() {
    let opts = editpad_core::LangOption::all(editpad_core::Lang::ZhCn);
    assert_eq!(opts.len(), editpad_core::Lang::ALL.len());
    let labels: Vec<String> = opts.iter().map(|o| o.to_string()).collect();
    assert!(labels.contains(&"中文（简体）".to_owned()));
    assert!(labels.contains(&"English".to_owned()));
    // 下拉框当前选中项必须能在选项列表里找到（否则会显示成占位符）
    for lang in editpad_core::Lang::ALL {
        let selected = editpad_core::LangOption { lang, ui: lang };
        assert!(
            opts.contains(&selected),
            "{lang:?} 的选中项必须存在于选项中"
        );
    }
}

/// 候选全不命中时的降级：UI 回落 iced 默认、行号回落正文字体，
/// 都不 panic、不引入失败模式。
#[test]
fn p154_font_picking_degrades_gracefully_when_no_candidate() {
    let mut app = Editpad::default();
    app.available_fonts = vec!["Some Random Font".to_owned()];
    app.ui_font_family = pick_ui_font_family(editpad_core::Lang::En, &app.available_fonts);
    app.gutter_font_family = pick_gutter_font_family(&app.available_fonts);
    assert!(app.ui_font_family.is_none());
    assert!(app.gutter_font_family.is_none());
    assert_eq!(app.ui_font(), Font::DEFAULT, "UI 回落 iced 默认字形族");
    // 字体候选全落空时，正文仍须正常布局（回落不得让编辑器塌掉）
    let body = ViewTree::layout_default(&app).editor_body();
    assert!(body.height > 150.0, "字体回落后正文仍应占据正常高度");
}

#[test]
fn font_selection_via_loose_match_canonicalizes_name() {
    // 手改 config 的变体名经统一解析收敛到规范名（与启动期同一条路径）
    let mut app = Editpad::default();
    app.available_fonts = vec!["Noto Sans Mono CJK SC".to_owned()];
    dispatch(
        &mut app,
        Message::SettingsFontSelected("noto sans mono cjk sc".to_owned()),
    );
    assert_eq!(
        app.active_font_family,
        Some("Noto Sans Mono CJK SC"),
        "生效值必须是清单里的规范族名"
    );
    assert_eq!(
        app.settings.font_family.as_deref(),
        Some("noto sans mono cjk sc"),
        "core 层只做损坏值裁剪，不负责大小写矫正"
    );
}

#[test]
fn font_filter_is_pure_ui_state_and_never_touches_settings() {
    let mut app = Editpad::default();
    dispatch(&mut app, Message::FontFilterChanged("mono".to_owned()));
    assert_eq!(app.font_filter, "mono");
    assert_eq!(app.settings.font_family, None, "过滤词不落盘不改配置");

    // Esc 关浮动栏不应清掉弹窗内输入态（重开时保留上次过滤词属可接受行为，
    // 这里钉住的是「过滤不影响设置」这一核心契约）
    dispatch(&mut app, Message::BarsDismissed);
    assert_eq!(app.settings.font_family, None);
}

#[test]
fn home_end_with_alt_route_to_logical_motion() {
    // P134（C8）：Alt 系不入注册表（AltGr 保护契约，见 hotkeys.rs）——
    // 裸命名键通道按 mods.alt() 分流到逻辑行变体
    use iced::keyboard::{key::Named, Key};
    let alt = iced::keyboard::Modifiers::ALT;
    let none = iced::keyboard::Modifiers::empty();
    let remap = std::collections::HashMap::new();
    assert!(matches!(
        handle_key(Key::Named(Named::Home), alt, &remap),
        Some(Message::Edit(crate::editor::EditOp::Motion(
            crate::editor::Motion::LogicalHome,
            false
        )))
    ));
    assert!(matches!(
        handle_key(Key::Named(Named::End), alt, &remap),
        Some(Message::Edit(crate::editor::EditOp::Motion(
            crate::editor::Motion::LogicalEnd,
            false
        )))
    ));
    // 无 Alt 恒 Home/End
    assert!(matches!(
        handle_key(Key::Named(Named::Home), none, &remap),
        Some(Message::Edit(crate::editor::EditOp::Motion(
            crate::editor::Motion::Home,
            false
        )))
    ));
}

#[test]
fn space_key_inserts_space_regardless_of_ime_state() {
    // P166：空格以 Named(Space) 形态上报（winit VK_SPACE → NamedKey::
    // Space，不经 Character）——缺分支 = 空格永远插不进文档：IME 关闭
    // 态直接打空格、组字上屏后补空格分隔均吞键（用户复报）。全角空格
    // 走 Ime::Commit 另一路径，不在此测。
    use iced::keyboard::{key::Named, Key};
    let remap = std::collections::HashMap::new();
    assert!(matches!(
        handle_key(
            Key::Named(Named::Space),
            iced::keyboard::Modifiers::empty(),
            &remap
        ),
        Some(Message::Edit(crate::editor::EditOp::InsertText(text)))
        if text == " "
    ));
    // Shift+空格（部分输入法的全半角切换残留 Shift 时）仍插半角空格
    assert!(matches!(
        handle_key(
            Key::Named(Named::Space),
            iced::keyboard::Modifiers::SHIFT,
            &remap
        ),
        Some(Message::Edit(crate::editor::EditOp::InsertText(text)))
        if text == " "
    ));
}
