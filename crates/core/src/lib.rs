//! editpad-core：Editpad 的纯逻辑层。
//!
//! 零 GUI 依赖——文档模型、编码加载、搜索、设置持久化都在这里，
//! 方便单独测试，将来做终端版时也能直接复用。

pub mod document;
pub mod error;
pub mod highlight;
pub mod loader;
pub mod saver;
pub mod search;
pub mod settings;

pub use document::{Document, LineEnding};
pub use error::CoreError;
pub use highlight::{LazyHighlighter, StyledRun};
pub use loader::{load_file, load_file_streaming, decode, LoadProgress, LoadedText};
pub use saver::save_atomic;
pub use search::{find_all, next_from, prev_from, replace_all, ascii_case_eq, MatchPos};
pub use settings::Settings;
