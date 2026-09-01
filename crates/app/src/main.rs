// P24：发布版隐藏随 GUI 一起弹出的控制台黑窗（Windows 子系统属性）；
// 调试构建保留控制台，便于直接 cargo run 看日志输出。
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

//! Editpad —— 极简记事本。
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
    Font, Padding, Point, Shadow, Shrink, Subscription, Task, Theme};

use editor::{EditorHandle, EditOp, Motion};

fn main() -> iced::Result {
    // iced 0.14：第一个参数是 boot 函数（返回初始状态），title/theme/subscription 走 builder
    iced::application(Editpad::new, Editpad::update, Editpad::view)
        .title(Editpad::title)
        .theme(Editpad::theme)
        .subscription(Editpad::subscription)
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
    /// 保存完成：(落盘内容的内容版本号, 结果)（P18 版本守卫）
    Saved(u64, Result<(), String>),
    /// 标签页保存完成（「保存并关闭」流程用）：(页, 快照版本, 结果)
    TabSaved(usize, u64, Result<(), String>),
    /// 自动保存完成：(标签页, 快照版本, 结果)。版本不符=期间又有编辑，不清脏
    TabAutosaved(usize, u64, Result<(), String>),

    FindToggled,
    FindQueryChanged(String),
    FindNext,
    FindPrev,
    CaseToggled(bool),
    ReplaceQueryChanged(String),
    ReplaceCurrent,
    ReplaceAll,
    /// 后台查找扫描完成：(任务序号, 命中表)。序号过期的结果直接丢弃（P10）
    FindScanDone(u64, Vec<editpad_core::MatchPos>),

    /// 可见区高亮缺档超内联预算，请求安排后台分批补建（P12）。
    /// 同代在途时应用层幂等跳过，重复发布无害。
    HighlightPaveNeeded,

    // ---------- 多标签（P21） ----------
    /// 新建空标签页（Ctrl+T）
    NewTab,
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
    /// 菜单「另存为/重命名」：切到第 `idx` 页走既有另存为对话框
    /// （§3 P28 第 2 条：重命名 v1 用另存为兜底）
    RenameOrSaveAsTab(usize),
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
    F: FnOnce(&editpad_core::Document, &str, bool) -> Vec<editpad_core::MatchPos>
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
                scan(&payload.doc, &payload.query, payload.case_sensitive)
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

/// P6 编码知情权：saver 只写 UTF-8，原文件若是其他编码（或带 BOM），
/// 首次保存即发生不可逆转码 / BOM 丢失。返回需要展示的提示；None 表示无需提示。
fn transcode_notice(original_encoding: &str) -> Option<String> {
    match original_encoding {
        "" | "UTF-8" => None,
        "UTF-8(BOM)" => {
            Some("已按 UTF-8（无 BOM）保存：原文件的 BOM 已丢失".to_owned())
        }
        other => Some(format!("已按 UTF-8 保存：原编码为 {other}，转码不可逆")),
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

/// 一次自动保存的驱动：专属 OS 线程「睡满防抖窗 → 分块原子落盘」，
/// 结果经 std mpsc 桥接回异步端（P5/P10 同款；执行器仅阻塞等待结果，
/// 且按页 inflight 去重保证同一页至多一个这样的线程）。
async fn drive_autosave_once(
    tab: usize,
    path: PathBuf,
    doc: editpad_core::Document,
    version: u64,
    delay: std::time::Duration,
) -> Message {
    let (tx, rx) = std_mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let result =
            editpad_core::save_document_atomic(&path, &doc).map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    let result = rx
        .recv()
        .unwrap_or_else(|_| Err("自动保存线程意外终止".to_owned()));
    Message::TabAutosaved(tab, version, result)
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

impl Editpad {
    // ---------- 多标签访问器（P21） ----------

    /// 新建标签页的唯一出口：`Tab::empty()` + 套用全局字号设置。
    ///
    /// 历史教训（P43）：曾有多处直接用 `Tab::empty()` 建页——`EditorCore`
    /// 默认字号恒为 16，恢复会话/新建页/关光补页全部脱离设置里的字号，
    /// 表现为「调到 24 生效、重启后字体回到原大小，但设置面板仍显示 24」
    /// （恢复路径整体替换 tabs，boot 仅对首个默认页应用过字号）。
    /// 今后凡「新建页」一律走这里；boot 首帧仍显式 set_font_size 兜底
    /// （fresh_tab 幂等，重复应用无害）。
    fn fresh_tab(&mut self) -> Tab {
        let tab = Tab::empty();
        // 设置里的字号可能未归一（旧配置/手改），与 boot 同一 clamp 规则
        tab.editor
            .borrow_mut()
            .set_font_size(editor::normalize_font_size(self.settings.font_size));
        tab
    }

    /// 进入 busy（对话框/IO 互斥）。P39 浮层化后的新约束：同时收起
    /// 右键菜单浮层——它的透明背板会挡住整窗点击，busy 期间不能留它挡道
    /// （旧内嵌面板无此问题，故此前各 busy 置位点都无需理会菜单态）。
    fn enter_busy(&mut self) {
        self.busy = true;
        self.tab_context_menu = None;
    }

    /// 当前激活标签页。
    fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// 当前激活标签页（可变）。
    fn tab_mut(&mut self) -> &mut Tab {
        let i = self.active_tab;
        &mut self.tabs[i]
    }

    /// 活动页句柄的长期引用（跨语句借用走 `cur_handle` 字段；
    /// 单表达式内的临时借用也可用 [`Self::cur`]）。
    fn cur(&self) -> EditorHandle {
        self.cur_handle.clone()
    }

    /// 为第 `idx` 页分配下一个未命名序号（P25）：全局单调、不复用
    /// 已关闭页的号码——杜绝两个同名未命名页。
    fn assign_untitled_num(&mut self, idx: usize) {
        let n = self.untitled_next;
        self.untitled_next += 1;
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.untitled_num = Some(n);
        }
    }

    /// 切换活动页并同步长期别名（所有 active_tab 变更必须经此或
    /// [`Self::refresh_cur_handle`])。
    fn set_active_tab(&mut self, idx: usize) {
        self.active_tab = idx.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 与 tabs 对齐刷新别名（增删页后调用）。
    fn refresh_cur_handle(&mut self) {
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 任一标签页有未保存改动（窗口关闭确认的聚合口径）。
    fn any_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    /// 第 `idx` 页的路径（存在该页时）。
    fn path_of_tab(&self, idx: usize) -> Option<PathBuf> {
        self.tabs.get(idx).and_then(|t| t.path.clone())
    }

    /// 关闭第 `idx` 个标签页；关到最后一个时重置为新的空标签页
    /// （新页分配下一个未命名序号）。返回是否真的移除了页面。
    fn close_tab_now(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() {
            return false;
        }
        // P32：移除前把该页光标/滚动回写最近文件记忆
        self.remember_tab_views(&[idx]);
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
            let last = self.tabs.len() - 1;
            self.assign_untitled_num(last);
        }
        // P31：页集合结构已变——下一拍重写清单，防崩溃恢复复活已关的页
        self.session_manifest_stale = true;
        // 与 tabs 对齐（含越界夹紧），并同步活动页句柄别名
        self.refresh_cur_handle();
        true
    }

    /// 批量移除多个标签页（P28）：按下标从大到小逐个 remove，
    /// 保证剩余下标始终有效；全部移光时重置一个新的空标签页
    /// （tabs 恒非空不变式，同 [`Self::close_tab_now`]）。
    /// 返回实际移除的页数。越界/重复下标安全跳过。
    fn close_tabs_now(&mut self, indices: &[usize]) -> usize {
        // P32：移除前按原下标批量回写光标/滚动记忆
        self.remember_tab_views(indices);
        let mut idxs = indices.to_vec();
        idxs.sort_unstable();
        idxs.dedup();
        let mut removed = 0usize;
        for &idx in idxs.iter().rev() {
            if idx < self.tabs.len() {
                self.tabs.remove(idx);
                removed += 1;
            }
        }
        if removed == 0 {
            return 0;
        }
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
            let last = self.tabs.len() - 1;
            self.assign_untitled_num(last);
        }
        self.session_manifest_stale = true;
        self.refresh_cur_handle();
        removed
    }

    /// 开启批量关闭流程（P28）：目标列表非空且任一置脏 → 弹一次聚合
    /// 确认条（确认后统一放弃，§3 P28 第 3 条）；全部干净 → 直接移除。
    /// 无可关目标时静默 no-op。同一时刻只保留一条确认条。
    fn begin_batch_close(&mut self, scope: BatchCloseScope) {
        // 菜单项无论走向如何都算「已选中」，右键菜单随之收起
        self.tab_context_menu = None;
        if self.busy {
            return;
        }
        let targets = batch_close_targets(&self.tabs, scope);
        if targets.is_empty() {
            return;
        }
        if targets.iter().any(|&i| self.tabs[i].dirty) {
            self.close_tab_confirm = None;
            self.batch_close_confirm = Some(targets);
        } else if self.close_tabs_now(&targets) > 0 {
            self.cancel_find_scan();
        }
    }

    /// 打开文件应落入的标签下标：当前页「未命名且干净且为空」→
    /// 就地打开；否则新开一页。
    fn target_tab_for_open(&self) -> usize {
        let t = self.tab();
        let fresh = t.path.is_none() && !t.dirty && t.editor.borrow().doc.is_empty();
        if fresh {
            self.active_tab
        } else {
            self.tabs.len()
        }
    }

    fn new() -> (Self, Task<Message>) {
        // P33：先把 Family::Monospace 的解析目标钉到 CJK 等宽候选（幂等、
        // 进程内一次）——必须发生在首帧排版之前，否则排版缓存里已固化的
        // 逐字回退不会重排
        apply_default_cjk_mono_pin();
        let settings = editpad_core::Settings::load();
        // P34：枚举系统字体清单（P33 钉字之后，同一 fontdb 全局），并解析
        // 配置的字体——未安装时只回退本次渲染并提示一次，**不抹掉配置**
        // （重装字体后自动恢复用户意图）。
        let available_fonts = enumerate_available_families();
        let (active_font_family, configured_font_missing) =
            resolve_startup_font(settings.font_family.as_deref(), &available_fonts);
        // P29：快照总开关关闭时清空存量快照区——只关开关不清数据等于没关
        // （对齐 P20「记住最近文件」先例）
        if !settings.enable_snapshots {
            if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                editpad_core::snapshot::clear_session(&dir);
            }
        }
        let dark_mode = settings.is_dark();
        // 设置里的字号可能未归一（旧配置/手改），boot 时按同一规则 clamp
        let font_size = editor::normalize_font_size(settings.font_size);
        let mut state = Self {
            settings,
            dark_mode,
            available_fonts,
            active_font_family: active_font_family.map(leak_font_family),
            ..Self::default()
        };
        state.cur().borrow_mut().set_font_size(font_size);
        if configured_font_missing {
            if let Some(name) = state.settings.font_family.as_deref() {
                state.status = format!("配置的字体「{name}」未安装，本次启动回退默认等宽");
            }
        }
        // P18 打磨：启动光标闪烁心跳链（自我续期，占用一个睡眠节拍）
        let caret_chain = Task::perform(
            async {
                std::thread::sleep(std::time::Duration::from_millis(
                    editor::CARET_BLINK_MS,
                ));
            },
            |_| Message::CaretTick,
        );
        // P31 周期快照心跳链：同一自我续期模式——运行中每隔
        // snapshot_interval_secs 巡检置脏页增量写快照（崩溃至多丢一个
        // 间隔的输入）；随窗口关闭/进程退出自然销毁。
        let heartbeat_chain = Task::perform(
            async move {
                std::thread::sleep(std::time::Duration::from_secs(u64::from(
                    state.settings.snapshot_interval_secs,
                )));
            },
            |_| Message::SnapshotHeartbeatTick,
        );
        // P30：启动会话恢复——读清单重建标签；命名干净页经加载管线回填。
        // 开关判定在 boot_restore 内部（关闭 = 空白启动 + 存量清场）。
        let restore_task = state.boot_restore();
        (
            state,
            Task::batch([caret_chain, heartbeat_chain, restore_task]),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // ---------- 编辑器 ----------
            Message::Edit(op) => {
                let changed = self.apply_edit(op);
                let mut tasks: Vec<Task<Message>> = Vec::new();
                if changed {
                    // P10：编辑后不再同步重扫（每键全文扫描会卡 UI），排队后台防抖扫描
                    if self.find_visible {
                        tasks.push(self.schedule_find_scan());
                    }
                    // P18：编辑置脏后排队一次防抖自动保存（inflight 去重）
                    tasks.push(self.maybe_schedule_autosave());
                }
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
            }
            Message::EditorNavChanged => Task::none(), // 视图重建即可刷新状态栏

            // ---------- 剪贴板（P4） ----------
            Message::CopyRequested => {
                let Some(text) = self.cur_handle.borrow().selected_text() else {
                    return Task::none();
                };
                iced::clipboard::write(text)
            }
            Message::CutRequested => {
                let Some(text) = self.cur_handle.borrow().selected_text() else {
                    return Task::none();
                };
                // 先写剪贴板，再走统一编辑入口删除选区（Delete 在有选区时只删选区）。
                // clipboard::write 是泛型 Task<T>，直接以 Message 实例化后 chain。
                let write: Task<Message> = iced::clipboard::write(text);
                write.chain(Task::done(Message::Edit(EditOp::Delete)))
            }
            Message::PasteRequested => {
                // clipboard::read 返回 Task<Option<String>>
                iced::clipboard::read()
                    .map(|content| Message::Pasted(content.unwrap_or_default()))
            }
            Message::Pasted(text) => {
                if text.is_empty() {
                    Task::none()
                } else {
                    self.update(Message::Edit(EditOp::InsertText(text)))
                }
            }

            // ---------- 打开 ----------
            Message::OpenRequested => {
                if self.busy {
                    return Task::none();
                }
                self.enter_busy();
                self.status.clear();
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("所有文件", &["*"])
                            .pick_file()
                            .await
                    },
                    |handle| Message::FileChosen(handle.map(|f| f.path().to_path_buf())),
                )
            }
            Message::FileChosen(None) => {
                self.busy = false;
                Task::none()
            }
            Message::FileChosen(Some(path)) => {
                // 对话框阶段结束：busy 若不清零，会撞上后续守卫导致卡死
                self.busy = false;
                self.request_open(path)
            }
            // 拖拽文件进窗口 = 打开；同一路径重复拖拽也允许重新加载
            // （每次 job_seq 递增，无路径去重；dirty/busy 保护在 request_open 内）
            Message::FileDropped(path) => self.request_open(path),

            Message::LoadProgress(job_id, bytes_read, total_bytes) => {
                if self.active_load.as_ref().is_some_and(|j| j.id == job_id) {
                    self.progress = Some((bytes_read, total_bytes));
                }
                Task::none()
            }
            Message::Loaded(job_id, result) => {
                // 过期任务的迟到消息直接丢弃
                let Some(job) = self.active_load.clone() else {
                    return Task::none();
                };
                if job.id != job_id {
                    return Task::none();
                }
                self.active_load = None;
                self.progress = None;
                // P21：结果路由回发起加载的标签页——期间切走也不串页
                let target = job.tab;
                // P30：恢复任务的待还原视图随任务号取出；None = 普通打开
                let pending_view = self.restore_views.remove(&job_id);
                let is_restore = pending_view.is_some();
                let mut tasks: Vec<Task<Message>> = Vec::new();
                match result {
                    // P30 防串写护栏（第二半在 Ok(_) 分支）：普通打开照旧；
                    // 恢复任务要求目标仍是空净无名占位页，否则走丢弃分支
                    Ok((doc, sample, encoding))
                        if !is_restore || self.restore_placeholder_ready(target) =>
                    {
                        // P22：语言解析下沉 core——扩展名别名层 + 无扩展名
                        // 内容嗅探（shebang/XML/JSON/YAML/约定文件名）
                        let language =
                            editpad_core::resolve_language(Some(job.path.as_path()), &sample);
                        let tab = &mut self.tabs[target];
                        {
                            let mut ed = tab.editor.borrow_mut();
                            // P19：rope 直入，不再有 from_str 的二次全文拷贝
                            ed.reset_document(doc);
                            ed.set_language_by_name(language.as_deref());
                            // P30：恢复任务的视图回填——光标与滚动回到
                            // 上次退出时的位置（无副作用定位入口）
                            if let Some((line, col, scroll_top, scroll_left)) = pending_view {
                                ed.restore_view(line, col, scroll_top, scroll_left);
                            }
                            // P32：普通打开命中最近文件记忆 → 光标/滚动就位。
                            // 恢复任务以会话清单视图为准（上方已应用），不覆盖。
                            if !is_restore {
                                if let Some(view) = self.settings.recent_view(&job.path) {
                                    ed.restore_view(view.line, view.col, view.scroll_top, 0.0);
                                }
                            }
                        }
                        tab.path = Some(job.path.clone());
                        tab.encoding_label = encoding;
                        tab.dirty = false;
                        // P25：真实文件已就位，未命名序号使命完成
                        tab.untitled_num = None;
                        // P31：页内容整体换血（打开/恢复回填）——已提交清单
                        // 对本页的描述过期，下一拍重写
                        self.session_manifest_stale = true;
                        if let Some(path) = self.path_of_tab(target) {
                            self.record_recent(&path);
                        }
                        self.busy = false;
                        // 新文档已就位：旧文档的关闭确认语义过期
                        self.confirm_visible = false;
                        self.pending_close = false;
                        self.status.clear();
                        if self.find_visible && target == self.active_tab {
                            tasks.push(self.schedule_find_scan());
                        }
                    }
                    Ok(_) => {
                        // P30：占位页已被用户动过（关页/新页导致下标漂移）——
                        // 宁可丢弃结果也不能覆盖用户内容；计入失败汇总
                        self.busy = false;
                        self.restore_failed += 1;
                    }
                    Err(error) => {
                        if is_restore {
                            // P30：恢复页加载失败（文件被删等）——移除占位页
                            // 继续恢复其余页，不阻断（§3 P30 第 6 条）
                            self.drop_restore_placeholder(target);
                            self.restore_failed += 1;
                            self.busy = false;
                        } else {
                            self.busy = false;
                            self.status = format!("打开失败:{error}");
                        }
                    }
                }
                // P30 恢复链推进：本步收尾后队列非空则续排下一页。
                // 注意这里必须**同步调用**而非塞进 Task 延后——方法调用
                // 本身就完成「弹出下一页 + 登记任务」的全部状态变更，
                // 加载流由 subscription 依据 active_load 重建自然接管；
                // 返回的 Task 恒为 none，无需借道批处理。
                if is_restore && self.settle_restore_step() {
                    let _ = self.begin_restore_load();
                }
                if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                }
            }

            // ---------- 保存 ----------
            Message::SaveRequested => match self.tab().path.clone() {
                Some(_) => self.save(),
                None => self.save_as_dialog(),
            },
            Message::SaveAsRequested => self.save_as_dialog(),
            Message::SaveTargetChosen(None) => {
                self.busy = false;
                // 另存为被取消：若它属于「保存并关闭」，一并放弃关闭
                self.pending_close = false;
                Task::none()
            }
            Message::SaveTargetChosen(Some(path)) => {
                let tab = self.tab_mut();
                tab.path = Some(path);
                // P25：另存为转正后未命名序号使命完成
                tab.untitled_num = None;
                // 对话框阶段结束再交给 save() 的 busy 守卫（原实现在此卡死 busy）
                self.busy = false;
                self.save()
            }
            Message::Saved(version, Ok(())) => {
                // P18 版本守卫：保存期间又有编辑则保持置脏，防止丢改动标记
                self.tab_mut().dirty = self.tab().version != version;
                self.busy = false;
                if !self.tab().dirty {
                    // P38：落盘成功且期间无新编辑——当前内容即磁盘内容，
                    // 刷新撤销回基线的判定基准（版本不符时不得动基线：
                    // 那时磁盘上是旧快照）
                    self.cur_handle.borrow_mut().mark_saved();
                    // P31：内存态比已提交清单「更干净」——下一拍重写清单，
                    // 防崩溃恢复把已落盘内容按旧快照复活成置脏页
                    self.session_manifest_stale = true;
                }
                if let Some(path) = self.tab().path.clone() {
                    self.record_recent(&path);
                }
                // P6 编码知情权：发生转码/BOM 丢失时明确告知，而不是静默落盘
                if let Some(notice) = transcode_notice(&self.tab().encoding_label) {
                    self.status = notice;
                } else {
                    self.status.clear();
                }
                // 落盘后文件已是纯 UTF-8，标签同步归一（避免后续保存重复提示）
                self.tab_mut().encoding_label = "UTF-8".to_owned();
                if self.pending_close {
                    // 落盘确认后才真正关窗。P29：保存的是活动页，
                    // 其余置脏页走快照直退（不再二次弹窗），快照失败才降级
                    self.pending_close = false;
                    if session_restore_allowed(
                        self.settings.enable_snapshots,
                        self.settings.remember_session,
                    ) && self.settings.exit_mode == editpad_core::EXIT_MODE_SNAPSHOT
                    {
                        if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                            return self.exit_via_snapshot(&dir);
                        }
                    }
                    return self.close_window();
                }
                Task::none()
            }
            Message::Saved(_, Err(error)) => {
                self.busy = false;
                // 保存失败不关窗：留在应用里让用户处理
                self.pending_close = false;
                self.status = format!("保存失败:{error}");
                Task::none()
            }

            // ---------- 即时保存（P18，按页路由） ----------
            Message::TabAutosaved(idx, version, result) => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.autosave_inflight = false;
                    match result {
                        Ok(()) => {
                            // 版本一致 = 快照之后没有新编辑：可以安全清脏
                            if tab.version == version {
                                tab.dirty = false;
                                // P38：当前内容即磁盘内容，刷新落盘基线
                                tab.editor.borrow_mut().mark_saved();
                                // P31：auto-save 成功清脏 = 内存比清单干净，
                                // 下一拍重写清单（§3 P31 第 3 条的顺带刷新）
                                self.session_manifest_stale = true;
                            }
                        }
                        Err(error) => {
                            // 失败必须留痕（不能无声吞掉），但不打断编辑；
                            // 清掉 inflight 后，下一次编辑会重新排队
                            self.status = format!("自动保存失败:{error}");
                        }
                    }
                }
                Task::none()
            }

            // ---------- 关闭确认 ----------
            Message::CloseRequested(id) => {
                self.handle_close_request(id, editpad_core::snapshot::snapshot_dir())
            }
            Message::ConfirmSaveAndClose => {
                self.confirm_visible = false;
                self.pending_close = true;
                match self.tab().path.clone() {
                    Some(_) => self.save(),
                    // 未命名文档：先走另存为，落盘成功后自动关窗
                    None => self.save_as_dialog(),
                }
            }
            Message::DiscardAndClose => {
                self.discard_all_and_close(editpad_core::snapshot::snapshot_dir())
            }
            Message::CancelClose => {
                self.confirm_visible = false;
                self.pending_close = false;
                Task::none()
            }

            // ---------- 启动会话恢复（P30） ----------
            Message::SessionRecoverAccepted => {
                self.accept_session_recover(editpad_core::snapshot::snapshot_dir())
            }
            Message::SessionRecoverDiscarded => {
                self.discard_session_recover(editpad_core::snapshot::snapshot_dir())
            }

            // ---------- 打开确认 ----------
            Message::ConfirmOpenDiscard => {
                let Some(path) = self.open_confirm.take() else {
                    return Task::none();
                };
                // 明确放弃：不再触发下一次确认；若关闭确认条还开着，其前提已消失
                self.tab_mut().dirty = false;
                self.confirm_visible = false;
                self.pending_close = false;
                let tab = self.target_tab_for_open();
                self.start_loading(path, tab)
            }
            Message::ConfirmOpenCancel => {
                self.open_confirm = None;
                Task::none()
            }

            // ---------- 查找 / 替换 ----------
            Message::FindToggled => {
                self.find_visible = !self.find_visible;
                if self.find_visible {
                    self.goto_visible = false;
                    return self.schedule_find_scan();
                } else {
                    // 关栏即取消在途扫描并清结果（旧实现只清结果）
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::FindQueryChanged(query) => {
                self.find_query = query;
                // P10：查询变化只排队后台扫描（防抖），UI 线程零全文拷贝；
                // 查询为空时内部转为取消 + 清结果
                self.schedule_find_scan()
            }
            Message::FindNext => self.step_match(true),
            Message::FindPrev => self.step_match(false),
            Message::CaseToggled(value) => {
                self.case_sensitive = value;
                self.schedule_find_scan()
            }
            Message::ReplaceQueryChanged(query) => {
                self.replace_query = query;
                Task::none()
            }
            Message::ReplaceCurrent => self.replace_current(),
            Message::ReplaceAll => {
                if self.busy || self.find_query.is_empty() || self.find_scanning() {
                    // 扫描在途时禁止全部替换：此刻的全文快照可能是过期的
                    return Task::none();
                }
                // P11：直接在 rope 上流式替换，省掉 to_text() 全文拷贝
                // P22 补充：查询与替换文本先做转义解析（\n \r \t \\）
                let (new_contents, count) = {
                    let editor = self.cur_handle.borrow();
                    editpad_core::replace_all_document(
                        &editor.doc,
                        &unescape_query(&self.find_query),
                        &unescape_query(&self.replace_query),
                        self.case_sensitive,
                    )
                };
                if count > 0 {
                    self.cur()
                        .borrow_mut()
                        .replace_whole_document(editpad_core::Document::from_str(&new_contents));
                    self.tab_mut().dirty = true;
                    // P18：内容版本与防抖起点同步推进
                    self.tab_mut().note_mutation();
                }
                // P10：替换后的重扫走后台防抖，不再同步刷
                let mut tasks = vec![self.schedule_find_scan()];
                if count > 0 {
                    // P18：内容变了 → 排队一次防抖自动保存
                    tasks.push(self.maybe_schedule_autosave());
                }
                self.status = format!("已替换 {count} 处");
                return Task::batch(tasks);
            }
            Message::FindScanDone(seq, found) => {
                // 过期结果丢弃：只认当前排队中的那次扫描（P10 的 job 序号过滤，
                // 与 Loaded 按 job_id 过滤同构）
                if self.find_scan == Some(seq) {
                    self.find_scan = None;
                    self.matches = found;
                    self.match_idx = None;
                }
                Task::none()
            }

            // ---------- 高亮后台分批补建（P12） ----------
            Message::HighlightPaveNeeded => self.schedule_highlight_pave(),
            Message::FormatJson => {
                const FORMAT_JSON_MAX_CHARS: usize = 4_000_000;
                // 仅当前语法为 JSON 时生效（P22 第二批：按当前语法判断）
                if self.cur_handle.borrow().highlight_syntax_name().as_deref() != Some("JSON") {
                    self.status = "格式化 JSON 仅对 JSON 文件可用（Ctrl+Shift+F）".to_owned();
                    return Task::none();
                }
                let (text, chars) = {
                    let ed = self.cur_handle.borrow();
                    (ed.doc.to_text(), ed.doc.text_len())
                };
                if chars > FORMAT_JSON_MAX_CHARS {
                    // 单遍重排是同步操作，超大文件会冻结 UI——先挡下并提示
                    self.status =
                        format!("文档过大（{chars} 字符），暂不支持格式化（上限 {FORMAT_JSON_MAX_CHARS}）");
                    return Task::none();
                }
                match editpad_core::format_json(&text) {
                    Ok(pretty) => {
                        // replace_whole_document 内部快照 → 可撤销；光标复位到文首
                        self.cur()
                            .borrow_mut()
                            .replace_whole_document(editpad_core::Document::from_str(&pretty));
                        self.tab_mut().dirty = true;
                        // P18：内容版本与防抖起点同步推进
                        self.tab_mut().note_mutation();
                        self.status = "已格式化 JSON".to_owned();
                        if self.find_visible {
                            // 内容变了：命中表过期，走后台防抖重扫（P10 同款）
                            let find_task = self.schedule_find_scan();
                            return Task::batch([
                                find_task,
                                self.maybe_schedule_autosave(),
                            ]);
                        }
                        return self.maybe_schedule_autosave();
                    }
                    Err(error) => {
                        self.status = format!("JSON 格式化失败：{error}");
                        Task::none()
                    }
                }
            }
            Message::HlPaveProgress(gen, strides_done) => {
                // 双重代次检查：任务登记一致且高亮器未换代（换文件后
                // 旧任务的迟到进度不得污染新会话的状态栏）
                if self.hl_paving == Some(gen)
                    && self.cur_handle.borrow().highlight_generation() == Some(gen)
                {
                    let total_strides = (self.cur_handle.borrow().doc.line_count()
                        / editpad_core::highlight::STRIDE)
                        .max(1);
                    let pct = (strides_done as usize).min(total_strides) * 100 / total_strides;
                    self.status = format!("语法分析中…{pct}%（后台）");
                }
                Task::none()
            }
            Message::HlPaved(gen, paved) => {
                if self.hl_paving == Some(gen) {
                    self.hl_paving = None;
                    // 代次一致才安装；期间编辑过则整体丢弃——缺口由下一帧
                    // needs_paving 重新评估并续排（从存活检查点出发，代价小）
                    let installed = self
                        .cur_handle
                        .borrow_mut()
                        .install_highlighter_if_current(gen, paved);
                    let _ = installed;
                    if self.status.starts_with("语法分析") {
                        self.status.clear();
                    }
                }
                Task::none()
            }

            // ---------- 跳转 ----------
            Message::GotoToggled => {
                self.goto_visible = !self.goto_visible;
                if self.goto_visible {
                    self.find_visible = false;
                }
                Task::none()
            }
            Message::GotoInputChanged(value) => {
                self.goto_input = value;
                Task::none()
            }
            Message::GotoSubmit => match self.goto_input.trim().parse::<usize>() {
                Ok(n) if n >= 1 => {
                    self.cur_handle.borrow_mut().jump_to_line(n);
                    self.goto_visible = false;
                    self.status.clear();
                    Task::none()
                }
                _ => {
                    self.status = "请输入有效行号（从 1 开始）".to_owned();
                    Task::none()
                }
            },

            // ---------- 最近文件 ----------
            Message::RecentsToggled => {
                self.recents_visible = !self.recents_visible;
                Task::none()
            }
            Message::RecentSelected(entry) => self.request_open(PathBuf::from(entry)),
            Message::RecentsCleared => {
                // P20 隐私：立即写回空列表，config.toml 不再含历史路径
                self.settings.clear_recent_files();
                self.persist_settings();
                self.status = "已清空最近文件记录".to_owned();
                Task::none()
            }

            Message::BarsDismissed => {
                self.find_visible = false;
                self.goto_visible = false;
                self.recents_visible = false;
                // P27：Esc 一并关闭设置弹窗
                self.settings_visible = false;
                // Esc 同时视作放弃关闭/打开确认
                self.confirm_visible = false;
                self.pending_close = false;
                self.open_confirm = None;
                // P21：Esc 也取消标签页关闭确认
                self.close_tab_confirm = None;
                // P28：Esc 同时收起右键菜单与批量关闭确认
                self.tab_context_menu = None;
                self.batch_close_confirm = None;
                // P10：取消在途扫描 + 清结果（含序号失效）
                self.cancel_find_scan();
                Task::none()
            }

            // ---------- 多标签（P21） ----------
            Message::NewTab => {
                let tab = self.fresh_tab();
                self.tabs.push(tab);
                let last = self.tabs.len() - 1;
                self.assign_untitled_num(last);
                self.set_active_tab(last);
                // 查找态全局：切页即作废旧命中，防串页
                self.cancel_find_scan();
                // P28：页集合已变，右键菜单随之下收
                self.tab_context_menu = None;
                Task::none()
            }
            Message::SwitchTabNext => {
                let next = (self.active_tab + 1) % self.tabs.len();
                self.set_active_tab(next);
                self.cancel_find_scan();
                self.tab_context_menu = None;
                Task::none()
            }
            Message::SwitchTab(i) => {
                if i < self.tabs.len() && i != self.active_tab {
                    self.set_active_tab(i);
                    self.cancel_find_scan();
                    self.tab_context_menu = None;
                }
                Task::none()
            }
            Message::CloseTabRequest => {
                let idx = self.active_tab;
                // P28：固定页对键盘路径（Ctrl+W）同样豁免，与右键菜单一致
                if self.tabs[idx].pinned {
                    self.status = "固定标签页需先取消固定再关闭".to_owned();
                } else if self.tabs[idx].dirty {
                    // 置脏页先确认（骨架版仅提供「放弃更改」出口）
                    self.close_tab_confirm = Some(idx);
                } else if self.close_tab_now(idx) {
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::ConfirmCloseTabDiscard(idx) => {
                self.close_tab_confirm = None;
                if idx < self.tabs.len() {
                    // 关最后一页时槽位会被复用：先清空内容与路径
                    let tab = &mut self.tabs[idx];
                    tab.dirty = false;
                    tab.path = None;
                    tab.editor
                        .borrow_mut()
                        .reset_document(editpad_core::Document::new());
                }
                if self.close_tab_now(idx) {
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::CancelCloseTab => {
                self.close_tab_confirm = None;
                Task::none()
            }
            Message::CaretTick => {
                // 打磨项：翻转闪烁相位（update 本身会触发重绘）。
                // 心跳链在 new() 启动后自我续期，占用一个常驻睡眠节拍。
                self.cur_handle.borrow_mut().tick_blink();
                Task::perform(
                    async {
                        std::thread::sleep(std::time::Duration::from_millis(
                            editor::CARET_BLINK_MS,
                        ));
                    },
                    |_| Message::CaretTick,
                )
            }
            Message::SnapshotHeartbeatTick => {
                // P31 自我续期：无论本轮是否干活，下一拍恒排队（与光标
                // 闪烁同一模式；进程退出即销毁，无残留计时器）。间隔取
                // 加载时已归一的设置值，运行期视为不变。
                let interval = std::time::Duration::from_secs(u64::from(
                    self.settings.snapshot_interval_secs,
                ));
                let rearm = Task::perform(
                    async move { std::thread::sleep(interval) },
                    |_| Message::SnapshotHeartbeatTick,
                );
                // 快照底座任一开关关闭 / ask 模式 = 心跳整体停摆：
                // 清单不写，退出流与启动恢复同样不依赖它（P29/P30 语义）
                if !session_restore_allowed(
                    self.settings.enable_snapshots,
                    self.settings.remember_session,
                ) || self.settings.exit_mode != editpad_core::EXIT_MODE_SNAPSHOT
                {
                    return rearm;
                }
                // 至多一个提交在途：本轮巡检跳过（页级账目不受影响）
                if self.heartbeat_inflight {
                    return rearm;
                }
                // 目录解析走注入点（生产为 None → 系统配置目录）
                let dir = self
                    .snapshot_dir_override
                    .clone()
                    .or_else(editpad_core::snapshot::snapshot_dir);
                let Some(dir) = dir else {
                    return rearm;
                };
                match self.prepare_heartbeat_commit(&dir) {
                    // 无变化且清单不过期：什么都不写（防无谓 IO）
                    None => rearm,
                    Some(payload) => {
                        self.heartbeat_inflight = true;
                        Task::batch([
                            rearm,
                            Task::perform(
                                async move { drive_heartbeat(payload).await },
                                |message| message,
                            ),
                        ])
                    }
                }
            }
            Message::HeartbeatDone(outcome) => {
                self.heartbeat_apply(outcome);
                Task::none()
            }
            Message::PreviewToggled => {
                // 仅 Markdown 语法页可开预览（按钮本身已禁用，此处双保险）
                if self.cur_handle.borrow().highlight_syntax_name().as_deref()
                    == Some("Markdown")
                {
                    self.preview_visible = !self.preview_visible;
                } else {
                    self.status = "预览仅支持 Markdown 文件".to_owned();
                }
                Task::none()
            }
            Message::CloseTabSave(idx) => {
                // 「保存并关闭」：已命名的置脏页先落盘，
                // TabSaved 成功且清脏后再真正移除页面
                if idx >= self.tabs.len() || self.busy {
                    return Task::none();
                }
                if self.tabs[idx].path.is_none() {
                    self.status = "未命名标签页请先另存为再关闭".to_owned();
                    return Task::none();
                }
                self.enter_busy();
                let path = self.tabs[idx].path.clone().expect("上方已确认非空");
                let doc = self.tabs[idx].editor.borrow().doc.clone();
                let version = self.tabs[idx].version;
                self.pending_close_tab = Some(idx);
                Task::perform(
                    async move {
                        let saved = editpad_core::save_document_atomic(&path, &doc)
                            .map_err(|e| e.to_string());
                        (version, saved)
                    },
                    move |(version, result)| Message::TabSaved(idx, version, result),
                )
            }
            Message::TabSaved(idx, version, result) => {
                self.busy = false;
                if idx >= self.tabs.len() {
                    return Task::none();
                }
                match result {
                    Ok(()) => {
                        // 版本守卫同款：期间又有编辑则保持置脏、不关闭
                        let clean = self.tabs[idx].version == version;
                        if clean {
                            self.tabs[idx].dirty = false;
                            // P38：落盘成功且版本守卫通过——内容即磁盘内容。
                            // 本页通常随即被移除，此处是 close_tab_now 失败
                            // 等幸存路径的基线兜底
                            self.tabs[idx].editor.borrow_mut().mark_saved();
                            if self.pending_close_tab == Some(idx)
                                && self.close_tab_now(idx)
                            {
                                self.cancel_find_scan();
                            }
                            self.pending_close_tab = None;
                        } else {
                            self.status = "保存后又有新改动，已取消自动关闭".to_owned();
                        }
                    }
                    Err(error) => {
                        self.status = format!("保存失败:{error}");
                        self.pending_close_tab = None;
                    }
                }
                Task::none()
            }

            // ---------- 标签右键菜单（P28；P39 起为浮层） ----------
            Message::TabContextMenu(i) => {
                // busy（对话框/IO 中）不开菜单；越界下标（页刚被关掉）忽略
                if !self.busy && i < self.tabs.len() {
                    // P39：浮层锚点 = 打开那一刻的指针位置（标签条 mouse_area 跟踪）
                    self.menu_anchor = self.cursor_pos;
                    self.tab_context_menu = Some(i);
                }
                Task::none()
            }
            // ---------- 浮层弹窗定位（P39/P40） ----------
            Message::CursorMoved(p) => {
                self.cursor_pos = (p.x, p.y);
                Task::none()
            }
            Message::ViewportResized(w, h) => {
                self.viewport_size = (w, h);
                Task::none()
            }
            Message::TabContextMenuClosed => {
                self.tab_context_menu = None;
                Task::none()
            }
            Message::TogglePinTab(i) => {
                self.tab_context_menu = None;
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.pinned = !tab.pinned;
                }
                Task::none()
            }
            Message::SaveTabFromMenu(i) => {
                self.tab_context_menu = None;
                // v1 决策：右键保存 = 先切到目标页再走既有活动页保存流——
                // Saved 回报、最近文件记录、转码提示全部复用活动页语义，
                // 不为后台页另铺一条带 idx 的回报管线。未命名置脏页自动
                // 落另存为对话框（与 Ctrl+S 同语义）。
                if !self.busy && i < self.tabs.len() && self.tabs[i].dirty {
                    self.set_active_tab(i);
                    return match self.tab().path.clone() {
                        Some(_) => self.save(),
                        None => self.save_as_dialog(),
                    };
                }
                Task::none()
            }
            Message::RenameOrSaveAsTab(i) => {
                self.tab_context_menu = None;
                // §3 P28 第 2 条：重命名 v1 用「另存为」兜底；未命名页同款。
                if !self.busy && i < self.tabs.len() {
                    self.set_active_tab(i);
                    return self.save_as_dialog();
                }
                Task::none()
            }
            Message::CloseTabAt(idx) => {
                self.tab_context_menu = None;
                if !self.busy && idx < self.tabs.len() {
                    if self.tabs[idx].pinned {
                        self.status = "固定标签页需先取消固定再关闭".to_owned();
                    } else if self.tabs[idx].dirty {
                        // 置脏走既有单页确认条（含「保存并关闭」出口）
                        self.batch_close_confirm = None;
                        self.close_tab_confirm = Some(idx);
                    } else if self.close_tab_now(idx) {
                        self.cancel_find_scan();
                    }
                }
                Task::none()
            }
            Message::CloseOtherTabs(keep) => {
                self.begin_batch_close(BatchCloseScope::Others(keep));
                Task::none()
            }
            Message::CloseTabsRight(from) => {
                self.begin_batch_close(BatchCloseScope::RightOf(from));
                Task::none()
            }
            Message::ConfirmBatchCloseDiscard => {
                if let Some(targets) = self.batch_close_confirm.take() {
                    // 统一放弃：先清各页置脏与内容（与 ConfirmCloseTabDiscard
                    // 同款，防「已移除页的 rope 仍被别名引用」的错觉），再移除。
                    for &idx in &targets {
                        if let Some(tab) = self.tabs.get_mut(idx) {
                            tab.dirty = false;
                            tab.path = None;
                            tab.editor
                                .borrow_mut()
                                .reset_document(editpad_core::Document::new());
                        }
                    }
                    if self.close_tabs_now(&targets) > 0 {
                        self.cancel_find_scan();
                    }
                }
                Task::none()
            }
            Message::CancelBatchCloseTabs => {
                self.batch_close_confirm = None;
                Task::none()
            }

            // ---------- 外观 ----------
            Message::ThemeToggled => {
                self.dark_mode = !self.dark_mode;
                self.settings.set_theme(self.dark_mode);
                self.persist_settings();
                Task::none()
            }
            Message::FontSizeDelta(delta) => {
                let next = editor::normalize_font_size(self.display_font_size() + delta);
                self.settings.font_size = next;
                self.persist_settings();
                self.cur_handle.borrow_mut().set_font_size(next);
                Task::none()
            }

            // ---------- 设置弹窗（P27） ----------
            Message::SettingsToggled => {
                // busy（加载/保存中）禁开，与工具栏其余按钮同一守卫语义
                if !self.busy {
                    self.settings_visible = !self.settings_visible;
                }
                Task::none()
            }
            Message::SettingsAutosaveToggled(value) => {
                self.settings.autosave_enabled = value;
                self.persist_settings();
                Task::none()
            }
            Message::SettingsAutosaveDelayDelta(delta) => {
                let next = (self.settings.autosave_delay_secs as i64 + delta as i64)
                    .clamp(
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
                let next = (self.settings.snapshot_interval_secs as i64 + delta as i64)
                    .clamp(
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
        }
    }

    // ---------- 编辑分发 ----------

    /// 执行一次按键编辑；返回是否真的改动了文本。
    fn apply_edit(&mut self, op: EditOp) -> bool {
        // 加载进行中不接收编辑，避免打到即将被替换的旧文档上
        if self.active_load.is_some() {
            return false;
        }

        use EditOp as E;
        let mut hint: Option<&'static str> = None;
        // P38：撤销/重做后内容是否恰好回到落盘基线（打字/删除路径不查询，
        // 维持保守置脏，避免大文档每键全量比对）
        let mut back_to_saved = false;

        let mut editor = self.cur_handle.borrow_mut();
        let changed = match op {
            E::Motion(motion, extend) => {
                editor.apply_motion(motion, extend);
                false
            }
            E::SelectAll => {
                editor.select_all();
                false
            }
            E::Undo => {
                let changed = editor.undo();
                if !changed {
                    hint = Some("没有更多撤销历史");
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::Redo => {
                let changed = editor.redo();
                if !changed {
                    hint = Some("已在最新状态");
                } else {
                    back_to_saved = editor.is_at_saved_content();
                }
                changed
            }
            E::InsertText(text) => {
                editor.insert_str(&text);
                true
            }
            E::Backspace => {
                editor.backspace();
                true
            }
            E::Delete => {
                editor.delete_forward();
                true
            }
        };
        drop(editor);

        if changed {
            {
                let tab = self.tab_mut();
                // P38：撤销/重做按「内容是否回到最近落盘版本」重算置脏——
                // 退净到基线即与磁盘一致，● 消失、关窗不再无谓拦截；
                // 其余编辑路径 back_to_saved 恒 false，行为不变
                tab.dirty = !back_to_saved;
                // P18：内容版本 +1 并刷新防抖起点（自动保存的触发依据）
                tab.note_mutation();
            }
            if back_to_saved {
                // P31：内存态变得比已提交清单更干净（清单还记着置脏页），
                // 下一拍心跳重写清单，防崩溃恢复把已回清的内容按旧快照复活
                self.session_manifest_stale = true;
            }
            self.status.clear();
        } else if let Some(hint) = hint {
            self.status = hint.to_owned();
        }
        changed
    }

    // ---------- 加载管线 ----------

    /// 启动一次后台加载：登记任务后由 [`Editpad::subscription`] 的流接管。
    ///
    /// P21：`tab` 指明结果应落入的标签页（可能等于 `tabs.len()`，
    /// 表示「新开一页」——此处先占位创建，保证路由目标恒存在）；
    /// 期间用户切走标签也不影响结果归页。
    fn start_loading(&mut self, path: PathBuf, tab: usize) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        // P21 内存护栏（§3 P19 总则第 2 条的骨架实现）：全部页字符量
        // 按 3 字节/字符保守估算，加上待载文件大小，超上限即拒开并提示
        let incoming = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let existing: usize = self
            .tabs
            .iter()
            .map(|t| t.editor.borrow().doc.text_len())
            .sum();
        if !mem_guard_allows(existing, incoming, MULTI_TAB_MEM_CAP_BYTES) {
            self.status =
                "内存保护：合计内容超过上限，请先关闭部分大文档再打开".to_owned();
            return Task::none();
        }
        if tab >= self.tabs.len() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
        }
        self.register_load_job(path, tab);
        // P21：加载落在新页时直接切过去（符合「打开即聚焦」直觉）。
        // ⚠️ 必须经 set_active_tab 同步 cur_handle 长期别名——直接赋值
        // active_tab 会造成「下标指向新页、别名仍指旧页」的失步：
        // 输入与渲染都走别名，表现为打开文件后敲字打进上一个文档
        // （P32 本轮发现并修复；既有测试均在断言前显式切换而未暴露）。
        self.set_active_tab(tab);
        self.status.clear();
        Task::none()
    }

    /// 登记一个后台加载任务（任务号分配 + busy 置位 + 进度条复位）。
    /// 内存守卫、占位页创建与焦点切换由调用方决定：用户打开要切过去
    /// （[`Self::start_loading`]），恢复链绝不抢焦点（P30）。
    fn register_load_job(&mut self, path: PathBuf, tab: usize) -> u64 {
        self.job_seq += 1;
        let id = self.job_seq;
        self.active_load = Some(LoadJob { id, path, tab });
        self.progress = Some((0, 0));
        self.enter_busy();
        id
    }

    /// 统一的换文档入口（打开对话框/拖拽/最近文件共用）：
    /// 当前页 dirty 时绝不静默丢弃修改（含撤销链），先弹打开确认条。
    /// P21：落点 = [`Self::target_tab_for_open`]（空净当前页就地打开，否则新页）。
    fn request_open(&mut self, path: PathBuf) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        if self.tab().dirty {
            self.open_confirm = Some(path);
            return Task::none();
        }
        let tab = self.target_tab_for_open();
        self.start_loading(path, tab)
    }

    fn subscription(&self) -> Subscription<Message> {
        let load = match &self.active_load {
            Some(job) => Subscription::run_with(job.clone(), build_load_stream),
            None => Subscription::none(),
        };
        // P10 的查找扫描走 Task::perform（见 schedule_find_scan），不经订阅
        // 0.14 没有 keyboard::on_key_press 了，用 listen_with 手动过滤按键；
        // 同一条流顺带捕获拖拽文件（FileDropped；FileHovered 忽略）
        let events =
            iced::event::listen_with(|event, status, _window| match (event, status) {
                (
                    iced::Event::Keyboard(keyboard::Event::KeyPressed {
                        key, modifiers, ..
                    }),
                    iced::event::Status::Ignored,
                ) => handle_key(key, modifiers),
                (
                    iced::Event::Window(window::Event::FileDropped(path)),
                    _,
                ) => Some(Message::FileDropped(path)),
                // P39/P40：窗口逻辑尺寸（浮层贴边钳制依据；iced_winit 已
                // 换算成逻辑坐标，与 mouse_area 光标坐标同空间）
                (iced::Event::Window(window::Event::Resized(size)), _) => {
                    Some(Message::ViewportResized(size.width, size.height))
                }
                _ => None,
            });
        // 窗口关闭请求：exit_on_close_request(false) 后以订阅事件流转
        let close_requests = window::close_requests().map(Message::CloseRequested);
        // P18 即时保存不走订阅：编辑后由 maybe_schedule_autosave 直接派发
        // 「睡眠防抖→落盘」的专用线程（inflight 去重，至多一个挂起）
        Subscription::batch([load, events, close_requests])
    }

    // ---------- 保存 ----------

    fn save_as_dialog(&mut self) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        self.enter_busy();
        let suggested = self.suggested_name();
        Task::perform(
            async move {
                rfd::AsyncFileDialog::new()
                    .set_file_name(&suggested)
                    .save_file()
                    .await
            },
            |handle| Message::SaveTargetChosen(handle.map(|f| f.path().to_path_buf())),
        )
    }

    fn save(&mut self) -> Task<Message> {
        if self.busy || self.tab().path.is_none() {
            return Task::none();
        }
        self.enter_busy();
        let path = self.tab().path.clone().expect("上方已确认非空");
        // P19 行动项 3：rope 结构共享克隆（O(1)），分块原子写盘，
        // 不再经 to_text() 产生全文 String（50MB 场景省 ~50MB 峰值）
        let doc = self.cur_handle.borrow().doc.clone();
        // P18 版本守卫：记录本次落盘对应的内容版本
        let version = self.tab().version;
        Task::perform(
            async move {
                let saved = editpad_core::save_document_atomic(&path, &doc)
                    .map_err(|e| e.to_string());
                (version, saved)
            },
            move |(version, result)| Message::Saved(version, result),
        )
    }

    // ---------- 即时保存（P18，按页独立） ----------

    /// 单个标签页的自动保存是否就绪：已命名、有未存改动、
    /// 无在途 IO、不与手动保存互斥、本页没有挂起任务。
    fn tab_autosave_ready(&self, idx: usize) -> bool {
        self.settings.autosave_enabled
            && self.tabs[idx].dirty
            && self.tabs[idx].path.is_some()
            && self.active_load.is_none()
            && !self.busy
            && !self.tabs[idx].autosave_inflight
    }

    /// 编辑后调用：遍历全部标签页，把所有就绪页各排一个「睡满防抖窗 →
    /// 落盘 → 回报版本」的专用任务（P5/P10 同构的 OS 线程桥接）。
    ///
    /// 至多每页一个挂起（autosave_inflight 去重）；任务醒来落盘的是
    /// **调度时刻**的快照——若期间又有编辑，版本守卫会保持置脏，
    /// 本次编辑结束后由新任务覆盖最新内容（最终一致）。
    fn maybe_schedule_autosave(&mut self) -> Task<Message> {
        if !self.settings.autosave_enabled || self.busy || self.active_load.is_some() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        for idx in 0..self.tabs.len() {
            if !self.tab_autosave_ready(idx) {
                continue;
            }
            let Some(path) = self.tabs[idx].path.clone() else {
                continue;
            };
            let doc = self.tabs[idx].editor.borrow().doc.clone();
            let version = self.tabs[idx].version;
            let delay =
                std::time::Duration::from_secs(u64::from(self.settings.autosave_delay_secs));
            self.tabs[idx].autosave_inflight = true;
            tasks.push(Task::perform(
                async move { drive_autosave_once(idx, path, doc, version, delay).await },
                |message| message,
            ));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn record_recent(&mut self, path: &Path) {
        self.settings.push_recent(path);
        self.persist_settings();
    }

    /// P32：把指定页的当前光标/滚动写进最近文件记忆（未命名页无路径跳过），
    /// 任一页有实际变化才落盘一次 config.toml。
    fn remember_tab_views(&mut self, indices: &[usize]) {
        let mut changed = false;
        for &idx in indices {
            let Some(path) = self.path_of_tab(idx) else {
                continue;
            };
            let ed = self.tabs[idx].editor.borrow();
            let view = editpad_core::RecentView {
                line: ed.cursor.line,
                col: ed.cursor.col,
                scroll_top: ed.scroll_top,
            };
            drop(ed);
            changed |= self.settings.set_recent_view(&path, view);
        }
        if changed {
            self.persist_settings();
        }
    }

    /// 统一设置落盘入口：测试注入 `settings_path_override` 时写到
    /// 临时目录，绝不动真实 %APPDATA%；否则走系统配置目录（尽力而为）。
    fn persist_settings(&self) {
        if let Some(path) = &self.settings_path_override {
            let _ = self.settings.save_to(path);
        } else {
            self.settings.save();
        }
    }

    /// 应用主题（boot 从设置读入，工具栏可切换）。
    fn theme(&self) -> Theme {
        if self.dark_mode {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// 设置里归一后的当前字号（显示与按钮可用性判断都用它）。
    fn display_font_size(&self) -> f32 {
        editor::normalize_font_size(self.settings.font_size)
    }

    /// 正文与 UI 的统一字形族（P34 换装点）：生效族名 Some → 以
    /// `Family::Name` 引用（系统字体已在 fontdb 里，无需装载字节），
    /// None / 未配置 → 默认等宽（P33 的 CJK 钉字仍生效）。
    /// iced 排版缓存按 Font 值做键——运行期换族即换键，无陈旧缓存问题。
    fn body_font(&self) -> Font {
        match self.active_font_family {
            Some(name) => Font {
                family: iced::font::Family::Name(name),
                ..editor::BODY_FONT
            },
            None => editor::BODY_FONT,
        }
    }

    /// 「放弃更改并关闭」：P21 聚合放弃 + P29 连快照一起丢
    /// （§3 P29 第 4 条——不清场的话，下次启动会把已放弃的内容
    /// 当会话恢复回来）。`snapshot_dir` 注入点同 [`Self::handle_close_request`]。
    fn discard_all_and_close(&mut self, snapshot_dir: Option<PathBuf>) -> Task<Message> {
        // P21：放弃关闭 = 全部标签页的未保存标记一并放弃
        for tab in &mut self.tabs {
            tab.dirty = false;
        }
        self.confirm_visible = false;
        if self.settings.enable_snapshots {
            if let Some(dir) = snapshot_dir {
                editpad_core::snapshot::clear_session(&dir);
            }
        }
        self.close_window()
    }

    // ---------- 周期快照心跳（P31） ----------

    /// Tab → 清单页元数据的统一映射。`file` 由调用方决定：
    /// 退出流恒 None（全量重写）；心跳流对「版本未变的置脏页」预填
    /// 旧文件名（core 侧据此复用不重写）。
    fn session_tab_metadata(
        t: &Tab,
        file: Option<String>,
    ) -> editpad_core::snapshot::SessionTab {
        let ed = t.editor.borrow();
        editpad_core::snapshot::SessionTab {
            path: t.path.as_ref().map(|p| p.display().to_string()),
            untitled_num: t.untitled_num,
            dirty: t.dirty,
            file,
            cursor_line: ed.cursor.line,
            cursor_col: ed.cursor.col,
            scroll_top: ed.scroll_top,
            scroll_left: ed.scroll_left,
        }
    }

    /// 心跳选页：返回需要**重写内容**的 (下标, 派发时刻版本) 列表。
    /// 判定核心在 core 的 [`editpad_core::snapshot::heartbeat_page_selected`]：
    /// 置脏 + 未超大小上限（按 rope 真实字节数）+ 版本自上次快照有推进。
    fn heartbeat_plan(&self) -> Vec<(usize, u64)> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let last = t.heartbeat_snap.as_ref().map(|(v, _)| *v);
                editpad_core::snapshot::heartbeat_page_selected(
                    t.dirty,
                    t.editor.borrow().doc.text_len_bytes() as u64,
                    last,
                    t.version,
                )
            })
            .map(|(i, t)| (i, t.version))
            .collect()
    }

    /// 组装全部页的心跳提交输入（逐页意图见 core [`HeartbeatPage`]）：
    /// * 计划内的页 `rewrite=true` → 内容落新文件；
    /// * 计划外但账目显示版本未变的置脏页：预填旧文件名 → 复用不重写；
    /// * 计划外的其余置脏页（超限节流）：不预填 → 只记账不落内容；
    /// * 干净页只记元数据。文档一律 rope 结构共享克隆（O(1)）。
    fn build_heartbeat_pages(
        &self,
        plan: &[(usize, u64)],
    ) -> Vec<editpad_core::snapshot::HeartbeatPage> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(idx, t)| {
                let rewrite = plan.iter().any(|(i, _)| *i == idx);
                let reuse = t
                    .heartbeat_snap
                    .as_ref()
                    .filter(|(v, _)| !rewrite && *v == t.version)
                    .map(|(_, f)| f.clone());
                editpad_core::snapshot::HeartbeatPage {
                    page: editpad_core::snapshot::SessionPage {
                        tab: Self::session_tab_metadata(t, reuse),
                        doc: t.editor.borrow().doc.clone(),
                    },
                    rewrite,
                }
            })
            .collect()
    }

    /// 心跳提交准备：无变化且清单不过期时返回 None（本轮零 IO）。
    fn prepare_heartbeat_commit(&self, dir: &Path) -> Option<HeartbeatPayload> {
        let plan = self.heartbeat_plan();
        if plan.is_empty() && !self.session_manifest_stale {
            return None;
        }
        Some(HeartbeatPayload {
            dir: dir.to_path_buf(),
            pages: self.build_heartbeat_pages(&plan),
            active: self.active_tab,
            next_untitled: self.untitled_next,
            plan,
        })
    }

    /// 心跳回报落地（update 与测试同步入口共用的唯一出口）：
    /// 成功 → 清过期标记，按派发时刻的计划逐页回填账目——
    /// 版本仍一致且仍置脏才记（期间编辑过/转干净/已关闭的页自然跳过，
    /// 其下一拍会重新入选或不再需要）；失败 → 全部账目作废（下一拍
    /// 全量重试），状态栏留痕不无声吞掉（P18 同款取舍）。
    fn heartbeat_apply(&mut self, outcome: HeartbeatOutcome) {
        self.heartbeat_inflight = false;
        match outcome.result {
            Ok(manifest) => {
                self.session_manifest_stale = false;
                for (idx, version) in outcome.plan {
                    let Some(tab) = self.tabs.get_mut(idx) else {
                        continue; // 期间被关闭/下标漂移：放弃这条账目
                    };
                    if !(tab.dirty && tab.version == version) {
                        continue;
                    }
                    if let Some(name) =
                        manifest.tabs.get(idx).and_then(|t| t.file.clone())
                    {
                        tab.heartbeat_snap = Some((version, name));
                    }
                }
            }
            Err(error) => {
                for tab in &mut self.tabs {
                    tab.heartbeat_snap = None;
                }
                self.status = format!("快照心跳失败:{error}");
            }
        }
    }

    /// 同步执行一次完整心跳周期——生产路径走异步 [`Self::drive_heartbeat`]，
    /// 本入口供测试端到端验证磁盘产物与账目回填（共用 prepare/apply 两半）。
    #[cfg(test)]
    fn run_heartbeat_cycle(&mut self, dir: &Path) -> Option<HeartbeatOutcome> {
        let payload = self.prepare_heartbeat_commit(dir)?;
        self.heartbeat_inflight = true;
        let result = editpad_core::snapshot::write_heartbeat_session(
            &payload.dir,
            &payload.pages,
            payload.active,
            payload.next_untitled,
        )
        .map_err(|e| e.to_string());
        let outcome = HeartbeatOutcome {
            plan: payload.plan,
            result,
        };
        self.heartbeat_apply(outcome.clone());
        Some(outcome)
    }

    // ---------- 关窗流（P29 快照直退） ----------

    /// 关窗请求处置：快照直退的前提 = 总开关开启 + 模式为快照 + 快照目录
    /// 可用；任一不满足即回退旧行为（置脏弹确认条 / 干净直接关）。
    ///
    /// `snapshot_dir` 由调用方解析传入——测试注入项目内目录，
    /// 避免触碰真实 %APPDATA%（None = 无目录可用，功能自动降级）。
    fn handle_close_request(&mut self, id: window::Id, snapshot_dir: Option<PathBuf>) -> Task<Message> {
        // 捕获主窗口 id（仅有的窗口），供后续 window::close 使用
        self.main_window = Some(id);
        // P32：关窗前把全部命名页的光标/滚动回写最近文件记忆
        // （快照直退与旧确认条两条路径都要覆盖；未命名页自然跳过）
        let all: Vec<usize> = (0..self.tabs.len()).collect();
        self.remember_tab_views(&all);
        // P30：remember_session 关闭 = 退出不写会话清单，回退旧确认条
        if !session_restore_allowed(
            self.settings.enable_snapshots,
            self.settings.remember_session,
        ) || self.settings.exit_mode != editpad_core::EXIT_MODE_SNAPSHOT
        {
            return self.confirm_or_close();
        }
        match snapshot_dir {
            Some(dir) => self.exit_via_snapshot(&dir),
            None => self.confirm_or_close(),
        }
    }

    /// 旧关窗行为（P29 前的原语义）：任一标签页置脏即弹确认（聚合口径）。
    fn confirm_or_close(&mut self) -> Task<Message> {
        if self.any_dirty() {
            self.confirm_visible = true;
            Task::none()
        } else {
            self.close_window()
        }
    }

    /// P29 退出零询问：全部置脏页写内容快照 → 清单提交 → 直接关窗。
    ///
    /// 同步执行——关窗瞬间阻塞 UI 数百毫秒量级（50MB 分块写 ~200ms），
    /// 换取「快照必然反映最终状态」的无竞话语义；窗口即将关闭，用户无感。
    /// 干净页只记元数据（路径/光标/滚动），不产生内容文件；
    /// 失败降级：回退旧确认条（数据仍在内存不丢），状态栏留原因。
    fn exit_via_snapshot(&mut self, dir: &Path) -> Task<Message> {
        let pages: Vec<editpad_core::snapshot::SessionPage> = self
            .tabs
            .iter()
            .map(|t| editpad_core::snapshot::SessionPage {
                // 退出流全量重写：file 恒 None，置脏页一律落新文件
                tab: Self::session_tab_metadata(t, None),
                doc: t.editor.borrow().doc.clone(),
            })
            .collect();
        match editpad_core::snapshot::write_session(
            dir,
            &pages,
            self.active_tab,
            self.untitled_next,
        ) {
            Ok(_) => {
                self.status.clear();
                self.close_window()
            }
            Err(error) => {
                self.status = format!("会话快照失败:{error}");
                self.confirm_or_close()
            }
        }
    }

    /// 关闭主窗口；id 来自 close_requests 订阅的捕获。
    fn close_window(&self) -> Task<Message> {
        match self.main_window {
            Some(id) => window::close(id),
            None => Task::none(),
        }
    }

    // ---------- 启动会话恢复（P30） ----------

    /// 启动时会话恢复入口：开关任一关闭 = 空白启动（存量会话清场，
    /// 「只关开关不清数据等于没关」沿用 P20 先例）；清单缺失/损坏/空页
    /// = 无会话；clean_exit=false（上次异常退出，P31 心跳的中间态即此
    /// 形态）= 弹一次性恢复提示等用户裁决；否则静默全量还原。
    fn boot_restore(&mut self) -> Task<Message> {
        if !session_restore_allowed(
            self.settings.enable_snapshots,
            self.settings.remember_session,
        ) {
            // enable_snapshots=false 的清场已在 new() 做过；这里补上
            // remember_session=false 的清场
            if !self.settings.remember_session {
                if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                    editpad_core::snapshot::clear_session(&dir);
                }
            }
            return Task::none();
        }
        match editpad_core::snapshot::snapshot_dir() {
            Some(dir) => self.restore_from_dir(&dir),
            None => Task::none(),
        }
    }

    /// 从注入的快照目录执行恢复决策。生产路径经 [`Self::boot_restore`]；
    /// 测试注入项目内目录，避免触碰真实 %APPDATA%（同 P29 先例）。
    fn restore_from_dir(&mut self, dir: &Path) -> Task<Message> {
        let Some(manifest) = editpad_core::snapshot::read_manifest(dir) else {
            return Task::none(); // 无会话/损坏清单一律按空白启动，绝不 panic
        };
        if manifest.tabs.is_empty() {
            return Task::none();
        }
        if !manifest.clean_exit {
            // 孤儿检测 = 崩溃恢复入口（§3 P30 第 2 条）：弹一次性提示，
            // 数据原封留在磁盘，等用户选择恢复或丢弃
            self.recover_prompt = Some(manifest);
            return Task::none();
        }
        self.restore_from_manifest(dir, &manifest)
    }

    /// 按清单重建标签页（P30 主路径）：
    /// * 未命名页与置脏命名页从快照**同步**还原内容——v1 一律信快照
    ///   （§3 P30 第 3 条，主流编辑器「所见即所得」），不从磁盘重载覆盖；
    /// * 干净命名页建占位页排队，逐个经既有加载管线回填（失败跳过不阻断）；
    /// * 未命名编号延续单调性；激活页最后切换；
    /// * 内存护栏在规划期整体截断（先保激活页）。
    fn restore_from_manifest(
        &mut self,
        dir: &Path,
        manifest: &editpad_core::snapshot::SessionManifest,
    ) -> Task<Message> {
        let (kept, dropped) =
            plan_restore_order(manifest, dir, MULTI_TAB_MEM_CAP_BYTES);
        self.restore_dropped = dropped;
        self.restore_failed = 0;

        let mut tabs: Vec<Tab> = Vec::with_capacity(kept.len());
        let mut queue: Vec<RestoreLoad> = Vec::new();
        let mut next_untitled = 2u64;

        for &i in &kept {
            let meta = &manifest.tabs[i];
            let mut tab = self.fresh_tab();
            match (&meta.path, meta.file.as_deref()) {
                // ---- 未命名页：内容只可能来自快照 ----
                (None, _) => {
                    tab.untitled_num = meta.untitled_num;
                    if let Some(n) = meta.untitled_num {
                        next_untitled = next_untitled.max(n.saturating_add(1));
                    }
                    if meta.dirty {
                        match editpad_core::snapshot::read_page(dir, meta) {
                            Some(doc) => {
                                {
                                    let mut ed = tab.editor.borrow_mut();
                                    ed.reset_document(doc);
                                    // P38：快照内容不是磁盘内容，不得当落盘
                                    // 基线——否则撤销回快照态会错误清脏，
                                    // 未存改动在零询问退出后静默丢失
                                    ed.clear_saved_baseline();
                                    ed.restore_view(
                                        meta.cursor_line,
                                        meta.cursor_col,
                                        meta.scroll_top,
                                        meta.scroll_left,
                                    );
                                    // 内容嗅探让未命名草稿同样享受配色
                                    let sample = head_sample(&ed.doc);
                                    ed.set_language_by_name(
                                        editpad_core::resolve_language(None, &sample)
                                            .as_deref(),
                                    );
                                }
                                tab.dirty = true;
                            }
                            None => {
                                // 快照缺失/损坏：内容已不可得——留空页继续，
                                // 计入失败汇总（单页失败不阻断，§3 P30 第 6 条）
                                self.restore_failed += 1;
                            }
                        }
                    }
                }
                // ---- 置脏命名页：信快照（所见即所得） ----
                (Some(path), Some(_)) => {
                    match editpad_core::snapshot::read_page(dir, meta) {
                        Some(doc) => {
                            {
                                let mut ed = tab.editor.borrow_mut();
                                ed.reset_document(doc);
                                // P38：同未命名页——快照 ≠ 磁盘，基线保守置空，
                                // 撤销回快照态不得清脏（磁盘还是旧内容）
                                ed.clear_saved_baseline();
                                ed.restore_view(
                                    meta.cursor_line,
                                    meta.cursor_col,
                                    meta.scroll_top,
                                    meta.scroll_left,
                                );
                                let sample = head_sample(&ed.doc);
                                ed.set_language_by_name(
                                    editpad_core::resolve_language(
                                        Some(Path::new(path)),
                                        &sample,
                                    )
                                    .as_deref(),
                                );
                            }
                            tab.path = Some(PathBuf::from(path));
                            // 快照恒为 UTF-8 落盘；原文件编码知情权随下次保存归一
                            tab.encoding_label = "UTF-8".to_owned();
                            tab.dirty = true;
                        }
                        None => {
                            // 快照读不出（半截写盘等）：退化为按干净命名页
                            // 从磁盘加载——内容退回上次保存态的现实兜底，
                            // 但未存改动确实丢了，计入失败汇总如实告知
                            self.restore_failed += 1;
                            queue.push(RestoreLoad {
                                path: PathBuf::from(path),
                                tab: tabs.len(),
                                cursor_line: meta.cursor_line,
                                cursor_col: meta.cursor_col,
                                scroll_top: meta.scroll_top,
                                scroll_left: meta.scroll_left,
                            });
                        }
                    }
                }
                // ---- 干净命名页：占位 + 排队走既有加载管线 ----
                (Some(path), None) => {
                    if meta.dirty {
                        // P31 超限节流页（置脏但心跳从未落其内容）：
                        // 退回磁盘上次保存态，未存改动不可得——如实入汇总
                        self.restore_failed += 1;
                    }
                    queue.push(RestoreLoad {
                        path: PathBuf::from(path),
                        tab: tabs.len(),
                        cursor_line: meta.cursor_line,
                        cursor_col: meta.cursor_col,
                        scroll_top: meta.scroll_top,
                        scroll_left: meta.scroll_left,
                    });
                }
            }
            tabs.push(tab);
        }

        self.tabs = tabs;
        // 激活页最后还原；编号计数取「清单值」与「实际用号+1」的较大者
        self.active_tab = manifest.active.min(self.tabs.len().saturating_sub(1));
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
        next_untitled = next_untitled.max(manifest.next_untitled);
        self.untitled_next = next_untitled;

        self.restore_queue = queue;
        self.restore_pending = self.restore_queue.len();
        self.begin_restore_load()
    }

    /// 恢复链：载入队列中的下一个命名页（队列空则收尾出汇总）。
    /// 与用户打开的区别：绝不切焦点、不做逐次内存守卫（规划期已整体
    /// 截断）、登记待还原视图供 Loaded 回填光标滚动。
    fn begin_restore_load(&mut self) -> Task<Message> {
        // 队列语义必须 FIFO（remove(0) 而非 pop()）：加载顺序 = 清单下标
        // 顺序，占位页与排队项的下标对应关系才不会错位（LIFO 会让
        // 「失败移除」作用在错误的页上——回归测试现场抓过）
        if self.restore_queue.is_empty() {
            self.finish_restore_summary();
            return Task::none();
        }
        let entry = self.restore_queue.remove(0);
        if self.busy || self.active_load.is_some() {
            // 载入通道被占用（理论不可达：恢复链独占调度）：塞回队首等下轮
            self.restore_queue.insert(0, entry);
            return Task::none();
        }
        let id = self.register_load_job(entry.path, entry.tab);
        self.restore_views
            .insert(id, (entry.cursor_line, entry.cursor_col, entry.scroll_top, entry.scroll_left));
        Task::none()
    }

    /// 恢复链的一步收尾：待载数递减；队列排空时出汇总状态。
    /// 返回是否需要续排下一页（Loaded 处理器据此链接任务）。
    fn settle_restore_step(&mut self) -> bool {
        self.restore_pending = self.restore_pending.saturating_sub(1);
        if self.restore_queue.is_empty() {
            self.finish_restore_summary();
            false
        } else {
            true
        }
    }

    /// 恢复收尾汇总：只有出现值得告知的情况才打扰状态栏
    /// （失败页/截断页）；全部成功则保持安静，界面本身即是恢复事实。
    fn finish_restore_summary(&mut self) {
        let mut notes: Vec<String> = Vec::new();
        if self.restore_failed > 0 {
            notes.push(format!("{} 页未能恢复原内容", self.restore_failed));
        }
        if self.restore_dropped > 0 {
            notes.push(format!("{} 页超出内存护栏未恢复", self.restore_dropped));
        }
        if !notes.is_empty() {
            self.status = format!("会话恢复完成:{}", notes.join("，"));
        }
        self.restore_failed = 0;
        self.restore_dropped = 0;
        self.restore_pending = 0;
    }

    /// 移除第 idx 个恢复占位页，并把队列中大于 idx 的目标下标整体前移
    /// （页面移除后，后续排队页的下标随之左移）。仅当该页仍是空净无名
    /// 占位页才动手——防误删恢复期间用户产生的内容。
    fn drop_restore_placeholder(&mut self, idx: usize) {
        let placeholder_ok = self.tabs.get(idx).is_some_and(|t| {
            t.path.is_none()
                && !t.dirty
                && t.untitled_num.is_none()
                && t.encoding_label.is_empty()
                && t.editor.borrow().doc.is_empty()
        });
        if !placeholder_ok {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            let tab = self.fresh_tab();
            self.tabs.push(tab);
        }
        for entry in &mut self.restore_queue {
            if entry.tab > idx {
                entry.tab -= 1;
            }
        }
        // active_tab 可能越界：与 tabs 对齐并同步别名
        self.refresh_cur_handle();
    }

    /// 目标页是否仍是待填充的恢复占位页（防串写护栏）：恢复期间用户
    /// 关页/新建页会让队列下标漂移，宁可丢弃结果也不能覆盖用户内容。
    fn restore_placeholder_ready(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|t| {
            t.path.is_none()
                && !t.dirty
                && t.untitled_num.is_none()
                && t.encoding_label.is_empty()
                && t.editor.borrow().doc.is_empty()
        })
    }

    /// 崩溃恢复提示条「恢复」：按暂存清单全量重建（含脏页内容）。
    fn accept_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
        let Some(manifest) = self.recover_prompt.take() else {
            return Task::none();
        };
        match dir {
            Some(dir) => self.restore_from_manifest(&dir, &manifest),
            // 目录没了：无从恢复；数据仍在磁盘原处，本次空白起步
            None => Task::none(),
        }
    }

    /// 「丢弃」= 连快照一起丢（P29 放弃语义同族）：清场后空白起步，
    /// 防止下次启动把已放弃的内容再次当会话恢复回来。
    fn discard_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
        self.recover_prompt = None;
        if let Some(dir) = dir {
            editpad_core::snapshot::clear_session(&dir);
        }
        Task::none()
    }

    // ---------- 查找 / 替换内部逻辑 ----------

    /// 排队一次后台查找扫描（P10）。查询为空或查找栏已关闭时转为取消。
    /// UI 线程只做廉价操作：文档快照是 rope 结构共享克隆，全文扫描
    /// 在防抖 200ms 后的后台线程进行，结果按序号回填。
    fn schedule_find_scan(&mut self) -> Task<Message> {        if !self.find_visible || self.find_query.is_empty() {
            self.cancel_find_scan();
            return Task::none();
        }
        // 作废上一代任务（若它还睡在防抖窗口里，醒来即退出）
        self.find_cancel.store(true, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.find_cancel = cancelled.clone();
        self.find_seq += 1;
        let payload = FindScanPayload {
            seq: self.find_seq,
            doc: self.cur_handle.borrow().doc.clone(),
            // P22 补充：查询做转义解析（\n \r \t \\）后再扫描
            query: unescape_query(&self.find_query),
            case_sensitive: self.case_sensitive,
            cancelled,
            debounce_ms: FIND_DEBOUNCE_MS,
        };
        self.find_scan = Some(self.find_seq);
        Task::perform(drive_find_scan(payload, |doc, q, cs| {
            editpad_core::find_all_document(doc, q, cs)
        }), |message| message)
    }

    /// 取消在途扫描并清空结果（关查找栏/Esc/清空查询共用）。
    /// 序号递增 + 取消标志置位双保险，保证在途任务的结果回来后必被丢弃。
    fn cancel_find_scan(&mut self) {
        self.find_cancel.store(true, Ordering::Relaxed);
        self.find_seq += 1;
        self.find_scan = None;
        self.matches.clear();
        self.match_idx = None;
    }

    fn find_scanning(&self) -> bool {
        self.find_scan.is_some()
    }

    /// 安排一次后台高亮铺建（P12）。
    ///
    /// * 同代已在途 → 幂等跳过（控件每帧 RedrawRequested 都可能喊一次）；
    /// * 期间发生过编辑（换代）→ 作废旧任务，从当前存活检查点重新出发；
    /// * UI 线程成本 = rope 结构共享克隆 + 高亮器检查点向量拷贝，零解析。
    fn schedule_highlight_pave(&mut self) -> Task<Message> {
        let current_gen = self.cur_handle.borrow().highlight_generation();
        if let (Some(active), Some(current)) = (self.hl_paving, current_gen) {
            if active == current {
                return Task::none();
            }
        }
        let Some((gen, highlighter)) = self.cur_handle.borrow().highlight_pave_snapshot() else {
            // 无高亮器（纯文本路径）：清掉可能残留的旧任务登记
            self.hl_paving = None;
            return Task::none();
        };
        let doc = self.cur_handle.borrow().doc.clone();
        let total_lines = doc.line_count();

        // 作废上一代任务：批间取消标志 + 结果按代次过滤双保险
        self.hl_pave_cancel.store(true, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.hl_pave_cancel = cancelled.clone();

        self.hl_paving = Some(gen);
        Task::stream(build_hl_pave_stream(HlPavePayload {
            gen,
            doc,
            highlighter,
            total_lines,
            cancelled,
            batch_strides: HL_PAVE_BATCH_STRIDES,
        }))
    }

    fn step_match(&mut self, forward: bool) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        // 扫描在途：不基于过期命中表跳转
        if self.find_scanning() {
            self.status = "查找中…".to_owned();
            return Task::none();
        }
        if self.matches.is_empty() {
            // 懒触发：开栏即按 Enter 而扫描还没排队过时，先补一次扫描
            return self.schedule_find_scan();
        }
        if self.matches.is_empty() {
            self.status = "无匹配".to_owned();
            return Task::none();
        }

        let cursor = self.cur_handle.borrow().cursor;
        let index = if forward {
            editpad_core::next_from(&self.matches, cursor.line, cursor.col)
        } else {
            editpad_core::prev_from(&self.matches, cursor.line, cursor.col)
        };
        self.match_idx = index;

        if let (Some(i), Some(pos)) =
            (index, index.and_then(|i| self.matches.get(i).copied()))
        {
            // P26：选区跨度直接用命中自带的 len_chars（扫描器产出的
            // 「选区显示跨度」口径），不再按当前输入现算查询长度——
            // 单行命中两者相等，跨行命中的正确性由数据自身保证，
            // 不依赖「命中表与输入框同步」这条时序假设
            self.cur_handle
                .borrow_mut()
                .select_span(pos.line, pos.col, pos.len_chars);
            self.status = format!("第 {}/{} 处匹配", i + 1, self.matches.len());
        }
        Task::none()
    }

    fn replace_current(&mut self) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        let effective_query = unescape_query(&self.find_query);
        let effective_replacement = unescape_query(&self.replace_query);
        let hit_selected = {
            let editor = self.cur_handle.borrow();
            let eol = editor.doc.line_ending();
            editor
                .selected_text()
                .is_some_and(|selected| {
                    // P26：两侧行尾归一后再比（复用 P9 口径）——CRLF 文档上
                    // 跨行命中的选区文本含 \r\n，而查询是 \n；不归一会让
                    // 「替换当前」永远判不等、退化为「跳下一个」
                    strings_equal(
                        &eol.normalize(&selected),
                        &eol.normalize(&effective_query),
                        self.case_sensitive,
                    )
                })
        };

        if hit_selected {
            self.cur_handle
                .borrow_mut()
                .replace_selection(&effective_replacement);
            self.tab_mut().dirty = true;
            // P10：替换后命中表已过期，排队后台重扫；「跳到下一个」等重扫完成
            // 后由用户再按（旧行为是同步重扫后立即跳，会卡大文档 UI）
            return self.schedule_find_scan();
        }
        // 没有可替换的选区：行为不变——跳到下一个匹配
        self.step_match(true)
    }

    // ---------- 展示辅助 ----------

    fn title(&self) -> String {
        let name = self.tab().base_name();
        if self.tab().dirty {
            format!("● {name} - Editpad")
        } else {
            format!("{name} - Editpad")
        }
    }

    fn suggested_name(&self) -> String {
        match self.file_display_name() {
            Some(name) => name,
            // P25：未命名页的另存为建议名带序号并补扩展名
            None => match self.tab().untitled_num {
                Some(n) => format!("未命名{n}.txt"),
                None => "未命名.txt".into(),
            },
        }
    }

    fn file_display_name(&self) -> Option<String> {
        self.tab()
            .path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_owned)
    }

    /// P28 标签右键菜单面板：固定/保存/另存为(重命名)/关闭/关闭其他/
    /// 关闭右侧。菜单项按页面状态禁用（busy、干净页的保存、固定页的
    /// 关闭、无可关目标的批量项）；调用方保证 idx < tabs.len()。
    fn tab_context_panel(&self, idx: usize) -> Element<'_, Message> {
        let tab = &self.tabs[idx];
        let interactive = !self.busy;
        // P33/P36：UI 与正文同族，字号固定不随正文缩放；P34：族随设置
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();

        let mut panel = column![
            row![
                text(format!("「{}」", tab.base_name()))
                    .size(uipx)
                    .font(uifont)
                    .color([0.5, 0.5, 0.5]),
                button(text("×").size(uipx).font(uifont))
                    .padding([2, 8])
                    .on_press(Message::TabContextMenuClosed),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(2)
        .padding([4, 10]);

        // 固定 / 取消固定（固定页豁免一切关闭路径）
        let pin_label = if tab.pinned { "取消固定" } else { "📌 固定标签页" };
        panel = panel.push(
            button(container(text(pin_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .on_press_maybe(interactive.then_some(Message::TogglePinTab(idx))),
        );
        // 保存：仅置脏可用（与工具栏「保存」同一口径）；
        // 未命名页在 update 层自动落另存为
        panel = panel.push(
            button(container(text("保存").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .on_press_maybe(
                    (interactive && tab.dirty).then_some(Message::SaveTabFromMenu(idx)),
                ),
        );
        // 另存为 / 重命名（v1 同一动作兜底，§3 P28 第 2 条）
        let rename_label = if tab.path.is_some() { "重命名…" } else { "另存为…" };
        panel = panel.push(
            button(container(text(rename_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .on_press_maybe(interactive.then_some(Message::RenameOrSaveAsTab(idx))),
        );

        panel = panel.push(rule::horizontal(1));

        // 关闭：固定页拒绝（update 层守卫 + 菜单项灰掉双保险）
        panel = panel.push(
            button(container(text("关闭").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .on_press_maybe(
                    (interactive && !tab.pinned).then_some(Message::CloseTabAt(idx)),
                ),
        );
        // 关闭其他 / 关闭右侧：无可关目标（全部是固定页或没有其他页）时禁用
        let others = batch_close_targets(&self.tabs, BatchCloseScope::Others(idx));
        let right = batch_close_targets(&self.tabs, BatchCloseScope::RightOf(idx));
        panel = panel.push(
            button(
                container(text(format!("关闭其他标签页({})", others.len()))
                    .size(uipx)
                    .font(uifont))
                    .width(Fill),
            )
            .width(Fill)
            .on_press_maybe(
                (!others.is_empty() && interactive).then_some(Message::CloseOtherTabs(idx)),
            ),
        );
        panel = panel.push(
            button(
                container(text(format!("关闭右侧标签页({})", right.len()))
                    .size(uipx)
                    .font(uifont))
                    .width(Fill),
            )
            .width(Fill)
            .on_press_maybe(
                (!right.is_empty() && interactive).then_some(Message::CloseTabsRight(idx)),
            ),
        );
        panel.into()
    }

    /// P27 设置弹窗面板：收编原本只能手改 config.toml 的散落设置
    /// （主题/字号/即时保存/隐私/会话）+ 只读热键速查表。改动即写回。
    fn settings_panel(&self) -> Element<'_, Message> {
        let s = &self.settings;
        // P33/P36：UI 与正文同族，字号固定不随正文缩放；P34：族随设置
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();

        // 热键速查表（只读）：数据源 = HOTKEYS，与 README「快捷键」段同源
        let mut hotkey_col = column![text("热键（速查）").size(uipx).font(uifont)].spacing(2);
        for (combo, desc) in HOTKEYS {
            hotkey_col = hotkey_col.push(
                row![
                    text(*combo).size(uipx).font(uifont).width(150),
                    text(*desc).size(uipx).font(uifont).color([0.5, 0.5, 0.5]),
                ]
                .spacing(8),
            );
        }

        container(
            column![
                row![
                    text("设置").size(uipx * 1.25).font(uifont),
                    button(text("×").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press(Message::SettingsToggled),
                ]
                .align_y(Alignment::Center)
                .spacing(8),
                rule::horizontal(1),

                // ---- 外观 ----
                text("外观").size(uipx).font(uifont),
                row![
                    text("主题").size(uipx).font(uifont),
                    button(text(if s.is_dark() { "深色" } else { "浅色" })
                        .size(uipx)
                        .font(uifont))
                        .padding([2, 8])
                        .on_press(Message::ThemeToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text("字号").size(uipx).font(uifont),
                    button(text("A-").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (self.display_font_size()
                                > editpad_core::settings::MIN_FONT_SIZE)
                                .then_some(Message::FontSizeDelta(-2.0))
                        ),
                    text(format!("{:.0}", self.display_font_size()))
                        .size(uipx)
                        .font(uifont),
                    button(text("A+").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (self.display_font_size()
                                < editpad_core::settings::MAX_FONT_SIZE)
                                .then_some(Message::FontSizeDelta(2.0))
                        ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),

                // ---- 正文字体（P34） ----
                text("正文字体").size(uipx).font(uifont),
                row![
                    text(match (&s.font_family, &self.active_font_family) {
                        (Some(cfg), Some(eff)) if cfg.as_str() == *eff => {
                            format!("当前：{eff}")
                        }
                        (Some(cfg), _) => {
                            format!("当前：默认等宽（配置的「{cfg}」未安装）")
                        }
                        (None, _) => "当前：默认（等宽）".to_owned(),
                    })
                    .size(uipx)
                    .font(uifont),
                    button(text("回退默认").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            s.font_family.is_some().then_some(Message::SettingsFontReset)
                        ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text("建议选含中文字形的等宽字体；非等宽字体的列对齐会漂移")
                    .size(uipx * 0.85)
                    .font(uifont)
                    .color([0.5, 0.5, 0.5]),
                // 过滤框 + 候选列表（数据源 = 启动期 fontdb 枚举，名字即选即用）
                text_input("输入关键字过滤字体", &self.font_filter)
                    .size(uipx)
                    .font(uifont)
                    .on_input(Message::FontFilterChanged)
                    .width(Fill),
                {
                    let needle = editor::normalize_family(&self.font_filter);
                    let matches: Vec<&String> = self
                        .available_fonts
                        .iter()
                        .filter(|f| {
                            needle.is_empty()
                                || editor::normalize_family(f).contains(&needle)
                        })
                        .collect();
                    let total = matches.len();
                    // 显式标注：两个分支的 widget 类型不同，靠 Into 目标统一
                    let picker: Element<'_, Message> =
                        if self.available_fonts.is_empty() {
                            text("无法枚举系统字体（保持默认等宽）")
                                .size(uipx)
                                .font(uifont)
                                .color([0.7, 0.4, 0.1])
                                .into()
                        } else {
                            let mut list = column![].spacing(2);
                            for name in matches.iter().take(FONT_PICKER_MAX_ROWS) {
                                list = list.push(
                                    button(container(
                                        text(name.as_str()).size(uipx).font(uifont),
                                    )
                                    .width(Fill))
                                    .width(Fill)
                                    .on_press(Message::SettingsFontSelected(
                                        (*name).clone(),
                                    )),
                                );
                            }
                            if total > FONT_PICKER_MAX_ROWS {
                                list = list.push(
                                    text(format!(
                                        "…共 {total} 个命中，请继续输入关键字缩小范围"
                                    ))
                                    .size(uipx * 0.85)
                                    .font(uifont)
                                    .color([0.5, 0.5, 0.5]),
                                );
                            }
                            scrollable(list).height(180).into()
                        };
                    picker
                },

                // ---- 即时保存（P18） ----
                text("即时保存").size(uipx).font(uifont),
                checkbox(s.autosave_enabled)
                    .label("停手后自动落盘")
                    .text_size(uipx)
                    .font(uifont)
                    .on_toggle(Message::SettingsAutosaveToggled),
                row![
                    text("防抖秒数").size(uipx).font(uifont),
                    button(text("-").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (s.autosave_delay_secs
                                > editpad_core::settings::MIN_AUTOSAVE_DELAY_SECS)
                                .then_some(Message::SettingsAutosaveDelayDelta(-1))
                        ),
                    text(format!("{}", s.autosave_delay_secs))
                        .size(uipx)
                        .font(uifont),
                    button(text("+").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (s.autosave_delay_secs
                                < editpad_core::settings::MAX_AUTOSAVE_DELAY_SECS)
                                .then_some(Message::SettingsAutosaveDelayDelta(1))
                        ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),

                // ---- 隐私与会话（P20/P29/P30/P31） ----
                text("隐私与会话").size(uipx).font(uifont),
                checkbox(s.remember_recent_files)
                    .label("记住最近打开的文件")
                    .text_size(uipx)
                    .font(uifont)
                    .on_toggle(Message::SettingsRememberRecentToggled),
                checkbox(s.enable_snapshots)
                    .label("会话快照（关窗自动保存未存内容）")
                    .text_size(uipx)
                    .font(uifont)
                    .on_toggle(Message::SettingsSnapshotsToggled),
                checkbox(s.remember_session)
                    .label("启动时恢复上次界面")
                    .text_size(uipx)
                    .font(uifont)
                    .on_toggle(Message::SettingsRememberSessionToggled),
                row![
                    text("关窗行为").size(uipx).font(uifont),
                    button(text(if s.exit_mode == editpad_core::settings::EXIT_MODE_SNAPSHOT {
                        "快照直退"
                    } else {
                        "每次询问"
                    })
                    .size(uipx)
                    .font(uifont))
                    .padding([2, 8])
                    .on_press(Message::SettingsExitModeToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text("心跳间隔秒数").size(uipx).font(uifont),
                    button(text("-").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (s.snapshot_interval_secs
                                > editpad_core::settings::MIN_SNAPSHOT_INTERVAL_SECS)
                                .then_some(Message::SettingsIntervalDelta(-5))
                        ),
                    text(format!("{}", s.snapshot_interval_secs))
                        .size(uipx)
                        .font(uifont),
                    button(text("+").size(uipx).font(uifont))
                        .padding([2, 8])
                        .on_press_maybe(
                            (s.snapshot_interval_secs
                                < editpad_core::settings::MAX_SNAPSHOT_INTERVAL_SECS)
                                .then_some(Message::SettingsIntervalDelta(5))
                        ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),

                rule::horizontal(1),
                hotkey_col,
            ]
            .spacing(6)
            .padding(12),
        )
        .width(460)
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        // P22 第三批：当前页是否为 Markdown（决定预览按钮可用性）
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
        // P33/P36：UI 全部控件与正文同族，字号固定不随正文缩放——
        // A-/A+ 与 Ctrl+滚轮只调节文件内容；P34：族随设置切换
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();

        let toolbar = row![
            button(text("打开…").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::OpenRequested)),
            button(text("保存").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy && self.tab().dirty)
                    .then_some(Message::SaveRequested)),
            button(text("另存为…").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::SaveAsRequested)),
            // P22 第三批：Markdown 预览开关（仅 Markdown 语法页可用）
            button(text(if self.preview_visible {
                "关闭预览"
            } else {
                "MD 预览"
            })
            .size(uipx)
            .font(uifont))
            .padding([4, 12])
            .on_press_maybe(is_markdown.then_some(Message::PreviewToggled)),
            button(text("查找/替换").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::FindToggled)),
            button(text("跳转到行").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::GotoToggled)),
            button(text("最近文件").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::RecentsToggled)),
            button(text(if self.dark_mode {
                "主题:深色"
            } else {
                "主题:浅色"
            })
            .size(uipx)
            .font(uifont))
            .padding([4, 12])
            .on_press(Message::ThemeToggled),
            row![
                button(text("A-").size(uipx).font(uifont))
                    .padding([4, 10])
                    .on_press_maybe((self.display_font_size()
                        > editpad_core::settings::MIN_FONT_SIZE)
                        .then_some(Message::FontSizeDelta(-2.0))),
                text(format!("{:.0}", self.display_font_size()))
                    .size(uipx)
                    .font(uifont),
                button(text("A+").size(uipx).font(uifont))
                    .padding([4, 10])
                    .on_press_maybe((self.display_font_size()
                        < editpad_core::settings::MAX_FONT_SIZE)
                        .then_some(Message::FontSizeDelta(2.0))),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
            // P27：设置弹窗入口（busy 时禁开，与其余工具栏按钮同一守卫）
            button(text("设置").size(uipx).font(uifont))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::SettingsToggled)),
            text(if self.tab().dirty { "● 未保存" } else { "" })
                .size(uipx)
                .font(uifont)
                .color([0.85, 0.55, 0.1]),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 10]);

        // P21 标签条：恒显示（单页也给出「当前文件名」的可见反馈）。
        // 点击切换；置脏页带 ● 前缀；活动页加 ▸ 指示；固定页加 📌（P28）。
        // P28：按钮外包 mouse_area 捕获右键弹菜单——MouseArea 先把事件
        // 交给子组件，左键被 button 捕获后自身跳过，故切换不受影响；
        // 右键无人捕获，落到 on_right_press。
        // P39：整条标签条再包一层 mouse_area 跟踪指针位置（浮层菜单
        // 锚点数据源；只在标签条悬停时产生消息，量级可忽略）。
        let mut body = column![toolbar, rule::horizontal(1)];
        {
            let mut strip = row![].spacing(2).padding([4, 6]);
            for (i, tab) in self.tabs.iter().enumerate() {
                let marker = if i == self.active_tab { "▸ " } else { "  " };
                let pin = if tab.pinned { "📌 " } else { "" };
                strip = strip.push(
                    mouse_area(
                        button(text(format!(
                            "{marker}{pin}{}",
                            tab.display_name()
                        ))
                        .size(uipx)
                        .font(uifont))
                        .padding([2, 10])
                        .on_press_maybe((!self.busy).then_some(Message::SwitchTab(i))),
                    )
                    .on_right_press(Message::TabContextMenu(i)),
                );
            }
            body = body.push(
                mouse_area(strip).on_move(Message::CursorMoved),
            );
        }

        // 中间主区域：Markdown 预览面板 或 自绘虚拟化编辑器
        // P34：两路都按设置的字形族渲染（预览属内容渲染随族切换；
        // 画布经 view_with_font 每帧传参，无跨帧同步状态）
        let content_font = self.body_font();
        if self.preview_visible && is_markdown {
            let text = self.cur_handle.borrow().doc.to_text();
            body = body.push(markdown_preview_element(
                &text,
                self.display_font_size(),
                content_font,
            ));
        } else {
            body = body.push(self.cur_handle.view(content_font));
        }

        if let Some((bytes_read, total_bytes)) = self.progress {
            body = body.push(
                row![
                    text("加载中…").size(uipx).font(uifont),
                    container(
                        progress_bar(0.0..=total_bytes.max(1) as f32, bytes_read as f32)
                    )
                    .width(Fill),
                    text(format!(
                        "{} / {} KB",
                        bytes_read / 1024,
                        total_bytes.max(1) / 1024
                    ))
                    .size(uipx)
                    .font(uifont),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if self.recents_visible {
            let mut panel = column![].spacing(2).padding([4, 10]);
            if self.settings.recent_files.is_empty() {
                panel = panel.push(
                    text("（暂无最近文件）")
                        .size(uipx)
                        .font(uifont)
                        .color([0.5, 0.5, 0.5]),
                );
            }
            for entry in &self.settings.recent_files {
                panel = panel.push(
                    button(container(text(entry).size(uipx).font(uifont)).width(Fill))
                        .width(Fill)
                        .on_press_maybe(
                            (!self.busy).then_some(Message::RecentSelected(entry.clone())),
                        ),
                );
            }
            // P20 隐私出口：一键抹掉 config.toml 里的全部历史路径
            if !self.settings.recent_files.is_empty() {
                panel = panel.push(
                    row![
                        button(text("清空记录").size(uipx).font(uifont))
                            .padding([2, 8])
                            .on_press_maybe((!self.busy).then_some(Message::RecentsCleared)),
                        text("从 config.toml 移除全部路径")
                            .size(uipx)
                            .font(uifont)
                            .color([0.5, 0.5, 0.5]),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
            }
            body = body.push(rule::horizontal(1)).push(panel);
        }

        if self.find_visible {
            let total = self.matches.len();
            // P10：扫描在途时明确显示状态，按钮基于过期结果禁用
            let scanning = self.find_scanning();
            let position_label = if scanning {
                "查找中…".to_owned()
            } else if total == 0 {
                "无匹配".to_owned()
            } else {
                match self.match_idx {
                    Some(i) => format!("第 {}/{} 处", i + 1, total),
                    None => format!("{total} 处"),
                }
            };
            let has_matches = !scanning && !self.matches.is_empty();

            body = body.push(rule::horizontal(1)).push(
                row![
                    text_input("查找内容", &self.find_query)
                        .size(uipx)
                        .font(uifont)
                        .on_input(Message::FindQueryChanged)
                        .on_submit(Message::FindNext)
                        .width(200),
                    text(position_label).size(uipx).font(uifont),
                    button(text("↑ 上一个").size(uipx).font(uifont))
                        .on_press_maybe(has_matches.then_some(Message::FindPrev)),
                    button(text("↓ 下一个").size(uipx).font(uifont))
                        .on_press_maybe(has_matches.then_some(Message::FindNext)),
                    checkbox(self.case_sensitive)
                        .label("区分大小写")
                        .text_size(uipx)
                        .font(uifont)
                        .on_toggle(Message::CaseToggled),
                    button(text("×").size(uipx).font(uifont)).on_press(Message::FindToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );

            body = body.push(
                row![
                    text_input("替换为", &self.replace_query)
                        .size(uipx)
                        .font(uifont)
                        .on_input(Message::ReplaceQueryChanged)
                        .width(200),
                    button(text("替换当前").size(uipx).font(uifont))
                        .on_press_maybe(has_matches.then_some(Message::ReplaceCurrent)),
                    // 扫描在途时禁用：此刻的全文快照可能是过期的
                    button(text("全部替换").size(uipx).font(uifont)).on_press_maybe(
                        (!scanning).then_some(Message::ReplaceAll),
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if self.goto_visible {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text("跳转到行:").size(uipx).font(uifont),
                    text_input("行号", &self.goto_input)
                        .size(uipx)
                        .font(uifont)
                        .on_input(Message::GotoInputChanged)
                        .on_submit(Message::GotoSubmit)
                        .width(140),
                    button(text("跳转").size(uipx).font(uifont)).on_press(Message::GotoSubmit),
                    button(text("×").size(uipx).font(uifont)).on_press(Message::GotoToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P27 设置弹窗：P40 起为居中浮层模态（见 view 尾部 Stack 组装），
        // 不再内嵌推进 body 列。

        // 未保存关闭确认条：置于状态区域上方
        if self.confirm_visible {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text("文档有未保存的更改，确定要关闭吗？")
                        .size(uipx)
                        .font(uifont),
                    button(text("保存并关闭").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press_maybe(
                            (!self.busy).then_some(Message::ConfirmSaveAndClose)
                        ),
                    button(text("放弃更改").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::DiscardAndClose),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::CancelClose),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P21 标签页关闭确认：骨架版仅提供「放弃更改并关闭」出口，
        // 想保留改动请先 Ctrl+S（完整保存后关闭随 P21 完整版补齐）
        if let Some(idx) = self.close_tab_confirm {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!(
                        "第 {} 个标签页有未保存的更改",
                        idx.saturating_add(1)
                    ))
                    .size(uipx)
                    .font(uifont),
                    button(text("放弃更改并关闭").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::ConfirmCloseTabDiscard(idx)),
                    // P21 完整版：已命名的页可直接「保存并关闭」
                    button(text("保存并关闭").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press_maybe(
                            (!self.busy && self.tabs[idx].path.is_some())
                                .then_some(Message::CloseTabSave(idx)),
                        ),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::CancelCloseTab),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P28 批量关闭确认条：关闭其他/右侧的目标中有置脏页时弹一次，
        // 确认后统一放弃并移除（固定页本就不在目标列表内）
        if let Some(targets) = &self.batch_close_confirm {
            let total = targets.len();
            let dirty = targets
                .iter()
                .filter(|&&i| self.tabs.get(i).is_some_and(|t| t.dirty))
                .count();
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!(
                        "要关闭的 {total} 个标签页中 {dirty} 个有未保存的更改，全部放弃并关闭？"
                    ))
                    .size(uipx)
                    .font(uifont),
                    button(text("放弃更改并关闭").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::ConfirmBatchCloseDiscard),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::CancelBatchCloseTabs),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // 未保存时打开新文件的确认条（样式沿用关闭确认条）
        if let Some(path) = &self.open_confirm {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!("{} 有未保存的更改，放弃并打开？", path.display()))
                        .size(uipx)
                        .font(uifont),
                    button(text("放弃更改并打开").size(uipx).font(uifont))                        .padding([4, 12])
                        .on_press(Message::ConfirmOpenDiscard),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::ConfirmOpenCancel),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P30 崩溃恢复一次性提示条：上次未正常收尾（崩溃/P31 心跳中间态）。
        // 「恢复」按快照全量重建；「丢弃」连快照一起丢。数据原封留在磁盘，
        // 不裁决就一直挂着——与「未保存确认条」同级的强提醒语义。
        if self.recover_prompt.is_some() {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text("检测到上次未正常退出的未保存工作区")
                        .size(uipx)
                        .font(uifont),
                    button(text("恢复").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::SessionRecoverAccepted),
                    button(text("丢弃").size(uipx).font(uifont))
                        .padding([4, 12])
                        .on_press(Message::SessionRecoverDiscarded),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if !self.status.is_empty() {
            body = body.push(
                container(
                    text(format!("⚠ {}", self.status))
                        .size(uipx)
                        .font(uifont)
                        .color([0.9, 0.25, 0.25]),
                )
                .padding([4, 10]),
            );
        }

        let cursor = self.cur_handle.borrow().cursor;
        let status_bar = row![
            text(
                self.tab()
                    .path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| format!("({})", self.tab().base_name()))
            )
            .size(uipx)
            .font(uifont)
            .width(Fill),
            text(if self.tab().encoding_label.is_empty() {
                "—".to_owned()
            } else {
                self.tab().encoding_label.clone()
            })
            .size(uipx)
            .font(uifont),
            text(format!("{} 行", self.cur_handle.borrow().doc.line_count()))
                .size(uipx)
                .font(uifont),
            text(format!("Ln {}, Col {}", cursor.line + 1, cursor.col + 1))
                .size(uipx)
                .font(uifont),
        ]
        .spacing(24)
        .align_y(Alignment::Center)
        .padding([6, 10]);

        body = body.push(rule::horizontal(1)).push(status_bar);

        // P39/P40：浮层经 Stack 叠加——首层（正文）定尺寸，顶层浮在
        // 其上、不占布局空间（iced_widget-0.14.2 Stack 语义），正文可用
        // 面积不再被菜单/设置弹窗挤占。opaque 背板捕获层内点击，
        // 防止点菜单外的落穿透到正文。
        let base: Element<'_, Message> = container(body).width(Fill).height(Fill).into();
        let mut layered = Stack::new().push(base);
        if let Some(idx) = self.tab_context_menu {
            if idx < self.tabs.len() {
                layered = layered.push(self.context_menu_overlay(idx));
            }
        }
        if self.settings_visible {
            layered = layered.push(self.settings_overlay());
        }
        layered.into()
    }

    // ---------- 浮层弹窗（P39/P40） ----------

    /// P39：标签右键菜单浮层——整窗透明背板（点击即收起）+ 锚在指针
    /// 位置、贴边钳制后的菜单卡片。opaque 双层防穿透：背板捕获菜单外
    /// 点击不落到正文，卡片捕获卡片内空白处点击不触发背板关闭。
    /// P43：卡片高度按窗口钳制（小窗口不溢出、不盖满全屏），内容超高
    /// 时内部滚动——与设置弹窗同款适配策略。
    fn context_menu_overlay(&self, idx: usize) -> Element<'_, Message> {
        let vh = self.viewport_size.1;
        let card_h = ctx_menu_card_h(vh);
        let (ax, ay) = clamp_menu_anchor(
            self.menu_anchor,
            self.viewport_size,
            CTX_MENU_W,
            card_h,
        );
        let card = opaque(
            // P46：宽度必须 Shrink——scrollable 默认宽度是 Fill，
            // 会把菜单卡片横向撑满整窗（功能文字左侧、右侧一大片空白）
            container(scrollable(self.tab_context_panel(idx)).height(card_h).width(Shrink))
                .padding(4)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
        )
        .on_press(Message::TabContextMenuClosed)
        .on_right_press(Message::TabContextMenuClosed)
        .into()
    }

    /// P40：设置弹窗浮层——整窗背板（点击关闭）+ 居中卡片，卡片内部
    /// 滚动（内容高约 700px+，小窗口不溢出）。opaque 卡片让点击卡片
    /// 空白处（padding/标题行旁）不误触背板关闭。
    fn settings_overlay(&self) -> Element<'_, Message> {
        // 卡片内容高度上限 = 窗口高 − 边距；窗口尺寸未知时给保守值
        let vh = self.viewport_size.1;
        let list_h = if vh > 120.0 { vh - 80.0 } else { 600.0 };
        let card = opaque(
            container(scrollable(self.settings_panel()).width(Fill).height(list_h))
                .width(500)
                .padding(4)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(Alignment::Center)
                .padding(16),
        )
        .on_press(Message::SettingsToggled)
        .into()
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
fn ctx_menu_card_h(vh: f32) -> f32 {
    if vh > 48.0 {
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

/// 全局快捷键速查表（P27 v1 只读速查）。
///
/// 这是快捷键的**唯一展示数据源**：设置弹窗的热键表与 README「快捷键」
/// 段都从这里派生（`(组合键, 功能)`）。新增/修改快捷键时必须同步
/// [`handle_key`] 与 README，避免三处漂移——改动后跑
/// `hotkey_table_is_well_formed` 与 `hotkey_table_matches_handle_key`
/// 两个测试即可当场暴露漏改。
const HOTKEYS: &[(&str, &str)] = &[
    ("Ctrl+O", "打开文件"),
    ("Ctrl+S", "保存"),
    ("Ctrl+A", "全选"),
    ("Ctrl+F / Ctrl+H", "查找/替换栏"),
    ("Ctrl+G", "跳转到行"),
    ("Ctrl+Z", "撤销"),
    ("Ctrl+Y", "重做"),
    ("Ctrl+C", "复制选区"),
    ("Ctrl+X", "剪切选区"),
    ("Ctrl+V", "粘贴"),
    ("Ctrl+T", "新建标签页"),
    ("Ctrl+W", "关闭当前标签页"),
    ("Ctrl+Tab", "循环切换标签页"),
    ("Ctrl+Shift+F", "格式化 JSON（仅 JSON 文件）"),
    ("Ctrl+Home", "跳到文档首"),
    ("Ctrl+End", "跳到文档尾"),
    ("Shift+滚轮", "横向滚动"),
];

/// 全局按键分发：Ctrl 组合快捷键优先，其次编辑键与光标移动。
fn handle_key(key: keyboard::Key, mods: keyboard::Modifiers) -> Option<Message> {
    use keyboard::Key;

    let edit = |op| Some(Message::Edit(op));

    // Ctrl 组合（Windows/Linux 语义）。
    // P8：AltGr 在 Windows 上报为 Ctrl+Alt，欧洲键盘的 AltGr 字符
    // （德语 @=AltGr+Q 等）若进此分支匹配不到就被静默吞掉；
    // 因此仅「纯 Ctrl」才当快捷键，带 Alt 的一律按普通字符处理。
    if mods.control() && !mods.alt() {
        // P22 第二批：Ctrl+Shift+F = 格式化 JSON。必须先于普通字母映射
        // 分流，否则 Shift 产生的 'F' 会被小写化成 f 撞上「查找」。
        if mods.shift() {
            if let Key::Character(letter) = &key {
                if letter.to_ascii_lowercase() == "f" {
                    return Some(Message::FormatJson);
                }
            }
        }
        if let Key::Character(letter) = &key {
            let message = match letter.to_ascii_lowercase().as_str() {
                "o" => Message::OpenRequested,
                "s" => Message::SaveRequested,
                "f" | "h" => Message::FindToggled,
                "g" => Message::GotoToggled,
                "z" => Message::Edit(EditOp::Undo),
                "y" => Message::Edit(EditOp::Redo),
                "a" => Message::Edit(EditOp::SelectAll),
                // P4 剪贴板三件套
                "c" => Message::CopyRequested,
                "x" => Message::CutRequested,
                "v" => Message::PasteRequested,
                // P21 标签页
                "t" => Message::NewTab,
                "w" => Message::CloseTabRequest,
                _ => return None,
            };
            return Some(message);
        }
        // Ctrl+Home/End：文档首尾
        return match &key {
            Key::Named(Named::Home) => edit(EditOp::Motion(Motion::DocStart, mods.shift())),
            Key::Named(Named::End) => edit(EditOp::Motion(Motion::DocEnd, mods.shift())),
            // P21：Ctrl+Tab 循环切到下一个标签页
            Key::Named(Named::Tab) => Some(Message::SwitchTabNext),
            _ => None,
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::{executor::block_on, StreamExt};

    /// 项目内落盘目录：系统 TEMP 在部分沙箱下不可写。
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("editpad-app-tests")
            .join(format!("{tag}-{}", std::process::id()));
        fs_create_dir_all(&dir);
        dir
    }

    /// std::fs 的薄封装（避免测试里到处 use std::fs）。
    fn fs_create_dir_all(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
    }

    #[test]
    fn panic_message_handles_known_payloads() {
        assert_eq!(panic_message(&"字符串字面量"), "字符串字面量");
        assert_eq!(panic_message(&(String::from("堆上字符串"))), "堆上字符串");
        assert_eq!(panic_message(&42_i32), "未知原因");
    }

    // ---------- P33 字体一致性 ----------

    #[test]
    fn font_pin_smoke_call_twice_is_safe() {
        // P33 冒烟：公开路径 iced::advanced::graphics::text::font_system()
        // 在测试环境真实可用（不只是类型层面编译通过）；AtomicBool 幂等
        // 保证二次调用直接短路。副作用仅为 fontdb 的 Family::Monospace
        // 解析目标被钉到 CJK 等宽候选——测试无渲染断言，不受影响。
        apply_default_cjk_mono_pin();
        apply_default_cjk_mono_pin();
    }

    // ---------- P8 AltGr ----------

    #[test]
    fn altgr_character_falls_through_to_text_insert() {
        use iced::keyboard::{self, key::Named};
        let altgr = keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT;

        // 德语布局 @ = AltGr+Q：winit 上报 Ctrl+Alt + 字符 q/@，
        // 不得落入 Ctrl 快捷键分支被吞，必须按普通文本插入
        let at = keyboard::Key::Character("@".into());
        assert!(matches!(
            handle_key(at, altgr),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "@"
        ));

        let q = keyboard::Key::Character("q".into());
        assert!(matches!(
            handle_key(q, altgr),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "q"
        ));

        // 纯 Ctrl 的快捷键行为不受影响
        let ctrl = keyboard::Modifiers::CTRL;
        let o = keyboard::Key::Character("O".into());
        assert!(matches!(handle_key(o, ctrl), Some(Message::OpenRequested)));

        // Ctrl+未绑定字母仍返回 None（不插入；注意 x 已是 P4 剪切键）
        let q = keyboard::Key::Character("q".into());
        assert!(handle_key(q, ctrl).is_none());

        // Shift+字符（无 Ctrl）照常插入
        let bang = keyboard::Key::Character("!".into());
        assert!(matches!(
            handle_key(bang, keyboard::Modifiers::SHIFT),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "!"
        ));
        let _ = Named::Tab; // 保持 import 使用
    }

    // ---------- P4 剪贴板 ----------

    #[test]
    fn ctrl_c_x_v_map_to_clipboard_messages() {
        use iced::keyboard::{self, key::Named};
        let ctrl = keyboard::Modifiers::CTRL;
        let _ = Named::Tab;

        for (letter, expected) in [
            ("c", "copy"),
            ("x", "cut"),
            ("v", "paste"),
            ("C", "copy"), // 大写（Shift 同按）也走同一快捷键
        ] {
            let key = keyboard::Key::Character(letter.into());
            let message = handle_key(key, ctrl).expect("应产生消息");
            let ok = match (&message, expected) {
                (Message::CopyRequested, "copy")
                | (Message::CutRequested, "cut")
                | (Message::PasteRequested, "paste") => true,
                _ => false,
            };
            assert!(ok, "{letter} 应映射到剪贴板消息，实际 {message:?}");
        }
    }

    #[test]
    fn pasted_text_enters_document_as_dominant_eol() {
        // P9：粘贴不再在 app 层预归一；insert_str 按文档主导行尾统一改写。
        // CRLF 文档进来什么行尾都落成 \r\n，LF 文档落成 \n（与旧版行为一致）。
        let mut crlf = editor::EditorCore::default();
        crlf.reset_document(editpad_core::Document::from_str("a\r\nb"));
        crlf.cursor = editor::CursorPos { line: 1, col: 1 }; // 文末
        crlf.insert_str("x\r\ny\rz");
        assert_eq!(crlf.doc.to_text(), "a\r\nbx\r\ny\r\nz");

        let mut lf = editor::EditorCore::default();
        lf.insert_str(&"x\r\ny\rz");
        assert_eq!(lf.doc.to_text(), "x\ny\nz");
    }

    // ---------- P21 多标签骨架 ----------

    #[test]
    fn new_tab_switch_and_pages_stay_independent() {
        let mut app = Editpad::default();
        assert_eq!(app.tabs.len(), 1);

        // 初始页写内容 → 置脏
        dispatch(&mut app, Message::Edit(EditOp::InsertText("page0".into())));
        assert!(app.tab().dirty);

        // 连开两个新页：自动切换过去，且为空净未命名
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.active_tab, 2);
        assert!(
            app.cur_handle.borrow().doc.is_empty(),
            "新页应为空文档"
        );

        // 第 3 页编辑后切走再切回：内容与置脏状态各自独立
        dispatch(&mut app, Message::Edit(EditOp::InsertText("page2".into())));
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "page0");
        assert!(app.tab().dirty);
        dispatch(&mut app, Message::SwitchTab(2));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "page2");

        // Ctrl+Tab 循环
        dispatch(&mut app, Message::SwitchTabNext);
        assert_eq!(app.active_tab, 0, "从末页循环回首页");
    }

    #[test]
    fn loaded_result_routes_to_origin_tab_after_switch() {
        let mut app = Editpad::default();

        // 打开 A：落入初始空净页
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq_a = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq_a,
                Ok((editpad_core::Document::from_str("content A"), String::new(), "UTF-8".to_owned())),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "content A");

        // 当前页非空 → B 落新页 idx1 并切换过去；加载完成前用户切回页 0
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        assert_eq!(app.active_tab, 1, "非空当前页应新开一页承接打开");
        let seq_b = app.job_seq;
        dispatch(&mut app, Message::SwitchTab(0));

        dispatch(
            &mut app,
            Message::Loaded(
                seq_b,
                Ok((editpad_core::Document::from_str("content B"), String::new(), "UTF-8".to_owned())),
            ),
        );

        assert_eq!(app.active_tab, 0, "切走的用户不被加载结果拉回");
        assert_eq!(app.tabs.len(), 2);
        let origin_doc = app.tabs[1].editor.borrow().doc.to_text();
        assert_eq!(origin_doc, "content B", "迟到结果必须归入发起页");
    }

    #[test]
    fn close_tab_flow_respects_dirty_and_never_empties_tabs() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("dirty page".into())));

        // 置脏页关闭 → 先确认不移除
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.close_tab_confirm, Some(0));
        assert_eq!(app.tabs.len(), 1);

        // 取消：页面原样保留
        dispatch(&mut app, Message::CancelCloseTab);
        assert_eq!(app.close_tab_confirm, None);
        assert_eq!(app.tabs.len(), 1);

        // 确认放弃：清空内容并移除；最后一页被新的空页替代
        dispatch(&mut app, Message::CloseTabRequest);
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
        assert_eq!(app.tabs.len(), 1, "恒保有一个标签页");
        assert!(!app.tab().dirty);
        assert!(app.cur_handle.borrow().doc.is_empty());

        // 干净页关闭即刻生效（两页 → 一页）
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs.len(), 2);
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), 1, "干净页直接关闭");
        assert!(app.close_tab_confirm.is_none());
    }

    #[test]
    fn window_close_confirms_when_any_background_tab_is_dirty() {
        let mut app = Editpad::default();
        // P29 起默认快照直退；本测试钉住「每次询问」模式的聚合确认口径
        app.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("make dirty".into())));
        dispatch(&mut app, Message::NewTab); // 切到干净的新页

        // 活动页干净，但后台页置脏 → 关窗仍需确认（聚合口径）
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(
            app.confirm_visible,
            "任一页置脏都必须弹关窗确认"
        );
    }

    // ---------- P28 标签右键菜单 ----------

    /// 构造 N 页应用：页 0 置脏（内容 "d0"），其余干净；返回应用。
    fn app_with_tabs(n: usize) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d0".into())));
        for _ in 1..n {
            dispatch(&mut app, Message::NewTab);
        }
        app
    }

    #[test]
    fn batch_close_targets_excludes_pinned_and_handles_out_of_range() {
        // 布局：[0]=固定 [1] [2]=固定 [3]
        let mut tabs = vec![Tab::empty(), Tab::empty(), Tab::empty(), Tab::empty()];
        tabs[0].pinned = true;
        tabs[2].pinned = true;

        // 关闭其他(keep=1)：候选 {0,2,3} 排除固定页 0/2 → 仅剩 3
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(1)),
            vec![3],
            "固定页必须被豁免出批量目标"
        );
        // 关闭其他(keep=3)：其余页里只有非固定的页 1 可关
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(3)),
            vec![1],
        );
        // 关闭其他(keep=3) 但除 keep 外全固定 → 空
        tabs[1].pinned = true;
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(3)),
            Vec::<usize>::new(),
        );
        tabs[1].pinned = false;

        // 关闭右侧：只收 from 右侧的非固定页
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(1)),
            vec![3],
            "右侧的固定页 2 必须被豁免"
        );
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(2)),
            vec![3],
        );

        // 越界安全：keep/from 超出范围一律空表（菜单项据此禁用）
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(9)),
            Vec::<usize>::new()
        );
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(9)),
            Vec::<usize>::new()
        );
    }

    // ---------- P39/P40 浮层弹窗 ----------

    /// P43 回归：右键菜单卡片高度随窗口钳制（小窗不溢出、终生不盖满全屏）。
#[test]
fn ctx_menu_card_h_adapts_to_viewport() {
    // 常规窗口（600 高）：内容本身约 260px，卡在常量估高上（上下留边）
    assert_eq!(ctx_menu_card_h(600.0), CTX_MENU_H);
    // 大窗口也不突破估高
    assert_eq!(ctx_menu_card_h(1080.0), CTX_MENU_H);
    // 小窗：高度 = 窗高 − 16 边距（内容超高时内部滚动），不再盖满全屏
    assert_eq!(ctx_menu_card_h(200.0), 184.0);
    assert_eq!(ctx_menu_card_h(100.0), 84.0);
    // 未知/极小窗口：回退保守估高（首帧未收到 Resized 的兜底）
    assert_eq!(ctx_menu_card_h(0.0), CTX_MENU_H);
    assert_eq!(ctx_menu_card_h(40.0), CTX_MENU_H);
    // 与锚点钳制联用：卡片恒在窗口内（小窗下 y 夹 0，不越下缘）
    let (_, ay) = clamp_menu_anchor((100.0, 500.0), (800.0, 120.0), CTX_MENU_W, ctx_menu_card_h(120.0));
    assert!(ay + ctx_menu_card_h(120.0) <= 120.0, "锚点+高度必须落在窗内");
}

#[test]
    fn clamp_menu_anchor_keeps_overlay_inside_window() {
        // 常规位置不钳制
        assert_eq!(
            clamp_menu_anchor((100.0, 100.0), (800.0, 600.0), 200.0, 280.0),
            (100.0, 100.0)
        );
        // 右缘：x 拉回 窗宽−菜单宽（等效于主流编辑器的向左翻开）
        assert_eq!(
            clamp_menu_anchor((750.0, 100.0), (800.0, 600.0), 200.0, 280.0),
            (600.0, 100.0)
        );
        // 下缘：y 拉回 窗高−菜单高
        assert_eq!(
            clamp_menu_anchor((100.0, 500.0), (800.0, 600.0), 200.0, 280.0),
            (100.0, 320.0)
        );
        // 窗口尺寸未知 (0,0) → 不钳制（启动后首个 Resized 事件校准）
        assert_eq!(
            clamp_menu_anchor((750.0, 500.0), (0.0, 0.0), 200.0, 280.0),
            (750.0, 500.0)
        );
        // 小窗整体夹 0；负坐标夹 0
        assert_eq!(
            clamp_menu_anchor((50.0, 40.0), (100.0, 80.0), 200.0, 280.0),
            (0.0, 0.0)
        );
        assert_eq!(
            clamp_menu_anchor((-5.0, -5.0), (800.0, 600.0), 200.0, 280.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn context_menu_anchor_recorded_from_cursor_and_viewport_tracked() {
        let mut app = app_with_tabs(1);
        // 窗口逻辑尺寸经 Resized 事件入账（浮层贴边钳制依据）
        dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
        assert_eq!(app.viewport_size, (1024.0, 768.0));

        // 指针位置先于右键到达 → 菜单锚点 = 打开那一刻的指针位置
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(300.0, 200.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, Some(0));
        assert_eq!(app.menu_anchor, (300.0, 200.0));

        // 进入 busy（对话框/IO 互斥）必须收起已开的浮层：背板挡整窗不能留
        app.enter_busy();
        assert_eq!(app.tab_context_menu, None);

        // busy 期间右键拒开新菜单，锚点也不被改写（下次打开取届时位置）
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(10.0, 10.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, None);
        assert_eq!(app.menu_anchor, (300.0, 200.0));
    }

    #[test]
    fn view_builds_with_floating_overlays_open() {
        // 无头冒烟：菜单浮层、设置浮层、两者叠加时视图树均可构造
        // （Stack + opaque + 背板 mouse_area 的组合在构造期不 panic）
        let mut app = app_with_tabs(2);
        dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(40.0, 40.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        let _ = app.view();
        dispatch(&mut app, Message::SettingsToggled);
        let _ = app.view();
    }

    #[test]
    fn context_menu_opens_closes_and_respects_busy_guard() {
        // 注意：app_with_tabs(2) 的活动页在下标 1（NewTab 会自动切换）
        let mut app = app_with_tabs(2);

        // 右键第 0 页 → 菜单打开并指向该页
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, Some(0));

        // 左键切到另一页（0 ≠ 当前活动页 1）→ 菜单随切换收起
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.tab_context_menu, None);

        // Esc 收起（BarsDismissed 一并清批量确认态）
        dispatch(&mut app, Message::TabContextMenu(1));
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.tab_context_menu, None);

        // busy 中右键不开菜单
        app.busy = true;
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, None, "busy 时不得打开右键菜单");
        app.busy = false;

        // × 按钮 / 显式收起消息
        dispatch(&mut app, Message::TabContextMenu(0));
        dispatch(&mut app, Message::TabContextMenuClosed);
        assert_eq!(app.tab_context_menu, None);

        // 已被关掉的页下标：忽略不 panic
        dispatch(&mut app, Message::CloseTabAt(1));
        dispatch(&mut app, Message::TabContextMenu(5));
        assert_eq!(app.tab_context_menu, None);
    }

    #[test]
    fn close_tab_at_routes_clean_dirty_and_pinned_pages() {
        let mut app = app_with_tabs(2);

        // 干净后台页：即刻关闭
        dispatch(&mut app, Message::CloseTabAt(1));
        assert_eq!(app.tabs.len(), 1, "干净页应被立即关闭");

        // 置脏页：转既有单页确认条（含保存并关闭出口）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::SwitchTab(0));
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.close_tab_confirm, Some(0), "置脏页应弹关闭确认条");
        dispatch(&mut app, Message::CancelCloseTab);

        // 固定页：拒绝关闭并留痕状态栏
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(app.tabs[0].pinned, "固定开关应翻转");
        let len_before = app.tabs.len();
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.tabs.len(), len_before, "固定页不得被关闭");
        assert!(
            app.status.contains("取消固定"),
            "拒绝原因应写入状态栏，实际:{}",
            app.status
        );
        // 再翻转一次恢复非固定；Ctrl+W 对固定页同样豁免（键盘路径一致性）
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(!app.tabs[0].pinned);
        dispatch(&mut app, Message::TogglePinTab(0));
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), len_before, "Ctrl+W 不得绕过固定豁免");
        assert!(app.status.contains("取消固定"));
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(!app.tabs[0].pinned);
    }

    #[test]
    fn close_other_tabs_aggregates_dirty_confirm_and_spares_pinned() {
        // 页 0 固定+置脏；页 1 干净；页 2 置脏。在页 1 上「关闭其他」。
        let mut app = app_with_tabs(3);
        dispatch(&mut app, Message::TogglePinTab(0)); // 页 0 固定（仍置脏）
        dispatch(&mut app, Message::SwitchTab(2));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d2".into())));
        dispatch(&mut app, Message::SwitchTab(1));

        dispatch(&mut app, Message::CloseOtherTabs(1));
        // 目标 = 非固定且非 keep = 仅 {2}（页 0 因固定被豁免）；
        // 含置脏页 → 弹聚合确认而非直接关
        let targets = app.batch_close_confirm.clone().expect("应弹聚合确认");
        assert_eq!(targets, vec![2], "固定页必须被豁免出批量目标");

        // 取消：一切原样
        dispatch(&mut app, Message::CancelBatchCloseTabs);
        assert_eq!(app.tabs.len(), 3);
        assert!(app.tabs[2].dirty);

        // 再次确认放弃：仅移除目标页，固定页与 keep 页幸存
        dispatch(&mut app, Message::CloseOtherTabs(1));
        dispatch(&mut app, Message::ConfirmBatchCloseDiscard);
        assert_eq!(app.tabs.len(), 2, "只有目标页被移除");
        assert!(app.tabs[0].pinned && app.tabs[0].dirty, "固定置脏页原样幸存");
        assert_eq!(app.batch_close_confirm, None);
        assert_eq!(app.tab_context_menu, None, "选中菜单项后菜单应收起");

        // 幸存两页全干净后「关闭其他」= 无置脏目标直接关，不再弹确认
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0)); // 显式放弃固定置脏页
        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert!(app.batch_close_confirm.is_none(), "全部干净不应弹确认");
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn close_right_tabs_only_touches_right_side_non_pinned() {
        // 4 页（页 0 置脏，其余干净）：页 2 固定。「在页 0 上关闭其他」
        // → 目标 = {1, 3} 全干净 → 立即移除，固定页 2 与 keep 页 0 幸存。
        let mut app = app_with_tabs(4);
        dispatch(&mut app, Message::TogglePinTab(2));

        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert!(app.batch_close_confirm.is_none());
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[1].pinned, "原页 2（现下标 1）仍固定且幸存");

        // 重验 CloseTabsRight 的右侧口径与置脏聚合：
        // 4 页，页 0 干净、页 1 置脏。
        let mut app = Editpad::default();
        dispatch(&mut app, Message::NewTab); // 挤掉初始空净页的就地打开优惠
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d1".into())));
        dispatch(&mut app, Message::SwitchTab(0));

        dispatch(&mut app, Message::CloseTabsRight(0));
        let targets = app.batch_close_confirm.clone().expect("含置脏页应弹确认");
        assert_eq!(targets, vec![1, 2, 3], "右侧全部非固定页入列");
        dispatch(&mut app, Message::CancelBatchCloseTabs);
        assert_eq!(app.tabs.len(), 4);

        // 清掉置脏的右侧页后，右侧关闭立即生效
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(1));
        dispatch(&mut app, Message::CloseTabsRight(0));
        assert!(app.batch_close_confirm.is_none());
        assert_eq!(app.tabs.len(), 1, "右侧干净页应被直接移除");
    }

    #[test]
    fn menu_save_and_rename_route_through_active_tab_flow() {
        let mut app = Editpad::default();

        // 页 0 命名并置脏；页 1 新建未命名
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((editpad_core::Document::from_str("A"), String::new(), "UTF-8".to_owned())),
            ),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("change".into())));
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.active_tab, 1);

        // 菜单「保存」后台命名置脏页：先切页再走活动页保存流（busy 进入保存任务）
        dispatch(&mut app, Message::SaveTabFromMenu(0));
        assert_eq!(app.active_tab, 0, "菜单保存应先切到目标页");
        assert!(app.busy, "应进入既有保存流的 busy 守卫");
        app.busy = false;

        // 干净页的菜单保存：no-op（与工具栏「保存」同口径，按钮侧禁用）
        dispatch(&mut app, Message::SaveTabFromMenu(1));
        assert!(!app.busy, "干净页保存应为 no-op");

        // 菜单「重命名」= 切页 + 另存为对话框阶段
        dispatch(&mut app, Message::RenameOrSaveAsTab(1));
        assert_eq!(app.active_tab, 1, "重命名应切到目标页");
        assert!(app.busy, "另存为对话框阶段应置 busy");
        app.busy = false;

        // 未命名置脏页的菜单保存自动落另存为（Ctrl+S 同语义）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().path.is_none() && app.tab().dirty);
        let active = app.active_tab;
        dispatch(&mut app, Message::SaveTabFromMenu(active));
        assert!(app.busy, "未命名页保存应转入另存为对话框");
    }

    #[test]
    fn batch_close_never_empties_tabs_and_keeps_invariants() {
        // 两页全部固定：在页 0 上「关闭其他」→ 无可关目标 no-op。
        let mut app = app_with_tabs(2);
        dispatch(&mut app, Message::TogglePinTab(0));
        dispatch(&mut app, Message::TogglePinTab(1));
        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert_eq!(app.tabs.len(), 2, "无可关目标应保持原样");
        assert!(app.batch_close_confirm.is_none());

        // 单页应用上关闭其他/右侧：同样 no-op
        let mut solo = Editpad::default();
        dispatch(&mut solo, Message::CloseOtherTabs(0));
        dispatch(&mut solo, Message::CloseTabsRight(0));
        assert_eq!(solo.tabs.len(), 1);

        // close_tabs_now 兜底：移除最后一页时重置新空页（tabs 恒非空不变式）
        let mut bare = Editpad::default();
        let removed = bare.close_tabs_now(&[0]);
        assert_eq!(removed, 1);
        assert_eq!(bare.tabs.len(), 1, "移除最后一页应重置新空页");
        assert!(!bare.tab().dirty);
        assert!(bare.cur_handle.borrow().doc.is_empty());
        // 越界/重复下标安全
        assert_eq!(bare.close_tabs_now(&[5]), 0);
        assert_eq!(bare.close_tabs_now(&[0, 0]), 1);
    }

    // ---------- P32 最近文件光标/滚动记忆 ----------

    #[test]
    fn reopened_recent_file_restores_cursor_and_scroll() {
        let mut app = Editpad::default();
        // 文档要足够大（默认视口 ~400px 高可容纳全部行时，
        // restore_view 的行程钳制会把小滚动值顶到 max+1——P30 设计语义）
        let mut text = String::new();
        for i in 1..=60 {
            text.push_str(&format!("line{i}\n"));
        }
        let doc = || editpad_core::Document::from_str(&text);

        // 首开 C:/a.txt：进最近列表（尚无记忆）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq1, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.settings.recent_view(Path::new("C:/a.txt")).is_none());

        // 种入上次退出时的位置（模拟历史记忆）
        assert!(app.settings.set_recent_view(
            Path::new("C:/a.txt"),
            editpad_core::RecentView { line: 2, col: 1, scroll_top: 5.0 },
        ));

        // 重开同一文件（当前页已命名非空 → 落新页并聚焦）→ 光标滚动就位
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq2, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        let ed = app.cur_handle.borrow();
        assert_eq!(
            ed.cursor,
            editor::CursorPos { line: 2, col: 1 },
            "光标应回到记忆位置"
        );
        assert_eq!(ed.scroll_top, 5.0, "垂直滚动应回到记忆位置");
    }

    #[test]
    fn closing_named_tab_writes_back_cursor_memory_to_disk() {
        let dir = scratch_dir("p32-writeback");
        let mut app = Editpad::default();
        // 注入配置路径：验证回写真的落盘，且绝不碰真实 %APPDATA%
        app.settings_path_override = Some(dir.join("config.toml"));

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("r0\nr1\nr2\nr3"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 光标挪到第 3 行（jump_to_line 1 基 → 0 基 line=2），页面干净可直接关
        app.cur_handle.borrow_mut().jump_to_line(3);
        assert!(!app.tab().dirty);
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.tabs.len(), 1, "干净命名页应被关闭");

        let recorded = app.settings.recent_view(Path::new("C:/b.txt"));
        assert_eq!(
            recorded.map(|v| v.line),
            Some(2),
            "关页必须把光标行（0 基）写回记忆"
        );
        // 注入路径端到端：重载 config.toml 记忆仍在
        let persisted = editpad_core::Settings::load_from(&dir.join("config.toml"));
        assert_eq!(persisted.recent_view(Path::new("C:/b.txt")).map(|v| v.line), Some(2));

        fs_remove_dir_all(&dir);
    }

    #[test]
    fn window_close_writes_back_all_named_tabs_views() {
        let mut app = Editpad::default();
        let dir = scratch_dir("p32-windowclose");
        app.settings_path_override = Some(dir.join("config.toml"));

        // 两页各开一个文件，光标停在不同行
        for (name, jump) in [("C:/w1.txt", 1usize), ("C:/w2.txt", 3usize)] {
            dispatch(&mut app, Message::FileDropped(PathBuf::from(name)));
            let seq = app.job_seq;
            dispatch(
                &mut app,
                Message::Loaded(
                    seq,
                    Ok((
                        editpad_core::Document::from_str("a\nb\nc\nd\ne"),
                        String::new(),
                        "UTF-8".to_owned(),
                    )),
                ),
            );
            if jump > 1 {
                app.cur_handle.borrow_mut().jump_to_line(jump);
            }
        }
        assert_eq!(app.tabs.len(), 2);

        // 关窗请求（快照直退默认路径；目录传 None 走降级不影响断言）：
        // 入口处必须先把两页的光标都记账
        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, None);

        assert_eq!(
            app.settings.recent_view(Path::new("C:/w1.txt")).map(|v| v.line),
            Some(0),
            "未跳转的页记文首"
        );
        assert_eq!(
            app.settings.recent_view(Path::new("C:/w2.txt")).map(|v| v.line),
            Some(2),
            "跳转过光标的页按 0 基记账"
        );

        fs_remove_dir_all(&dir);
    }

    /// std::fs::remove_dir_all 的薄封装（测试尾部清理）。
    fn fs_remove_dir_all(dir: &Path) {
        std::fs::remove_dir_all(dir).ok();
    }

    // ---------- P35 打开落新页的别名失步（P32 本轮发现） ----------

    #[test]
    fn opening_into_new_tab_keeps_editor_alias_in_sync() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq_a = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq_a,
                Ok((
                    editpad_core::Document::from_str("content A"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 打开 B：当前页已命名 → 落新页 idx1 并「聚焦」（active_tab 切过去）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq_b = app.job_seq;
        assert_eq!(app.active_tab, 1, "打开即聚焦新页");
        dispatch(
            &mut app,
            Message::Loaded(
                seq_b,
                Ok((
                    editpad_core::Document::from_str("content B"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 加载完成后直接输入（无任何显式切换）：必须落在聚焦的新页。
        // 失步缺陷下 cur_handle 仍指页 0——敲字打进上一个文档。
        // （打开文件后光标在文首 0:0，插入落在开头是既有正确语义；
        //  本测试钉住的是「落进哪一页」，不是行内位置。）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("+X".into())));
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "+Xcontent B",
            "输入必须落在聚焦的新页"
        );
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "content A",
            "旧页不得被误改"
        );
        // 渲染源与活动页一致
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "+Xcontent B");
    }

    #[test]
    fn preview_toggle_only_flips_for_markdown_documents() {
        // .md 扩展名经别名层得到 Markdown 语法 → 开关生效
        let mut md = Editpad::default();
        dispatch(&mut md, Message::FileDropped(PathBuf::from("C:/doc/readme.md")));
        let seq = md.job_seq;
        dispatch(
            &mut md,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("# 标题\n正文"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert!(!md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(!md.preview_visible);

        // 非 Markdown 页：不翻转并提示
        let mut app = json_app("{}");
        dispatch(&mut app, Message::PreviewToggled);
        assert!(!app.preview_visible);
        assert!(app.status.contains("仅支持 Markdown"), "{:?}", app.status);
    }

    // ---------- P24/P25 手测反馈修复 ----------

    #[test]
    fn untitled_tabs_get_unique_sequential_names() {
        let mut app = Editpad::default();
        // 初始页即「未命名1」
        assert_eq!(app.tab().base_name(), "未命名1");
        assert_eq!(app.title(), "未命名1 - Editpad");

        // 连开两页：未命名2、未命名3，全局唯一
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        let names: Vec<String> =
            app.tabs.iter().map(|t| t.base_name()).collect();
        assert_eq!(names, vec!["未命名1", "未命名2", "未命名3"]);

        // 置脏前缀进标签名但不进 title 的基础名判断之外重复
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().display_name().starts_with("● 未命名"));
    }

    #[test]
    fn closed_untitled_numbers_are_never_reused_and_saving_clears_them() {
        let mut app = Editpad::default();

        // 关闭「未命名2」（干净直接关）后新建：号码不复用，拿「未命名3」
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs[1].base_name(), "未命名2");
        app.set_active_tab(1);
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), 1);

        dispatch(&mut app, Message::NewTab);
        assert_eq!(
            app.tabs[1].base_name(),
            "未命名3",
            "单调分配杜绝重名"
        );

        // 另存为转正后序号清除，标签显示真实文件名
        dispatch(
            &mut app,
            Message::SaveTargetChosen(Some(PathBuf::from("C:/x/real.txt"))),
        );
        assert_eq!(
            app.tabs[1].path.as_deref(),
            Some(Path::new("C:/x/real.txt"))
        );
        assert_eq!(app.tabs[1].untitled_num, None);
        assert_eq!(app.tabs[1].base_name(), "real.txt");

        // 加载真实文件同样清除序号（Loaded 路径）
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/r/a.md")));
        let seq = app2.job_seq;
        dispatch(
            &mut app2,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("x"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app2.tabs[0].untitled_num, None);
        assert_eq!(app2.tabs[0].base_name(), "a.md");
    }

    #[test]
    fn tab_keymap_new_next_close() {
        use iced::keyboard::{self};
        let ctrl = keyboard::Modifiers::CTRL;

        assert!(matches!(
            handle_key(keyboard::Key::Character("t".into()), ctrl),
            Some(Message::NewTab)
        ));
        assert!(matches!(
            handle_key(keyboard::Key::Character("w".into()), ctrl),
            Some(Message::CloseTabRequest)
        ));
        assert!(matches!(
            handle_key(keyboard::Key::Named(Named::Tab), ctrl),
            Some(Message::SwitchTabNext)
        ));
    }

    // ---------- 健壮性边界用例批 ----------

    #[test]
    fn duplicate_open_while_loading_is_guards_and_reload_after_done() {
        // 加载进行中再次拖入同一文件：busy 守卫必须拒绝重入（防双任务竞态）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert!(app.active_load.is_some(), "首次拖入应登记加载任务");
        let seq_first = app.job_seq;

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(app.job_seq, seq_first, "加载中不得排队第二个任务");

        // 加载完成后允许重复打开同一文件（每次新 job，无路径去重）
        let doc = editpad_core::Document::from_str("reloaded\n");
        dispatch(
            &mut app,
            Message::Loaded(seq_first, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none(), "完成后任务应解除");
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(
            app.job_seq,
            seq_first + 1,
            "完成后的重复打开必须作为新任务重新加载"
        );
    }

    #[test]
    fn close_during_load_exits_without_confirm_bar_or_stuck_state() {
        // 加载中关窗：dirty 必为 false（打开确认已清），应走直接关窗路径，
        // 不弹未保存确认条、不残留 pending 状态。
        // P29：快照直退路径有专门测试（注入目录），此处关掉开关钉住降级行为，
        // 同时避免测试触碰真实 %APPDATA%。
        let mut app = Editpad::default();
        app.settings.enable_snapshots = false;
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/big.log")));
        assert!(app.active_load.is_some());
        assert!(!app.tab().dirty);

        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert_eq!(app.main_window, Some(id), "窗口 id 应被捕获供 close 使用");
        assert!(
            !app.confirm_visible && !app.pending_close,
            "加载中关窗不得触发确认条"
        );
        // 关窗后迟到的加载完成消息照常按 job 过滤消化，不 panic
        let doc = editpad_core::Document::from_str("late arrival");
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none());
    }

    // ---------- P29 会话快照直退（关窗状态机） ----------

    /// 项目内落盘目录：快照测试绝不触碰真实 %APPDATA%。
    fn snapshot_scratch_dir(tag: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("app-p29-{tag}-{}", std::process::id()))
    }

    #[test]
    fn snapshot_exit_writes_session_and_closes_without_confirm() {
        let dir = snapshot_scratch_dir("exit");
        let mut app = Editpad::default();

        // 页 0：未命名页，已编辑置脏（未命名页正是 auto-save 的盲区）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿内容\r\n第二行".into())));
        // 页 1：命名干净页——只记元数据不写内容文件
        let mut named = Tab::empty();
        named.path = Some(PathBuf::from("C:/work/notes.md"));
        app.tabs.push(named);

        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, Some(dir.clone()));

        assert!(!app.confirm_visible, "快照直退零询问");
        assert_eq!(app.main_window, Some(id));

        // 清单完整落盘且可解析
        let manifest = editpad_core::snapshot::read_manifest(&dir)
            .expect("关窗后必须有可解析的会话清单");
        assert!(manifest.clean_exit, "正常退出必须带收尾标记");
        assert_eq!(manifest.tabs.len(), 2);
        assert_eq!(manifest.active, 0);
        assert_eq!(manifest.next_untitled, app.untitled_next);

        // 置脏未命名页：序号延续 + 内容逐字回来
        // （插入文本经 P9 归一为文档主导行尾 LF;CRLF 保真已由 core 测试钉住）
        let t0 = &manifest.tabs[0];
        assert!(t0.dirty);
        assert_eq!(t0.path, None);
        assert_eq!(t0.untitled_num, Some(1));
        let doc = editpad_core::snapshot::read_page(&dir, t0).expect("置脏页必须有快照");
        assert_eq!(doc.to_text(), "草稿内容\n第二行");

        // 干净命名页：路径记住、无内容文件
        let t1 = &manifest.tabs[1];
        assert!(!t1.dirty);
        assert_eq!(t1.path.as_deref(), Some("C:/work/notes.md"));
        assert_eq!(t1.file, None);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn snapshot_exit_captures_cursor_and_scroll_positions() {
        let dir = snapshot_scratch_dir("cursor");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("a\nb\nc\n".into())));
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.jump_to_line(3); // 跳到末行
            ed.scroll_top = 7.5;
            ed.scroll_left = 2.0;
        }
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        let t0 = &manifest.tabs[0];
        assert_eq!(t0.cursor_line, 2, "光标行（0 基）必须被记录");
        assert!((t0.scroll_top - 7.5).abs() < f32::EPSILON);
        assert!((t0.scroll_left - 2.0).abs() < f32::EPSILON);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn disabled_or_ask_mode_falls_back_to_confirm_bar_without_writes() {
        let dir = snapshot_scratch_dir("fallback");
        fs::create_dir_all(&dir).unwrap();

        // 开关关闭 + 置脏 → 旧确认条，且磁盘上什么都没写
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        dispatch(&mut off, Message::Edit(EditOp::InsertText("x".into())));
        let _ = off.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(off.confirm_visible, "开关关闭必须回退旧确认条");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "降级路径不得写任何快照"
        );

        // 模式为「每次询问」+ 置脏 → 同样回退
        let mut ask = Editpad::default();
        ask.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut ask, Message::Edit(EditOp::InsertText("y".into())));
        let _ = ask.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(ask.confirm_visible);
        assert!(editpad_core::snapshot::read_manifest(&dir).is_none());

        // 目录不可用（None）→ 功能自动降级，同样回退确认条
        let mut nodir = Editpad::default();
        dispatch(&mut nodir, Message::Edit(EditOp::InsertText("z".into())));
        let _ = nodir.handle_close_request(iced::window::Id::unique(), None);
        assert!(nodir.confirm_visible, "无快照目录时必须降级而不是丢数据");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_failure_degrades_to_confirm_bar_with_status_reason() {
        // 用一个普通文件冒充快照目录 → create_dir_all 必败
        let dir = snapshot_scratch_dir("fail");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, "").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText(" precious ".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        assert!(app.confirm_visible, "快照失败必须降级回确认条保住数据");
        assert!(
            app.status.contains("会话快照失败"),
            "失败原因要留在状态栏:{:?}",
            app.status
        );
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "失败的提交不得产生清单"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discard_and_close_clears_snapshot_area() {
        let dir = snapshot_scratch_dir("discard");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("will discard".into())));
        // 先正常快照一次（模拟上一轮退出留下的会话记录）
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(editpad_core::snapshot::read_manifest(&dir).is_some());

        // 新会话里用户选择「放弃更改」→ 快照区必须一并清空，
        // 否则下次启动会把已放弃的内容当会话恢复回来
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("new session".into())));
        let _ = app2.discard_all_and_close(Some(dir.clone()));
        assert!(!app2.confirm_visible && !app2.any_dirty());
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "放弃语义 = 连快照一起丢"
        );
        assert!(!dir.exists(), "清场应删除整个快照目录");

        // 开关关闭时放弃不清场（本来就没有快照承诺）
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        let _ = off.discard_all_and_close(Some(dir.clone()));
        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P30 启动会话恢复 ----------

    /// 构造一个干净命名页的清单元数据（测试辅助）。
    fn clean_named_tab(path: &str) -> editpad_core::snapshot::SessionTab {
        editpad_core::snapshot::SessionTab {
            path: Some(path.to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
        }
    }

    #[test]
    fn session_restore_rebuilds_tabs_content_active_and_pending_loads() {
        let dir = snapshot_scratch_dir("p30-restore");

        // 准备上一轮正常退出留下的会话：置脏未命名页（内容+光标）+ 干净命名页
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(4),
            dirty: true,
            file: None,
            cursor_line: 1,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/notes.md".to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 10,
            cursor_col: 3,
            scroll_top: 50.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("第一行\r\n草稿内容"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            7,
        )
        .expect("写会话应成功");
        assert!(manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 0, "激活页按清单还原");
        assert!(app.recover_prompt.is_none(), "clean_exit=true 静默恢复，不弹提示条");

        // 置脏未命名页：内容逐字还原 + 光标就位 + 编号延续
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "第一行\r\n草稿内容");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(4));
        let ed0 = app.tabs[0].editor.borrow();
        assert_eq!((ed0.cursor.line, ed0.cursor.col), (1, 2), "光标回到退出时位置");
        drop(ed0);
        assert_eq!(app.untitled_next, 7, "未命名编号从清单延续单调性");

        // 干净命名页：占位页已建、加载任务已自动开跑（不抢焦点）
        assert!(app.restore_placeholder_ready(1));
        let job = app.active_load.as_ref().expect("命名页应立即开载").clone();
        assert_eq!(job.tab, 1);
        assert!(app.busy);
        assert_eq!(app.active_tab, 0, "恢复链绝不把用户拽到加载页");

        // 加载完成：内容回填 + 视图回填 + 收尾
        let body = editpad_core::Document::from_str(&"line\n".repeat(30));
        dispatch(
            &mut app,
            Message::Loaded(job.id, Ok((body, String::new(), "UTF-8".to_owned()))),
        );
        assert_eq!(app.tabs[1].path.as_deref(), Some(Path::new("C:/w/notes.md")));
        assert!(!app.tabs[1].dirty);
        let ed1 = app.tabs[1].editor.borrow();
        assert_eq!(
            (ed1.cursor.line, ed1.cursor.col),
            (10, 3),
            "加载完成的命名页同样回填光标"
        );
        assert!(
            ed1.scroll_top > 0.0 && ed1.scroll_top <= 50.0,
            "滚动值被应用且经行程钳制，实际 {}",
            ed1.scroll_top
        );
        drop(ed1);
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0, "恢复链排空");
        assert!(app.status.is_empty(), "全部成功的恢复不打扰状态栏");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// P43 回归：新建标签页唯一出口必须套用全局字号设置
    /// （曾有多处直接 `Tab::empty()`，编辑器默认 16px 脱离设置）。
    #[test]
    fn fresh_tab_carries_global_font_size() {
        let mut app = Editpad::default();
        app.settings.font_size = 22.0;
        let tab = app.fresh_tab();
        assert_eq!(tab.editor.borrow().font_size(), 22.0);

        // 归一规则与 boot 同源：非法（超界/负）值收敛到合法区间
        let mut app = Editpad::default();
        app.settings.font_size = 9999.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            editor::normalize_font_size(9999.0)
        );
        let mut app = Editpad::default();
        app.settings.font_size = -5.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            editor::normalize_font_size(-5.0)
        );
    }

    /// P43 回归：会话恢复（P30）创建的标签页必须套用全局字号——
    /// 曾恢复路径整体替换 tabs，boot 只给恢复前默认页应用过字号，
    /// 表现为「字号调到 24 生效，重启后字体回到原大小，设置面板仍 24」。
    #[test]
    fn restored_tabs_carry_global_font_size() {
        let dir = snapshot_scratch_dir("p43-restore-font-size");
        let tab = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("待恢复的草稿"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        app.settings.font_size = 24.0;
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().font_size(),
            24.0,
            "恢复页必须应用全局字号设置"
        );
        assert_eq!(app.settings.font_size, 24.0, "设置本身不受影响");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn dirty_named_tab_restores_snapshot_instead_of_disk_reload() {
        let dir = snapshot_scratch_dir("p30-dirty-named");
        let tab = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/doc.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 4,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("磁盘上没有的未保存内容"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "磁盘上没有的未保存内容",
            "v1 一律信快照（所见即所得），绝不从磁盘重载覆盖未存改动"
        );
        assert!(app.tabs[0].dirty, "恢复出的置脏页保持置脏");
        assert_eq!(app.tabs[0].encoding_label, "UTF-8", "快照恒为 UTF-8 落盘");
        assert!(app.active_load.is_none(), "快照同步还原，无需排队加载");
        assert!(app.status.is_empty());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restored_dirty_pages_stay_dirty_when_undo_revisits_snapshot_state() {
        // P38 × P30 交互回归：恢复出来的置脏页不得把快照内容当落盘基线，
        // 否则「编辑 → 撤销回快照态」会错误清脏 → 零询问退出后下次启动
        // 命名页从磁盘旧内容重载、未命名页直接空白——未存改动静默丢失。
        let dir = snapshot_scratch_dir("p38-restore-baseline");
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/report.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("草稿"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::from_str("磁盘上没有的修改"),
                },
            ],
            0,
            4,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[0].dirty && app.tabs[1].dirty);

        // 未命名页：编辑一步再撤销回快照态——必须保持置脏（无基线保守）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        assert!(app.tabs[0].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(
            app.tabs[0].dirty,
            "撤销回快照态不清脏：内容从未落盘（P30 恢复页基线已清空）"
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "草稿");

        // 置脏命名页同理：磁盘还是旧内容，快照态不算「已保存」
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
        assert!(app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(app.tabs[1].dirty, "命名恢复页撤销回快照态同样保持置脏");
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "磁盘上没有的修改"
        );

        // 真正落盘一次后基线重建，撤销回清能力恢复正常语义
        let v = app.tabs[1].version;
        dispatch(&mut app, Message::TabAutosaved(1, v, Ok(())));
        assert!(!app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.tabs[1].dirty, "落盘之后撤销回基线应正常回清");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// 在指定目录种一个「异常退出中间态」会话（clean_exit=false，
    /// 即 P31 心跳写的清单形态）：手工落一份可解析 TOML + 真实页文件。
    fn plant_orphan_session(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("s7-t0.snap"), "崩溃前的未保存草稿").unwrap();
        fs::write(
            dir.join(editpad_core::snapshot::MANIFEST_NAME),
            concat!(
                "generation = 7\n",
                "active = 0\n",
                "next_untitled = 3\n",
                "clean_exit = false\n",
                "\n",
                "[[tabs]]\n",
                "untitled_num = 2\n",
                "dirty = true\n",
                "file = \"s7-t0.snap\"\n",
                "cursor_line = 0\n",
                "cursor_col = 3\n",
            ),
        )
        .unwrap();
    }

    #[test]
    fn crash_orphan_manifest_prompts_then_restores_or_discards() {
        // 「恢复」路径：弹一次性提示条 → 接受后全量重建（含置脏内容）
        let dir_a = snapshot_scratch_dir("p30-orphan-a");
        plant_orphan_session(&dir_a);
        let manifest = editpad_core::snapshot::read_manifest(&dir_a).unwrap();
        assert!(!manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_dir(&dir_a);
        assert!(app.recover_prompt.is_some(), "异常退出必须弹一次性恢复条");
        assert_eq!(app.tabs.len(), 1, "未裁决前不得动当前标签");

        let _ = app.accept_session_recover(Some(dir_a.clone()));
        assert!(app.recover_prompt.is_none());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "崩溃前的未保存草稿");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(2), "编号延续");
        assert_eq!(app.untitled_next, 3, "计数器取清单的 next_untitled");
        editpad_core::snapshot::clear_session(&dir_a);

        // 「丢弃」路径：连快照一起丢（P29 放弃语义同族），空白起步
        let dir_b = snapshot_scratch_dir("p30-orphan-b");
        plant_orphan_session(&dir_b);
        let mut app2 = Editpad::default();
        let _ = app2.restore_from_dir(&dir_b);
        assert!(app2.recover_prompt.is_some());
        let _ = app2.discard_session_recover(Some(dir_b.clone()));
        assert!(app2.recover_prompt.is_none());
        assert!(
            editpad_core::snapshot::read_manifest(&dir_b).is_none(),
            "丢弃语义必须清场，防下次启动复活已弃内容"
        );
        fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn remember_session_disabled_means_blank_start_and_ask_style_close() {
        // 总开关判定纯函数：两个开关都开才允许
        assert!(session_restore_allowed(true, true));
        assert!(!session_restore_allowed(false, true), "快照总开关是前提");
        assert!(!session_restore_allowed(true, false), "关闭会话记忆 = 不恢复也不写清单");

        // 退出侧：remember_session 关闭时关窗回退旧确认条，不写任何会话数据
        let dir = snapshot_scratch_dir("p30-gate");
        let mut app = Editpad::default();
        app.settings.remember_session = false;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("keep me".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(app.confirm_visible, "回退旧确认条而不是静默丢改动");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "记住会话关闭后退出不写清单（§3 P30 第 4 条）"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_skips_unloadable_page_and_keeps_chain_going() {
        let dir = snapshot_scratch_dir("p30-fail");
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/gone.txt"),
                    doc: editpad_core::Document::new(),
                },
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/stays.txt"),
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.restore_pending, 2);
        let seq_a = app.active_load.as_ref().unwrap().id;

        // 第一页加载失败（文件已被删等）：占位移除、失败计数、
        // 续排同步完成——B 已顶上载通道，队列排空
        dispatch(&mut app, Message::Loaded(seq_a, Err("文件不存在".to_owned())));
        assert_eq!(app.tabs.len(), 1, "失败占位页应被移除（单页失败不阻断）");
        assert_eq!(app.restore_failed, 1);
        assert!(
            app.restore_queue.is_empty(),
            "续排是同步的：下一页应已登记为在途任务"
        );

        // 第二页自动续排并成功落地
        let job_b = app.active_load.as_ref().expect("失败后必须续排下一页").clone();
        assert_eq!(job_b.tab, 0, "队列下标必须随页面移除整体左移");
        dispatch(
            &mut app,
            Message::Loaded(
                job_b.id,
                Ok((
                    editpad_core::Document::from_str("b body"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "b body");
        assert_eq!(app.tabs[0].path.as_deref(), Some(Path::new("C:/w/stays.txt")));
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0);
        assert!(
            app.status.contains("会话恢复完成") && app.status.contains("1 页未能恢复"),
            "失败页要有状态栏汇总:{:?}",
            app.status
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restore_plan_prioritizes_active_page_and_truncates_over_cap() {
        let dir = snapshot_scratch_dir("p30-plan");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f100.snap"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("f200.snap"), vec![0u8; 200]).unwrap();

        // t0 = 干净命名页（估算读其磁盘文件 100B）；t1 = 置脏页（快照 200B）；
        // t2 = 干净未命名页（空文档，0B）
        let manifest = |active: usize| editpad_core::snapshot::SessionManifest {
            generation: 1,
            tabs: vec![
                clean_named_tab(&dir.join("f100.snap").display().to_string()),
                editpad_core::snapshot::SessionTab {
                    file: Some("f200.snap".to_owned()),
                    dirty: true,
                    ..clean_named_tab("")
                },
                editpad_core::snapshot::SessionTab {
                    untitled_num: Some(5),
                    ..clean_named_tab("")
                },
            ],
            active,
            next_untitled: 6,
            clean_exit: true,
        };

        // 宽松上限：全保
        let (kept, dropped) = plan_restore_order(&manifest(0), &dir, u64::MAX);
        assert_eq!((kept, dropped), (vec![0, 1, 2], 0));

        // 250B 上限、激活页是 200B 快照页：先保激活页 → 100B 页出局
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 250);
        assert_eq!((kept, dropped), (vec![1, 2], 1));

        // 150B 上限、激活页是 200B 快照页：激活页自身超限同样出局
        // （「先保激活页」是装填优先级，不是护栏豁免权），
        // 随后 100B 页与空页照常装填
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 150);
        assert_eq!((kept, dropped), (vec![0, 2], 1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loaded_language_routes_through_alias_and_sniff() {
        // P22：扩展名别名层 —— .log 命中「Editpad Log」内嵌语法
        // （即使内容长得像 JSON，扩展名可信优先）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/logs/app.log")));
        let doc = editpad_core::Document::from_str("{\"level\":1}");
        let sample = "{\"level\":1}";
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, sample.to_owned(), "UTF-8".to_owned()))),
        );
        assert_eq!(
            app.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Editpad Log")
        );

        // 无扩展名 + shebang 样本 → 内容嗅探接管
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/bin/build")));
        let doc2 = editpad_core::Document::from_str("#!/bin/sh\necho hi\n");
        let seq2 = app2.job_seq;
        dispatch(
            &mut app2,
            Message::Loaded(
                seq2,
                Ok((doc2, "#!/bin/sh\necho hi\n".to_owned(), "UTF-8".to_owned())),
            ),
        );
        assert_eq!(
            app2.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Bourne Again Shell (bash)")
        );
    }

    // ---------- 查找/替换转义解析（\n \r \t \\） ----------

    #[test]
    fn unescape_resolves_known_escapes_and_keeps_unknown_literal() {
        assert_eq!(unescape_query("\\n"), "\n");
        assert_eq!(unescape_query("a\\tb"), "a\tb");
        assert_eq!(unescape_query("\\r"), "\r");
        assert_eq!(unescape_query("a\\\\b"), "a\\b");
        // 未知转义保持原样两个字符；孤立反斜杠保持字面
        assert_eq!(unescape_query("\\q"), "\\q");
        assert_eq!(unescape_query("trailing\\"), "trailing\\");
        // 无转义内容不受影响
        assert_eq!(unescape_query("plain 中文 🚀"), "plain 中文 🚀");
    }

    #[test]
    fn find_and_replace_support_escaped_tab_and_newline() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("a\tb"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 查找 \t 并替换为换行：输入框里的字面反斜杠序列被解析成真实控制字符
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("\\t".into()));
        dispatch(&mut app, Message::ReplaceQueryChanged("\\n".into()));
        let raw_query_kept = app.find_query.clone();
        assert_eq!(raw_query_kept, "\\t", "输入框保留用户原始输入");
        // 模拟在途扫描已完成（测试中任务被丢弃，不会自动清除）
        app.find_scan = None;

        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\nb",
            "替换文本中的 \\n 转义应成为真实换行"
        );
        assert!(app.tab().dirty);
    }

    // ---------- P22 第二批：格式化 JSON ----------

    /// 构造一个已按 .json 加载完成的应用。
    fn json_app(text: &str) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/x/data.json")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str(text);
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((doc, String::new(), "UTF-8".to_owned())),
            ),
        );
        app
    }

    #[test]
    fn format_json_pretty_prints_and_is_revertible() {
        let mut app = json_app("{\"b\":1,\"a\":[2,3]}");
        dispatch(&mut app, Message::FormatJson);
        let expected = "{\n  \"b\": 1,\n  \"a\": [\n    2,\n    3\n  ]\n}";
        assert_eq!(app.cur_handle.borrow().doc.to_text(), expected);
        assert!(app.tab().dirty, "格式化属于内容修改，必须置脏");
        assert_eq!(app.status, "已格式化 JSON");

        // 可撤销：replace_whole_document 走快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{\"b\":1,\"a\":[2,3]}",
            "撤销应还原到格式化前"
        );
    }

    #[test]
    fn format_json_reports_error_position_and_keeps_document() {
        let bad = "{\"a\": 1,,}";
        let mut app = json_app(bad);
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            bad,
            "校验失败不得改动文档"
        );
        assert!(
            app.status.contains("JSON 格式化失败") && app.status.contains("第"),
            "状态栏应带出错误行列，实际 {:?}",
            app.status
        );
    }

    #[test]
    fn format_json_is_noop_for_non_json_documents() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/notes/plain.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("{not:json,but:plain txt}");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{not:json,but:plain txt}",
            "非 JSON 文档不得被改动"
        );
        assert!(app.status.contains("仅对 JSON"), "应提示语法不匹配");
    }

    #[test]
    fn ctrl_shift_f_maps_to_format_json_but_ctrl_f_stays_find() {
        use iced::keyboard::{self};
        let shift_ctrl = keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
        assert!(matches!(
            handle_key(keyboard::Key::Character("F".into()), shift_ctrl),
            Some(Message::FormatJson)
        ));
        // 普通 Ctrl+F 不受影响
        assert!(matches!(
            handle_key(keyboard::Key::Character("f".into()), keyboard::Modifiers::CTRL),
            Some(Message::FindToggled)
        ));
    }

    // ---------- P18 即时保存 ----------

    /// 构造一个已按 .txt 加载完成的应用（纯文本路径）。
    fn loaded_txt_app() -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/note.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((editpad_core::Document::from_str("base"), String::new(), "UTF-8".to_owned())),
            ),
        );
        app
    }

    #[test]
    fn edit_schedules_single_inflight_autosave_and_success_clears_dirty() {
        let mut app = loaded_txt_app();
        assert!(app.settings.autosave_enabled);

        // 编辑置脏并派发防抖任务（版本号在页上推进）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(
            app.tabs[0].autosave_inflight,
            "首次编辑应排队本页防抖任务"
        );
        let scheduled_version = app.tab().version;

        // 连续再编辑：inflight 去重不重复排队；版本继续推进
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        assert!(app.tabs[0].autosave_inflight);
        assert_eq!(app.tab().version, scheduled_version + 1);

        // 任务回报且版本一致 → 清脏解除挂起
        dispatch(
            &mut app,
            Message::TabAutosaved(0, scheduled_version + 1, Ok(())),
        );
        assert!(!app.tab().dirty, "版本一致时落盘应清脏");
        assert!(!app.tabs[0].autosave_inflight);
    }

    #[test]
    fn autosave_stale_version_keeps_dirty() {
        // 快照之后又有编辑：迟到的「保存成功」不得清脏（否则丢改动标记）
        let mut app = loaded_txt_app();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let stale = app.tab().version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        dispatch(&mut app, Message::TabAutosaved(0, stale, Ok(())));

        assert!(
            app.tab().dirty && !app.tabs[0].autosave_inflight,
            "版本不符应保持置脏并解除挂起"
        );
    }

    #[test]
    fn autosave_failure_traces_status_keeps_dirty_and_allows_requeue() {
        let mut app = loaded_txt_app();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tabs[0].autosave_inflight);

        let version = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(0, version, Err("disk full".into())),
        );

        assert!(!app.tabs[0].autosave_inflight, "失败也要解除挂起");
        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(
            app.status.contains("自动保存失败") && app.status.contains("disk full"),
            "失败必须留痕不能无声吞掉，实际 {:?}",
            app.status
        );

        // 失败解除挂起后，下一次编辑仍可重新排队
        dispatch(&mut app, Message::Edit(EditOp::InsertText("z".into())));
        assert!(app.tabs[0].autosave_inflight, "新编辑应重新排队");
    }

    #[test]
    fn autosave_skipped_for_untitled_or_disabled() {
        // 未命名文档（path == None）：绝不自动落盘
        let mut untitled = Editpad::default();
        dispatch(&mut untitled, Message::Edit(EditOp::InsertText("x".into())));
        assert!(untitled.tab().dirty);
        assert!(
            !untitled.tabs[0].autosave_inflight,
            "未命名文档不参与自动保存"
        );

        // 设置关闭：同样跳过
        let mut app = loaded_txt_app();
        app.settings.autosave_enabled = false;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(!app.tabs[0].autosave_inflight, "开关关闭时不排队");

        // busy（手动 IO 进行中）时也跳过
        let mut busy_app = loaded_txt_app();
        busy_app.busy = true;
        dispatch(&mut busy_app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(!busy_app.tabs[0].autosave_inflight, "busy 时不得排队自动保存");
    }

    // ---------- P38 撤销回基线清脏 ----------

    #[test]
    fn undo_back_to_saved_content_clears_dirty_and_redo_restores() {
        let mut app = loaded_txt_app();
        assert!(!app.tab().dirty);

        // 基线之后编辑置脏 → 自动保存成功落盘 → 当前内容成为磁盘内容
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        assert!(app.tab().dirty);
        let v = app.tab().version;
        dispatch(&mut app, Message::TabAutosaved(0, v, Ok(())));
        assert!(!app.tab().dirty);

        // 再编辑后撤销：内容恰好回到磁盘版本 → dirty 如实回清。
        // 注意先移动光标打断 P37 打字组——否则「?」并入上一组，
        // 一次撤销会直接退回基线之前（那时保持置脏才是正确行为）
        dispatch(
            &mut app,
            Message::Edit(EditOp::Motion(Motion::Right, false)),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
        assert!(app.tab().dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(
            !app.tab().dirty,
            "撤销回已保存内容必须回清 dirty（旧实现保持置脏）"
        );
        assert!(!app.any_dirty(), "单页应用：聚合口径同样干净");

        // 重做离开基线 → 重新置脏；再撤又回清
        dispatch(&mut app, Message::Edit(EditOp::Redo));
        assert!(app.tab().dirty, "重做到未保存状态必须重新置脏");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.tab().dirty);
    }

    #[test]
    fn untitled_page_undo_to_blank_clears_dirty() {
        let mut app = Editpad::default();
        // 未命名页的基线 = 初始空白：打字置脏，退净即安全可关
        dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿".into())));
        assert!(app.any_dirty());
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.any_dirty(), "撤销回空白的未命名页不应再拦截关窗");
        assert_eq!(app.tabs.len(), 1);

        // 部分撤销仍视为有未保存内容：两步输入只退一步
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("a".into())));
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("\n".into()))); // 换行独立成组
        dispatch(&mut app2, Message::Edit(EditOp::Undo));
        assert_eq!(app2.cur_handle.borrow().doc.to_text(), "a");
        assert!(app2.any_dirty(), "\"a\" 从未落盘，必须保持置脏");
    }

    #[test]
    fn tab_key_inserts_literal_tab() {
        // P14：Tab 不再被吞——插入真实制表符（显示层展开，见 editor.rs）
        use iced::keyboard::{self, key::Named};
        let message =
            handle_key(keyboard::Key::Named(Named::Tab), keyboard::Modifiers::empty())
                .expect("Tab 应产生编辑消息");
        assert!(matches!(
            message,
            Message::Edit(EditOp::InsertText(ref t)) if t == "\t"
        ));
    }

    // ---------- P6 编码知情权 ----------

    #[test]
    fn transcode_notice_covers_all_encoding_labels() {
        // 纯 UTF-8 / 未打开：无需提示
        assert_eq!(transcode_notice("UTF-8"), None);
        assert_eq!(transcode_notice(""), None);

        // BOM 丢失要提示
        let bom = transcode_notice("UTF-8(BOM)").expect("BOM 丢失应有提示");
        assert!(bom.contains("BOM"));

        // 转码要提示且带出原编码名
        for label in ["GBK", "UTF-16LE", "UTF-16BE"] {
            let notice = transcode_notice(label).expect("转码应有提示");
            assert!(notice.contains(label), "提示需含原编码 {label}: {notice}");
            assert!(notice.contains("UTF-8"));
        }
    }

    #[test]
    fn load_stream_happy_path_emits_progress_then_done() {
        let dir = scratch_dir("happy");
        let path = dir.join("note.txt");
        // 超过单块（64KB）以触发多次进度回调
        let content = "Editpad 加载流测试\n".repeat(8_000);
        std::fs::write(&path, &content).unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                7,
                path.clone(),
                |p, cb| editpad_core::load_document_streaming(p, cb),
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let progress_count = messages
            .iter()
            .filter(|m| matches!(m, Message::LoadProgress(7, _, _)))
            .count();
        assert!(progress_count >= 2, "应有多条进度消息，实际 {progress_count}");

        let dones: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(id, result) => Some((*id, result)),
                _ => None,
            })
            .collect();
        assert_eq!(dones.len(), 1, "恰好一条 Loaded");
        let (id, Ok((doc, _sample, encoding))) = dones[0] else {
            panic!("应为成功加载，实际 {:?}", dones[0]);
        };
        assert_eq!(id, 7);
        // P19：消息携带的是 rope 文档本体
        assert_eq!(doc.to_text(), content);
        assert_eq!(encoding.as_str(), "UTF-8");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_stream_reports_failure_when_loader_panics() {
        // P5 回归：加载线程崩溃也必须让 UI 收到 Loaded(Err)，busy 才能解除
        let dir = scratch_dir("panic");
        let path = dir.join("boom.txt");
        std::fs::write(&path, b"data").unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                9,
                path.clone(),
                |_p, _cb| -> Result<editpad_core::LoadedDocument, editpad_core::CoreError> {
                    panic!("模拟加载线程崩溃");
                },
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let failures: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(_, Err(text)) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1, "panic 后应恰好一条失败 Loaded，实际 {messages:?}");
        assert!(failures[0].contains("加载线程崩溃"), "错误需含兜底前缀: {}", failures[0]);
        assert!(failures[0].contains("模拟加载线程崩溃"), "错误需含 panic 信息: {}", failures[0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P10 后台查找扫描 ----------

    #[test]
    fn find_scan_stream_emits_exactly_one_done_with_matches() {
        let payload = FindScanPayload {
            seq: 42,
            doc: editpad_core::Document::from_str("foo\nbar foo\n"),
            query: "foo".to_owned(),
            case_sensitive: true,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        // 恰好一条完成消息（函数直接返回它）
        let message = block_on(drive_find_scan(payload, |doc, q, cs| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 42);
                assert_eq!(
                    hits,
                    &vec![
                        editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
                        editpad_core::MatchPos { line: 1, col: 4, len_chars: 3 },
                    ]
                );
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_cancelled_task_skips_scanning_and_replies_empty() {
        // 新输入排队时会把上一代的取消标志置位；被作废的任务醒来即退出，
        // 不浪费一次全文扫描，但仍回一条空结果消息保持「恰好一条」语义
        let flag = Arc::new(AtomicBool::new(false));
        flag.store(true, Ordering::Relaxed);
        let payload = FindScanPayload {
            seq: 3,
            doc: editpad_core::Document::from_str("target target"),
            query: "target".to_owned(),
            case_sensitive: true,
            cancelled: flag,
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(payload, |doc, q, cs| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(seq, 3);
                assert!(hits.is_empty(), "被取消的任务不得产出命中");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_survives_panic_and_still_replies() {
        // 扫描函数崩溃也必须回消息（空表），否则查找栏永久停在「查找中…」
        let payload = FindScanPayload {
            seq: 7,
            doc: editpad_core::Document::new(),
            query: "x".to_owned(),
            case_sensitive: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(
            payload,
            |_doc, _q, _cs| -> Vec<editpad_core::MatchPos> { panic!("模拟扫描崩溃") },
        ));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 7);
                assert!(hits.is_empty(), "panic 兜底应回空命中表");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_results_are_filtered_by_sequence_number() {
        let mut app = Editpad::default();
        // 两份可区分的命中表：过期投递不得覆盖已采纳/待采纳的状态
        let hit_a = || vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 1 }];
        let hit_b = || vec![editpad_core::MatchPos { line: 1, col: 4, len_chars: 1 }];
        /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
        fn dispatch(app: &mut Editpad, message: Message) {
            let _ = app.update(message);
        }

        app.find_visible = true;

        // 排队 → 采纳当前序号的结果
        dispatch(&mut app, Message::FindQueryChanged("a".into()));
        assert_eq!(app.find_scan, Some(1), "查询变化应排队一次后台扫描");
        dispatch(&mut app, Message::FindScanDone(1, hit_a()));
        assert_eq!(app.matches, hit_a());
        assert_eq!(app.find_scan, None, "结果采纳后在途标记应清除");
        assert_eq!(app.match_idx, None, "新结果后跳转游标复位");

        // 已消费的序号再回来（重复投递）不得二次生效
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a());

        // 新输入换新序号；重扫期间保留旧命中（防闪烁），但旧序号的迟到结果不许覆盖
        dispatch(&mut app, Message::FindQueryChanged("ab".into()));
        assert_eq!(app.find_scan, Some(2));
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a(), "过期结果必须被丢弃、不得覆盖");
        dispatch(&mut app, Message::FindScanDone(2, hit_b()));
        assert_eq!(app.matches, hit_b());
        assert_eq!(app.find_scan, None);

        // 大小写切换同样触发重扫
        dispatch(&mut app, Message::CaseToggled(true));
        assert_eq!(app.find_scan, Some(3));

        // 文档编辑（查找栏开着时）触发重扫
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert_eq!(app.find_scan, Some(4));

        // 关闭查找栏 = 取消：清结果且在途结果作废
        dispatch(&mut app, Message::FindToggled);
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(4, hit_a()));
        assert!(app.matches.is_empty(), "取消后的迟到结果必须被丢弃");

        // Esc 关栏同样取消（注意：上一步关栏的取消已把序号推到 5，本次排队为 6）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("abc".into()));
        assert_eq!(app.find_scan, Some(6));
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.find_scan, None);
        dispatch(&mut app, Message::FindScanDone(6, hit_a()));
        assert!(app.matches.is_empty());

        // 清空查询 = 取消而非空表扫描（BarsDismissed 取消后序号为 7，本次排队 8）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("q".into()));
        assert_eq!(app.find_scan, Some(8));
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert_eq!(app.matches, hit_a());
        dispatch(&mut app, Message::FindQueryChanged(String::new()));
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert!(app.matches.is_empty(), "清空查询取消后，同号迟到结果也必须被丢弃");
    }

    #[test]
    fn find_next_while_scanning_reports_progress_not_stale_jump() {
        let mut app = Editpad::default();
        app.find_visible = true;
        app.find_query = "zzz".into();

        // 有结果在途：不基于过期命中表跳转
        app.find_scan = Some(9);
        app.matches = vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 }];
        let _ = app.update(Message::FindNext);
        assert_eq!(app.status, "查找中…", "扫描在途时 Enter 应提示进度");

        // 无结果且无在途扫描：懒补排一次扫描而不是误报「无匹配」
        let mut fresh = Editpad::default();
        fresh.find_visible = true;
        fresh.find_query = "zzz".into();
        let _ = fresh.update(Message::FindNext);
        assert!(fresh.find_scan.is_some(), "Enter 应懒触发一次后台扫描");
    }

    // ---------- P26 跨行查询（CRLF 文档上的选区还原与替换当前） ----------

    /// 构造一个已加载 CRLF 文档的应用，返回 (应用, 文档快照)。
    fn crlf_find_app() -> (Editpad, editpad_core::Document) {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/crlf.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("first\r\nsecond\r\nthird");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc.clone(), String::new(), "UTF-8".to_owned()))),
        );
        (app, doc)
    }

    #[test]
    fn multiline_match_selects_across_lines_on_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        // 用户输入转义序列：\n 解析为真实换行 → 查询含换行、跨行匹配
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        assert!(app.find_scan.is_some(), "查询变化应排队后台扫描");

        // 用与后台线程相同的核心函数产出命中表，再按协议回填
        let query = unescape_query("st\\nse");
        let hits = editpad_core::find_all_document(&doc, &query, true);
        assert_eq!(
            hits,
            vec![editpad_core::MatchPos { line: 0, col: 3, len_chars: 5 }],
            "命中跨度应为显示口径 5（2 字符 + 1 次跨行 + 2 字符），不是原始字符数 6"
        );
        let seq = app.find_scan.unwrap();
        dispatch(&mut app, Message::FindScanDone(seq, hits));
        assert!(app.matches.len() == 1);

        // 跳到该命中：选区必须跨行且恰好覆盖「st\r\nse」（\r\n 只算一格）
        dispatch(&mut app, Message::FindNext);
        assert_eq!(app.status, "第 1/1 处匹配");
        let selected = app.cur_handle.borrow().selected_text();
        assert_eq!(
            selected.as_deref(),
            Some("st\r\nse"),
            "跨行选区应包含文档真实的 \\r\\n，而不是多走/少走一格"
        );
    }

    #[test]
    fn replace_current_replaces_multiline_match_in_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        let query = unescape_query("st\\nse");
        let seq = {
            let hits = editpad_core::find_all_document(&doc, &query, true);
            let s = app.find_scan.unwrap();
            dispatch(&mut app, Message::FindScanDone(s, hits));
            s
        };
        let _ = seq;
        dispatch(&mut app, Message::FindNext);
        dispatch(&mut app, Message::ReplaceQueryChanged("-".into()));

        // 替换当前：选区文本 st\r\nse 与查询 st\nse 行尾归一后判等才可替换。
        // （P26 前的行为：字面比较判不等 → 误跳下一个、文档不动。）
        dispatch(&mut app, Message::ReplaceCurrent);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            // "second" 被吃掉开头 "se" 后剩 "cond"，不是 "ond"
            "fir-cond\r\nthird",
            "跨行命中应被整体替换为替换文本"
        );
        assert!(app.tab().dirty);

        // 可撤销：替换走 insert_str 快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "first\r\nsecond\r\nthird");
    }

    // ---------- P11 全部替换（rope 流式路径） ----------

    #[test]
    fn replace_all_swaps_document_and_sets_dirty() {
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("foo bar foo\nfoo"));
        app.find_visible = true;
        app.find_query = "foo".into();
        app.replace_query = "baz".into();

        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "baz bar baz\nbaz",
            "全部替换应改写文档内容"
        );
        assert!(app.tab().dirty, "全部替换后必须置脏");
        assert_eq!(app.status, "已替换 3 处");

        // 无命中时不改文档也不置脏
        let mut app2 = Editpad::default();
        app2.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("untouched"));
        app2.find_query = "zzz".into();
        app2.replace_query = "x".into();
        let _ = app2.update(Message::ReplaceAll);
        assert_eq!(app2.cur_handle.borrow().doc.to_text(), "untouched");
        assert!(!app2.tab().dirty);
        assert_eq!(app2.status, "已替换 0 处");
    }

    #[test]
    fn replace_all_is_blocked_while_scan_in_flight() {
        // P10 守卫在 P11 新路径上仍然生效：扫描在途时的全文快照可能过期
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("keep me"));
        app.find_query = "me".into();
        app.replace_query = "you".into();
        app.find_scan = Some(11);
        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "keep me",
            "后台扫描在途时不得执行全部替换"
        );
        assert!(!app.tab().dirty);
    }

    // ---------- P12 高亮后台分批补建 ----------

    /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
    fn dispatch(app: &mut Editpad, message: Message) {
        let _ = app.update(message);
    }

    /// 构造一个启用了 Rust 语法高亮的 N 行文档应用，返回 (状态, 当时代次)。
    fn app_with_rs_doc(lines: usize) -> (Editpad, u64) {
        let app = Editpad::default();
        let text = "fn f(x: f64) -> f64 { x /* 注释 */ }\n".repeat(lines);
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str(&text));
            ed.set_language(Some("rs"));
        }
        let gen = app
            .cur_handle
            .borrow()
            .highlight_generation()
            .expect("已启用高亮");
        (app, gen)
    }

    fn pave_payload(app: &Editpad, batch: usize) -> (HlPavePayload, u64) {
        let (gen, highlighter) = app
            .cur_handle
            .borrow()
            .highlight_pave_snapshot()
            .expect("已启用高亮");
        let doc = app.cur_handle.borrow().doc.clone();
        (
            HlPavePayload {
                gen,
                doc,
                highlighter,
                total_lines: app.cur_handle.borrow().doc.line_count(),
                cancelled: Arc::new(AtomicBool::new(false)),
                batch_strides: batch,
            },
            gen,
        )
    }

    #[test]
    fn hl_pave_stream_reports_progress_then_installs_final_state() {
        let (mut app, _) = app_with_rs_doc(600);
        let (payload, gen) = pave_payload(&app, 1);

        // 调度登记：HighlightPaveNeeded 应把任务记入在途（幂等性另有专测）
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(app.hl_paving.is_some(), "应登记在途铺建任务");

        // 直接驱动一次完整流并逐条喂给 update（模拟运行时投递）
        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(payload, pave_run, &mut tx).await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        // 600 行 → 3 个完整档位，batch=1 → 3 条进度 + 恰好 1 条完成
        let progresses: Vec<u64> = messages
            .iter()
            .filter_map(|m| match m {
                Message::HlPaveProgress(g, done) => {
                    assert_eq!(*g, gen, "进度消息必须携带调度代次");
                    Some(*done)
                }
                _ => None,
            })
            .collect();
        assert_eq!(progresses, vec![1, 2, 3], "进度应为累计档位数");
        let dones: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::HlPaved(g, _) => Some(*g),
                _ => None,
            })
            .collect();
        assert_eq!(dones, vec![gen], "恰好一条完成消息且代次一致");

        // 进度消息刷新状态栏；完成消息安装成果并解除登记
        for message in messages {
            dispatch(&mut app, message);
        }
        assert!(matches!(
            app.status.as_str(),
            "语法分析中…100%（后台）" | ""
        ), "铺建完成后状态应归位，实际 {:?}", app.status);
        assert_eq!(app.hl_paving, None, "完成后必须解除在途登记");
        assert_eq!(
            app.cur_handle.borrow().highlight_checkpoints_len(),
            Some(1 + 3),
            "初始检查点 + 3 个后台档位"
        );
    }

    #[test]
    fn stale_hl_pave_result_is_dropped_after_generation_change() {
        let (mut app, gen0) = app_with_rs_doc(600);
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert_eq!(app.hl_paving, Some(gen0));

        // 编辑换代：真实编辑触发 invalidate_from → 换代
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert_ne!(
            app.cur_handle.borrow().highlight_generation(),
            Some(gen0),
            "编辑必须换代"
        );
        let len_now = app.cur_handle.borrow().highlight_checkpoints_len();

        // 迟到的旧代成果（内容无关紧要，代次闸门负责拒收）
        let (stale_payload, _) = pave_payload(&app, 32);
        let mut enriched_hl = stale_payload.highlighter.clone();
        enriched_hl.advance_checkpoints(4, 600, &mut |i| format!("let e{i} = {i};"));
        dispatch(&mut app, Message::HlPaved(gen0, enriched_hl));

        assert_eq!(app.hl_paving, None, "过期任务的登记必须解除");
        assert_eq!(
            app.cur_handle.borrow().highlight_checkpoints_len(),
            len_now,
            "换代后的迟到成果不得覆盖当前高亮器"
        );
    }

    #[test]
    fn cancelled_hl_pave_skips_work_but_still_replies_done() {
        let (app, _) = app_with_rs_doc(600);
        let (payload, _gen) = pave_payload(&app, 1);
        payload.cancelled.store(true, Ordering::Relaxed);
        let base_len = payload.highlighter.checkpoints_len();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(payload, pave_run, &mut tx).await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        assert!(
            messages.iter().all(|m| !matches!(m, Message::HlPaveProgress(..))),
            "被取消的任务不得产出任何推进进度"
        );
        let dones = messages
            .iter()
            .filter(|m| matches!(m, Message::HlPaved(..)))
            .count();
        assert_eq!(dones, 1, "取消也必须回一条完成消息保持「恰好一条」语义");

        // 兜底/取消路径送回的是起点克隆：安装它等于无变化，UI 不受损
        if let Some(Message::HlPaved(_, hl)) = messages.into_iter().next() {
            assert_eq!(hl.checkpoints_len(), base_len);
        }
    }

    #[test]
    fn hl_pave_runner_panic_still_replies_done() {
        // 推进函数崩溃也必须回完成消息（起点克隆兜底），
        // 否则「语法分析中…」永不解除——P5/P10 同款契约
        let (mut app, _) = app_with_rs_doc(600);
        let (payload, gen) = pave_payload(&app, 1);
        let base_len = payload.highlighter.checkpoints_len();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(
                payload,
                |_payload, _report| -> editpad_core::LazyHighlighter { panic!("模拟铺建崩溃") },
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        assert_eq!(messages.len(), 1, "panic 后只应有兜底完成消息");
        match &messages[0] {
            Message::HlPaved(g, hl) => {
                assert_eq!(*g, gen);
                assert_eq!(hl.checkpoints_len(), base_len, "兜底必须是未推进的起点");
            }
            other => panic!("应为 HlPaved，实际 {other:?}"),
        }

        // 喂给 update 后登记解除、状态归零（不卡死）
        dispatch(&mut app, messages.into_iter().next().unwrap());
        assert_eq!(app.hl_paving, None);
    }

    #[test]
    fn schedule_highlight_pave_is_idempotent_until_generation_changes() {
        let (mut app, _) = app_with_rs_doc(600);

        // 首次调度 + 同代重发：不得作废旧任务（取消标志不变、代次不变）
        dispatch(&mut app, Message::HighlightPaveNeeded);
        let flag_first = app.hl_pave_cancel.clone();
        let slot_first = app.hl_paving;
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(
            Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
            "同代重复请求必须幂等跳过"
        );
        assert_eq!(app.hl_paving, slot_first);

        // 编辑换代后再发：旧任务作废（换新取消标志）、新代次重新出发
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(
            !Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
            "换代后必须重排新任务"
        );
        assert_ne!(app.hl_paving, slot_first, "新任务应携带换代后的代次");

        // 纯文本文档（无高亮器）：请求直接清空登记不派发
        let mut plain = Editpad::default();
        plain.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("plain text only\n"));
        dispatch(&mut plain, Message::HighlightPaveNeeded);
        assert_eq!(plain.hl_paving, None);
    }

    // ---------- P21 内存护栏 ----------

    #[test]
    fn mem_guard_rejects_when_estimate_exceeds_cap() {
        let cap = 1_000u64;
        // 现有 100 字符 ×3 字节估算 = 300；再开 500 字节文件 → 800 ≤ 1000 允许
        assert!(mem_guard_allows(100, 500, cap));
        // 再大一点就超
        assert!(!mem_guard_allows(100, 900, cap));
        // 极端值不 panic（饱和运算）
        assert!(mem_guard_allows(usize::MAX, u64::MAX, u64::MAX));
        assert!(!mem_guard_allows(usize::MAX, u64::MAX, 0));
    }

    #[test]
    fn start_loading_enforces_mem_guard() {
        let mut app = Editpad::default();
        // 预置一个超大字符量的当前页（直接改 doc 以绕过真实大文件）
        let huge = editpad_core::Document::from_str(&"x".repeat(200_000_000));
        app.cur_handle.borrow_mut().reset_document(huge);
        // 当前页非空 → 打开会走新页，但护栏按全页合计判定
        let big_path = PathBuf::from("C:/definitely/too/big.bin");
        let task = app.start_loading(big_path.clone(), app.tabs.len());
        let _ = task;
        assert!(
            app.status.contains("内存保护"),
            "超限打开应被拒绝并提示，实际 {:?}",
            app.status
        );
        assert!(app.active_load.is_none(), "被拒的打开不得登记任务");
        assert_eq!(app.tabs.len(), 1, "拒绝时不得占位新页");
    }

    // ---------- P31 周期快照心跳 ----------

    /// 构造已加载命名文档、注入快照目录的心跳测试应用。
    fn heartbeat_app(dir: &Path) -> Editpad {
        let mut app = loaded_txt_app();
        app.snapshot_dir_override = Some(dir.to_path_buf());
        app
    }

    #[test]
    fn heartbeat_writes_changed_dirty_page_and_marks_intermediate_manifest() {
        let dir = scratch_dir("hb-e2e");
        let mut app = heartbeat_app(&dir);

        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("有变化的置脏页应触发一次提交");
        assert!(outcome.result.is_ok(), "提交应成功，实际 {:?}", outcome.result);

        // 磁盘形态：clean_exit=false 的中间态清单 + 内容文件（未命名页
        // 与命名页同规则收录——P18 的盲区由此获得兜底）
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!manifest.clean_exit, "心跳写的必须是中间态清单");
        assert!(manifest.tabs[0].dirty);
        let doc = editpad_core::snapshot::read_page(&dir, &manifest.tabs[0]).unwrap();
        assert_eq!(doc.to_text(), "xbase", "快照必须反映当前内容（光标在行首插入）");

        // 账目回填：版本一致 → 记录 (版本, 文件名)
        assert_eq!(
            app.tabs[0].heartbeat_snap,
            Some((app.tabs[0].version, manifest.tabs[0].file.clone().unwrap()))
        );
        assert!(!app.heartbeat_inflight, "回报落地后必须解除在途标记");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_commit_entirely_when_nothing_changed() {
        let dir = scratch_dir("hb-dedupe");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));

        let first = app.run_heartbeat_cycle(&dir).expect("首次应有提交");
        let manifest_before = editpad_core::snapshot::read_manifest(&dir).unwrap();

        // 无新编辑：代次去重命中 → 零 IO，磁盘原封不动
        let second = app.run_heartbeat_cycle(&dir);
        assert!(second.is_none(), "无变化且清单不过期时不得产生任何写盘");
        assert_eq!(
            editpad_core::snapshot::read_manifest(&dir),
            Some(manifest_before),
            "清单必须保持原样（含代次号）"
        );
        assert_eq!(
            first.plan,
            vec![(0usize, app.tabs[0].version)],
            "计划只含变化页"
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_refreshes_manifest_after_autosave_cleans_tab() {
        let dir = scratch_dir("hb-stale");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let _ = app.run_heartbeat_cycle(&dir).unwrap();
        let dirty_manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(dirty_manifest.tabs[0].dirty);

        // 模拟 auto-save 成功清脏：内存比已提交清单「更干净」→ 过期标记
        let version = app.tabs[0].version;
        dispatch(&mut app, Message::TabAutosaved(0, version, Ok(())));
        assert!(!app.tab().dirty);
        assert!(app.session_manifest_stale, "清脏后下一拍必须重写清单");

        // 下一拍即使无页被选中也要提交：清单转为「干净页纯元数据」，
        // 否则崩溃恢复会把已落盘内容按旧快照复活成置脏页
        app.run_heartbeat_cycle(&dir).expect("过期清单应触发重写");
        let refreshed = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!refreshed.tabs[0].dirty && refreshed.tabs[0].file.is_none());
        assert!(!app.session_manifest_stale, "成功提交后过期标记归零");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_oversized_page_content_but_still_records_it_dirty() {
        let dir = scratch_dir("hb-oversize");
        let mut app = heartbeat_app(&dir);
        // 17MB 文档 > HEARTBEAT_MAX_PAGE_BYTES(16MB)：不进心跳重写，
        // 只在退出时随全量提交落盘（§3 P29 第 5 条的 IO 保护）
        let big = editpad_core::Document::from_str(&"a".repeat(17_000_000));
        {
            let tab = &mut app.tabs[0];
            tab.editor.borrow_mut().reset_document(big);
            tab.dirty = true;
            tab.version += 1;
        }

        // 首拍：Loaded 留下的清单过期标记驱动一次提交——超限页被记账
        // （保持置脏）但绝不产生内容文件
        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("过期清单 + 置脏页应触发一次记账提交");
        assert!(outcome.result.is_ok());
        assert!(
            outcome.plan.is_empty(),
            "超限页不得进入重写计划，实际 {:?}",
            outcome.plan
        );
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(manifest.tabs[0].dirty, "超限页保持置脏记账");
        assert_eq!(manifest.tabs[0].file, None, "超限页不得落内容文件");

        let snap_files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".snap"))
            .collect();
        assert!(snap_files.is_empty(), "磁盘上不得出现任何内容快照");

        // 次拍：过期标记已清、计划仍为空 → 零 IO（节流的常态形态）
        assert!(app.run_heartbeat_cycle(&dir).is_none());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_failure_clears_accounts_traces_status_and_keeps_dirty() {
        let dir = scratch_dir("hb-fail");
        // 注入目录指向一个普通文件 → create_dir_all 必败 → 提交必败
        let blocker = dir.join("occupied-as-file");
        std::fs::write(&blocker, b"not a dir").unwrap();

        let mut app = heartbeat_app(&blocker);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&blocker)
            .expect("有置脏页就应有提交尝试");
        assert!(outcome.result.is_err(), "目录不可用时应失败");

        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(app.tabs[0].heartbeat_snap.is_none(), "失败账目作废");
        assert!(!app.heartbeat_inflight, "失败也解除在途（允许下拍重试）");
        assert!(
            app.status.contains("快照心跳失败"),
            "失败必须留痕不无声吞掉，实际 {:?}",
            app.status
        );

        // 账目作废后下一拍会全量重试（此处仅验证状态复位，不再造真实目录）
        let recovery_dir = scratch_dir("hb-fail-recovery");
        let mut recovered = app.clone();
        recovered.snapshot_dir_override = Some(recovery_dir.clone());
        let retry = recovered.run_heartbeat_cycle(&recovery_dir);
        assert!(retry.is_some_and(|o| o.result.is_ok()), "复位后重试应成功");

        editpad_core::snapshot::clear_session(&dir);
        editpad_core::snapshot::clear_session(&recovery_dir);
    }

    #[test]
    fn heartbeat_result_with_stale_version_does_not_record_account() {
        // 回报落地时的版本守卫：派发后页面又编辑过 → 迟到的快照不得入账
        // （否则账目声称「版本 v 已快照」，实际盘上是更旧的内容）
        let dir = scratch_dir("hb-stale-version");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let planned_version = app.tabs[0].version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        let outcome = HeartbeatOutcome {
            plan: vec![(0, planned_version)],
            result: Ok(editpad_core::snapshot::SessionManifest {
                generation: 1,
                tabs: vec![editpad_core::snapshot::SessionTab {
                    path: None,
                    untitled_num: None,
                    dirty: true,
                    file: Some("s1-t0.snap".to_owned()),
                    cursor_line: 0,
                    cursor_col: 0,
                    scroll_top: 0.0,
                    scroll_left: 0.0,
                }],
                active: 0,
                next_untitled: 2,
                clean_exit: false,
            }),
        };
        app.heartbeat_apply(outcome);

        assert!(
            app.tabs[0].heartbeat_snap.is_none(),
            "版本不符的迟到成果不得记账"
        );
        assert!(!app.session_manifest_stale, "成功路径仍应清过期标记");
        assert!(!app.heartbeat_inflight);

        editpad_core::snapshot::clear_session(&dir);
    }

    // ---------- P27 设置按钮 + 设置弹窗 + 热键速查表 ----------

    #[test]
    fn hotkey_table_is_well_formed() {
        assert!(!HOTKEYS.is_empty(), "热键速查表不得为空");
        for (combo, desc) in HOTKEYS {
            assert!(!combo.trim().is_empty(), "组合键列不得为空");
            assert!(!desc.trim().is_empty(), "功能说明不得为空：{combo}");
        }
        // 条目唯一：重复条目意味着展示数据已经漂移
        let mut combos: Vec<_> = HOTKEYS.iter().map(|(c, _)| *c).collect();
        let total = combos.len();
        combos.sort_unstable();
        combos.dedup();
        assert_eq!(combos.len(), total, "热键表存在重复条目");
    }

    #[test]
    fn hotkey_table_entries_all_dispatch_through_handle_key() {
        use iced::keyboard::{self, key::Named};
        // 速查表的每个组合键都必须在 handle_key 里有真实分支——
        // 表格与分发器任何一侧改动漏同步，本测试当场暴露（防漂移的兑现）
        for (combo, _) in HOTKEYS {
            if combo.contains("滚轮") {
                continue; // 鼠标事件不经 handle_key 分发，无法在此验证
            }
            // 单元格可能聚合多个组合（「Ctrl+F / Ctrl+H」），逐个验证；
            // 单个组合形如 Ctrl+Shift+F——按 + 切分，末段=键名，其余=修饰键
            for part in combo.split('/') {
                let part = part.trim();
                let mut mods = keyboard::Modifiers::empty();
                let mut tokens = part.split('+').map(str::trim).rev();
                let key_name = tokens.next().unwrap_or_else(|| panic!("空组合键 {part:?}"));
                for tok in tokens {
                    mods |= match tok {
                        "Ctrl" => keyboard::Modifiers::CTRL,
                        "Shift" => keyboard::Modifiers::SHIFT,
                        "Alt" => keyboard::Modifiers::ALT,
                        other => panic!("速查表出现未知修饰键 {other:?}"),
                    };
                }
                let key = match key_name {
                    "Tab" => keyboard::Key::Named(Named::Tab),
                    "Home" => keyboard::Key::Named(Named::Home),
                    "End" => keyboard::Key::Named(Named::End),
                    single if single.chars().count() == 1 => {
                        keyboard::Key::Character(single.to_ascii_lowercase().into())
                    }
                    other => panic!("速查表出现未支持的键名 {other:?}"),
                };
                let dispatched = handle_key(key, mods);
                assert!(
                    dispatched.is_some(),
                    "速查表条目 {part:?} 在 handle_key 中无对应分支——快捷键与展示数据已漂移"
                );
            }
        }
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
            EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT, MAX_AUTOSAVE_DELAY_SECS,
            MAX_SNAPSHOT_INTERVAL_SECS, MIN_AUTOSAVE_DELAY_SECS, MIN_SNAPSHOT_INTERVAL_SECS,
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
        assert_eq!(effective_font_family(Some("不存在的字体"), &available), None);
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
        app.available_fonts =
            vec!["Arial".to_owned(), "Sarasa Mono SC".to_owned()];

        // 未选择时 = 默认等宽（P33 钉字语义不变）
        assert_eq!(app.body_font(), editor::BODY_FONT);

        // 选择系统字体：设置与生效族名同步更新并落盘
        dispatch(&mut app, Message::SettingsFontSelected("Arial".to_owned()));
        assert_eq!(app.settings.font_family.as_deref(), Some("Arial"));
        assert_eq!(app.active_font_family.as_deref(), Some("Arial"));
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
        assert_eq!(app.body_font(), editor::BODY_FONT);
        assert_eq!(
            editpad_core::Settings::load_from(&config).font_family,
            None,
            "回退默认后 config.toml 不应再留字体名"
        );

        std::fs::remove_dir_all(&dir).ok();
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
            app.active_font_family.as_deref(),
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
}
