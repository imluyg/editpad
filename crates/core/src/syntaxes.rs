//! 内嵌迷你语法定义（P22 第一批）。
//!
//! syntect 默认包没有 Log 与 TOML 语法，而它们恰是记事本的高频格式；
//! 这里以字符串内嵌两份迷你 `.sublime-syntax`，在语法集初始化时经
//! [`SyntaxSetBuilder::add`] 注入——单文件分发、无需运行时资源目录。
//!
//! 着色走标准 scope 名（comment/string/keyword/invalid/
//! constant.numeric），任何默认主题都会给它们可区分的颜色，
//! 不必随应用分发自定义主题。

/// 日志语法：时间戳 + 级别词（ERROR/WARN/INFO/DEBUG）分色。
///
/// scope 选型（借道默认主题的既有配色）：
/// * `constant.numeric` → 时间戳（蓝）
/// * `invalid`          → FATAL/ERROR（红）
/// * `keyword.control`  → WARN/WARNING（醒目色）
/// * `string`           → INFO/NOTICE（弱强调）
/// * `comment`          → DEBUG/TRACE（灰）
pub const LOG: &str = r#"%YAML 1.2
---
name: Editpad Log
file_extensions: [editpad-log]
scope: source.editpad.log
contexts:
  main:
    - match: '\b(FATAL|ERROR|ERR|PANIC)\b'
      scope: invalid
    - match: '\b(WARN|WARNING)\b'
      scope: keyword.control
    - match: '\b(INFO|NOTICE)\b'
      scope: string
    - match: '\b(DEBUG|TRACE)\b'
      scope: comment
    - match: '[0-9]{4}-[0-9]{2}-[0-9]{2}([T ][0-9]{2}:[0-9]{2}:[0-9]{2}([.,][0-9]+)?(Z|[+-][0-9]{2}:?[0-9]{2})?)?'
      scope: constant.numeric
"#;

/// TOML 语法：节名 / 键 / 字符串 / 布尔 / 数字 / 注释。
pub const TOML: &str = r#"%YAML 1.2
---
name: Editpad TOML
file_extensions: [editpad-toml]
scope: source.editpad.toml
contexts:
  main:
    - match: '"[^"]*"'
      scope: string
    - match: "'[^']*'"
      scope: string
    - match: '#.*$'
      scope: comment
    - match: '^\s*\[\[?[^\]]*\]\]?'
      scope: entity.name
    - match: '\b(true|false)\b'
      scope: constant.language
    - match: '\b[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?\b'
      scope: constant.numeric
"#;
