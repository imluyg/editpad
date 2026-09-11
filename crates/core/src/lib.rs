//! editpad-core：Editpad 的纯逻辑层。
//!
//! 零 GUI 依赖——文档模型、编码加载、搜索、设置持久化都在这里，
//! 方便单独测试，将来做终端版时也能直接复用。

pub mod toolkit;
pub mod brackets;
pub mod document;
pub mod error;
pub mod find_in_files;
pub mod highlight;
pub mod json;
pub mod lang;
pub mod loader;
pub mod markdown;
pub mod paths;
pub(crate) mod syntaxes;
pub mod saver;
pub mod search;
pub mod settings;
pub mod snapshot;

pub use brackets::{bracket_kind, scan_forward, BRACKET_PAIRS, MAX_BRACKET_SCAN_CHARS};
pub use document::{Document, LineEnding};
pub use error::CoreError;
pub use find_in_files::{filter_whole_word_text, find_in_file, find_in_file_with, walk_files,
    FifMatcher, WalkOutput, IGNORED_DIRS, MAX_SCAN_FILE_BYTES, MAX_WALK_DEPTH};
pub use highlight::{resolve_language, syntax_for_extension, LazyHighlighter, StyledRun};
pub use json::{format_json, validate_json, JsonError};
pub use lang::{
    fmt_core_error, fmt_fif_panel_title, fmt_fif_summary, fmt_file_hits, fmt_find_counter,
    fmt_font_picker_more, fmt_match_counter, fmt_match_total, fmt_page_count, fmt_replaced,
    fmt_restore_note, fmt_selection, fmt_suffix, fmt_wrapped, untitled_prefix, Key, Lang,
    LangOption, LANG_NAME_EN, LANG_NAME_ZH_CN,
};
pub use loader::{load_file, load_file_streaming, load_document_streaming, decode, LoadProgress,
    LoadedText, LoadedDocument};
pub use paths::{data_root, data_root_for, data_root_for_base, instance_mutex_name,
    pending_open_path};
pub use saver::{save_atomic, save_document_atomic, save_document_encoded, EncodeNotice,
    SaveEncoding};
pub use search::{
    expand_regex_at, filter_whole_word, find_all, find_all_document, for_each_line, is_word_char,
    next_from, prev_from, replace_all, replace_all_document, replace_all_word, ascii_case_eq,
    compile_regex, find_all_regex, find_all_regex_compiled, replace_all_regex, MatchPos,
};
pub use settings::{Settings, RecentView, normalize_combo, normalize_settings_page, EXIT_MODE_ASK,
    EXIT_MODE_SNAPSHOT, SETTINGS_PAGES, SETTINGS_PAGE_APPEARANCE, SETTINGS_PAGE_FONT,
    SETTINGS_PAGE_SAVE, SETTINGS_PAGE_SESSION, SETTINGS_PAGE_HOTKEYS, SETTINGS_PAGE_ABOUT};
pub use snapshot::{
    clear_session, enforce_quota, heartbeat_page_selected, read_manifest, read_page,
    snapshot_dir, write_heartbeat_session, write_session, HeartbeatPage, SessionManifest,
    SessionPage, SessionTab, HEARTBEAT_MAX_PAGE_BYTES, SNAPSHOT_QUOTA_BYTES,
};
