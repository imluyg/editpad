use super::*;

/// B9 列编辑器对话框草稿（Phase 2）。数值字段以字符串承载——输入框
/// 原始态，确认时统一校验解析（无效输入不关闭对话框、状态栏提示）。
/// 草稿跨开合保留（命令面板查询串同口径），不落盘不入快照。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColumnEditorDraft {
    /// true = 序号模式；false = 文本模式
    pub(crate) number_mode: bool,
    /// 文本模式：每行插入的内容（多行 = 循环填充 + 块扩展，v1 口径）
    pub(crate) text: String,
    /// 序号模式：起始值（i64）
    pub(crate) start: String,
    /// 序号模式：步长（i64，可负）
    pub(crate) step: String,
    /// 序号模式：进制（十六进制时 hex_upper 生效）
    pub(crate) base: crate::editor::NumBase,
    /// 序号模式：补零宽度（"0" = 不补；上限 32）
    pub(crate) pad_width: String,
    /// 序号模式：十六进制字母大写
    pub(crate) hex_upper: bool,
}

impl Default for ColumnEditorDraft {
    fn default() -> Self {
        Self {
            number_mode: false,
            text: String::new(),
            start: "1".to_owned(),
            step: "1".to_owned(),
            base: crate::editor::NumBase::Dec,
            pad_width: "0".to_owned(),
            hex_upper: false,
        }
    }
}

/// 应用状态（P21 骨架）：`tabs` 是标签页真值集合；
/// busy/status/查找/跳转/确认条等交互态保持全局。
#[derive(Debug, Clone)]
pub(crate) struct Editpad {
    // ---------- 多标签（P21 骨架） ----------
    /// 标签页集合；恒非空（关闭最后一个会重置为新的空标签页）。
    pub(crate) tabs: Vec<Tab>,
    /// 当前激活的标签下标。
    pub(crate) active_tab: usize,
    /// 活动页编辑器句柄的**长期别名**：与 `tabs[active_tab].editor`
    /// 指向同一 RefCell。单独存一份是因为 view()/apply_edit 等需要
    /// 跨语句借用，临时克隆的句柄活不过当前函数。
    pub(crate) cur_handle: EditorHandle,
    /// 对话框/IO 进行中，防止重复触发
    pub(crate) busy: bool,
    pub(crate) status: String,
    /// 上一条状态是否为错误：错误以红色 ⚠ 渲染，且**不被编辑噪声
    /// 清除**（如「保存失败」必须持久到用户做出下一个有效动作才让位）。
    /// 写入一律走 [`Editpad::set_status`] / [`Editpad::set_status_error`]。
    pub(crate) status_is_error: bool,
    /// 手动保存时暂存的「已备份旧版」提示：备份消息在异步落盘完成前
    /// 写入会被 [`Message::Saved`] 分支立即覆盖/抹掉，故随保存请求暂存，
    /// 落盘成功且无转码提示时补显。
    pub(crate) pending_backup_notice: Option<String>,

    // ---------- 即时保存（P18，版本号已下沉 Tab） ----------
    /// 「保存后关闭标签」的目标页 id（P146：曾存下标，存盘期间关页/
    /// 切页会让下标漂移——错页无确认关闭、静默丢弃内容）；
    /// Saved/TabSaved 完成后据此关页
    pub(crate) pending_close_tab: Option<u64>,

    // ---------- 设置 ----------
    pub(crate) settings: editpad_core::Settings,
    /// 配置文件注入点（测试用）；None = 系统配置目录。
    /// 所有 `persist_settings` 都走这里，保证单测绝不碰真实 %APPDATA%。
    pub(crate) settings_path_override: Option<PathBuf>,
    /// 设置弹窗是否可见（P27）：工具栏「设置」按钮开、Esc/关闭按钮关。
    pub(crate) settings_visible: bool,
    /// 设置弹窗当前分类页（P47 侧栏导航；重开弹窗保留上次位置）。
    pub(crate) settings_page: SettingsPage,
    /// 设置弹窗侧栏搜索词（P47；纯 UI 态不落盘，关弹窗/点导航即清）。
    pub(crate) settings_search: String,
    /// P50：外部修改提示条队列（P52 起聚合多页）：待用户裁决的页下标，
    /// 按标签顺序排列。None/空 = 提示条不可见。Esc（BarsDismissed）/
    /// 逐个裁决/全部忽略即清。
    pub(crate) external_change: Option<Vec<usize>>,
    /// P55：就地重命名的目标页下标；Some = 标签条上该页显示为输入框。
    pub(crate) renaming_tab: Option<usize>,
    /// P55：就地重命名的输入内容（预填当前文件名，纯 UI 态）。
    pub(crate) rename_input: String,
    /// P67：状态栏「编码」弹出菜单可见。
    pub(crate) encoding_menu: bool,
    /// P67：状态栏「行尾」弹出菜单可见（与编码菜单互斥）。
    pub(crate) eol_menu: bool,
    /// P70：正则查找模式（查找栏「.*」开关；会话态不持久化）。
    pub(crate) regex_enabled: bool,
    /// 整词匹配开关（查找栏「整词」；命中前后均非词字符。正则模式
    /// 下不参与。会话态不持久化，与大小写开关同口径）。
    pub(crate) whole_word: bool,
    /// P65 双击重命名：标签条上最近一次左键点击的 (页下标, 时刻)。
    /// 同页在 [`TAB_DOUBLE_CLICK_MS`] 窗内再点一次 = 重命名意图。
    /// 纯应用层检测——内层 button 会捕获左键，外层 MouseArea 收不到
    /// on_double_click（iced 事件流实测），故在 SwitchTab 里记账判定。
    pub(crate) last_tab_click: Option<(usize, std::time::Instant)>,
    /// P112：指针悬停的页签下标（页签胶囊悬停底色的数据源；纯 UI 态，
    /// 不落盘）。离开页签 = None；页签被关闭/移除时关闭路径清空，
    /// 越界下标在消息处理层复位为 None（防陈旧悬停染色错页）。
    pub(crate) hovered_tab: Option<usize>,
    /// P62：热键捕获态——Some(动作 id) = 设置热键页正在等待新组合键。
    pub(crate) hotkey_capture: Option<&'static str>,

    // ---------- 字体选择（P34） ----------
    /// 启动期从 fontdb 枚举的系统字体族名清单（去重、不区分大小写排序）。
    /// 设置弹窗的选择列表数据源；空 = 枚举失败（弹窗显示提示并隐藏列表）。
    pub(crate) available_fonts: Vec<String>,
    /// 实际生效的字体族名（经 [`effective_font_family`] 对系统清单解析后的
    /// 规范名，已 `'static` 化——见 [`leak_font_family`]）。None = 默认等宽。
    /// 配置值与生效值的分离让「卸载了所选字体」只回退本次渲染，不抹掉
    /// 用户配置（重装后自动恢复）。**P154 起只作用于正文与预览**。
    pub(crate) active_font_family: Option<&'static str>,
    /// P154：UI 字形族名（按 [`Settings::language`] 从候选表解析、已
    /// `'static` 化）。None = 候选全未命中 → 回落 `Font::DEFAULT`。
    /// 与正文族完全解耦：只影响除正文/预览之外的界面文字；行号栏另有
    /// [`Self::gutter_font_family`]。
    pub(crate) ui_font_family: Option<&'static str>,
    /// P154：行号栏字形族名（等宽候选链解析，见
    /// [`crate::fonts::pick_gutter_font_family`]）。None = 全未命中 →
    /// 回落 UI 字体（行号宽度仍由实测字宽保护，见 P150）。
    pub(crate) gutter_font_family: Option<&'static str>,
    /// 设置弹窗字体列表的过滤词（纯 UI 态，不落盘）。
    pub(crate) font_filter: String,

    // ---------- 后台加载 ----------
    pub(crate) job_seq: u64,
    /// 进行中的加载任务；None 表示没有
    pub(crate) active_load: Option<LoadJob>,
    /// (已读字节, 总字节)
    pub(crate) progress: Option<(u64, u64)>,
    /// P103：命令行传入的待打开文件队列（双击/「打开方式」）。boot 注入，
    /// [`Message::OpenNextCliFile`] 逐个弹出；加载管线单任务承接，故串行。
    pub(crate) pending_cli: VecDeque<PathBuf>,

    // ---------- 查找 / 替换 ----------
    pub(crate) find_visible: bool,
    pub(crate) find_query: String,
    pub(crate) replace_query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) matches: Vec<editpad_core::MatchPos>,
    pub(crate) match_idx: Option<usize>,
    /// 在途后台扫描的序号；None 表示没有。迟到的旧结果按它丢弃（P10）
    pub(crate) find_scan: Option<u64>,
    pub(crate) find_seq: u64,
    /// 当前代扫描的取消标志；新任务排队时把旧标志置位（P10 防抖取消）
    pub(crate) find_cancel: Arc<AtomicBool>,
    /// 「查找全部」结果面板可见（第 62 轮）：数据源 = matches 全量命中表，
    /// 扫描刷新时自动跟随；仅当查找栏可见时有意义（面板停靠在查找区内）
    pub(crate) find_all_visible: bool,
    /// 查找浮层左上角位置（窗口坐标）。None = 用默认位置（窗口中间偏上）；
    /// 用户点单：浮层遮住正文目标行时必须能拖开，故位置由应用层持有。
    pub(crate) find_pos: Option<Point>,
    /// 拖动中的上一帧光标位置（按住拖动条时按逐帧增量平移浮层）；
    /// None = 未在拖动。按下时用 [`Self::find_cursor`] 作锚点（on_press
    /// 不带坐标，锚点取最近一次 on_move 的位置）。
    pub(crate) find_drag: Option<Point>,
    /// 最近一次已知光标位置（窗口坐标；`on_move` 层每帧更新）
    pub(crate) find_cursor: Point,
    /// 状态栏当前显示的是查找进度（「第 N/M 处匹配」）——关闭查找栏时
    /// 据此清理，避免关栏后左下角残留（用户复报）
    pub(crate) find_status: bool,
    /// P153：查找框淡出态——用户点了正文内容（编辑/选词）后置位，浮层
    /// 背板转半透明（[`crate::chrome::FIND_CARD_DIM_ALPHA`]）让开被遮挡的
    /// 正文；点回查找卡片（任意处）即复位。纯 UI 态，不落盘不入快照；
    /// 关栏时一并清零（重开恒为不透明）。
    pub(crate) find_dimmed: bool,

    // ---------- A8：在文件中查找（设计 docs/find-in-files-design.md） ----------
    /// FIF 模式开启（面板与查找全部面板同槽互斥；查找栏关闭即隐）
    pub(crate) fif_visible: bool,
    /// 扫描根目录；None = 未命名页（无 base_dir 锚点，入口禁用）
    pub(crate) fif_dir: Option<PathBuf>,
    pub(crate) fif_seq: u64,
    /// 在途 FIF 扫描代次（None = 没有）；迟到旧结果按它丢弃
    pub(crate) fif_scan: Option<u64>,
    /// 当前代 FIF 扫描取消标志（新扫描/关闭时置位上一代）
    pub(crate) fif_cancel: Arc<AtomicBool>,
    /// 已扫描文件数（后台线程累加，面板标题每帧读数 O(1)）
    pub(crate) fif_progress: Arc<AtomicUsize>,
    /// 最近一次完成的结果（跨开合保留；不入会话快照——纯搜索态）
    pub(crate) fif_results: Vec<crate::find_scan::FileHits>,
    /// 结果是否因封顶截断（2 万文件 / 5000 总命中）
    pub(crate) fif_truncated: bool,
    /// FIF 命中点击的待跳转（行 0 起, 列, 跨度）——下一次 Loaded 装载
    /// 结算后一次性消费（select_span 选区落在命中上）
    pub(crate) pending_fif_goto: Option<(usize, usize, usize)>,

    // ---------- 高亮后台分批补建（P12） ----------
    /// 在途铺建任务的代次；None = 没有。同代幂等、异代重排
    pub(crate) hl_paving: Option<u64>,
    /// 在途铺建任务的取消标志（编辑换代/新任务排队时置位）
    pub(crate) hl_pave_cancel: Arc<AtomicBool>,

    // ---------- 跳转 ----------
    pub(crate) goto_visible: bool,
    pub(crate) goto_input: String,

    // ---------- 最近文件 ----------
    pub(crate) recents_visible: bool,

    // ---------- 未保存关闭确认 ----------
    /// 确认条是否可见
    pub(crate) confirm_visible: bool,
    /// 「保存并关闭」进行中：Saved(Ok) 后执行真正的窗口关闭
    pub(crate) pending_close: bool,
    /// 主窗口 id。iced 0.14 没有 Id::MAIN 之类的常量，只能从 close_requests 订阅捕获
    pub(crate) main_window: Option<window::Id>,
    /// P126：全屏态（F11）。全屏期间窗口几何不记忆（尺寸是全屏值，
    /// 退出后按它恢复会错）。
    pub(crate) fullscreen: bool,
    /// P126：置顶态（F9）。
    pub(crate) always_on_top: bool,
    /// P129：命令面板/快速标签切换浮层可见性
    pub(crate) palette_visible: bool,
    /// P129：面板模式（命令 = 全部注册表动作；标签 = 当前会话页间跳转）
    pub(crate) palette_mode: PaletteMode,
    /// P129：面板查询串（跨开合保留，主流编辑器同口径）
    pub(crate) palette_input: String,
    /// P129：当前选中行（过滤后列表下标）
    pub(crate) palette_idx: usize,
    // ---------- B9 列编辑器对话框（Phase 2） ----------
    /// 对话框可见性（居中模态，与设置弹窗/命令面板互斥——打开时关对方）
    pub(crate) column_editor_visible: bool,
    /// 对话框草稿（跨开合保留）
    pub(crate) column_editor: ColumnEditorDraft,
    /// P130：待归页的监视重载（tab 下标, 是否 tail 跟随, 重载前视图）——
    /// check_external_changes 发起监视重载时捕获，Loaded 归页时消费
    ///（元组即重载前视图快照，一次性搬运不设类型别名）
    #[allow(clippy::type_complexity)]
    pub(crate) monitor_pending: Option<(usize, bool, Option<(usize, usize, f32, f32)>)>,
    /// P133：链接点击的待跳行号（1 起）——Ctrl+点击 `file.c:42` 时暂存，
    /// 下一次 Loaded 装载结算后一次性消费（jump_to_line）
    pub(crate) pending_link_goto: Option<u32>,

    // ---------- 打开确认 ----------
    /// dirty 时暂存待打开的路径；Some 即打开确认条可见
    pub(crate) open_confirm: Option<PathBuf>,

    // ---------- 标签页关闭确认（P21） ----------
    /// Some(idx) = 第 idx 个标签页置脏，正在确认「放弃更改并关闭」
    pub(crate) close_tab_confirm: Option<usize>,
    // ---------- 标签右键菜单（P28） ----------
    /// Some(idx) = 正在展示第 idx 个标签页的右键菜单
    pub(crate) tab_context_menu: Option<usize>,
    // ---------- 浮层弹窗定位（P39/P40） ----------
    /// 右键菜单锚点（窗口系坐标）：打开菜单那一刻的指针位置
    pub(crate) menu_anchor: (f32, f32),
    /// 指针在标签条区域内的最新位置（锚点数据源；标签条 mouse_area 跟踪）
    pub(crate) cursor_pos: (f32, f32),
    /// 窗口逻辑尺寸（Resized 事件；(0,0) = 未知，贴边钳制跳过）
    pub(crate) viewport_size: (f32, f32),
    /// Some(targets) = 批量关闭（关闭其他/右侧）目标列表，任一置脏时
    /// 先弹一次聚合确认；确认后统一放弃并移除。固定页不在列表内。
    pub(crate) batch_close_confirm: Option<Vec<usize>>,
    // ---------- 未命名页编号（P25） ----------
    /// 下一个未命名页序号（P25 曾为全局单调赋号的来源；P168 起赋号改
    /// 走「最小空闲复用」（见 `assign_untitled_num`），本字段不再被
    /// 赋号消费，仅随会话快照写入/恢复维持清单格式兼容）。
    pub(crate) untitled_next: u64,

    // ---------- 启动会话恢复（P30） ----------
    /// 待载入的命名干净页队列：占位页已在 tabs 中就位，
    /// 逐个经既有加载管线回填内容与视图。
    pub(crate) restore_queue: Vec<RestoreLoad>,
    /// 恢复加载任务的待还原视图：(job id → (行, 列, 垂直滚动, 水平滚动))。
    /// LoadJob 参与 Hash/Eq 不能携带 f32，故挂在应用状态侧按任务号取用；
    /// 取出即视为恢复任务（普通打开不在此表）。
    pub(crate) restore_views: HashMap<u64, (usize, usize, f32, f32)>,
    /// 一次性崩溃恢复提示条（clean_exit=false 的异常退出清单）；
    /// Some = 提示中，等用户裁决恢复或丢弃，数据原封留在磁盘。
    pub(crate) recover_prompt: Option<editpad_core::snapshot::SessionManifest>,
    /// 本轮恢复中未能还原原内容的页数（快照缺失/加载失败；汇总提示口径）
    pub(crate) restore_failed: usize,
    /// 因内存护栏被放弃恢复的页数（§3 P30 第 5 条的截断提示口径）
    pub(crate) restore_dropped: usize,
    /// 尚未落地的恢复加载页数（归零时出汇总状态）
    pub(crate) restore_pending: usize,

    // ---------- 周期快照心跳（P31） ----------
    /// 心跳提交任务在途标记（全局至多一个；页级账目在 Tab 上）
    pub(crate) heartbeat_inflight: bool,
    /// 「内存态比最近一次已提交清单更干净/结构已变」标记：保存清脏、
    /// 关页、换文档等事件置位——下一拍即使无页被代次去重选中也要重写
    /// 清单，防止崩溃恢复把用户已落盘/已关闭的内容按旧快照复活
    /// （编辑置脏不置位：崩溃丢 ≤1 个间隔的输入正是心跳的设计语义）。
    pub(crate) session_manifest_stale: bool,
    /// P146：过期标记的代次计数——每次置位 +1。心跳派发时随载荷带走，
    /// 成功回报只在「派发后没有新置位」时才清标记（在途期间关页曾被
    /// 成功回报误清，清单留着已关页永不重写，崩溃恢复复活已关页）。
    pub(crate) manifest_rev: u64,
    /// 快照目录注入点（测试用）；None = 系统配置目录。
    pub(crate) snapshot_dir_override: Option<PathBuf>,
    /// P102：窗口几何最后一次落盘时刻（拖动/拉伸事件高频，节流用）。
    /// None = 本会话尚无几何变化（关闭时的兜底落盘据此门控，测试
    /// 环境不发窗口事件 = 永不触碰真实配置目录）。
    pub(crate) last_geometry_persist: Option<std::time::Instant>,

    /// 「恢复上次关闭的文件」记忆栈（第 64 轮）：会话内 Vec<PathBuf>，
    /// 最近期在前；close_tabs_now 统一入栈、ReopenLastClosedFile 出栈
    pub(crate) closed_stack: Vec<PathBuf>,
    /// 第 69 轮：顶部菜单栏当前展开的菜单（None = 全收起）
    pub(crate) menu_bar_open: Option<usize>,
    /// 菜单栏条上最近指针位置（浮层菜单锚点）
    pub(crate) menubar_pos: (f32, f32),
    /// 第 70 轮：展开瞬间的冻结锚点（浮层位置只读它——展开期间悬停
    /// 变化不影响已打开菜单的位置）
    pub(crate) menubar_anchor: (f32, f32),

    // ---------- 外观 ----------
    pub(crate) dark_mode: bool,
    /// Markdown 预览面板可见（P22 第三批；仅 Markdown 语法页渲染）
    pub(crate) preview_visible: bool,
}

impl Default for Editpad {
    fn default() -> Self {
        // P25：初始页即「未命名1」，下一个新页为「未命名2」
        let mut first = Tab::empty();
        first.untitled_num = Some(1);
        let tabs = vec![first];
        let cur_handle = tabs[0].editor.clone();
        Self {
            // P21：初始恒有一个空标签页（tabs 恒非空不变式）
            tabs,
            active_tab: 0,
            cur_handle,
            busy: false,
            status: String::new(),
            status_is_error: false,
            pending_backup_notice: None,
            settings: editpad_core::Settings::default(),
            settings_path_override: None,
            settings_visible: false,
            settings_page: SettingsPage::default(),
            settings_search: String::new(),
            external_change: None,
            renaming_tab: None,
            rename_input: String::new(),
            encoding_menu: false,
            eol_menu: false,
            regex_enabled: false,
            whole_word: false,
            last_tab_click: None,
            hovered_tab: None,
            hotkey_capture: None,
            available_fonts: Vec::new(),
            active_font_family: None,
            ui_font_family: None,
            gutter_font_family: None,
            font_filter: String::new(),
            job_seq: 0,
            active_load: None,
            progress: None,
            pending_cli: VecDeque::new(),
            find_visible: false,
            find_query: String::new(),
            replace_query: String::new(),
            case_sensitive: false,
            matches: Vec::new(),
            match_idx: None,
            find_scan: None,
            find_seq: 0,
            find_cancel: Arc::default(),
            find_all_visible: false,
            find_pos: None,
            find_drag: None,
            find_cursor: Point::ORIGIN,
            find_status: false,
            find_dimmed: false,
            fif_visible: false,
            fif_dir: None,
            fif_seq: 0,
            fif_scan: None,
            fif_cancel: Arc::default(),
            fif_progress: Arc::new(AtomicUsize::new(0)),
            fif_results: Vec::new(),
            fif_truncated: false,
            pending_fif_goto: None,
            hl_paving: None,
            hl_pave_cancel: Arc::default(),
            goto_visible: false,
            goto_input: String::new(),
            recents_visible: false,
            confirm_visible: false,
            pending_close: false,
            main_window: None,
            fullscreen: false,
            palette_visible: false,
            palette_mode: PaletteMode::Commands,
            palette_input: String::new(),
            palette_idx: 0,
            column_editor_visible: false,
            column_editor: ColumnEditorDraft::default(),
            monitor_pending: None,
            pending_link_goto: None,
            always_on_top: false,
            open_confirm: None,
            close_tab_confirm: None,
            tab_context_menu: None,
            // P39/P40：浮层定位初值——锚点给一个可见的保守位置，
            // 尺寸未知 (0,0) = 贴边钳制跳过（启动后首个 Resized 事件校准）
            menu_anchor: (24.0, 56.0),
            cursor_pos: (0.0, 0.0),
            viewport_size: (0.0, 0.0),
            batch_close_confirm: None,
            pending_close_tab: None,
            // 第 64 轮：会话内「上次关闭」栈，启动为空（不跨重启）
            closed_stack: Vec::new(),
            menu_bar_open: None,
            menubar_pos: (12.0, 8.0),
            menubar_anchor: (12.0, 8.0),
            dark_mode: false,
            preview_visible: false,
            // P25：初始页即「未命名1」，下一个新页为「未命名2」
            untitled_next: 2,
            // P30：启动会话恢复状态（boot_restore 按清单填充）
            restore_queue: Vec::new(),
            restore_views: HashMap::new(),
            recover_prompt: None,
            restore_failed: 0,
            restore_dropped: 0,
            restore_pending: 0,
            // P31：周期快照心跳状态
            heartbeat_inflight: false,
            session_manifest_stale: false,
            manifest_rev: 0,
            snapshot_dir_override: None,
            last_geometry_persist: None,
        }
    }
}

impl Editpad {
    /// 写入普通信息状态（编辑动作可清除，中性配色渲染）。
    pub(crate) fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_is_error = false;
        // 已被别的状态顶替：查找进度标记失效（否则关查找栏会误清这条）
        self.find_status = false;
    }

    /// 写入错误状态（红色 ⚠ 渲染；编辑噪声不清除，直到下一条状态让位）。
    pub(crate) fn set_status_error(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_is_error = true;
        self.find_status = false;
    }

    /// 写入**查找进度**状态（「第 N/M 处匹配」）。与 [`Self::set_status`] 的
    /// 唯一差别是打上 [`Self::find_status`] 标记：关闭查找栏时据此清掉这条
    /// 自己写的提示（用户复报：关栏后左下角仍残留「第 1/23 处匹配」）。
    pub(crate) fn set_find_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_is_error = false;
        self.find_status = true;
    }
}


/// P129：命令面板模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaletteMode {
    /// 全部注册表命令（Ctrl+Shift+P）
    Commands,
    /// 当前会话标签页间跳转（Ctrl+P）
    Tabs,
}
