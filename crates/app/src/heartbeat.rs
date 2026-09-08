use super::*;

/// 一次心跳提交的输入：注入目录、全部页的逐页意图、派发时刻捕获的逐页计划。
pub(crate) struct HeartbeatPayload {
    pub(crate) dir: PathBuf,
    pub(crate) pages: Vec<editpad_core::snapshot::HeartbeatPage>,
    pub(crate) active: usize,
    pub(crate) next_untitled: u64,
    /// 需要重写内容的 (页下标, 派发时刻内容版本)——回报据此回填账目
    pub(crate) plan: Vec<(usize, u64)>,
    /// P146：派发时刻的清单过期代次——回报时比对，防「在途期间关页
    /// 后成功回报把 stale 误清」（清单里还留着已关页，却永不重写）。
    pub(crate) rev: u64,
}

/// 一次心跳提交的回报：派发时刻的计划 + 写盘结果（成功时含新清单，
/// 用于取各页实际文件名）。Clone 仅为测试同步驱动入口服务。
#[derive(Debug, Clone)]
pub(crate) struct HeartbeatOutcome {
    pub(crate) plan: Vec<(usize, u64)>,
    pub(crate) rev: u64,
    pub(crate) result: Result<editpad_core::snapshot::SessionManifest, String>,
}
