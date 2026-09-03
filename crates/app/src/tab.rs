use super::*;

/// 批量关闭的范围（纯函数 [`batch_close_targets`] 的入参）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchCloseScope {
    /// 除 `keep` 页以外的全部候选
    Others(usize),
    /// `from` 页右侧的全部候选
    RightOf(usize),
}

/// 批量关闭的目标集合（纯函数便于测试，§3 P28 第 3 条）：
/// 在指定范围内收集全部**非固定**页下标（升序）。固定页豁免批量关闭；
/// keep/from 越界时返回空表（无目标 = 菜单项禁用、update 层 no-op）。
pub(crate) fn batch_close_targets(tabs: &[Tab], scope: BatchCloseScope) -> Vec<usize> {
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

/// 单个标签页的完整状态（P21）。
///
/// [`EditorHandle`] 内聚文档/光标/选区/撤销/高亮/滚动；标签页另持
/// 路径、置脏标记与编码标签。P18 版本守卫所需的版本号/防抖起点/
/// 自动保存在途标记也**按页独立**——后台页同样参与自动保存。
#[derive(Debug, Clone)]
pub(crate) struct Tab {
    pub(crate) editor: EditorHandle,
    pub(crate) path: Option<PathBuf>,
    pub(crate) dirty: bool,
    pub(crate) encoding_label: String,
    /// 内容版本号：本页每次真实改动 +1（保存回报据此判断是否清脏）
    pub(crate) version: u64,
    /// 本页自动保存任务在途标记
    pub(crate) autosave_inflight: bool,
    /// 本页最后一次内容改动的时刻（防抖窗口计时起点）
    pub(crate) last_edit_at: Option<std::time::Instant>,
    /// 未命名页的递增序号（P25）：显示为「未命名N」，
    /// 全局单调不复用——杜绝两个同名未命名页的保存歧义；
    /// 另存为成功或加载真实文件后清除。
    pub(crate) untitled_num: Option<u64>,
    /// P31 心跳账目：本页最后一次被心跳快照收录时的 (内容版本, 页文件名)。
    /// None = 从未参与。版本与文件名成对维护，保证「版本没变 → 旧文件
    /// 仍有效 → 沿用不重写」的复用判定不会错位；账目随页走（增删页/
    /// 调序后仍与正确的内容文件配对）。
    pub(crate) heartbeat_snap: Option<(u64, String)>,
    /// 固定标记（P28）：固定页豁免单页与批量关闭（菜单项灰掉），
    /// 标签条以 📌 标识。v1 取舍：不自动前置排序（保持用户手动排列的
    /// 稳定顺序）；不入会话快照清单（会话内临时状态）。
    pub(crate) pinned: bool,
    /// P50 外部修改检测戳：载入/保存成功时刻的 (mtime, size)。
    /// None = 从未记录（未命名页/会话恢复占位页未落地的），不参与判定。
    pub(crate) file_stamp: Option<(std::time::SystemTime, u64)>,
    /// P67：本页的保存编码偏好。None = 默认 UTF-8（历史行为）；
    /// 用户在状态栏「编码」菜单选择后记住，此后每次保存沿用，
    /// 重新加载/另存为新路径时重置。不入会话快照（v1 取舍）。
    pub(crate) save_encoding: Option<editpad_core::SaveEncoding>,
}

impl Tab {
    pub(crate) fn empty() -> Self {
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
            file_stamp: None,
            save_encoding: None,
        }
    }

    /// 不含置脏标记的基础显示名：真实文件名优先，
    /// 未命名页显示「未命名N」（N 为全局单调序号）。
    pub(crate) fn base_name(&self) -> String {
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
    pub(crate) fn display_name(&self) -> String {
        let base = self.base_name();
        if self.dirty {
            format!("● {base}")
        } else {
            base
        }
    }

    /// 记一次真实改动（版本推进 + 防抖起点刷新）。
    pub(crate) fn note_mutation(&mut self) {
        self.version += 1;
        self.last_edit_at = Some(std::time::Instant::now());
    }
}
