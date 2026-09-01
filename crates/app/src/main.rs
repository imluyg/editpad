//! Editpad —— 极简记事本。
//!
//! M2 里程碑：自绘虚拟化编辑器接管渲染，ropey Document 成为唯一数据源。
//!
//! * 渲染只为可见行排版（见 editor.rs）——50MB 与 5KB 的每帧成本相同；
//! * 键盘编辑在应用层订阅统一分发；鼠标与输入法在控件层处理；
//! * 撤销/重做基于 rope 的廉价结构共享快照。

mod editor;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};

use iced::futures::SinkExt;
use iced::keyboard::{self, key::Named};
use iced::widget::{button, checkbox, column, container, progress_bar, row, rule, text, text_input};
use iced::{stream, window, Alignment, Element, Fill, Subscription, Task, Theme};

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
    /// 后台加载完成：(任务 id, (rope 直入的文档, 编码标签))。
    /// P19 起携带 Document，不再有全量 String 中转
    Loaded(
        u64,
        Result<(editpad_core::Document, String), String>,
    ),
    SaveRequested,
    SaveAsRequested,
    SaveTargetChosen(Option<PathBuf>),
    Saved(Result<(), String>),

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
}

/// 后台加载线程 → 订阅流的事件。
enum LoadEvent {
    Progress(editpad_core::LoadProgress),
    Done(Result<editpad_core::LoadedDocument, String>),
}

/// 后台加载任务：订阅标识 + 目标路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LoadJob {
    id: u64,
    path: PathBuf,
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
                result.map(|loaded| (loaded.doc, loaded.encoding.to_string())),
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

#[derive(Default)]
struct Editpad {
    // ---------- 文档状态 ----------
    path: Option<PathBuf>,
    /// 打开/另存为进行中暂存的路径，成功落盘后才转正
    pending_path: Option<PathBuf>,
    encoding_label: String,
    /// 唯一数据源（M2 起）
    editor: EditorHandle,
    dirty: bool,
    /// 对话框/IO 进行中，防止重复触发
    busy: bool,
    status: String,

    // ---------- 设置 ----------
    settings: editpad_core::Settings,

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

    // ---------- 外观 ----------
    dark_mode: bool,
}

impl Editpad {
    fn new() -> (Self, Task<Message>) {
        let settings = editpad_core::Settings::load();
        let dark_mode = settings.is_dark();
        // 设置里的字号可能未归一（旧配置/手改），boot 时按同一规则 clamp
        let font_size = editor::normalize_font_size(settings.font_size);
        let state = Self {
            settings,
            dark_mode,
            ..Self::default()
        };
        state.editor.borrow_mut().set_font_size(font_size);
        (state, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // ---------- 编辑器 ----------
            Message::Edit(op) => {
                if self.apply_edit(op) && self.find_visible {
                    // P10：编辑后不再同步重扫（每键全文扫描会卡 UI），排队后台防抖扫描
                    return self.schedule_find_scan();
                }
                Task::none()
            }
            Message::EditorNavChanged => Task::none(), // 视图重建即可刷新状态栏

            // ---------- 剪贴板（P4） ----------
            Message::CopyRequested => {
                let Some(text) = self.editor.borrow().selected_text() else {
                    return Task::none();
                };
                iced::clipboard::write(text)
            }
            Message::CutRequested => {
                let Some(text) = self.editor.borrow().selected_text() else {
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
                self.busy = true;
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
                if !self.active_load.as_ref().is_some_and(|j| j.id == job_id) {
                    return Task::none();
                }
                self.active_load = None;
                self.progress = None;
                match result {
                    Ok((doc, encoding)) => {
                        // 按扩展名启用语法高亮（未知类型自动退回纯文本）
                        let extension = self
                            .pending_path
                            .as_ref()
                            .and_then(|p| p.extension())
                            .and_then(std::ffi::OsStr::to_str)
                            .map(str::to_owned);
                        {
                            let mut ed = self.editor.borrow_mut();
                            // P19：rope 直入，不再有 from_str 的二次全文拷贝
                            ed.reset_document(doc);
                            ed.set_language(extension.as_deref());
                        }
                        if let Some(path) = self.pending_path.take() {
                            self.record_recent(&path);
                            self.path = Some(path);
                        }
                        self.encoding_label = encoding;
                        self.dirty = false;
                        self.busy = false;
                        // 新文档已就位：旧文档的关闭确认语义过期
                        self.confirm_visible = false;
                        self.pending_close = false;
                        self.status.clear();
                        if self.find_visible {
                            return self.schedule_find_scan();
                        }
                    }
                    Err(error) => {
                        self.pending_path = None;
                        self.busy = false;
                        self.status = format!("打开失败:{error}");
                    }
                }
                Task::none()
            }

            // ---------- 保存 ----------
            Message::SaveRequested => match self.path.clone() {
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
                self.path = Some(path);
                // 对话框阶段结束再交给 save() 的 busy 守卫（原实现在此卡死 busy）
                self.busy = false;
                self.save()
            }
            Message::Saved(Ok(())) => {
                self.dirty = false;
                self.busy = false;
                if let Some(path) = self.path.clone() {
                    self.record_recent(&path);
                }
                // P6 编码知情权：发生转码/BOM 丢失时明确告知，而不是静默落盘
                if let Some(notice) = transcode_notice(&self.encoding_label) {
                    self.status = notice;
                } else {
                    self.status.clear();
                }
                // 落盘后文件已是纯 UTF-8，标签同步归一（避免后续保存重复提示）
                self.encoding_label = "UTF-8".to_owned();
                if self.pending_close {
                    // 落盘确认后才真正关窗
                    self.pending_close = false;
                    return self.close_window();
                }
                Task::none()
            }
            Message::Saved(Err(error)) => {
                self.busy = false;
                // 保存失败不关窗：留在应用里让用户处理
                self.pending_close = false;
                self.status = format!("保存失败:{error}");
                Task::none()
            }

            // ---------- 关闭确认 ----------
            Message::CloseRequested(id) => {
                // 捕获主窗口 id（仅有的窗口），供后续 window::close 使用
                self.main_window = Some(id);
                if self.dirty {
                    self.confirm_visible = true;
                    Task::none()
                } else {
                    self.close_window()
                }
            }
            Message::ConfirmSaveAndClose => {
                self.confirm_visible = false;
                self.pending_close = true;
                match self.path.clone() {
                    Some(_) => self.save(),
                    // 未命名文档：先走另存为，落盘成功后自动关窗
                    None => self.save_as_dialog(),
                }
            }
            Message::DiscardAndClose => {
                self.dirty = false;
                self.confirm_visible = false;
                self.close_window()
            }
            Message::CancelClose => {
                self.confirm_visible = false;
                self.pending_close = false;
                Task::none()
            }

            // ---------- 打开确认 ----------
            Message::ConfirmOpenDiscard => {
                let Some(path) = self.open_confirm.take() else {
                    return Task::none();
                };
                // 明确放弃：不再触发下一次确认；若关闭确认条还开着，其前提已消失
                self.dirty = false;
                self.confirm_visible = false;
                self.pending_close = false;
                self.start_loading(path)
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
                let (new_contents, count) = {
                    let editor = self.editor.borrow();
                    editpad_core::replace_all_document(
                        &editor.doc,
                        &self.find_query,
                        &self.replace_query,
                        self.case_sensitive,
                    )
                };
                if count > 0 {
                    self.editor
                        .borrow_mut()
                        .replace_whole_document(editpad_core::Document::from_str(&new_contents));
                    self.dirty = true;
                }
                // P10：替换后的重扫走后台防抖，不再同步刷
                let task = self.schedule_find_scan();
                self.status = format!("已替换 {count} 处");
                return task;
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
            Message::HlPaveProgress(gen, strides_done) => {
                // 双重代次检查：任务登记一致且高亮器未换代（换文件后
                // 旧任务的迟到进度不得污染新会话的状态栏）
                if self.hl_paving == Some(gen)
                    && self.editor.borrow().highlight_generation() == Some(gen)
                {
                    let total_strides = (self.editor.borrow().doc.line_count()
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
                        .editor
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
                    self.editor.borrow_mut().jump_to_line(n);
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
                self.settings.save();
                self.status = "已清空最近文件记录".to_owned();
                Task::none()
            }

            Message::BarsDismissed => {
                self.find_visible = false;
                self.goto_visible = false;
                self.recents_visible = false;
                // Esc 同时视作放弃关闭/打开确认
                self.confirm_visible = false;
                self.pending_close = false;
                self.open_confirm = None;
                // P10：取消在途扫描 + 清结果（含序号失效）
                self.cancel_find_scan();
                Task::none()
            }

            // ---------- 外观 ----------
            Message::ThemeToggled => {
                self.dark_mode = !self.dark_mode;
                self.settings.set_theme(self.dark_mode);
                self.settings.save();
                Task::none()
            }
            Message::FontSizeDelta(delta) => {
                let next = editor::normalize_font_size(self.display_font_size() + delta);
                self.settings.font_size = next;
                self.settings.save();
                self.editor.borrow_mut().set_font_size(next);
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

        let mut editor = self.editor.borrow_mut();
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
                }
                changed
            }
            E::Redo => {
                let changed = editor.redo();
                if !changed {
                    hint = Some("已在最新状态");
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
            self.dirty = true;
            self.status.clear();
        } else if let Some(hint) = hint {
            self.status = hint.to_owned();
        }
        changed
    }

    // ---------- 加载管线 ----------

    /// 启动一次后台加载：登记任务后由 [`Editpad::subscription`] 的流接管。
    fn start_loading(&mut self, path: PathBuf) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        self.job_seq += 1;
        let job = LoadJob {
            id: self.job_seq,
            path: path.clone(),
        };
        self.pending_path = Some(path);
        self.active_load = Some(job);
        self.progress = Some((0, 0));
        self.busy = true;
        self.status.clear();
        Task::none()
    }

    /// 统一的换文档入口（打开对话框/拖拽/最近文件共用）：
    /// dirty 时绝不静默丢弃修改（含撤销链），先弹打开确认条。
    fn request_open(&mut self, path: PathBuf) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        if self.dirty {
            self.open_confirm = Some(path);
            return Task::none();
        }
        self.start_loading(path)
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
                _ => None,
            });
        // 窗口关闭请求：exit_on_close_request(false) 后以订阅事件流转
        let close_requests = window::close_requests().map(Message::CloseRequested);
        Subscription::batch([load, events, close_requests])
    }

    // ---------- 保存 ----------

    fn save_as_dialog(&mut self) -> Task<Message> {
        if self.busy {
            return Task::none();
        }
        self.busy = true;
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
        if self.busy || self.path.is_none() {
            return Task::none();
        }
        self.busy = true;
        let path = self.path.clone().expect("上方已确认非空");
        // P19 行动项 3：rope 结构共享克隆（O(1)），分块原子写盘，
        // 不再经 to_text() 产生全文 String（50MB 场景省 ~50MB 峰值）
        let doc = self.editor.borrow().doc.clone();
        Task::perform(
            async move { editpad_core::save_document_atomic(&path, &doc) },
            |result| Message::Saved(result.map_err(|e| e.to_string())),
        )
    }

    fn record_recent(&mut self, path: &Path) {
        self.settings.push_recent(path);
        self.settings.save();
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

    /// 关闭主窗口；id 来自 close_requests 订阅的捕获。
    fn close_window(&self) -> Task<Message> {
        match self.main_window {
            Some(id) => window::close(id),
            None => Task::none(),
        }
    }

    // ---------- 查找 / 替换内部逻辑 ----------

    /// 排队一次后台查找扫描（P10）。查询为空或查找栏已关闭时转为取消。
    /// UI 线程只做廉价操作：文档快照是 rope 结构共享克隆，全文扫描
    /// 在防抖 200ms 后的后台线程进行，结果按序号回填。
    fn schedule_find_scan(&mut self) -> Task<Message> {
        if !self.find_visible || self.find_query.is_empty() {
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
            doc: self.editor.borrow().doc.clone(),
            query: self.find_query.clone(),
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
        let current_gen = self.editor.borrow().highlight_generation();
        if let (Some(active), Some(current)) = (self.hl_paving, current_gen) {
            if active == current {
                return Task::none();
            }
        }
        let Some((gen, highlighter)) = self.editor.borrow().highlight_pave_snapshot() else {
            // 无高亮器（纯文本路径）：清掉可能残留的旧任务登记
            self.hl_paving = None;
            return Task::none();
        };
        let doc = self.editor.borrow().doc.clone();
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

        let cursor = self.editor.borrow().cursor;
        let index = if forward {
            editpad_core::next_from(&self.matches, cursor.line, cursor.col)
        } else {
            editpad_core::prev_from(&self.matches, cursor.line, cursor.col)
        };
        self.match_idx = index;

        if let (Some(i), Some(pos)) =
            (index, index.and_then(|i| self.matches.get(i).copied()))
        {
            let query_len = self.find_query.chars().count();
            self.editor.borrow_mut().select_span(pos.line, pos.col, query_len);
            self.status = format!("第 {}/{} 处匹配", i + 1, self.matches.len());
        }
        Task::none()
    }

    fn replace_current(&mut self) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        let hit_selected = {
            let editor = self.editor.borrow();
            editor
                .selected_text()
                .is_some_and(|selected| {
                    strings_equal(&selected, &self.find_query, self.case_sensitive)
                })
        };

        if hit_selected {
            self.editor.borrow_mut().replace_selection(&self.replace_query.clone());
            self.dirty = true;
            // P10：替换后命中表已过期，排队后台重扫；「跳到下一个」等重扫完成
            // 后由用户再按（旧行为是同步重扫后立即跳，会卡大文档 UI）
            return self.schedule_find_scan();
        }
        // 没有可替换的选区：行为不变——跳到下一个匹配
        self.step_match(true)
    }

    // ---------- 展示辅助 ----------

    fn title(&self) -> String {
        let name = self.file_display_name().unwrap_or_else(|| "未命名".into());
        if self.dirty {
            format!("● {name} - Editpad")
        } else {
            format!("{name} - Editpad")
        }
    }

    fn suggested_name(&self) -> String {
        self.file_display_name().unwrap_or_else(|| "未命名.txt".into())
    }

    fn file_display_name(&self) -> Option<String> {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_owned)
    }

    fn view(&self) -> Element<'_, Message> {
        let toolbar = row![
            button(text("打开…"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::OpenRequested)),
            button(text("保存"))
                .padding([4, 12])
                .on_press_maybe((!self.busy && self.dirty).then_some(Message::SaveRequested)),
            button(text("另存为…"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::SaveAsRequested)),
            button(text("查找/替换"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::FindToggled)),
            button(text("跳转到行"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::GotoToggled)),
            button(text("最近文件"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::RecentsToggled)),
            button(text(if self.dark_mode {
                "主题:深色"
            } else {
                "主题:浅色"
            }))
            .padding([4, 12])
            .on_press(Message::ThemeToggled),
            row![
                button(text("A-"))
                    .padding([4, 10])
                    .on_press_maybe((self.display_font_size()
                        > editpad_core::settings::MIN_FONT_SIZE)
                        .then_some(Message::FontSizeDelta(-2.0))),
                text(format!("{:.0}", self.display_font_size())),
                button(text("A+"))
                    .padding([4, 10])
                    .on_press_maybe((self.display_font_size()
                        < editpad_core::settings::MAX_FONT_SIZE)
                        .then_some(Message::FontSizeDelta(2.0))),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
            text(if self.dirty { "● 未保存" } else { "" }).color([0.85, 0.55, 0.1]),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 10]);

        // M2 核心：自绘虚拟化编辑器，数据源是 ropey Document
        let editor_view = self.editor.view();

        let mut body = column![toolbar, rule::horizontal(1), editor_view];

        if let Some((bytes_read, total_bytes)) = self.progress {
            body = body.push(
                row![
                    text("加载中…"),
                    container(
                        progress_bar(0.0..=total_bytes.max(1) as f32, bytes_read as f32)
                    )
                    .width(Fill),
                    text(format!(
                        "{} / {} KB",
                        bytes_read / 1024,
                        total_bytes.max(1) / 1024
                    )),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if self.recents_visible {
            let mut panel = column![].spacing(2).padding([4, 10]);
            if self.settings.recent_files.is_empty() {
                panel = panel.push(text("（暂无最近文件）").color([0.5, 0.5, 0.5]));
            }
            for entry in &self.settings.recent_files {
                panel = panel.push(
                    button(container(text(entry)).width(Fill))
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
                        button(text("清空记录"))
                            .padding([2, 8])
                            .on_press_maybe((!self.busy).then_some(Message::RecentsCleared)),
                        text("从 config.toml 移除全部路径").color([0.5, 0.5, 0.5]),
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
                        .on_input(Message::FindQueryChanged)
                        .on_submit(Message::FindNext)
                        .width(200),
                    text(position_label),
                    button(text("↑ 上一个"))
                        .on_press_maybe(has_matches.then_some(Message::FindPrev)),
                    button(text("↓ 下一个"))
                        .on_press_maybe(has_matches.then_some(Message::FindNext)),
                    checkbox(self.case_sensitive)
                        .label("区分大小写")
                        .on_toggle(Message::CaseToggled),
                    button(text("×")).on_press(Message::FindToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );

            body = body.push(
                row![
                    text_input("替换为", &self.replace_query)
                        .on_input(Message::ReplaceQueryChanged)
                        .width(200),
                    button(text("替换当前"))
                        .on_press_maybe(has_matches.then_some(Message::ReplaceCurrent)),
                    // 扫描在途时禁用：此刻的全文快照可能是过期的
                    button(text("全部替换")).on_press_maybe(
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
                    text("跳转到行:"),
                    text_input("行号", &self.goto_input)
                        .on_input(Message::GotoInputChanged)
                        .on_submit(Message::GotoSubmit)
                        .width(140),
                    button(text("跳转")).on_press(Message::GotoSubmit),
                    button(text("×")).on_press(Message::GotoToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // 未保存关闭确认条：置于状态区域上方
        if self.confirm_visible {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text("文档有未保存的更改，确定要关闭吗？"),
                    button(text("保存并关闭"))
                        .padding([4, 12])
                        .on_press_maybe(
                            (!self.busy).then_some(Message::ConfirmSaveAndClose)
                        ),
                    button(text("放弃更改"))
                        .padding([4, 12])
                        .on_press(Message::DiscardAndClose),
                    button(text("取消"))
                        .padding([4, 12])
                        .on_press(Message::CancelClose),
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
                    text(format!("{} 有未保存的更改，放弃并打开？", path.display())),
                    button(text("放弃更改并打开"))
                        .padding([4, 12])
                        .on_press(Message::ConfirmOpenDiscard),
                    button(text("取消"))
                        .padding([4, 12])
                        .on_press(Message::ConfirmOpenCancel),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if !self.status.is_empty() {
            body = body.push(
                container(text(format!("⚠ {}", self.status)).color([0.9, 0.25, 0.25]))
                    .padding([4, 10]),
            );
        }

        let cursor = self.editor.borrow().cursor;
        let status_bar = row![
            text(
                self.path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(未命名)".into())
            )
            .width(Fill),
            text(if self.encoding_label.is_empty() {
                "—".to_owned()
            } else {
                self.encoding_label.clone()
            }),
            text(format!("{} 行", self.editor.borrow().doc.line_count())),
            text(format!("Ln {}, Col {}", cursor.line + 1, cursor.col + 1)),
        ]
        .spacing(24)
        .align_y(Alignment::Center)
        .padding([6, 10]);

        body = body.push(rule::horizontal(1)).push(status_bar);

        container(body).width(Fill).height(Fill).into()
    }
}

/// 全局按键分发：Ctrl 组合快捷键优先，其次编辑键与光标移动。
fn handle_key(key: keyboard::Key, mods: keyboard::Modifiers) -> Option<Message> {
    use keyboard::Key;

    let edit = |op| Some(Message::Edit(op));

    // Ctrl 组合（Windows/Linux 语义）。
    // P8：AltGr 在 Windows 上报为 Ctrl+Alt，欧洲键盘的 AltGr 字符
    // （德语 @=AltGr+Q 等）若进此分支匹配不到就被静默吞掉；
    // 因此仅「纯 Ctrl」才当快捷键，带 Alt 的一律按普通字符处理。
    if mods.control() && !mods.alt() {
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
                _ => return None,
            };
            return Some(message);
        }
        // Ctrl+Home/End：文档首尾
        return match &key {
            Key::Named(Named::Home) => edit(EditOp::Motion(Motion::DocStart, mods.shift())),
            Key::Named(Named::End) => edit(EditOp::Motion(Motion::DocEnd, mods.shift())),
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
            Message::Loaded(
                seq_first,
                Ok((doc, "UTF-8".to_owned())),
            ),
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
        // 不弹未保存确认条、不残留 pending 状态
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/big.log")));
        assert!(app.active_load.is_some());
        assert!(!app.dirty);

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
        dispatch(&mut app, Message::Loaded(seq, Ok((doc, "UTF-8".to_owned()))));
        assert!(app.active_load.is_none());
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
        let (id, Ok((doc, encoding))) = dones[0] else {
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
                        editpad_core::MatchPos { line: 0, col: 0 },
                        editpad_core::MatchPos { line: 1, col: 4 },
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
        let hit_a = || vec![editpad_core::MatchPos { line: 0, col: 0 }];
        let hit_b = || vec![editpad_core::MatchPos { line: 1, col: 4 }];
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
        app.matches = vec![editpad_core::MatchPos { line: 0, col: 0 }];
        let _ = app.update(Message::FindNext);
        assert_eq!(app.status, "查找中…", "扫描在途时 Enter 应提示进度");

        // 无结果且无在途扫描：懒补排一次扫描而不是误报「无匹配」
        let mut fresh = Editpad::default();
        fresh.find_visible = true;
        fresh.find_query = "zzz".into();
        let _ = fresh.update(Message::FindNext);
        assert!(fresh.find_scan.is_some(), "Enter 应懒触发一次后台扫描");
    }

    // ---------- P11 全部替换（rope 流式路径） ----------

    #[test]
    fn replace_all_swaps_document_and_sets_dirty() {
        let mut app = Editpad::default();
        app.editor
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("foo bar foo\nfoo"));
        app.find_visible = true;
        app.find_query = "foo".into();
        app.replace_query = "baz".into();

        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.editor.borrow().doc.to_text(),
            "baz bar baz\nbaz",
            "全部替换应改写文档内容"
        );
        assert!(app.dirty, "全部替换后必须置脏");
        assert_eq!(app.status, "已替换 3 处");

        // 无命中时不改文档也不置脏
        let mut app2 = Editpad::default();
        app2.editor
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("untouched"));
        app2.find_query = "zzz".into();
        app2.replace_query = "x".into();
        let _ = app2.update(Message::ReplaceAll);
        assert_eq!(app2.editor.borrow().doc.to_text(), "untouched");
        assert!(!app2.dirty);
        assert_eq!(app2.status, "已替换 0 处");
    }

    #[test]
    fn replace_all_is_blocked_while_scan_in_flight() {
        // P10 守卫在 P11 新路径上仍然生效：扫描在途时的全文快照可能过期
        let mut app = Editpad::default();
        app.editor
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("keep me"));
        app.find_query = "me".into();
        app.replace_query = "you".into();
        app.find_scan = Some(11);
        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.editor.borrow().doc.to_text(),
            "keep me",
            "后台扫描在途时不得执行全部替换"
        );
        assert!(!app.dirty);
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
            let mut ed = app.editor.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str(&text));
            ed.set_language(Some("rs"));
        }
        let gen = app
            .editor
            .borrow()
            .highlight_generation()
            .expect("已启用高亮");
        (app, gen)
    }

    fn pave_payload(app: &Editpad, batch: usize) -> (HlPavePayload, u64) {
        let (gen, highlighter) = app
            .editor
            .borrow()
            .highlight_pave_snapshot()
            .expect("已启用高亮");
        let doc = app.editor.borrow().doc.clone();
        (
            HlPavePayload {
                gen,
                doc,
                highlighter,
                total_lines: app.editor.borrow().doc.line_count(),
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
            app.editor.borrow().highlight_checkpoints_len(),
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
            app.editor.borrow().highlight_generation(),
            Some(gen0),
            "编辑必须换代"
        );
        let len_now = app.editor.borrow().highlight_checkpoints_len();

        // 迟到的旧代成果（内容无关紧要，代次闸门负责拒收）
        let (stale_payload, _) = pave_payload(&app, 32);
        let mut enriched_hl = stale_payload.highlighter.clone();
        enriched_hl.advance_checkpoints(4, 600, &mut |i| format!("let e{i} = {i};"));
        dispatch(&mut app, Message::HlPaved(gen0, enriched_hl));

        assert_eq!(app.hl_paving, None, "过期任务的登记必须解除");
        assert_eq!(
            app.editor.borrow().highlight_checkpoints_len(),
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
        plain.editor
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("plain text only\n"));
        dispatch(&mut plain, Message::HighlightPaveNeeded);
        assert_eq!(plain.hl_paving, None);
    }
}
