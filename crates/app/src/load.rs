use super::*;

/// 后台加载线程 → 订阅流的事件。
pub(crate) enum LoadEvent {
    Progress(editpad_core::LoadProgress),
    Done(Result<editpad_core::LoadedDocument, String>),
}

/// 后台加载任务：订阅标识 + 目标路径 + 目标标签页（P21 路由归属）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LoadJob {
    pub(crate) id: u64,
    pub(crate) path: PathBuf,
    /// 结果应写入的标签下标（登记时刻的值；恢复队列的平移记账仍用它）
    pub(crate) tab: usize,
    /// P145：目标页的稳定 id——`Loaded` 归页按 id 解析实际位置，
    /// 期间关页/换位导致的下标漂移不再串页/越界。
    pub(crate) tab_id: u64,
    /// P377（池 ⑩）：**登记时刻**的界面语言。错误文案在后台线程成形，那里拿不到
    /// 设置；跟着任务走而不是跟着订阅走——中途换语言不会撤销在途加载，
    /// 也不会让已经发起的那次失败换一种说法。
    pub(crate) lang: editpad_core::Lang,
}

/// 把一次加载任务构造成事件流（OS 线程做阻塞 IO，std mpsc 桥接到异步端）。
pub(crate) fn build_load_stream(job: &LoadJob) -> impl iced::futures::Stream<Item = Message> {
    let job = job.clone();
    stream::channel(
        64,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            drive_load(
                job.id,
                job.lang,
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
pub(crate) async fn drive_load<L>(
    job_id: u64,
    lang: editpad_core::Lang,
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
            // P377（池 ⑩）：文案在后台线程成形，界面语言只能随任务带进来——
            // 原先直接 `to_string()` 取的是 `CoreError` 的 `Display`（恒中文），
            // 于是英文界面的「打开失败：」后面跟着一句中文。
            Ok(result) => LoadEvent::Done(result.map_err(|e| e.message(lang))),
            Err(payload) => {
                // 必须显式 &*payload 解出内部值：直接传 &payload 时，
                // 因为 Box<dyn Any + Send> 自身也实现了 Any，编译器会走
                // unsize 强制转换（把整个 Box 当作 Any 对象）而不是解引用，
                // downcast 的 TypeId 就对不上了（实测踩坑）。
                let message = panic_message(&*payload);
                LoadEvent::Done(Err(format!(
                    "{}{message}",
                    editpad_core::Key::StLoadThreadCrashed.text(lang)
                )))
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
                result.map(|loaded| (loaded.doc, loaded.sample, loaded.encoding.to_string())),
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
                Err(editpad_core::Key::StLoadStreamEnded.text(lang).to_owned()),
            ))
            .await;
    }
}

/// 把 panic 载荷转成可读描述（P5 兜底提示用）。
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知原因".to_owned()
    }
}
