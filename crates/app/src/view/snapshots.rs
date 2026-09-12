//! 心跳快照：会话页元数据采集、快照计划与提交/回执。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

impl Editpad {
    /// Tab → 清单页元数据的统一映射。`file` 由调用方决定：
    /// 退出流恒 None（全量重写）；心跳流对「版本未变的置脏页」预填
    /// 旧文件名（core 侧据此复用不重写）。
    pub(super) fn session_tab_metadata(
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
            // P134：每页显示覆盖随会话保存（C7）
            wrap_override: t.wrap_override,
            font_size_override: t.font_size_override,
        }
    }

    /// 心跳选页：返回需要**重写内容**的 (下标, 派发时刻版本) 列表。
    /// 判定核心在 core 的 [`editpad_core::snapshot::heartbeat_page_selected`]：
    /// 置脏 + 未超大小上限（按 rope 真实字节数）+ 版本自上次快照有推进。
    pub(super) fn heartbeat_plan(&self) -> Vec<(usize, u64)> {
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
    pub(super) fn build_heartbeat_pages(
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
            rev: self.manifest_rev,
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
                // P146：只在「派发后清单没有再过期」时清过期标记——曾无条件
                // 清零：在途心跳期间关掉的页不在本次写出的清单里，标记被清
                // 后永不重写，崩溃恢复会把已关页连同旧内容复活。
                if outcome.rev == self.manifest_rev {
                    self.session_manifest_stale = false;
                }
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
                self.set_status_error(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StSnapshotHeartbeatFailed, &error.to_string()));
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
            rev: payload.rev,
            result,
        };
        self.heartbeat_apply(outcome.clone());
        Some(outcome)
    }

}
