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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};

use iced::futures::SinkExt;
use iced::keyboard::{self, key::Named};
use iced::widget::{button, checkbox, column, container, mouse_area, opaque, progress_bar, row, rule,
    scrollable, text, text_input, Stack};
use iced::{border::Radius, stream, window, Alignment, Background, Border, Color, Element, Fill,
    Font, Padding, Point, Shadow, Subscription, Task, Theme};

use editor::{BlankKind, CaseKind, EditorHandle, EditOp, Motion, SortOrder, TabSpaceKind, TrimMode};

fn main() -> iced::Result {
    // iced 0.14：第一个参数是 boot 函数（返回初始状态），title/theme/subscription 走 builder
    // 第 76 轮（用户点单）：窗口标题栏图标 = 应用自定义 Logo（P71 四档
    // ICO 的 48px 条目；解析失败回退 exe 资源图标，见 window_title_icon）
    iced::application(Editpad::new, Editpad::update, Editpad::view)
        .title(Editpad::title)
        .theme(Editpad::theme)
        .subscription(Editpad::subscription)
        .window(window::Settings {
            icon: window_title_icon(),
            ..window::Settings::default()
        })
        // 关闭请求必须以事件流转到 subscription（exit_on_close_request 默认 true，
        // 不显式关掉的话点 X 会直接退进程，永远轮不到未保存确认）
        .exit_on_close_request(false)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    /// 编辑器按键编辑（字符、删除、移动……）
    Edit(EditOp),
    /// 控件层通知：光标/选区变了（刷新状态栏）
    EditorNavChanged,

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
    /// 保存完成：(落盘内容的内容版本号, 结果)（P18 版本守卫；P67 起成功
    /// 分支携带编码落盘的附带信息，如 GBK 不可映射字符告警）
    Saved(u64, Result<editpad_core::EncodeNotice, String>),
    /// 标签页保存完成（「保存并关闭」流程用）：(页, 快照版本, 结果)
    TabSaved(usize, u64, Result<(), String>),
    /// 自动保存完成（P63 载荷扩展）：(标签页, 快照版本, 调度时页路径,
    /// 结局)。版本不符 = 期间又有编辑，不清脏；路径不符 = 页集合在防抖
    /// 睡眠期间变动导致下标漂移，整条丢弃；结局三分见 [`AutosaveOutcome`]。
    TabAutosaved(usize, u64, PathBuf, AutosaveOutcome),
    /// P67：状态栏「编码」标签点开的弹出菜单开关
    ToggleEncodingMenu,
    /// P67：状态栏「行尾」标签点开的弹出菜单开关
    ToggleEolMenu,
    /// P67：以指定编码保存当前页（记住偏好并立即走保存管线）
    SaveWithEncoding(editpad_core::SaveEncoding),
    /// P67：把当前页行尾统一转换为目标风格（可撤销的文档编辑）
    ConvertEol(editpad_core::LineEnding),

    FindToggled,
    FindQueryChanged(String),
    FindNext,
    FindPrev,
    CaseToggled(bool),
    ReplaceQueryChanged(String),
    ReplaceCurrent,
    /// P70：正则模式的「替换当前」（对当前命中做 $1 展开替换）
    ReplaceCurrentRegex,
    ReplaceAll,
    /// 后台查找扫描完成：(任务序号, 命中表)。序号过期的结果直接丢弃（P10）
    FindScanDone(u64, Vec<editpad_core::MatchPos>),
    /// P70：正则模式开关（开启/关闭都会触发重扫）
    RegexToggled(bool),
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
    /// 设置：保存时备份模式循环切换（第 64 轮 ⑭，none→simple→
    /// timestamped→none，仿关窗行为的三态按钮）
    SettingsBackupModeToggled,
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
    /// 窗口逻辑尺寸变化（浮层贴边钳制的依据；启动时 winit 也会发一次）
    ViewportResized(f32, f32),
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
    /// 则整体丢弃，缺口由下一帧重新评估续排（P12）
    HlPaved(u64, editpad_core::LazyHighlighter),

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
    /// 打开确认条「放弃更改并打开」：丢弃未保存修改并加载暂存路径
    ConfirmOpenDiscard,
    /// 打开确认条「取消」：留在当前文档
    ConfirmOpenCancel,

    /// 深浅主题切换（写回设置）
    ThemeToggled,
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

/// 后台加载线程 → 订阅流的事件。
enum LoadEvent {
    Progress(editpad_core::LoadProgress),
    Done(Result<editpad_core::LoadedDocument, String>),
}

/// 后台加载任务：订阅标识 + 目标路径 + 目标标签页（P21 路由归属）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LoadJob {
    id: u64,
    path: PathBuf,
    /// 结果应写入的标签下标（期间切走标签不影响归页）
    tab: usize,
}

/// 把一次加载任务构造成事件流（OS 线程做阻塞 IO，std mpsc 桥接到异步端）。
fn build_load_stream(job: &LoadJob) -> impl iced::futures::Stream<Item = Message> {
    let job = job.clone();
    stream::channel(
        64,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            drive_load(
                job.id,
                job.path,
                |path, on_progress| editpad_core::load_document_streaming(path, on_progress),
                &mut output,
            )
            .await;
        },
    )
}

/// 加载任务的事件驱动（P5 重构：loader 可注入以便测试）。
///
/// 保证语义：无论加载函数成功、失败还是 **panic**，UI 都必然收到恰好一条
/// `Message::Loaded`——否则 `busy` 会永久卡死，除主题/字号外全部按钮禁用。
/// * 第一道兜底：线程体包 `catch_unwind`，崩溃也发送 `Done(Err(..))`；
/// * 第二道兜底：接收端通道关闭仍未收到 Done（线程被强杀等极端情形），
///   补发一条失败消息。
/// 加载任务的事件驱动（P5 重构：loader 可注入以便测试）。
///
/// 保证语义：无论加载函数成功、失败还是 **panic**，UI 都必然收到恰好一条
/// `Message::Loaded`——否则 `busy` 会永久卡死，除主题/字号外全部按钮禁用。
/// * 第一道兜底：线程体包 `catch_unwind`，崩溃也发送 `Done(Err(..))`；
/// * 第二道兜底：接收端通道关闭仍未收到 Done（线程被强杀等极端情形），
///   补发一条失败消息。
async fn drive_load<L>(
    job_id: u64,
    path: PathBuf,
    loader: L,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
) where
    L: FnOnce(
        &Path,
        &mut dyn FnMut(editpad_core::LoadProgress),
    ) -> Result<editpad_core::LoadedDocument, editpad_core::CoreError>
        + Send
        + 'static,
{
    let (notify_tx, notify_rx) = std_mpsc::channel::<LoadEvent>();
    std::thread::spawn(move || {
        // AssertUnwindSafe：跨 unwind 捕获的闭包引用（tx/path/loader）只需保证
        // panic 后不再使用其内部可变性，这里仅透传结果，安全。
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loader(&path, &mut |progress| {
                let _ = notify_tx.send(LoadEvent::Progress(progress));
            })
        }));
        let done = match outcome {
            Ok(result) => LoadEvent::Done(result.map_err(|e| e.to_string())),
            Err(payload) => {
                // 必须显式 &*payload 解出内部值：直接传 &payload 时，
                // 因为 Box<dyn Any + Send> 自身也实现了 Any，编译器会走
                // unsize 强制转换（把整个 Box 当作 Any 对象）而不是解引用，
                // downcast 的 TypeId 就对不上了（实测踩坑）。
                let message = panic_message(&*payload);
                LoadEvent::Done(Err(format!("加载线程崩溃: {message}")))
            }
        };
        let _ = notify_tx.send(done);
        // notify_tx 在此 drop：接收端循环随之结束
    });

    let mut done_received = false;
    while let Ok(event) = notify_rx.recv() {
        if matches!(event, LoadEvent::Done(_)) {
            done_received = true;
        }
        let message = match event {
            LoadEvent::Progress(p) => Message::LoadProgress(job_id, p.bytes_read, p.total_bytes),
            LoadEvent::Done(result) => Message::Loaded(
                job_id,
                result.map(|loaded| {
                    (loaded.doc, loaded.sample, loaded.encoding.to_string())
                }),
            ),
        };
        if output.send(message).await.is_err() {
            break;
        }
    }
    if !done_received {
        let _ = output
            .send(Message::Loaded(
                job_id,
                Err("加载流意外终止（未收到完成事件）".to_owned()),
            ))
            .await;
    }
}

/// 把 panic 载荷转成可读描述（P5 兜底提示用）。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知原因".to_owned()
    }
}

// ---------- 后台查找扫描（P10） ----------

/// 查找防抖窗口：窗口内的新输入会作废旧任务（置位其取消标志）并另排新任务。
const FIND_DEBOUNCE_MS: u64 = 200;

/// 一次后台查找扫描的输入快照。`doc` 是 rope 的结构共享克隆（O(1)，
/// 不拷贝正文）；`cancelled` 指向应用状态里的当前代取消标志——
/// 新输入会把上一代的标志置位，睡醒后的旧任务检查到即放弃扫描。
#[derive(Clone)]
struct FindScanPayload {
    seq: u64,
    doc: editpad_core::Document,
    query: String,
    case_sensitive: bool,
    /// P70：正则模式（query 为原始正则；扫描走全文 to_text + fancy-regex）
    regex: bool,
    /// 本代任务的取消标志（新任务排队时把上一代置位）
    cancelled: Arc<AtomicBool>,
    /// 防抖窗口毫秒数（生产走 [`FIND_DEBOUNCE_MS`]；测试注入小值）
    debounce_ms: u64,
}

/// 查找任务的事件驱动（扫描函数与防抖时长均可注入以便测试，同 [`drive_load`] 做法）。
///
/// 保证语义：无论扫描成功、被取消还是 **panic**，都恰好回一条 `FindScanDone`
/// ——否则查找栏会永久停在「查找中…」。过期结果由 update 按 seq 二次过滤。
async fn drive_find_scan<F>(
    payload: FindScanPayload,
    scan: F,
) -> Message
where
    F: FnOnce(&editpad_core::Document, &str, bool, bool) -> Vec<editpad_core::MatchPos>
        + Send
        + 'static,
{
    let (notify_tx, notify_rx) = std_mpsc::channel::<Vec<editpad_core::MatchPos>>();
    std::thread::spawn(move || {
        // 防抖：真正的取消由 cancelled 标志完成——新输入排队时置位上一代，
        // 这里睡满窗口后检查，被作废的任务直接退出、不浪费一次全文扫描
        std::thread::sleep(std::time::Duration::from_millis(payload.debounce_ms));
        let matches = if payload.cancelled.load(Ordering::Relaxed) {
            Vec::new()
        } else {
            // P5 同款兜底：扫描崩溃也要回消息（空表），不能让 UI 永久等待
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scan(&payload.doc, &payload.query, payload.case_sensitive, payload.regex)
            }))
            .unwrap_or_default()
        };
        let _ = notify_tx.send(matches);
    });

    // 阻塞 recv 与 drive_load 的取舍相同：OS 线程结果桥接到异步端，
    // iced 线程池可承受短暂阻塞
    let matches = notify_rx.recv().unwrap_or_default();
    Message::FindScanDone(payload.seq, matches)
}

/// 与 core::search 一致的大小写语义：实现已下沉到 core（P15 去重），
/// 这里只保留「区分大小写走整串比较」的快速路径。
fn strings_equal(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.chars().count() == b.chars().count()
            && a.chars()
                .zip(b.chars())
                .all(|(x, y)| editpad_core::ascii_case_eq(x, y, false))
    }
}

/// 查找/替换输入的转义解析（P22 补充能力）：
/// `\n` `\r` `\t` `\\` 分别解析为换行、回车、制表符、反斜杠；
/// 其他未知转义保持原样（`\q` 仍是 `\q`），不做半截猜测。
///
/// 这让用户可以搜索/替换换行与制表符等不可见字符——
/// 也是未来多行查询的输入入口。
fn unescape_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    let mut chars = q.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            // 未知转义或孤立反斜杠：按字面保留
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

// ---------- 高亮后台分批补建（P12） ----------

/// 每个后台批次铺建的检查点档位数。一档 = STRIDE 行 ≈ 10ms 量级，
/// 32 档约 0.3s——进度粒度足够顺滑，取消响应延迟有界。
const HL_PAVE_BATCH_STRIDES: usize = 32;

/// 一次后台高亮铺建任务的输入快照。`doc` 与 `highlighter` 都是调度
/// 时刻的克隆（rope 结构共享 O(1) / 检查点向量拷贝）；worker 只读它们、
/// 推进自己的高亮器副本，完成时整体送回，不做任何共享可变状态。
struct HlPavePayload {
    gen: u64,
    doc: editpad_core::Document,
    highlighter: editpad_core::LazyHighlighter,
    total_lines: usize,
    /// 取消标志：编辑换代/新任务排队时置位，worker 批间检查后提前收工。
    /// 收工时的部分成果仍然有效（完整档位只依赖快照内的一致前缀），
    /// 会照常送回安装。
    cancelled: Arc<AtomicBool>,
    batch_strides: usize,
}

/// worker 的推进循环：反复 [`advance_checkpoints`](editpad_core::LazyHighlighter::advance_checkpoints)
/// 直到铺满/取消/无活可干；每批后上报累计档位数。
/// 独立成函数以便测试注入 panic 路径（同 drive_load 的 loader 注入）。
fn pave_run(payload: &HlPavePayload, report: &mut dyn FnMut(u64)) -> editpad_core::LazyHighlighter {
    let mut hl = payload.highlighter.clone();
    let mut built_total = 0u64;
    loop {
        if payload.cancelled.load(Ordering::Relaxed) {
            break;
        }
        let built = hl.advance_checkpoints(
            payload.batch_strides,
            payload.total_lines,
            &mut |i| payload.doc.line_str(i),
        );
        if built == 0 {
            break;
        }
        built_total += built as u64;
        report(built_total);
    }
    hl
}

/// 铺建任务的事件流：OS 线程分批推进，std mpsc 桥接到异步端（P5/P10 同款）。
///
/// 保证语义：无论推进成功、被取消还是 **panic**，都恰好回一条 `HlPaved`
/// ——否则「语法分析中…」状态永不解除。panic 兜底回未推进的起点克隆，
/// 安装它等于无变化，UI 不受损。
fn build_hl_pave_stream(payload: HlPavePayload) -> impl iced::futures::Stream<Item = Message> {
    stream::channel(
        8,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            drive_hl_pave(payload, pave_run, &mut output).await;
        },
    )
}

/// 铺建任务的事件驱动（推进函数可注入以便测试）。
async fn drive_hl_pave<F>(
    payload: HlPavePayload,
    pave: F,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
) where
    F: FnOnce(&HlPavePayload, &mut dyn FnMut(u64)) -> editpad_core::LazyHighlighter
        + Send
        + 'static,
{
    enum HlEvent {
        Progress(u64),
        Done(editpad_core::LazyHighlighter),
    }
    let (notify_tx, notify_rx) = std_mpsc::channel::<HlEvent>();
    let gen = payload.gen;
    std::thread::spawn(move || {
        // AssertUnwindSafe：panic 后仅透传兜底克隆，不再触碰线程局部可变性
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pave(&payload, &mut |done| {
                    let _ = notify_tx.send(HlEvent::Progress(done));
                })
            }));
        let done = match outcome {
            Ok(hl) => HlEvent::Done(hl),
            Err(..) => HlEvent::Done(payload.highlighter.clone()),
        };
        let _ = notify_tx.send(done);
        // notify_tx 在此 drop：接收端循环随之结束
    });

    while let Ok(event) = notify_rx.recv() {
        let message = match event {
            HlEvent::Progress(done) => Message::HlPaveProgress(gen, done),
            HlEvent::Done(hl) => Message::HlPaved(gen, hl),
        };
        if output.send(message).await.is_err() {
            break;
        }
    }
}

/// P6 编码知情权（P67 口径升级）：保存前后编码不一致 = 发生不可逆转换，
/// 必须告知；用户在状态栏主动选择的目标编码不再视为「意外转码」。
/// `unmappable` = 有字符无法用目标编码表示（已按 `&#N;` 写入），优先告警。
/// 返回需要展示的提示；None 表示无需提示。
fn transcode_notice(
    original_encoding: &str,
    target_label: &str,
    unmappable: bool,
) -> Option<String> {
    if unmappable {
        return Some(
            "部分字符无法用目标编码表示，已按 &#编号; 形式写入".to_owned(),
        );
    }
    if original_encoding.is_empty() || original_encoding == target_label {
        return None;
    }
    match (original_encoding, target_label) {
        ("UTF-8(BOM)", "UTF-8") => {
            Some("已按 UTF-8（无 BOM）保存：原文件的 BOM 已丢失".to_owned())
        }
        (from, to) => Some(format!("已从 {from} 转码为 {to} 落盘（转码不可逆）")),
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

/// 把解析出的块级元素排成只读预览列（滚动容器包裹）。
///
/// `font_size` = 当前正文字号：预览排版随 Ctrl+滚轮缩放（P36 口径：
/// 预览属文件内容渲染故跟随；工具栏等 UI 控件不跟随），各级基准值与
/// 旧硬编码一致（正文默认 16px 时逐项像素相等）。
/// `base` = 正文字形族（P34：预览属内容渲染，随设置切换）。
fn markdown_preview_element(
    source: &str,
    font_size: f32,
    base: Font,
) -> Element<'static, Message> {
    use iced::font::Weight;

    // 预览各级字号相对正文默认 16px 的既有比例
    let scaled = |px: f32| px * (font_size / 16.0);

    let blocks = editpad_core::markdown::parse_markdown(source);
    let mut col = column![].spacing(10).padding(14);
    for block in blocks {
        match block {
            editpad_core::markdown::MdBlock::Heading { level, spans } => {
                let px = match level {
                    1 => scaled(26.0),
                    2 => scaled(23.0),
                    3 => scaled(20.0),
                    4 => scaled(18.0),
                    5 => scaled(17.0),
                    _ => scaled(16.0),
                };
                col = col.push(md_spans_row(spans, px, true, base));
            }
            editpad_core::markdown::MdBlock::Paragraph { spans } => {
                col = col.push(md_spans_row(spans, scaled(15.0), false, base));
            }
            editpad_core::markdown::MdBlock::ListItem { spans } => {
                let mut line =
                    row![text("• ").size(scaled(15.0)).font(base)];
                for span in spans {
                    let font = md_font(base, span.is_bold(), span.is_italic());
                    let mut t =
                        text(span.text.clone()).size(scaled(15.0)).font(font);
                    if span.is_code() {
                        t = t.color([0.12, 0.36, 0.6]);
                    }
                    line = line.push(t);
                }
                col = col.push(line);
            }
            editpad_core::markdown::MdBlock::Quote { spans } => {
                let mut line = row![
                    text("▌ ")
                        .size(scaled(15.0))
                        .font(base)
                        .color([0.55, 0.55, 0.6])
                ];
                for span in spans {
                    let font = md_font(base, span.is_bold(), span.is_italic());
                    let t = text(span.text.clone())
                        .size(scaled(15.0))
                        .font(font)
                        .color([0.45, 0.45, 0.5]);
                    line = line.push(t);
                }
                col = col.push(line);
            }
            editpad_core::markdown::MdBlock::CodeBlock { lines } => {
                for line in lines {
                    col = col.push(
                        text(format!("▏ {line}"))
                            .size(scaled(14.0))
                            .font(Font {
                                weight: Weight::Normal,
                                ..base
                            })
                            .color([0.25, 0.35, 0.45]),
                    );
                }
            }
            editpad_core::markdown::MdBlock::Rule => {
                col = col.push(rule::horizontal(2));
            }
        }
    }
    scrollable(container(col).width(Fill)).height(Fill).into()
}

/// 按行内样式构造文本片段行。
fn md_spans_row(
    spans: Vec<editpad_core::markdown::MdSpan>,
    px: f32,
    bold: bool,
    base: Font,
) -> iced::widget::Row<'static, Message> {
    use iced::font::{Style as FontStyle, Weight};

    let mut row = row![].spacing(0);
    if spans.is_empty() {
        row = row.push(text(""));
        return row.into();
    }
    for span in spans {
        let weight = if bold || span.is_bold() {
            Weight::Bold
        } else {
            Weight::Normal
        };
        let style = if span.is_italic() {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        let font = Font {
            weight,
            style,
            ..base
        };
        let mut t = text(span.text.clone()).size(px).font(font);
        if span.is_code() {
            t = t.color([0.12, 0.36, 0.6]);
        }
        row = row.push(t);
    }
    row
}

fn md_font(base: Font, bold: bool, italic: bool) -> Font {
    Font {
        weight: if bold {
            iced::font::Weight::Bold
        } else {
            iced::font::Weight::Normal
        },
        style: if italic {
            iced::font::Style::Italic
        } else {
            iced::font::Style::Normal
        },
        ..base
    }
}

// ---------- 字体一致性（P33） ----------

/// 启动期把 `Family::Monospace` 的解析目标钉到系统里第一个可用的 CJK
/// 等宽字体（方案 c，候选与优先级见 [`editor::CJK_MONO_CANDIDATES`]）。
///
/// 根因回顾：正文主字体 MONOSPACE 在 Windows 上不含 CJK 字形，cosmic-text
/// 对缺字形的文本按内建回退表（font/fallback/windows.rs：Han 按 locale 分流
/// ja→Yu Gothic / zh→Microsoft YaHei UI…）逐 run 兜底——共享码位汉字
/// （如「我」）与简化专有字（如「现/试」）落入不同回退字体，正是用户截图里
/// 「逐字字形不一」的主缺陷。主字体一旦覆盖 CJK，逐字回退不再参与。
///
/// 实现要点：
/// * 入口 = `iced::advanced::graphics::text::font_system()`（iced 0.14 公开
///   全局，wgpu / tiny-skia 两后端共用）；`raw().db_mut()` 直达 fontdb；
/// * 只改解析目标、不装载任何字体字节——零体积、零内存增量（预算总则入账）；
/// * 进程内一次（AtomicBool 幂等），且发生在首帧排版之前，无缓存失效问题；
/// * 无候选命中（非 CJK 环境/极简系统）静默保持现状，零行为变化。
fn apply_default_cjk_mono_pin() {
    static APPLIED: AtomicBool = AtomicBool::new(false);
    if APPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
        return;
    };
    let families: Vec<String> = font_system
        .raw()
        .db_mut()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    if let Some(family) = editor::pick_cjk_mono_family(&families) {
        font_system.raw().db_mut().set_monospace_family(family);
    }
}

// ---------- 字体选择（P34） ----------

/// 设置弹窗字体列表单帧最多渲染的行数；超出提示继续过滤。
/// Windows 全量族名可达数百条——无上限的 widget 树会让弹窗每帧变重，
/// v1 用「过滤词 + 上限」控制规模（滚动列表本身有高度限制）。
const FONT_PICKER_MAX_ROWS: usize = 200;

/// 枚举系统已装字体的全部族名（精确去重、不区分大小写排序）。
///
/// 数据源与 P33 钉字同路：iced 全局 font_system 的 fontdb——Windows 上
/// 已自动装载 `C:\Windows\Fonts` 等系统目录，无需手写目录扫描（§3 P34
/// 的「手写扫描」路线就此作废，连文件名→族名的换算都省了）。
/// 本地化别名（如「新宋体」与 NSimSun）都会出现在清单里，均为 fontdb
/// 认可的合法名字。失败（锁中毒等）返回空表：弹窗降级为提示文案。
/// 注：包装器的 `raw()` 签名是 `&mut self`，读清单也只能拿写锁（P33 同款，
/// 启动期一次性的短暂临界区）。
fn enumerate_available_families() -> Vec<String> {
    let Ok(mut font_system) = iced::advanced::graphics::text::font_system().write() else {
        return Vec::new();
    };
    let mut names: Vec<String> = font_system
        .raw()
        .db()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// 配置字体名 → 实际生效的系统族名（纯函数便于测试）。
///
/// * 配置为空 → None（用默认等宽）；
/// * 精确命中清单 → Some(该条目)；
/// * 宽松命中（去空白 + 小写相等，复用 [`editor::normalize_family`]）→
///   Some(规范条目)——手改 config.toml 的大小写/空白变体被自动矫正；
/// * 未命中 → None：**只回退本次渲染，不抹掉配置**（用户重装字体后
///   自动恢复；启动时给一次状态栏提示）。
fn effective_font_family(configured: Option<&str>, available: &[String]) -> Option<String> {
    let name = configured?;
    if available.iter().any(|f| f == name) {
        return Some(name.to_owned());
    }
    let want = editor::normalize_family(name);
    available
        .iter()
        .find(|f| editor::normalize_family(f) == want)
        .cloned()
}

/// 启动期解析（纯函数便于测试）：返回 (生效族名, 是否提示「未安装」)。
fn resolve_startup_font(
    configured: Option<&str>,
    available: &[String],
) -> (Option<String>, bool) {
    let effective = effective_font_family(configured, available);
    let missing = configured.is_some() && effective.is_none();
    (effective, missing)
}

/// 把族名 `'static` 化：iced 0.14 的 `Family::Name(&'static str)` 不收
/// String，运行期选定的族名只能泄漏进进程生存期。
/// 内存入账（预算总则第 1 条）：每次**切换到不同字体**泄漏一个族名
/// 字符串（几十字节）；字体选择是设置级低频操作，进程内总量可忽略。
/// 启动期至多一次、选择消息每次至多一次，无循环放大路径。
fn leak_font_family(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

// ---------- 多开内存护栏（P21，§3 P19 总则第 2 条） ----------

/// 多开内存护栏上限（字节）：全部页内容 + 待载文件的保守估算。
/// 约 2.56 亿字符 ≈ 数个 50MB 级大文档同时驻留的量级。
const MULTI_TAB_MEM_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// 内存护栏判定（纯函数便于测试）：现有页字符按 3 字节/字符保守
/// 估算 UTF-8 上界，加上待载文件实际字节数后与上限比较。
fn mem_guard_allows(existing_chars: usize, incoming_bytes: u64, cap_bytes: u64) -> bool {
    let estimate = (existing_chars as u64).saturating_mul(3);
    estimate.saturating_add(incoming_bytes) <= cap_bytes
}

// ---------- 启动会话恢复（P30） ----------

/// 启动会话恢复的总开关判定（纯函数便于测试）：
/// 快照底座与「记住会话」两个开关都开启才允许恢复/写清单。
fn session_restore_allowed(enable_snapshots: bool, remember_session: bool) -> bool {
    enable_snapshots && remember_session
}

// ---------- 标签右键菜单（P28） ----------

/// 批量关闭的范围（纯函数 [`batch_close_targets`] 的入参）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchCloseScope {
    /// 除 `keep` 页以外的全部候选
    Others(usize),
    /// `from` 页右侧的全部候选
    RightOf(usize),
}

/// 批量关闭的目标集合（纯函数便于测试，§3 P28 第 3 条）：
/// 在指定范围内收集全部**非固定**页下标（升序）。固定页豁免批量关闭；
/// keep/from 越界时返回空表（无目标 = 菜单项禁用、update 层 no-op）。
fn batch_close_targets(tabs: &[Tab], scope: BatchCloseScope) -> Vec<usize> {
    let len = tabs.len();
    match scope {
        BatchCloseScope::Others(keep) if keep < len => (0..len)
            .filter(|&i| i != keep && !tabs[i].pinned)
            .collect(),
        BatchCloseScope::RightOf(from) if from < len => ((from + 1)..len)
            .filter(|&i| !tabs[i].pinned)
            .collect(),
        _ => Vec::new(),
    }
}

/// 恢复链中一个待载入的命名干净页：路径、目标占位页下标与待还原视图。
#[derive(Debug, Clone)]
struct RestoreLoad {
    path: PathBuf,
    tab: usize,
    cursor_line: usize,
    cursor_col: usize,
    scroll_top: f32,
    scroll_left: f32,
}

/// 恢复截断规划（纯函数便于测试）：估算各页字节量——置脏页按其快照
/// 文件、干净命名页按磁盘文件、干净未命名页为 0——先保激活页再按下标
/// 顺序装填，累计超上限的页放弃恢复（§3 P30 第 5 条）。
/// 返回 (保留下标升序, 被截断页数)。
fn plan_restore_order(
    manifest: &editpad_core::snapshot::SessionManifest,
    snapshot_dir: &Path,
    cap_bytes: u64,
) -> (Vec<usize>, usize) {
    fn page_estimate(
        tab: &editpad_core::snapshot::SessionTab,
        snapshot_dir: &Path,
    ) -> u64 {
        if let Some(file) = &tab.file {
            fs::metadata(snapshot_dir.join(file))
                .map(|m| m.len())
                .unwrap_or(0)
        } else if let Some(path) = &tab.path {
            fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        } else {
            0 // 干净未命名页 = 空文档
        }
    }

    let count = manifest.tabs.len();
    let active = manifest.active.min(count.saturating_sub(1));
    // 装填顺序：激活页优先（「先保激活页」），其余按下标升序
    let mut order = vec![active];
    order.extend((0..count).filter(|&i| i != active));

    let mut kept = Vec::new();
    let mut total = 0u64;
    for i in order {
        let size = page_estimate(&manifest.tabs[i], snapshot_dir);
        if total.saturating_add(size) > cap_bytes {
            continue; // 截断：该页放弃恢复，不阻断其余页
        }
        total += size;
        kept.push(i);
    }
    kept.sort_unstable();
    let dropped = count - kept.len();
    (kept, dropped)
}

/// 取文档头部样本（≤4096 字符）供语言嗅探：迭代 rope 存储块拼接，
/// 不产生全文拷贝（P19 内存口径）。
fn head_sample(doc: &editpad_core::Document) -> String {
    const LIMIT: usize = 4096;
    let mut sample = String::new();
    'outer: for chunk in doc.chunks() {
        for ch in chunk.chars() {
            sample.push(ch);
            if sample.chars().count() >= LIMIT {
                break 'outer;
            }
        }
    }
    sample
}

/// 一次自动保存的结局（P63）：写盘成功 / 撞上外部修改被拒写 / 写盘失败。
/// 拒写不是失败——磁盘上发生了别人（其他编辑器/同步工具）的改动，
/// 盲写会覆盖它；裁决权交给 P52 外部修改提示条。
#[derive(Debug, Clone)]
enum AutosaveOutcome {
    Written,
    SkippedExternalChange,
    Failed(String),
}

/// 一次自动保存的驱动：专属 OS 线程「睡满防抖窗 → 写前校验 → 分块原子
/// 落盘」，结果经 std mpsc 桥接回异步端（P5/P10 同款；执行器仅阻塞等待
/// 结果，且按页 inflight 去重保证同一页至多一个这样的线程）。
///
/// P63 写前校验：防抖睡眠期间磁盘可能被外部修改（焦点巡检只在窗口
/// 重聚焦时跑，救不了后台线程）。调度时刻的 `(mtime, size)` 戳随任务
/// 下发，醒来先比对，不一致即拒写并回报 [`AutosaveOutcome::
/// SkippedExternalChange`]——原文件绝不盲写覆盖外部内容。期望戳为
/// None（从未记录，如测试注入的不存在路径）时保持旧语义直接写。
async fn drive_autosave_once(
    tab: usize,
    path: PathBuf,
    doc: editpad_core::Document,
    version: u64,
    expected_stamp: Option<(std::time::SystemTime, u64)>,
    delay: std::time::Duration,
    backup_mode: String,
) -> Message {
    let (tx, rx) = std_mpsc::channel::<AutosaveOutcome>();
    let thread_path = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let outcome = if autosave_must_skip(expected_stamp, file_stamp(&path)) {
            AutosaveOutcome::SkippedExternalChange
        } else {
            // 第 64 轮 ⑭：写前备份磁盘旧版（自动保存静默口径——备份
            // 提示不打扰，失败同样降级不阻断）
            let _ = perform_backup_before_overwrite(&path, &backup_mode);
            match editpad_core::save_document_atomic(&path, &doc) {
                Ok(()) => AutosaveOutcome::Written,
                Err(e) => AutosaveOutcome::Failed(e.to_string()),
            }
        };
        let _ = tx.send(outcome);
    });
    let outcome = rx
        .recv()
        .unwrap_or_else(|_| AutosaveOutcome::Failed("自动保存线程意外终止".to_owned()));
    // 路径本体已随闭包移入写盘线程；回报携带同内容的克隆
    Message::TabAutosaved(tab, version, thread_path, outcome)
}

/// 自动保存写前判定（纯函数可单测，P63）：期望戳已知（Some）且与当前
/// 磁盘戳不一致 = 有外部修改（含文件被删），必须拒写。期望戳 None =
/// 从未记录（无从比对），不拦截——与 [`file_changed_externally`] 的
/// 「记录缺失不判定」口径一致，但这里反过来以期望戳为主语。
fn autosave_must_skip(
    expected: Option<(std::time::SystemTime, u64)>,
    current: Option<(std::time::SystemTime, u64)>,
) -> bool {
    expected.is_some() && current != expected
}

// ---------- 第 64 轮 ⑭：保存时备份磁盘旧版 ----------

/// 大文件豁免阈值：源文件超过此字节数跳过备份（复制耗时会拖慢保存，
/// 且 64MB+ 的日志类文件通常有专门的轮转手段）。取值对齐性能基准
/// bench-50mb.log 量级再留余量。
pub(crate) const MAX_BACKUP_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// 写前备份磁盘旧版（⑭）。返回状态栏提示文本；None = 无事发生。
///
/// 口径：
/// * 目标文件不存在（新建/另存到新路径）→ 无旧版可备份，None；
/// * 源超过 [`MAX_BACKUP_SOURCE_BYTES`] → 跳过并提示；
/// * simple → 同目录 `name.bak` 覆盖式；
/// * timestamped → 同目录 `name.bak.d/` **目录**内
///   `name.YYYYMMDD-HHMMSS.bak` 历史留存（`.bak.d` 与 simple 的
///   `name.bak` 文件不同名，两模式可自由切换互不污染）；
/// * 任何 IO 失败都**不阻断保存**——降级为状态栏提示（备份是锦上添
///   花，不能成为丢保存的理由）。
pub(crate) fn perform_backup_before_overwrite(path: &Path, mode: &str) -> Option<String> {
    use editpad_core::settings::{BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED};
    if mode == editpad_core::settings::BACKUP_MODE_NONE {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    if meta.len() > MAX_BACKUP_SOURCE_BYTES {
        return Some("文件超过 64MB，按策略跳过备份".to_owned());
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let report = |r: std::io::Result<PathBuf>| match r {
        Ok(p) => Some(format!("已备份旧版 → {}", p.display())),
        Err(e) => Some(format!("备份失败（继续保存）：{e}")),
    };
    match mode {
        BACKUP_MODE_SIMPLE => {
            let bak = path.with_file_name(format!("{name}.bak"));
            report(std::fs::copy(path, &bak).map(|_| bak))
        }
        BACKUP_MODE_TIMESTAMPED => {
            let dir = path.with_file_name(format!("{name}.bak.d"));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Some(format!("备份失败（继续保存）：{e}"));
            }
            let stamp = editor::local_datetime_stamp_compact();
            let target = dir.join(format!("{name}.{stamp}.bak"));
            report(std::fs::copy(path, &target).map(|_| target))
        }
        _ => None,
    }
}

// ---------- 周期快照心跳（P31） ----------

/// 一次心跳提交的输入：注入目录、全部页的逐页意图、派发时刻捕获的逐页计划。
struct HeartbeatPayload {
    dir: PathBuf,
    pages: Vec<editpad_core::snapshot::HeartbeatPage>,
    active: usize,
    next_untitled: u64,
    /// 需要重写内容的 (页下标, 派发时刻内容版本)——回报据此回填账目
    plan: Vec<(usize, u64)>,
}

/// 一次心跳提交的回报：派发时刻的计划 + 写盘结果（成功时含新清单，
/// 用于取各页实际文件名）。Clone 仅为测试同步驱动入口服务。
#[derive(Debug, Clone)]
struct HeartbeatOutcome {
    plan: Vec<(usize, u64)>,
    result: Result<editpad_core::snapshot::SessionManifest, String>,
}

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
    let result = rx
        .recv()
        .unwrap_or_else(|e| Err(format!("心跳线程意外终止:{e}")));
    Message::HeartbeatDone(HeartbeatOutcome {
        plan: payload.plan,
        result,
    })
}

/// 单个标签页的完整状态（P21）。
///
/// [`EditorHandle`] 内聚文档/光标/选区/撤销/高亮/滚动；标签页另持
/// 路径、置脏标记与编码标签。P18 版本守卫所需的版本号/防抖起点/
/// 自动保存在途标记也**按页独立**——后台页同样参与自动保存。
#[derive(Debug, Clone)]
struct Tab {
    editor: EditorHandle,
    path: Option<PathBuf>,
    dirty: bool,
    encoding_label: String,
    /// 内容版本号：本页每次真实改动 +1（保存回报据此判断是否清脏）
    version: u64,
    /// 本页自动保存任务在途标记
    autosave_inflight: bool,
    /// 本页最后一次内容改动的时刻（防抖窗口计时起点）
    last_edit_at: Option<std::time::Instant>,
    /// 未命名页的递增序号（P25）：显示为「未命名N」，
    /// 全局单调不复用——杜绝两个同名未命名页的保存歧义；
    /// 另存为成功或加载真实文件后清除。
    untitled_num: Option<u64>,
    /// P31 心跳账目：本页最后一次被心跳快照收录时的 (内容版本, 页文件名)。
    /// None = 从未参与。版本与文件名成对维护，保证「版本没变 → 旧文件
    /// 仍有效 → 沿用不重写」的复用判定不会错位；账目随页走（增删页/
    /// 调序后仍与正确的内容文件配对）。
    heartbeat_snap: Option<(u64, String)>,
    /// 固定标记（P28）：固定页豁免单页与批量关闭（菜单项灰掉），
    /// 标签条以 📌 标识。v1 取舍：不自动前置排序（保持用户手动排列的
    /// 稳定顺序）；不入会话快照清单（会话内临时状态）。
    pinned: bool,
    /// P50 外部修改检测戳：载入/保存成功时刻的 (mtime, size)。
    /// None = 从未记录（未命名页/会话恢复占位页未落地的），不参与判定。
    file_stamp: Option<(std::time::SystemTime, u64)>,
    /// P67：本页的保存编码偏好。None = 默认 UTF-8（历史行为）；
    /// 用户在状态栏「编码」菜单选择后记住，此后每次保存沿用，
    /// 重新加载/另存为新路径时重置。不入会话快照（v1 取舍）。
    save_encoding: Option<editpad_core::SaveEncoding>,
}

impl Tab {
    fn empty() -> Self {
        Self {
            editor: EditorHandle::default(),
            path: None,
            dirty: false,
            encoding_label: String::new(),
            version: 0,
            autosave_inflight: false,
            last_edit_at: None,
            untitled_num: None,
            heartbeat_snap: None,
            pinned: false,
            file_stamp: None,
            save_encoding: None,
        }
    }

    /// 不含置脏标记的基础显示名：真实文件名优先，
    /// 未命名页显示「未命名N」（N 为全局单调序号）。
    fn base_name(&self) -> String {
        if let Some(name) = self
            .path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
        {
            return name.to_owned();
        }
        match self.untitled_num {
            Some(n) => format!("未命名{n}"),
            None => "未命名".to_owned(),
        }
    }

    /// 标签条上的显示名：基础名 + 置脏前缀 ●。
    fn display_name(&self) -> String {
        let base = self.base_name();
        if self.dirty {
            format!("● {base}")
        } else {
            base
        }
    }

    /// 记一次真实改动（版本推进 + 防抖起点刷新）。
    fn note_mutation(&mut self) {
        self.version += 1;
        self.last_edit_at = Some(std::time::Instant::now());
    }
}

/// 应用状态（P21 骨架）：`tabs` 是标签页真值集合；
/// busy/status/查找/跳转/确认条等交互态保持全局。
#[derive(Debug, Clone)]
struct Editpad {
    // ---------- 多标签（P21 骨架） ----------
    /// 标签页集合；恒非空（关闭最后一个会重置为新的空标签页）。
    tabs: Vec<Tab>,
    /// 当前激活的标签下标。
    active_tab: usize,
    /// 活动页编辑器句柄的**长期别名**：与 `tabs[active_tab].editor`
    /// 指向同一 RefCell。单独存一份是因为 view()/apply_edit 等需要
    /// 跨语句借用，临时克隆的句柄活不过当前函数。
    cur_handle: EditorHandle,
    /// 对话框/IO 进行中，防止重复触发
    busy: bool,
    status: String,

    // ---------- 即时保存（P18，版本号已下沉 Tab） ----------
    /// 「保存后关闭标签」的目标页；Saved/TabSaved 完成后据此关页
    pending_close_tab: Option<usize>,

    // ---------- 设置 ----------
    settings: editpad_core::Settings,
    /// 配置文件注入点（测试用）；None = 系统配置目录。
    /// 所有 `persist_settings` 都走这里，保证单测绝不碰真实 %APPDATA%。
    settings_path_override: Option<PathBuf>,
    /// 设置弹窗是否可见（P27）：工具栏「设置」按钮开、Esc/关闭按钮关。
    settings_visible: bool,
    /// 设置弹窗当前分类页（P47 侧栏导航；重开弹窗保留上次位置）。
    settings_page: SettingsPage,
    /// 设置弹窗侧栏搜索词（P47；纯 UI 态不落盘，关弹窗/点导航即清）。
    settings_search: String,
    /// P50：外部修改提示条队列（P52 起聚合多页）：待用户裁决的页下标，
    /// 按标签顺序排列。None/空 = 提示条不可见。Esc（BarsDismissed）/
    /// 逐个裁决/全部忽略即清。
    external_change: Option<Vec<usize>>,
    /// P55：就地重命名的目标页下标；Some = 标签条上该页显示为输入框。
    renaming_tab: Option<usize>,
    /// P55：就地重命名的输入内容（预填当前文件名，纯 UI 态）。
    rename_input: String,
    /// P67：状态栏「编码」弹出菜单可见。
    encoding_menu: bool,
    /// P67：状态栏「行尾」弹出菜单可见（与编码菜单互斥）。
    eol_menu: bool,
    /// P70：正则查找模式（查找栏「.*」开关；会话态不持久化）。
    regex_enabled: bool,
    /// P65 双击重命名：标签条上最近一次左键点击的 (页下标, 时刻)。
    /// 同页在 [`TAB_DOUBLE_CLICK_MS`] 窗内再点一次 = 重命名意图。
    /// 纯应用层检测——内层 button 会捕获左键，外层 MouseArea 收不到
    /// on_double_click（iced 事件流实测），故在 SwitchTab 里记账判定。
    last_tab_click: Option<(usize, std::time::Instant)>,
    /// P62：热键捕获态——Some(动作 id) = 设置热键页正在等待新组合键。
    hotkey_capture: Option<&'static str>,

    // ---------- 字体选择（P34） ----------
    /// 启动期从 fontdb 枚举的系统字体族名清单（去重、不区分大小写排序）。
    /// 设置弹窗的选择列表数据源；空 = 枚举失败（弹窗显示提示并隐藏列表）。
    available_fonts: Vec<String>,
    /// 实际生效的字体族名（经 [`effective_font_family`] 对系统清单解析后的
    /// 规范名，已 `'static` 化——见 [`leak_font_family`]）。None = 默认等宽。
    /// 配置值与生效值的分离让「卸载了所选字体」只回退本次渲染，不抹掉
    /// 用户配置（重装后自动恢复）。
    active_font_family: Option<&'static str>,
    /// 设置弹窗字体列表的过滤词（纯 UI 态，不落盘）。
    font_filter: String,

    // ---------- 后台加载 ----------
    job_seq: u64,
    /// 进行中的加载任务；None 表示没有
    active_load: Option<LoadJob>,
    /// (已读字节, 总字节)
    progress: Option<(u64, u64)>,

    // ---------- 查找 / 替换 ----------
    find_visible: bool,
    find_query: String,
    replace_query: String,
    case_sensitive: bool,
    matches: Vec<editpad_core::MatchPos>,
    match_idx: Option<usize>,
    /// 在途后台扫描的序号；None 表示没有。迟到的旧结果按它丢弃（P10）
    find_scan: Option<u64>,
    find_seq: u64,
    /// 当前代扫描的取消标志；新任务排队时把旧标志置位（P10 防抖取消）
    find_cancel: Arc<AtomicBool>,
    /// 「查找全部」结果面板可见（第 62 轮）：数据源 = matches 全量命中表，
    /// 扫描刷新时自动跟随；仅当查找栏可见时有意义（面板停靠在查找区内）
    find_all_visible: bool,

    // ---------- 高亮后台分批补建（P12） ----------
    /// 在途铺建任务的代次；None = 没有。同代幂等、异代重排
    hl_paving: Option<u64>,
    /// 在途铺建任务的取消标志（编辑换代/新任务排队时置位）
    hl_pave_cancel: Arc<AtomicBool>,

    // ---------- 跳转 ----------
    goto_visible: bool,
    goto_input: String,

    // ---------- 最近文件 ----------
    recents_visible: bool,

    // ---------- 未保存关闭确认 ----------
    /// 确认条是否可见
    confirm_visible: bool,
    /// 「保存并关闭」进行中：Saved(Ok) 后执行真正的窗口关闭
    pending_close: bool,
    /// 主窗口 id。iced 0.14 没有 Id::MAIN 之类的常量，只能从 close_requests 订阅捕获
    main_window: Option<window::Id>,

    // ---------- 打开确认 ----------
    /// dirty 时暂存待打开的路径；Some 即打开确认条可见
    open_confirm: Option<PathBuf>,

    // ---------- 标签页关闭确认（P21） ----------
    /// Some(idx) = 第 idx 个标签页置脏，正在确认「放弃更改并关闭」
    close_tab_confirm: Option<usize>,
    // ---------- 标签右键菜单（P28） ----------
    /// Some(idx) = 正在展示第 idx 个标签页的右键菜单
    tab_context_menu: Option<usize>,
    // ---------- 浮层弹窗定位（P39/P40） ----------
    /// 右键菜单锚点（窗口系坐标）：打开菜单那一刻的指针位置
    menu_anchor: (f32, f32),
    /// 指针在标签条区域内的最新位置（锚点数据源；标签条 mouse_area 跟踪）
    cursor_pos: (f32, f32),
    /// 窗口逻辑尺寸（Resized 事件；(0,0) = 未知，贴边钳制跳过）
    viewport_size: (f32, f32),
    /// Some(targets) = 批量关闭（关闭其他/右侧）目标列表，任一置脏时
    /// 先弹一次聚合确认；确认后统一放弃并移除。固定页不在列表内。
    batch_close_confirm: Option<Vec<usize>>,
    // ---------- 未命名页编号（P25） ----------
    /// 下一个未命名页序号（全局单调，不复用已关闭页的号码）
    untitled_next: u64,

    // ---------- 启动会话恢复（P30） ----------
    /// 待载入的命名干净页队列：占位页已在 tabs 中就位，
    /// 逐个经既有加载管线回填内容与视图。
    restore_queue: Vec<RestoreLoad>,
    /// 恢复加载任务的待还原视图：(job id → (行, 列, 垂直滚动, 水平滚动))。
    /// LoadJob 参与 Hash/Eq 不能携带 f32，故挂在应用状态侧按任务号取用；
    /// 取出即视为恢复任务（普通打开不在此表）。
    restore_views: HashMap<u64, (usize, usize, f32, f32)>,
    /// 一次性崩溃恢复提示条（clean_exit=false 的异常退出清单）；
    /// Some = 提示中，等用户裁决恢复或丢弃，数据原封留在磁盘。
    recover_prompt: Option<editpad_core::snapshot::SessionManifest>,
    /// 本轮恢复中未能还原原内容的页数（快照缺失/加载失败；汇总提示口径）
    restore_failed: usize,
    /// 因内存护栏被放弃恢复的页数（§3 P30 第 5 条的截断提示口径）
    restore_dropped: usize,
    /// 尚未落地的恢复加载页数（归零时出汇总状态）
    restore_pending: usize,

    // ---------- 周期快照心跳（P31） ----------
    /// 心跳提交任务在途标记（全局至多一个；页级账目在 Tab 上）
    heartbeat_inflight: bool,
    /// 「内存态比最近一次已提交清单更干净/结构已变」标记：保存清脏、
    /// 关页、换文档等事件置位——下一拍即使无页被代次去重选中也要重写
    /// 清单，防止崩溃恢复把用户已落盘/已关闭的内容按旧快照复活
    /// （编辑置脏不置位：崩溃丢 ≤1 个间隔的输入正是心跳的设计语义）。
    session_manifest_stale: bool,
    /// 快照目录注入点（测试用）；None = 系统配置目录。
    snapshot_dir_override: Option<PathBuf>,

    /// 「恢复上次关闭的文件」记忆栈（第 64 轮）：会话内 Vec<PathBuf>，
    /// 最近期在前；close_tabs_now 统一入栈、ReopenLastClosedFile 出栈
    closed_stack: Vec<PathBuf>,
    /// 第 69 轮：顶部菜单栏当前展开的菜单（None = 全收起）
    menu_bar_open: Option<usize>,
    /// 菜单栏条上最近指针位置（浮层菜单锚点）
    menubar_pos: (f32, f32),
    /// 第 70 轮：展开瞬间的冻结锚点（浮层位置只读它——展开期间悬停
    /// 变化不影响已打开菜单的位置）
    menubar_anchor: (f32, f32),

    // ---------- 外观 ----------
    dark_mode: bool,
    /// Markdown 预览面板可见（P22 第三批；仅 Markdown 语法页渲染）
    preview_visible: bool,
}

impl Default for Editpad {
    fn default() -> Self {
        // P25：初始页即「未命名1」，下一个新页为「未命名2」
        let mut first = Tab::empty();
        first.untitled_num = Some(1);
        let tabs = vec![first];
        let cur_handle = tabs[0].editor.clone();
        Self {
            // P21：初始恒有一个空标签页（tabs 恒非空不变式）
            tabs,
            active_tab: 0,
            cur_handle,
            busy: false,
            status: String::new(),
            settings: editpad_core::Settings::default(),
            settings_path_override: None,
            settings_visible: false,
            settings_page: SettingsPage::default(),
            settings_search: String::new(),
            external_change: None,
            renaming_tab: None,
            rename_input: String::new(),
            encoding_menu: false,
            eol_menu: false,
            regex_enabled: false,
            last_tab_click: None,
            hotkey_capture: None,
            available_fonts: Vec::new(),
            active_font_family: None,
            font_filter: String::new(),
            job_seq: 0,
            active_load: None,
            progress: None,
            find_visible: false,
            find_query: String::new(),
            replace_query: String::new(),
            case_sensitive: false,
            matches: Vec::new(),
            match_idx: None,
            find_scan: None,
            find_seq: 0,
            find_cancel: Arc::default(),
            find_all_visible: false,
            hl_paving: None,
            hl_pave_cancel: Arc::default(),
            goto_visible: false,
            goto_input: String::new(),
            recents_visible: false,
            confirm_visible: false,
            pending_close: false,
            main_window: None,
            open_confirm: None,
            close_tab_confirm: None,
            tab_context_menu: None,
            // P39/P40：浮层定位初值——锚点给一个可见的保守位置，
            // 尺寸未知 (0,0) = 贴边钳制跳过（启动后首个 Resized 事件校准）
            menu_anchor: (24.0, 56.0),
            cursor_pos: (0.0, 0.0),
            viewport_size: (0.0, 0.0),
            batch_close_confirm: None,
            pending_close_tab: None,
            // 第 64 轮：会话内「上次关闭」栈，启动为空（不跨重启）
            closed_stack: Vec::new(),
            menu_bar_open: None,
            menubar_pos: (12.0, 8.0),
            menubar_anchor: (12.0, 8.0),
            dark_mode: false,
            preview_visible: false,
            // P25：初始页即「未命名1」，下一个新页为「未命名2」
            untitled_next: 2,
            // P30：启动会话恢复状态（boot_restore 按清单填充）
            restore_queue: Vec::new(),
            restore_views: HashMap::new(),
            recover_prompt: None,
            restore_failed: 0,
            restore_dropped: 0,
            restore_pending: 0,
            // P31：周期快照心跳状态
            heartbeat_inflight: false,
            session_manifest_stale: false,
            snapshot_dir_override: None,
        }
    }
}


// ---------- 浮层弹窗几何与样式（P39/P40） ----------

/// 右键菜单卡片的估宽（px）：最宽项「关闭其他标签页(N)」≈ 8 汉字 ×16px
/// + 内边距。仅用于贴边钳制，与实际 Shrink 宽度的少量偏差可接受。
const CTX_MENU_W: f32 = 200.0;
/// 右键菜单卡片的估高（px）：标题行 + 6 个菜单项 + 分隔线 + 内边距。
/// 窗口高度未知或极小（≤48px）时作为保守值：钳制与滚动共用。
const CTX_MENU_H: f32 = 280.0;

/// P43：右键菜单卡片高度适配——窗口高度已知且足够时最高占
/// `vh − 16`（上下各留 8px 边距），内容超高时卡片内部滚动；
/// 窗口高度未知/过小（首帧或极小窗）回退常量估高（仍保证 ≤ 全高，
/// 永不盖满界面）。纯函数可单测。
// ---------- 第 70 轮：菜单栏槽位几何 ----------

/// 菜单栏按钮槽宽与左缘（与 view.rs 菜单栏布局耦合：五按钮同宽 2 字
/// 文本 + padding [3,12] + 1px 边框 + spacing 2；布局改动需同步）。
pub(crate) const MENU_SLOT_W: f32 = 54.0;
pub(crate) const MENU_BAR_LEFT: f32 = 6.0;
/// 菜单栏条带高度（浮层背板据此判定「点击落在菜单栏上=切换菜单」）。
pub(crate) const MENU_BAR_H: f32 = 36.0;

/// 悬停 x → 所属菜单槽位序号（0..=4，纯函数可单测）。
pub(crate) fn menubar_slot_idx(x: f32) -> usize {
    (((x - MENU_BAR_LEFT) / MENU_SLOT_W).floor().max(0.0) as usize).min(4)
}

fn ctx_menu_card_h(vh: f32) -> f32 {    if vh > 48.0 {
        (vh - 16.0).min(CTX_MENU_H)
    } else {
        CTX_MENU_H
    }
}

/// P39：浮层锚点贴边钳制——菜单整体保持在窗口内（右缘翻左/下缘翻上
/// 的效果 = 把锚点往回拉）。窗口尺寸未知（宽或高为 0）时该轴不钳制；
/// 结果恒非负。纯函数可单测。
fn clamp_menu_anchor(
    pos: (f32, f32),
    viewport: (f32, f32),
    menu_w: f32,
    menu_h: f32,
) -> (f32, f32) {
    let (mut x, mut y) = pos;
    let (vw, vh) = viewport;
    if vw > 0.0 && menu_w.is_finite() && menu_w > 0.0 {
        x = x.min((vw - menu_w).max(0.0));
    }
    if vh > 0.0 && menu_h.is_finite() && menu_h > 0.0 {
        y = y.min((vh - menu_h).max(0.0));
    }
    (x.max(0.0), y.max(0.0))
}

/// P39/P40：浮层卡片样式——主题背景 + 1px 描边（**无投影**），深浅主题
/// 通用（从 palette 派生，与编辑器 EditorColors::resolve 同一口径）。
///
/// 无投影是有意取舍（P43 追加）：tiny-skia 后端部分重绘的 damage 区只覆盖
/// quad.bounds + 1px，而 shadow 模糊带外扩 blur_radius（4px）且在 alpha 合成
/// 下残留叠加——鼠标移动反复触发 hover 重绘时阴影逐帧变浓，表现为
/// 「弹窗随鼠标移动越来越暗」。1px 描边 + 圆角已足以区分浮层与背景。
fn popup_card_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Background::Color(palette.background)),
        border: Border {
            color: Color { a: 0.35, ..palette.text },
            width: 1.0,
            radius: Radius::from(6.0),
        },
        shadow: Shadow::default(),
        ..container::Style::default()
    }
}

// ---------- 热键系统（P62 可重映射） ----------

/// 热键动作：持久化主键 `id` + 默认组合 + 说明。
///
/// `id` 是 config.toml `hotkeys` 映射的键——**改 id = 破坏用户配置**，
/// 禁止；改默认组合只影响未重映射的用户。新增动作 = 追加一行 +
/// [`dispatch_action`] 加一个分支（测试清单自动覆盖）。
struct HotkeyAction {
    id: &'static str,
    default_combo: &'static str,
    desc: &'static str,
}

/// 键盘热键动作注册表（展示顺序 = 设置热键页顺序）。
/// Shift+滚轮 / Ctrl+滚轮为鼠标动作，不可重映射，仅 README 展示。
const HOTKEY_ACTIONS: &[HotkeyAction] = &[
    HotkeyAction { id: "open", default_combo: "Ctrl+O", desc: "打开文件" },
    HotkeyAction { id: "save", default_combo: "Ctrl+S", desc: "保存" },
    HotkeyAction { id: "select_all", default_combo: "Ctrl+A", desc: "全选" },
    HotkeyAction { id: "find", default_combo: "Ctrl+F", desc: "查找/替换栏" },
    HotkeyAction { id: "goto", default_combo: "Ctrl+G", desc: "跳转到行" },
    HotkeyAction { id: "undo", default_combo: "Ctrl+Z", desc: "撤销" },
    HotkeyAction { id: "redo", default_combo: "Ctrl+Y", desc: "重做" },
    HotkeyAction { id: "copy", default_combo: "Ctrl+C", desc: "复制选区" },
    HotkeyAction { id: "cut", default_combo: "Ctrl+X", desc: "剪切选区" },
    HotkeyAction { id: "paste", default_combo: "Ctrl+V", desc: "粘贴" },
    // 行操作套件（第 57 轮，仿主流编辑器编辑菜单）
    HotkeyAction { id: "dup_line", default_combo: "Ctrl+D", desc: "在下方复制当前行" },
    HotkeyAction { id: "del_line", default_combo: "Ctrl+L", desc: "删除当前行" },
    HotkeyAction { id: "move_line_up", default_combo: "Ctrl+Shift+Up", desc: "当前行上移" },
    HotkeyAction { id: "move_line_down", default_combo: "Ctrl+Shift+Down", desc: "当前行下移" },
    // 大小写转换与行首尾清理（第 58 轮；大小写默认键与主流编辑器一致，
    // Trim 在主流编辑器无默认键——取 Ctrl+Shift+T/L/B 助记：Trim/Leading/Both）
    HotkeyAction { id: "to_uppercase", default_combo: "Ctrl+Shift+U", desc: "转为大写（选区/全文）" },
    HotkeyAction { id: "to_lowercase", default_combo: "Ctrl+U", desc: "转为小写（选区/全文）" },
    HotkeyAction { id: "trim_trailing", default_combo: "Ctrl+Shift+T", desc: "去除行尾空白（选区行/全文）" },
    HotkeyAction { id: "trim_leading", default_combo: "Ctrl+Shift+L", desc: "去除行首空白（选区行/全文）" },
    HotkeyAction { id: "trim_both", default_combo: "Ctrl+Shift+B", desc: "去除行首尾空白（选区行/全文）" },
    // 行排序与去重（第 59 轮，仿主流编辑器行操作菜单；S/D=升/降序助记，K=去重）
    HotkeyAction { id: "sort_lines_asc", default_combo: "Ctrl+Shift+S", desc: "行升序排序（选区行/全文）" },
    HotkeyAction { id: "sort_lines_desc", default_combo: "Ctrl+Shift+D", desc: "行降序排序（选区行/全文）" },
    HotkeyAction { id: "dedupe_lines", default_combo: "Ctrl+Shift+K", desc: "去除重复行（选区行/全文）" },
    // 书签套件（第 60 轮，仿主流编辑器书签导航）。F2 家族与主流编辑器
    // 同默认键；Alt 系键位因 AltGr 保护不可用，清除全部取 Ctrl+Shift+F2；
    // 标记行批量操作无主流默认键，取 C/X 助记 = 复制/删除
    HotkeyAction { id: "bookmark_toggle", default_combo: "Ctrl+F2", desc: "切换当前行书签" },
    HotkeyAction { id: "bookmark_next", default_combo: "F2", desc: "跳到下一个书签" },
    HotkeyAction { id: "bookmark_prev", default_combo: "Shift+F2", desc: "跳到上一个书签" },
    HotkeyAction { id: "bookmark_clear_all", default_combo: "Ctrl+Shift+F2", desc: "清除全部书签" },
    HotkeyAction { id: "copy_marked_lines", default_combo: "Ctrl+Shift+C", desc: "复制全部标记行" },
    HotkeyAction { id: "del_marked_lines", default_combo: "Ctrl+Shift+X", desc: "删除全部标记行" },
    // 括号匹配（第 61 轮，仿主流编辑器；M=Matching 助记——主流的 Ctrl+\
    // 因反斜杠非字母/数字不入组合键白名单）
    HotkeyAction { id: "jump_bracket", default_combo: "Ctrl+Shift+M", desc: "跳到配对括号" },
    // 行操作扩充（第 62 轮；空闲字母键取 I/O/P/J/H/N/R，均可在设置页重映射）
    HotkeyAction { id: "tabs_to_spaces_leading", default_combo: "Ctrl+Shift+I", desc: "行首制表符转空格（选区行/全文）" },
    HotkeyAction { id: "tabs_to_spaces_all", default_combo: "Ctrl+Shift+O", desc: "全部制表符转空格（选区行/全文）" },
    HotkeyAction { id: "spaces_to_tabs_leading", default_combo: "Ctrl+Shift+P", desc: "行首空格转制表符（选区行/全文）" },
    HotkeyAction { id: "merge_lines", default_combo: "Ctrl+Shift+J", desc: "合并行（触及块合成一行/并入下一行）" },
    HotkeyAction { id: "split_line", default_combo: "Ctrl+Shift+H", desc: "拆分行（光标处断行/选区独立成行）" },
    HotkeyAction { id: "del_empty_lines", default_combo: "Ctrl+Shift+N", desc: "删除空行（选区行/全文）" },
    HotkeyAction { id: "del_blank_lines", default_combo: "Ctrl+Shift+R", desc: "删除空白行（含纯空白行）" },
    // 查找全部结果面板（A = All matches 助记；数据源复用查找栏的后台扫描命中表）
    HotkeyAction { id: "find_all_panel", default_combo: "Ctrl+Shift+A", desc: "查找全部结果面板" },
    // 第 63 轮：F5 插入日期时间（记事本同款裸功能键）+ 复制路径/文件名
    // （G/Q 为空闲字母，弱助记但 desc 明示；均可在设置页重映射）
    HotkeyAction { id: "insert_date_time", default_combo: "F5", desc: "插入日期时间（YYYY-MM-DD HH:MM）" },
    HotkeyAction { id: "copy_file_path", default_combo: "Ctrl+Shift+G", desc: "复制完整路径（当前页）" },
    HotkeyAction { id: "copy_file_name", default_combo: "Ctrl+Shift+Q", desc: "复制文件名（当前页）" },
    // 第 64 轮：行注释切换（Ctrl+Q，多编辑器通用的注释切换键位）+
    // 恢复上次关闭的标签页（W=与 Ctrl+W 关页互逆的助记）
    HotkeyAction { id: "toggle_line_comment", default_combo: "Ctrl+Q", desc: "切换行注释（按语法选 // # -- 等）" },
    HotkeyAction { id: "reopen_closed_tab", default_combo: "Ctrl+Shift+W", desc: "恢复上次关闭的标签页" },
    HotkeyAction { id: "new_tab", default_combo: "Ctrl+T", desc: "新建标签页" },
    HotkeyAction { id: "close_tab", default_combo: "Ctrl+W", desc: "关闭当前标签页" },
    HotkeyAction { id: "next_tab", default_combo: "Ctrl+Tab", desc: "循环切换标签页" },
    HotkeyAction { id: "format_json", default_combo: "Ctrl+Shift+F", desc: "格式化 JSON（仅 JSON 文件）" },
    HotkeyAction { id: "doc_start", default_combo: "Ctrl+Home", desc: "跳到文档首" },
    HotkeyAction { id: "doc_end", default_combo: "Ctrl+End", desc: "跳到文档尾" },
];

/// 动作 id 的默认组合（未重映射时的组合键串）。
fn default_combo_of(id: &str) -> Option<&'static str> {
    HOTKEY_ACTIONS
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.default_combo)
}

/// 动作 id 的当前生效组合：用户重映射优先，否则默认。
fn effective_combo<'a>(
    id: &str,
    remap: &'a HashMap<String, String>,
) -> Option<&'a str> {
    remap
        .get(id)
        .map(String::as_str)
        .or_else(|| default_combo_of(id))
}

/// 过滤用户重映射表：动作 id 不在注册表中的条目删除（启动加载时调用）。
fn sanitize_hotkeys(settings: &mut editpad_core::Settings) {
    settings
        .hotkeys
        .retain(|id, _| HOTKEY_ACTIONS.iter().any(|a| a.id == *id));
}

/// 按键 → 规范组合串（P62；第 60 轮放宽功能键）：`Ctrl [+Shift] +键名`。
///
/// 契约（与 core `normalize_combo` 同步）：不得含 Alt（AltGr 保护，P8
/// 同口径——AltGr 在 Windows 上报为 Ctrl+Alt）；
/// - 含 Ctrl：键名 = 单个字母/数字（大写化）或白名单命名键，形如
///   `Ctrl+F2`；
/// - 无 Ctrl：仅放行功能键 F1~F12（修饰键至多 Shift），形如 `F2` /
///   `Shift+F2`——书签导航的主流默认键位所需；其余无 Ctrl 按键返回
///   None（该按键不参与热键系统，交回普通编辑路径）。
fn combo_string(mods: keyboard::Modifiers, key: &keyboard::Key) -> Option<String> {
    use keyboard::Key;
    if mods.alt() {
        return None;
    }
    let key_name = match key {
        Key::Character(chars) => {
            // 无 Ctrl 的字符键是打字正文，绝不参与热键
            if !mods.control() {
                return None;
            }
            let mut it = chars.chars();
            match (it.next(), it.next()) {
                (Some(ch), None) if ch.is_ascii_alphanumeric() => {
                    ch.to_ascii_uppercase().to_string()
                }
                _ => return None,
            }
        }
        Key::Named(named) => {
            let name = match named {
                Named::Home => "Home",
                Named::End => "End",
                Named::PageUp => "PageUp",
                Named::PageDown => "PageDown",
                Named::Tab => "Tab",
                Named::Insert => "Insert",
                Named::Delete => "Delete",
                Named::ArrowUp => "Up",
                Named::ArrowDown => "Down",
                Named::ArrowLeft => "Left",
                Named::ArrowRight => "Right",
                Named::F1 => "F1",
                Named::F2 => "F2",
                Named::F3 => "F3",
                Named::F4 => "F4",
                Named::F5 => "F5",
                Named::F6 => "F6",
                Named::F7 => "F7",
                Named::F8 => "F8",
                Named::F9 => "F9",
                Named::F10 => "F10",
                Named::F11 => "F11",
                Named::F12 => "F12",
                _ => return None,
            };
            name.to_string()
        }
        _ => return None,
    };
    if mods.control() {
        let mut combo = String::from("Ctrl");
        if mods.shift() {
            combo.push_str("+Shift");
        }
        combo.push('+');
        combo.push_str(&key_name);
        Some(combo)
    } else if is_function_key_name(&key_name) {
        // 第 60 轮：无 Ctrl 仅放行 F1~F12（至多带 Shift）
        let mut combo = String::new();
        if mods.shift() {
            combo.push_str("Shift+");
        }
        combo.push_str(&key_name);
        Some(combo)
    } else {
        None
    }
}

/// 键名是否为功能键 `F1`~`F12`（第 60 轮：无 Ctrl 组合的白名单）。
fn is_function_key_name(name: &str) -> bool {
    name.len() >= 2
        && name.starts_with('F')
        && name[1..].parse::<u8>().map(|n| (1..=12).contains(&n)).unwrap_or(false)
}

/// 生效动作查询：精确组合 → 用户重映射优先，其次**未重映射**动作的默认
/// 组合（已重映射的动作让出默认键位——用户显式挪走即视为放弃旧键位）。
fn effective_action(
    combo: &str,
    remap: &HashMap<String, String>,
) -> Option<&'static str> {
    for action in HOTKEY_ACTIONS {
        if remap.get(action.id).map(String::as_str) == Some(combo) {
            return Some(action.id);
        }
    }
    for action in HOTKEY_ACTIONS {
        if !remap.contains_key(action.id) && action.default_combo == combo {
            return Some(action.id);
        }
    }
    None
}

/// 去掉组合串中的 Shift 修饰（文档导航的选区变体：Ctrl+Shift+Home 与
/// Ctrl+Home 同指 doc_start，Shift 经由 dispatch 透传给 Motion）。
fn shiftless(combo: &str) -> String {
    combo.replace("Shift+", "")
}

/// 动作 id → 消息。`mods` 供文档导航动作透传 Shift（选区语义）。
fn dispatch_action(id: &str, mods: keyboard::Modifiers) -> Option<Message> {
    let edit = |op| Some(Message::Edit(op));
    match id {
        "open" => Some(Message::OpenRequested),
        "save" => Some(Message::SaveRequested),
        "select_all" => Some(Message::Edit(EditOp::SelectAll)),
        "find" => Some(Message::FindToggled),
        "goto" => Some(Message::GotoToggled),
        "undo" => Some(Message::Edit(EditOp::Undo)),
        "redo" => Some(Message::Edit(EditOp::Redo)),
        "copy" => Some(Message::CopyRequested),
        "cut" => Some(Message::CutRequested),
        "paste" => Some(Message::PasteRequested),
        // 行操作套件（第 57 轮）
        "dup_line" => edit(EditOp::DuplicateLines),
        "del_line" => edit(EditOp::DeleteLines),
        "move_line_up" => edit(EditOp::MoveLinesUp),
        "move_line_down" => edit(EditOp::MoveLinesDown),
        // 大小写转换与行首尾清理（第 58 轮）
        "to_uppercase" => edit(EditOp::ConvertCase(CaseKind::Upper)),
        "to_lowercase" => edit(EditOp::ConvertCase(CaseKind::Lower)),
        "trim_leading" => edit(EditOp::TrimLines(TrimMode::Leading)),
        "trim_trailing" => edit(EditOp::TrimLines(TrimMode::Trailing)),
        "trim_both" => edit(EditOp::TrimLines(TrimMode::Both)),
        // 行排序与去重（第 59 轮）
        "sort_lines_asc" => edit(EditOp::SortLines(SortOrder::Ascending)),
        "sort_lines_desc" => edit(EditOp::SortLines(SortOrder::Descending)),
        "dedupe_lines" => edit(EditOp::RemoveDuplicateLines),
        // 书签套件（第 60 轮；copy_marked_lines 的剪贴板写入在消息层拦截）
        "bookmark_toggle" => edit(EditOp::ToggleBookmark),
        "bookmark_next" => edit(EditOp::BookmarkNext),
        "bookmark_prev" => edit(EditOp::BookmarkPrev),
        "bookmark_clear_all" => edit(EditOp::BookmarksClearAll),
        "copy_marked_lines" => edit(EditOp::CopyBookmarkedLines),
        "del_marked_lines" => edit(EditOp::RemoveBookmarkedLines),
        // 括号匹配（第 61 轮）
        "jump_bracket" => edit(EditOp::JumpToMatchingBracket),
        // 行操作扩充（第 62 轮）
        "tabs_to_spaces_leading" => {
            edit(EditOp::ConvertTabsSpaces(TabSpaceKind::LeadingTabsToSpaces))
        }
        "tabs_to_spaces_all" => edit(EditOp::ConvertTabsSpaces(TabSpaceKind::AllTabsToSpaces)),
        "spaces_to_tabs_leading" => {
            edit(EditOp::ConvertTabsSpaces(TabSpaceKind::LeadingSpacesToTabs))
        }
        "merge_lines" => edit(EditOp::MergeLines),
        "split_line" => edit(EditOp::SplitLine),
        "del_empty_lines" => edit(EditOp::DeleteEmptyLines(BlankKind::Empty)),
        "del_blank_lines" => edit(EditOp::DeleteEmptyLines(BlankKind::Whitespace)),
        // 查找全部结果面板（第 62 轮）
        "find_all_panel" => Some(Message::FindAllToggled),
        // 第 63 轮：热键版复制路径/文件名以活动页为目标（None）；右键
        // 菜单走同一消息的 Some(idx) 形态
        "insert_date_time" => edit(EditOp::InsertDateTime),
        "copy_file_path" => Some(Message::CopyFilePath(None)),
        "copy_file_name" => Some(Message::CopyFileName(None)),
        // 第 64 轮
        "toggle_line_comment" => edit(EditOp::ToggleLineComment),
        "reopen_closed_tab" => Some(Message::ReopenLastClosedFile),
        "new_tab" => Some(Message::NewTab),
        "close_tab" => Some(Message::CloseTabRequest),
        "next_tab" => Some(Message::SwitchTabNext),
        "format_json" => Some(Message::FormatJson),
        "doc_start" => edit(EditOp::Motion(Motion::DocStart, mods.shift())),
        "doc_end" => edit(EditOp::Motion(Motion::DocEnd, mods.shift())),
        _ => None,
    }
}

/// 全局按键分发（P62 可重映射）。
///
/// 顺序：① Ctrl 组合（无 Alt，P8 AltGr 保护）→ 规范组合串 → 生效动作表
/// （用户重映射优先，其次默认）→ 分发；Ctrl+Shift+Home/End 经 shiftless
/// 回退命中 doc_start/doc_end 并透传 Shift（选区语义）。② 其余按键
/// （字符输入/光标移动/回车退格/Esc）走既有编辑路径。
fn handle_key(
    key: keyboard::Key,
    mods: keyboard::Modifiers,
    remap: &HashMap<String, String>,
) -> Option<Message> {
    use keyboard::Key;
    let edit = |op| Some(Message::Edit(op));

    if mods.control() && !mods.alt() {
        if let Some(combo) = combo_string(mods, &key) {
            let id = effective_action(&combo, remap).or_else(|| {
                let stripped = shiftless(&combo);
                if stripped == combo {
                    return None;
                }
                // 仅文档导航接受 Shift 变体回退（其余动作 Shift 组合是
                // 独立动作，如 Ctrl+Shift+F ≠ Ctrl+F）
                effective_action(&stripped, remap)
                    .filter(|id| matches!(*id, "doc_start" | "doc_end"))
            });
            if let Some(id) = id {
                return dispatch_action(id, mods);
            }
        }
        return None;
    }

    // 第 60 轮：裸功能键热键（书签导航 F2 家族）。combo_string 在无 Ctrl
    // 时只对 F1~F12 产出组合串，未注册动作的功能键放行（F 键本无编辑
    // 语义，落到末尾返回 None 与既有行为一致）
    if let Some(combo) = combo_string(mods, &key) {
        if let Some(id) = effective_action(&combo, remap) {
            return dispatch_action(id, mods);
        }
    }

    match &key {
        Key::Character(chars) => edit(EditOp::InsertText(chars.to_string())),
        Key::Named(Named::Backspace) => edit(EditOp::Backspace),
        Key::Named(Named::Delete) => edit(EditOp::Delete),
        // P9：统一插 \n，由 insert_str 归一为文档主导行尾（CRLF 文档得 \r\n）
        Key::Named(Named::Enter) => edit(EditOp::InsertText("\n".into())),
        // P14：Tab 插入真实制表符；显示层由 editor::char_cols 展开到制表位，
        // 文档字节保持原样（保存往返不失真）
        Key::Named(Named::Tab) => edit(EditOp::InsertText("\t".into())),
        Key::Named(Named::Escape) => Some(Message::BarsDismissed),

        Key::Named(Named::ArrowLeft) => edit(EditOp::Motion(Motion::Left, mods.shift())),
        Key::Named(Named::ArrowRight) => edit(EditOp::Motion(Motion::Right, mods.shift())),
        Key::Named(Named::ArrowUp) => edit(EditOp::Motion(Motion::Up, mods.shift())),
        Key::Named(Named::ArrowDown) => edit(EditOp::Motion(Motion::Down, mods.shift())),
        Key::Named(Named::Home) => edit(EditOp::Motion(Motion::Home, mods.shift())),
        Key::Named(Named::End) => edit(EditOp::Motion(Motion::End, mods.shift())),
        Key::Named(Named::PageUp) => edit(EditOp::Motion(Motion::PageUp, mods.shift())),
        Key::Named(Named::PageDown) => edit(EditOp::Motion(Motion::PageDown, mods.shift())),

        _ => None,
    }
}

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

/// ICO 条目：尺寸与 DIB 像素区（BMP BITMAPINFOHEADER 起，含 AND mask）。
struct IcoEntry {
    width: u32,
    height: u32,
    bit_count: u16,
    /// 图像数据区整体（40B BITMAPINFOHEADER + 像素 + AND mask）。
    data: &'static [u8],
}

/// 解析 app.ico 的全部条目（ICONDIR 骨架，零依赖）。
/// 本资源（P71 用户四档 ICO，首字节 28 00 00 00 = BITMAPINFOHEADER）
/// 全部为 ICO 内嵌 DIB；PNG 条目不在支持范围（不存在于本资源）。
fn parse_app_ico_entries(raw: &'static [u8]) -> Vec<IcoEntry> {
    let mut out = Vec::new();
    if raw.len() < 6 {
        return out;
    }
    let count = u16::from_le_bytes([raw[4], raw[5]]) as usize;
    for i in 0..count {
        let o = 6 + i * 16;
        if o + 16 > raw.len() {
            break;
        }
        let width = if raw[o] == 0 { 256 } else { raw[o] as u32 };
        let height = if raw[o + 1] == 0 { 256 } else { raw[o + 1] as u32 };
        let bytes = u32::from_le_bytes([raw[o + 8], raw[o + 9], raw[o + 10], raw[o + 11]]) as usize;
        let off = u32::from_le_bytes([raw[o + 12], raw[o + 13], raw[o + 14], raw[o + 15]]) as usize;
        if off + bytes > raw.len() || bytes < 40 {
            continue;
        }
        let bit_count = u16::from_le_bytes([raw[off + 14], raw[off + 15]]);
        out.push(IcoEntry { width, height, bit_count, data: &raw[off..off + bytes] });
    }
    out
}

/// ICO 内嵌 32bpp DIB → RGBA（自底向上翻转行，忽略 AND mask）。
/// 仅支持 BI_RGB + 32bpp（本资源全为此格式）；其余返回 None。
fn dib_bgra32_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if dib.len() < 40 {
        return None;
    }
    let w = i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]);
    let h2 = i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]);
    if w <= 0 || h2 <= 0 {
        return None;
    }
    // ICO 内嵌 DIB 的 biHeight = 2 × 实际高度（高度 + AND mask 各一份）
    let h = h2 / 2;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let comp = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    if bit_count != 32 || comp != 0 {
        return None;
    }
    let (w, h) = (w as u32, h as u32);
    let row_stride = w as usize * 4;
    let px = 40usize; // 32bpp 无调色板：头部后直接是底向上 BGRA 像素
    if dib.len() < px + row_stride * h as usize {
        return None;
    }
    let mut rgba = vec![0u8; row_stride * h as usize];
    for y in 0..h as usize {
        let src = &dib[px + row_stride * y..px + row_stride * (y + 1)];
        let dst = &mut rgba[row_stride * (h as usize - 1 - y)..row_stride * (h as usize - y)];
        for x in 0..w as usize {
            dst[x * 4] = src[x * 4 + 2]; // BGR → RGB
            dst[x * 4 + 1] = src[x * 4 + 1];
            dst[x * 4 + 2] = src[x * 4];
            dst[x * 4 + 3] = src[x * 4 + 3];
        }
    }
    Some((w, h, rgba))
}

/// 窗口标题栏/任务栏图标（第 76 轮用户点单：替换标题栏默认通用图标）：
/// P71 自定义四档 ICO 的 **48px** 条目 → RGBA（winit 期望**预乘 alpha**，
/// 防透明边缘发亮/漏色）。解析失败返回 None（winit 回退 exe 资源图标）。
/// OnceLock 缓存：只解析一次（Icon: Clone，重复取零成本）。
pub(crate) fn window_title_icon() -> Option<iced::window::Icon> {
    static ICON: std::sync::OnceLock<Option<iced::window::Icon>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        for e in parse_app_ico_entries(include_bytes!("../assets/app.ico")) {
            if e.width == 48 && e.height == 48 && e.bit_count == 32 {
                if let Some((w, h, rgba)) = dib_bgra32_to_rgba(e.data) {
                    // sRGB → 预乘 alpha（winit/windows 标题栏渲染要求）
                    let premul: Vec<u8> = rgba
                        .chunks_exact(4)
                        .flat_map(|px| {
                            let a = px[3] as u32;
                            [
                                (px[0] as u32 * a / 255) as u8,
                                (px[1] as u32 * a / 255) as u8,
                                (px[2] as u32 * a / 255) as u8,
                                px[3],
                            ]
                        })
                        .collect();
                    return iced::window::icon::from_rgba(premul, w, h).ok();
                }
            }
        }
        None
    })
    .clone()
}

/// P55/P64：标签就地重命名输入框的唯一 id。视图侧 text_input 挂同一
/// id；进入重命名态时聚焦 + 全选（iced::widget::operation 对该 id 的
/// 操作在下一帧视图含输入框后生效），用户可直接键入覆盖预填旧名。
pub(crate) fn rename_input_id() -> iced::widget::Id {
    iced::widget::Id::new("editpad-tab-rename")
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
mod settings_ui;
mod update;
mod view;
#[cfg(test)]
mod tests;

use settings_ui::SettingsPage;
