//! 应用设置持久化：最近打开的文件列表。
//!
//! 存储位置由 `dirs::config_dir()` 决定（Windows 上是
//! `%APPDATA%\editpad\config.toml`）。读写都是尽力而为：
//! 配置损坏或目录不可写时静默回退默认值，绝不影响编辑器本体。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最近文件列表上限。
pub const MAX_RECENT_FILES: usize = 10;

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
}

// 手写 Default 而非 derive：f32/String 的派生默认值（0.0 / ""）不是合法偏好，
// 必须落到规范默认值（"light" / 16.0）。
impl Default for Settings {
    fn default() -> Self {
        Self {
            recent_files: Vec::new(),
            theme: THEME_LIGHT.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            remember_recent_files: true,
            autosave_enabled: true,
            autosave_delay_secs: DEFAULT_AUTOSAVE_DELAY_SECS,
        }
    }
}

/// 自动保存防抖默认秒数。
pub const DEFAULT_AUTOSAVE_DELAY_SECS: u32 = 2;
/// 防抖秒数允许范围（闭区间），越界值加载时被 clamp。
pub const MIN_AUTOSAVE_DELAY_SECS: u32 = 1;
pub const MAX_AUTOSAVE_DELAY_SECS: u32 = 60;

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
        if !self.remember_recent_files && !self.recent_files.is_empty() {
            self.recent_files.clear();
        }
        // P18：防抖秒数收敛到合法区间（0 秒会变成每秒写盘风暴）
        self.autosave_delay_secs = self
            .autosave_delay_secs
            .clamp(MIN_AUTOSAVE_DELAY_SECS, MAX_AUTOSAVE_DELAY_SECS);
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
    }

    /// 清空最近文件列表（P20「清空记录」按钮）。
    /// 只改内存；调用方随后 `save()` 才会从 config.toml 抹掉痕迹。
    pub fn clear_recent_files(&mut self) {
        self.recent_files.clear();
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
}
