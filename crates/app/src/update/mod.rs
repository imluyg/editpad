//! 消息分发与状态变更（P157 自单文件 `update.rs` 拆为目录：纯移动零行为变更）。
//!
//! 本文件保留：
//! * [`Message`] 总入口 [`Editpad::update`] 与按域分派；
//! * 标签/页的核心状态管理——新建、切换、脏标记、非空不变量收口
//!   （[`Editpad::ensure_nonempty_tabs`]）、待关闭状态机；
//! * 应用生命周期：`new` / CLI 续排 / `subscription` / 语言与字体应用 /
//!   设置与窗口几何落盘；
//! * 平台相关自由函数（资源管理器定位、外部打开）与 `tick_stream`。
//!
//! 分域 impl 见子模块：
//! * `edit`——编辑内核入口（按键/编辑操作分发、编辑总闸、列编辑器、热键捕获）
//! * `tabs`——标签开关与批量关闭
//! * `file`——打开/加载/保存/另存为/重命名、最近文件、自动保存、外部修改巡检
//! * `find`——查找/替换、查找全部、在文件中查找
//! * `settings`——设置弹窗、界面语言与字体切换
//!
//! 约定：子模块里的私有方法经 `pub(super)` 跨文件可见；共享导入在本文件
//! **`pub(crate)` 再导出**，否则子模块的 `use super::*` 拿不到（私有 `use`
//! 不跨模块传递）——与 `editor/core.rs` 的 Phase 2c 枢纽同款做法。

use super::settings_ui::SettingsPage;

pub(crate) use super::*;

// 按域拆分的 impl 块（P157：纯移动零行为变更；私有方法经 pub(super) 跨文件可见）
mod edit;
mod tabs;
mod file;
mod find;
mod settings;

// ---------- P149：周期节拍订阅（OS 线程独占睡眠 + async channel 桥接） ----------

/// 节拍种类：既描述间隔与产出消息，也充当订阅身份（`run_with` 的 data，
/// 内容变化即重键 → 旧流撤销、新流按新参数启动）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TickKind {
    /// 光标闪烁 / 滚动条淡出快拍（间隔随淡出态切换）
    Caret { interval_ms: u64 },
    /// 周期快照心跳（间隔 = 设置值，5~120s）
    Heartbeat { interval_secs: u32 },
    /// 文件监视巡检（固定 2s）
    Monitor,
    /// 单实例转发握手轮询（固定 400ms）
    PendingOpen,
}

type TickStream = std::pin::Pin<Box<dyn iced::futures::Stream<Item = Message> + Send>>;

/// 节拍流（`run_with` 的 builder 需要 fn 指针 → 返回具名 boxed 流）。
///
/// 桥接线程独占 `thread::sleep`（自有 OS 线程，随便睡），经 async channel
/// `try_send` 投递；执行器 worker 只 `pending().await` 正确 async 停车，
/// 不再被整段睡眠占用。曾用 `Task::perform(async { thread::sleep })` 自我
/// 续期：caret/心跳/监视/转发轮询四条链常驻占死线程池 worker（futures
/// ThreadPool 规模 ≈ 核数），与加载/查找扫描共用时极端情形饿死表现为
/// 假死。注：`iced::time::every` 在本工程 feature 组合（thread-pool，无
/// smol/tokio）下是空实现，离线环境也无法引入 smol 依赖——故手写同语义
/// 桥接。缓冲写满丢拍不断链；订阅撤销后 try_send 失败，桥接线程自然退出。
pub(crate) fn tick_stream(kind: &TickKind) -> TickStream {
    let (interval, message) = match *kind {
        TickKind::Caret { interval_ms } => (
            std::time::Duration::from_millis(interval_ms),
            (|| Message::CaretTick) as fn() -> Message,
        ),
        TickKind::Heartbeat { interval_secs } => (
            std::time::Duration::from_secs(u64::from(interval_secs)),
            (|| Message::SnapshotHeartbeatTick) as fn() -> Message,
        ),
        TickKind::Monitor => (
            std::time::Duration::from_secs(2),
            (|| Message::MonitorTick) as fn() -> Message,
        ),
        TickKind::PendingOpen => (
            std::time::Duration::from_millis(400),
            (|| Message::PendingOpenTick) as fn() -> Message,
        ),
    };
    Box::pin(stream::channel(
        4,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            std::thread::spawn(move || loop {
                std::thread::sleep(interval);
                match output.try_send(message()) {
                    Ok(()) => {}
                    // 丢拍不断链（UI 停顿瞬间缓冲写满）
                    Err(e) if e.is_full() => {}
                    // 订阅已撤销：线程自然收敛
                    Err(_) => break,
                }
            });
            std::future::pending::<()>().await
        },
    ))
}

impl Editpad {
    // ---------- 多标签访问器（P21） ----------

    /// 新建标签页的唯一出口：`Tab::empty()` + 套用全局字号设置。
    ///
    /// 历史教训（P43）：曾有多处直接用 `Tab::empty()` 建页——`EditorCore`
    /// 默认字号恒为 16，恢复会话/新建页/关光补页全部脱离设置里的字号，
    /// 表现为「调到 24 生效、重启后字体回到原大小，但设置面板仍显示 24」
    /// （恢复路径整体替换 tabs，boot 仅对首个默认页应用过字号）。
    /// 今后凡「新建页」一律走这里；boot 首帧仍显式 set_font_size 兜底
    /// （fresh_tab 幂等，重复应用无害）。
    pub(crate) fn fresh_tab(&mut self) -> Tab {
        let tab = Tab::empty();
        // 设置里的字号可能未归一（旧配置/手改），与 boot 同一 clamp 规则
        tab.editor
            .borrow_mut()
            .set_font_size(editor::normalize_font_size(self.settings.font_size));
        // 第 64 轮：不可见字符标记与字号同口径下发（fresh_tab 幂等）
        tab.editor.borrow_mut().set_invisibles(
            self.settings.show_whitespace,
            self.settings.show_line_endings,
        );
        // 第 73 轮 ⑯：自动换行（软换行）同口径下发（fresh_tab 幂等）
        tab.editor
            .borrow_mut()
            .set_word_wrap(self.settings.word_wrap);
        // P132：缩进参考线 / 右缘标尺同口径下发（fresh_tab 幂等）
        tab.editor
            .borrow_mut()
            .set_indent_guides(self.settings.indent_guides);
        tab.editor
            .borrow_mut()
            .set_edge_column(self.settings.edge_column);
        // P154：行号栏字体族与字号同口径下发（fresh_tab 幂等；漏下发会让
        // 新页行号回落到正文字体，与既有页观感不一致）
        tab.editor
            .borrow_mut()
            .set_gutter_font(self.gutter_font_family.map(fonts::family_font));
        tab
    }

    /// 进入 busy（对话框/IO 互斥）。P39 浮层化后的新约束：同时收起
    /// 右键菜单浮层——它的透明背板会挡住整窗点击，busy 期间不能留它挡道
    /// （旧内嵌面板无此问题，故此前各 busy 置位点都无需理会菜单态）。
    pub(crate) fn enter_busy(&mut self) {
        self.busy = true;
        self.tab_context_menu = None;
        // P67：状态栏弹出菜单同样会被 busy 互斥挡住，直接收起
        self.encoding_menu = false;
        self.eol_menu = false;
    }

    /// 当前激活标签页。
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// 当前激活标签页（可变）。
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        let i = self.active_tab;
        &mut self.tabs[i]
    }

    /// 活动页句柄的长期引用（跨语句借用走 `cur_handle` 字段；
    /// 单表达式内的临时借用也可用 [`Self::cur`]）。
    fn cur(&self) -> EditorHandle {
        self.cur_handle.clone()
    }

    /// 为第 `idx` 页分配未命名序号。P25 原为「全局单调、不复用」——用户
    /// 复报关一个未命名页序号就爬一级（关 3 出 4、关 4 出 5），改为
    /// **最小空闲复用**（P168）：取未被任何打开页占用的最小正整数。
    /// 同名歧义防护不变：同号页在打开页集合内唯一；未命名草稿的会话
    /// 快照按代次+页下标命名（snapshot::page_file_name），磁盘上不存在
    /// 「未命名N」文件，复用号码无落盘冲突。`untitled_next` 仍随快照
    /// 维护（格式兼容），赋号不再消费。
    fn assign_untitled_num(&mut self, idx: usize) {
        let used: std::collections::HashSet<u64> =
            self.tabs.iter().filter_map(|t| t.untitled_num).collect();
        let n = (1u64..).find(|n| !used.contains(n)).unwrap_or(1);
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.untitled_num = Some(n);
        }
    }

    /// 保证标签页集合非空：删页后若已空，补一个干净未命名页。
    /// 返回补出来的新页下标（原本非空时为 `None`）。
    ///
    /// ## 不变量：`tabs` 恒非空
    /// `set_active_tab` / `refresh_cur_handle` 用 `len() - 1` 钳制，
    /// `SwitchTabNext` / `SwitchTabPrev` 用 `% len()` 回绕——空集会
    /// 分别导致**下溢**与**除零**。此前由三处删页点各自手写
    /// `if self.tabs.is_empty() { … }` 守卫维护，属重复实现：新增第四处
    /// 删页一旦漏写即 panic，且编译器无从提示。现统一收口到本函数，
    /// 三处删页点必须都走它（回归测试 `tabs_stay_nonempty_after_closing_last`
    /// 钉住该契约）。
    pub(crate) fn ensure_nonempty_tabs(&mut self) -> Option<usize> {
        if !self.tabs.is_empty() {
            return None;
        }
        let tab = self.fresh_tab();
        self.tabs.push(tab);
        Some(self.tabs.len() - 1)
    }

    /// 切换活动页并同步长期别名（所有 active_tab 变更必须经此或
    /// [`Self::refresh_cur_handle`])。
    pub(crate) fn set_active_tab(&mut self, idx: usize) {
        self.active_tab = idx.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 与 tabs 对齐刷新别名（增删页后调用）。
    pub(crate) fn refresh_cur_handle(&mut self) {
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.cur_handle = self.tabs[self.active_tab].editor.clone();
    }

    /// 任一标签页有未保存改动（窗口关闭确认的聚合口径）。
    pub(crate) fn any_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.dirty)
    }

    /// 第 `idx` 页的路径（存在该页时）。
    fn path_of_tab(&self, idx: usize) -> Option<PathBuf> {
        self.tabs.get(idx).and_then(|t| t.path.clone())
    }

    /// 第 64 轮：命名页关闭时进「上次关闭」记忆栈（会话内，最近期在前；
    /// 连续重复只保最近一位），容量 10 截断。单页/批量两条移除漏斗
    /// 都要在移除前调用。
    fn remember_closed_tab(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get(idx) {
            if let Some(p) = &tab.path {
                if self.closed_stack.first().map(|f| f == p).unwrap_or(false) {
                    return;
                }
                self.closed_stack.insert(0, p.clone());
                self.closed_stack.truncate(10);
            }
        }
    }

    /// P146：置位「清单过期」并推进代次。心跳派发时把代次随载荷带走，
    /// 成功回报只在代次未变时才清标记——在途心跳期间的结构变化（关页）
    /// 不会被人写出的旧清单「洗白」。
    fn touch_manifest_stale(&mut self) {
        self.session_manifest_stale = true;
        self.manifest_rev += 1;
    }





    pub(crate) fn new(cli_files: Vec<PathBuf>) -> (Self, Task<Message>) {
        // P33：先把 Family::Monospace 的解析目标钉到 CJK 等宽候选（幂等、
        // 进程内一次）——必须发生在首帧排版之前，否则排版缓存里已固化的
        // 逐字回退不会重排
        apply_default_cjk_mono_pin();
        let mut settings = editpad_core::Settings::load();
        // P62：热键重映射表消毒——动作 id 不在注册表中的条目删除
        // （core 只做组合串格式归一，动作清单是 app 层知识）
        sanitize_hotkeys(&mut settings);
        // P34：枚举系统字体清单（P33 钉字之后，同一 fontdb 全局），并解析
        // 配置的字体——未安装时只回退本次渲染并提示一次，**不抹掉配置**
        // （重装字体后自动恢复用户意图）。
        let available_fonts = enumerate_available_families();
        let (active_font_family, configured_font_missing) =
            resolve_startup_font(settings.font_family.as_deref(), &available_fonts);
        // P154：UI 字体（按界面语言解析）与行号位等宽字体——两者都与正文族
        // 解耦；候选全未命中则 None（UI 回落 iced 默认、行号回落 UI 字体），
        // 不引入失败模式。
        let ui_font_family =
            pick_ui_font_family(settings.language, &available_fonts)
                .map(|name| leak_font_family(name.to_owned()));
        let gutter_font_family = pick_gutter_font_family(&available_fonts)
            .map(|name| leak_font_family(name.to_owned()));        // P29：快照总开关关闭时清空存量快照区——只关开关不清数据等于没关
        // （对齐 P20「记住最近文件」先例）
        if !settings.enable_snapshots {
            if let Some(dir) = editpad_core::snapshot::snapshot_dir() {
                editpad_core::snapshot::clear_session(&dir);
            }
        }
        let dark_mode = settings.is_dark();
        // 设置里的字号可能未归一（旧配置/手改），boot 时按同一规则 clamp
        let font_size = editor::normalize_font_size(settings.font_size);
        let mut state = Self {
            settings,
            dark_mode,
            available_fonts,
            active_font_family: active_font_family.map(leak_font_family),
            ui_font_family,
            gutter_font_family,
            ..Self::default()
        };
        {
            let mut ed = state.cur_handle.borrow_mut();
            ed.set_font_size(font_size);
            // P154：行号栏字体按解析出的等宽族（None = 回落 UI 字体）
            ed.set_gutter_font(gutter_font_family.map(fonts::family_font));
            // 外观设置与 fresh_tab 同口径兜底：boot 初始页走 Self::default()
            // 的 Tab::empty()、不经过 fresh_tab——漏下发则「重启后自动换行/
            // 空白标记失效但设置里仍显示勾选」（用户实测：启动加载的文件
            // 不折行、水平滚动条照旧出现，手动重勾一次才恢复）
            ed.set_invisibles(
                state.settings.show_whitespace,
                state.settings.show_line_endings,
            );
            ed.set_word_wrap(state.settings.word_wrap);
            // P132：同口径兜底（漏下发则重启后参考线/标尺失效但设置仍显示开启）
            ed.set_indent_guides(state.settings.indent_guides);
            ed.set_edge_column(state.settings.edge_column);
        }
        if configured_font_missing {
            if let Some(name) = state.settings.font_family.as_deref() {
                state.status = format!("{}{name}{}", editpad_core::Key::StFontMissing.text(state.settings.language), editpad_core::Key::StFontMissingSuffix.text(state.settings.language));
            }
        }
        // P149：caret 闪烁 / 快照心跳的节拍链已改为订阅时钟驱动（见
        // subscription 的 tick_stream），boot 不再起 Task 睡眠链。
        // P30：启动会话恢复——读清单重建标签；命名干净页经加载管线回填。
        // 开关判定在 boot_restore 内部（关闭 = 空白启动 + 存量清场）。
        // P103：命令行文件（资源管理器双击 / 「打开方式」/ 多选打开）——
        // 用户意图优先于会话恢复：跳过恢复（快照原封留存，下次无参数
        // 启动仍可恢复），文件经打开管线加载。首个文件在 boot 期同步
        // 登记（只改状态，加载流由 subscription 依据 active_load 重建
        // 接管——与拖拽同一条已验证通路）；其余文件留 pending_cli，
        // 每个 Loaded 结算后由 update 内的同步续排弹出下一个。全程不
        // 依赖 boot 任务的消息投递（iced 0.14 实测：boot 任务「立即
        // 输出」在窗口创建初期丢失，CaretTick 类延时输出正常；详见
        // 归档第 79 轮）。
        let (state, boot_task) = if cli_files.is_empty() {
            let task = state.boot_restore();
            (state, task)
        } else {
            state.boot_cli_kickoff(cli_files);
            (state, Task::none())
        };
        // 单实例转发轮询：已运行实例经实例目录的批次文件接收第二实例
        // 递来的待开文件（双击文件而实例已在跑 = 新标签页打开而非弹窗）。
        // P149：轮询节拍已改订阅时钟驱动（见 subscription），boot 不再起链。
        // 注：窗口标题栏图标（第 76 轮用户点单）在 main() 的
        // `.window(Settings { icon })` 声明期下发（见 window_title_icon）
        (state, boot_task)
    }

    /// P103：boot 期把命令行文件清单转成打开链路的初始状态——
    /// 首个文件**同步登记**加载任务（active_load/busy 只是状态变更，
    /// 加载流由 boot 后的 subscription 依据 active_load 重建自动接管，
    /// 与拖拽同一通路）；其余文件进 pending_cli 队列，由每个 Loaded
    /// 结算后的同步续排（[`Message::OpenNextCliFile`]）串行弹出。
    /// 纯状态操作，可在事件循环启动前安全调用。
    pub(crate) fn boot_cli_kickoff(&mut self, cli_files: Vec<PathBuf>) {
        debug_assert!(!cli_files.is_empty(), "空清单不应走到本函数");
        let mut rest = cli_files;
        let first = rest.remove(0);
        let tab = self.target_tab_for_open();
        // start_loading 返回恒为 none（登记即完成；加载流归订阅接管）
        let _ = self.start_loading(first, tab);
        self.pending_cli = rest.into();
    }

    /// P151：把输入焦点交给应用内文本框（查找/替换、标签重命名、命令面板）。
    ///
    /// **正文编辑器必须同步交出焦点**：编辑器核心的 `focused` 只在鼠标按下
    /// 时更新（`pointer_focus`），程序化聚焦不经过鼠标 → 编辑器仍以为焦点
    /// 在自己身上，同一个 IME 组字事件被两处消费：预编辑串在文本框与正文
    /// 各画一份（用户复现「怎么有两个」），**上屏文本还会被编辑器吃进文档**
    /// （在查找框里打中文 = 改正文）。交出焦点 = `pointer_focus(false)`
    /// （清残留组字 + 打断打字组）。
    pub(crate) fn focus_text_field(&mut self, id: iced::widget::Id) -> Task<Message> {
        self.cur_handle.borrow_mut().pointer_focus(false);
        iced::widget::operation::focus(id)
    }

    /// P151：文本框类 UI 关闭后把输入焦点还给正文（IME 组字恢复可用）。
    /// 鼠标点在正文里的情形由控件自身的 `pointer_focus(true)` 覆盖，二者等价。
    pub(crate) fn focus_editor(&mut self) {
        self.cur_handle.borrow_mut().pointer_focus(true);
    }

    /// 消息总入口：按域分派到 `update_editor` / `update_tabs` / `update_file` /
    /// `update_find` / `update_settings` / `update_menus` / `update_session`。
    ///
    /// ## 递归重入约束（改动前必读）
    /// 本函数会被**同步递归调用**——剪切/粘贴、列编辑器确认、CLI 文件续排
    /// 等路径都靠它继承置脏与自动保存调度（当前 8 处调用点）。编辑器状态存
    /// 在 `Rc<RefCell<EditorCore>>` 里，所以**递归调用点必须落在借用作用域
    /// 之外**：若在 `borrow()` / `borrow_mut()` 仍存活时重入，之后任何
    /// `borrow_mut()` 都会 panic（`already mutably borrowed`），而栈回溯离
    /// 现场很远、极难定位。现有调用点均为安全形态——形如
    /// `let t = self.cur_handle.borrow().x();` 的临时借用在语句末即释放，
    /// 递归发生在借用期之外；**新增递归调用时请照此办理**。
    ///
    /// 调试构建会在入口自检一次（见下方断言），把这类问题暴露在最近的位置。
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        // 入口自检：持有 editor 借用时重入 = 后续必然 panic。探测守卫在
        // 表达式结束即丢弃，不改变借用状态；发布构建零开销。
        debug_assert!(
            self.cur_handle.try_borrow_mut().is_ok(),
            "update 不得在持有 editor 借用时被调用——递归重入必须落在借用作用域之外"
        );
        match &message {
            // ---------- 编辑器/剪贴板/光标/预览/高亮铺路 ----------
            Message::Edit(..)
            | Message::ToggleOverwrite
            | Message::ToggleReadOnly
            | Message::EditorNavChanged
            | Message::CopyRequested
            | Message::CutRequested
            | Message::PasteRequested
            | Message::Pasted(..)
            | Message::KeyPressed(..)
            | Message::HighlightPaveNeeded
            | Message::FormatJson
            | Message::HlPaveProgress(..)
            | Message::HlPaved(..)
            | Message::CaretTick
            | Message::PreviewToggled
            | Message::CursorMoved(..)
            | Message::EditorBodyPressed
            | Message::ViewportResized(..) => self.update_editor(message),
            // ---------- 标签页/右键菜单/批关 ----------
            Message::OpenContainingFolder
            | Message::CopyFilePath(..)
            | Message::CopyFileName(..)
            | Message::ReopenLastClosedFile
            | Message::NewTab
            | Message::TabStripBlankPressed
            | Message::SwitchTabNext
            | Message::SwitchTabPrev
            | Message::SwitchTab(..)
            | Message::CloseTabRequest
            | Message::ConfirmCloseTabDiscard(..)
            | Message::CancelCloseTab
            | Message::CloseTabSave(..)
            | Message::TabContextMenu(..)
            | Message::TabContextMenuClosed
            | Message::TogglePinTab(..)
            | Message::SaveTabFromMenu(..)
            | Message::RenameOrSaveAsTab(..)
            | Message::TabRenameInputChanged(..)
            | Message::TabRenameCommitted
            | Message::TabRenameCancelled
            | Message::CloseTabAt(..)
            | Message::CloseOtherTabs(..)
            | Message::CloseTabsRight(..)
            | Message::ConfirmBatchCloseDiscard
            | Message::CancelBatchCloseTabs
            | Message::TabHovered(..) => self.update_tabs(message),
            // ---------- 设置/字体/热键 ----------
            Message::SettingsShowWhitespaceToggled(..)
            | Message::SettingsShowLineEndingsToggled(..)
            | Message::SettingsWordWrapToggled(..)
            | Message::SettingsIndentGuidesToggled(..)
            | Message::SettingsEdgeColumnDelta(..)
            | Message::HotkeyCaptureStarted(..)
            | Message::HotkeyCaptureKey(..)
            | Message::HotkeyCaptureCancel
            | Message::HotkeysResetAll
            | Message::ThemeToggled
            | Message::LanguageToggled
            | Message::LanguageOptionSelected(..)
            | Message::FontSizeDelta(..)
            | Message::TabFontSizeDelta(..)
            | Message::TabFontSizeReset
            | Message::TabWrapOverrideToggled
            | Message::SettingsToggled
            | Message::SettingsPageSelected(..)
            | Message::SettingsSearchChanged(..)
            | Message::SettingsAutosaveToggled(..)
            | Message::SettingsAutosaveDelayDelta(..)
            | Message::SettingsRememberRecentToggled(..)
            | Message::SettingsSnapshotsToggled(..)
            | Message::SettingsBackupModeToggled
            | Message::SettingsRememberSessionToggled(..)
            | Message::SettingsExitModeToggled
            | Message::SettingsIntervalDelta(..)
            | Message::SettingsFontSelected(..)
            | Message::SettingsFontReset
            | Message::FontFilterChanged(..) => self.update_settings(message),
            // ---------- 文件：打开/保存/编码/行尾/拖放/外部变更 ----------
            Message::OpenRequested
            | Message::FileChosen(..)
            | Message::FileDropped(..)
            | Message::OpenNextCliFile
            | Message::PendingOpenTick
            | Message::LoadProgress(..)
            | Message::Loaded(..)
            | Message::LinkClicked(..)
            | Message::SaveRequested
            | Message::SaveAsRequested
            | Message::SaveTargetChosen(..)
            | Message::Saved(..)
            | Message::TabAutosaved(..)
            | Message::WindowFocused
            | Message::ConfirmExternalReload(..)
            | Message::IgnoreExternalChange(..)
            | Message::IgnoreAllExternalChanges
            | Message::ConfirmOpenDiscard
            | Message::ConfirmOpenCancel
            | Message::ToggleEncodingMenu
            | Message::ToggleEolMenu
            | Message::SaveWithEncoding(..)
            | Message::ConvertEol(..)
            | Message::TabSaved(..) => self.update_file(message),
            // ---------- 菜单栏/最近文件/面板 ----------
            Message::MenuToggled(..)
            | Message::MenubarHovered(..)
            | Message::MenubarPressed
            | Message::RecentsToggled
            | Message::RecentSelected(..)
            | Message::RecentsCleared
            | Message::BarsDismissed => self.update_menus(message),
            // ---------- 会话/快照/关窗/窗口 ----------
            Message::CloseRequested(..)
            | Message::ConfirmSaveAndClose
            | Message::DiscardAndClose
            | Message::CancelClose
            | Message::SessionRecoverAccepted
            | Message::SessionRecoverDiscarded
            | Message::SnapshotHeartbeatTick
            | Message::HeartbeatDone(..)
            | Message::ToggleFullscreen
            | Message::ToggleAlwaysOnTop
            | Message::WindowMoved(..) => self.update_session(message),
            // ---------- 查找/替换/跳转/查找全部 ----------
            Message::ToggleMonitorFile
            | Message::MonitorTick
            | Message::PaletteToggled(_)
            | Message::PaletteInputChanged(_)
            | Message::PaletteMove(_)
            | Message::PaletteExecute
            | Message::PalettePick(_)
            | Message::ColumnEditorToggled
            | Message::ColumnEditorModeToggled
            | Message::ColumnEditorTextChanged(_)
            | Message::ColumnEditorStartChanged(_)
            | Message::ColumnEditorStepChanged(_)
            | Message::ColumnEditorWidthChanged(_)
            | Message::ColumnEditorBaseCycled
            | Message::ColumnEditorHexUpperToggled
            | Message::ColumnEditorConfirmed
            | Message::FindToggled
            | Message::FindCursorMoved(..)
            | Message::FindDragStart
            | Message::FindDragEnd
            | Message::FindDragReset
            | Message::FindBoxPressed
            | Message::FindQueryChanged(..)
            | Message::FindNext
            | Message::FindPrev
            | Message::FindAllToggled
            | Message::FindAllGoto(..)
            | Message::FindInFilesToggled
            | Message::FifBrowseFolder
            | Message::FifDirPicked(..)
            | Message::FifScanDone(..)
            | Message::FifGoto(..)
            | Message::CaseToggled(..)
            | Message::RegexToggled(..)
            | Message::WholeWordToggled(..)
            | Message::ReplaceQueryChanged(..)
            | Message::ReplaceCurrent
            | Message::ReplaceCurrentRegex
            | Message::ReplaceAll
            | Message::ReplaceAllRegexDone(..)
            | Message::FindScanDone(..)
            | Message::GotoToggled
            | Message::GotoInputChanged(..)
            | Message::GotoSubmit => self.update_find(message),
        }
    }





    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：菜单栏/最近文件/面板。臂体自原 update() 逐字搬移，零行为变更。
    fn update_menus(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 顶部菜单栏（第 69 轮） ----------
            Message::MenuToggled(idx) => {
                // 同项再点关闭，异项切换（互斥展开）。
                // 第 70 轮：打开瞬间**冻结锚点**——悬停位置此后继续变化
                // （含背板 on_move）不再影响已展开浮层的位置（修「弹窗
                // 会移动」：旧实现锚点实时读悬停点，展开期间漂移）
                self.menubar_anchor = self.menubar_pos;
                self.menu_bar_open = if self.menu_bar_open == Some(idx) {
                    None
                } else {
                    Some(idx)
                };
                Task::none()
            }
            Message::MenubarHovered(pos) => {
                self.menubar_pos = (pos.x, pos.y);
                Task::none()
            }
            // 背板点击：菜单栏条带内 = 横移切换到目标菜单；条带外 = 收起
            Message::MenubarPressed => {
                if self.menubar_pos.1 < MENU_BAR_H {
                    let idx = menubar_slot_idx(self.menubar_pos.0);
                    self.menubar_anchor = self.menubar_pos;
                    self.menu_bar_open = if self.menu_bar_open == Some(idx) {
                        None
                    } else {
                        Some(idx)
                    };
                } else {
                    self.menu_bar_open = None;
                }
                Task::none()
            }
            // ---------- 最近文件 ----------
            Message::RecentsToggled => {
                self.recents_visible = !self.recents_visible;
                Task::none()
            }
            Message::RecentSelected(entry) => {
                // 存在性预检：文件已删除/移动时点开必然走完整加载失败
                // 管线，直接明示；条目保留（可能只是暂时移动/离线盘符）
                let path = PathBuf::from(&entry);
                if path.exists() {
                    self.request_open(path)
                } else {
                    self.set_status_error(self.t_suffix(editpad_core::Key::StRecentMissing, &entry));
                    Task::none()
                }
            }
            Message::RecentsCleared => {
                // P20 隐私：立即写回空列表，config.toml 不再含历史路径
                self.settings.clear_recent_files();
                self.persist_settings();
                self.set_status(self.t(editpad_core::Key::StRecentsCleared).to_owned());
                Task::none()
            }
            Message::BarsDismissed => {
                self.find_visible = false;
                self.goto_visible = false;
                self.recents_visible = false;
                // P151：Esc 同时收起命令面板与重命名态（下方既有分支同款），
                // 收尾把输入焦点还给正文——否则中文输入法在正文里失效
                self.palette_visible = false;
                self.focus_editor();
                // P27：Esc 一并关闭设置弹窗
                self.settings_visible = false;
                // Esc 同时视作放弃关闭/打开确认
                self.confirm_visible = false;
                self.pending_close = false;
                self.open_confirm = None;
                // P21：Esc 也取消标签页关闭确认
                self.close_tab_confirm = None;
                // P28：Esc 同时收起右键菜单与批量关闭确认
                self.tab_context_menu = None;
                self.batch_close_confirm = None;
                // 第 69 轮：Esc 同时收起顶部菜单栏浮层
                self.menu_bar_open = None;
                // P50：Esc 一并收起外部修改提示条
                self.external_change = None;
                // P55：Esc 一并取消就地重命名（一切保持原状）
                self.renaming_tab = None;
                self.rename_input.clear();
                // P67：Esc 一并收起状态栏编码/行尾菜单
                self.encoding_menu = false;
                self.eol_menu = false;
                // P10：取消在途扫描 + 清结果（含序号失效）
                self.cancel_find_scan();
                // 关栏（Esc 路径）同样清掉查找进度提示，避免左下角残留；
                // 只清自己写的那条（换过别的状态时标记已失效）
                if self.find_status {
                    self.status.clear();
                    self.status_is_error = false;
                    self.find_status = false;
                }
                self.find_drag = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    // ---------- domain methods (round 81 Phase 1: update() split) ----------
    /// 域：会话/快照/关窗/窗口。臂体自原 update() 逐字搬移，零行为变更。
    fn update_session(&mut self, msg: Message) -> Task<Message> {
        match msg {
            // ---------- 关闭确认 ----------
            Message::CloseRequested(id) => {
                self.handle_close_request(id, editpad_core::snapshot::snapshot_dir())
            }
            Message::ConfirmSaveAndClose => {
                self.confirm_visible = false;
                self.pending_close = true;
                match self.tab().path.clone() {
                    Some(_) => self.save(),
                    // 未命名文档：先走另存为，落盘成功后自动关窗
                    None => self.save_as_dialog(),
                }
            }
            Message::DiscardAndClose => {
                self.discard_all_and_close(editpad_core::snapshot::snapshot_dir())
            }
            Message::CancelClose => {
                self.confirm_visible = false;
                self.pending_close = false;
                Task::none()
            }
            // ---------- 启动会话恢复（P30） ----------
            Message::SessionRecoverAccepted => {
                self.accept_session_recover(editpad_core::snapshot::snapshot_dir())
            }
            Message::SessionRecoverDiscarded => {
                self.discard_session_recover(editpad_core::snapshot::snapshot_dir())
            }
            Message::SnapshotHeartbeatTick => {
                // P149：节拍已改订阅时钟驱动（subscription 按快照底座开关/
                // 模式门控），本臂只做巡检，不再自我续期。
                // 快照底座任一开关关闭 / ask 模式 = 心跳整体停摆：
                // 清单不写，退出流与启动恢复同样不依赖它（P29/P30 语义）
                if !session_restore_allowed(
                    self.settings.enable_snapshots,
                    self.settings.remember_session,
                ) || self.settings.exit_mode != editpad_core::EXIT_MODE_SNAPSHOT
                {
                    return Task::none();
                }
                // 至多一个提交在途：本轮巡检跳过（页级账目不受影响）
                if self.heartbeat_inflight {
                    return Task::none();
                }
                // 目录解析走注入点（生产为 None → 系统配置目录）
                let dir = self
                    .snapshot_dir_override
                    .clone()
                    .or_else(editpad_core::snapshot::snapshot_dir);
                let Some(dir) = dir else {
                    return Task::none();
                };
                match self.prepare_heartbeat_commit(&dir) {
                    // 无变化且清单不过期：什么都不写（防无谓 IO）
                    None => Task::none(),
                    Some(payload) => {
                        self.heartbeat_inflight = true;
                        Task::perform(
                            async move { drive_heartbeat(payload).await },
                            |message| message,
                        )
                    }
                }
            }
            Message::HeartbeatDone(outcome) => {
                self.heartbeat_apply(outcome);
                Task::none()
            }
            // P126：全屏切换（F11）。iced 0.14：window::set_mode。
            Message::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                let mode = if self.fullscreen {
                    iced::window::Mode::Fullscreen
                } else {
                    iced::window::Mode::Windowed
                };
                let task: Task<Message> = match self.main_window {
                    Some(id) => iced::window::set_mode(id, mode),
                    None => Task::none(),
                };
                self.set_status(if self.fullscreen {
                    self.t(editpad_core::Key::StFullscreenOn).to_owned()
                } else {
                    self.t(editpad_core::Key::StFullscreenOff).to_owned()
                });
                task
            }
            // P126：置顶切换（F9）。iced 0.14：window::set_level。
            Message::ToggleAlwaysOnTop => {
                self.always_on_top = !self.always_on_top;
                let level = if self.always_on_top {
                    iced::window::Level::AlwaysOnTop
                } else {
                    iced::window::Level::Normal
                };
                let task: Task<Message> = match self.main_window {
                    Some(id) => iced::window::set_level(id, level),
                    None => Task::none(),
                };
                self.set_status(if self.always_on_top {
                    self.t(editpad_core::Key::StAlwaysOnTopOn).to_owned()
                } else {
                    self.t(editpad_core::Key::StAlwaysOnTopOff).to_owned()
                });
                task
            }
            Message::WindowMoved(p) => {
                // P102：窗口位置记忆（逻辑坐标，iced 与建窗 Specific 同空间）
                self.settings.window_x = Some(p.x as i32);
                self.settings.window_y = Some(p.y as i32);
                self.persist_geometry_if_due();
                Task::none()
            }
            _ => Task::none(),
        }
    }


    // ---------- 编辑分发 ----------




    // ---------- 加载管线 ----------








    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let load = match &self.active_load {
            Some(job) => Subscription::run_with(job.clone(), build_load_stream),
            None => Subscription::none(),
        };
        // P149：周期节拍全部改订阅时钟驱动（OS 线程独占睡眠 + async
        // channel 桥接，见 tick_stream）——替代原「Task::perform 睡眠 +
        // 自我续期」的四条链（caret/心跳/监视/转发轮询），执行器 worker
        // 不再被常驻睡眠占用。订阅按 run_with 身份去重：开关/间隔变化
        // 即重键，旧流撤销、桥接线程自然收敛。
        let caret_interval = if self.cur_handle.borrow().scrollbar_fading() {
            editor::SCROLLBAR_FADE_TICK_MS
        } else {
            editor::CARET_BLINK_MS
        };
        let caret = Subscription::run_with(
            TickKind::Caret { interval_ms: caret_interval },
            tick_stream,
        );
        let heartbeat =
            if session_restore_allowed(
                self.settings.enable_snapshots,
                self.settings.remember_session,
            ) && self.settings.exit_mode == editpad_core::EXIT_MODE_SNAPSHOT {
                Subscription::run_with(
                    TickKind::Heartbeat { interval_secs: self.settings.snapshot_interval_secs },
                    tick_stream,
                )
            } else {
                Subscription::none()
            };
        let monitor = if self.tabs.iter().any(|t| t.monitor) {
            Subscription::run_with(TickKind::Monitor, tick_stream)
        } else {
            Subscription::none()
        };
        let pending_open = Subscription::run_with(TickKind::PendingOpen, tick_stream);
        // P10 的查找扫描走 Task::perform（见 schedule_find_scan），不经订阅
        // 0.14 没有 keyboard::on_key_press 了，用 listen_with 手动过滤按键；
        // 同一条流顺带捕获拖拽文件（FileDropped；FileHovered 忽略）。
        // P62：按键转发为 KeyPressed 消息、分发挪到 update——重映射表与
        // 热键捕获态必须读活状态，订阅闭包捕获会陈旧。
        let events = iced::event::listen_with(|event, status, _window| match (event, status) {
            (
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }),
                iced::event::Status::Ignored,
            ) => Some(Message::KeyPressed(key, modifiers)),
            (iced::Event::Window(window::Event::FileDropped(path)), _) => {
                Some(Message::FileDropped(path))
            }
            // P50：窗口重新聚焦 = 外部修改巡检时机（编辑器无常驻轮询，
            // 焦点回归是最自然的检查点——用户刚从外部工具切回来）
            (iced::Event::Window(window::Event::Focused), _) => Some(Message::WindowFocused),
            // P39/P40：窗口逻辑尺寸（浮层贴边钳制依据；iced_winit 已
            // 换算成逻辑坐标，与 mouse_area 光标坐标同空间）
            (iced::Event::Window(window::Event::Resized(size)), _) => {
                Some(Message::ViewportResized(size.width, size.height))
            }
            // P102：窗口移动（逻辑坐标）→ 几何记忆（节流落盘见 update）
            (iced::Event::Window(window::Event::Moved(position)), _) => {
                Some(Message::WindowMoved(position))
            }
            _ => None,
        });
        // 窗口关闭请求：exit_on_close_request(false) 后以订阅事件流转
        let close_requests = window::close_requests().map(Message::CloseRequested);
        // P18 即时保存不走订阅：编辑后由 maybe_schedule_autosave 直接派发
        // 「睡眠防抖→落盘」的专用线程（inflight 去重，至多一个挂起）
        Subscription::batch([load, events, close_requests, caret, heartbeat, monitor, pending_open])
    }

    // ---------- 保存 ----------



    // ---------- 即时保存（P18，按页独立） ----------





    /// P155：`Key` + 运行期值的拼接（`fmt_suffix` 的短封装）。
    /// 注：`self.t(..)` 定义在 view.rs（同一 `impl Editpad`，全局唯一）。
    fn t_suffix(&self, key: editpad_core::Key, value: &str) -> String {
        editpad_core::fmt_suffix(self.settings.language, key, value)
    }

    /// P155：切换界面语言的**唯一落地点**（下拉框与兼容循环入口都走它）。
    ///
    /// 界面文案不需要任何「失效」动作——view 层每帧按 `Settings.language`
    /// 现取；这里只做两件有副作用的事：① 重新解析 UI 字体族（候选链按
    /// 语言分派）；② 落盘 + 状态栏反馈（**用切换后的语言**书写，否则英文
    /// 界面会蹦出一句中文）。正文/预览与行号族完全不受影响。
    pub(crate) fn apply_language(&mut self, lang: editpad_core::Lang) {
        self.settings.language = lang;
        self.persist_settings();
        self.ui_font_family = pick_ui_font_family(lang, &self.available_fonts)
            .map(|name| leak_font_family(name.to_owned()));
        let key = editpad_core::Key::StLanguageSwitched;
        let status = match self.ui_font_family {
            Some(family) => {
                // 括号宽窄随语言（中文全角 / 英文半角）——英文界面里冒出
                // 全角括号是很显眼的混排瑕疵
                let wrap = match lang {
                    editpad_core::Lang::En => format!(" ({family})"),
                    editpad_core::Lang::ZhCn => format!("（{family}）"),
                };
                editpad_core::fmt_suffix(lang, key, &wrap)
            }
            None => key.text(lang).to_owned(),
        };
        self.set_status(status);
    }
    /// 统一设置落盘入口：测试注入 `settings_path_override` 时写到
    /// 临时目录，绝不动真实 %APPDATA%；否则走系统配置目录（尽力而为）。
    pub(crate) fn persist_settings(&self) {
        if let Some(path) = &self.settings_path_override {
            let _ = self.settings.save_to(path);
        } else {
            self.settings.save();
        }
    }

    /// P102：窗口几何节流落盘——拖动/拉伸是每帧事件，至少隔
    /// [`GEOMETRY_PERSIST_INTERVAL`] 才写一次盘；关闭路径在
    /// `handle_close_request` 里兜底补一次（见 view.rs）。
    fn persist_geometry_if_due(&mut self) {
        // P126：全屏期间不记忆几何——尺寸是全屏值，退出全屏按它恢复会错
        if self.fullscreen {
            return;
        }
        let now = std::time::Instant::now();
        let due = self
            .last_geometry_persist
            .is_none_or(|t| now.duration_since(t) >= GEOMETRY_PERSIST_INTERVAL);
        if due {
            self.last_geometry_persist = Some(now);
            self.persist_settings();
        }
    }
}

/// P102：窗口几何落盘节流窗（2 秒一道；最后一次状态由关闭路径兜底）。
const GEOMETRY_PERSIST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);


// ---------- P126：只读判定与资源管理器定位 ----------

/// P126：只读锁定的动作分类——true = 会改动文档内容（只读下拒收）。
/// 白名单只列「纯导航/纯标注/纯状态」动作；**默认 true**（fail-safe）：
/// 未来新增变体若忘登记将自动被只读拒收，如属纯导航需显式加入白名单。
pub(crate) fn edit_op_mutates(op: &EditOp) -> bool {
    !matches!(
        op,
        EditOp::SelectAll
            | EditOp::Motion(..)
            | EditOp::ToggleBookmark
            | EditOp::BookmarkNext
            | EditOp::BookmarkPrev
            | EditOp::BookmarksClearAll
            | EditOp::CopyBookmarkedLines
            | EditOp::JumpToMatchingBracket
            | EditOp::CancelBlock
            // B10：添加下一匹配只动光标集，不改文档（只读页可用）
            | EditOp::AddNextMatch
            // P135：DropSelection 的 copy 变体不改内容，但 move 变体改——
            // fail-safe 口径一律按可变处理（只读态拖拽本就禁启动）
            | EditOp::DropSelection { .. }
    )
}

/// P126：在资源管理器中定位并选中文件（explorer /select,路径）。
/// explorer 自身退出码不可靠，只校验进程能否启动。
#[cfg(windows)]
fn reveal_in_explorer(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
}

/// 非 Windows 平台暂无对应实现（本编辑器以 Windows 为一等公民）。
#[cfg(not(windows))]
fn reveal_in_explorer(_path: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::other("仅支持 Windows"))
}

/// P133：用系统默认处理程序打开 URL / file URI（http(s) → 默认浏览器，
/// file:/// → 资源管理器/浏览器）。Windows 走 explorer.exe 转交默认
/// 关联（与 reveal_in_explorer 同款：只校验进程能否启动）；非 Windows
/// 用 xdg-open。
#[cfg(windows)]
fn open_external(target: &str) -> std::io::Result<()> {
    // 测试态不真开系统程序（LinkClicked 处理链路可达本函数）
    #[cfg(test)]
    let _ = target;
    #[cfg(test)]
    return Ok(());
    #[cfg(not(test))]
    {
        std::process::Command::new("explorer").arg(target).spawn().map(|_| ())
    }
}

/// 非 Windows 平台走 xdg-open。
#[cfg(not(windows))]
fn open_external(target: &str) -> std::io::Result<()> {
    // 测试态不真开系统程序（与 Windows 分支同口径）
    #[cfg(test)]
    let _ = target;
    #[cfg(test)]
    return Ok(());
    #[cfg(not(test))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}
