//! editpad-core：Editpad 的纯逻辑层。
//!
//! 零 GUI 依赖——文档模型、编码加载、搜索、设置持久化都在这里，
//! 方便单独测试，将来做终端版时也能直接复用。

pub mod document;
pub mod error;
pub mod highlight;
pub mod json;
pub mod loader;
pub mod markdown;
pub(crate) mod syntaxes;
pub mod saver;
pub mod search;
pub mod settings;
pub mod snapshot;

pub use document::{Document, LineEnding};
pub use error::CoreError;
pub use highlight::{resolve_language, syntax_for_extension, LazyHighlighter, StyledRun};
pub use json::{format_json, validate_json, JsonError};
pub use loader::{load_file, load_file_streaming, load_document_streaming, decode, LoadProgress,
    LoadedText, LoadedDocument};
pub use saver::{save_atomic, save_document_atomic};
pub use search::{
    find_all, find_all_document, next_from, prev_from, replace_all, replace_all_document,
    ascii_case_eq, MatchPos,
};
pub use settings::{Settings, EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT};
pub use snapshot::{
    clear_session, enforce_quota, heartbeat_page_selected, read_manifest, read_page,
    snapshot_dir, write_heartbeat_session, write_session, HeartbeatPage, SessionManifest,
    SessionPage, SessionTab, HEARTBEAT_MAX_PAGE_BYTES, SNAPSHOT_QUOTA_BYTES,
};
