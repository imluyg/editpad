use super::*;

/// 每个后台批次铺建的检查点档位数。一档 = STRIDE 行 ≈ 10ms 量级，
/// 32 档约 0.3s——进度粒度足够顺滑，取消响应延迟有界。
pub(crate) const HL_PAVE_BATCH_STRIDES: usize = 32;

/// 一次后台高亮铺建任务的输入快照。`doc` 与 `highlighter` 都是调度
/// 时刻的克隆（rope 结构共享 O(1) / 检查点向量拷贝）；worker 只读它们、
/// 推进自己的高亮器副本，完成时整体送回，不做任何共享可变状态。
pub(crate) struct HlPavePayload {
    pub(crate) gen: u64,
    /// P146：发起页 id——完成回报按它归页安装。曾装进「回报时刻的
    /// 活动页」：A 页大文件铺建中切到 B 页（同语言、代次同为 0），
    /// A 的检查点状态会被装进 B，B 全文按 A 的语法状态错色。
    pub(crate) tab_id: u64,
    pub(crate) doc: editpad_core::Document,
    pub(crate) highlighter: editpad_core::LazyHighlighter,
    pub(crate) total_lines: usize,
    /// 取消标志：编辑换代/新任务排队时置位，worker 批间检查后提前收工。
    /// 收工时的部分成果仍然有效（完整档位只依赖快照内的一致前缀），
    /// 会照常送回安装。
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) batch_strides: usize,
}

/// worker 的推进循环：反复 [`advance_checkpoints`](editpad_core::LazyHighlighter::advance_checkpoints)
/// 直到铺满/取消/无活可干；每批后上报累计档位数。
/// 独立成函数以便测试注入 panic 路径（同 drive_load 的 loader 注入）。
pub(crate) fn pave_run(payload: &HlPavePayload, report: &mut dyn FnMut(u64)) -> editpad_core::LazyHighlighter {
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
pub(crate) fn build_hl_pave_stream(payload: HlPavePayload) -> impl iced::futures::Stream<Item = Message> {
    stream::channel(
        8,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            drive_hl_pave(payload, pave_run, &mut output).await;
        },
    )
}

/// 铺建任务的事件驱动（推进函数可注入以便测试）。
pub(crate) async fn drive_hl_pave<F>(
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
    let tab_id = payload.tab_id;
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
            HlEvent::Done(hl) => Message::HlPaved(gen, tab_id, hl),
        };
        if output.send(message).await.is_err() {
            break;
        }
    }
}
