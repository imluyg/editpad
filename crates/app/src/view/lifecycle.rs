//! 应用生命周期：窗口关闭与退出、退出前落盘、窗口标题。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

impl Editpad {
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
    pub(super) fn confirm_or_close(&mut self) -> Task<Message> {
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
                self.set_status_error(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StSessionSnapshotFailed, &error.to_string()));
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
                Some(n) => format!("{}{n}.txt", editpad_core::untitled_prefix(self.lang())),
                None => format!("{}.txt", editpad_core::untitled_prefix(self.lang())),
            },
        }
    }

    pub(super) fn file_display_name(&self) -> Option<String> {
        self.tab()
            .path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_owned)
    }

}
