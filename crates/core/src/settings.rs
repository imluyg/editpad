//! 应用设置持久化：最近打开的文件列表。
//!
//! 存储位置由 [`crate::paths`] 决定：P102 实例隔离——数据根目录 =
//! 系统配置目录下按 **exe 路径哈希** 分出的实例目录（Windows 上
//! `%APPDATA%\editpad\instances\<实例键>\`），每份拷贝各搞各的数据；
//! 首次运行把旧版遗留的 `%APPDATA%\editpad\` 整体搬入首个实例。
//! 读写都是尽力而为：目录不可写时回退默认值，绝不影响编辑器本体。
//! ⚠️ P355 起**「读不出」不再静默**：`config.toml` 在而读不出（非 UTF-8／语法整份坏）
//! 归 [`SettingsRead::Broken`]，由调用方留档并提示——旧形状是
//! `read_to_string(..).unwrap_or_default()`，把"读不出"当成"没有配置"，全部偏好回默认
//! 而被窗口几何本次启动写回磁盘＝丢失固化。个别坏字段仍按 R-15 逐键回默认，不算故障。

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

/// P154：界面语言的唯二合法取值（P155 起类型化为 [`crate::lang::Lang`]）；
/// 其他值在加载时归一为 [`LANG_ZH_CN`]。
///
/// 这两个常量保留为**持久化短码**的单一来源（[`crate::lang::Lang::code`]
/// 与配置文件内容都以此为准），避免短码字面量散落各处。
pub const LANG_ZH_CN: &str = "zh-CN";
pub const LANG_EN: &str = "en";

/// 界面语言归一（纯函数便于单测与设置层复用）：大小写/下划线/空白宽松
/// 匹配 `zh-CN` 与 `en`；未知值一律回落 [`LANG_ZH_CN`]（默认界面）。
///
/// P155：返回 [`crate::lang::Lang`]（旧签名返回 `&'static str`——字符串
/// 语言类型无法承载 i18n 文案表，且每次比较都要走短码）。
pub fn normalize_language(value: &str) -> crate::lang::Lang {
    crate::lang::Lang::from_code(value)
}

/// 字号允许范围（闭区间）与默认值；越界值在加载时被 clamp。
/// P116：上限 28 → 48（用户点单：调节范围太小；下限 10 保持）。
pub const MIN_FONT_SIZE: f32 = 10.0;
pub const MAX_FONT_SIZE: f32 = 48.0;
const DEFAULT_FONT_SIZE: f32 = 16.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// P352：缺字段取什么值，只有这一个来源＝下面那份手写 `Default`。
// 旧形状是每个字段各写一句 `#[serde(default = "…")]`——同一件事写两份，
// 25 格里恰好有 1 格相反（`autosave_enabled` 的文档与 `Default` 都是**关**，
// serde 那份还停在 P18 时代的**开**；P63 反转默认值时只翻了看得见的那两处）。
// 容器级 default 把「两份写法会漂」这一族在结构上封掉：新增字段不写任何属性
// 就自动继承规范默认值。唯一例外是 `settings_version`（它缺字段＝旧文件＝v0）。
#[serde(default)]
pub struct Settings {
    pub recent_files: Vec<String>,
    /// 最近文件的光标/滚动记忆（P32）：键 = 路径字符串，与
    /// [`Self::recent_files`] 对齐。修剪规则：键不在最近列表中即删除
    /// （加载归一与 push_recent 后都会对齐），「记住最近文件」关闭时整体清空
    /// ——路径本身是隐私数据（P20 同款口径），不能换个字段继续留痕。
    pub recent_views: HashMap<String, RecentView>,
    /// 界面主题：只允许 `"light"` 或 `"dark"`，非法输入在加载时归一。
    pub theme: String,
    /// P154：界面语言。P155 起类型化为 [`crate::lang::Lang`]（默认
    /// [`crate::lang::Lang::ZhCn`]），承担两件事：**界面文案**（`lang`
    /// 模块的字符串表）与 **UI 字体族**（app 层候选链）。旧配置的
    /// `"zh-CN"` / `"en"` 短码逐字节兼容；未知值反序列化回落中文简体。
    pub language: crate::lang::Lang,
    /// 编辑器字号，加载时 clamp 到 `[MIN_FONT_SIZE, MAX_FONT_SIZE]`。
    pub font_size: f32,
    /// 是否记住最近打开的文件（P20 隐私选项）。默认 true 保持既有行为；
    /// 关闭后 `push_recent` 变 no-op，且加载时清空存量列表——
    /// config.toml 不再留任何文件路径痕迹。
    pub remember_recent_files: bool,
    /// 编辑后自动写盘开关（P18 引入；P63 策略反转，对标主流编辑器）。
    /// **默认关闭**：修改只留在内存（标签页 ● 标记），直到显式 Ctrl+S——
    /// 原文件只在用户显式动作时被写；崩溃防护由会话快照（P31 心跳）
    /// 兜底，不动原文件。显式开启的用户选择照常持久化与尊重；
    /// 历史上默认开启时期持久化下来的 `true` 由 [`Self::settings_version`]
    /// 迁移一次性重置（见 [`SETTINGS_VERSION`]）。
    pub autosave_enabled: bool,
    /// 自动保存防抖秒数：加载时收敛到 `[1, 60]`。
    pub autosave_delay_secs: u32,
    /// 会话快照总开关（P29，隐私对齐 P20 先例）：明文快照落盘必须可一键关闭。
    /// 关闭后退出回旧确认条行为；加载/启动时清空存量快照区。
    pub enable_snapshots: bool,
    /// 关窗行为（P29）：[`EXIT_MODE_SNAPSHOT`] = 置脏页写快照后直接退出、
    /// 零询问（默认）；[`EXIT_MODE_ASK`] = 每次弹「未保存确认条」旧行为。
    /// 非法值在加载时归一为快照直退。
    pub exit_mode: String,
    /// 启动会话恢复开关（P30）。默认开启：「再次打开还是上次的界面」；
    /// 关闭 = 启动恒空白页、退出不写会话清单（存量快照区在启动时清空）。
    pub remember_session: bool,
    /// 周期快照心跳间隔秒数（P31）：运行中每隔 N 秒巡检一次置脏页，
    /// 把内容有变化的页增量写进快照区（崩溃至多丢一个间隔的输入）。
    /// 加载时收敛到 `[MIN_SNAPSHOT_INTERVAL_SECS, MAX_SNAPSHOT_INTERVAL_SECS]`。
    pub snapshot_interval_secs: u32,
    /// 正文字体族名（P34）。None = 默认等宽（现状行为，旧配置缺字段兼容）；
    /// Some(名字) = 用系统里该族渲染正文与 UI。加载时归一：空串/纯空白/
    /// 超长/含控制字符的值收敛为 None；「名字未安装」的回退在 app 层做
    /// （core 不掌握系统字体清单），且只影响本次生效，不抹掉用户配置。
    pub font_family: Option<String>,
    /// 第 63 轮：显示空白字符覆盖标记（空格点/制表符箭头）。默认关；
    /// 纯渲染层偏好，不影响文档模型与命中测试。
    pub show_whitespace: bool,
    /// 第 63 轮：显示行尾符标记（行尾短标）。默认关。
    pub show_line_endings: bool,
    /// 第 73 轮 ⑯：自动换行（软换行）开关。默认关——关闭时视觉得到
    /// 恒等退化（与软换行引入前的渲染逐像素一致），一键可回退。
    /// 开启时：正文按视口宽折行为若干视觉行；水平滚动条隐藏、
    /// scroll_left 锁 0、列块编辑禁用（设计文档）。
    pub word_wrap: bool,
    /// P132（路线图 C4）：缩进参考线——行首缩进制表位倍数处的淡竖线。
    /// 默认开（主流编辑器同款初始态）。
    pub indent_guides: bool,
    /// P132（路线图 C5）：右缘标尺列——固定显示列处的纵向辅助线。
    /// 0 = 关闭（默认）。加载归一钳到 [`MAX_EDGE_COLUMN`]。
    pub edge_column: u32,
    /// P322（功能队列①）：多行粘贴按光标所在行的缩进对齐。
    ///
    /// **默认关**——关着时粘贴与引入本设置前逐字节相同（用户点单：日志正文被
    /// 改缩进是真实伤害，这类行为变更必须显式开启）。开启后走
    /// [`crate::document::align_paste_indent`]：目标与块基准互为前缀才平移，
    /// 制表符块粘进空格行这类"平移量没有定义"的情形一律原样。
    pub paste_align_indent: bool,
    /// B12（功能队列⑤）：变更历史行边条——行号栏最左缘 2px 竖条，标出
    /// **相对上次落盘改过／新增的行**，同一变更块内本身没变的那一段走淡色
    /// （撤销回来的位置）。
    ///
    /// **默认开**：它是被动显示，不改任何内容、不参与置脏与撤销，风险面与
    /// `indent_guides` 同类（那条也默认开）。关掉后边条一格都不画，缓存一并清空。
    /// 超大文档（超过 core 的字节预算）不算，边条自然不显示——那是刻意的成本闸。
    pub change_history_strip: bool,
    /// 第 64 轮 ⑭：保存时备份模式。[`BACKUP_MODE_NONE`]（默认，不备份）/
    /// [`BACKUP_MODE_SIMPLE`]（同目录 `name.bak` 覆盖式）/
    /// [`BACKUP_MODE_TIMESTAMPED`]（`name.bak/` 目录内时间戳历史）。
    /// 非法值在加载时归一为 none。
    pub backup_mode: String,
    /// P62 热键重映射：动作 id → 组合键串（如 `"Ctrl+Shift+S"`）。
    /// 空 = 全部默认。加载时逐条经 [`normalize_combo`] 归一，非法组合
    /// 条目删除；动作 id 的合法性由 app 层过滤（core 不掌握动作清单）。
    pub hotkeys: HashMap<String, String>,
    /// 设置策略版本（P63）：旧配置缺字段 = 0，加载归一时逐级执行
    /// 策略迁移后推进到 [`SETTINGS_VERSION`]。只承载「默认值语义变更」
    /// 的一次性迁移；普通新增字段一律走 serde default 零迁移。
    ///
    /// ⚠️ 本结构里**唯一**保留字段级 `#[serde(default)]` 的一格：容器级默认
    /// 会给出 [`SETTINGS_VERSION`]（新装用户直接落在当前版本、不经历迁移），
    /// 而旧文件缺这个键必须读成 0，否则 P63 那条 v0→v1 梯子永不触发、
    /// 即时保存的默认反转对存量配置失效。
    #[serde(default)]
    pub settings_version: u32,
    /// P102：窗口几何记忆——上次关闭时的窗口位置（逻辑像素，屏幕坐标）。
    /// None = 未记录过（启动回退默认/居中）。只入 config.toml，不在
    /// 设置弹窗展示（它属于窗口自身状态）。
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// P102：窗口尺寸（逻辑像素）。None = 未记录过（启动回退默认 1024×768）。
    pub window_width: Option<f32>,
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
            language: crate::lang::Lang::ZhCn,
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
            // P132：参考线默认开（主流同款），右缘标尺默认关（0）
            indent_guides: true,
            edge_column: 0,
            // P322：粘贴缩进对齐默认关（关着＝与引入前逐字节相同）
            paste_align_indent: false,
            // B12：变更边条默认开（被动显示，同 `indent_guides` 的取舍）
            change_history_strip: true,
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
/// P132：右缘标尺列上限（超宽行对齐到 1000 列已无对齐意义）。
pub const MAX_EDGE_COLUMN: u32 = 1000;

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
pub(crate) const BACKUP_MODES: [&str; 3] = [
    BACKUP_MODE_NONE,
    BACKUP_MODE_SIMPLE,
    BACKUP_MODE_TIMESTAMPED,
];

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
/// P122 增补 `Backspace`（Ctrl+Backspace 删到词首）——与 app 侧
/// combo_string 白名单同步（两侧契约一致，注册表合法性测试把关）。
/// P313 增补 `Plus` / `Minus` / `Equal`（字号缩放的 `Ctrl+=`、`Ctrl+-`、
/// `Ctrl++`）：这三个符号在键盘上不是"一个字符键"那么整齐（`+` 多数布局要按
/// Shift，小键盘又自带 `+`/`-`），故规范形一律用**名字**；`-` `=` 额外接受
/// 符号字面写法（见 [`COMBO_SYMBOL_KEYS`]），`+` 因为本身就是分隔符而只能写名字。
const COMBO_NAMED_KEYS: [&str; 27] = [
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Tab",
    "Insert",
    "Delete",
    "Backspace",
    "Up",
    "Down",
    "Left",
    "Right",
    "Plus",
    "Minus",
    "Equal",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

/// 单个符号 → 规范名（P313）。用户在配置里写 `Ctrl+=` 与写 `Ctrl+Equal`
/// 必须归一到同一串，否则同一个组合能被登记两次而谁都失效。
///
/// ⚠️ 表里**没有** `+`：组合串以 `+` 为分隔符，`Ctrl++` 切出来是
/// `["Ctrl", "", ""]`——那个加号永远成不了"键名"那一段，故字面写法在这套
/// 语法里不可表达，只能写规范名 `Ctrl+Plus`（`combo_string` 上报的也正是它）。
const COMBO_SYMBOL_KEYS: [(&str, &str); 2] = [("-", "Minus"), ("=", "Equal")];

/// 组合键串归一（纯函数可单测）：`ctrl+shift+f` → `Ctrl+Shift+F`。
///
/// 契约（第 60 轮放宽功能键）：
/// - **含 Ctrl** 的组合（原有形态）：键名 = 单个字母/数字或
///   `COMBO_NAMED_KEYS` 白名单（大小写不敏感），修饰键顺序规范化为
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
                    if ch.is_ascii_alphanumeric() {
                        key = Some(ch.to_ascii_uppercase().to_string());
                    } else {
                        // P313：`-` `=` 按规范名收进来（`+` 做不到，见
                        // COMBO_SYMBOL_KEYS 的注记）
                        key = Some(
                            COMBO_SYMBOL_KEYS
                                .iter()
                                .find(|(sym, _)| *sym == token)
                                .map(|(_, name)| (*name).to_string())?,
                        );
                    }
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
        && key[1..]
            .parse::<u8>()
            .map(|n| (1..=12).contains(&n))
            .unwrap_or(false);
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

/// 读配置的结果分档（P355）。形状照 [`crate::snapshot::ManifestRead`]。
///
/// 为什么要分档：旧形状是 `fs::read_to_string(path).unwrap_or_default()`——
/// 「文件不存在」与「文件在却读不出」（非 UTF-8／被占用／权限）被收敛成同一个
/// 空串，于是主题／语言／热键／最近文件／窗口几何**一起**回默认，而窗口几何在
/// 本次启动就会把它写回磁盘＝把这次丢失固化。第 257 轮在会话清单侧已经三分过
/// （P348），设置侧是同一件事剩下的那一半。
#[derive(Debug, Clone)]
pub enum SettingsRead {
    /// 没有 config.toml：首次启动，安静用默认值就对了（不该打扰用户）。
    Absent,
    /// 有文件却读不出。`error` 是给人看的原因；`original` 是磁盘上的原文——
    /// 连字节都没读到（占用／权限）时为 `None`，调用方据此决定"留档"还是"只报位置"。
    Broken {
        error: String,
        original: Option<Vec<u8>>,
    },
    /// 读得出的偏好。个别坏字段按 R-15 逐键回默认，那**不算** Broken（不算"没了"）。
    /// 装箱只为让三档的尺寸差留在一次指针跳转内（`clippy::large_enum_variant`），
    /// 代价是 boot 期一次堆分配——这份枚举整个生命周期只构造一次。
    Good(Box<Settings>),
}

impl SettingsRead {
    /// 取出偏好：`Good` 用读到的那一份，`Absent`／`Broken` 回规范默认值并归一——
    /// 与 [`Settings::load_from`] 改前的契约逐字相同（分档只**加**信息，不改回落行为）。
    pub fn into_settings(self) -> Settings {
        match self {
            SettingsRead::Good(settings) => *settings,
            SettingsRead::Absent | SettingsRead::Broken { .. } => {
                let mut settings = Settings::default();
                settings.normalize();
                settings
            }
        }
    }
}

impl Settings {
    pub fn load_from(path: &Path) -> Self {
        Self::load_from_checked(path).into_settings()
    }

    /// [`Self::load_from`] 的**分档版**（P355）：把「没有配置文件」与「文件在却读不出」
    /// 分开。读侧仍不动磁盘上那份 `config.toml`。
    pub fn load_from_checked(path: &Path) -> SettingsRead {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return SettingsRead::Absent;
            }
            Err(source) => {
                return SettingsRead::Broken {
                    error: source.to_string(),
                    original: None,
                };
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(source) => {
                // 原文一个字节都没丢掉才能救得回来：整份字节交给调用方留档
                return SettingsRead::Broken {
                    error: format!("配置文件不是合法 UTF-8 文本（{source}）"),
                    original: Some(bytes),
                };
            }
        };
        // 只有**整份**读不通才算 Broken：单个坏字段已由 [`Self::parse_lenient`]
        // 逐键回默认（R-15），那不是"偏好没了"。判据＝连 `toml::Table` 都解不出
        // ＝语法坏，一个键都没取回。空文件不在此列（所有字段都有默认值，照旧静默）。
        if !text.trim().is_empty() {
            if let Err(source) = toml::from_str::<toml::Table>(text) {
                return SettingsRead::Broken {
                    error: source.to_string(),
                    original: Some(bytes),
                };
            }
        }
        let mut settings = Self::parse_lenient(text);
        settings.normalize();
        SettingsRead::Good(Box::new(settings))
    }

    /// 解析偏好文件：**一个坏字段不许连坐其余偏好**。
    ///
    /// 旧形状是 `toml::from_str(&s).ok().unwrap_or_default()`：某个字段类型
    /// 不符（手改 config.toml、或旧版本里同名字段形状不同）时 serde 把
    /// **整份**文件判失败 ⇒ 主题／语言／键位／最近文件／窗口几何一起回默认，
    /// 而窗口几何每次启动自动落盘＝把这一次丢失固化进磁盘。
    ///
    /// 做法＝先整份试解析（正常文件仍只付一次解析成本）；失败才退到「逐键
    /// 累加，加上这一键后解析不过就丢掉这一键」。字段的合法类型仍只由
    /// derive 出来的 `Deserialize` 定义一处——这里不维护第二份字段清单，
    /// 将来新增字段不会在恢复路径上漏登记。
    fn parse_lenient(text: &str) -> Self {
        if let Ok(settings) = toml::from_str::<Self>(text) {
            return settings;
        }
        let Ok(table) = toml::from_str::<toml::Table>(text) else {
            // 语法本身坏了：没有任何键能可信地取回，回默认（旧行为）
            return Self::default();
        };
        let mut kept = toml::Table::new();
        for (key, value) in table {
            let mut probe = kept.clone();
            probe.insert(key, value);
            // 加上这一键后整份仍读得通才留它，否则只有这一键回默认
            if toml::Value::Table(probe.clone()).try_into::<Self>().is_ok() {
                kept = probe;
            }
        }
        // kept 每加一键都验证过 ⇒ 这里必然成功，default 只是不再解一次的兜底
        toml::Value::Table(kept)
            .try_into::<Self>()
            .unwrap_or_default()
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
        // P154/P155：界面语言（`Lang` 的反序列化已把未知短码归一为默认语言，
        // 此处无需再归一；保留注释说明归一发生在类型层）
        // clamp 对 NaN 会原样返回 NaN，先排除非有限值。
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            DEFAULT_FONT_SIZE
        };
        // P102 窗口尺寸同上一条：非有限或非正一律丢弃为「未记录」（启动
        // 回默认 1024×768）。留着 NaN 不只是几何错位——TOML 不支持非有限
        // 浮点，它会让之后任意一次 save() 的**整份清单**序列化失败。
        self.window_width = self.window_width.filter(|w| w.is_finite() && *w > 0.0);
        self.window_height = self.window_height.filter(|h| h.is_finite() && *h > 0.0);
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
        // P132：右缘标尺列钳到合法区间（0 = 关闭）
        self.edge_column = self.edge_column.min(MAX_EDGE_COLUMN);
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
        self.hotkeys
            .retain(|_, combo| match normalize_combo(combo) {
                Some(canonical) => {
                    *combo = canonical;
                    true
                }
                None => false,
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

    /// P154/P155：设置界面语言。
    ///
    /// 入参仍是**字符串短码**（`"en"` / `"zh-CN"` / 宽松变体），由
    /// [`normalize_language`] 归一后落成 [`crate::lang::Lang`]——保留字符串
    /// 入口是为了既有的调用点与测试不必强绑枚举字面量。
    pub fn set_language(&mut self, language: &str) {
        self.language = normalize_language(language);
    }

    /// 原子保存（P7）：走同 crate 的 write_atomic（临时文件 + rename + sync）。
    /// 直接 `fs::write` 会在崩溃/掉电时留下半截 config.toml，
    /// 解析失败后静默回默认 → 最近文件列表无声丢失，这里从根上杜绝。
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // 序列化失败必须上抛，**不能**退化成「写一份空配置出去」：
        // toml 不接受非有限浮点（`window_width = nan` 是合法 TOML 字面量，
        // 加载后随任意一次 save() 让整个清单序列化失败），旧实现的
        // `unwrap_or_default()` 会把这条失败变成一次静默的原子覆盖，
        // 最近文件/主题/热键全没。失败时目标文件原封不动才是正确语义。
        let serialized = toml::to_string_pretty(self).map_err(|source| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("设置序列化失败，已放弃写入以保护现有配置：{source}"),
            )
        })?;
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

    /// 尽力保存：拿不到数据目录时什么都不做并返回 `Ok`（功能自动降级）。
    ///
    /// P347（池 ⑩）：写盘失败**必须回传**给调用方。旧形状是 `let _ = ...`，
    /// 于是"设置没记住"这件事在 UI 上零痕迹——core 不该替界面决定"要不要
    /// 打断用户"，它只负责别把错误吞掉。
    pub fn save(&self) -> std::io::Result<()> {
        match Self::config_path() {
            Some(p) => self.save_to(&p),
            None => Ok(()),
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

    /// 池 ⑨ 的探针（先量后果）：配置里**一个**字段类型写错时，其余偏好
    /// 到底还留不留。这份文件模拟「用户真的用过」：主题、语言、防抖秒数、
    /// 最近文件全偏离默认，只有 `font_size` 被写成字符串。
    #[test]
    fn one_bad_field_does_not_reset_the_whole_preference_file() {
        let dir = scratch_dir("p346-one-bad");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        // settings_version = 1：绕开 P63 那条 v0→v1 迁移（它会把
        // autosave_enabled 强制打回 false），别让迁移干扰本轮要量的那一格
        fs::write(
            &path,
            "settings_version = 1\n\
             theme = \"dark\"\n\
             language = \"en\"\n\
             font_size = \"big\"\n\
             autosave_enabled = true\n\
             autosave_delay_secs = 7\n\
             recent_files = [\"C:/a.txt\"]\n",
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.font_size, DEFAULT_FONT_SIZE, "坏字段自己回默认");
        assert_eq!(loaded.theme, THEME_DARK, "主题不该被别字段连坐");
        assert_eq!(loaded.language, crate::lang::Lang::En, "语言同理");
        assert!(loaded.autosave_enabled, "即时保存开关同理");
        assert_eq!(loaded.autosave_delay_secs, 7, "防抖秒数同理");
        assert_eq!(
            loaded.recent_files,
            vec!["C:/a.txt".to_owned()],
            "最近文件同理"
        );

        // 放大器那一半（同一主张的第二层，不另立用例）：偏好一旦连坐回默认，
        // 下一次自动落盘（窗口几何每次都写这份 config）就把这次丢失**固化**
        // 进磁盘，重启再也找不回来
        loaded.save_to(&path).unwrap();
        assert_eq!(
            Settings::load_from(&path).theme,
            THEME_DARK,
            "回写不得把连坐后的默认固化下来"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// 上面那条只放一个坏键。若恢复路径做成「剔掉第一个坏键后重试一次就收手」，
    /// 第二个坏键仍会连坐其余偏好——这条专门咬那种半吊子循环。
    #[test]
    fn several_bad_fields_lose_only_themselves() {
        let dir = scratch_dir("p346-several-bad");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "settings_version = 1\n\
             theme = \"dark\"\n\
             font_size = \"big\"\n\
             autosave_delay_secs = \"seven\"\n\
             edge_column = 80\n\
             recent_files = \"C:/a.txt\"\n",
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.theme, THEME_DARK, "合法键一个都不许丢");
        assert_eq!(loaded.edge_column, 80, "合法键一个都不许丢");
        assert_eq!(
            loaded.font_size, DEFAULT_FONT_SIZE,
            "类型写错的那一键自己回默认"
        );
        assert_eq!(
            loaded.autosave_delay_secs,
            Settings::default().autosave_delay_secs,
            "缺这一键时回手写 Default 的值（默认值只有一个来源）"
        );
        assert!(loaded.recent_files.is_empty(), "该是数组却写了字符串");

        fs::remove_dir_all(&dir).ok();
    }

    /// 恢复路径**不**去 salvage 语法本身坏掉的文件：那时任何键的取值都不可信，
    /// 回默认是旧行为。这条钉住「坏语法」与「坏字段」两档没被混成半读。
    #[test]
    fn broken_toml_syntax_still_falls_back_to_default() {
        let dir = scratch_dir("p346-syntax");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\nrecent_files = [\n").unwrap();

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
        assert_eq!(
            s.recent_files
                .iter()
                .filter(|p| **p == "C:/f/5.txt")
                .count(),
            1
        );
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = scratch_dir("roundtrip");
        let path = dir.join("nested").join("config.toml");

        let mut s = Settings::default();
        s.push_recent(Path::new("D:/笔记/中文 文件名.md"));
        s.save_to(&path).expect("保存应成功");

        let loaded = Settings::load_from(&path);
        assert_eq!(
            loaded.recent_files,
            vec!["D:/笔记/中文 文件名.md".to_string()]
        );

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
        // B12：新字段走 serde default ⇒ 旧配置缺字段即"默认开"，不需要迁移
        assert!(
            loaded.change_history_strip,
            "旧配置缺 change_history_strip 字段时必须落到默认值（开）"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// ⚠️ 诚实记账：体检报告里「NaN 让整份设置序列化失败 → 被
    /// `unwrap_or_default()` 空覆盖」这条链**实测不成立**——toml 0.8 会把
    /// `f32::NAN` 原样序列化成 `nan` 而不报错（下面那条 round-trip 用例即
    /// 证据：默认设置里塞 NaN 宽度照样保存成功）。所以 `save_to` 的改法修的是
    /// **错误处理形状**（真失败时不得清空用户配置），**不配「先红后绿」的敏感
    /// 测试**——造不出失败输入，硬写一个两头都绿的断言只会冒充覆盖。有实际
    /// 效果的是几何值归一，见 `non_finite_window_geometry_is_dropped_on_load_
    /// not_carried_into_save`。
    #[test]
    fn nan_geometry_serializes_so_load_side_must_sanitize_it() {
        // 反证报告假设：nan 不会让保存失败——正因如此，毒值会在配置里长期
        // 往返，消毒只能发生在加载侧（见下一条用例）。
        let dir = scratch_dir("nan-serializes");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut s = Settings::default();
        s.window_width = Some(f32::NAN);
        s.save_to(&path).expect("当前 toml 版本不拒绝 nan");
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("window_width = nan"),
            "实测：nan 被原样序列化写出，实际内容 {:?}",
            raw.lines().find(|l| l.contains("window_width"))
        );
        // 而读回后经归一必须丢掉它（app 侧拿到的是「未记录」而非 NaN）
        assert_eq!(Settings::load_from(&path).window_width, None);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_finite_window_geometry_is_dropped_on_load_not_carried_into_save() {
        // TOML 里 `window_width = nan` 是合法字面量 → 反序列化得到 Some(NaN)。
        // 归一化必须就地丢弃，否则它随任意一次 save() 触发上一条失败路径。
        let dir = scratch_dir("geo-nan");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "theme = \"dark\"\nwindow_width = nan\nwindow_height = inf\n",
        )
        .unwrap();

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.theme, "dark", "其余字段不受牵连");
        assert_eq!(loaded.window_width, None, "NaN 宽度应丢弃（回未记录）");
        assert_eq!(loaded.window_height, None, "inf 高度同理");
        // 归一之后配置必须能正常回写（不再有毒值）
        assert!(loaded.save_to(&path).is_ok(), "归一后 save_to 应成功");
        assert!(fs::read_to_string(&path).unwrap().contains("theme"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn indent_guides_and_edge_column_defaults_roundtrip_and_clamp() {
        // P132：默认 = 参考线开（主流同款）、标尺关
        let d = Settings::default();
        assert!(d.indent_guides);
        assert_eq!(d.edge_column, 0);

        // serde 零迁移 roundtrip：缺字段落默认，有值原样保留
        let dir = scratch_dir("p132-roundtrip");
        let path = dir.join("config.toml");
        let mut s = Settings::default();
        s.indent_guides = false;
        s.edge_column = 80;
        s.save_to(&path).expect("保存应成功");
        let loaded = Settings::load_from(&path);
        assert!(!loaded.indent_guides);
        assert_eq!(loaded.edge_column, 80);

        // 手改超限值加载归一钳到上限
        fs::write(
            &path,
            "indent_guides = true\nedge_column = 5000\nrecent_files = []",
        )
        .unwrap();
        let clamped = Settings::load_from(&path);
        assert_eq!(clamped.edge_column, MAX_EDGE_COLUMN);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_theme_and_font_size_are_normalized_on_load() {
        let dir = scratch_dir("invalid");
        fs::create_dir_all(&dir).unwrap();

        // 越界 + 非法主题 → 归一为 "light" / 上界（P116 起 48.0）
        let over = dir.join("over.toml");
        fs::write(&over, "theme = \"blue\"\nfont_size = 99\nrecent_files = []").unwrap();
        let loaded = Settings::load_from(&over);
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.font_size, MAX_FONT_SIZE);

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
        assert!(
            !s.autosave_enabled,
            "P63 起默认关闭：原文件只在显式保存时被写"
        );
        assert_eq!(
            s.settings_version, SETTINGS_VERSION,
            "新装用户直接落在当前策略版本"
        );
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
        fs::write(
            &legacy,
            "autosave_enabled = true\nautosave_delay_secs = 5\n",
        )
        .unwrap();
        let migrated = Settings::load_from(&legacy);
        assert!(
            !migrated.autosave_enabled,
            "v0 持久化的 true 必须被一次性重置"
        );
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

    // ---------- P352 缺字段的取值只许有一个来源 ----------

    /// P355（第 261 轮扫描池 ③）：「没有配置文件」与「文件在却读不出」必须分开。
    ///
    /// 旧形状是 `fs::read_to_string(path).unwrap_or_default()`，两者收敛成同一个空串
    /// ⇒ 主题／语言／热键／最近文件／窗口几何一起回默认，而窗口几何在本次启动就把它
    /// 写回磁盘＝**把丢失固化**，界面上零痕迹。会话清单侧第 257 轮已三分（P348），
    /// 本轮补的是同一件事剩下的那一半。
    #[test]
    fn unreadable_config_is_told_apart_from_absent_one() {
        let dir = scratch_dir("p355-read-kinds");
        fs::create_dir_all(&dir).unwrap();

        // 正对照一：文件不存在 = Absent（首次启动不该被当成故障）
        let missing = dir.join("nope.toml");
        assert!(matches!(
            Settings::load_from_checked(&missing),
            SettingsRead::Absent
        ));

        // 非 UTF-8：读不出，但**原文一个字节都不该丢**——那是唯一能救回来的东西
        let mut raw = b"theme = \"dark\"\nfont_size = 18\n".to_vec();
        raw.push(0xFF);
        let unreadable = dir.join("config.toml");
        fs::write(&unreadable, &raw).unwrap();
        match Settings::load_from_checked(&unreadable) {
            SettingsRead::Broken { error, original } => {
                assert!(!error.is_empty(), "必须带上为什么读不出来");
                assert_eq!(
                    original.as_deref(),
                    Some(raw.as_slice()),
                    "原文必须逐字节交回调用方"
                );
            }
            other => panic!("非 UTF-8 的配置必须归 Broken，实际 {other:?}"),
        }
        // 门面契约不许变：读不出仍是规范默认值——本轮只**加**信息，不改回落行为
        assert_eq!(Settings::load_from(&unreadable), Settings::default());

        // 语法整份坏（文本读到了、一个键都取不回）：同样归 Broken，也带原文
        let bad_syntax = dir.join("bad.toml");
        fs::write(&bad_syntax, "theme = \n").unwrap();
        match Settings::load_from_checked(&bad_syntax) {
            SettingsRead::Broken { original, .. } => assert_eq!(
                original.as_deref(),
                Some("theme = \n".as_bytes()),
                "语法坏那份原文也要留住"
            ),
            other => panic!("语法坏的配置必须归 Broken，实际 {other:?}"),
        }

        // 正对照二：读得出来的归 Good 且值真进来了（不是"逢启动就报坏"的过度报警）
        let good = dir.join("good.toml");
        fs::write(&good, "theme = \"dark\"\n").unwrap();
        match Settings::load_from_checked(&good) {
            SettingsRead::Good(settings) => assert_eq!(settings.theme, THEME_DARK),
            other => panic!("合法配置必须归 Good，实际 {other:?}"),
        }

        // 半坏那一格仍是 Good：一个坏字段只让自己回默认（R-15），那不叫"没了"
        let half = dir.join("half.toml");
        fs::write(&half, "theme = \"dark\"\nfont_size = \"big\"\n").unwrap();
        assert!(
            matches!(Settings::load_from_checked(&half), SettingsRead::Good(_)),
            "个别坏字段不该升级成整份故障"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_field_missing_from_config_takes_the_documented_default() {
        // 「手写 Default」与「serde 缺字段默认」写的是同一件事，历史上漂过一次：
        // `autosave_enabled` 的文档与 `Default` 都是**关**，serde 那一份仍是
        // `default_true`（P63 只翻了三行代码里的两处）。这里不逐字段列清单
        // （列了就会漏登记，与 R-15 同规），而是拿「除版本号外一个键都没有」的
        // 文件与 `Default` 整体对拍——将来任何一格两份写法不一致都当场红。
        let text = format!("settings_version = {SETTINGS_VERSION}\n");
        assert_eq!(
            Settings::parse_lenient(&text),
            Settings::default(),
            "缺字段必须逐字段等于手写 Default（版本号除外：它缺字段＝旧文件＝v0）"
        );
    }

    #[test]
    fn malformed_autosave_line_does_not_switch_autosave_on() {
        // 后果那一半：version 已是 1 的文件里这一行被手改坏成别的意思，严格解析
        // 失败 ⇒ R-15 的逐键累加丢掉这一键 ⇒ 缺字段默认值上位。旧形状下那等于
        // 「替用户打开即时保存」——他没按 Ctrl+S，原文件也会被防抖写盘。
        let dir = scratch_dir("p352-autosave");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            format!("settings_version = {SETTINGS_VERSION}\ntheme = \"dark\"\nautosave_enabled = \"on\"\n"),
        )
        .unwrap();
        let loaded = Settings::load_from(&path);
        assert!(
            !loaded.autosave_enabled,
            "坏掉的那一格回默认＝关，不许回成「开」"
        );
        // 连坐闸仍要成立：坏这一键不许把别的偏好一起带走
        assert_eq!(loaded.theme, THEME_DARK);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_version_roundtrips_to_disk() {
        // 版本号参与 roundtrip：落盘再读回不漂移
        let dir = scratch_dir("p63-roundtrip");
        let path = dir.join("config.toml");
        let s = Settings::default();
        s.save_to(&path).expect("保存应成功");
        assert_eq!(
            Settings::load_from(&path).settings_version,
            SETTINGS_VERSION
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn autosave_delay_clamped_to_legal_range() {
        let dir = scratch_dir("p18-clamp");
        fs::create_dir_all(&dir).unwrap();

        let zero = dir.join("zero.toml");
        fs::write(&zero, "autosave_enabled = true\nautosave_delay_secs = 0\n").unwrap();
        assert_eq!(
            Settings::load_from(&zero).autosave_delay_secs,
            1,
            "0 秒会变成写盘风暴，收敛到下界"
        );

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
        assert!(
            s.remember_session,
            "会话恢复默认开启（「重开还是上次的界面」）"
        );

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
        assert_eq!(
            Settings::load_from(&dense).snapshot_interval_secs,
            MIN_SNAPSHOT_INTERVAL_SECS
        );

        // 过疏 → 收敛到上界（防丢失窗口过大）
        let sparse = dir.join("sparse.toml");
        fs::write(&sparse, "snapshot_interval_secs = 99999\n").unwrap();
        assert_eq!(
            Settings::load_from(&sparse).snapshot_interval_secs,
            MAX_SNAPSHOT_INTERVAL_SECS
        );

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
            RecentView {
                line: 42,
                col: 7,
                scroll_top: 128.5,
            },
        );
        s.save_to(&path).expect("保存应成功");
        let loaded = Settings::load_from(&path);
        assert_eq!(
            loaded.recent_view(Path::new("C:/old.txt")),
            Some(RecentView {
                line: 42,
                col: 7,
                scroll_top: 128.5
            }),
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
            RecentView {
                line: 1,
                col: 0,
                scroll_top: 0.0
            },
        ));
        assert!(s.recent_views.is_empty());

        // 关闭「记住最近文件」：整体 no-op（路径是隐私数据，P20 口径）
        s.remember_recent_files = false;
        assert!(!s.set_recent_view(
            Path::new("C:/listed.txt"),
            RecentView {
                line: 1,
                col: 0,
                scroll_top: 0.0
            },
        ));
    }

    #[test]
    fn recent_view_updates_in_place_and_reports_changes() {
        let mut s = Settings::default();
        s.push_recent(Path::new("C:/doc.txt"));
        let v1 = RecentView {
            line: 3,
            col: 0,
            scroll_top: 10.0,
        };

        assert!(
            s.set_recent_view(Path::new("C:/doc.txt"), v1),
            "首次记录应报告变化"
        );
        // 相同值再写：无变化（调用方据此跳过落盘）
        assert!(!s.set_recent_view(Path::new("C:/doc.txt"), v1));
        // 值变化：再次报告
        let v2 = RecentView {
            line: 9,
            col: 2,
            scroll_top: 20.0,
        };
        assert!(s.set_recent_view(Path::new("C:/doc.txt"), v2));
        assert_eq!(s.recent_view(Path::new("C:/doc.txt")), Some(v2));

        // 非有限滚动值消毒为 0
        let bad = RecentView {
            line: 1,
            col: 0,
            scroll_top: f32::NAN,
        };
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
                RecentView {
                    line: i as usize,
                    col: 0,
                    scroll_top: 0.0,
                },
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
        assert!(
            s.recent_view(Path::new("C:/f/new.txt")).is_none(),
            "新文件尚无记忆"
        );

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
        assert!(
            loaded.recent_views.is_empty(),
            "关闭开关后记忆必须随加载清空"
        );

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

    // ---------- P154/P155：界面语言 ----------

    #[test]
    fn p154_language_normalization_contract() {
        use crate::lang::Lang;
        // 合法写法（大小写/下划线/区域变体宽松匹配）
        for raw in ["en", "EN", "en-US", "en_us", "En-Gb"] {
            assert_eq!(normalize_language(raw), Lang::En, "{raw:?} 应归一为 en");
        }
        for raw in ["zh", "zh-CN", "zh_cn", "ZH-Hans", "zh-SG"] {
            assert_eq!(
                normalize_language(raw),
                Lang::ZhCn,
                "{raw:?} 应归一为 zh-CN"
            );
        }
        // 未知/空/垃圾值 → 默认中文简体（界面默认语言，不因手改配置而错乱）
        for raw in ["", "  ", "fr", "日本語", "xx-YY", "en-US-x-private"] {
            assert_eq!(
                normalize_language(raw),
                Lang::ZhCn,
                "{raw:?} 未知值应回落默认中文简体"
            );
        }
    }

    #[test]
    fn p154_language_defaults_roundtrips_and_normalizes_on_load() {
        use crate::lang::Lang;
        let s = Settings::default();
        assert_eq!(s.language, Lang::ZhCn, "默认界面语言 = 中文简体");

        let dir = scratch_dir("p154-language");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // 旧配置缺字段 → 默认中文（serde default，零迁移）
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        assert_eq!(Settings::load_from(&path).language, Lang::ZhCn);

        // P155：坏值不阻断整体读取——语言字段回落默认，其余字段照常生效
        fs::write(&path, "language = \"fr\"\nfont_size = 18\n").unwrap();
        let broken = Settings::load_from(&path);
        assert_eq!(broken.language, Lang::ZhCn);
        assert_eq!(broken.font_size, 18.0, "未知语言不得拖垮其余字段");

        // 落盘往返
        let mut s = Settings::default();
        s.set_language(LANG_EN);
        assert_eq!(s.language, Lang::En);
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).language, Lang::En);

        // 手改非法值 → 加载回落默认（不报错、不拖垮整份配置）
        fs::write(&path, "language = \"klingon\"\n").unwrap();
        assert_eq!(Settings::load_from(&path).language, Lang::ZhCn);

        // setter 同样归一
        let mut s = Settings::default();
        s.set_language("en_US");
        assert_eq!(s.language, Lang::En, "setter 应归一为 en");
        s.set_language("bogus");
        assert_eq!(s.language, Lang::ZhCn);

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
            (
                "long",
                &format!(
                    "font_family = \"{}\"\n",
                    "F".repeat(MAX_FONT_FAMILY_LEN + 1)
                ),
            ),
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
        assert_eq!(
            normalize_settings_page(" no-such "),
            SETTINGS_PAGE_APPEARANCE
        );
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
        assert_eq!(
            normalize_combo("CTRL + shift + f"),
            Some("Ctrl+Shift+F".to_owned())
        );
        assert_eq!(normalize_combo("Ctrl+Home"), Some("Ctrl+Home".to_owned()));
        // P313：三个符号键——符号写法与规范名写法必须归一到同一串，
        // 否则同一个组合能被登记两次而两边都失效
        assert_eq!(normalize_combo("ctrl+="), Some("Ctrl+Equal".to_owned()));
        assert_eq!(normalize_combo("Ctrl+Equal"), Some("Ctrl+Equal".to_owned()));
        assert_eq!(
            normalize_combo("Ctrl+Shift+Plus"),
            Some("Ctrl+Shift+Plus".to_owned())
        );
        // 字面 `Ctrl++` 不可表达：`+` 就是分隔符，切出来没有"键名"那一段
        assert_eq!(normalize_combo("Ctrl++"), None);
        assert_eq!(normalize_combo("Ctrl+-"), Some("Ctrl+Minus".to_owned()));
        assert_eq!(
            normalize_combo("ctrl+shift+minus"),
            Some("Ctrl+Shift+Minus".to_owned())
        );
        // 其余符号仍一律拒绝（放行一个就多吞一次输入）
        assert_eq!(normalize_combo("Ctrl+%"), None);
        assert_eq!(normalize_combo("Ctrl+;"), None);
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
        s.hotkeys
            .insert("save".to_owned(), "ctrl+shift+s".to_owned());
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
