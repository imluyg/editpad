//! 应用设置持久化：最近打开的文件列表。
//!
//! 存储位置由 `dirs::config_dir()` 决定（Windows 上是
//! `%APPDATA%\editpad\config.toml`）。读写都是尽力而为：
//! 配置损坏或目录不可写时静默回退默认值，绝不影响编辑器本体。

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
    /// 即时保存开关（P18）。用户点名要此功能，默认开启；
    /// 编辑停手超过 [`Self::autosave_delay_secs`] 秒自动落盘。
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
}

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
            autosave_enabled: true,
            autosave_delay_secs: DEFAULT_AUTOSAVE_DELAY_SECS,
            enable_snapshots: true,
            exit_mode: EXIT_MODE_SNAPSHOT.to_string(),
            remember_session: true,
            snapshot_interval_secs: DEFAULT_SNAPSHOT_INTERVAL_SECS,
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
        // P31：心跳间隔收敛到合法区间（过密=写盘风暴，过疏=丢失窗口过大）
        self.snapshot_interval_secs = self
            .snapshot_interval_secs
            .clamp(MIN_SNAPSHOT_INTERVAL_SECS, MAX_SNAPSHOT_INTERVAL_SECS);
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

    /// 默认配置文件路径；拿不到系统配置目录时返回 None（功能自动降级）。
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("editpad").join("config.toml"))
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

    // ---------- P18 即时保存设置 ----------

    #[test]
    fn autosave_defaults_on_with_two_second_delay() {
        let s = Settings::default();
        assert!(s.autosave_enabled, "用户点名要即时保存，默认必须开启");
        assert_eq!(s.autosave_delay_secs, 2);

        // 旧配置缺 P18 字段 → 同样得到默认值
        let dir = scratch_dir("p18-legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "theme = \"dark\"\n").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(loaded.autosave_enabled);
        assert_eq!(loaded.autosave_delay_secs, 2);
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
}
