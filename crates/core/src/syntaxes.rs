//! 内嵌迷你语法定义（P22 第一批；Dockerfile 是 2026-10-16 档1 补的）。
//!
//! syntect 默认包没有 Log、TOML 与 Dockerfile 语法，而它们恰是记事本的高频格式
//! （**默认包实测 75 个语法里确实没有 Dockerfile**——别名表原先把 `.dockerfile`
//! 指到语法名 "Dockerfile"，`find_syntax_by_name` 精确相等找不到 ⇒ 一直是纯文本）；
//! 这里以字符串内嵌迷你语法定义（syntect 可解析的 YAML 格式），在语法集初始化时经
//! [`SyntaxSetBuilder::add`] 注入——单文件分发、无需运行时资源目录。
//!
//! 着色走标准 scope 名（comment/string/keyword/invalid/
//! constant.numeric），任何默认主题都会给它们可区分的颜色，
//! 不必随应用分发自定义主题。
//!
//! `file_extensions` 一律取 `editpad-*` 而不是真扩展名：真正的扩展名→语法映射握在
//! [`crate::highlight`] 的别名表里，让那张表成为唯一入口，才不会绕过它的取舍。

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
      # 节名用 entity.name.function（紫）而不是 entity.name / entity.name.type：
      # 2026-10-16 现量过 InspiredGitHub 与 base16-ocean.dark 两份内置主题对后两者
      # **一条规则都没有**，于是 README 里那句「节名上色」从来没成立过（整行拿默认色）。
      scope: entity.name.function
    - match: '\b(true|false)\b'
      scope: constant.language
    - match: '\b[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?\b'
      scope: constant.numeric
"#;

/// Dockerfile 语法：指令 / 字符串 / 变量与标志 / 注释 / 端口号。
///
/// 指令**只认大写**。小写不是漏掉而是刻意排除：迷你语法没有状态机，分不清
/// `\` 续行里的 shell 命令，`(?i)` 会把续行开头的 `run`/`add`/`copy`/`env`
/// 一起染成指令——宁可不染，也不要染错。
///
/// scope 只借 LOG/TOML 已经用惯的四类（comment / string / keyword.control /
/// constant.*），任何默认主题都给得出可区分的颜色；`constant.numeric` 负责
/// `EXPOSE 8080` 的端口与 `ENV` 的数值。
pub const DOCKERFILE: &str = r#"%YAML 1.2
---
name: Editpad Dockerfile
file_extensions: [editpad-dockerfile]
scope: source.editpad.dockerfile
contexts:
  main:
    - match: '^\s*(FROM|RUN|CMD|LABEL|MAINTAINER|EXPOSE|ENV|ADD|COPY|ENTRYPOINT|VOLUME|USER|WORKDIR|ARG|ONBUILD|STOPSIGNAL|HEALTHCHECK|SHELL)\b'
      scope: keyword.control
    - match: '"[^"]*"'
      scope: string
    - match: "'[^']*'"
      scope: string
    - match: '\$\{[^}]*\}|\$[A-Za-z_][A-Za-z0-9_]*'
      scope: constant.language
    - match: '(^|\s)--?[A-Za-z][A-Za-z0-9-]*'
      scope: constant.language
    - match: '#.*$'
      scope: comment
    - match: '\b[0-9]+(\.[0-9]+)?\b'
      scope: constant.numeric
"#;
