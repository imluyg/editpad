//! Editpad —— 极简记事本。
//!
//! M2 里程碑：自绘虚拟化编辑器接管渲染，ropey Document 成为唯一数据源。
//!
//! * 渲染只为可见行排版（见 editor.rs）——50MB 与 5KB 的每帧成本相同；
//! * 键盘编辑在应用层订阅统一分发；鼠标与输入法在控件层处理；
//! * 撤销/重做基于 rope 的廉价结构共享快照。

mod editor;

use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;

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
    /// 剪贴板读取完成：归一换行后按普通插入入文
    Pasted(String),

    OpenRequested,
    FileChosen(Option<PathBuf>),
    /// 后台加载进度：(任务 id, 已读字节, 总字节)
    LoadProgress(u64, u64, u64),
    /// 加载完成：(任务 id, (全文, 编码标签))
    Loaded(u64, Result<(String, String), String>),
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

    GotoToggled,
    GotoInputChanged(String),
    GotoSubmit,

    RecentsToggled,
    RecentSelected(String),
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
    Done(Result<editpad_core::LoadedText, String>),
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
                |path, on_progress| editpad_core::load_file_streaming(path, on_progress),
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
async fn drive_load<L>(
    job_id: u64,
    path: PathBuf,
    loader: L,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
) where
    L: FnOnce(
        &Path,
        &mut dyn FnMut(editpad_core::LoadProgress),
    ) -> Result<editpad_core::LoadedText, editpad_core::CoreError>
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
                result.map(|loaded| (loaded.text, loaded.encoding.to_string())),
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

/// 粘贴内容的换行归一：`\r\n` 与孤立 `\r` 统一为 `\n`。
/// 文档内部以 `\n` 为主导行尾（P9 落地主导师行尾后此函数可下沉/退役）。
fn normalize_paste_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
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
                    self.refresh_matches();
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
                let normalized = normalize_paste_newlines(&text);
                if normalized.is_empty() {
                    Task::none()
                } else {
                    self.update(Message::Edit(EditOp::InsertText(normalized)))
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
                    Ok((contents, encoding)) => {
                        // 按扩展名启用语法高亮（未知类型自动退回纯文本）
                        let extension = self
                            .pending_path
                            .as_ref()
                            .and_then(|p| p.extension())
                            .and_then(std::ffi::OsStr::to_str)
                            .map(str::to_owned);
                        {
                            let mut ed = self.editor.borrow_mut();
                            ed.reset_document(editpad_core::Document::from_str(&contents));
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
                            self.refresh_matches();
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
                    self.refresh_matches();
                } else {
                    self.matches.clear();
                    self.match_idx = None;
                }
                Task::none()
            }
            Message::FindQueryChanged(query) => {
                self.find_query = query;
                self.refresh_matches();
                Task::none()
            }
            Message::FindNext => self.step_match(true),
            Message::FindPrev => self.step_match(false),
            Message::CaseToggled(value) => {
                self.case_sensitive = value;
                self.refresh_matches();
                Task::none()
            }
            Message::ReplaceQueryChanged(query) => {
                self.replace_query = query;
                Task::none()
            }
            Message::ReplaceCurrent => self.replace_current(),
            Message::ReplaceAll => {
                if self.busy || self.find_query.is_empty() {
                    return Task::none();
                }
                let contents = self.editor.borrow().doc.to_text();
                let (new_contents, count) = editpad_core::replace_all(
                    &contents,
                    &self.find_query,
                    &self.replace_query,
                    self.case_sensitive,
                );
                if count > 0 {
                    self.editor
                        .borrow_mut()
                        .replace_whole_document(editpad_core::Document::from_str(&new_contents));
                    self.dirty = true;
                }
                self.refresh_matches();
                self.status = format!("已替换 {count} 处");
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

            Message::BarsDismissed => {
                self.find_visible = false;
                self.goto_visible = false;
                self.recents_visible = false;
                // Esc 同时视作放弃关闭/打开确认
                self.confirm_visible = false;
                self.pending_close = false;
                self.open_confirm = None;
                self.matches.clear();
                self.match_idx = None;
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
        let contents = self.editor.borrow().doc.to_text();
        Task::perform(
            async move { editpad_core::save_atomic(&path, &contents) },
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

    fn refresh_matches(&mut self) {
        self.matches = if self.find_query.is_empty() {
            Vec::new()
        } else {
            let contents = self.editor.borrow().doc.to_text();
            editpad_core::find_all(&contents, &self.find_query, self.case_sensitive)
        };
        self.match_idx = None;
    }

    fn step_match(&mut self, forward: bool) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        if self.matches.is_empty() {
            self.refresh_matches();
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
            self.refresh_matches();
        }
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
            body = body.push(rule::horizontal(1)).push(panel);
        }

        if self.find_visible {
            let total = self.matches.len();
            let position_label = if total == 0 {
                "无匹配".to_owned()
            } else {
                match self.match_idx {
                    Some(i) => format!("第 {}/{} 处", i + 1, total),
                    None => format!("{total} 处"),
                }
            };
            let has_matches = !self.matches.is_empty();

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
                    button(text("全部替换")).on_press(Message::ReplaceAll),
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
        Key::Named(Named::Enter) => edit(EditOp::InsertText("\n".into())),
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
    fn paste_newlines_are_normalized() {
        // CRLF / 孤立 CR / 混合行尾全部归一为 \n；无行尾内容原样保留
        assert_eq!(normalize_paste_newlines("a\r\nb\rc\n\r\nd"), "a\nb\nc\n\nd");
        assert_eq!(normalize_paste_newlines("中文🚀"), "中文🚀");
        assert_eq!(normalize_paste_newlines(""), "");

        // 归一后的文本可整体经 insert_str 入文且光标落在末尾
        let mut c = editor::EditorCore::default();
        c.insert_str(&normalize_paste_newlines("x\r\ny"));
        assert_eq!(c.doc.to_text(), "x\ny");
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
                |p, cb| editpad_core::load_file_streaming(p, cb),
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
        let (id, Ok((text, encoding))) = dones[0] else {
            panic!("应为成功加载，实际 {:?}", dones[0]);
        };
        assert_eq!(id, 7);
        assert_eq!(text.as_str(), content.as_str());
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
                |_p, _cb| -> Result<editpad_core::LoadedText, editpad_core::CoreError> {
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
}
