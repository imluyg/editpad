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

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};

use iced::futures::SinkExt;
use iced::keyboard::{self, key::Named};
use iced::widget::{button, checkbox, column, container, progress_bar, row, rule, scrollable,
    text, text_input};
use iced::{stream, window, Alignment, Element, Fill, Font, Subscription, Task, Theme};

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
    /// 切换 Markdown 预览面板（仅当前语法为 Markdown 时生效；P22 第三批）
    PreviewToggled,
    /// 光标闪烁心跳（打磨项）：翻转闪烁相位并触发重绘
    CaretTick,
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
fn markdown_preview_element(source: &str) -> Element<'static, Message> {
    use iced::font::Weight;

    let blocks = editpad_core::markdown::parse_markdown(source);
    let mut col = column![].spacing(10).padding(14);
    for block in blocks {
        match block {
            editpad_core::markdown::MdBlock::Heading { level, spans } => {
                let px = match level {
                    1 => 26.0,
                    2 => 23.0,
                    3 => 20.0,
                    4 => 18.0,
                    5 => 17.0,
                    _ => 16.0,
                };
                col = col.push(md_spans_row(spans, px, true));
            }
            editpad_core::markdown::MdBlock::Paragraph { spans } => {
                col = col.push(md_spans_row(spans, 15.0, false));
            }
            editpad_core::markdown::MdBlock::ListItem { spans } => {
                let mut line = row![text("• ").size(15)];
                for span in spans {
                    let font = md_font(span.is_bold(), span.is_italic());
                    let mut t = text(span.text.clone()).size(15).font(font);
                    if span.is_code() {
                        t = t.color([0.12, 0.36, 0.6]);
                    }
                    line = line.push(t);
                }
                col = col.push(line);
            }
            editpad_core::markdown::MdBlock::Quote { spans } => {
                let mut line = row![text("▌ ").color([0.55, 0.55, 0.6])];
                for span in spans {
                    let font = md_font(span.is_bold(), span.is_italic());
                    let t = text(span.text.clone())
                        .size(15)
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
                            .size(14)
                            .font(Font {
                                weight: Weight::Normal,
                                ..Font::MONOSPACE
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
            ..Font::MONOSPACE
        };
        let mut t = text(span.text.clone()).size(px).font(font);
        if span.is_code() {
            t = t.color([0.12, 0.36, 0.6]);
        }
        row = row.push(t);
    }
    row
}

fn md_font(bold: bool, italic: bool) -> Font {
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
        ..Font::MONOSPACE
    }
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
    // ---------- 未命名页编号（P25） ----------
    /// 下一个未命名页序号（全局单调，不复用已关闭页的号码）
    untitled_next: u64,

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
            pending_close_tab: None,
            dark_mode: false,
            preview_visible: false,
            // P25：初始页即「未命名1」，下一个新页为「未命名2」
            untitled_next: 2,
        }
    }
}

impl Editpad {
    // ---------- 多标签访问器（P21） ----------

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
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::empty());
            let last = self.tabs.len() - 1;
            self.assign_untitled_num(last);
        }
        // 与 tabs 对齐（含越界夹紧），并同步活动页句柄别名
        self.refresh_cur_handle();
        true
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
        let settings = editpad_core::Settings::load();
        let dark_mode = settings.is_dark();
        // 设置里的字号可能未归一（旧配置/手改），boot 时按同一规则 clamp
        let font_size = editor::normalize_font_size(settings.font_size);
        let state = Self {
            settings,
            dark_mode,
            ..Self::default()
        };
        state.cur().borrow_mut().set_font_size(font_size);
        // P18 打磨：启动光标闪烁心跳链（自我续期，占用一个睡眠节拍）
        let caret_chain = Task::perform(
            async {
                std::thread::sleep(std::time::Duration::from_millis(
                    editor::CARET_BLINK_MS,
                ));
            },
            |_| Message::CaretTick,
        );
        (state, caret_chain)
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
                match result {
                    Ok((doc, sample, encoding)) => {
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
                        }
                        tab.path = Some(job.path.clone());
                        tab.encoding_label = encoding;
                        tab.dirty = false;
                        // P25：真实文件已就位，未命名序号使命完成
                        tab.untitled_num = None;
                        if let Some(path) = self.path_of_tab(target) {
                            self.record_recent(&path);
                        }
                        self.busy = false;
                        // 新文档已就位：旧文档的关闭确认语义过期
                        self.confirm_visible = false;
                        self.pending_close = false;
                        self.status.clear();
                        if self.find_visible && target == self.active_tab {
                            return self.schedule_find_scan();
                        }
                    }
                    Err(error) => {
                        self.busy = false;
                        self.status = format!("打开失败:{error}");
                    }
                }
                Task::none()
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
                    // 落盘确认后才真正关窗
                    self.pending_close = false;
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
                // 捕获主窗口 id（仅有的窗口），供后续 window::close 使用
                self.main_window = Some(id);
                // P21：任一标签页置脏即弹确认（聚合口径）
                if self.any_dirty() {
                    self.confirm_visible = true;
                    Task::none()
                } else {
                    self.close_window()
                }
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
                // P21：放弃关闭 = 全部标签页的未保存标记一并放弃
                for tab in &mut self.tabs {
                    tab.dirty = false;
                }
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
                // P21：Esc 也取消标签页关闭确认
                self.close_tab_confirm = None;
                // P10：取消在途扫描 + 清结果（含序号失效）
                self.cancel_find_scan();
                Task::none()
            }

            // ---------- 多标签（P21） ----------
            Message::NewTab => {
                self.tabs.push(Tab::empty());
                let last = self.tabs.len() - 1;
                self.assign_untitled_num(last);
                self.set_active_tab(last);
                // 查找态全局：切页即作废旧命中，防串页
                self.cancel_find_scan();
                Task::none()
            }
            Message::SwitchTabNext => {
                let next = (self.active_tab + 1) % self.tabs.len();
                self.set_active_tab(next);
                self.cancel_find_scan();
                Task::none()
            }
            Message::SwitchTab(i) => {
                if i < self.tabs.len() && i != self.active_tab {
                    self.set_active_tab(i);
                    self.cancel_find_scan();
                }
                Task::none()
            }
            Message::CloseTabRequest => {
                let idx = self.active_tab;
                if self.tabs[idx].dirty {
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
                self.busy = true;
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
                self.cur_handle.borrow_mut().set_font_size(next);
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
            self.tab_mut().dirty = true;
            // P18：内容版本 +1 并刷新防抖起点（自动保存的触发依据）
            self.tab_mut().note_mutation();
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
        self.job_seq += 1;
        let job = LoadJob {
            id: self.job_seq,
            path: path.clone(),
            tab,
        };
        if tab >= self.tabs.len() {
            self.tabs.push(Tab::empty());
        }
        self.active_load = Some(job);
        self.progress = Some((0, 0));
        self.busy = true;
        // P21：加载落在新页时直接切过去（符合「打开即聚焦」直觉）
        self.active_tab = tab;
        self.status.clear();
        Task::none()
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
        if self.busy || self.tab().path.is_none() {
            return Task::none();
        }
        self.busy = true;
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
            // P22 补充：长度按转义解析后的查询计（含 \n 等不可见字符）
            let query_len = unescape_query(&self.find_query).chars().count();
            self.cur_handle.borrow_mut().select_span(pos.line, pos.col, query_len);
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
            editor
                .selected_text()
                .is_some_and(|selected| {
                    strings_equal(&selected, &effective_query, self.case_sensitive)
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

    fn view(&self) -> Element<'_, Message> {
        // P22 第三批：当前页是否为 Markdown（决定预览按钮可用性）
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");

        let toolbar = row![
            button(text("打开…"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::OpenRequested)),
            button(text("保存"))
                .padding([4, 12])
                .on_press_maybe((!self.busy && self.tab().dirty)
                    .then_some(Message::SaveRequested)),
            button(text("另存为…"))
                .padding([4, 12])
                .on_press_maybe((!self.busy).then_some(Message::SaveAsRequested)),
            // P22 第三批：Markdown 预览开关（仅 Markdown 语法页可用）
            button(text(if self.preview_visible {
                "关闭预览"
            } else {
                "MD 预览"
            }))
            .padding([4, 12])
            .on_press_maybe(is_markdown.then_some(Message::PreviewToggled)),
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
            text(if self.tab().dirty { "● 未保存" } else { "" }).color([0.85, 0.55, 0.1]),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 10]);

        // P21 标签条：恒显示（单页也给出「当前文件名」的可见反馈）。
        // 点击切换；置脏页带 ● 前缀；活动页加 ▸ 指示。
        let mut body = column![toolbar, rule::horizontal(1)];
        {
            let mut strip = row![].spacing(2).padding([4, 6]);
            for (i, tab) in self.tabs.iter().enumerate() {
                let marker = if i == self.active_tab { "▸ " } else { "  " };
                strip = strip.push(
                    button(text(format!(
                        "{marker}{}",
                        tab.display_name()
                    )))
                    .padding([2, 10])
                    .on_press_maybe((!self.busy).then_some(Message::SwitchTab(i))),
                );
            }
            body = body.push(strip);
        }

        // 中间主区域：Markdown 预览面板 或 自绘虚拟化编辑器
        if self.preview_visible && is_markdown {
            let text = self.cur_handle.borrow().doc.to_text();
            body = body.push(markdown_preview_element(&text));
        } else {
            body = body.push(self.cur_handle.view());
        }

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

        // P21 标签页关闭确认：骨架版仅提供「放弃更改并关闭」出口，
        // 想保留改动请先 Ctrl+S（完整保存后关闭随 P21 完整版补齐）
        if let Some(idx) = self.close_tab_confirm {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!(
                        "第 {} 个标签页有未保存的更改",
                        idx.saturating_add(1)
                    )),
                    button(text("放弃更改并关闭"))
                        .padding([4, 12])
                        .on_press(Message::ConfirmCloseTabDiscard(idx)),
                    // P21 完整版：已命名的页可直接「保存并关闭」
                    button(text("保存并关闭"))
                        .padding([4, 12])
                        .on_press_maybe(
                            (!self.busy && self.tabs[idx].path.is_some())
                                .then_some(Message::CloseTabSave(idx)),
                        ),
                    button(text("取消"))
                        .padding([4, 12])
                        .on_press(Message::CancelCloseTab),
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
                    button(text("放弃更改并打开"))                        .padding([4, 12])
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

        let cursor = self.cur_handle.borrow().cursor;
        let status_bar = row![
            text(
                self.tab()
                    .path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| format!("({})", self.tab().base_name()))
            )
            .width(Fill),
            text(if self.tab().encoding_label.is_empty() {
                "—".to_owned()
            } else {
                self.tab().encoding_label.clone()
            }),
            text(format!("{} 行", self.cur_handle.borrow().doc.line_count())),
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
        // 不弹未保存确认条、不残留 pending 状态
        let mut app = Editpad::default();
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
}
