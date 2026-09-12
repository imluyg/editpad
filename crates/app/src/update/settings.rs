//! 设置域消息处理（自 update.rs 拆出，纯移动零行为变更）。
//!
//! 承载设置弹窗各行控件、界面语言与字体切换、设置落盘。

use super::*;

impl Editpad {
    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：设置/字体/热键。臂体自原 update() 逐字搬移，零行为变更。
    pub(super) fn update_settings(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::SettingsShowWhitespaceToggled(value) => {
                self.settings.show_whitespace = value;
                for tab in &self.tabs {
                    tab.editor
                        .borrow_mut()
                        .set_invisibles(value, self.settings.show_line_endings);
                }
                self.persist_settings();
                Task::none()
            }
            Message::SettingsShowLineEndingsToggled(value) => {
                self.settings.show_line_endings = value;
                for tab in &self.tabs {
                    tab.editor
                        .borrow_mut()
                        .set_invisibles(self.settings.show_whitespace, value);
                }
                self.persist_settings();
                Task::none()
            }
            // 第 73 轮 ⑯：自动换行开关——全局默认变更只影响「跟随全局」
            // 的页（P134：有本页覆盖的页保持其覆盖，C7）
            Message::SettingsWordWrapToggled(value) => {
                self.settings.word_wrap = value;
                for tab in &self.tabs {
                    let effective = tab.wrap_override.unwrap_or(value);
                    tab.editor.borrow_mut().set_word_wrap(effective);
                }
                self.persist_settings();
                Task::none()
            }
            // P132：缩进参考线 / 右缘标尺——全标签页即时生效（纯绘制开关）
            Message::SettingsIndentGuidesToggled(value) => {
                self.settings.indent_guides = value;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().set_indent_guides(value);
                }
                self.persist_settings();
                Task::none()
            }
            Message::SettingsEdgeColumnDelta(delta) => {
                let next = (self.settings.edge_column as i32 + delta)
                    .clamp(0, editpad_core::settings::MAX_EDGE_COLUMN as i32)
                    as u32;
                self.settings.edge_column = next;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().set_edge_column(next);
                }
                self.persist_settings();
                Task::none()
            }
            Message::HotkeyCaptureStarted(id) => {
                self.hotkey_capture = Some(id);
                let lang = self.lang();
                let name = HOTKEY_ACTIONS
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.desc.text(lang))
                    .unwrap_or("");
                self.set_status(editpad_core::fmt_wrapped(
                    lang,
                    editpad_core::Key::StHotkeyCapturePromptPrefix,
                    name,
                    editpad_core::Key::StHotkeyCapturePromptSuffix,
                ));
                Task::none()
            }
            Message::HotkeyCaptureKey(combo) => self.commit_hotkey_capture(combo),
            Message::HotkeyCaptureCancel => {
                self.hotkey_capture = None;
                self.status.clear();
                Task::none()
            }
            Message::HotkeysResetAll => {
                self.settings.hotkeys.clear();
                self.persist_settings();
                self.hotkey_capture = None;
                self.set_status(self.t(editpad_core::Key::StHotkeysDefault).to_owned());
                Task::none()
            }
            // ---------- 外观 ----------
            Message::ThemeToggled => {
                self.dark_mode = !self.dark_mode;
                self.settings.set_theme(self.dark_mode);
                self.persist_settings();
                // 全部标签页的语法高亮随主题换档（新代次作废在途补建，
                // 下一帧按新主题重铺/重算可视区配色）
                let dark = self.dark_mode;
                for tab in &self.tabs {
                    tab.editor.borrow_mut().apply_highlight_theme(dark);
                }
                Task::none()
            }
            // P155：界面语言选择（下拉框）——与 P154 的循环按钮同一条链路，
            // 只多一层「目标语言」入参。三件事一起发生：界面**文案**换语言
            // （view 层每帧按 `Settings.language` 取文，无需额外失效）、
            // UI 字体族重解析、设置落盘。已选中同一语言 = no-op。
            Message::LanguageOptionSelected(opt) => {
                if opt.lang == self.settings.language {
                    return Task::none();
                }
                self.apply_language(opt.lang);
                Task::none()
            }
            // P154 兼容入口：循环切换（下拉框上线后仅测试路径使用）
            Message::LanguageToggled => {
                let cur = self.settings.language;
                let next = editpad_core::Lang::ALL
                    .iter()
                    .copied()
                    .cycle()
                    .skip_while(|l| *l != cur)
                    .nth(1)
                    .unwrap_or(editpad_core::Lang::ZhCn);
                self.apply_language(next);
                Task::none()
            }
            // P134（C7）：全局默认字号（设置步进器入口）——「跟随全局」
            // 的页全部生效；有本页覆盖的页保持其覆盖
            Message::FontSizeDelta(delta) => {
                let next = editor::normalize_font_size(self.display_font_size() + delta);
                self.settings.font_size = next;
                self.persist_settings();
                for tab in &self.tabs {
                    if tab.font_size_override.is_none() {
                        tab.editor.borrow_mut().set_font_size(next);
                    }
                }
                Task::none()
            }
            // ---------- P134：每页显示覆盖（C7） ----------
            Message::TabFontSizeDelta(delta) => {
                // Ctrl+滚轮：当前页覆盖字号——不动全局默认、不落盘
                //（随会话快照保存）；normalize 与全局链路同一 clamp 规则
                let cur = self
                    .tab()
                    .font_size_override
                    .unwrap_or(self.settings.font_size);
                let next = editor::normalize_font_size(cur + delta);
                let tab = self.tab_mut();
                tab.font_size_override = Some(next);
                tab.editor.borrow_mut().set_font_size(next);
                self.set_status(format!(
                    "{}{next:.0}{}{:.0}{}",
                    self.t(
editpad_core::Key::
StTabFontSizePrefix),
                    self.t(
editpad_core::Key::
StTabFontSizeMiddle),
                    self.settings.font_size,
                    self.t(
editpad_core::Key::
StTabFontSizeSuffix)
                ));
                Task::none()
            }
            Message::TabFontSizeReset => {
                let global = editor::normalize_font_size(self.settings.font_size);
                let tab = self.tab_mut();
                tab.font_size_override = None;
                tab.editor.borrow_mut().set_font_size(global);
                self.set_status(format!("{}{global:.0}", self.t(editpad_core::Key::StTabFontReset)));
                Task::none()
            }
            Message::TabWrapOverrideToggled => {
                // 三态循环：跟随全局 → 本页开 → 本页关 → 跟随全局
                let next = match self.tab().wrap_override {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
                let effective = next.unwrap_or(self.settings.word_wrap);
                let tab = self.tab_mut();
                tab.wrap_override = next;
                tab.editor.borrow_mut().set_word_wrap(effective);
                Task::none()
            }
            // ---------- 设置弹窗（P27） ----------
            Message::SettingsToggled => {
                // busy（加载/保存中）禁开，与工具栏其余按钮同一守卫语义
                if !self.busy {
                    self.settings_visible = !self.settings_visible;
                    if self.settings_visible {
                        // 第 64 轮用户点单：每次进入设置默认落在第一分类，
                        // 不记忆上次浏览位置（P51 撤销）
                        self.settings_page = SettingsPage::default();
                    } else {
                        // P47：关弹窗顺带清搜索词，下次打开回到分类浏览
                        self.settings_search.clear();
                    }
                }
                Task::none()
            }
            // P47：侧栏分类导航——点分类即离开搜索态（同款语义）。
            // 第 64 轮用户点单：分类位置不再持久化（P51 撤销），每次打开
            // 设置都回到第一分类；弹窗打开期间导航照常。
            Message::SettingsPageSelected(page) => {
                self.settings_page = page;
                self.settings_search.clear();
                Task::none()
            }
            Message::SettingsSearchChanged(query) => {
                self.settings_search = query;
                Task::none()
            }
            Message::SettingsAutosaveToggled(value) => {
                self.settings.autosave_enabled = value;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsAutosaveDelayDelta(delta) => {
                let next = (self.settings.autosave_delay_secs as i64 + delta as i64).clamp(
                    editpad_core::settings::MIN_AUTOSAVE_DELAY_SECS as i64,
                    editpad_core::settings::MAX_AUTOSAVE_DELAY_SECS as i64,
                ) as u32;
                self.settings.autosave_delay_secs = next;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsRememberRecentToggled(value) => {
                self.settings.remember_recent_files = value;
                // P20：关闭开关即清空存量列表（只关开关不清数据等于没关）
                if !value {
                    self.settings.clear_recent_files();
                }
                self.persist_settings();
                Task::none()
            }
            Message::SettingsSnapshotsToggled(value) => {
                self.settings.enable_snapshots = value;
                self.persist_settings();
                Task::none()
            }
            // 第 64 轮 ⑭：备份模式三态循环（none→simple→timestamped）
            Message::SettingsBackupModeToggled => {
                use editpad_core::settings::{
                    BACKUP_MODE_NONE, BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED,
                };
                self.settings.backup_mode = if self.settings.backup_mode == BACKUP_MODE_SIMPLE {
                    BACKUP_MODE_TIMESTAMPED
                } else if self.settings.backup_mode == BACKUP_MODE_TIMESTAMPED {
                    BACKUP_MODE_NONE
                } else {
                    BACKUP_MODE_SIMPLE
                }
                .to_owned();
                self.persist_settings();
                self.set_status(match self.settings.backup_mode.as_str() {
                    BACKUP_MODE_SIMPLE => self.t(editpad_core::Key::StBackupSimpleShort).to_owned(),
                    BACKUP_MODE_TIMESTAMPED => self.t(editpad_core::Key::StBackupTimestampedShort).to_owned(),
                    _ => self.t(editpad_core::Key::StBackupOffShort).to_owned(),
                });
                Task::none()
            }
            Message::SettingsRememberSessionToggled(value) => {
                self.settings.remember_session = value;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsExitModeToggled => {
                use editpad_core::settings::{EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT};
                self.settings.exit_mode = if self.settings.exit_mode == EXIT_MODE_SNAPSHOT {
                    EXIT_MODE_ASK
                } else {
                    EXIT_MODE_SNAPSHOT
                }
                .to_string();
                self.persist_settings();
                Task::none()
            }
            Message::SettingsIntervalDelta(delta) => {
                let next = (self.settings.snapshot_interval_secs as i64 + delta as i64).clamp(
                    editpad_core::settings::MIN_SNAPSHOT_INTERVAL_SECS as i64,
                    editpad_core::settings::MAX_SNAPSHOT_INTERVAL_SECS as i64,
                ) as u32;
                self.settings.snapshot_interval_secs = next;
                self.persist_settings();
                Task::none()
            }
            // ---------- 字体选择（P34） ----------
            Message::SettingsFontSelected(name) => {
                // 名字来自启动期枚举清单，必然可解析；仍走统一解析保持
                // 「设置值 ↔ 生效值」同源（清单为空的异常环境会回退默认）
                self.settings.set_font_family(Some(name));
                self.active_font_family = effective_font_family(
                    self.settings.font_family.as_deref(),
                    &self.available_fonts,
                )
                .map(leak_font_family);
                self.persist_settings();
                Task::none()
            }
            Message::SettingsFontReset => {
                self.settings.set_font_family(None);
                self.active_font_family = None;
                self.persist_settings();
                Task::none()
            }
            Message::FontFilterChanged(filter) => {
                self.font_filter = filter;
                Task::none()
            }
            _ => Task::none(),
        }
    }

}
