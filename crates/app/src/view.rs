use super::*;
use iced::widget::column;
use super::settings_ui::{
    chrome_button_style, chrome_menu_item_style, menubar_text_style, tab_close_style,
    tab_label_style, tab_pill_style,
};

impl Editpad {
    /// 应用主题（boot 从设置读入，工具栏可切换）。
    pub(crate) fn theme(&self) -> Theme {
        if self.dark_mode {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// 设置里归一后的当前字号（显示与按钮可用性判断都用它）。
    pub(crate) fn display_font_size(&self) -> f32 {
        editor::normalize_font_size(self.settings.font_size)
    }

    /// 正文与 UI 的统一字形族（P34 换装点）：生效族名 Some → 以
    /// `Family::Name` 引用（系统字体已在 fontdb 里，无需装载字节），
    /// None / 未配置 → 默认等宽（P33 的 CJK 钉字仍生效）。
    /// iced 排版缓存按 Font 值做键——运行期换族即换键，无陈旧缓存问题。
    pub(crate) fn body_font(&self) -> Font {
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
    pub(crate) fn discard_all_and_close(&mut self, snapshot_dir: Option<PathBuf>) -> Task<Message> {
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
    pub(crate) fn prepare_heartbeat_commit(&self, dir: &Path) -> Option<HeartbeatPayload> {
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
    pub(crate) fn heartbeat_apply(&mut self, outcome: HeartbeatOutcome) {
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
                self.set_status_error(format!("快照心跳失败:{error}"));
            }
        }
    }

    /// 同步执行一次完整心跳周期——生产路径走异步 [`Self::drive_heartbeat`]，
    /// 本入口供测试端到端验证磁盘产物与账目回填（共用 prepare/apply 两半）。
    #[cfg(test)]
    pub(crate) fn run_heartbeat_cycle(&mut self, dir: &Path) -> Option<HeartbeatOutcome> {
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
    pub(crate) fn handle_close_request(&mut self, id: window::Id, snapshot_dir: Option<PathBuf>) -> Task<Message> {
        // 捕获主窗口 id（仅有的窗口），供后续 window::close 使用
        self.main_window = Some(id);
        // P102：窗口几何兜底落盘——最后一次拖动/拉伸可能仍在节流窗内，
        // 关窗前补写（门控 = 本会话确有几何变化：测试环境不发窗口事件，
        // 恒不触碰真实配置目录）
        if self.last_geometry_persist.is_some() {
            self.persist_settings();
        }
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
    pub(crate) fn exit_via_snapshot(&mut self, dir: &Path) -> Task<Message> {
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
                self.set_status_error(format!("会话快照失败:{error}"));
                self.confirm_or_close()
            }
        }
    }

    /// 关闭主窗口；id 来自 close_requests 订阅的捕获。
    pub(crate) fn close_window(&self) -> Task<Message> {
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
    pub(crate) fn boot_restore(&mut self) -> Task<Message> {
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
    pub(crate) fn restore_from_dir(&mut self, dir: &Path) -> Task<Message> {
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
    pub(crate) fn restore_from_manifest(
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
    pub(crate) fn begin_restore_load(&mut self) -> Task<Message> {
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
    pub(crate) fn settle_restore_step(&mut self) -> bool {
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
            self.set_status(format!("会话恢复完成:{}", notes.join("，")));
        }
        self.restore_failed = 0;
        self.restore_dropped = 0;
        self.restore_pending = 0;
    }

    /// 移除第 idx 个恢复占位页，并把队列中大于 idx 的目标下标整体前移
    /// （页面移除后，后续排队页的下标随之左移）。仅当该页仍是空净无名
    /// 占位页才动手——防误删恢复期间用户产生的内容。
    pub(crate) fn drop_restore_placeholder(&mut self, idx: usize) {
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
    pub(crate) fn restore_placeholder_ready(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|t| {
            t.path.is_none()
                && !t.dirty
                && t.untitled_num.is_none()
                && t.encoding_label.is_empty()
                && t.editor.borrow().doc.is_empty()
        })
    }

    /// 崩溃恢复提示条「恢复」：按暂存清单全量重建（含脏页内容）。
    pub(crate) fn accept_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
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
    pub(crate) fn discard_session_recover(&mut self, dir: Option<PathBuf>) -> Task<Message> {
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
    pub(crate) fn schedule_find_scan(&mut self) -> Task<Message> {        if !self.find_visible || self.find_query.is_empty() {
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
                self.set_status_error(format!("正则无效：{e}"));
                return Task::none();
            }
        }
        let regex = self.regex_enabled;
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
        Task::perform(drive_find_scan(payload, |doc, q, cs, rx| {
            if rx {
                // P70：正则走全文扫描（to_text 拷贝发生在后台线程）；
                // 编译已在 UI 线程预校验，此处 Err 视为竞态失效回空表
                editpad_core::find_all_regex(&doc.to_text(), q, cs)
                    .unwrap_or_default()
            } else {
                editpad_core::find_all_document(doc, q, cs)
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
            doc,
            highlighter,
            total_lines,
            cancelled,
            batch_strides: HL_PAVE_BATCH_STRIDES,
        }))
    }

    /// 第 62 轮：按索引跳到「查找全部」结果面板中的某一条命中。
    /// 与 [`Self::step_match`] 同一 select_span 口径（len_chars 自带
    /// 选区跨度），只是定位方式从光标相对序改为面板行号直选。
    pub(crate) fn goto_match_index(&mut self, index: usize) {
        let Some(pos) = self.matches.get(index).copied() else {
            return;
        };
        self.match_idx = Some(index);
        self.cur_handle
            .borrow_mut()
            .select_span(pos.line, pos.col, pos.len_chars);
        self.set_status(format!("第 {}/{} 处匹配", index + 1, self.matches.len()));
    }

    /// 第 63 轮：复制某页的完整路径或文件名到剪贴板。
    /// `target`：None = 活动页（热键），Some(i) = 指定页（右键菜单）；
    /// 未命名页无路径可写，给状态栏提示不产生剪贴板写入。
    pub(crate) fn copy_tab_ident(&mut self, target: Option<usize>, full_path: bool) -> Task<Message> {
        let idx = target.unwrap_or(self.active_tab);
        let Some(tab) = self.tabs.get(idx) else {
            return Task::none();
        };
        let Some(path) = tab.path.clone() else {
            self.set_status("未命名页没有路径可复制（先保存）".to_owned());
            return Task::none();
        };
        let payload = if full_path {
            path.display().to_string()
        } else {
            path.file_name()
                .map(|n| n.display().to_string())
                .unwrap_or_else(|| path.display().to_string())
        };
        self.set_status(format!("已复制 {payload}"));
        iced::clipboard::write(payload)
    }

    pub(crate) fn step_match(&mut self, forward: bool) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        // 扫描在途：不基于过期命中表跳转
        if self.find_scanning() {
            self.set_status("查找中…".to_owned());
            return Task::none();
        }
        if self.matches.is_empty() {
            // 懒触发：开栏即按 Enter 而扫描还没排队过时，先补一次扫描
            return self.schedule_find_scan();
        }

        // 「上一个」以选区起点为原点：光标/选区正落在某命中上时一步跳到
        // 上一处——以命中末尾为原点会让第一按重新选中当前命中（需两按）；
        // 「下一个」维持光标（命中末尾）原点不变
        let (origin_line, origin_col) = {
            let ed = self.cur_handle.borrow();
            if forward {
                (ed.cursor.line, ed.cursor.col)
            } else {
                let (start, _) = ed.ordered_selection().unwrap_or((ed.cursor, ed.cursor));
                (start.line, start.col)
            }
        };
        let index = if forward {
            editpad_core::next_from(&self.matches, origin_line, origin_col)
        } else {
            editpad_core::prev_from(&self.matches, origin_line, origin_col)
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
            self.set_status(format!("第 {}/{} 处匹配", i + 1, self.matches.len()));
        }
        Task::none()
    }

    pub(crate) fn replace_current(&mut self) -> Task<Message> {
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

    pub(crate) fn title(&self) -> String {
        let name = self.tab().base_name();
        if self.tab().dirty {
            format!("● {name} - Editpad")
        } else {
            format!("{name} - Editpad")
        }
    }

    pub(crate) fn suggested_name(&self) -> String {
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
                    .style(chrome_button_style)
                    .on_press(Message::TabContextMenuClosed),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(2)
        .padding([4, 10]);

        // 固定 / 取消固定（固定页豁免一切关闭路径）。
        // P56：菜单项一律中性列表样式（透明底+悬停淡染），不再用 iced
        // 默认实心蓝（用户截图：菜单项像全选中的高亮条）
        let pin_label = if tab.pinned { "取消固定" } else { "📌 固定标签页" };
        panel = panel.push(
            button(container(text(pin_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(interactive.then_some(Message::TogglePinTab(idx))),
        );
        // 保存：仅置脏可用（与工具栏「保存」同一口径）；
        // 未命名页在 update 层自动落另存为
        panel = panel.push(
            button(container(text("保存").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && tab.dirty).then_some(Message::SaveTabFromMenu(idx)),
                ),
        );
        // 另存为 / 重命名（v1 同一动作兜底，§3 P28 第 2 条）
        let rename_label = if tab.path.is_some() { "重命名…" } else { "另存为…" };
        panel = panel.push(
            button(container(text(rename_label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(interactive.then_some(Message::RenameOrSaveAsTab(idx))),
        );
        // 第 63 轮：复制完整路径 / 文件名（未命名页无路径，菜单项灰掉）
        let named = tab.path.is_some();
        panel = panel.push(
            button(container(text("复制完整路径").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && named).then_some(Message::CopyFilePath(Some(idx))),
                ),
        );
        panel = panel.push(
            button(container(text("复制文件名").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
                .on_press_maybe(
                    (interactive && named).then_some(Message::CopyFileName(Some(idx))),
                ),
        );

        panel = panel.push(rule::horizontal(1));

        // 关闭：固定页拒绝（update 层守卫 + 菜单项灰掉双保险）
        panel = panel.push(
            button(container(text("关闭").size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .style(chrome_menu_item_style)
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
            .style(chrome_menu_item_style)
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
            .style(chrome_menu_item_style)
            .on_press_maybe(
                (!right.is_empty() && interactive).then_some(Message::CloseTabsRight(idx)),
            ),
        );
        panel.into()
    }

    // 设置面板渲染（settings_panel/sidebar/content/row_widget/stepper/font_picker 等 8 方法）已迁往 settings_ui.rs（第 82 轮 Phase 2a）

pub(crate) fn view(&self) -> Element<'_, Message> {
        // P22 第三批：当前页是否为 Markdown（决定预览按钮可用性）
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
        // P33/P36：UI 全部控件与正文同族，字号固定不随正文缩放——
        // Ctrl+滚轮（P48）与设置面板只调节文件内容；P34：族随设置切换
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();

        // 第 69 轮：顶部菜单栏（文件/编辑/查看/视图/设置）——整条包
        // mouse_area 跟踪指针作浮层锚点（仿标签条 P39 模式）；展开的
        // 菜单以高亮态标示。功能项自工具栏收编迁移（见各 menubar_panel）。
        // 第 76 轮（用户点单）：按钮改纯文字扁平（menubar_text_style，
        // 无背景无边框无凸起），行高压缩（按钮 padding [1,8] + 行内
        // padding [0,4]）——看起来不像按钮、更紧凑。
        let menu_names = ["文件", "编辑", "查看", "设置"];
        let mut menubar_inner = row![].spacing(4);
        for (idx, name) in menu_names.iter().enumerate() {
            let open = self.menu_bar_open == Some(idx);
            menubar_inner = menubar_inner.push(
                button(
                    text(*name).size(uipx).font(uifont),
                )
                .padding([1, 8])
                .style(move |theme, status| menubar_text_style(theme, status, open))
                .on_press_maybe((!self.busy).then_some(Message::MenuToggled(idx))),
            );
        }
        let menubar = mouse_area(
            menubar_inner.padding([0, 4]),
        )
        .on_move(Message::MenubarHovered);

        // 第 70 轮：工具栏整体移除——打开/保存/查找/设置全部由顶部菜单
        // 栏承载（用户裁决：与文件菜单重复）；置脏指示由标签页 ● 前缀
        // （P21）与标题栏承担。查找/替换快捷入口 = Ctrl+F 与编辑菜单。

        // P21 标签条：恒显示（单页也给出「当前文件名」的可见反馈）。
        // 点击切换；置脏页带 ● 前缀；活动页加 ▸ 指示；固定页加 📌（P28）。
        // P28：按钮外包 mouse_area 捕获右键弹菜单——MouseArea 先把事件
        // 交给子组件，左键被 button 捕获后自身跳过，故切换不受影响；
        // 右键无人捕获，落到 on_right_press。
        // P39：整条标签条再包一层 mouse_area 跟踪指针位置（浮层菜单
        // 锚点数据源；只在标签条悬停时产生消息，量级可忽略）。
        let mut body = column![menubar, rule::horizontal(1)];
        {
            // P100 勘误：P98 把「空白双击」命中面从 Fill 占位按钮改到外层
            // mouse_area 时，strip Row 默认 Shrink 宽——外层鼠标区只盖住
            // 标签本身，右侧空白不在命中面内，双击无消息。补 width(Fill)
            // 让整条标签条横贯窗口（无任何 Fill 高子项，P98 的半窗回归
            // 不会重现），外层 on_double_click 全覆盖右侧空白。
            let mut strip = row![].spacing(2).padding([4, 6]).width(Fill);
            // 注：窗口标题栏图标见 [`crate::window_title_icon`]（main.rs）；
            // 标签条保持纯文本前缀（▸ 活动 / ● 置脏 / 📌 固定，P21/P28
            // 语义，用户点名「标签栏黑点保持原样」不动）。
            for (i, tab) in self.tabs.iter().enumerate() {
                // P55：就地重命名——该页的标签按钮替换为输入框 + ✓/× 微型按钮
                // （Enter 等价 ✓；Esc 走 BarsDismissed 取消）
                if self.renaming_tab == Some(i) {
                    strip = strip.push(
                        row![
                            text_input("新名称", &self.rename_input)
                                .id(rename_input_id()) // P64：与聚焦操作同源
                                .size(uipx)
                                .font(uifont)
                                .on_input(Message::TabRenameInputChanged)
                                .on_submit(Message::TabRenameCommitted)
                                .width(150),
                            button(text("✓").size(uipx).font(uifont))
                                .padding([2, 7])
                                .style(chrome_button_style)
                                .on_press(Message::TabRenameCommitted),
                            button(text("×").size(uipx).font(uifont))
                                .padding([2, 7])
                                .style(chrome_button_style)
                                .on_press(Message::TabRenameCancelled),
                        ]
                        .spacing(2)
                        .align_y(Alignment::Center),
                    );
                    continue;
                }
                let marker = if i == self.active_tab { "▸ " } else { "  " };
                let pin = if tab.pinned { "📌 " } else { "" };
                // P57：页签换中性样式——活动页淡底描边、非活动透明悬停淡染
                let active = i == self.active_tab;
                // P112：页签 = 「胶囊容器 + 文字按钮 + × 关闭按钮」。
                // 胶囊底画在容器上（活动/悬停染色），内部两个按钮全透明
                // 扁平——若底色由两个按钮各自画会露接缝；悬停数据源 =
                // 指针进出页签的 TabHovered（纯 UI 态）。× 与右键菜单
                // 「关闭」同消息同口径（置脏弹确认条、busy 置灰），固定
                // 页不渲染 ×（豁免口径与菜单一致）；文字按钮仍承接
                // SwitchTab（含 P65 双击重命名）。
                let closeable = !tab.pinned;
                let mut pill = row![
                    button(
                        text(format!(
                            "{marker}{pin}{}",
                            tab.display_name()
                        ))
                        .size(uipx)
                        .font(uifont),
                    )
                    .padding(Padding {
                        top: 2.0,
                        right: if closeable { 4.0 } else { 10.0 },
                        bottom: 2.0,
                        left: 10.0,
                    })
                    .style(tab_label_style)
                    .on_press_maybe((!self.busy).then_some(Message::SwitchTab(i))),
                ]
                .align_y(Alignment::Center);
                if closeable {
                    pill = pill.push(
                        button(text("×").size(uipx).font(uifont))
                            .padding([2, 8])
                            .style(tab_close_style)
                            .on_press_maybe(
                                (!self.busy).then_some(Message::CloseTabAt(i)),
                            ),
                    );
                }
                let hovered = self.hovered_tab == Some(i);
                let pill_area = mouse_area(
                    container(pill)
                        .style(move |theme| tab_pill_style(theme, active, hovered)),
                )
                .on_enter(Message::TabHovered(Some(i)))
                .on_exit(Message::TabHovered(None))
                .on_right_press(Message::TabContextMenu(i));
                // 中键关闭（主流浏览器/编辑器手感）；置脏确认与固定页
                // 豁免由 CloseTabAt 处理器统一判定，busy 守卫与 × 按钮同口径
                let pill_area = if self.busy {
                    pill_area
                } else {
                    pill_area.on_middle_press(Message::CloseTabAt(i))
                };
                strip = strip.push(pill_area);
            }
            // 第 76 轮（用户点单）：标签条右侧空白区双击新建标签页——
            // 命中面就是整条标签条本身（外层 mouse_area 的 on_double_click）：
            // 标签按钮/重命名输入框会先消费左键（iced 事件委托语义，P28
            // 同款），空白区无子组件消费 → 双击判定落在外层。因此**不**
            // 需要额外的 Fill 按钮占位（此前版本用 Space(Fill, Fill) 撑高
            // 了标签条、占半窗——已移除）。双击现有标签不触发（其 click
            // 被 button 消费），与 P65 重命名互不干扰。
            body = body.push(
                mouse_area(strip)
                    .on_move(Message::CursorMoved)
                    .on_double_click(Message::TabStripBlankPressed),
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
                        .style(chrome_menu_item_style)
                        .on_press_maybe(
                            (!self.busy).then_some(Message::RecentSelected(entry.clone())),
                        ),
                );
            }
            // 第 64 轮：会话内「上次关闭」栈非空时，置顶提供一键恢复
            if !self.closed_stack.is_empty() {
                panel = panel.push(
                    button(
                        container(text(format!(
                            "恢复上次关闭的文件（{}）",
                            self.closed_stack[0]
                                .file_name()
                                .map(|n| n.display().to_string())
                                .unwrap_or_else(|| self.closed_stack[0].display().to_string())
                        ))
                        .size(uipx)
                        .font(uifont))
                        .width(Fill),
                    )
                    .width(Fill)
                    .style(chrome_menu_item_style)
                    .on_press_maybe(
                        (!self.busy).then_some(Message::ReopenLastClosedFile),
                    ),
                );
            }
            // P20 隐私出口：一键抹掉 config.toml 里的全部历史路径
            if !self.settings.recent_files.is_empty() {
                panel = panel.push(
                    row![
                        button(text("清空记录").size(uipx).font(uifont))
                            .padding([2, 8])
                            .style(chrome_button_style)
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
                        .style(chrome_button_style)
                        .on_press_maybe(has_matches.then_some(Message::FindPrev)),
                    button(text("↓ 下一个").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press_maybe(has_matches.then_some(Message::FindNext)),
                    checkbox(self.case_sensitive)
                        .label("区分大小写")
                        .text_size(uipx)
                        .font(uifont)
                        .on_toggle(Message::CaseToggled),
                    // P70：正则模式开关（.* 是各编辑器通用的正则图标语义）
                    checkbox(self.regex_enabled)
                        .label(".* 正则")
                        .text_size(uipx)
                        .font(uifont)
                        .on_toggle(Message::RegexToggled),
                    // 第 62 轮：查找全部结果面板开关（扫描在途/无命中时禁用）
                    button(text("查找全部").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press_maybe(has_matches.then_some(Message::FindAllToggled)),
                    button(text("×").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press(Message::FindToggled),
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
                    // P70：正则模式替换当前 = 对命中做 $1 展开替换
                    button(text("替换当前").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press_maybe(has_matches.then_some(if self.regex_enabled {
                            Message::ReplaceCurrentRegex
                        } else {
                            Message::ReplaceCurrent
                        })),
                    // 扫描在途时禁用：此刻的全文快照可能是过期的
                    button(text("全部替换").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press_maybe(
                            (!scanning).then_some(Message::ReplaceAll),
                        ),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );

            // 第 62 轮：查找全部结果面板（停靠在查找区内，栏关即隐；
            // 数据源 = 后台扫描的全量命中表，重扫刷新时自动跟随）
            if self.find_all_visible {
                body = body
                    .push(rule::horizontal(1))
                    .push(self.find_all_panel(uipx, uifont));
            }
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
                    button(text("跳转").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press(Message::GotoSubmit),
                    button(text("×").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press(Message::GotoToggled),
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
                        .style(chrome_button_style)
                        .on_press_maybe(
                            (!self.busy).then_some(Message::ConfirmSaveAndClose)
                        ),
                    button(text("放弃更改").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::DiscardAndClose),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
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
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmCloseTabDiscard(idx)),
                    // P21 完整版：已命名的页可直接「保存并关闭」
                    button(text("保存并关闭").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press_maybe(
                            (!self.busy && self.tabs[idx].path.is_some())
                                .then_some(Message::CloseTabSave(idx)),
                        ),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
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
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmBatchCloseDiscard),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
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
                    button(text("放弃更改并打开").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmOpenDiscard),
                    button(text("取消").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmOpenCancel),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P50/P52 外部修改提示条：队列首页展示，聚合时带总数与「全部忽略」。
        // 下标失效（页已关）时跳过，等下次聚焦重算。
        if let Some(queue) = self.external_change.as_ref() {
            if let Some(first) = queue.first().copied() {
                if let Some(tab) = self.tabs.get(first) {
                    if tab.path.is_some() {
                        let total = queue.len();
                        let mut bar = row![
                            text(if total > 1 {
                                format!(
                                    "「{}」已被外部修改（共 {total} 个文件），是否重新加载？",
                                    tab.display_name()
                                )
                            } else {
                                format!(
                                    "「{}」已被外部修改，是否重新加载？",
                                    tab.display_name()
                                )
                            })
                            .size(uipx)
                            .font(uifont),
                            button(text("重新加载").size(uipx).font(uifont))
                                .padding([4, 12])
                                .style(chrome_button_style)
                                .on_press(Message::ConfirmExternalReload(first)),
                            button(text("忽略").size(uipx).font(uifont))
                                .padding([4, 12])
                                .style(chrome_button_style)
                                .on_press(Message::IgnoreExternalChange(first)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .padding([6, 10]);
                        if total > 1 {
                            bar = bar.push(
                                button(text("全部忽略").size(uipx).font(uifont))
                                    .padding([4, 12])
                                    .style(chrome_button_style)
                                    .on_press(Message::IgnoreAllExternalChanges),
                            );
                        }
                        body = body.push(rule::horizontal(1)).push(bar);
                    }
                }
            }
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
                        .style(chrome_button_style)
                        .on_press(Message::SessionRecoverAccepted),
                    button(text("丢弃").size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::SessionRecoverDiscarded),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if !self.status.is_empty() {
            // 错误（红色 ⚠）与普通信息（主题文字色，无警号）区分渲染：
            // 信息持久化后满屏红字会放大无关紧要的提示
            let (icon, color) = if self.status_is_error {
                ("⚠ ", [0.9, 0.25, 0.25])
            } else {
                let palette = if self.dark_mode {
                    Theme::Dark.palette()
                } else {
                    Theme::Light.palette()
                };
                let Color { r, g, b, .. } = palette.text;
                ("", [r, g, b])
            };
            body = body.push(
                container(
                    text(format!("{icon}{}", self.status))
                        .size(uipx)
                        .font(uifont)
                        .color(color),
                )
                .padding([4, 10]),
            );
        }

        let cursor = self.cur_handle.borrow().cursor;
        // P67：行尾短标签（主导行尾随文档实时读取）
        let eol = eol_label(self.cur_handle.borrow().doc.line_ending());
        // 第 70 轮：状态栏再排——文件路径移到**最左**并定宽截断（超长
        // 加省略号），随后竖线分隔：路径 │ 统计组(弹性吸收) │ 右组
        // （选区定宽/行尾/编码）。两条竖线位置固定，两侧互不影响。
        // ⚠️ 第 71 轮勘误：竖线禁用 rule::vertical——iced 0.14 的
        // vertical rule 在 Row 内长度默认 Fill，会把整条状态栏撑满剩余
        // 高度（用户实测截图暴露）；改为 1px 定宽定高的 container 竖条。
        let palette = if self.dark_mode {
            Theme::Dark.palette()
        } else {
            Theme::Light.palette()
        };
        let sep_color = Color { a: 0.30, ..palette.text };
        // 第 76 轮：状态栏行高与顶部菜单栏一致压缩（竖条 20→14px 随行高
        // 同缩；整行 padding [6,10]→[1,8]）
        let sep_v = move || -> Element<'_, Message> {
            container(text(""))
                .width(1)
                .height(14)
                .style(move |_: &Theme| {
                    container::Style {
                        background: Some(Background::Color(sep_color)),
                        ..container::Style::default()
                    }
                })
                .into()
        };
        let (doc_chars, line_count, sel_chars, cur_off) = {
            let ed = self.cur_handle.borrow();
            (
                ed.doc.text_len(),
                ed.doc.line_count(),
                ed.selection_display_len(),
                ed.cursor_offset(),
            )
        };
        let path_full = self
            .tab()
            .path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("({})", self.tab().base_name()));
        let path_show = {
            const MAX: usize = 40;
            let chars: Vec<char> = path_full.chars().collect();
            if chars.len() > MAX {
                format!("…{}", chars[chars.len() - MAX..].iter().collect::<String>())
            } else {
                path_full.clone()
            }
        };
        let status_bar = row![
            text(path_show).size(uipx).font(uifont).width(280),
            sep_v(),
            text(format!("长度: {doc_chars}")).size(uipx).font(uifont),
            text(format!("行数: {line_count}")).size(uipx).font(uifont),
            text(format!("行: {}", cursor.line + 1)).size(uipx).font(uifont),
            text(format!("列: {}", cursor.col + 1)).size(uipx).font(uifont),
            text(format!("位置: {cur_off}")).size(uipx).font(uifont),
            // 弹性段吸收中部余量：左右两组竖线位置恒定
            text("").width(Fill),
            sep_v(),
            // 选区段：定宽占位（无选区显示占位空白），保证右侧组零推移
            container(
                text(match sel_chars {
                    Some(n) => format!("选 {n} 字符"),
                    None => String::new(),
                })
                .size(uipx)
                .font(uifont)
            )
            .width(110)
            .align_x(iced::alignment::Horizontal::Right),
            button(text(eol).size(uipx).font(uifont))
                .padding([1, 6])
                .style(chrome_button_style)
                .on_press_maybe((!self.busy).then_some(Message::ToggleEolMenu)),
            button(
                text(if self.tab().encoding_label.is_empty() {
                    "—".to_owned()
                } else {
                    self.tab().encoding_label.clone()
                })
                .size(uipx)
                .font(uifont)
            )
            .padding([1, 6])
            .style(chrome_button_style)
            .on_press_maybe((!self.busy).then_some(Message::ToggleEncodingMenu)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([1, 8]);

        body = body.push(rule::horizontal(1)).push(status_bar);

        // P39/P40：浮层经 Stack 叠加——首层（正文）定尺寸，顶层浮在
        // 其上、不占布局空间（iced_widget-0.14.2 Stack 语义），正文可用
        // 面积不再被菜单/设置弹窗挤占。opaque 背板捕获层内点击，
        // 防止点菜单外的落穿透到正文。
        let base: Element<'_, Message> = container(body).width(Fill).height(Fill).into();
        let mut layered = Stack::new().push(base);
        // 第 69 轮：顶部菜单栏浮层（最上层优先渲染）
        if let Some(idx) = self.menu_bar_open {
            layered = layered.push(self.menubar_overlay(idx));
        }
        if let Some(idx) = self.tab_context_menu {
            if idx < self.tabs.len() {
                layered = layered.push(self.context_menu_overlay(idx));
            }
        }
        if self.settings_visible {
            layered = layered.push(self.settings_overlay());
        }
        // P67：状态栏编码/行尾弹出菜单（互斥，update 层保证）
        if self.encoding_menu {
            layered = layered.push(self.encoding_menu_overlay());
        }
        if self.eol_menu {
            layered = layered.push(self.eol_menu_overlay());
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
            // P56：宽度必须定宽 CTX_MENU_W——P46 的 Shrink 修法在 iced
            // 布局下不成立（Fill 子控件在 Shrink 测量中会撑到窗口宽，
            // 用户截图实测卡片 ~970px）；定宽与贴边钳制共用同一常量，
            // 估宽即实宽，不再有漂移
            container(
                scrollable(self.tab_context_panel(idx))
                    .height(card_h)
                    .width(CTX_MENU_W),
            )
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

    // ---------- 第 69 轮：顶部菜单栏浮层 ----------

    /// 菜单栏浮层：整窗透明背板 + 锚在触发按钮槽位下方的卡片。
    ///
    /// 第 70 轮修订（用户反馈「弹窗会移动」）：
    /// * 锚点读**展开瞬间冻结**的 menubar_anchor，且 x 对齐到按钮槽位
    ///   左缘（menubar_slot_idx 反推）——同一菜单无论点按钮哪个部位、
    ///   展开期间鼠标怎么动，浮层位置恒定；
    /// * 背板 on_move 跟踪指针 + on_press 时若落在菜单栏条带内 → 视为
    ///   点击另一菜单（MenuToggled 切换，主流横移手感），否则收起。
    fn menubar_overlay(&self, idx: usize) -> Element<'_, Message> {
        const W: f32 = 240.0;
        let card_h = ctx_menu_card_h(self.viewport_size.1).min(380.0);
        let slot_x = MENU_BAR_LEFT + menubar_slot_idx(self.menubar_anchor.0) as f32 * MENU_SLOT_W;
        let (ax, ay) = clamp_menu_anchor(
            (slot_x, MENU_BAR_H),
            self.viewport_size,
            W,
            card_h,
        );
        let ay = ay.max(MENU_BAR_H); // 永不遮住菜单栏本身
        let card = opaque(
            container(
                scrollable(self.menubar_panel(idx))
                    .width(W)
                    .height(card_h),
            )
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
        .on_move(Message::MenubarHovered)
        .on_press(Message::MenubarPressed)
        .into()
    }

    /// 第 `idx` 个菜单的面板内容。全部复用既有消息（零新编辑逻辑）；
    /// 守卫口径与原工具栏按钮一致（busy / dirty / is_markdown）。
    fn menubar_panel(&self, idx: usize) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let interactive = !self.busy;
        let item = |label: String, msg: Option<Message>| {
            button(container(text(label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .padding([5, 10])
                .style(chrome_menu_item_style)
                .on_press_maybe(msg)
        };
        let sep = || rule::horizontal(1);
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
        let mut panel = column![].spacing(2).padding([4, 6]);
        match idx {
            // ---------- 文件 ----------
            0 => {
                panel = panel
                    .push(item(
                        "打开…  Ctrl+O".to_owned(),
                        interactive.then_some(Message::OpenRequested),
                    ))
                    .push(item(
                        "保存  Ctrl+S".to_owned(),
                        (interactive && self.tab().dirty).then_some(Message::SaveRequested),
                    ))
                    .push(item(
                        "另存为…".to_owned(),
                        interactive.then_some(Message::SaveAsRequested),
                    ))
                    .push(sep())
                    .push(item(
                        format!(
                            "恢复上次关闭的标签页  Ctrl+Shift+W{}",
                            if self.closed_stack.is_empty() { "（空）" } else { "" }
                        ),
                        (!self.busy && !self.closed_stack.is_empty())
                            .then_some(Message::ReopenLastClosedFile),
                    ))
                    .push(item(
                        "最近文件".to_owned(),
                        interactive.then_some(Message::RecentsToggled),
                    ));
            }
            // ---------- 编辑 ----------
            1 => {
                panel = panel
                    .push(item(
                        "撤销  Ctrl+Z".to_owned(),
                        interactive.then_some(Message::Edit(EditOp::Undo)),
                    ))
                    .push(item(
                        "重做  Ctrl+Y".to_owned(),
                        interactive.then_some(Message::Edit(EditOp::Redo)),
                    ))
                    .push(sep())
                    .push(item(
                        "剪切  Ctrl+X".to_owned(),
                        interactive.then_some(Message::CutRequested),
                    ))
                    .push(item(
                        "复制  Ctrl+C".to_owned(),
                        interactive.then_some(Message::CopyRequested),
                    ))
                    .push(item(
                        "粘贴  Ctrl+V".to_owned(),
                        interactive.then_some(Message::PasteRequested),
                    ))
                    .push(item(
                        "全选  Ctrl+A".to_owned(),
                        interactive.then_some(Message::Edit(EditOp::SelectAll)),
                    ))
                    .push(sep())
                    .push(item(
                        "查找/替换栏  Ctrl+F".to_owned(),
                        interactive.then_some(Message::FindToggled),
                    ))
                    .push(item(
                        "跳转到行  Ctrl+G".to_owned(),
                        interactive.then_some(Message::GotoToggled),
                    ))
                    .push(sep())
                    .push(item(
                        "插入日期时间  F5".to_owned(),
                        interactive.then_some(Message::Edit(EditOp::InsertDateTime)),
                    ))
                    .push(item(
                        "切换行注释  Ctrl+Q".to_owned(),
                        interactive.then_some(Message::Edit(EditOp::ToggleLineComment)),
                    ));
            }
            // ---------- 查看 ----------
            2 => {
                let step = editor::FONT_ZOOM_STEP;
                panel = panel
                    .push(item(
                        "放大  Ctrl+滚轮".to_owned(),
                        interactive.then_some(Message::FontSizeDelta(step)),
                    ))
                    .push(item(
                        "缩小  Ctrl+滚轮".to_owned(),
                        interactive.then_some(Message::FontSizeDelta(-step)),
                    ))
                    .push(item(
                        "重置缩放".to_owned(),
                        interactive.then_some(Message::FontSizeDelta(
                            16.0 - self.display_font_size(),
                        )),
                    ))
                    .push(item(
                        "切换深浅主题".to_owned(),
                        interactive.then_some(Message::ThemeToggled),
                    ))
                    .push(sep())
                    .push(item(
                        format!(
                            "{}显示空白字符",
                            if self.settings.show_whitespace { "✓ " } else { "" }
                        ),
                        interactive.then_some(Message::SettingsShowWhitespaceToggled(
                            !self.settings.show_whitespace,
                        )),
                    ))
                    .push(item(
                        format!(
                            "{}显示行尾符",
                            if self.settings.show_line_endings { "✓ " } else { "" }
                        ),
                        interactive.then_some(Message::SettingsShowLineEndingsToggled(
                            !self.settings.show_line_endings,
                        )),
                    ))
                    // 第 73 轮 ⑯：自动换行开关（查看菜单入口，与设置页同消息）
                    .push(item(
                        format!(
                            "{}自动换行",
                            if self.settings.word_wrap { "✓ " } else { "" }
                        ),
                        interactive.then_some(Message::SettingsWordWrapToggled(
                            !self.settings.word_wrap,
                        )),
                    ))
                    // 原「视图」菜单并入：MD 预览按当前语法门控（最近文件
                    // 面板与文件菜单的「最近文件」重复，不再单列）
                    .push(sep())
                    .push(item(
                        if self.preview_visible {
                            "关闭 MD 预览".to_owned()
                        } else {
                            "MD 预览".to_owned()
                        },
                        (interactive && is_markdown).then_some(Message::PreviewToggled),
                    ));
            }
            // ---------- 设置 ----------
            _ => {
                panel = panel
                    .push(item(
                        "打开设置…".to_owned(),
                        interactive.then_some(Message::SettingsToggled),
                    ))
                    .push(item(
                        format!(
                            "保存时备份：{}",
                            match self.settings.backup_mode.as_str() {
                                editpad_core::settings::BACKUP_MODE_SIMPLE => "覆盖式",
                                editpad_core::settings::BACKUP_MODE_TIMESTAMPED => {
                                    "时间戳历史"
                                }
                                _ => "关闭",
                            }
                        ),
                        interactive.then_some(Message::SettingsBackupModeToggled),
                    ));
            }
        }
        panel.into()
    }

    /// 第 62 轮：「查找全部」结果面板（停靠式，非浮层）——数据源 =
    /// 查找后台扫描的全量命中表，重扫刷新自动跟随。渲染行数封顶
    /// [`FIND_ALL_MAX_ROWS`]：chrome 行按钮无虚拟化，10 万级命中全量
    /// 渲染会拖垮帧率，超出部分明示截断提示。
    fn find_all_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let total = self.matches.len();
        let scanning = self.find_scanning();
        let shown = total.min(FIND_ALL_MAX_ROWS);
        // 标题行：文本 width(Fill) 把关闭按钮推到右缘
        let header = row![
            text(if scanning {
                "查找中…".to_owned()
            } else if total == 0 {
                "无匹配".to_owned()
            } else {
                format!("全部匹配：{total} 处")
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::FindAllToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if shown > 0 {
            let editor = self.cur_handle.borrow();
            for i in 0..shown {
                let m = self.matches[i];
                let raw = editor.line_text(m.line);
                let excerpt = match_excerpt(&raw, m.col, FIND_ALL_EXCERPT_COLS);
                rows = rows.push(
                    button(
                        text(format!("{}:{}  {}", m.line + 1, m.col + 1, excerpt))
                            .size(uipx)
                            .font(uifont)
                            .width(Fill),
                    )
                    .width(Fill)
                    .padding([3, 10])
                    .style(chrome_menu_item_style)
                    .on_press(Message::FindAllGoto(i)),
                );
            }
        }
        if total > shown {
            rows = rows.push(
                text(format!(
                    "已显示前 {shown} 条（共 {total} 处）——请细化关键词"
                ))
                .size(uipx)
                .font(uifont),
            );
        }
        // 高度按窗口钳制：小窗不溢出、大窗不占满（浮层卡片同策略）
        let h = (self.viewport_size.1 * 0.35).clamp(120.0, 300.0);
        container(
            column![header, rule::horizontal(1), scrollable(rows)]
                .spacing(0)
                .width(Fill),
        )
        .width(Fill)
        .height(h)
        .into()
    }

    /// P67：状态栏弹出菜单的通用浮层骨架——背板点击关闭 + 右下角贴
    /// 状态栏上缘的定宽卡片（锚点按窗口尺寸现算，无需指针跟踪）。
    fn status_menu_overlay<'a>(
        &self,
        card_w: f32,
        card_h: f32,
        panel: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let ax = (self.viewport_size.0 - card_w - 12.0).max(8.0);
        // 底缘贴状态栏上方（状态栏高 ≈ 32px），小窗口钳到 8px
        let ay = (self.viewport_size.1 - card_h - 36.0).max(8.0);
        let card = opaque(
            container(panel)
                .width(card_w)
                .height(card_h)
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
        .on_press(Message::BarsDismissed)
        .into()
    }

    /// P67：编码弹出菜单——三种目标编码，未命名页禁用（无路径可写）。
    fn encoding_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 200.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let named = self.tab().path.is_some();
        let item = |label: &str, enc: editpad_core::SaveEncoding| {
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe(named.then_some(Message::SaveWithEncoding(enc)))
        };
        let panel = column![
            item("以 UTF-8 保存", editpad_core::SaveEncoding::Utf8),
            item("以 UTF-8(BOM) 保存", editpad_core::SaveEncoding::Utf8Bom),
            item("以 GBK 保存", editpad_core::SaveEncoding::Gbk),
        ]
        .spacing(2);
        self.status_menu_overlay(W, ITEM_H * 3.0 + 12.0, panel.into())
    }

    /// P67：行尾弹出菜单——当前主导行尾标头 + 两个转换项（已是目标
    /// 则禁用）。转换为可撤销的文档编辑。
    fn eol_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 220.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.body_font();
        let current = self.cur_handle.borrow().doc.line_ending();
        let item = |label: &str, target: editpad_core::LineEnding| {
            let disabled = current == target;
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe((!disabled).then_some(Message::ConvertEol(target)))
        };
        let panel = column![
            text(format!("当前行尾：{}", eol_label(current)))
                .size(uipx)
                .font(uifont),
            item(
                "转换为 CRLF（Windows）",
                editpad_core::LineEnding::CrLf
            ),
            item("转换为 LF（Unix）", editpad_core::LineEnding::Lf),
        ]
        .spacing(4);
        self.status_menu_overlay(W, ITEM_H * 2.0 + 28.0, panel.into())
    }

    // settings_overlay 已迁往 settings_ui.rs（第 82 轮 Phase 2a）

}

// ---------- 「查找全部」结果面板助手（第 62 轮） ----------

/// 结果面板渲染行数封顶：chrome 行按钮无虚拟化，超出部分在面板尾部
/// 明示截断（提示细化关键词），避免超大命中集拖垮每帧构建。
pub(crate) const FIND_ALL_MAX_ROWS: usize = 500;
/// 单条结果的摘录字符数上限（以命中列为窗心向两侧取半）。
const FIND_ALL_EXCERPT_COLS: usize = 96;

/// 结果面板行摘录：剥行尾 → 以命中列为窗心取最多 `max_cols` 个字符，
/// 两端截断处补省略号 `…`。纯函数便于单测。
pub(crate) fn match_excerpt(line_text: &str, col: usize, max_cols: usize) -> String {
    let body = line_text.trim_end_matches(['\n', '\r']);
    let chars: Vec<char> = body.chars().collect();
    let col = col.min(chars.len());
    if chars.len() <= max_cols {
        return chars.into_iter().collect();
    }
    // 内窗预算：两端截断各占 1 位省略号（单端截断时另一侧多还 1 位）
    let take = max_cols - 2;
    let mut start = col.saturating_sub(take.div_ceil(2));
    let end = (start + take).min(chars.len());
    if end == chars.len() {
        // 尾部没截：把省下的右侧预算回填给头部
        start = start.saturating_sub(take - (end - start));
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}
