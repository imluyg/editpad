//! 应用设置持久化：最近打开的文件列表。
//!
//! 存储位置由 [`crate::paths`] 决定：P102 实例隔离——数据根目录 =
//! 系统配置目录下按 **exe 路径哈希** 分出的实例目录（Windows 上
//! `%APPDATA%\editpad\instances\<实例键>\`），每份拷贝各搞各的数据；
//! 首次运行把旧版遗留的 `%APPDATA%\editpad\` 整体搬入首个实例。
//! 读写都是尽力而为：配置损坏或目录不可写时静默回退默认值，绝不
//! 影响编辑器本体。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最近文件列表上限。
pub const MAX_RECENT_FILES: usize = 10;

/// 单个最近文件的光标/滚动记忆（P32）。
///
/// 与 [`Settings::recent_files`] **平行**存储（键 = 路径字符串）而非内嵌
/// 进条目：旧 config.toml 的 `recent_files = [".."]` 纯字符串数组因此
/// 零迁移兼容；缺字段落 0（serde default）。滚动值非有限时归一为 0。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecentView {
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub col: usize,
    #[serde(default)]
    pub scroll_top: f32,
}

/// 主题的唯二合法取值；其他值在加载时归一为 [`THEME_LIGHT`]。
pub const THEME_LIGHT: &str = "light";
pub const THEME_DARK: &str = "dark";

/// 字号允许范围（闭区间）与默认值；越界值在加载时被 clamp。
pub const MIN_FONT_SIZE: f32 = 10.0;
pub const MAX_FONT_SIZE: f32 = 28.0;
const DEFAULT_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub recent_files: Vec<String>,
    /// 最近文件的光标/滚动记忆（P32）：键 = 路径字符串，与
    /// [`Self::recent_files`] 对齐。修剪规则：键不在最近列表中即删除
    /// （加载归一与 push_recent 后都会对齐），「记住最近文件」关闭时整体清空
    /// ——路径本身是隐私数据（P20 同款口径），不能换个字段继续留痕。
    #[serde(default)]
    pub recent_views: HashMap<String, RecentView>,
    /// 界面主题：只允许 `"light"` 或 `"dark"`，非法输入在加载时归一。
    // 注意：不能用裸 #[serde(default)]——那会落到 String::default()（空串），
    // 必须指向规范默认值，保证旧 config.toml 缺字段时直接得到合法偏好。
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 编辑器字号，加载时 clamp 到 `[MIN_FONT_SIZE, MAX_FONT_SIZE]`。
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// 是否记住最近打开的文件（P20 隐私选项）。默认 true 保持既有行为；
    /// 关闭后 `push_recent` 变 no-op，且加载时清空存量列表——
    /// config.toml 不再留任何文件路径痕迹。
    #[serde(default = "default_true")]
    pub remember_recent_files: bool,
    /// 编辑后自动写盘开关（P18 引入；P63 策略反转，对标主流编辑器）。
    /// **默认关闭**：修改只留在内存（标签页 ● 标记），直到显式 Ctrl+S——
    /// 原文件只在用户显式动作时被写；崩溃防护由会话快照（P31 心跳）
    /// 兜底，不动原文件。显式开启的用户选择照常持久化与尊重；
    /// 历史上默认开启时期持久化下来的 `true` 由 [`Self::settings_version`]
    /// 迁移一次性重置（见 [`SETTINGS_VERSION`]）。
    #[serde(default = "default_true")]
    pub autosave_enabled: bool,
    /// 自动保存防抖秒数：加载时收敛到 `[1, 60]`。
    #[serde(default = "default_autosave_delay_secs")]
    pub autosave_delay_secs: u32,
    /// 会话快照总开关（P29，隐私对齐 P20 先例）：明文快照落盘必须可一键关闭。
    /// 关闭后退出回旧确认条行为；加载/启动时清空存量快照区。
    #[serde(default = "default_true")]
    pub enable_snapshots: bool,
    /// 关窗行为（P29）：[`EXIT_MODE_SNAPSHOT`] = 置脏页写快照后直接退出、
    /// 零询问（默认）；[`EXIT_MODE_ASK`] = 每次弹「未保存确认条」旧行为。
    /// 非法值在加载时归一为快照直退。
    #[serde(default = "default_exit_mode")]
    pub exit_mode: String,
    /// 启动会话恢复开关（P30）。默认开启：「再次打开还是上次的界面」；
    /// 关闭 = 启动恒空白页、退出不写会话清单（存量快照区在启动时清空）。
    #[serde(default = "default_true")]
    pub remember_session: bool,
    /// 周期快照心跳间隔秒数（P31）：运行中每隔 N 秒巡检一次置脏页，
    /// 把内容有变化的页增量写进快照区（崩溃至多丢一个间隔的输入）。
    /// 加载时收敛到 `[MIN_SNAPSHOT_INTERVAL_SECS, MAX_SNAPSHOT_INTERVAL_SECS]`。
    #[serde(default = "default_snapshot_interval_secs")]
    pub snapshot_interval_secs: u32,
    /// 正文字体族名（P34）。None = 默认等宽（现状行为，旧配置缺字段兼容）；
    /// Some(名字) = 用系统里该族渲染正文与 UI。加载时归一：空串/纯空白/
    /// 超长/含控制字符的值收敛为 None；「名字未安装」的回退在 app 层做
    /// （core 不掌握系统字体清单），且只影响本次生效，不抹掉用户配置。
    #[serde(default)]
    pub font_family: Option<String>,
    /// 第 63 轮：显示空白字符覆盖标记（空格点/制表符箭头）。默认关；
    /// 纯渲染层偏好，不影响文档模型与命中测试。
    #[serde(default)]
    pub show_whitespace: bool,
    /// 第 63 轮：显示行尾符标记（行尾短标）。默认关。
    #[serde(default)]
    pub show_line_endings: bool,
    /// 第 73 轮 ⑯：自动换行（软换行）开关。默认关——关闭时视觉得到
    /// 恒等退化（与软换行引入前的渲染逐像素一致），一键可回退。
    /// 开启时：正文按视口宽折行为若干视觉行；水平滚动条隐藏、
    /// scroll_left 锁 0、列块编辑禁用（设计文档）。
    #[serde(default)]
    pub word_wrap: bool,
    /// 第 64 轮 ⑭：保存时备份模式。[`BACKUP_MODE_NONE`]（默认，不备份）/
    /// [`BACKUP_MODE_SIMPLE`]（同目录 `name.bak` 覆盖式）/ 
    /// [`BACKUP_MODE_TIMESTAMPED`]（`name.bak/` 目录内时间戳历史）。
    /// 非法值在加载时归一为 none。
    #[serde(default = "default_backup_mode")]
    pub backup_mode: String,
    /// P62 热键重映射：动作 id → 组合键串（如 `"Ctrl+Shift+S"`）。
    /// 空 = 全部默认。加载时逐条经 [`normalize_combo`] 归一，非法组合
    /// 条目删除；动作 id 的合法性由 app 层过滤（core 不掌握动作清单）。
    #[serde(default)]
    pub hotkeys: HashMap<String, String>,
    /// 设置策略版本（P63）：旧配置缺字段 = 0，加载归一时逐级执行
    /// 策略迁移后推进到 [`SETTINGS_VERSION`]。只承载「默认值语义变更」
    /// 的一次性迁移；普通新增字段一律走 serde default 零迁移。
    #[serde(default)]
    pub settings_version: u32,
    /// P102：窗口几何记忆——上次关闭时的窗口位置（逻辑像素，屏幕坐标）。
    /// None = 未记录过（启动回退默认/居中）。只入 config.toml，不在
    /// 设置弹窗展示（它属于窗口自身状态）。
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    /// P102：窗口尺寸（逻辑像素）。None = 未记录过（启动回退默认 1024×768）。
    #[serde(default)]
    pub window_width: Option<f32>,
    #[serde(default)]
    pub window_height: Option<f32>,
}

/// 当前设置策略版本（P63）。历史：
/// - v0→v1：即时保存默认反转——`autosave_enabled` 强制重置为 `false`
///   （对标主流编辑器：已有文件的修改不自动写盘）。老配置在默认开启
///   时期持久化下来的 `true` 若不迁移，翻代码默认值对它们无效；
///   升级后在设置里重新勾选的用户（version 已是 1）不受影响。
pub const SETTINGS_VERSION: u32 = 1;

// 手写 Default 而非 derive：f32/String 的派生默认值（0.0 / ""）不是合法偏好，
// 必须落到规范默认值（"light" / 16.0）。
impl Default for Settings {
    fn default() -> Self {
        Self {
            recent_files: Vec::new(),
            recent_views: HashMap::new(),
            theme: THEME_LIGHT.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            remember_recent_files: true,
            // P63 策略反转：对标主流编辑器，默认不自动写盘（显式 Ctrl+S 才落盘）
            autosave_enabled: false,
            autosave_delay_secs: DEFAULT_AUTOSAVE_DELAY_SECS,
            enable_snapshots: true,
            exit_mode: EXIT_MODE_SNAPSHOT.to_string(),
            remember_session: true,
            snapshot_interval_secs: DEFAULT_SNAPSHOT_INTERVAL_SECS,
            font_family: None,
            // 第 63 轮：不可见字符标记默认全关（主流编辑器同款初始态）
            show_whitespace: false,
            show_line_endings: false,
            // 第 73 轮 ⑯：自动换行默认关（恒等退化回退路径）
            word_wrap: false,
            // 第 64 轮：默认不备份（保持既有「零额外文件」预期）
            backup_mode: BACKUP_MODE_NONE.to_string(),
            hotkeys: HashMap::new(),
            // 新装用户直接落在当前策略版本：不经历迁移（迁移只面向旧文件）
            settings_version: SETTINGS_VERSION,
            // P102：窗口几何默认未记录（首次启动回退默认尺寸/居中）
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

/// 自动保存防抖默认秒数。
pub const DEFAULT_AUTOSAVE_DELAY_SECS: u32 = 2;
/// 防抖秒数允许范围（闭区间），越界值加载时被 clamp。
pub const MIN_AUTOSAVE_DELAY_SECS: u32 = 1;
pub const MAX_AUTOSAVE_DELAY_SECS: u32 = 60;

/// 快照心跳间隔默认秒数（P31）：10 秒 ≈ 崩溃丢失窗口与 IO 频率的折中
/// （§3 P31 第 1 条）。
pub const DEFAULT_SNAPSHOT_INTERVAL_SECS: u32 = 10;
/// 心跳间隔允许范围（闭区间）：下界防写盘风暴，上界防丢失窗口过大。
pub const MIN_SNAPSHOT_INTERVAL_SECS: u32 = 5;
pub const MAX_SNAPSHOT_INTERVAL_SECS: u32 = 120;

/// 关窗模式（P29）：快照直退——置脏页写快照后直接退出，零询问。
pub const EXIT_MODE_SNAPSHOT: &str = "snapshot";
/// 关窗模式（P29）：旧行为——置脏即弹「未保存确认条」逐次询问。
pub const EXIT_MODE_ASK: &str = "ask";

/// 字体族名的长度上限（P34）。真实族名远短于此；超长值视为配置损坏。
pub const MAX_FONT_FAMILY_LEN: usize = 128;

// ---------- 设置弹窗分类页键（P51） ----------

/// 分类页持久化键：config 值与 app 层映射的单一来源（防两套字符串漂移）。
pub const SETTINGS_PAGE_APPEARANCE: &str = "appearance";
pub const SETTINGS_PAGE_FONT: &str = "font";
pub const SETTINGS_PAGE_SAVE: &str = "save";
pub const SETTINGS_PAGE_SESSION: &str = "session";
pub const SETTINGS_PAGE_HOTKEYS: &str = "hotkeys";
pub const SETTINGS_PAGE_ABOUT: &str = "about";

/// 全部合法的分类页键（展示顺序）。
pub const SETTINGS_PAGES: [&str; 6] = [
    SETTINGS_PAGE_APPEARANCE,
    SETTINGS_PAGE_FONT,
    SETTINGS_PAGE_SAVE,
    SETTINGS_PAGE_SESSION,
    SETTINGS_PAGE_HOTKEYS,
    SETTINGS_PAGE_ABOUT,
];

/// 分类页键归一（纯函数）：空白/未知值收敛为默认页（外观）。
/// 第 64 轮用户点单后设置弹窗不再持久化分类位置——本函数保留仅为
/// 公开 API 兼容（历史配置文件里遗留的 settings_page 键由 serde 忽略，
/// 下次保存自然消失），app 侧已无调用方。
pub fn normalize_settings_page(value: &str) -> &'static str {
    let v = value.trim();
    SETTINGS_PAGES
        .iter()
        .copied()
        .find(|key| *key == v)
        .unwrap_or(SETTINGS_PAGE_APPEARANCE)
}

// ---------- 第 64 轮 ⑭：保存时备份 ----------

/// 备份模式：不备份（默认）。
pub const BACKUP_MODE_NONE: &str = "none";
/// 备份模式：同目录 `name.bak`，每次覆盖。
pub const BACKUP_MODE_SIMPLE: &str = "simple";
/// 备份模式：`name.bak/` 目录内 `name.YYYYMMDD-HHMMSS.bak` 历史留存。
pub const BACKUP_MODE_TIMESTAMPED: &str = "timestamped";

#[cfg(test)]
pub(crate) const BACKUP_MODES: [&str; 3] =
    [BACKUP_MODE_NONE, BACKUP_MODE_SIMPLE, BACKUP_MODE_TIMESTAMPED];

fn default_backup_mode() -> String {
    BACKUP_MODE_NONE.to_string()
}

/// 备份模式归一（纯函数可单测）：未知/空白收敛为 none。
pub fn normalize_backup_mode(value: &str) -> &'static str {
    match value.trim() {
        BACKUP_MODE_SIMPLE => BACKUP_MODE_SIMPLE,
        BACKUP_MODE_TIMESTAMPED => BACKUP_MODE_TIMESTAMPED,
        _ => BACKUP_MODE_NONE,
    }
}

// ---------- P62 组合键串归一 ----------

/// 可作热键的命名键（大小写不敏感匹配，规范形如列首大写形式）。
const COMBO_NAMED_KEYS: [&str; 23] = [
    "Home", "End", "PageUp", "PageDown", "Tab", "Insert", "Delete", "Up", "Down", "Left", "Right",
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// 组合键串归一（纯函数可单测）：`ctrl+shift+f` → `Ctrl+Shift+F`。
///
/// 契约（第 60 轮放宽功能键）：
/// - **含 Ctrl** 的组合（原有形态）：键名 = 单个字母/数字或
///   [`COMBO_NAMED_KEYS`] 白名单（大小写不敏感），修饰键顺序规范化为
///   `Ctrl [+Shift] +键名`；
/// - **不含 Ctrl** 的组合：仅放行功能键 `F1`~`F12`（修饰键至多 Shift，
///   规范形 `F2` / `Shift+F2`）——书签导航等主流编辑器同款默认键位所需；
///   裸字母/数字/其余命名键仍拒绝（裸字母是打字正文，放行会吞输入）；
/// - Alt 一律拒绝（AltGr 保护——AltGr 在 Windows 上报为 Ctrl+Alt，
///   放行会挤占欧洲键盘字符输入）。不合法返回 None。
pub fn normalize_combo(value: &str) -> Option<String> {
    let mut ctrl = false;
    let mut shift = false;
    let mut key: Option<String> = None;
    for token in value.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                if ctrl || key.is_some() {
                    return None;
                }
                ctrl = true;
            }
            "shift" => {
                if shift || key.is_some() {
                    return None;
                }
                shift = true;
            }
            "alt" | "meta" | "super" | "win" | "cmd" => return None, // AltGr 保护
            token => {
                if key.is_some() {
                    return None; // 多个键名
                }
                if token.len() == 1 {
                    let ch = token.chars().next()?;
                    if !ch.is_ascii_alphanumeric() {
                        return None;
                    }
                    key = Some(ch.to_ascii_uppercase().to_string());
                } else {
                    let hit = COMBO_NAMED_KEYS
                        .iter()
                        .find(|k| k.eq_ignore_ascii_case(token))?;
                    key = Some((*hit).to_string());
                }
            }
        }
    }
    let key = key?;
    if ctrl {
        let mut out = String::from("Ctrl");
        if shift {
            out.push_str("+Shift");
        }
        out.push('+');
        out.push_str(&key);
        return Some(out);
    }
    // 第 60 轮：无 Ctrl 时仅放行功能键 F1~F12（修饰键至多 Shift）。
    // 键名已过 COMBO_NAMED_KEYS 白名单规范化，这里只再验「F + 1~12」形态。
    let is_function_key = key.len() >= 2
        && key.starts_with('F')
        && key[1..].parse::<u8>().map(|n| (1..=12).contains(&n)).unwrap_or(false);
    if !is_function_key {
        return None;
    }
    let mut out = String::new();
    if shift {
        out.push_str("Shift+");
    }
    out.push_str(&key);
    Some(out)
}

/// `#[serde(default)]` 用：缺字段时的主题默认值。
fn default_theme() -> String {
    THEME_LIGHT.to_string()
}

/// `#[serde(default)]` 用：缺字段时的字号默认值。
fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}

/// `#[serde(default)]` 用：P20 开关缺字段时的默认值（true，不惊扰老用户）。
fn default_true() -> bool {
    true
}

/// `#[serde(default)]` 用：P18 防抖秒数缺字段时的默认值。
fn default_autosave_delay_secs() -> u32 {
    DEFAULT_AUTOSAVE_DELAY_SECS
}

/// `#[serde(default)]` 用：P29 关窗模式缺字段时的默认值（快照直退）。
fn default_exit_mode() -> String {
    EXIT_MODE_SNAPSHOT.to_string()
}

/// `#[serde(default)]` 用：P31 心跳间隔缺字段时的默认值。
fn default_snapshot_interval_secs() -> u32 {
    DEFAULT_SNAPSHOT_INTERVAL_SECS
}

impl Settings {
    pub fn load_from(path: &Path) -> Self {
        let mut settings: Settings = fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        settings.normalize();
        settings
    }

    /// 把可能来自旧文件或手改文件的字段值收敛到合法域。
    /// 在 `load_from` 读完后统一调用，而不是手写反序列化器。
    fn normalize(&mut self) {
        // P63 策略迁移梯子：v0 → 当前版本逐级执行，幂等（重复调用无害）。
        // v0→v1：即时保存默认反转。只重置「旧策略时代持久化下来的 true」，
        // 用户升级后显式勾选的 true（version 已是 1）不再被动。
        if self.settings_version < 1 {
            self.autosave_enabled = false;
            self.settings_version = 1;
        }
        if self.theme != THEME_LIGHT && self.theme != THEME_DARK {
            self.theme = THEME_LIGHT.to_string();
        }
        // clamp 对 NaN 会原样返回 NaN，先排除非有限值。
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            DEFAULT_FONT_SIZE
        };
        // P20：关闭「记住最近文件」时，存量列表一并清空——
        // 只关开关不清数据等于没关（config.toml 里仍躺着完整路径）。
        // P32：光标记忆的键同样是完整路径，必须一起清。
        if !self.remember_recent_files && !self.recent_files.is_empty() {
            self.recent_files.clear();
        }
        if !self.remember_recent_files && !self.recent_views.is_empty() {
            self.recent_views.clear();
        }
        // P32：滚动值消毒（非有限 → 0，防单个坏值；P29 清单同款先例）
        for view in self.recent_views.values_mut() {
            if !view.scroll_top.is_finite() {
                view.scroll_top = 0.0;
            }
        }
        // P32：修剪孤儿——键不在最近列表里的记忆删除（防跨年无界增长）
        self.prune_recent_views();
        // P18：防抖秒数收敛到合法区间（0 秒会变成每秒写盘风暴）
        self.autosave_delay_secs = self
            .autosave_delay_secs
            .clamp(MIN_AUTOSAVE_DELAY_SECS, MAX_AUTOSAVE_DELAY_SECS);
        // P29：关窗模式只认两个规范值，手改/旧文件非法值归一为快照直退
        if self.exit_mode != EXIT_MODE_SNAPSHOT && self.exit_mode != EXIT_MODE_ASK {
            self.exit_mode = EXIT_MODE_SNAPSHOT.to_string();
        }
        // 第 64 轮 ⑭：备份模式归一——未知/空白收敛为 none
        self.backup_mode = normalize_backup_mode(&self.backup_mode).to_string();
        // P31：心跳间隔收敛到合法区间（过密=写盘风暴，过疏=丢失窗口过大）
        self.snapshot_interval_secs = self
            .snapshot_interval_secs
            .clamp(MIN_SNAPSHOT_INTERVAL_SECS, MAX_SNAPSHOT_INTERVAL_SECS);
        // P34：字体族名收敛——空串/纯空白/超长/含控制字符一律回退默认。
        // 只裁剪明显损坏的值；「系统里没装这个字体」由 app 层回退（core
        // 不掌握系统字体清单），且不抹掉配置本身。
        if let Some(name) = &self.font_family {
            let broken = name.trim().is_empty()
                || name.len() > MAX_FONT_FAMILY_LEN
                || name.chars().any(char::is_control);
            if broken {
                self.font_family = None;
            } else if name != name.trim() {
                self.font_family = Some(name.trim().to_owned());
            }
        }
        // P62：热键重映射逐条归一——非法组合条目删除（动作 id 的合法性
        // 由 app 层过滤，core 不掌握动作清单）
        self.hotkeys.retain(|_, combo| {
            match normalize_combo(combo) {
                Some(canonical) => {
                    *combo = canonical;
                    true
                }
                None => false,
            }
        });
    }

    /// 切换正文字体族（P34）：None = 回退默认等宽。Some 值经与加载归一
    /// 同一套规则收敛（空白裁剪），调用方无需预处理。
    pub fn set_font_family(&mut self, family: Option<String>) {
        self.font_family = family;
        self.normalize();
    }

    /// 当前是否为深色主题（仅规范值 `"dark"` 视为深色）。
    pub fn is_dark(&self) -> bool {
        self.theme == THEME_DARK
    }

    /// 切换主题，写回的总是规范字符串 `"light"` / `"dark"`。
    pub fn set_theme(&mut self, dark: bool) {
        self.theme = if dark { THEME_DARK } else { THEME_LIGHT }.to_string();
    }

    /// 原子保存（P7）：走同 crate 的 write_atomic（临时文件 + rename + sync）。
    /// 直接 `fs::write` 会在崩溃/掉电时留下半截 config.toml，
    /// 解析失败后静默回默认 → 最近文件列表无声丢失，这里从根上杜绝。
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(self).unwrap_or_default();
        crate::saver::write_atomic(path, serialized.as_bytes())
    }

    /// 默认配置文件路径；拿不到数据目录时返回 None（功能自动降级）。
    /// P101：便携模式（exe 同目录存在 `portable.txt`）下为 exe 目录内的
    /// `config.toml`；否则走系统配置目录（Windows `%APPDATA%\editpad`），
    /// 见 [`crate::paths`]。
    pub fn config_path() -> Option<PathBuf> {
        crate::paths::data_root().map(|d| d.join("config.toml"))
    }

    pub fn load() -> Self {
        Self::config_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// 尽力保存，错误忽略（设置丢失可接受，不该打断用户）。
    pub fn save(&self) {
        if let Some(p) = Self::config_path() {
            let _ = self.save_to(&p);
        }
    }

    /// 记录一次打开：去重后提到最前，超出上限截断。
    /// P20：关闭「记住最近文件」后变 no-op——路径根本不落盘。
    pub fn push_recent(&mut self, path: &Path) {
        if !self.remember_recent_files {
            return;
        }
        let entry = path.display().to_string();
        self.recent_files.retain(|p| p != &entry);
        self.recent_files.insert(0, entry);
        self.recent_files.truncate(MAX_RECENT_FILES);
        // P32：被挤出上限的条目，其光标记忆一并修剪（列表与记忆恒对齐）
        self.prune_recent_views();
    }

    /// 查询某路径的光标/滚动记忆（P32）；无记录返回 None。
    pub fn recent_view(&self, path: &Path) -> Option<RecentView> {
        let key = path.display().to_string();
        self.recent_views.get(&key).copied()
    }

    /// 记录/更新某路径的光标/滚动记忆（P32）。仅在「记住最近文件」开启
    /// 且该路径已在最近列表中时生效（未列表化的路径不单独记账）；
    /// 滚动值非有限时归一为 0。返回是否产生了实际变化（调用方据此决定
    /// 是否落盘）。
    pub fn set_recent_view(&mut self, path: &Path, view: RecentView) -> bool {
        if !self.remember_recent_files {
            return false;
        }
        let key = path.display().to_string();
        if !self.recent_files.iter().any(|p| p == &key) {
            return false;
        }
        let mut view = view;
        if !view.scroll_top.is_finite() {
            view.scroll_top = 0.0;
        }
        if self.recent_views.get(&key) == Some(&view) {
            return false; // 无变化不落盘
        }
        self.recent_views.insert(key, view);
        true
    }

    /// 修剪孤儿光标记忆：键不在最近文件列表中的条目删除（P32）。
    fn prune_recent_views(&mut self) {
        self.recent_views
            .retain(|key, _| self.recent_files.iter().any(|p| p == key));
    }

    /// 清空最近文件列表（P20「清空记录」按钮）。
    /// 只改内存；调用方随后 `save()` 才会从 config.toml 抹掉痕迹。
    /// P32：光标记忆的键同样是路径，一并清空。
    pub fn clear_recent_files(&mut self) {
        self.recent_files.clear();
        self.recent_views.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 项目内落盘目录：系统 TEMP 在部分沙箱下不可写。
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("settings-{tag}-{}", std::process::id()))
    }

    #[test]
    fn missing_config_falls_back_to_default() {
        let dir = scratch_dir("missing");
        let path = dir.join("no-such.toml");
        assert_eq!(Settings::load_from(&path), Settings::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_recent_dedupes_moves_front_and_truncates() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.push_recent(Path::new(&format!("C:/f/{i}.txt")));
        }
        assert_eq!(s.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(s.recent_files[0], "C:/f/11.txt");

        // 再次打开旧文件 → 提到最前且不重复
        s.push_recent(Path::new("C:/f/5.txt"));
        assert_eq!(s.recent_files[0], "C:/f/5.txt");
        assert_eq!(s.recent_files.iter().filter(|p| **p == "C:/f/5.txt").count(), 1);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("nested").join("config.toml");

        let mut s = Settings::default();
        s.push_recent(Path::new("D:/笔记/中文 文件名.md"));
        s.save_to(&path).expect("保存应成功");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.recent_files, vec!["D:/笔记/中文 文件名.md".to_string()]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_config_without_new_fields_gets_defaults() {
        let dir = scratch_dir("legacy");
        let path = dir.join("config.toml");
        fs::create_dir_all(&dir).unwrap();
        // 旧格式：只有 recent_files，没有 theme / font_size
        fs::write(&path, "recent_files = [\"C:/old.txt\"]").unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.recent_files, vec!["C:/old.txt".to_string()]);
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.font_size, 16.0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_theme_and_font_size_are_normalized_on_load() {
        let dir = scratch_dir("invalid");
        fs::create_dir_all(&dir).unwrap();

        // 越界 + 非法主题 → 归一为 "light" / 上界 28.0
        let over = dir.join("over.toml");
        fs::write(&over, "theme = \"blue\"\nfont_size = 99\nrecent_files = []").unwrap();
        let loaded = Settings::load_from(&over);
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.font_size, 28.0);

        // 下界同样被 clamp，合法 dark 值保留
        let under = dir.join("under.toml");
        fs::write(&under, "theme = \"dark\"\nfont_size = -3\n").unwrap();
        let low = Settings::load_from(&under);
        assert!(low.is_dark());
        assert_eq!(low.font_size, 10.0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn theme_helpers_and_roundtrip_preserve_fields() {
        let mut s = Settings::default();
        assert!(!s.is_dark());
        s.set_theme(true);
        assert!(s.is_dark());
        assert_eq!(s.theme, "dark");
        s.set_theme(false);
        assert_eq!(s.theme, "light");

        s.set_theme(true);
        s.font_size = 20.5;
        s.push_recent(Path::new("E:/edit/note.md"));

        let dir = scratch_dir("roundtrip2");
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, s);
        assert!(loaded.is_dark());
        assert_eq!(loaded.font_size, 20.5);

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P7 原子保存 ----------

    #[test]
    fn failed_save_leaves_no_tmp_and_keeps_old_config_intact() {
        // P7 回归：保存走「临时文件 + rename」而非直接覆写。
        // 目标位置被目录占用 → File::create 必败；断言：
        // 1) 不留 *.editpad-tmp 残留（失败即清理现场）；
        // 2) 既有 config.toml 内容原封不动（不存在被截断成半截的窗口）。
        let dir = scratch_dir("atomic-fail");
        fs::create_dir_all(&dir).unwrap();

        let target = dir.join("config.toml");
        fs::write(&target, "recent_files = [\"C:/keep.txt\"]").unwrap();

        // 占住目标路径：同名目录让写入必然失败
        let blocker = dir.join("occupied");
        fs::create_dir_all(&blocker).unwrap();

        let result = Settings::default().save_to(&blocker);
        assert!(result.is_err(), "写目录路径应失败");

        let tmp = dir.join("occupied.editpad-tmp");
        assert!(!tmp.exists(), "失败的保存不得留下临时文件残留");

        let kept = fs::read_to_string(&target).unwrap();
        assert_eq!(kept, "recent_files = [\"C:/keep.txt\"]", "旧配置必须完好");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_overwrite_consumes_tmp_sibling() {
        // 连续两轮保存都成功后，同目录不允许有 .editpad-tmp 兜底残留
        let dir = scratch_dir("atomic-overwrite");
        let path = dir.join("nested").join("config.toml");

        let mut s = Settings::default();
        s.push_recent(Path::new("C:/a.txt"));
        s.save_to(&path).unwrap();
        s.push_recent(Path::new("C:/b.txt"));
        s.save_to(&path).unwrap();

        let tmp = path.with_file_name("config.toml.editpad-tmp");
        assert!(!tmp.exists(), "rename 成功后临时文件必须消失");
        assert_eq!(
            Settings::load_from(&path).recent_files,
            vec!["C:/b.txt".to_string(), "C:/a.txt".to_string()]
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P20 最近文件隐私选项 ----------

    #[test]
    fn push_recent_is_noop_when_remember_disabled() {
        // 关闭「记住最近文件」：打开动作不得在配置里留下任何路径
        let mut s = Settings::default();
        s.remember_recent_files = false;
        s.push_recent(Path::new("C:/secret/report.docx"));
        s.push_recent(Path::new("D:/私人/合同.pdf"));
        assert!(s.recent_files.is_empty(), "关闭后 push_recent 必须 no-op");
    }

    #[test]
    fn loading_with_remember_disabled_purges_existing_list() {
        // 只关开关不清数据等于没关：加载时存量列表必须被清空
        let dir = scratch_dir("p20-purge");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "remember_recent_files = false\nrecent_files = [\"C:/old/a.txt\", \"C:/old/b.txt\"]",
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        assert!(!loaded.remember_recent_files);
        assert!(loaded.recent_files.is_empty(), "存量路径必须随加载清空");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_recent_files_roundtrips_to_disk() {
        // 「清空记录」按钮语义：内存清空 + 落盘后 config.toml 里不再有路径
        let dir = scratch_dir("p20-clear");
        let path = dir.join("config.toml");

        let mut s = Settings::default();
        s.push_recent(Path::new("C:/keep-before-clear.txt"));
        s.save_to(&path).expect("首次保存应成功");

        s.clear_recent_files();
        assert!(s.recent_files.is_empty());
        s.save_to(&path).expect("清空后保存应成功");

        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("keep-before-clear"),
            "落盘内容不得再含任何历史路径"
        );
        assert!(Settings::load_from(&path).recent_files.is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_config_without_remember_flag_keeps_recording() {
        // 旧 config.toml 缺 P20 字段 → 默认 true，行为与升级前一致
        let dir = scratch_dir("p20-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "recent_files = [\"C:/old.txt\"]").unwrap();

        let loaded = Settings::load_from(&path);
        assert!(loaded.remember_recent_files, "缺字段必须回默认 true");
        assert_eq!(loaded.recent_files.len(), 1, "既有记录不受影响");

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P18 即时保存设置 / P63 策略反转 ----------

    #[test]
    fn autosave_defaults_off_and_delay_still_two_seconds() {
        // P63：默认反转——对标主流编辑器，已有文件的修改不自动写盘；
        // 防抖秒数默认值不变（重新开启的用户沿用 2s）。
        let s = Settings::default();
        assert!(!s.autosave_enabled, "P63 起默认关闭：原文件只在显式保存时被写");
        assert_eq!(s.settings_version, SETTINGS_VERSION, "新装用户直接落在当前策略版本");
        assert_eq!(s.autosave_delay_secs, 2);

        // 旧配置缺字段 → 同样得到默认关 + 当前版本号
        let dir = scratch_dir("p18-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(!loaded.autosave_enabled);
        assert_eq!(loaded.autosave_delay_secs, 2);
        assert_eq!(loaded.settings_version, SETTINGS_VERSION);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn p63_migration_resets_persisted_autosave_true_exactly_once() {
        // 核心迁移场景：默认开启时代写下的 autosave_enabled = true，
        // 只翻代码默认值对它无效——version 迁移必须把它一次性重置。
        let dir = scratch_dir("p63-migrate");
        fs::create_dir_all(&dir).unwrap();

        let legacy = dir.join("v0.toml");
        fs::write(&legacy, "autosave_enabled = true\nautosave_delay_secs = 5\n").unwrap();
        let migrated = Settings::load_from(&legacy);
        assert!(!migrated.autosave_enabled, "v0 持久化的 true 必须被一次性重置");
        assert_eq!(migrated.autosave_delay_secs, 5, "非策略字段不受迁移影响");
        assert_eq!(migrated.settings_version, SETTINGS_VERSION);

        // 重置结果落盘后（version=1），用户在设置里重新勾选的 true 必须被尊重
        let mut reenabled = migrated;
        reenabled.autosave_enabled = true;
        let current = dir.join("v1.toml");
        reenabled.save_to(&current).unwrap();
        let loaded = Settings::load_from(&current);
        assert!(
            loaded.autosave_enabled,
            "升级后用户的显式选择不得被二次重置"
        );
        assert_eq!(loaded.settings_version, SETTINGS_VERSION);

        // version=1 的文件无论开关真假都不再触发迁移
        let off = dir.join("v1-off.toml");
        fs::write(
            &off,
            format!("settings_version = {SETTINGS_VERSION}\nautosave_enabled = false\n"),
        )
        .unwrap();
        let loaded_off = Settings::load_from(&off);
        assert!(!loaded_off.autosave_enabled && loaded_off.settings_version == SETTINGS_VERSION);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_version_roundtrips_to_disk() {
        // 版本号参与 roundtrip：落盘再读回不漂移
        let dir = scratch_dir("p63-roundtrip");
        let path = dir.join("config.toml");
        let s = Settings::default();
        s.save_to(&path).expect("保存应成功");
        assert_eq!(Settings::load_from(&path).settings_version, SETTINGS_VERSION);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn autosave_delay_clamped_to_legal_range() {
        let dir = scratch_dir("p18-clamp");
        fs::create_dir_all(&dir).unwrap();

        let zero = dir.join("zero.toml");
        fs::write(&zero, "autosave_enabled = true\nautosave_delay_secs = 0\n").unwrap();
        assert_eq!(Settings::load_from(&zero).autosave_delay_secs, 1, "0 秒会变成写盘风暴，收敛到下界");

        let huge = dir.join("huge.toml");
        fs::write(&huge, "autosave_delay_secs = 9999\n").unwrap();
        assert_eq!(Settings::load_from(&huge).autosave_delay_secs, 60);

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P29 会话快照设置 ----------

    #[test]
    fn snapshot_settings_default_on_with_snapshot_exit_and_legacy_compat() {
        let s = Settings::default();
        assert!(s.enable_snapshots, "快照默认开启（P30/P31 的前提底座）");
        assert_eq!(s.exit_mode, EXIT_MODE_SNAPSHOT, "默认快照直退零询问");

        // 旧 config.toml 缺 P29 字段 → 同样得到默认值
        let dir = scratch_dir("p29-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(loaded.enable_snapshots);
        assert_eq!(loaded.exit_mode, EXIT_MODE_SNAPSHOT);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exit_mode_normalized_and_legal_values_preserved() {
        let dir = scratch_dir("p29-normalize");
        fs::create_dir_all(&dir).unwrap();

        // 非法值归一为快照直退
        let weird = dir.join("weird.toml");
        fs::write(&weird, "exit_mode = \"never\"\n").unwrap();
        assert_eq!(Settings::load_from(&weird).exit_mode, EXIT_MODE_SNAPSHOT);

        // 合法的「每次询问」原样保留
        let ask = dir.join("ask.toml");
        fs::write(&ask, format!("exit_mode = \"{EXIT_MODE_ASK}\"\n")).unwrap();
        assert_eq!(Settings::load_from(&ask).exit_mode, EXIT_MODE_ASK);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_settings_roundtrip() {
        let mut s = Settings::default();
        s.enable_snapshots = false;
        s.exit_mode = EXIT_MODE_ASK.to_string();

        let dir = scratch_dir("p29-roundtrip");
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");
        assert_eq!(Settings::load_from(&path), s, "P29 字段必须参与 roundtrip");

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P30 启动会话恢复设置 ----------

    #[test]
    fn remember_session_defaults_on_and_legacy_config_compatible() {
        let s = Settings::default();
        assert!(s.remember_session, "会话恢复默认开启（「重开还是上次的界面」）");

        // 旧 config.toml 缺 P30 字段 → 默认 true，行为与升级前一致
        let dir = scratch_dir("p30-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(loaded.remember_session, "缺字段必须回默认 true");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remember_session_roundtrips_to_disk() {
        let mut s = Settings::default();
        s.remember_session = false;

        let dir = scratch_dir("p30-roundtrip");
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");
        assert_eq!(Settings::load_from(&path), s, "P30 字段必须参与 roundtrip");

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P31 周期快照心跳设置 ----------

    #[test]
    fn snapshot_interval_defaults_and_legacy_config_compatible() {
        let s = Settings::default();
        assert_eq!(
            s.snapshot_interval_secs, 10,
            "心跳间隔默认 10s（崩溃丢失窗口与 IO 频率的折中）"
        );

        // 旧 config.toml 缺 P31 字段 → 同样得到默认值
        let dir = scratch_dir("p31-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        assert_eq!(Settings::load_from(&path).snapshot_interval_secs, 10);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_interval_clamped_to_legal_range() {
        let dir = scratch_dir("p31-clamp");
        fs::create_dir_all(&dir).unwrap();

        // 过密 → 收敛到下界（防写盘风暴）
        let dense = dir.join("dense.toml");
        fs::write(&dense, "snapshot_interval_secs = 0\n").unwrap();
        assert_eq!(Settings::load_from(&dense).snapshot_interval_secs, MIN_SNAPSHOT_INTERVAL_SECS);

        // 过疏 → 收敛到上界（防丢失窗口过大）
        let sparse = dir.join("sparse.toml");
        fs::write(&sparse, "snapshot_interval_secs = 99999\n").unwrap();
        assert_eq!(Settings::load_from(&sparse).snapshot_interval_secs, MAX_SNAPSHOT_INTERVAL_SECS);

        // 合法值原样保留
        let legal = dir.join("legal.toml");
        fs::write(&legal, "snapshot_interval_secs = 45\n").unwrap();
        assert_eq!(Settings::load_from(&legal).snapshot_interval_secs, 45);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_interval_roundtrips_to_disk() {
        let mut s = Settings::default();
        s.snapshot_interval_secs = 45;

        let dir = scratch_dir("p31-roundtrip");
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");
        assert_eq!(Settings::load_from(&path), s, "P31 字段必须参与 roundtrip");

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P102 窗口几何记忆 ----------

    #[test]
    fn window_geometry_roundtrips_to_disk_with_defaults_empty() {
        // 默认 None：旧配置缺字段加载后仍为 None（零迁移）
        let mut s = Settings::default();
        assert_eq!((s.window_x, s.window_y), (None, None));
        assert_eq!((s.window_width, s.window_height), (None, None));

        let dir = scratch_dir("p102-geometry");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        // 缺字段的旧配置：加载后几何为 None
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let legacy = Settings::load_from(&path);
        assert_eq!((legacy.window_x, legacy.window_y), (None, None));

        // 记录后 roundtrip 逐字还原（含小数尺寸）
        s.window_x = Some(120);
        s.window_y = Some(-8);
        s.window_width = Some(1280.5);
        s.window_height = Some(720.25);
        s.save_to(&path).expect("保存应成功");
        let loaded = Settings::load_from(&path);
        assert_eq!((loaded.window_x, loaded.window_y), (Some(120), Some(-8)));
        assert_eq!(
            (loaded.window_width, loaded.window_height),
            (Some(1280.5), Some(720.25)),
            "窗口几何必须参与 roundtrip"
        );

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P32 最近文件光标/滚动记忆 ----------

    #[test]
    fn recent_view_roundtrips_with_legacy_string_list() {
        // 旧 config.toml（recent_files 纯字符串数组）加载后光标记忆为空；
        // 新写入的记忆与列表一起 roundtrip，且旧字段格式不被破坏
        let dir = scratch_dir("p32-roundtrip");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // 第一步：旧格式文件照常解析，记忆为空
        fs::write(&path, "recent_files = [\"C:/old.txt\"]").unwrap();
        let legacy = Settings::load_from(&path);
        assert_eq!(legacy.recent_files, vec!["C:/old.txt".to_string()]);
        assert!(legacy.recent_views.is_empty(), "旧配置不应凭空长出记忆");

        // 第二步：补记忆 → 落盘 → 重载逐字还原
        let mut s = legacy;
        s.set_recent_view(
            Path::new("C:/old.txt"),
            RecentView { line: 42, col: 7, scroll_top: 128.5 },
        );
        s.save_to(&path).expect("保存应成功");
        let loaded = Settings::load_from(&path);
        assert_eq!(
            loaded.recent_view(Path::new("C:/old.txt")),
            Some(RecentView { line: 42, col: 7, scroll_top: 128.5 }),
            "光标记忆必须参与 roundtrip"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_recent_view_rejects_unlisted_paths_and_disabled_flag() {
        let mut s = Settings::default();
        s.push_recent(Path::new("C:/listed.txt"));

        // 未在最近列表中的路径：不记账（避免记忆无界增长）
        assert!(!s.set_recent_view(
            Path::new("C:/never-opened.txt"),
            RecentView { line: 1, col: 0, scroll_top: 0.0 },
        ));
        assert!(s.recent_views.is_empty());

        // 关闭「记住最近文件」：整体 no-op（路径是隐私数据，P20 口径）
        s.remember_recent_files = false;
        assert!(!s.set_recent_view(
            Path::new("C:/listed.txt"),
            RecentView { line: 1, col: 0, scroll_top: 0.0 },
        ));
    }

    #[test]
    fn recent_view_updates_in_place_and_reports_changes() {
        let mut s = Settings::default();
        s.push_recent(Path::new("C:/doc.txt"));
        let v1 = RecentView { line: 3, col: 0, scroll_top: 10.0 };

        assert!(s.set_recent_view(Path::new("C:/doc.txt"), v1), "首次记录应报告变化");
        // 相同值再写：无变化（调用方据此跳过落盘）
        assert!(!s.set_recent_view(Path::new("C:/doc.txt"), v1));
        // 值变化：再次报告
        let v2 = RecentView { line: 9, col: 2, scroll_top: 20.0 };
        assert!(s.set_recent_view(Path::new("C:/doc.txt"), v2));
        assert_eq!(s.recent_view(Path::new("C:/doc.txt")), Some(v2));

        // 非有限滚动值消毒为 0
        let bad = RecentView { line: 1, col: 0, scroll_top: f32::NAN };
        assert!(s.set_recent_view(Path::new("C:/doc.txt"), bad));
        assert_eq!(
            s.recent_view(Path::new("C:/doc.txt")).unwrap().scroll_top,
            0.0,
            "NaN 滚动必须归一为 0"
        );
    }

    #[test]
    fn pruning_keeps_recent_views_aligned_with_the_list() {
        let mut s = Settings::default();
        for i in 0..MAX_RECENT_FILES as u32 {
            s.push_recent(Path::new(&format!("C:/f/{i}.txt")));
            s.set_recent_view(
                Path::new(&format!("C:/f/{i}.txt")),
                RecentView { line: i as usize, col: 0, scroll_top: 0.0 },
            );
        }
        assert_eq!(s.recent_views.len(), MAX_RECENT_FILES);

        // 再开一个新文件：f/0 被挤出上限 → 它的光标记忆必须随之消失
        s.push_recent(Path::new("C:/f/new.txt"));
        assert!(
            !s.recent_views.contains_key("C:/f/0.txt"),
            "被挤出的条目不得残留孤儿记忆"
        );
        // 剩余记忆 = f1..f9 共 9 条（new 刚打开尚无光标记录）
        assert_eq!(s.recent_views.len(), MAX_RECENT_FILES - 1);
        assert!(s.recent_view(Path::new("C:/f/new.txt")).is_none(), "新文件尚无记忆");

        // 清空记录按钮：列表与记忆一起清（键即路径，不能留痕）
        s.clear_recent_files();
        assert!(s.recent_views.is_empty());

        // 加载归一同款修剪：手改 config 塞进孤儿键 → 加载时被清掉
        let dir = scratch_dir("p32-prune");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "recent_files = [\"C:/real.txt\"]\n\
             [recent_views.\"C:/ghost.txt\"]\nline = 5\ncol = 0\nscroll_top = 0.0\n",
        )
        .unwrap();
        let loaded = Settings::load_from(&path);
        assert!(
            !loaded.recent_views.contains_key("C:/ghost.txt"),
            "孤儿记忆必须在加载归一时修剪"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn disabling_remember_recent_files_purges_views_on_load() {
        // P20 开关关闭时，光标记忆里的路径同样是隐私痕迹——一并清空
        let dir = scratch_dir("p32-purge");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "remember_recent_files = false\nrecent_files = [\"C:/a.txt\"]\n\
             [recent_views.\"C:/a.txt\"]\nline = 2\ncol = 1\nscroll_top = 4.0\n",
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        assert!(loaded.recent_files.is_empty());
        assert!(loaded.recent_views.is_empty(), "关闭开关后记忆必须随加载清空");

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P34 字体选择设置 ----------

    #[test]
    fn font_family_defaults_to_none_and_legacy_config_compatible() {
        let s = Settings::default();
        assert_eq!(s.font_family, None, "默认必须回退现状等宽行为");

        // 旧 config.toml 缺 P34 字段 → None（serde default），零迁移升级
        let dir = scratch_dir("p34-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\nfont_size = 18\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.font_family, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn font_family_broken_values_normalized_on_load() {
        let dir = scratch_dir("p34-normalize");
        fs::create_dir_all(&dir).unwrap();

        // 空串 / 纯空白 / 超长 / 控制字符：一律收敛为 None
        for (tag, raw) in [
            ("empty", "font_family = \"\"\n"),
            ("blank", "font_family = \"   \\t \"\n"),
            ("long", &format!("font_family = \"{}\"\n", "F".repeat(MAX_FONT_FAMILY_LEN + 1))),
            ("control", "font_family = \"Con\\ntrol\"\n"),
        ] {
            let path = dir.join(format!("{tag}.toml"));
            fs::write(&path, raw).unwrap();
            assert_eq!(
                Settings::load_from(&path).font_family,
                None,
                "{tag} 值必须在加载时收敛为默认"
            );
        }

        // 首尾空白裁剪后保留；合法值原样保留
        let padded = dir.join("padded.toml");
        fs::write(&padded, "font_family = \"  NSimSun  \"\n").unwrap();
        assert_eq!(
            Settings::load_from(&padded).font_family,
            Some("NSimSun".to_owned())
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn font_family_roundtrips_and_setter_normalizes() {
        let mut s = Settings::default();
        s.set_font_family(Some("Sarasa Mono SC".to_owned()));
        assert_eq!(s.font_family.as_deref(), Some("Sarasa Mono SC"));

        let dir = scratch_dir("p34-roundtrip");
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");
        assert_eq!(
            Settings::load_from(&path).font_family,
            Some("Sarasa Mono SC".to_owned()),
            "P34 字段必须参与 roundtrip"
        );

        // 切换入口复用同一套归一规则：坏值进不来
        s.set_font_family(Some("   ".to_owned()));
        assert_eq!(s.font_family, None);
        // 回退默认等宽
        s.set_font_family(None);
        assert_eq!(s.font_family, None);

        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P51 设置分类页记忆（第 64 轮撤销后仅存归一纯函数） ----------

    #[test]
    fn normalize_settings_page_registry_and_fallbacks() {
        // 合法键透传，未知/空白归一默认（函数保留仅为公开 API 兼容，
        // app 侧已不再持久化/恢复分类位置）
        assert_eq!(normalize_settings_page("hotkeys"), SETTINGS_PAGE_HOTKEYS);
        assert_eq!(normalize_settings_page(" no-such "), SETTINGS_PAGE_APPEARANCE);
        assert_eq!(normalize_settings_page(""), SETTINGS_PAGE_APPEARANCE);
        // 六个注册键全部合法（注册表完整性）
        for key in SETTINGS_PAGES {
            assert_eq!(normalize_settings_page(key), key, "注册键 {key} 必须合法");
        }
        // 旧 config.toml 里的 settings_page 遗留键：serde 忽略未知字段，
        // 加载成功且不再回写该键
        let dir = scratch_dir("p51-legacy");
        fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("legacy.toml");
        fs::write(&legacy, "settings_page = \"hotkeys\"\ntheme = \"dark\"\n").unwrap();
        let s = Settings::load_from(&legacy);
        assert_eq!(s.theme, "dark");
        let path = dir.join("roundtrip.toml");
        s.save_to(&path).expect("保存应成功");
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("settings_page"), "遗留键不得被回写");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_mode_normalize_default_and_roundtrip() {
        // 默认 none；三模式合法键透传；未知/带空白归一
        assert_eq!(Settings::default().backup_mode, BACKUP_MODE_NONE);
        for mode in BACKUP_MODES {
            assert_eq!(normalize_backup_mode(mode), mode, "mode {mode}");
        }
        assert_eq!(normalize_backup_mode(" simple "), BACKUP_MODE_SIMPLE);
        assert_eq!(normalize_backup_mode("nope"), BACKUP_MODE_NONE);
        // roundtrip：timestamped 落盘原样读回；损坏值加载归一为 none
        let dir = scratch_dir("backup-mode");
        fs::create_dir_all(&dir).unwrap();
        let mut s = Settings::default();
        s.backup_mode = BACKUP_MODE_TIMESTAMPED.to_string();
        let path = dir.join("config.toml");
        s.save_to(&path).expect("保存应成功");
        assert_eq!(
            Settings::load_from(&path).backup_mode,
            BACKUP_MODE_TIMESTAMPED
        );
        let weird = dir.join("weird.toml");
        fs::write(&weird, "backup_mode = \"hacked\"\n").unwrap();
        assert_eq!(Settings::load_from(&weird).backup_mode, BACKUP_MODE_NONE);
        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P62 热键重映射 ----------

    #[test]
    fn normalize_combo_contract() {
        // 规范化：大小写/修饰键顺序收敛为「Ctrl [+Shift] +键名」
        assert_eq!(normalize_combo("ctrl+s"), Some("Ctrl+S".to_owned()));
        assert_eq!(normalize_combo("CTRL + shift + f"), Some("Ctrl+Shift+F".to_owned()));
        assert_eq!(normalize_combo("Ctrl+Home"), Some("Ctrl+Home".to_owned()));
        assert_eq!(normalize_combo("ctrl+8"), Some("Ctrl+8".to_owned()));
        // 必须含 Ctrl
        assert_eq!(normalize_combo("shift+s"), None);
        assert_eq!(normalize_combo("s"), None);
        // Alt 一律拒绝（AltGr 保护）
        assert_eq!(normalize_combo("ctrl+alt+q"), None);
        assert_eq!(normalize_combo("alt+f4"), None);
        // 键名白名单
        assert_eq!(normalize_combo("ctrl+foo"), None);
        assert_eq!(normalize_combo("ctrl+1+2"), None);
        assert_eq!(normalize_combo("ctrl++"), None);
        // 命名键大小写不敏感
        assert_eq!(normalize_combo("ctrl+home"), Some("Ctrl+Home".to_owned()));
        assert_eq!(normalize_combo("ctrl+F12"), Some("Ctrl+F12".to_owned()));
        // 第 60 轮：裸/Shift 功能键放行（书签导航默认键位）
        assert_eq!(normalize_combo("f2"), Some("F2".to_owned()));
        assert_eq!(normalize_combo("shift+f2"), Some("Shift+F2".to_owned()));
        assert_eq!(normalize_combo("SHIFT + F12"), Some("Shift+F12".to_owned()));
        // 无 Ctrl 的非功能键仍拒绝（裸字母是打字正文，不能被热键吞掉）
        assert_eq!(normalize_combo("home"), None);
        assert_eq!(normalize_combo("insert"), None);
        // F13 不在白名单
        assert_eq!(normalize_combo("f13"), None);
    }

    #[test]
    fn hotkeys_map_normalized_roundtrip_and_legacy() {
        // 默认 = 空表（全默认组合）
        assert!(Settings::default().hotkeys.is_empty());

        let dir = scratch_dir("p62-hotkeys");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // 合法条目规范化保留；非法条目删除；旧 config 无字段 → 空
        let mut s = Settings::default();
        s.hotkeys.insert("save".to_owned(), "ctrl+shift+s".to_owned());
        s.hotkeys.insert("bogus".to_owned(), "ctrl+foo".to_owned());
        s.save_to(&path).expect("保存应成功");
        let loaded = Settings::load_from(&path);
        assert_eq!(
            loaded.hotkeys.get("save").map(String::as_str),
            Some("Ctrl+Shift+S"),
            "合法重映射必须规范化保留并 roundtrip"
        );
        assert!(!loaded.hotkeys.contains_key("bogus"), "非法组合必须删除");

        // 旧 config（无 P62 字段）→ 空表，零迁移
        let legacy = dir.join("legacy.toml");
        fs::write(&legacy, "theme = \"dark\"\n").unwrap();
        assert!(Settings::load_from(&legacy).hotkeys.is_empty());

        fs::remove_dir_all(&dir).ok();
    }
}
