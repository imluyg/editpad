//! P155：界面语言与文案字符串表。
//!
//! 设计口径（用户点单 + 动工前确认）：
//!
//! * [`Lang`] 是**界面语言**类型（P154 起同时承担「选 UI 字体族」）；
//! * [`Key`] 是**语言无关的文案键**——每个用户可见字符串一个变体，
//!   [`Key::text`] 按语言取文；`match` 穷尽，**漏译 = 编译失败**
//!   （这是选「枚举 + 编译期穷尽」而非查表的核心理由）；
//! * 带参数的文案不进 [`Key`]，由本模块的 `fmt_*` 系列助手按语言格式化
//!   （如 [`fmt_suffix`]；另有 [`LANG_NAME_ZH_CN`] 这类单独常量）；
//! * 设置行的**行键**（控件匹配 + 元数据声明共用）也从中文文案收敛为
//!   语言无关的 [`ROW_LANGUAGE`] 等 `ROW_*` 常量——否则切到英文后行键
//!   会跟着文案漂移，控件匹配当场失配；
//! * 本模块是**纯逻辑**：零 GUI 依赖，可在 core 单测里穷尽校验。
//!
//! 加一门新语言的步骤：① [`Lang`] 加一个变体；② [`Lang::ALL`] 加一项；
//! ③ `Display` / `FromStr` 各加一支；④ `Key::text` 的每个 arm 加一支
//!   （编译器会把漏掉的位置逐个报出来）。不需要动任何调用点。

use std::fmt;

/// 界面语言。
///
/// 持久化短码 = BCP-47 风格（`"zh-CN"` / `"en"`），与 P154 落盘的
/// config.toml 逐字节兼容；未知值反序列化回落 [`Lang::ZhCn`]（见下面的
/// 手写 `Deserialize`——不用 `#[serde(other)]`，那对字符串枚举无效，
/// 会让整个配置读取失败：配置损坏永不阻断启动）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    /// 中文（简体）——默认界面语言。
    #[default]
    ZhCn,
    /// English。
    En,
}

impl serde::Serialize for Lang {
    /// 落盘 = [`Lang::code`] 的短码（`"zh-CN"` / `"en"`）——与 P154 已
    /// 写出的 config.toml 逐字节兼容（若直接 derive，会写成变体名
    /// `"ZhCn"`，旧配置读得回来、新旧混用却会互相看不懂）。
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

impl<'de> serde::Deserialize<'de> for Lang {
    /// 反序列化为**宽松**归一：未知短码回落 [`Lang::ZhCn`] 而不是报错。
    ///
    /// 为什么不用 `#[serde(other)]`：那个属性只对**内部标签**枚举生效，
    /// 对字符串形态的枚举变体不适用（实测未知码会让整个 `toml::from_str`
    /// 失败 → 手工改坏一个字段就丢掉整份配置：最近文件、字体、窗口几何
    /// 全没）。这里显式实现，保证「配置里任何无法识别的语言值都不阻断
    /// 启动，也不拖垮同文件其它字段」。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Lang::from_code(&raw))
    }
}

impl Lang {
    /// 全部支持的语言（顺序 = 设置下拉框顺序；将来加语言只改这里）。
    pub const ALL: [Lang; 2] = [Lang::ZhCn, Lang::En];

    /// 持久化/展示用短码（与 `serde` 的 rename 一致）。
    ///
    /// 只用于诊断与测试；取用户可见的语言名请用 [`Lang::native_name`]。
    pub const fn code(self) -> &'static str {
        match self {
            Lang::ZhCn => "zh-CN",
            Lang::En => "en",
        }
    }

    /// 该语言**用自己的文字**写出来的语言名（语言选择下拉框的显示口径——
    /// 与 Windows/浏览器一致：认不出当前界面语言的人也能找到自己的语言）。
    pub const fn native_name(self) -> &'static str {
        match self {
            Lang::ZhCn => LANG_NAME_ZH_CN,
            Lang::En => LANG_NAME_EN,
        }
    }

    /// 宽松归一（大小写 / 下划线 / 空白 / 地区后缀都不敏感）。
    ///
    /// 未知值一律回落 [`Lang::ZhCn`]（默认界面）——纯函数，便于单测与
    /// 设置层复用；[`std::str::FromStr`] 是它的错误版包装。
    pub fn from_code(value: &str) -> Lang {
        let v = value.trim().to_ascii_lowercase().replace('_', "-");
        match v.as_str() {
            "en" | "en-us" | "en-gb" | "en-ca" | "en-au" => Lang::En,
            _ => Lang::ZhCn,
        }
    }
}

/// 语言名（本族文字）。单独常量是因为它们既是下拉框文案，也是
/// [`Lang::native_name`] 的实现细节——两处共用一个字面量防漂移。
pub const LANG_NAME_ZH_CN: &str = "中文（简体）";
pub const LANG_NAME_EN: &str = "English";

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::str::FromStr for Lang {
    type Err = std::convert::Infallible;

    /// 与 [`Lang::from_code`] 同语义（未知值回落默认，不报错）。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Lang::from_code(s))
    }
}

/// 语言无关的**设置行键**（控件匹配 + 元数据声明共用）。
///
/// 与 [`Key`] 是两套东西：行键是程序内部的稳定标识，文案是给人看的。
/// 历史（P154 及以前）行键直接借用中文文案，切语言后必然失配，故 P155
/// 一次性收敛为下面这些 `&'static str` 常量。
pub const ROW_LANGUAGE: &str = "language";
pub const ROW_THEME: &str = "theme";
pub const ROW_SHOW_WHITESPACE: &str = "show_whitespace";
pub const ROW_SHOW_LINE_ENDINGS: &str = "show_line_endings";
pub const ROW_WORD_WRAP: &str = "word_wrap";
pub const ROW_INDENT_GUIDES: &str = "indent_guides";
pub const ROW_EDGE_COLUMN: &str = "edge_column";
pub const ROW_FONT_FAMILY: &str = "font_family";
pub const ROW_FONT_SIZE: &str = "font_size";
pub const ROW_AUTOSAVE: &str = "autosave";
pub const ROW_AUTOSAVE_DELAY: &str = "autosave_delay";
pub const ROW_BACKUP_MODE: &str = "backup_mode";
pub const ROW_REMEMBER_RECENT: &str = "remember_recent_files";
pub const ROW_SNAPSHOTS: &str = "snapshots";
pub const ROW_REMEMBER_SESSION: &str = "remember_session";
pub const ROW_EXIT_MODE: &str = "exit_mode";
pub const ROW_SNAPSHOT_INTERVAL: &str = "snapshot_interval";
pub const ROW_ABOUT_NAME: &str = "about_name";
pub const ROW_ABOUT_VERSION: &str = "about_version";
pub const ROW_ABOUT_RENDERER: &str = "about_renderer";
pub const ROW_ABOUT_LICENSE: &str = "about_license";

/// 声明 [`Key`]（全部用户可见文案的语言无关键）与它在**每种语言**下的文案。
///
/// 为什么用宏：[`Key::ALL`] 必须与枚举变体严格一致（漏一个，穷尽校验测试
/// 就少覆盖一条）。宏把「枚举定义」与「全量清单」绑成同一份输入——加变体
/// 只改一行，清单自动跟上，不存在两处漂移。
///
/// 变体名 = 语义（不是文案）；`zh_cn` / `en` 两个 `match` 必须为每一支给出
/// 文案——**漏译即编译失败**，这是本方案对「漏译」的第一道防线。
macro_rules! key_table {
    ($({ $variant:ident => $zh:literal, $en:literal $(,)? }),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Key { $($variant),* }

        impl Key {
            /// 全部文案键（穷尽校验与遍历用）。
            pub const ALL: &'static [Key] = &[$(Key::$variant),*];

            /// 取当前界面语言下的文案。
            pub fn text(self, lang: Lang) -> &'static str {
                match lang {
                    Lang::ZhCn => zh_cn(self),
                    Lang::En => en(self),
                }
            }
        }

        fn zh_cn(k: Key) -> &'static str {
            match k { $(Key::$variant => $zh),* }
        }

        fn en(k: Key) -> &'static str {
            match k { $(Key::$variant => $en),* }
        }
    };
}
key_table! {
    { MenuFile => "文件", "File" },
    { MenuEdit => "编辑", "Edit" },
    { MenuView => "查看", "View" },
    { MenuSettings => "设置", "Settings" },
    { MenuOpen => "打开…", "Open…" },
    { MenuSave => "保存", "Save" },
    { MenuSaveAs => "另存为…", "Save As…" },
    { MenuReopenClosed => "恢复上次关闭的标签页", "Reopen Closed Tab" },
    { MenuRecents => "最近文件", "Recent Files" },
    { MenuUndo => "撤销", "Undo" },
    { MenuRedo => "重做", "Redo" },
    { MenuCut => "剪切", "Cut" },
    { MenuCopy => "复制", "Copy" },
    { MenuPaste => "粘贴", "Paste" },
    { MenuSelectAll => "全选", "Select All" },
    { MenuFind => "查找/替换栏", "Find/Replace Bar" },
    { MenuGoto => "跳转到行", "Go to Line" },
    { MenuInsertDateTime => "插入日期时间", "Insert Date and Time" },
    { MenuToggleComment => "切换行注释", "Toggle Line Comment" },
    { MenuColumnEditor => "列编辑器…", "Column Editor…" },
    { MenuZoomIn => "放大", "Zoom In" },
    { MenuZoomOut => "缩小", "Zoom Out" },
    { MenuZoomReset => "重置缩放", "Reset Zoom" },
    { MenuThemeToggle => "切换深浅主题", "Toggle Light/Dark Theme" },
    { MenuShowWhitespace => "显示空白字符", "Show Whitespace" },
    { MenuShowLineEndings => "显示行尾符", "Show Line Endings" },
    { MenuWordWrap => "自动换行", "Word Wrap" },
    { MenuTabWrapOverride => "本页自动换行", "Word Wrap For This Tab" },
    { MenuTabWrapFollow => "本页自动换行（跟随全局）", "Word Wrap For This Tab (follow global)" },
    { MenuTabWrapOn => "本页自动换行（开）", "Word Wrap For This Tab (on)" },
    { MenuTabWrapOff => "本页自动换行（关）", "Word Wrap For This Tab (off)" },
    { ComboWheel => "Ctrl+滚轮", "Ctrl+wheel" },
    { BackupSimpleShort => "覆盖式", "Overwrite" },
    { BackupTimestampedShort => "时间戳历史", "Timestamped" },
    { MenuTabFontReset => "本页字号重置（跟随全局）", "Reset This Tab's Font Size (Follow Global)" },
    { MenuPreview => "MD 预览", "Markdown Preview" },
    { MenuPreviewClose => "关闭 MD 预览", "Close Markdown Preview" },
    { MenuOpenSettings => "打开设置…", "Open Settings…" },
    { MenuBackupMode => "保存时备份", "Backup On Save" },
    { Settings => "设置", "Settings" },
    { SettingsOptions => "选项", "Options" },
    { SettingsSearchPlaceholder => "搜索设置…", "Search settings…" },
    { SettingsRestoreDefaults => "全部恢复默认", "Restore All Defaults" },
    { SettingsApplyHint => "修改后立即生效并写入 config.toml", "Changes apply immediately and are saved to config.toml" },
    { SettingsNoMatchPrefix => "没有匹配「", "No settings match “" },
    { SettingsNoMatchSuffix => "」的设置", "”" },
    { PageAppearance => "外观", "Appearance" },
    { PageFont => "字体", "Font" },
    { PageSave => "保存", "Saving" },
    { PageSession => "会话与隐私", "Session & Privacy" },
    { PageHotkeys => "快捷键", "Shortcuts" },
    { PageAbout => "关于", "About" },
    { RowLanguage => "界面语言", "Interface Language" },
    { RowLanguageDesc => "界面文案与界面字体族都随它切换（正文与 Markdown 预览不受影响）。", "Switches both the interface text and the interface font family \
                            (the editor text and Markdown preview are unaffected)." },
    { RowTheme => "主题", "Theme" },
    { RowThemeDesc => "切换深色 / 浅色主题，立即生效并记住。", "Switch between dark and light themes; applies immediately and is remembered." },
    { RowShowWhitespace => "显示空白字符", "Show Whitespace" },
    { RowShowWhitespaceDesc => "在空格与制表符位置画淡色标记（不改文档内容）。", "Draw faint marks where spaces and tabs are (the document itself is unchanged)." },
    { RowShowLineEndings => "显示行尾符", "Show Line Endings" },
    { RowShowLineEndingsDesc => "在每行末尾画一个短标，标出换行位置。", "Draw a small mark at the end of every line to show where it breaks." },
    { RowWordWrap => "自动换行", "Word Wrap" },
    { RowWordWrapDesc => "长行按窗口宽度软换行显示（不改文档内容）；折行开态下水平滚动条隐藏。", "Soft-wrap long lines to the window width (the document itself is unchanged); \
                            the horizontal scrollbar is hidden while wrapping." },
    { RowIndentGuides => "缩进参考线", "Indent Guides" },
    { RowIndentGuidesDesc => "在行首缩进的每个制表位层级画淡竖线，辅助对齐嵌套层级。", "Draw a faint vertical line at every indent level for easier alignment." },
    { RowEdgeColumn => "右缘标尺列", "Right Edge Column" },
    { RowEdgeColumnDesc => "在指定显示列处画一条纵向辅助线（如 80 列限宽提醒）；0 = 关闭。", "Draw a vertical guide at a given display column (e.g. an 80-column reminder); 0 = off." },
    { RowFontFamily => "正文字体", "Editor Font" },
    { RowFontFamilyDesc => "只作用于正文与 Markdown 预览；菜单/状态栏/行号等界面文字由\
                             「界面语言」与行号位候选链决定。建议选含中文字形的等宽字体，\
                             非等宽字体的列对齐会漂移。", "Affects only the editor text and the Markdown preview; menus, status bar \
                              and line numbers follow the interface language. A monospaced font with CJK \
                              glyphs is recommended — proportional fonts drift out of column alignment." },
    { RowFontSize => "字号", "Font Size" },
    { RowFontSizeDesc => "正文文字大小；编辑器内 Ctrl+滚轮 缩放，或在此步进调节。", "Editor text size. Use Ctrl+wheel inside the editor, or step it here." },
    { RowAutosave => "编辑后自动写盘", "Save Automatically After Edits" },
    { RowAutosaveDesc => "默认关闭（主流编辑器口径）：修改留在窗口内，Ctrl+S 才写原文件；\
                           崩溃防护由会话快照兜底。开启后若文件被外部改动会先拒写并提示。", "Off by default (mainstream editor convention): edits stay in the window until \
                            Ctrl+S. Session snapshots cover crashes. When on, a file changed externally is \
                            refused and reported first." },
    { RowAutosaveDelay => "自动写盘延迟（秒）", "Autosave Delay (seconds)" },
    { RowAutosaveDelayDesc => "停手多少秒后执行自动写盘；仅在上面的开关开启时生效。", "How long to wait after you stop typing before saving; only applies when the switch above is on." },
    { RowBackupMode => "保存时备份", "Backup On Save" },
    { RowBackupModeDesc => "覆盖已有文件前把磁盘旧版复制一份：「覆盖式」= 同目录 name.bak；\
                             「时间戳历史」= name.bak.d 目录内逐次留存；超过 64MB 的文件自动\
                             跳过，备份失败不阻断保存。", "Copy the previous on-disk version before overwriting: “Overwrite” = name.bak \
                              beside it; “Timestamped” = successive copies in a name.bak.d directory. Files \
                              over 64MB are skipped; a failed backup never blocks the save." },
    { RowRememberRecent => "记住最近打开的文件", "Remember Recently Opened Files" },
    { RowRememberRecentDesc => "在「最近打开」保留历史；关闭开关会一并清空存量记录。", "Keep history in “Recent Files”; turning the switch off clears existing records too." },
    { RowSnapshots => "会话快照", "Session Snapshots" },
    { RowSnapshotsDesc => "关窗时自动保存未存内容，异常退出后可恢复。", "Save unsaved content on close so it can be restored after an abnormal exit." },
    { RowRememberSession => "启动时恢复上次界面", "Restore Previous Session On Start" },
    { RowRememberSessionDesc => "启动时还原上次的标签页与内容。", "Restore the previous tabs and their content on startup." },
    { RowExitMode => "关窗行为", "Close Window Behavior" },
    { RowExitModeDesc => "「快照直退」不打断；「每次询问」先确认未保存内容。", "“Snapshot and quit” never interrupts; “Ask every time” confirms unsaved content first." },
    { RowSnapshotInterval => "快照心跳间隔（秒）", "Snapshot Heartbeat (seconds)" },
    { RowSnapshotIntervalDesc => "后台周期保存快照的间隔。", "Interval between background snapshot saves." },
    { RowAboutName => "名称", "Name" },
    { RowAboutNameDesc => "Editpad —— 轻量文本编辑器。", "Editpad — a lightweight text editor." },
    { RowAboutVersion => "版本", "Version" },
    { RowAboutRenderer => "渲染后端", "Renderer" },
    { RowAboutRendererDesc => "tiny-skia 软渲染（内存取舍：进程内存约为 GPU 路径的 1/12）。", "tiny-skia software rendering (roughly 1/12 the process memory of the GPU path)." },
    { RowAboutLicense => "开源协议", "License" },
    { ButtonModify => "修改", "Change" },
    { ButtonResetDefault => "回退默认", "Use Default" },
    { ButtonRestoreAll => "全部恢复默认", "Restore All Defaults" },
    { HotkeyCaptureHint => "按下新组合键…（Esc 取消）", "Press a new shortcut… (Esc to cancel)" },
    { ThemeLight => "浅色", "Light" },
    { ThemeDark => "深色", "Dark" },
    { BackupSimple => "覆盖式 name.bak", "Overwrite name.bak" },
    { BackupTimestamped => "时间戳历史", "Timestamped history" },
    { BackupOff => "关闭", "Off" },
    { ExitSnapshot => "快照直退", "Snapshot and quit" },
    { ExitAsk => "每次询问", "Ask every time" },
    { SwitchOff => "关", "Off" },
    { FontFilterPlaceholder => "输入关键字过滤字体", "Filter fonts by keyword" },
    { FontNoneAvailable => "无法枚举系统字体（保持默认等宽）", "Cannot enumerate system fonts (keeping the default monospace)" },
    { FontCurrentPrefix => "当前：", "Current: " },
    { FontCurrentDefault => "当前：默认（等宽）", "Current: default (monospace)" },
    { FontCurrentMissingPrefix => "当前：默认等宽（配置的「", "Current: default monospace (configured “" },
    { FontCurrentMissingSuffix => "」未安装）", "” is not installed)" },
    { TabPin => "📌 固定标签页", "📌 Pin Tab" },
    { TabUnpin => "取消固定", "Unpin" },
    { TabSave => "保存", "Save" },
    { TabRename => "重命名…", "Rename…" },
    { TabSaveAs => "另存为…", "Save As…" },
    { TabCopyPath => "复制完整路径", "Copy Full Path" },
    { TabCopyName => "复制文件名", "Copy File Name" },
    { TabClose => "关闭", "Close" },
    { TabCloseOthers => "关闭其他标签页", "Close Other Tabs" },
    { TabCloseRight => "关闭右侧标签页", "Close Tabs To The Right" },
    { TabUntitled => "未命名", "Untitled" },
    { TabHeaderPrefix => "「", "“" },
    { TabHeaderSuffix => "」", "”" },
    { TabUnsaved => "（未保存）", "(unsaved)" },
    { RecentsLoading => "加载中…", "Loading…" },
    { RecentsEmpty => "（暂无最近文件）", "(no recent files)" },
    { RecentsClear => "清空记录", "Clear History" },
    { RecentsClearHint => "从 config.toml 移除全部路径", "Remove all paths from config.toml" },
    { StatusLength => "长度", "Length" },
    { StatusLines => "行数", "Lines" },
    { StatusLine => "行", "Ln" },
    { StatusColumn => "列", "Col" },
    { StatusPosition => "位置", "Pos" },
    { StatusSelected => "选", "Sel" },
    { Ready => "就绪", "Ready" },
    { HintNoMoreUndo => "没有更多撤销历史", "No more undo history" },
    { HintAtLatest => "已在最新状态", "Already at the latest state" },
    { HintNoBracketPair => "光标不在括号旁（或未找到配对）", "Cursor is not next to a bracket (or no match found)" },
    { GotoTitle => "跳转到行：", "Go to line:" },
    { GotoButton => "跳转", "Go" },
    { GotoInvalidLine => "请输入有效行号（从 1 开始）", "Enter a valid line number (starting from 1)" },
    { CloseConfirmDirty => "文档有未保存的更改，确定要关闭吗？", "This document has unsaved changes. Close it anyway?" },
    { ButtonSaveAndClose => "保存并关闭", "Save and Close" },
    { ButtonDiscardChanges => "放弃更改", "Discard Changes" },
    { ButtonCancel => "取消", "Cancel" },
    { ButtonDiscardAndClose => "放弃更改并关闭", "Discard and Close" },
    { DiscardClosePrefix => "「", "“" },
    { DiscardCloseSuffix => "」有未保存的更改，放弃并关闭？", "” has unsaved changes. Discard them and close?" },
    { DiscardOpenPrefix => "", "" },
    { DiscardOpenSuffix => " 有未保存的更改，放弃并打开？", " has unsaved changes. Discard them and open?" },
    { ButtonDiscardAndOpen => "放弃更改并打开", "Discard and Open" },
    { ButtonReload => "重新加载", "Reload" },
    { ButtonIgnore => "忽略", "Ignore" },
    { ButtonIgnoreAll => "全部忽略", "Ignore All" },
    { RestoreTitle => "检测到上次未正常退出的未保存工作区", "An unsaved workspace from an abnormal exit was found" },
    { ButtonRestore => "恢复", "Restore" },
    { ButtonDiscard => "丢弃", "Discard" },
    { FindGrip => "≡  拖动（双击复位）", "≡  Drag (double-click to reset)" },
    { FindPrev => "↑ 上一个", "↑ Previous" },
    { FindNext => "↓ 下一个", "↓ Next" },
    { FindDir => "目录：", "Folder:" },
    { FindBrowse => "浏览…", "Browse…" },
    { FindReplaceCurrent => "替换当前", "Replace" },
    { FindReplaceAll => "全部替换", "Replace All" },
    { FindFindAll => "查找全部", "Find All" },
    { FindAllTitle => "全部匹配", "All Matches" },
    { FindAllTruncatedPrefix => "", "" },
    { FindAllTruncatedSuffix => "（已达封顶，结果不完整）", " (result cap reached; list is incomplete)" },
    { FindInputQueryHint => "先输入查询内容", "Enter a search term first" },
    { FindFilesSummary => "在文件中查找：", "Find in Files: " },
    { FindFileHitsPrefix => "（", " (" },
    { FindFileHitsSuffix => " 处）", " hits)" },
    { FindNoMatch => "无匹配", "No matches" },
    { FifTitle => "在文件中查找", "Find in Files" },
    { FifReachedCapSuffix => "（已达封顶，结果不完整）", " (cap reached; results are incomplete)" },
    { FifQueryEmpty => "先输入查询内容", "Enter a search term first" },
    { PaletteNoMatch => "无匹配命令或标签", "No matching command or tab" },
    { PaletteTabsDetail => "（未保存）", "(unsaved)" },
    { ColumnEditorTitle => "列编辑器", "Column Editor" },
    { ButtonOk => "确定", "OK" },
    { ColumnEditorNoText => "请输入要插入的文本", "Enter the text to insert" },
    { ColumnEditorNoBlock => "先建立列块选区（Alt+Shift 拖拽）", "Create a column block first (drag with Alt+Shift)" },
    { ColumnEditorStartInteger => "起始值须为整数（可负）", "Start value must be an integer (may be negative)" },
    { ColumnEditorStepInteger => "步长须为整数（可负）", "Step must be an integer (may be negative)" },
    { ColumnEditorPadInteger => "补零宽度须为非负整数（0 = 不补）", "Zero-pad width must be a non-negative integer (0 = no padding)" },
    { EolCurrentPrefix => "当前行尾：", "Current line ending: " },
    { StUndoNothing => "没有更多撤销历史", "No more undo history" },
    { StSnapshotHeartbeatFailed => "快照心跳失败：", "Snapshot heartbeat failed: " },
    { StSessionSnapshotFailed => "会话快照失败：", "Session snapshot failed: " },
    { StSessionRestored => "会话恢复完成：", "Session restored: " },
    { StSessionRestoreDroppedPrefix => "", "" },
    { StSessionRestoreDroppedSuffix => " 页超出内存护栏未恢复", " page(s) exceeded the memory guard and were not restored" },
    { StSessionRestoreFailedPrefix => "", "" },
    { StSessionRestoreFailedSuffix => " 页未能恢复原内容", " page(s) could not be restored" },
    { StCopied => "已复制 ", "Copied " },
    { StInvalidRegex => "正则无效：", "Invalid regular expression: " },
    { StMatchCounter => " 处匹配", " matches" },
    { StOverwriteOn => "覆写模式（Insert 切回插入）：打字将逐字替换光标处字符", "Overwrite mode (press Insert to go back): typing replaces the character at the cursor" },
    { StInsertMode => "插入模式", "Insert mode" },
    { StReadOnlyOn => "已锁定只读：编辑与撤销被拒收（Ctrl+R 解除）", "Read-only locked: edits and undo are rejected (Ctrl+R to unlock)" },
    { StReadOnlyOff => "已解除只读", "Read-only unlocked" },
    { StReadOnlyLocked => "文档已锁定只读（Ctrl+R 解除）", "Document is locked read-only (Ctrl+R to unlock)" },
    { StJsonOnly => "格式化 JSON 仅对 JSON 文件可用（Ctrl+Shift+F）", "Format JSON is only available for JSON files (Ctrl+Shift+F)" },
    { StJsonFormatted => "已格式化 JSON", "JSON formatted" },
    { StJsonFailed => "JSON 格式化失败：", "JSON formatting failed: " },
    { StSyntaxAnalyzing => "语法分析中…", "Analysing syntax…" },
    { StPreviewMarkdownOnly => "预览仅支持 Markdown 文件", "Preview is only available for Markdown files" },
    { StRevealedInExplorer => "已在资源管理器中定位文件", "Revealed in File Explorer" },
    { StRevealFailed => "打开所在文件夹失败：", "Failed to open the containing folder: " },
    { StRevealNeedsSave => "未命名页需先保存才能定位所在文件夹", "Save this untitled tab first to locate it in its folder" },
    { StNoClosedTab => "没有可恢复的已关闭文件", "No closed file to reopen" },
    { StPinnedMustUnpin => "固定标签页需先取消固定再关闭", "Unpin the tab before closing it" },
    { StRenameUntitledFirst => "未命名标签页请先另存为再关闭", "Save this untitled tab before closing it" },
    { StHotkeysDefault => "已恢复默认热键", "Default shortcuts restored" },
    { StLanguageSwitched => "界面语言已切换", "Interface language switched" },
    { StTabFontReset => "本页字号已重置为全局默认 ", "This tab's font size was reset to the global default " },
    { StBackupSimple => "保存时备份：同目录 name.bak 覆盖式", "Backup on save: name.bak in the same folder (overwrite)" },
    { StBackupTimestamped => "保存时备份：name.bak/ 目录按时间戳留存", "Backup on save: timestamped copies under name.bak/" },
    { StBackupOff => "保存时备份：已关闭", "Backup on save: off" },
    { StTabGone => "目标标签页已关闭，本次加载结果已丢弃", "The target tab was closed; this load result was discarded" },
    { StOpenFailed => "打开失败：", "Open failed: " },
    { StOpenedExternal => "已用系统默认程序打开 ", "Opened with the system default application: " },
    { StSwitchCancelledClose => "保存期间切换了标签页，已取消关窗", "The tab changed while saving; closing was cancelled" },
    { StSaveFailed => "保存失败：", "Save failed: " },
    { StExternallyModifiedSkip => "文件已被外部修改，已跳过自动写盘", "The file changed on disk; autosave was skipped" },
    { StAutosaveFailed => "自动保存失败：", "Autosave failed: " },
    { StUntitledPickEncoding => "未命名页请先「另存为」取得路径，再选择保存编码", "Use “Save As…” to give this untitled tab a path before choosing a save encoding" },
    { StEolAlready => "行尾已是 ", "Line ending is already " },
    { StEolConverted => "已转换为 ", "Converted to " },
    { StCancelledAutoClose => "保存后又有新改动，已取消自动关闭", "New edits arrived after saving; auto-close was cancelled" },
    { StRecentMissing => "文件不存在或已被移动：", "File does not exist or was moved: " },
    { StRecentsCleared => "已清空最近文件记录", "Recent files cleared" },
    { StFullscreenOn => "已进入全屏（F11 退出；全屏期间不记忆窗口几何）", "Entered full screen (F11 to exit; window geometry is not remembered while full screen)" },
    { StFullscreenOff => "已退出全屏", "Exited full screen" },
    { StAlwaysOnTopOn => "窗口已置顶（F9 取消）", "Window pinned on top (F9 to cancel)" },
    { StAlwaysOnTopOff => "已取消置顶", "Always-on-top disabled" },
    { StRegexModeHint => "正则模式：替换支持 $1/${1} 组引用，^$ 逐行锚定用 (?m)", "Regex mode: $1/${1} group references in replacements; use (?m) for line anchors" },
    { StRegexReplacing => "正则替换中…（后台）", "Replacing with regex… (background)" },
    { StReplaced => "已替换 ", "Replaced " },
    { StRegexReplaceFailed => "正则替换失败：", "Regex replace failed: " },
    { StMonitorOn => "已开启文件监视：磁盘变化时自动重载（干净页）并跟随文末（F8 关闭）", "File monitoring on: external changes reload clean pages and follow the end of file (F8 to turn off)" },
    { StMonitorOff => "已关闭文件监视", "File monitoring off" },
    { StColumnEditorWrapOff => "自动换行开启时不可用列编辑器（先关闭折行）", "The column editor is unavailable while word wrap is on (turn wrap off first)" },
    { StColumnEditorNeedBlock => "先建立列块选区（Alt+Shift 拖拽，竖直拖出零宽插入列亦可）", "Create a column block first (drag with Alt+Shift; a zero-width vertical drag works too)" },
    { StDocReadOnlyLocked => "文档已锁定只读（Ctrl+R 解除）", "Document is locked read-only (Ctrl+R to unlock)" },
    { StMemoryGuard => "内存保护：合计内容超过上限，请先关闭部分大文档再打开", "Memory guard: total content exceeds the limit; close some large documents first" },
    { StInvalidHotkey => " 不是有效的热键组合", " is not a valid shortcut" },
    { StHotkeyRebound => " 已绑定 ", " is now bound to " },
    { StHotkeyCapturePromptPrefix => "为「", "Press a new shortcut for “" },
    { StHotkeyCapturePromptSuffix => "」按下新组合键（Esc 取消）", "” (Esc to cancel)" },
    { StHotkeyConflictPrefix => "「", "“" },
    { StHotkeyConflictMiddle => "」已被「", "” is already used by “" },
    { StHotkeyConflictSuffix => "」占用，换一个组合再试（Esc 取消）", "”. Try another combination (Esc to cancel)" },
    { StBusyRetry => "加载/保存进行中，请稍后再重命名", "Loading or saving in progress; try renaming again later" },
    { StRenameInvalid => "名称不能为空或含 \\/:*?\"<>| 等字符", "The name cannot be empty or contain \\/:*?\"<>|" },
    { StRenameExists => "重命名失败：目标已存在「", "Rename failed: the target already exists: " },
    { StRenamed => "已重命名：", "Renamed to " },
    { StRenameFailed => "重命名失败：", "Rename failed: " },
    { StExternalPaused => "检测到外部修改，已暂停保存：请先在提示条选择「重新加载」或「忽略」", "External change detected; saving is paused — choose “Reload” or “Ignore” in the banner first" },
    { StFontMissing => "配置的字体「", "Configured font “" },
    { StFontMissingSuffix => "」未安装，本次启动回退默认等宽", "” is not installed; falling back to the default monospace for this session" },
    { StSyntaxAnalyzingMiddle => "%（后台）", "% (background)" },
    { StBackupSimpleShort => "保存时备份：同目录 name.bak 覆盖式", "Backup on save: name.bak in the same folder (overwrite)" },
    { StBackupTimestampedShort => "保存时备份：name.bak/ 目录按时间戳留存", "Backup on save: timestamped copies under name.bak/" },
    { StBackupOffShort => "保存时备份：已关闭", "Backup on save: off" },
    { StEolAlreadyMiddle => "行尾已是 ", "Line ending is already " },
    { StInsertedStampPrefix => "已插入 ", "Inserted " },
    { StColumnEditorPadCapPrefix => "补零宽度上限 ", "Zero-pad width is limited to " },
    { StColumnEditorPadCapSuffix => " 位", " digits" },

    // ---- 编辑器动作拒绝原因（editor 层返回类型化错误，此处取文案） ----
    { EmCursorNotOnWord => "光标不在词上（Ctrl+M 需要词或选区）", "The cursor is not on a word (Ctrl+M needs a word or a selection)" },
    { EmExtraCursorCapPrefix => "附加光标数量已达上限（", "Extra cursor limit reached (" },
    { EmExtraCursorCapSuffix => "）", ")" },
    { EmNoMoreMatch => "没有更多匹配", "No more matches" },
    { EmNoSelection => "请先选中要处理的文本", "Select some text first" },
    { EmBase64DecodeInvalid => "Base64 解码失败：选区不是有效的 Base64 文本", "Base64 decode failed: the selection is not valid Base64" },
    { EmBase64DecodeNotUtf8 => "Base64 解码结果不是有效的 UTF-8 文本", "Base64 decode produced invalid UTF-8" },
    { EmUrlDecodeInvalid => "URL 解码失败：选区含有无效的百分号转义", "URL decode failed: the selection contains an invalid percent escape" },

    // ---- 过大文档的守卫提示（占位 `{}` 由代码填数字） ----
    { StTooLargeJsonPrefix => "文档过大（", "Document too large (" },
    { StTooLargeJsonMiddle => " 字符），暂不支持格式化（上限 ", " characters); formatting is unavailable (limit " },
    { StTooLargeJsonSuffix => "）", ")" },
    { StTooLargeEolPrefix => "文档过大（", "Document too large (" },
    { StTooLargeEolMiddle => " 字符），暂不支持行尾转换（上限 ", " characters); line-ending conversion is unavailable (limit " },
    { StTooLargeEolSuffix => "）", ")" },
    { StTooLargeRegexPrefix => "文档过大（", "Document too large (" },
    { StTooLargeRegexMiddle => " 字符），正则替换暂不支持（上限 ", " characters); regex replace is unavailable (limit " },
    { StTooLargeRegexSuffix => "）；可改用字面模式", "); try literal mode instead" },
    { StTooLargeWordPrefix => "文档过大（", "Document too large (" },
    { StTooLargeWordMiddle => " 字符），整词替换暂不支持（上限 ", " characters); whole-word replace is unavailable (limit " },
    { StTooLargeWordSuffix => "）；可关闭整词后重试", "); turn whole-word off and retry" },
    { StFifTruncatedPrefix => "在文件中查找：已达封顶截断，结果不完整（", "Find in Files: result cap reached, list is incomplete (" },
    { StFifTruncatedMiddle => " 个文件 / ", " file(s) / " },
    { StFifTruncatedSuffix => " 处）", " hit(s))" },
    { StTabFontSizePrefix => "本页字号 ", "This tab's font size " },
    { StTabFontSizeMiddle => "（全局默认 ", " (global default " },
    { StTabFontSizeSuffix => "；查看菜单可重置）", "; use the View menu to reset)" },
    { StColumnEditorRowsPrefix => "列块行数 ", "The block has " },
    { StColumnEditorRowsMiddle => " 超过列编辑器上限 ", " rows, over the column editor limit " },
    { StHeartbeatThreadGone => "心跳线程意外终止：", "The heartbeat thread terminated unexpectedly: " },
    { StFileTypesAll => "所有文件", "All files" },
    { StWindowsOnly => "仅支持 Windows", "Windows only" },
    { StAutosaveThreadGone => "自动保存线程意外终止", "The autosave thread terminated unexpectedly" },
    { StBackupSkipLarge => "文件超过 64MB，按策略跳过备份", "File exceeds 64MB; backup skipped by policy" },
    { StBackupDone => "已备份旧版 → ", "Previous version backed up to " },
    { StBackupFailed => "备份失败（继续保存）：", "Backup failed (saving continues): " },
    { StLoadThreadCrashed => "加载线程崩溃：", "Load thread crashed: " },
    { StLoadStreamEnded => "加载流意外终止（未收到完成事件）", "The load stream ended unexpectedly (no completion event)" },
    { StUnknownReason => "未知原因", "unknown reason" },
    { NoticeEncodeSubstituted => "部分字符无法用目标编码表示，已按 &#编号; 形式写入", "Some characters cannot be represented in the target encoding; they were written as &#number;" },
    { NoticeBomLost => "已按 UTF-8（无 BOM）保存：原文件的 BOM 已丢失", "Saved as UTF-8 (no BOM): the original file's BOM was lost" },
    { NoticeTranscoded => "已从 ", "Transcoded from " },
    { NoticeTranscodedMiddle => " 转码为 ", " to " },
    { NoticeTranscodedSuffix => " 落盘（转码不可逆）", " (the conversion is irreversible)" },
    { ErrRead => "无法读取文件", "Cannot read file" },
    { ErrWrite => "无法写入文件", "Cannot write file" },
    { ErrBinaryDetected => "疑似二进制文件（含 NUL 字节或替换符占比过高），已拒绝打开以保护原文件", "looks like a binary file (NUL bytes or too many replacement characters); opening was refused to protect the original" },
    { RecentReopenPrefix => "恢复上次关闭的文件（", "Reopen last closed file (" },
    { RecentReopenSuffix => "）", ")" },
    { ExternalModifiedFilesPrefix => "」已被外部修改（共 ", "” changed on disk (" },
    { ExternalModifiedFilesSuffix => " 个文件），是否重新加载？", " files). Reload it?" },
    { ExternalModifiedOneSuffix => "」已被外部修改，是否重新加载？", "” changed on disk. Reload it?" },
    { AlsoOpenInNewWindow => "同时在新窗口打开", "Also open in a new window" },
    { FindInFilesMode => "在文件中查找", "Find in Files" },
    { TabIndexDirtyPrefix => "第 ", "Tab " },
    { TabIndexDirtyMiddle => " 个标签页有未保存的更改", " has unsaved changes" },
    { BatchCloseConfirmPrefix => "要关闭的 ", "Of the " },
    { BatchCloseConfirmMiddle => " 个标签页中 ", " tab(s) to close, " },
    { BatchCloseConfirmMiddle2 => " 个有未保存的更改，全部放弃并关闭？", " of them with unsaved changes. Discard all and close?" },
    { TooManyTabsMiddle => " 个文件）", " files)" },
    { NoSearchResultsTitle => "（无匹配）", "(no matches)" },
    { FifNoResultsTitle => "（无匹配结果）", "(no matches)" },
    { FifScanProgressPrefix => "（已扫 ", "(scanned " },
    { FifScanProgressMiddle => " 个文件）", " files)" },
    { AllMatchesTitle => "全部匹配", "All Matches" },
    { UndoLabel => "撤销", "Undo" },
    { RedoLabel => "重做", "Redo" },

    // ---- 编码 / 行尾弹出菜单 ----
    { SaveAsEncodingPrefix => "以 ", "Save as " },
    { SaveAsEncodingSuffix => " 保存", "" },
    { ConvertEolPrefix => "转换为 ", "Convert to " },
    { ConvertEolCrLfSuffix => "（Windows）", " (Windows)" },
    { ConvertEolLfSuffix => "（Unix）", " (Unix)" },
    { StCopyPathUntitled => "未命名页没有路径可复制（先保存）", "This untitled tab has no path to copy (save it first)" },
    { ListSeparator => "，", ", " },

    // ---- 名称输入框 / 占位符 ----
    { RenamePlaceholder => "新名称", "New name" },
    { GotoPlaceholder => "行号", "Line number" },
    { PalettePlaceholder => "输入命令或标签名…", "Type a command or tab name…" },
    { FindQueryPlaceholder => "查找内容", "Find" },
    { ReplaceQueryPlaceholder => "替换为", "Replace with" },
    { FindCaseSensitive => "区分大小写", "Match case" },
    { FindRegexMode => ".* 正则", ".* Regex" },
    { FindWholeWord => "整词", "Whole word" },
    { FindInFilesExit => "退出目录查找", "Exit Find in Files" },
    { FindSearching => "查找中…", "Searching…" },
    { FindAllShownPrefix => "已显示前 ", "Showing the first " },
    { FindAllShownSuffix => " 条——请细化关键词", " — narrow the search to see more" },
    { FindAllShownTotalMiddle => " 条（共 ", " of " },
    { FindAllShownTotalSuffix => " 处）——请细化关键词", " matches — narrow the search to see more" },
    { ScanProgressPrefix => "目录扫描中…（已扫 ", "Scanning folder… (" },
    { ScanProgressSuffix => " 个文件）", " files scanned)" },
    { DirNoMatch => "目录中无匹配", "No matches in this folder" },
    { FifDirUnnamed => "（当前页未命名：先保存得到所在目录，或「浏览…」选择）", "(current tab is untitled: save it to get a folder, or pick one with “Browse…”)" },

    // ---- 列编辑器对话框 ----
    { CeModeText => "文本", "Text" },
    { CeModeNumber => "序号", "Numbers" },
    { CeStart => "起始", "Start" },
    { CeStep => "步长", "Step" },
    { CeBase => "进制", "Base" },
    { CeBaseDec => "十进制", "Decimal" },
    { CeBaseHex => "十六进制", "Hexadecimal" },
    { CeBaseBin => "二进制", "Binary" },
    { CeBaseOct => "八进制", "Octal" },
    { CePadWidth => "补零宽", "Zero pad" },
    { CeHexUpper => "十六进制字母大写（A-F）", "Uppercase hex digits (A-F)" },
    { CeTextContent => "每行插入的内容（多行 = 循环填充）", "Text inserted into every line (multiple lines cycle)" },
    { CeStartHint => "如 1（整数，可负）", "e.g. 1 (integer, may be negative)" },
    { CeStepHint => "如 1（整数，可负）", "e.g. 1 (integer, may be negative)" },
    { CePadHint => "0 = 不补，上限 32", "0 = no padding, up to 32" },
    { CeTargetBlockPrefix => "目标列块：", "Target block: " },
    { CeTargetBlockSuffix => " 行", " line(s)" },
    { CeNoBlockWarn => "⚠ 当前无列块选区，确认前请先框选", "⚠ No column block selected — make a selection first" },

    // ---- 热键动作描述（设置页快捷键页 / 命令面板） ----
    { HkOpen => "打开文件", "Open a file" },
    { HkSave => "保存", "Save" },
    { HkSelectAll => "全选", "Select all" },
    { HkFind => "查找/替换栏", "Find/replace bar" },
    { HkGoto => "跳转到行", "Go to line" },
    { HkUndo => "撤销", "Undo" },
    { HkRedo => "重做（Ctrl+Y 或主流的 Ctrl+Shift+Z）", "Redo (Ctrl+Y, or Ctrl+Shift+Z in most editors)" },
    { HkCopy => "复制选区", "Copy the selection" },
    { HkCut => "剪切选区", "Cut the selection" },
    { HkPaste => "粘贴", "Paste" },
    { HkDupLine => "在下方复制当前行", "Duplicate the current line below" },
    { HkDelLine => "删除当前行", "Delete the current line" },
    { HkMoveLineUp => "当前行上移", "Move the current line up" },
    { HkMoveLineDown => "当前行下移", "Move the current line down" },
    { HkToUppercase => "转为大写（选区/全文）", "To upper case (selection or whole file)" },
    { HkToLowercase => "转为小写（选区/全文）", "To lower case (selection or whole file)" },
    { HkTrimTrailing => "去除行尾空白（选区行/全文）", "Trim trailing whitespace (selection lines or whole file)" },
    { HkTrimLeading => "去除行首空白（选区行/全文）", "Trim leading whitespace (selection lines or whole file)" },
    { HkTrimBoth => "去除行首尾空白（选区行/全文）", "Trim whitespace at both ends (selection lines or whole file)" },
    { HkSortLinesAsc => "行升序排序（选区行/全文）", "Sort lines ascending (selection lines or whole file)" },
    { HkSortLinesDesc => "行降序排序（选区行/全文）", "Sort lines descending (selection lines or whole file)" },
    { HkDedupeLines => "去除重复行（选区行/全文）", "Remove duplicate lines (selection lines or whole file)" },
    { HkBookmarkToggle => "切换当前行书签", "Toggle a bookmark on the current line" },
    { HkBookmarkNext => "跳到下一个书签", "Jump to the next bookmark" },
    { HkBookmarkPrev => "跳到上一个书签", "Jump to the previous bookmark" },
    { HkBookmarkClearAll => "清除全部书签", "Clear all bookmarks" },
    { HkCopyMarkedLines => "复制全部标记行", "Copy all bookmarked lines" },
    { HkDelMarkedLines => "删除全部标记行", "Delete all bookmarked lines" },
    { HkJumpBracket => "跳到配对括号", "Jump to the matching bracket" },
    { HkTabsToSpacesLeading => "行首制表符转空格（选区行/全文）", "Leading tabs to spaces (selection lines or whole file)" },
    { HkTabsToSpacesAll => "全部制表符转空格（选区行/全文）", "All tabs to spaces (selection lines or whole file)" },
    { HkSpacesToTabsLeading => "行首空格转制表符（选区行/全文）", "Leading spaces to tabs (selection lines or whole file)" },
    { HkMergeLines => "合并行（触及块合成一行/并入下一行）", "Join lines (the affected block becomes one line, or joins the next)" },
    { HkSplitLine => "拆分行（光标处断行/选区独立成行）", "Split lines (break at the cursor, or make each selection its own line)" },
    { HkDelEmptyLines => "删除空行（选区行/全文）", "Delete empty lines (selection lines or whole file)" },
    { HkDelBlankLines => "删除空白行（含纯空白行）", "Delete blank lines (including whitespace-only ones)" },
    { HkFindAllPanel => "查找全部结果面板", "Find-all results panel" },
    { HkInsertDateTime => "插入日期时间（YYYY-MM-DD HH:MM）", "Insert date and time (YYYY-MM-DD HH:MM)" },
    { HkCopyFilePath => "复制完整路径（当前页）", "Copy the full path (current tab)" },
    { HkCopyFileName => "复制文件名（当前页）", "Copy the file name (current tab)" },
    { HkToggleLineComment => "切换行注释（按语法选 // # -- 等）", "Toggle line comment (// # -- … by syntax)" },
    { HkReopenClosedTab => "恢复上次关闭的标签页", "Reopen the last closed tab" },
    { HkNewTab => "新建标签页", "New tab" },
    { HkCloseTab => "关闭当前标签页", "Close the current tab" },
    { HkNextTab => "循环切换标签页", "Cycle to the next tab" },
    { HkFormatJson => "格式化 JSON（仅 JSON 文件）", "Format JSON (JSON files only)" },
    { HkDocStart => "跳到文档首", "Go to the start of the document" },
    { HkDocEnd => "跳到文档尾", "Go to the end of the document" },
    { HkFindNext => "查找下一个", "Find next" },
    { HkFindPrev => "查找上一个", "Find previous" },
    { HkPrevTab => "循环切换标签页（反向）", "Cycle to the previous tab" },
    { HkWordLeft => "跳到上一个词首", "Jump to the previous word" },
    { HkWordRight => "跳到下一个词尾", "Jump to the next word" },
    { HkDelWordLeft => "删除到词首", "Delete to the start of the word" },
    { HkDelWordRight => "删除到词尾", "Delete to the end of the word" },
    { HkReverseLines => "行序反转（选区行/全文）", "Reverse line order (selection lines or whole file)" },
    { HkSortLinesNumericAsc => "按数值升序（行首整数为键，无数字行排末尾）", "Sort by number ascending (leading integer as key; lines without one go last)" },
    { HkSortLinesNumericDesc => "按数值降序（行首整数为键）", "Sort by number descending (leading integer as key)" },
    { HkSortLinesLengthAsc => "按行长升序", "Sort by line length ascending" },
    { HkSortLinesLengthDesc => "按行长降序", "Sort by line length descending" },
    { HkDedupeConsecutiveLines => "去连续重复行（保首次出现）", "Remove consecutive duplicate lines (keep the first)" },
    { HkToTitlecase => "词首大写（选区/全文；数字带 = 扩展转换助记）", "Title case words (selection or whole file; the number row holds the extended conversions)" },
    { HkToggleReadOnly => "切换只读锁定（编辑与撤销拒收）", "Toggle read-only lock (edits and undo are rejected)" },
    { HkOpenContainingFolder => "打开所在文件夹（资源管理器定位当前文件）", "Open the containing folder (reveal the current file)" },
    { HkToggleFullscreen => "切换全屏（全屏期间不记忆窗口几何）", "Toggle full screen (window geometry is not remembered while full screen)" },
    { HkToggleAlwaysOnTop => "切换窗口置顶", "Toggle always-on-top" },
    { HkToolBase64Encode => "Base64 编码（选区）", "Base64 encode (selection)" },
    { HkToolBase64Decode => "Base64 解码（选区）", "Base64 decode (selection)" },
    { HkToolUrlEncode => "URL 编码（选区）", "URL encode (selection)" },
    { HkToolUrlDecode => "URL 解码（选区）", "URL decode (selection)" },
    { HkToolMd5 => "计算 MD5 替换选区", "Replace the selection with its MD5" },
    { HkToolSha256 => "计算 SHA-256 替换选区", "Replace the selection with its SHA-256" },
    { HkCommandPalette => "命令面板（模糊搜全部命令）", "Command palette (fuzzy-search every command)" },
    { HkQuickSwitchTab => "快速切换标签页（模糊跳转）", "Quick tab switcher (fuzzy jump)" },
    { HkToggleMonitorFile => "切换当前页文件监视（tail 跟随）", "Toggle file monitoring for the current tab (tail follow)" },
    { HkColumnEditor => "列编辑器（向列块插入序号/文本）", "Column editor (insert numbers or text into a column block)" },
    { HkAddNextMatch => "多光标：添加下一匹配（环形搜索）", "Multi-cursor: add the next match (wraps around)" },
    { HkFindInFiles => "在文件中查找（当前页所在目录，可换目录）", "Find in files (folder of the current tab; can be changed)" },
}

// ---------- 带参数的文案组装 ----------
//
// 约定：一条带参文案 = 「前缀键 + 值 + 后缀键」，两侧顺序在两种语言里恰好
// 都成立（中文用全角括号、英文用半角），故不需要 printf 式的模板引擎。

/// 「{值}{后缀}」——值在前缀型（如 `正则无效：{e}` → 前缀键 + e）。
pub fn fmt_suffix(lang: Lang, key: Key, value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 24);
    s.push_str(key.text(lang));
    s.push_str(value);
    s
}

/// 「{前缀键}{值}{后缀键}」——值夹在中间（如 `没有匹配「{q}」的设置`）。
pub fn fmt_wrapped(lang: Lang, pre: Key, value: &str, post: Key) -> String {
    let mut s = String::with_capacity(value.len() + pre.text(lang).len() + post.text(lang).len() + 8);
    s.push_str(pre.text(lang));
    s.push_str(value);
    s.push_str(post.text(lang));
    s
}

/// 当前匹配计数（`第 3/12 处匹配` / `Match 3 of 12`）。
pub fn fmt_match_counter(lang: Lang, index: usize, total: usize) -> String {
    match lang {
        Lang::ZhCn => format!("第 {}/{} 处匹配", index, total),
        Lang::En => format!("Match {index} of {total}"),
    }
}

/// 「全部匹配：N 处」/「N matches」。
pub fn fmt_match_total(lang: Lang, total: usize) -> String {
    match lang {
        Lang::ZhCn => format!("全部匹配：{total} 处"),
        Lang::En => match total {
            1 => "1 match".to_owned(),
            n => format!("{n} matches"),
        },
    }
}

/// 查找浮层标题右侧的计数（`第 i/total 处` / `i of total`；无命中给总数）。
pub fn fmt_find_counter(lang: Lang, index: Option<usize>, total: usize) -> String {
    match lang {
        Lang::ZhCn => match index {
            Some(i) => format!("第 {}/{} 处", i + 1, total),
            None => format!("{total} 处"),
        },
        Lang::En => match index {
            Some(i) => format!("{} of {total}", i + 1),
            None => format!("{total} matches"),
        },
    }
}

/// 字体挑选列表被单帧行数上限截断时的提示（`…共 N 个命中，请继续输入
/// 关键字缩小范围` / `…N matches in total; keep typing to narrow it down`）。
pub fn fmt_font_picker_more(lang: Lang, total: usize) -> String {
    match lang {
        Lang::ZhCn => format!("…共 {total} 个命中，请继续输入关键字缩小范围"),
        Lang::En => format!("…{total} matches in total; keep typing to narrow it down"),
    }
}

/// 全部替换结果（`已替换 N 处` / `Replaced N`）。
pub fn fmt_replaced(lang: Lang, count: usize) -> String {
    match lang {
        Lang::ZhCn => format!("已替换 {count} 处"),
        Lang::En => match count {
            1 => "Replaced 1 occurrence".to_owned(),
            n => format!("Replaced {n} occurrences"),
        },
    }
}

/// 在文件中查找的扫描摘要（`在文件中查找：N 个文件 M 处`）。
pub fn fmt_fif_summary(lang: Lang, files: usize, hits: usize) -> String {
    match lang {
        Lang::ZhCn => format!(
            "{}{} 个文件共 {} 处",
            Key::FindFilesSummary.text(lang),
            files,
            hits
        ),
        Lang::En => format!(
            "{}{} file(s), {} hit(s)",
            Key::FindFilesSummary.text(lang),
            files,
            hits
        ),
    }
}

/// 结果面板标题（`在文件中查找：N 个文件 M 处` + 可选封顶后缀）。
pub fn fmt_fif_panel_title(lang: Lang, files: usize, hits: usize, truncated: bool) -> String {
    let mut s = match lang {
        Lang::ZhCn => format!("在文件中查找：{files} 个文件 {hits} 处"),
        Lang::En => format!("Find in Files: {files} file(s), {hits} hit(s)"),
    };
    if truncated {
        s.push_str(Key::FifReachedCapSuffix.text(lang));
    }
    s
}

/// 结果面板里的单个文件头（`relative/path  （3 处）`）。
pub fn fmt_file_hits(lang: Lang, rel: &str, hits: usize) -> String {
    format!(
        "{}  {}{}{}",
        rel,
        Key::FindFileHitsPrefix.text(lang),
        hits,
        Key::FindFileHitsSuffix.text(lang)
    )
}

/// 命中的中文计数后缀（` 处匹配` / ` matches`）。
pub fn match_counter_suffix(lang: Lang) -> &'static str {
    Key::StMatchCounter.text(lang)
}

/// 会话恢复摘要的一页（`N 页未能恢复原内容` / `N page(s) could not be restored`）。
pub fn fmt_restore_note(lang: Lang, count: usize, dropped: bool) -> String {
    match lang {
        Lang::ZhCn => format!(
            "{count} 页{}",
            if dropped {
                "超出内存护栏未恢复"
            } else {
                "未能恢复原内容"
            }
        ),
        Lang::En => format!(
            "{count} page(s) {}",
            if dropped {
                "exceeded the memory guard and were not restored"
            } else {
                "could not be restored"
            }
        ),
    }
}

/// 「N 字符」选区计数（`选 N 字符` / `Sel N chars`）。
pub fn fmt_selection(lang: Lang, chars: usize) -> String {
    match lang {
        Lang::ZhCn => format!("选 {chars} 字符"),
        Lang::En => format!("Sel {chars} chars"),
    }
}

/// 「未命名N」默认标签名。**文件名不进 i18n 文案表**，故单独在此给两种
/// 语言的前缀；带扩展名的变体由 app 层拼接（`未命名1.txt` / `Untitled1.txt`）。
pub fn untitled_prefix(lang: Lang) -> &'static str {
    match lang {
        Lang::ZhCn => "未命名",
        Lang::En => "Untitled",
    }
}

/// `N 页…` 会话恢复里的「N 页」量词（供 app 层复用）。
pub fn fmt_page_count(lang: Lang, count: usize) -> String {
    match lang {
        Lang::ZhCn => format!("{count} 页"),
        Lang::En => format!("{count} page(s)"),
    }
}

/// `core::CoreError` 的本地化文案（core 是纯逻辑层，错误类型本身不带
/// 语言；展示侧在此按当前语言组装）。
pub fn fmt_core_error(lang: Lang, err: &crate::CoreError) -> String {
    match err {
        crate::CoreError::Read { path, source } => match lang {
            Lang::ZhCn => format!("无法读取文件 {}: {source}", path.display()),
            Lang::En => format!("Cannot read file {}: {source}", path.display()),
        },
        crate::CoreError::Write { path, source } => match lang {
            Lang::ZhCn => format!("无法写入文件 {}: {source}", path.display()),
            Lang::En => format!("Cannot write file {}: {source}", path.display()),
        },
        crate::CoreError::BinaryDetected { path } => match lang {
            Lang::ZhCn => format!(
                "{} 疑似二进制文件（含 NUL 字节或替换符占比过高），已拒绝打开以保护原文件",
                path.display()
            ),
            Lang::En => format!(
                "{} looks like a binary file (NUL bytes or too many replacement characters); opening was refused to protect the original",
                path.display()
            ),
        },
    }
}

/// 语言下拉框的一个条目。
///
/// `iced` 的 `pick_list` 要求选项实现 `ToString`（下拉框直接用它的
/// `Display` 画每一行），而语言名本身要按当前界面语言决定显示方式——
/// 故用这个「值 + 当前界面语言」的小包装承载上下文。
///
/// **判等只看 [`LangOption::lang`]**（`ui` 是渲染上下文，不参与身份）：
/// 否则切语言后 `pick_list` 会认为「选中项不再在选项列表里」而把选中态
/// 显示成占位符。
#[derive(Debug, Clone, Copy, Eq)]
pub struct LangOption {
    /// 该条目代表的语言。
    pub lang: Lang,
    /// 当前界面语言（决定 `Display` 取哪种写法）。
    pub ui: Lang,
}

impl PartialEq for LangOption {
    fn eq(&self, other: &Self) -> bool {
        self.lang == other.lang
    }
}

impl std::hash::Hash for LangOption {
    /// 与 [`PartialEq`] 保持契约：只哈希参与判等的字段（`ui` 是渲染
    /// 上下文，不参与身份）。derive 会连 `ui` 一起哈希，clippy 的
    /// `derived_hash_with_manual_eq` 正是拦这个。
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.lang.hash(state);
    }
}

impl LangOption {
    /// 全部条目（按 [`Lang::ALL`] 顺序）。
    pub fn all(ui: Lang) -> Vec<LangOption> {
        Lang::ALL.into_iter().map(|lang| LangOption { lang, ui }).collect()
    }
}

impl fmt::Display for LangOption {
    /// 语言名**用自己的文字**书写（与 Windows/浏览器一致）——认不出当前
    /// 界面语言的人也能在下拉框里找到自己的语言。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.lang.native_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 语言短码往返 + 宽松归一（大小写/下划线/地区后缀/空白）。
    #[test]
    fn lang_codes_normalize_and_round_trip() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), lang, "短码往返");
        }
        for raw in ["en", "EN", "en_US", "  en-GB ", "en-AU"] {
            assert_eq!(Lang::from_code(raw), Lang::En, "{raw:?} 应归一为 en");
        }
        for raw in ["zh", "zh-CN", "ZH_cn", "zh-Hans", "zh-SG", "", "??", "ja"] {
            assert_eq!(Lang::from_code(raw), Lang::ZhCn, "{raw:?} 应归一为 zh-CN");
        }
    }

    /// serde 往返 = 落盘短码；未知值不报错、回落默认语言。
    #[test]
    fn lang_serde_round_trip_and_unknown_fallback() {
        // 语言只作为配置字段出现，故用 toml 往返（与 config.toml 同一条路径）
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            language: Lang,
        }
        for lang in Lang::ALL {
            let text = toml::to_string(&Wrapper { language: lang }).expect("序列化");
            assert!(
                text.contains(&format!("\"{}\"", lang.code())),
                "落盘必须是短码：{text:?}"
            );
            let back: Wrapper = toml::from_str(&text).expect("反序列化");
            assert_eq!(back.language, lang);
        }
        // 未知语言码：整体读取不失败，回落中文简体（配置损坏不阻断启动）
        let unknown: Wrapper = toml::from_str("language = \"fr\"").expect("未知码不应报错");
        assert_eq!(unknown.language, Lang::ZhCn);
    }

    /// 语言名自述用本族文字（英文字面量与中文界面语言无关）。
    #[test]
    fn lang_native_names_are_self_describing() {
        assert_eq!(Lang::ZhCn.native_name(), LANG_NAME_ZH_CN);
        assert_eq!(Lang::En.native_name(), LANG_NAME_EN);
        for lang in Lang::ALL {
            assert!(!lang.native_name().is_empty());
        }
    }

    /// 下拉框条目：全语言覆盖 + `Display` = 语言名自述。
    #[test]
    fn lang_options_cover_all_languages_and_display_native_names() {
        let options = LangOption::all(Lang::En);
        assert_eq!(options.len(), Lang::ALL.len());
        for (opt, lang) in options.iter().zip(Lang::ALL) {
            assert_eq!(opt.lang, lang);
            assert_eq!(opt.to_string(), lang.native_name());
        }
        // ui 字段不影响条目标识（pick_list 用它判等）
        assert_eq!(
            LangOption { lang: Lang::En, ui: Lang::ZhCn },
            LangOption { lang: Lang::En, ui: Lang::En }
        );
    }

    /// 设置行键必须两两互异（控件匹配按字符串判等，撞键即错配）。
    #[test]
    fn row_keys_are_unique_and_ascii() {
        let rows = [
            ROW_LANGUAGE,
            ROW_THEME,
            ROW_SHOW_WHITESPACE,
            ROW_SHOW_LINE_ENDINGS,
            ROW_WORD_WRAP,
            ROW_INDENT_GUIDES,
            ROW_EDGE_COLUMN,
            ROW_FONT_FAMILY,
            ROW_FONT_SIZE,
            ROW_AUTOSAVE,
            ROW_AUTOSAVE_DELAY,
            ROW_BACKUP_MODE,
            ROW_REMEMBER_RECENT,
            ROW_SNAPSHOTS,
            ROW_REMEMBER_SESSION,
            ROW_EXIT_MODE,
            ROW_SNAPSHOT_INTERVAL,
            ROW_ABOUT_NAME,
            ROW_ABOUT_VERSION,
            ROW_ABOUT_RENDERER,
            ROW_ABOUT_LICENSE,
        ];
        for (i, a) in rows.iter().enumerate() {
            assert!(a.is_ascii(), "行键必须语言无关（ASCII）：{a}");
            assert!(!a.is_empty());
            for b in &rows[i + 1..] {
                assert_ne!(a, b, "行键重复：{a}");
            }
        }
    }

    /// 最后一道防线：每条文案在每种语言下都非空，键在两个语言分支里
    /// 一一对应（`Key::ALL` 与枚举同源，穷尽 match 保证编译期覆盖）。
    /// 白名单 = 设计上就是空串的**前缀/后缀**片段（值由代码拼接）。
    #[test]
    fn every_key_has_non_empty_text_in_every_language() {
        const EMPTY_BY_DESIGN: &[Key] = &[
            Key::DiscardOpenPrefix,
            Key::FindAllTruncatedPrefix,
            Key::FindFileHitsPrefix,
            Key::SaveAsEncodingSuffix,
            Key::StSessionRestoreDroppedPrefix,
            Key::StSessionRestoreFailedPrefix,
        ];
        for k in Key::ALL {
            let k = *k;
            for lang in Lang::ALL {
                let t = k.text(lang);
                if EMPTY_BY_DESIGN.contains(&k) {
                    continue;
                }
                assert!(!t.trim().is_empty(), "{k:?} 在 {lang:?} 下为空");
            }
        }
    }

    /// 英文文案里不应出现汉字（防止新增键时英文分支直接抄中文）。
    /// 允许含中文的例外：专有名词/单位字样（如「Editpad 轻量文本编辑器」
    /// 一类的品牌串不在此表——此处只做「整条不是中文」的粗筛）。
    #[test]
    fn english_table_never_uses_han_characters() {
        for k in Key::ALL {
            let t = en(*k);
            assert!(
                !t.chars().any(|c| matches!(c, '\u{4e00}'..='\u{9fff}')),
                "{k:?} 的英文文案含汉字：{t:?}"
            );
        }
    }
}
