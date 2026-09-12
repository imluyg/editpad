// P24：发布版隐藏随 GUI 一起弹出的控制台黑窗（Windows 子系统属性）；
// 调试构建保留控制台，便于直接 cargo run 看日志输出。
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

//! Editpad —— 轻量文本编辑器。
//!
//! M2 里程碑：自绘虚拟化编辑器接管渲染，ropey Document 成为唯一数据源。
//!
//! * 渲染只为可见行排版（见 editor.rs）——50MB 与 5KB 的每帧成本相同；
//! * 键盘编辑在应用层订阅统一分发；鼠标与输入法在控件层处理；
//! * 撤销/重做基于 rope 的廉价结构共享快照。

mod editor;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};

use iced::futures::SinkExt;
use iced::keyboard::{self, key::Named};
// Phase 2b：以下两组为各子模块（load/find_scan/.../settings_ui/update/view/tests）
// 经 `use super::*` 消费的共享导入枢纽——main.rs 自身不再直接使用，故显式
// pub(crate) 再导出（子模块的 unqualified 引用经由 glob 链解析）。
pub(crate) use iced::widget::{button, checkbox, container, mouse_area, opaque, progress_bar, row, rule,
    scrollable, text, text_input, Stack};
pub(crate) use iced::{border::Radius, stream, window, Alignment, Background, Border, Color, Element, Fill,
    Font, Padding, Point, Shadow, Subscription, Task, Theme};

use editor::{BlankKind, CaseKind, EditorHandle, EditOp, Motion, SortOrder, TabSpaceKind, ToolKind, TrimMode};

fn main() -> iced::Result {
    // `--help` / `--version` 必须在单实例判定与 iced 初始化之前处理：
    // 两者都有副作用（迁移遗留数据 / 建窗），而用户此刻并不想开窗口。
    // 修前这两个选项被当「未知选项」静默忽略，`editpad --help` 的结果是
    // 打开一个空白窗口——正是用户第一反应会试的入口。
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(opt) = parse_cli_option(argv.iter().cloned()) {
        emit_cli_text(match opt {
            CliOption::Help => HELP_TEXT,
            CliOption::Version => VERSION_LINE,
        });
        return Ok(());
    }
    // P103：命令行参数 = 待打开文件。资源管理器「双击文件」/右键
    // 「打开方式」/多选右键「打开」都会把文件路径作为 argv 传给本程序
    // （拖动进窗口走 FileDropped 事件，是另一条路）。必须在单实例
    // 判定之前解析——第二实例要靠它决定「转发」还是「弹提示」
    let cli_files = parse_cli_file_args(argv);
    // 单实例互斥：同一份拷贝（同 exe 位置 → 同实例键）只允许一个进程。
    // 有待开文件的第二实例把路径**转发**给已运行实例（握手文件 + 轮询，
    // 在新标签页打开）后静默退出；裸启动的第二实例才弹提示。
    // 拦截必须先于任何 iced/配置初始化——配置加载会触发遗留数据迁移
    // 等副作用，绝不与已运行实例并发
    if !single_instance::acquire_single_instance() {
        if cli_files.is_empty() {
            single_instance::notify_already_running();
        } else {
            single_instance::forward_pending_open(&cli_files);
        }
        return Ok(());
    }
    // iced 0.14：第一个参数是 boot 函数（返回初始状态），title/theme/subscription 走 builder
    // 第 76 轮（用户点单）：窗口标题栏图标 = 应用自定义 Logo（P71 四档
    // ICO 的 48px 条目；解析失败回退 exe 资源图标，见 window_title_icon）
    // P102：窗口几何记忆——按上次关闭的位置/尺寸建窗（缺省回退
    // 默认尺寸居中；配置损坏等异常均回退，不影响启动）
    let geometry = editpad_core::Settings::load();
    let (window_size, window_position) = restore_window_geometry(&geometry);
    // boot 函数不收参数，用闭包捕获文件清单传入——boot 仅在事件循环
    // 启动时调用一次，clone 开销可忽略。
    iced::application(move || Editpad::new(cli_files.clone()), Editpad::update, Editpad::view)
        .title(Editpad::title)
        .theme(Editpad::theme)
        .subscription(Editpad::subscription)
        .window(window::Settings {
            icon: window_title_icon(),
            size: window_size,
            position: window_position,
            ..window::Settings::default()
        })
        // 关闭请求必须以事件流转到 subscription（exit_on_close_request 默认 true，
        // 不显式关掉的话点 X 会直接退进程，永远轮不到未保存确认）
        .exit_on_close_request(false)
        .run()
}

/// `--version` 输出（单行）。
const VERSION_LINE: &str = concat!("editpad ", env!("CARGO_PKG_VERSION"));

/// `--help` 输出。版本号经 `concat!` 编进常量，零运行时开销。
const HELP_TEXT: &str = concat!(
    "editpad ",
    env!("CARGO_PKG_VERSION"),
    " —— 轻量文本编辑器\n",
    "\n",
    "用法:\n",
    "  editpad                 启动（恢复上次会话）\n",
    "  editpad <文件>...       打开一个或多个文件（逐个开成标签页）\n",
    "\n",
    "选项:\n",
    "  -h, --help              显示本帮助后退出\n",
    "  -V, --version           显示版本号后退出\n",
    "      --                  其后全部按文件路径处理（含以 - 开头的文件名）\n",
    "\n",
    "说明:\n",
    "  资源管理器双击文件、右键「打开方式」、多选打开，都会把路径作为命令行\n",
    "  参数传入。同一份拷贝同时只运行一个实例，第二实例带文件参数时会把路径\n",
    "  转发给已运行的窗口后自行退出。\n",
);

/// 启动期就要处理的命令行选项（与待开文件互斥）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliOption {
    Help,
    Version,
}

/// 识别 `--help` / `--version`（纯函数便于单测）。
///
/// 与 [`parse_cli_file_args`] 同口径：跳过程序自身路径，`--` 之后停止
/// 扫描（让 `-- --help` 这样的合法文件名不被误判成选项）。
fn parse_cli_option<I>(args: I) -> Option<CliOption>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    for arg in args.into_iter().skip(1) {
        match arg.to_string_lossy().as_ref() {
            "--" => break,
            "--help" | "-h" => return Some(CliOption::Help),
            "--version" | "-V" => return Some(CliOption::Version),
            _ => {}
        }
    }
    None
}

/// 输出命令行文本（`--help` / `--version` 的结果）。
///
/// 调试构建（有控制台）与非 Windows 平台走 stdout；**发布构建的 Windows
/// 版没有控制台**（P24 windows 子系统），改弹原生消息框——与单实例提示
/// 同一手法，否则用户敲完 `--help` 什么也看不到。
fn emit_cli_text(text: &str) {
    #[cfg(test)]
    let _ = text;
    #[cfg(not(test))]
    {
        #[cfg(any(debug_assertions, not(windows)))]
        println!("{text}");
        #[cfg(all(not(debug_assertions), windows))]
        single_instance::show_message("Editpad", text);
    }
}

/// 收集命令行传入的待打开文件路径（P103：双击文件 / 「打开方式」/
/// 多选右键「打开」时资源管理器把全部路径作为 argv 传给本程序）。
///
/// 约定（纯函数便于单测）：
/// * 第 0 个参数是程序自身路径，跳过；
/// * `--` 之后全部视为文件路径（让以 `-` 开头的合法文件名也能打开）；
/// * 其余以 `-` 开头的参数按未知选项忽略（本程序尚无任何命令行选项）；
/// * 其余按文件路径原样收集——用 `args_os` 全程不经 UTF-8 转换，
///   非 Unicode 文件名（GBK 等）也能无损打开。
fn parse_cli_file_args<I>(args: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut files = Vec::new();
    let mut options_done = false;
    for arg in args.into_iter().skip(1) {
        if options_done {
            files.push(PathBuf::from(arg));
            continue;
        }
        let lossy = arg.to_string_lossy();
        if lossy == "--" {
            options_done = true;
        } else if lossy.starts_with('-') {
            // 未知选项：忽略并继续（无选项可解析，任何 `-x` 都不可能是文件）
        } else if !lossy.is_empty() {
            files.push(PathBuf::from(arg));
        }
    }
    files
}

/// P102：把配置里的窗口几何换算成 iced 建窗参数。值域钳制防坏配置
/// （越界坐标/极端尺寸）建出用户无法操作的窗口；缺省 = 1024×768 居中。
fn restore_window_geometry(s: &editpad_core::Settings) -> (iced::Size, window::Position) {
    const MAX_GEOM: f32 = 16384.0;
    let size = match (s.window_width, s.window_height) {
        (Some(w), Some(h))
            if w.is_finite() && h.is_finite() && w >= 400.0 && h >= 300.0 =>
        {
            iced::Size::new(w.min(MAX_GEOM), h.min(MAX_GEOM))
        }
        _ => iced::Size::new(1024.0, 768.0),
    };
    let position = match (s.window_x, s.window_y) {
        // i32 全幅理论上界以内才采用（防坏值把窗口建到屏幕外十万像素）
        (Some(x), Some(y)) if x.abs() < (1 << 20) && y.abs() < (1 << 20) => {
            window::Position::Specific(Point::new(x as f32, y as f32))
        }
        _ => window::Position::Centered,
    };
    (size, position)
}

#[derive(Debug, Clone)]
enum Message {
    /// 编辑器按键编辑（字符、删除、移动……）
    Edit(EditOp),
    /// 控件层通知：光标/选区变了（刷新状态栏）
    EditorNavChanged,

    /// P125：Insert 切换覆写/插入模式（每页独立，不入注册表）
    ToggleOverwrite,
    /// P126：切换当前页只读锁定（Ctrl+R）
    ToggleReadOnly,
    /// P126：资源管理器定位当前文件所在文件夹（Ctrl+Shift+V）
    OpenContainingFolder,
    /// P126：切换全屏（F11；全屏期间窗口几何不记忆）
    ToggleFullscreen,
    /// P126：切换窗口置顶（F9）
    ToggleAlwaysOnTop,
    /// P129：打开/切换/关闭命令面板（目标模式；同模式再按 = 关闭）
    PaletteToggled(crate::state::PaletteMode),
    /// P129：面板查询串变化（重置选中行）
    PaletteInputChanged(String),
    /// P129：面板选中行移动（true = 向下）
    PaletteMove(bool),
    /// P129：执行面板当前选中项并关闭
    PaletteExecute,
    /// P129：鼠标点击面板某行 = 选中并立即执行
    PalettePick(usize),
    /// P130：切换当前页文件监视（F8；tail 跟随日志场景）
    ToggleMonitorFile,
    /// P130：监视巡检拍（自我续期链，2s 一拍）
    MonitorTick,

    /// 复制当前选区到系统剪贴板（Ctrl+C；无选区时无操作）
    CopyRequested,
    /// 剪切 = 复制 + 删除选区（Ctrl+X；无选区时无操作）
    CutRequested,
    /// 请求读系统剪贴板（Ctrl+V），内容经 Pasted 回来
    PasteRequested,
    /// 剪贴板读取完成：按普通插入入文
    /// （换行归一由 insert_str 按文档主导行尾完成，P9 起不再在 app 层预处理）
    Pasted(String),

    OpenRequested,
    FileChosen(Option<PathBuf>),
    /// 后台加载进度：(任务 id, 已读字节, 总字节)
    LoadProgress(u64, u64, u64),
    /// 后台加载完成：(任务 id, (rope 直入的文档, 嗅探样本, 编码标签))。
    /// P19 起携带 Document，不再有全量 String 中转；
    /// P22 起附带解码头部样本供语言嗅探
    Loaded(
        u64,
        Result<(editpad_core::Document, String, String), String>,
    ),
    SaveRequested,
    SaveAsRequested,
    SaveTargetChosen(Option<PathBuf>),
    /// 保存完成：(发起页 id, 落盘内容的内容版本号, 结果)（P18 版本守卫；
    /// P67 起成功分支携带编码落盘的附带信息，如 GBK 不可映射字符告警）。
    /// P146：按页 id 归账——保存异步期间切页/关页曾把账目记到「完成时刻
    /// 的活动页」上，错清别页置脏标记。
    Saved(u64, u64, Result<editpad_core::EncodeNotice, String>),
    /// 标签页保存完成（「保存并关闭」流程用）：(页 id, 快照版本, 结果)
    TabSaved(u64, u64, Result<(), String>),
    /// 自动保存完成（P63 载荷扩展）：(页 id, 快照版本, 调度时页路径,
    /// 结局)。版本不符 = 期间又有编辑，不清脏；页 id 解析不到 = 页已被
    /// 关（账目整条丢弃）；结局四分见 [`AutosaveOutcome`]。
    TabAutosaved(u64, u64, PathBuf, AutosaveOutcome),
    /// P67：状态栏「编码」标签点开的弹出菜单开关
    ToggleEncodingMenu,
    /// P67：状态栏「行尾」标签点开的弹出菜单开关
    ToggleEolMenu,
    /// P67：以指定编码保存当前页（记住偏好并立即走保存管线）
    SaveWithEncoding(editpad_core::SaveEncoding),
    /// P67：把当前页行尾统一转换为目标风格（可撤销的文档编辑）
    ConvertEol(editpad_core::LineEnding),

    FindToggled,
    /// 查找浮层拖动：光标移动（窗口坐标；`on_move` 层每帧上报）
    FindCursorMoved(Point),
    /// 查找浮层拖动开始（按住卡片顶部拖动条）
    FindDragStart,
    /// 查找浮层拖动结束（左键松开；`on_release` 不捕获事件，不影响正文拖拽）
    FindDragEnd,
    /// 双击拖动条：浮层位置复位到默认（窗口中间偏上）
    FindDragReset,
    /// P153：正文内容被左键按下（自绘控件内、非浮层区域）——查找栏开着时
    /// 让查找框转半透明，把被遮挡的正文让出来（见 `Editpad::find_dimmed`）
    EditorBodyPressed,
    /// P153：查找卡片被点击（卡片内任意未被内部控件消费的按下，含拖动条
    /// 与空白处）——半透明态复位为不透明
    FindBoxPressed,
    FindQueryChanged(String),
    FindNext,
    FindPrev,
    CaseToggled(bool),
    /// 整词匹配开关（命中前后均非词字符；正则模式下不参与）
    WholeWordToggled(bool),
    ReplaceQueryChanged(String),
    ReplaceCurrent,
    /// P70：正则模式的「替换当前」（对当前命中做 $1 展开替换）
    ReplaceCurrentRegex,
    ReplaceAll,
    /// P148：正则「全部替换」后台计算完成——替换本体曾同步跑在 UI 线程，
    /// 回溯引擎对病态模式 + 大文档会冻结整个应用（回溯限制的是单次尝试
    /// 步数，全文逐位置尝试的总量无界）
    ReplaceAllRegexDone(Result<(String, usize), String>),
    /// 后台查找扫描完成：(任务序号, 命中表)。序号过期的结果直接丢弃（P10）
    FindScanDone(u64, Vec<editpad_core::MatchPos>),
    /// P70：正则模式开关（开启/关闭都会触发重扫）
    RegexToggled(bool),
    // ---------- A8：在文件中查找（设计 docs/find-in-files-design.md） ----------
    /// FIF 模式开关：开启 = 打开查找栏 + 目录取当前页所在目录并立即
    /// 扫描；关闭 = 取消在途扫描并收起面板（与查找全部面板同槽互斥）
    FindInFilesToggled,
    /// 「浏览…」触发目录选择对话框（open/save 同款 busy 包裹）
    FifBrowseFolder,
    /// 「浏览…」选定目录（None = 用户取消对话框，不动现有目录）
    FifDirPicked(Option<PathBuf>),
    /// 后台目录扫描完成：(任务序号, 结果, 是否封顶截断)。序号过期丢弃
    FifScanDone(u64, Vec<crate::find_scan::FileHits>, bool),
    /// 点击结果面板某文件的某条命中：已开页切换跳转，未开页装载后跳转
    FifGoto(usize, usize),
    // ---------- 行操作扩充 + 查找全部（第 62 轮） ----------
    /// 「查找全部」结果面板开关（数据源 = 既有后台扫描的全量命中表）
    FindAllToggled,
    /// 点击结果面板第 `index` 条：选中该命中并滚动到可见
    FindAllGoto(usize),
    // ---------- 第 63 轮：插入日期时间 / 复制路径与文件名 ----------
    /// 复制完整路径到剪贴板：None = 活动页（热键），Some(i) = 第 i 页
    /// （标签右键菜单）。未命名页给状态栏提示不写剪贴板。
    CopyFilePath(Option<usize>),
    /// 复制文件名（含扩展名）到剪贴板，作用域口径同上
    CopyFileName(Option<usize>),
    // ---------- 第 64 轮：恢复上次关闭 / 行注释切换 ----------
    /// 恢复最近一次关闭的命名页（会话内记忆栈，不跨重启）；栈空 no-op
    ReopenLastClosedFile,
    /// 行注释切换（Ctrl+Q，前缀按语法查表）
    // （走 EditOp::ToggleLineComment，无独立消息）
    /// 设置：显示空白字符覆盖标记（第 64 轮，外观页）
    SettingsShowWhitespaceToggled(bool),
    /// 设置：显示行尾符标记（第 64 轮，外观页）
    SettingsShowLineEndingsToggled(bool),
    /// 设置：自动换行（软换行）开关（第 73 轮 ⑯，外观页；开启即生效：
    /// 正文按视口折行、水平滚动隐藏、列块禁用）
    SettingsWordWrapToggled(bool),
    /// 设置：缩进参考线开关（P132，外观页；默认开）
    SettingsIndentGuidesToggled(bool),
    /// 设置：右缘标尺列步进（P132，外观页；0 = 关，±4 步进，上限
    /// core::settings::MAX_EDGE_COLUMN）
    SettingsEdgeColumnDelta(i32),
    // ---------- P134：每页显示覆盖（路线图 C7） ----------
    /// Ctrl+滚轮：当前页字号覆盖步进（不动全局默认、不落盘；随会话快照）
    TabFontSizeDelta(f32),
    /// 查看菜单：清除当前页字号覆盖（回到跟随全局；无覆盖时禁用）
    TabFontSizeReset,
    /// 查看菜单：当前页自动换行三态循环（跟随全局 → 本页开 → 本页关
    /// → 跟随全局）
    TabWrapOverrideToggled,
    /// Ctrl+点击链接（P133，路线图 E2）：URL/file URI 外开，文件路径
    /// 内开（可选 `:行号` 跳行）
    LinkClicked(editor::LinkTarget),
    /// 设置：保存时备份模式循环切换（第 64 轮 ⑭，none→simple→
    /// timestamped→none，仿关窗行为的三态按钮）
    SettingsBackupModeToggled,
    // ---------- B9 列编辑器对话框（路线图 B9 Phase 2） ----------
    /// 开/关对话框（F6 / 编辑菜单 / 背板点击关闭共用；打开前置守卫 =
    /// 非 busy 且当前页有列块且未开软换行，拒绝时状态栏提示）
    ColumnEditorToggled,
    /// 文本/序号两模式切换
    ColumnEditorModeToggled,
    /// 草稿编辑（文本/起始/步长/补零宽四个输入框共用 String 载荷）
    ColumnEditorTextChanged(String),
    ColumnEditorStartChanged(String),
    ColumnEditorStepChanged(String),
    ColumnEditorWidthChanged(String),
    /// 进制循环切换：十进制→十六进制→二进制→八进制→十进制
    ColumnEditorBaseCycled,
    /// 十六进制字母大小写开关
    ColumnEditorHexUpperToggled,
    /// 确认：校验草稿 → 生成文本 → 关闭对话框 → 走 EditOp::InsertText
    /// 进列块插入管线（撤销/书签/失效汇点/busy/只读全继承）
    ColumnEditorConfirmed,
    // ---------- 第 69 轮：顶部菜单栏 ----------
    /// 菜单栏第 `idx` 个菜单开/关（0 文件 1 编辑 2 查看 3 视图 4 设置；
    /// 再次点击同项关闭，点背板/Esc 走 BarsDismissed）
    MenuToggled(usize),
    /// 指针在菜单栏条上移动（浮层菜单锚点数据源，仿标签条 P39 模式）
    MenubarHovered(Point),
    /// 菜单浮层背板被点击：落点在菜单栏条带内 = 切换到该菜单（主流
    /// 横移手感），否则收起（第 70 轮，位置取 MenubarHovered 最近值）
    MenubarPressed,

    /// 可见区高亮缺档超内联预算，请求安排后台分批补建（P12）。
    /// 同代在途时应用层幂等跳过，重复发布无害。
    HighlightPaveNeeded,

    // ---------- 多标签（P21） ----------
    /// 新建空标签页（Ctrl+T）
    NewTab,
    /// 第 76 轮：标签条右侧空白区被左键点击（连点两次 = 新建标签页）。
    /// 现有标签按钮自身不产生本消息（其 on_press 是 SwitchTab）——双击
    /// 现有标签仍走 P65 重命名语义，互不干扰。
    TabStripBlankPressed,
    /// 切到下一个标签页（Ctrl+Tab，循环）
    SwitchTabNext,
    /// 切到上一个标签页（Ctrl+Shift+Tab，循环）
    SwitchTabPrev,
    /// 切换到第 `i` 个标签页（标签条点击）
    SwitchTab(usize),
    /// 请求关闭当前标签页：干净即关；置脏则先弹放弃确认条
    CloseTabRequest,
    /// 确认放弃第 `idx` 页的更改并关闭（P21 骨架版唯一出口；
    /// 「保存后关闭」随完整版补齐）
    ConfirmCloseTabDiscard(usize),
    /// 取消标签页关闭确认
    CancelCloseTab,
    /// 保存第 `idx` 页并在成功后关闭（P21 完整版；未命名页不支持）
    CloseTabSave(usize),

    // ---------- 标签右键菜单（P28） ----------
    /// 在第 `idx` 个标签页上打开右键菜单（标签按钮外包 mouse_area 捕获右键；
    /// 左键仍由内部 button 消费，切换不受影响）。P39 起菜单为浮层，
    /// 打开时以最近指针位置为锚点。
    TabContextMenu(usize),
    /// 收起右键菜单（选中单项 / Esc / 点菜单外 / 切换标签时）
    TabContextMenuClosed,
    // ---------- 浮层弹窗定位（P39/P40） ----------
    /// 指针在标签条区域内移动（右键菜单锚点的数据源；仅标签条挂了
    /// on_move，消息量与悬停频率同量级，可忽略）
    CursorMoved(Point),
    /// P112：指针进入/离开第 `idx` 个页签（页签胶囊悬停底色的数据源；
    /// None = 已离开页签）。纯 UI 态不落盘；页签被关闭/移除时由关闭
    /// 路径一并清空，越界下标一律复位为 None。
    TabHovered(Option<usize>),
    /// 窗口逻辑尺寸变化（浮层贴边钳制的依据；启动时 winit 也会发一次）
    ViewportResized(f32, f32),
    /// P102：窗口被移动（逻辑坐标）——记录到设置并节流落盘
    WindowMoved(Point),
    /// 固定/取消固定第 `idx` 页：固定页豁免单页与批量关闭
    TogglePinTab(usize),
    /// 菜单「保存」：切到第 `idx` 页并复用既有活动页保存流
    /// （v1 决策：右键保存先切页，Saved 回报/最近文件/转码提示全部走活动页语义）
    SaveTabFromMenu(usize),
    /// 菜单「另存为/重命名」：切到第 `idx` 页；命名页走就地重命名输入框
    /// （P55），未命名页保留另存为对话框兜底（§3 P28 第 2 条）
    RenameOrSaveAsTab(usize),
    /// 就地重命名的输入框变化（纯 UI 态；提交时才校验与落盘）
    TabRenameInputChanged(String),
    /// 就地重命名提交：校验名称 → 磁盘改名 → 页路径/戳/最近文件迁移
    TabRenameCommitted,
    /// 就地重命名取消（× 按钮 / Esc），一切保持原状
    TabRenameCancelled,
    /// 关闭第 `idx` 页（CloseTabRequest 的参数化版本：置脏弹既有确认条，
    /// 固定页拒绝并提示）
    CloseTabAt(usize),
    /// 关闭除第 `keep` 页以外的全部非固定页（任一目标置脏 → 先聚合确认）
    CloseOtherTabs(usize),
    /// 关闭第 `from` 页右侧的全部非固定页（同上）
    CloseTabsRight(usize),
    /// 批量关闭确认条「放弃更改并关闭」：统一放弃目标列表各页并移除
    ConfirmBatchCloseDiscard,
    /// 取消批量关闭确认条
    CancelBatchCloseTabs,
    /// 切换 Markdown 预览面板（仅当前语法为 Markdown 时生效；P22 第三批）
    PreviewToggled,
    /// 光标闪烁心跳（打磨项）：翻转闪烁相位并触发重绘
    CaretTick,
    /// 周期快照心跳节拍（P31）：巡检置脏页，把内容有变化的页增量写进
    /// 快照区（自我续期链；至多一个提交在途）
    SnapshotHeartbeatTick,
    /// 周期快照心跳提交完成（P31）：按派发时刻的逐页计划回填账目
    HeartbeatDone(HeartbeatOutcome),
    /// 格式化 JSON（Ctrl+Shift+F，仅当前语法为 JSON 时生效；P22 第二批）
    FormatJson,
    /// 后台高亮铺建进度：(代次, 已铺检查点档位累计数)
    HlPaveProgress(u64, u64),
    /// 后台高亮铺建完成：(代次, 推进后的高亮器)。期间编辑过（换代）
    /// 则整体丢弃，缺口由下一帧重新评估续排（P12）。高亮器装箱：
    /// 该分支载荷远大于其余消息，装箱后整枚消息按值传参不再放大。
    HlPaved(u64, u64, Box<editpad_core::LazyHighlighter>),

    GotoToggled,
    GotoInputChanged(String),
    GotoSubmit,

    RecentsToggled,
    RecentSelected(String),
    /// P20：清空最近文件记录（写回空表并立即落盘）
    RecentsCleared,
    /// Esc 关闭全部浮动栏
    BarsDismissed,

    // ---------- 启动会话恢复（P30） ----------
    /// 崩溃恢复提示条「恢复」：按清单重建上次的标签页集合（含脏页内容）
    SessionRecoverAccepted,
    /// 崩溃恢复提示条「丢弃」：连快照一起丢（P29 放弃语义同族），空白起步
    SessionRecoverDiscarded,

    /// 窗口关闭请求（X 按钮/Alt+F4）：dirty 时转确认条，否则直接关窗
    CloseRequested(window::Id),
    /// 确认条「保存并关闭」：落盘成功（Saved(Ok)）后执行真正的窗口关闭
    ConfirmSaveAndClose,
    /// 确认条「放弃更改」：直接关窗
    DiscardAndClose,
    /// 确认条「取消」：留在应用
    CancelClose,
    /// 文件拖入窗口：按打开流程加载
    FileDropped(PathBuf),
    /// P103：弹出命令行排队中的下一个文件并按打开流程加载（boot 首发，
    /// 每次 Loaded 结算后续排——加载管线同一时刻只承接一个任务，多个
    /// 命令行文件必须串行；队列见 [`Editpad::pending_cli`]）
    OpenNextCliFile,
    /// 单实例转发轮询拍：读实例目录握手文件里的待开路径（无则空转续期）
    PendingOpenTick,
    /// 打开确认条「放弃更改并打开」：丢弃未保存修改并加载暂存路径
    ConfirmOpenDiscard,
    /// 打开确认条「取消」：留在当前文档
    ConfirmOpenCancel,

    /// 深浅主题切换（写回设置）
    ThemeToggled,
    /// P155：下拉框选中项（`pick_list` 按选项类型发消息，故单列一档，
    /// 由 update 解包转 [`Message::apply_language`] 并带上完整语言信息）。
    LanguageOptionSelected(editpad_core::LangOption),
    /// P154 兼容入口：界面语言循环切换（中文简体 → English → …）。
    /// 下拉框上线后设置页不再用它；保留给键盘/测试路径（故生产构建里
    /// 没有构造点，dead_code 豁免）。
    #[allow(dead_code)]
    LanguageToggled,
    /// 字号增减（±2.0，clamp 后写回设置并即时生效）
    FontSizeDelta(f32),

    // ---------- 设置弹窗（P27） ----------
    /// 打开/关闭设置弹窗（busy 时禁开；Esc 与关闭按钮退出）
    SettingsToggled,
    /// 自动保存开关（P18）翻转，写回设置
    SettingsAutosaveToggled(bool),
    /// 自动保存防抖秒数增减（±1，clamp 后写回）
    SettingsAutosaveDelayDelta(i32),
    /// 「记住最近文件」开关（P20）翻转，写回设置
    SettingsRememberRecentToggled(bool),
    /// 会话快照总开关（P29）翻转，写回设置
    SettingsSnapshotsToggled(bool),
    /// 启动会话恢复开关（P30）翻转，写回设置
    SettingsRememberSessionToggled(bool),
    /// 关窗行为切换：快照直退 ↔ 每次询问（P29），写回设置
    SettingsExitModeToggled,
    /// 周期快照心跳间隔秒数增减（±5，clamp 后写回；P31）
    SettingsIntervalDelta(i32),

    // ---------- 字体选择（P34） ----------
    /// 设置弹窗选中一个系统字体族（名字来自启动期枚举清单），写回设置
    SettingsFontSelected(String),
    /// 回退默认等宽字体（settings.font_family = None）
    SettingsFontReset,
    /// 设置弹窗的字体过滤输入框变化（仅影响列表展示，不落盘）
    FontFilterChanged(String),

    // ---------- 设置弹窗分类导航（P47，侧栏分类风格） ----------
    /// 侧栏选中一个分类页（同时清空搜索词，离开搜索态——同款语义）
    SettingsPageSelected(SettingsPage),
    /// 侧栏搜索框变化：内容区跨分类过滤命中行（纯 UI 态，不落盘）
    SettingsSearchChanged(String),

    // ---------- 热键可重映射（P62） ----------
    /// 设置热键页「修改」：进入捕获态，下一个可作热键的按键成为新组合
    HotkeyCaptureStarted(&'static str),
    /// 捕获态收到一个规范组合串：校验冲突后写映射并持久化
    HotkeyCaptureKey(String),
    /// 捕获态取消（Esc 在 update 内直接清除；此变体供测试/将来 UI × 按钮用）
    #[cfg_attr(not(test), expect(dead_code))]
    HotkeyCaptureCancel,
    /// 热键页「全部恢复默认」：清空重映射表（全默认组合）并持久化
    HotkeysResetAll,
    /// 订阅转发的原始按键（P62：分发挪到 update——重映射表与捕获态
    /// 必须读活状态，订阅闭包捕获会陈旧）
    KeyPressed(keyboard::Key, keyboard::Modifiers),

    // ---------- 外部修改检测（P50） ----------
    /// 窗口重新获得焦点：巡检各命名页的 (mtime, size) 戳，外部已改的
    /// 干净活动页静默重载，其余弹提示条由用户裁决
    WindowFocused,
    /// 提示条「重新加载」：放弃该页未保存内容并从磁盘重读
    ConfirmExternalReload(usize),
    /// 提示条「忽略」：以当前磁盘状态重记时间戳，直到下次再变不再提示
    IgnoreExternalChange(usize),
    /// 提示条「全部忽略」（聚合态）：队列内所有页一律重记时间戳并收条
    IgnoreAllExternalChanges,
}

// ---------- 后台查找扫描（P10） ----------

// ---------- 高亮后台分批补建（P12） ----------

/// P6 编码知情权（P67 口径升级）：保存前后编码不一致 = 发生不可逆转换，
/// 必须告知；用户在状态栏主动选择的目标编码不再视为「意外转码」。
/// `unmappable` = 有字符无法用目标编码表示（已按 `&#N;` 写入），优先告警。
/// 返回需要展示的提示；None 表示无需提示。
fn transcode_notice(
    lang: editpad_core::Lang,
    original_encoding: &str,
    target_label: &str,
    unmappable: bool,
) -> Option<String> {
    use editpad_core::Key as K;
    if unmappable {
        return Some(K::NoticeEncodeSubstituted.text(lang).to_owned());
    }
    if original_encoding.is_empty() || original_encoding == target_label {
        return None;
    }
    match (original_encoding, target_label) {
        ("UTF-8(BOM)", "UTF-8") => Some(K::NoticeBomLost.text(lang).to_owned()),
        (from, to) => Some(format!(
            "{}{from}{}{to}{}",
            K::NoticeTranscoded.text(lang),
            K::NoticeTranscodedMiddle.text(lang),
            K::NoticeTranscodedSuffix.text(lang)
        )),
    }
}

/// P67：行尾风格的短标签（状态栏/提示用）。
fn eol_label(ending: editpad_core::LineEnding) -> &'static str {
    match ending {
        editpad_core::LineEnding::CrLf => "CRLF",
        editpad_core::LineEnding::Lf => "LF",
        editpad_core::LineEnding::Cr => "CR",
    }
}

// ---------- Markdown 预览面板（P22 第三批） ----------

// ---------- 字体一致性（P33） ----------

// ---------- 多开内存护栏（P21，§3 P19 总则第 2 条） ----------

// ---------- 标签右键菜单（P28） ----------

// ---------- 周期快照心跳（P31） ----------

/// 心跳提交的后台驱动：OS 线程执行 write-ahead 写序（P5/P18 同款桥接，
/// 执行器仅阻塞等待结果）。线程意外终止也必须回报失败——账目作废后
/// 下一拍全量重试，「内容不丢失」不允许静默断链。
async fn drive_heartbeat(payload: HeartbeatPayload) -> Message {
    let (tx, rx) =
        std_mpsc::channel::<Result<editpad_core::snapshot::SessionManifest, String>>();
    std::thread::spawn(move || {
        let result = editpad_core::snapshot::write_heartbeat_session(
            &payload.dir,
            &payload.pages,
            payload.active,
            payload.next_untitled,
        )
        .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    let result = rx.recv().unwrap_or_else(|e| {
        // P155 取舍：心跳线程 panic 到断了 channel 才走到这里；与自动保存
        // 同口径固定取默认语言文案（不值得为不可达分支把 lang 透传下去）
        Err(format!(
            "{}{e}",
            editpad_core::Key::StHeartbeatThreadGone.text(editpad_core::Lang::default())
        ))
    });
    Message::HeartbeatDone(HeartbeatOutcome {
        plan: payload.plan,
        rev: payload.rev,
        result,
    })
}

// ---------- 浮层弹窗几何与样式（P39/P40） ----------

// ---------- 热键系统（P62 可重映射） ----------

/// P55：重命名目标路径推导（纯函数可单测）——同目录下改名；空名/含
/// 路径分隔符/Windows 非法文件名字符 → None（调用方提示并保持输入态）。
/// 首尾空白裁剪（输入框手滑容忍）。
fn rename_target_path(old: &Path, new_name: &str) -> Option<PathBuf> {
    let name = new_name.trim();
    if name.is_empty() {
        return None;
    }
    if name.chars().any(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return None;
    }
    Some(old.with_file_name(name))
}

/// 双击判定时间窗（P65）：与 Windows 系统双击时长一致。同页两次左键
/// 间隔不超过此值即视为双击 → 触发就地重命名。
pub(crate) const TAB_DOUBLE_CLICK_MS: u64 = 500;

/// 双击判定（纯函数可单测，P65）：同一页、且距上次点击不超过
/// [`TAB_DOUBLE_CLICK_MS`]。无记录/换页/超时都不算。
fn is_double_click(
    last: Option<(usize, std::time::Instant)>,
    idx: usize,
    now: std::time::Instant,
) -> bool {
    last.is_some_and(|(i, at)| {
        i == idx && now.duration_since(at).as_millis() as u64 <= TAB_DOUBLE_CLICK_MS
    })
}

// ---------- 第 76 轮：应用 Logo（P71 用户自定义 ICO → 标签栏渲染） ----------

/// P55/P64：标签就地重命名输入框的唯一 id。视图侧 text_input 挂同一
/// id；进入重命名态时聚焦 + 全选（iced::widget::operation 对该 id 的
/// 操作在下一帧视图含输入框后生效），用户可直接键入覆盖预填旧名。
pub(crate) fn rename_input_id() -> iced::widget::Id {
    iced::widget::Id::new("editpad-tab-rename")
}

/// P129：命令面板查询输入框的固定 id——打开时经 operation::focus
/// 自动聚焦（P64 重命名输入框同款手法）。
pub(crate) fn palette_input_id() -> iced::widget::Id {
    iced::widget::Id::new("editpad-palette-input")
}

/// 取外部修改比对戳 (mtime, size)：元数据或 mtime 不可得（文件已被删/
/// 平台不支持）时返回 None（调用方按「无从比对」处理）。
fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

/// 外部修改判定（纯函数可单测）：记录与当前都已知且 (mtime, size) 任一
/// 分量变化即视为外部修改（size 兜底 FAT 系 2s 粒度的 mtime 盲区）；
/// 记录缺失 = 从未记录，不判定；当前缺失 = 文件已被外部删除，同样算
/// 修改（提示用户；此时重载会得到明确的打开失败提示）。
fn file_changed_externally(
    recorded: Option<(std::time::SystemTime, u64)>,
    current: Option<(std::time::SystemTime, u64)>,
) -> bool {
    match (recorded, current) {
        (Some((rm, rs)), Some((cm, cs))) => rm != cm || rs != cs,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

// ---------- 模块声明（P60 自 main.rs 拆分：纯移动零行为变更） ----------
// update.rs = 消息处理/保存/加载/订阅；view.rs = 视图/面板/浮层；
// settings_ui.rs = 设置弹窗类型与中性样式；tests.rs = 应用层测试。
// ---------- Phase 2b（第 82 轮）：自本文件拆出的模块 ----------
mod load;
use load::*;
mod find_scan;
use find_scan::*;
mod highlight_pave;
use highlight_pave::*;
mod md_preview;
use md_preview::*;
mod fonts;
use fonts::*;
mod session;
use session::*;
mod tab;
use tab::*;
mod autosave;
use autosave::*;
mod heartbeat;
use heartbeat::*;
mod state;
use state::*;
mod chrome;
use chrome::*;
// P153：查找卡片的「点击观察层」（内容消费了按下事件也要能观察到）。
mod press_observer;
pub(crate) use press_observer::PressObserver;
mod hotkeys;
use hotkeys::*;
mod icon;
use icon::*;

mod settings_ui;
mod update;
mod view;
mod single_instance;
#[cfg(test)]
mod tests;

use settings_ui::SettingsPage;