//! 扫描调度：活动页查找、在文件中查找、高亮铺路的排程与取消。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

impl Editpad {
    /// A8：按当前模式路由查询触发（查询变化/大小写/正则/整词开关共用
    /// ——FIF 开态驱动目录扫描，否则驱动活动文档扫描）
    pub(crate) fn schedule_active_scan(&mut self) -> Task<Message> {
        if self.fif_visible {
            self.schedule_fif_scan()
        } else {
            self.schedule_find_scan()
        }
    }

    /// A8：排队一次「在文件中查找」后台目录扫描。查询为空 = 取消在途
    /// 扫描（面板给提示）；目录未定（未命名页）/装载中 = 静默拒。
    /// 正则预校验与文档扫描同口径；代次 + 取消标志与 find_scan 同构。
    pub(crate) fn schedule_fif_scan(&mut self) -> Task<Message> {
        if !self.fif_visible || self.find_query.is_empty() {
            self.cancel_fif_scan();
            return Task::none();
        }
        let Some(dir) = self.fif_dir.clone() else {
            return Task::none();
        };
        if self.active_load.is_some() {
            return Task::none();
        }
        // 作废上一代任务（若它还在扫描中，下个文件循环检查即退出）
        self.fif_cancel.store(true, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.fif_cancel = cancelled.clone();
        self.fif_seq += 1;
        let effective_query = if self.regex_enabled {
            self.find_query.clone()
        } else {
            unescape_query(&self.find_query)
        };
        if self.regex_enabled {
            if let Err(e) = editpad_core::compile_regex(&effective_query, self.case_sensitive) {
                self.cancel_fif_scan();
                self.set_status_error(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StInvalidRegex, &e.to_string()));
                return Task::none();
            }
        }
        let payload = FifScanPayload {
            seq: self.fif_seq,
            dir,
            query: effective_query,
            case_sensitive: self.case_sensitive,
            regex: self.regex_enabled,
            whole_word: self.whole_word && !self.regex_enabled,
            cancelled,
            progress: Arc::new(AtomicUsize::new(0)),
            max_files: FIF_MAX_FILES,
            max_total_hits: FIF_MAX_TOTAL_HITS,
            max_hits_per_file: FIF_MAX_HITS_PER_FILE,
        };
        self.fif_progress = payload.progress.clone();
        self.fif_scan = Some(self.fif_seq);
        Task::perform(drive_find_in_files(payload), |message| message)
    }

    /// A8：取消在途目录扫描（结果跨开合保留，不清理 fif_results）
    pub(crate) fn cancel_fif_scan(&mut self) {
        self.fif_cancel.store(true, Ordering::Relaxed);
        self.fif_scan = None;
    }

    /// 排队一次后台查找扫描（P10）。查询为空或查找栏已关闭时转为取消。
    /// UI 线程只做廉价操作：文档快照是 rope 结构共享克隆，全文扫描
    /// 在防抖 200ms 后的后台线程进行，结果按序号回填。
    pub(crate) fn schedule_find_scan(&mut self) -> Task<Message> {
        if !self.find_visible || self.find_query.is_empty() {
            self.cancel_find_scan();
            return Task::none();
        }
        // 作废上一代任务（若它还睡在防抖窗口里，醒来即退出）
        self.find_cancel.store(true, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.find_cancel = cancelled.clone();
        self.find_seq += 1;
        // P70：正则模式下反斜杠是正则语法——不做 \n 等转义解析；
        // 且先编译校验（典型模式微秒级），无效立即提示不排后台任务
        let effective_query = if self.regex_enabled {
            self.find_query.clone()
        } else {
            // P22 补充：查询做转义解析（\n \r \t \\）后再扫描
            unescape_query(&self.find_query)
        };
        if self.regex_enabled {
            if let Err(e) = editpad_core::compile_regex(&effective_query, self.case_sensitive) {
                self.cancel_find_scan();
                self.set_status_error(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StInvalidRegex, &e.to_string()));
                return Task::none();
            }
        }
        let regex = self.regex_enabled;
        // 整词只作用于字面模式（正则的边界语义由模式自身表达）
        let whole_word = self.whole_word && !self.regex_enabled;
        let payload = FindScanPayload {
            seq: self.find_seq,
            doc: self.cur_handle.borrow().doc.clone(),
            query: effective_query,
            case_sensitive: self.case_sensitive,
            regex,
            cancelled,
            debounce_ms: FIND_DEBOUNCE_MS,
        };
        self.find_scan = Some(self.find_seq);
        Task::perform(drive_find_scan(payload, move |doc, q, cs, rx| {
            if rx {
                // P70：正则走全文扫描（to_text 拷贝发生在后台线程）；
                // 编译已在 UI 线程预校验，此处 Err 视为竞态失效回空表
                editpad_core::find_all_regex(&doc.to_text(), q, cs)
                    .unwrap_or_default()
            } else {
                let hits = editpad_core::find_all_document(doc, q, cs);
                if whole_word {
                    editpad_core::filter_whole_word(doc, hits)
                } else {
                    hits
                }
            }
        }), |message| message)
    }

    /// 取消在途扫描并清空结果（关查找栏/Esc/清空查询共用）。
    /// 序号递增 + 取消标志置位双保险，保证在途任务的结果回来后必被丢弃。
    pub(crate) fn cancel_find_scan(&mut self) {
        self.find_cancel.store(true, Ordering::Relaxed);
        self.find_seq += 1;
        self.find_scan = None;
        self.matches.clear();
        self.match_idx = None;
        self.sync_find_highlights();
    }

    /// P123：把当前命中表同步进编辑器的视口高亮层——查找栏开态下发
    /// 全部命中、关态清空。开/关/换结果的所有路径收口于此（关栏走
    /// cancel_find_scan，扫描完成走 FindScanDone）。
    pub(crate) fn sync_find_highlights(&mut self) {
        let hits = if self.find_visible { self.matches.clone() } else { Vec::new() };
        self.cur_handle.borrow_mut().set_find_highlights(hits);
    }

    pub(crate) fn find_scanning(&self) -> bool {
        self.find_scan.is_some()
    }

    /// 安排一次后台高亮铺建（P12）。
    ///
    /// * 同代已在途 → 幂等跳过（控件每帧 RedrawRequested 都可能喊一次）；
    /// * 期间发生过编辑（换代）→ 作废旧任务，从当前存活检查点重新出发；
    /// * UI 线程成本 = rope 结构共享克隆 + 高亮器检查点向量拷贝，零解析。
    pub(crate) fn schedule_highlight_pave(&mut self) -> Task<Message> {
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
            // P146：记下发起页 id——完成回报按它归页安装，不再装进
            // 「回报时刻的活动页」（A 页铺建中切到 B 页曾把 A 的语法
            // 状态装进 B，B 全文错色到下次编辑换代）
            tab_id: self.tabs[self.active_tab].id,
            doc,
            highlighter,
            total_lines,
            cancelled,
            batch_strides: HL_PAVE_BATCH_STRIDES,
        }))
    }

}
