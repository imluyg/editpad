//! 视图层：应用主视图组装与各浮层/面板渲染（P158 自单文件 `view.rs` 拆为目录，
//! 纯移动零行为变更）。
//!
//! 本文件保留：
//! * 字体与语言 helper——`theme` / `ui_font` / `body_font` / `display_font_size` /
//!   `lang` / `t`，渲染层各处共用；
//! * **`view()` 主入口**——整棵应用视图树的组装，含菜单栏、标签条、正文区、
//!   状态栏与各浮层的叠加次序；
//! * 「查找全部」结果面板助手与浮层几何常量（`FIND_*`）、`enc_label`、
//!   `match_excerpt`、`PaletteEntry`。
//!
//! 分域 impl 见子模块：`lifecycle`（窗口关闭/退出与标题）、`snapshots`（心跳
//! 快照）、`restore`（会话恢复）、`scans`（扫描调度）、`find_ops`（命中跳转与
//! 替换）、`tab_menu`（标签右键面板）。
//!
//! 约定：子模块里的私有方法经 `pub(super)` 跨文件可见；共享导入在本文件
//! **`pub(crate)` 再导出**（私有 `use` 不跨模块传递）。**`column` 宏是例外**——
//! 宏经 glob 再导出会与 prelude 撞名（E0659：`column` is ambiguous），故它
//! 不进枢纽，由用到它的子模块各自显式 `use iced::widget::column;`。

pub(crate) use super::*;
use iced::widget::column;

// 按域拆分的 impl 块（纯移动零行为变更；私有方法经 pub(super) 跨文件可见）
mod find_ops;
mod lifecycle;
mod restore;
mod scans;
mod snapshots;
mod tab_menu;

pub(crate) use super::settings_ui::{
    chrome_button_style, chrome_menu_item_style, menubar_text_style, tab_close_style,
    tab_label_style, tab_pill_style,
};

impl Editpad {
    /// 应用主题（boot 从设置读入，工具栏可切换）。
    pub(crate) fn theme(&self) -> Theme {
        if self.dark_mode {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// 设置里归一后的当前字号（显示与按钮可用性判断都用它）。
    pub(crate) fn display_font_size(&self) -> f32 {
        editor::normalize_font_size(self.settings.font_size)
    }

    /// P154：UI 字形族（菜单栏/工具栏/标签栏/状态栏/查找框/设置弹窗/命令
    /// 面板等**除正文与预览之外**的一切文字）。
    ///
    /// 族由界面语言决定（启动期解析、切语言即重解析，见
    /// [`crate::fonts::pick_ui_font_family`]）；未命中任何候选 → None，
    /// 此时回落 [`Font::DEFAULT`]（iced 默认无衬线），不引入失败模式。
    /// 与正文字体完全解耦：改正文不再影响 UI，改语言不再影响正文。
    pub(crate) fn ui_font(&self) -> Font {
        match self.ui_font_family {
            Some(name) => fonts::family_font(name),
            None => Font::DEFAULT,
        }
    }

    /// P155：当前界面语言（全部用户可见文案的取值依据）。
    ///
    /// 单一来源 = `Settings.language`（P155 起是 [`editpad_core::Lang`]
    /// 枚举）。视图层每帧现取即可：`Lang` 是 `Copy`，且切语言只改这一个
    /// 字段，不存在「两处语言状态失步」的可能。
    pub(crate) fn lang(&self) -> editpad_core::Lang {
        self.settings.language
    }

    /// P155：取一条界面文案（`Key::text` 的薄封装，只为让视图层的调用
    /// 短一点：`self.t(Key::MenuFile)`）。
    pub(crate) fn t(&self, key: editpad_core::Key) -> &'static str {
        key.text(self.settings.language)
    }

    /// 正文字形族（P34 换装点，**只作用于正文与 Markdown 预览**）。
    /// 生效族名 Some → 以 `Family::Name` 引用（系统字体已在 fontdb 里，
    /// 无需装载字节）；None / 未配置 → 默认等宽（P33 的 CJK 钉字仍生效）。
    /// iced 排版缓存按 Font 值做键——运行期换族即换键，无陈旧缓存问题。
    /// P154 拆分：本体不再供 UI 使用（UI 走 [`Self::ui_font`]）。
    pub(crate) fn body_font(&self) -> Font {
        match self.active_font_family {
            Some(name) => Font {
                family: iced::font::Family::Name(name),
                ..editor::BODY_FONT
            },
            None => editor::BODY_FONT,
        }
    }


    // ---------- 周期快照心跳（P31） ----------







    // ---------- 关窗流（P29 快照直退） ----------





    // ---------- 启动会话恢复（P30） ----------











    // ---------- 查找 / 替换内部逻辑 ----------













    // ---------- 展示辅助 ----------





    // 设置面板渲染（settings_panel/sidebar/content/row_widget/stepper/font_picker 等 8 方法）已迁往 settings_ui.rs（第 82 轮 Phase 2a）

pub(crate) fn view(&self) -> Element<'_, Message> {
        // P22 第三批：当前页是否为 Markdown（决定预览按钮可用性）
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
        // P33/P36：UI 全部控件与正文同族，字号固定不随正文缩放——
        // Ctrl+滚轮（P48）与设置面板只调节文件内容；P34：族随设置切换
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();

        // 第 69 轮：顶部菜单栏（文件/编辑/查看/视图/设置）——整条包
        // mouse_area 跟踪指针作浮层锚点（仿标签条 P39 模式）；展开的
        // 菜单以高亮态标示。功能项自工具栏收编迁移（见各 menubar_panel）。
        // 第 76 轮（用户点单）：按钮改纯文字扁平（menubar_text_style，
        // 无背景无边框无凸起），行高压缩（按钮 padding [1,8] + 行内
        // padding [0,4]）——看起来不像按钮、更紧凑。
        let menu_names = [
            editpad_core::Key::MenuFile,
            editpad_core::Key::MenuEdit,
            editpad_core::Key::MenuView,
            editpad_core::Key::MenuSettings,
        ];
        let mut menubar_inner = row![].spacing(4);
        for (idx, name) in menu_names.iter().enumerate() {
            let open = self.menu_bar_open == Some(idx);
            menubar_inner = menubar_inner.push(
                button(
                    text(self.t(*name)).size(uipx).font(uifont),
                )
                .padding([1, 8])
                .style(move |theme, status| menubar_text_style(theme, status, open))
                .on_press_maybe((!self.busy).then_some(Message::MenuToggled(idx))),
            );
        }
        let menubar = mouse_area(
            menubar_inner.padding([0, 4]),
        )
        .on_move(Message::MenubarHovered);

        // 第 70 轮：工具栏整体移除——打开/保存/查找/设置全部由顶部菜单
        // 栏承载（用户裁决：与文件菜单重复）；置脏指示由标签页 ● 前缀
        // （P21）与标题栏承担。查找/替换快捷入口 = Ctrl+F 与编辑菜单。

        // P21 标签条：恒显示（单页也给出「当前文件名」的可见反馈）。
        // 点击切换；置脏页带 ● 前缀；活动页加 ▸ 指示；固定页加 📌（P28）。
        // P28：按钮外包 mouse_area 捕获右键弹菜单——MouseArea 先把事件
        // 交给子组件，左键被 button 捕获后自身跳过，故切换不受影响；
        // 右键无人捕获，落到 on_right_press。
        // P39：整条标签条再包一层 mouse_area 跟踪指针位置（浮层菜单
        // 锚点数据源；只在标签条悬停时产生消息，量级可忽略）。
        let mut body = column![menubar, rule::horizontal(1)];
        {
            // P100 勘误：P98 把「空白双击」命中面从 Fill 占位按钮改到外层
            // mouse_area 时，strip Row 默认 Shrink 宽——外层鼠标区只盖住
            // 标签本身，右侧空白不在命中面内，双击无消息。补 width(Fill)
            // 让整条标签条横贯窗口（无任何 Fill 高子项，P98 的半窗回归
            // 不会重现），外层 on_double_click 全覆盖右侧空白。
            let mut strip = row![].spacing(2).padding([4, 6]).width(Fill);
            // 注：窗口标题栏图标见 [`crate::window_title_icon`]（main.rs）；
            // 标签条保持纯文本前缀（▸ 活动 / ● 置脏 / 📌 固定，P21/P28
            // 语义，用户点名「标签栏黑点保持原样」不动）。
            for (i, tab) in self.tabs.iter().enumerate() {
                // P55：就地重命名——该页的标签按钮替换为输入框 + ✓/× 微型按钮
                // （Enter 等价 ✓；Esc 走 BarsDismissed 取消）
                if self.renaming_tab == Some(i) {
                    strip = strip.push(
                        row![
                            text_input(self.t(editpad_core::Key::RenamePlaceholder), &self.rename_input)
                                .id(rename_input_id()) // P64：与聚焦操作同源
                                .size(uipx)
                                .font(uifont)
                                .on_input(Message::TabRenameInputChanged)
                                .on_submit(Message::TabRenameCommitted)
                                .width(150),
                            button(text("✓").size(uipx).font(uifont))
                                .padding([2, 7])
                                .style(chrome_button_style)
                                .on_press(Message::TabRenameCommitted),
                            button(text("×").size(uipx).font(uifont))
                                .padding([2, 7])
                                .style(chrome_button_style)
                                .on_press(Message::TabRenameCancelled),
                        ]
                        .spacing(2)
                        .align_y(Alignment::Center),
                    );
                    continue;
                }
                let marker = if i == self.active_tab { "▸ " } else { "  " };
                let pin = if tab.pinned { "📌 " } else { "" };
                // P57：页签换中性样式——活动页淡底描边、非活动透明悬停淡染
                let active = i == self.active_tab;
                // P112：页签 = 「胶囊容器 + 文字按钮 + × 关闭按钮」。
                // 胶囊底画在容器上（活动/悬停染色），内部两个按钮全透明
                // 扁平——若底色由两个按钮各自画会露接缝；悬停数据源 =
                // 指针进出页签的 TabHovered（纯 UI 态）。× 与右键菜单
                // 「关闭」同消息同口径（置脏弹确认条、busy 置灰），固定
                // 页不渲染 ×（豁免口径与菜单一致）；文字按钮仍承接
                // SwitchTab（含 P65 双击重命名）。
                let closeable = !tab.pinned;
                let mut pill = row![
                    button(
                        text(format!(
                            "{marker}{pin}{}",
                            tab.display_name()
                        ))
                        .size(uipx)
                        .font(uifont),
                    )
                    .padding(Padding {
                        top: 2.0,
                        right: if closeable { 4.0 } else { 10.0 },
                        bottom: 2.0,
                        left: 10.0,
                    })
                    .style(tab_label_style)
                    .on_press_maybe((!self.busy).then_some(Message::SwitchTab(i))),
                ]
                .align_y(Alignment::Center);
                if closeable {
                    pill = pill.push(
                        button(text("×").size(uipx).font(uifont))
                            .padding([2, 8])
                            .style(tab_close_style)
                            .on_press_maybe(
                                (!self.busy).then_some(Message::CloseTabAt(i)),
                            ),
                    );
                }
                let hovered = self.hovered_tab == Some(i);
                let pill_area = mouse_area(
                    container(pill)
                        .style(move |theme| tab_pill_style(theme, active, hovered)),
                )
                .on_enter(Message::TabHovered(Some(i)))
                .on_exit(Message::TabHovered(None))
                .on_right_press(Message::TabContextMenu(i));
                // 中键关闭（主流浏览器/编辑器手感）；置脏确认与固定页
                // 豁免由 CloseTabAt 处理器统一判定，busy 守卫与 × 按钮同口径
                let pill_area = if self.busy {
                    pill_area
                } else {
                    pill_area.on_middle_press(Message::CloseTabAt(i))
                };
                strip = strip.push(pill_area);
            }
            // 第 76 轮（用户点单）：标签条右侧空白区双击新建标签页——
            // 命中面就是整条标签条本身（外层 mouse_area 的 on_double_click）：
            // 标签按钮/重命名输入框会先消费左键（iced 事件委托语义，P28
            // 同款），空白区无子组件消费 → 双击判定落在外层。因此**不**
            // 需要额外的 Fill 按钮占位（此前版本用 Space(Fill, Fill) 撑高
            // 了标签条、占半窗——已移除）。双击现有标签不触发（其 click
            // 被 button 消费），与 P65 重命名互不干扰。
            body = body.push(
                mouse_area(strip)
                    .on_move(Message::CursorMoved)
                    .on_double_click(Message::TabStripBlankPressed),
            );
        }

        // 中间主区域：Markdown 预览面板 或 自绘虚拟化编辑器
        // P34：两路都按设置的字形族渲染（预览属内容渲染随族切换；
        // 画布经 view_with_font 每帧传参，无跨帧同步状态）
        let content_font = self.body_font();
        if self.preview_visible && is_markdown {
            let text = self.cur_handle.borrow().doc.to_text();
            body = body.push(markdown_preview_element(
                &text,
                self.display_font_size(),
                content_font,
            ));
        } else {
            body = body.push(self.cur_handle.view(content_font));
        }

        if let Some((bytes_read, total_bytes)) = self.progress {
            body = body.push(
                row![
                    text(self.t(editpad_core::Key::RecentsLoading)).size(uipx).font(uifont),
                    container(
                        progress_bar(0.0..=total_bytes.max(1) as f32, bytes_read as f32)
                    )
                    .width(Fill),
                    text(format!(
                        "{} / {} KB",
                        bytes_read / 1024,
                        total_bytes.max(1) / 1024
                    ))
                    .size(uipx)
                    .font(uifont),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if self.recents_visible {
            let mut panel = column![].spacing(2).padding([4, 10]);
            if self.settings.recent_files.is_empty() {
                panel = panel.push(
                    text(self.t(editpad_core::Key::RecentsEmpty))
                        .size(uipx)
                        .font(uifont)
                        .color([0.5, 0.5, 0.5]),
                );
            }
            for entry in &self.settings.recent_files {
                panel = panel.push(
                    button(container(text(entry).size(uipx).font(uifont)).width(Fill))
                        .width(Fill)
                        .style(chrome_menu_item_style)
                        .on_press_maybe(
                            (!self.busy).then_some(Message::RecentSelected(entry.clone())),
                        ),
                );
            }
            // 第 64 轮：会话内「上次关闭」栈非空时，置顶提供一键恢复
            if !self.closed_stack.is_empty() {
                panel = panel.push(
                    button(
                        container(text(format!(
                            "{}{}{}",
                            self.t(editpad_core::Key::RecentReopenPrefix),
                            self.closed_stack[0]
                                .file_name()
                                .map(|n| n.display().to_string())
                                .unwrap_or_else(|| self.closed_stack[0].display().to_string()),
                            self.t(editpad_core::Key::RecentReopenSuffix)
                        ))
                        .size(uipx)
                        .font(uifont))
                        .width(Fill),
                    )
                    .width(Fill)
                    .style(chrome_menu_item_style)
                    .on_press_maybe(
                        (!self.busy).then_some(Message::ReopenLastClosedFile),
                    ),
                );
            }
            // P20 隐私出口：一键抹掉 config.toml 里的全部历史路径
            if !self.settings.recent_files.is_empty() {
                panel = panel.push(
                    row![
                        button(text(self.t(editpad_core::Key::RecentsClear)).size(uipx).font(uifont))
                            .padding([2, 8])
                            .style(chrome_button_style)
                            .on_press_maybe((!self.busy).then_some(Message::RecentsCleared)),
                        text(self.t(editpad_core::Key::RecentsClearHint))
                            .size(uipx)
                            .font(uifont)
                            .color([0.5, 0.5, 0.5]),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                );
            }
            body = body.push(rule::horizontal(1)).push(panel);
        }

        // P150：查找/替换/结果面板不再作为底部停靠行挤占正文——改由
        // view 尾部的居中轻浮层承载（find_overlay + Stack 组装处）。
        // 正文可用面积不再随查找栏开关变化。

        if self.goto_visible {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(self.t(editpad_core::Key::GotoTitle)).size(uipx).font(uifont),
                    text_input(self.t(editpad_core::Key::GotoPlaceholder), &self.goto_input)
                        .size(uipx)
                        .font(uifont)
                        .on_input(Message::GotoInputChanged)
                        .on_submit(Message::GotoSubmit)
                        .width(140),
                    button(text(self.t(editpad_core::Key::GotoButton)).size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press(Message::GotoSubmit),
                    button(text("×").size(uipx).font(uifont))
                        .style(chrome_button_style)
                        .on_press(Message::GotoToggled),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P27 设置弹窗：P40 起为居中浮层模态（见 view 尾部 Stack 组装），
        // 不再内嵌推进 body 列。

        // 未保存关闭确认条：置于状态区域上方
        if self.confirm_visible {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(self.t(editpad_core::Key::CloseConfirmDirty))
                        .size(uipx)
                        .font(uifont),
                    button(text(self.t(editpad_core::Key::ButtonSaveAndClose)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press_maybe(
                            (!self.busy).then_some(Message::ConfirmSaveAndClose)
                        ),
                    button(text(self.t(editpad_core::Key::ButtonDiscardChanges)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::DiscardAndClose),
                    button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::CancelClose),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P21 标签页关闭确认：骨架版仅提供「放弃更改并关闭」出口，
        // 想保留改动请先 Ctrl+S（完整保存后关闭随 P21 完整版补齐）
        if let Some(idx) = self.close_tab_confirm {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!(
                        "{}{}{}",
                        self.t(editpad_core::Key::TabIndexDirtyPrefix),
                        idx.saturating_add(1),
                        self.t(editpad_core::Key::TabIndexDirtyMiddle),
                    ))
                    .size(uipx)
                    .font(uifont),
                    button(text(self.t(editpad_core::Key::ButtonDiscardAndClose)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmCloseTabDiscard(idx)),
                    // P21 完整版：已命名的页可直接「保存并关闭」
                    button(text(self.t(editpad_core::Key::ButtonSaveAndClose)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press_maybe(
                            // P145：夹紧兜底——确认条下标随关页平移/清理
                            //（close_tab_now），此处 get 防未来回归越界
                            (!self.busy
                                && self.tabs.get(idx).is_some_and(|t| t.path.is_some()))
                            .then_some(Message::CloseTabSave(idx)),
                        ),
                    button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::CancelCloseTab),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P28 批量关闭确认条：关闭其他/右侧的目标中有置脏页时弹一次，
        // 确认后统一放弃并移除（固定页本就不在目标列表内）
        if let Some(targets) = &self.batch_close_confirm {
            let total = targets.len();
            let dirty = targets
                .iter()
                .filter(|&&i| self.tabs.get(i).is_some_and(|t| t.dirty))
                .count();
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(format!(
                        "{}{}{}{}{}",
                        self.t(editpad_core::Key::BatchCloseConfirmPrefix),
                        total,
                        self.t(editpad_core::Key::BatchCloseConfirmMiddle),
                        dirty,
                        self.t(editpad_core::Key::BatchCloseConfirmMiddle2),
                    ))
                    .size(uipx)
                    .font(uifont),
                    button(text(self.t(editpad_core::Key::ButtonDiscardAndClose)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmBatchCloseDiscard),
                    button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::CancelBatchCloseTabs),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // 未保存时打开新文件的确认条（样式沿用关闭确认条）
        if let Some(path) = &self.open_confirm {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(editpad_core::fmt_wrapped(
                        self.lang(),
                        editpad_core::Key::DiscardOpenPrefix,
                        &path.display().to_string(),
                        editpad_core::Key::DiscardOpenSuffix,
                    ))
                    .size(uipx)
                        .font(uifont),
                    button(text(self.t(editpad_core::Key::ButtonDiscardAndOpen)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmOpenDiscard),
                    button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::ConfirmOpenCancel),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        // P50/P52 外部修改提示条：队列首页展示，聚合时带总数与「全部忽略」。
        // 下标失效（页已关）时跳过，等下次聚焦重算。
        if let Some(queue) = self.external_change.as_ref() {
            if let Some(first) = queue.first().copied() {
                if let Some(tab) = self.tabs.get(first) {
                    if tab.path.is_some() {
                        let total = queue.len();
                        let mut bar = row![
                            text(if total > 1 {
                                format!(
                                    "{}{}{}{}{}",
                                    self.t(editpad_core::Key::TabHeaderPrefix),
                                    tab.display_name(),
                                    self.t(editpad_core::Key::ExternalModifiedFilesPrefix),
                                    total,
                                    self.t(editpad_core::Key::ExternalModifiedFilesSuffix),
                                )
                            } else {
                                format!(
                                    "{}{}{}",
                                    self.t(editpad_core::Key::TabHeaderPrefix),
                                    tab.display_name(),
                                    self.t(editpad_core::Key::ExternalModifiedOneSuffix),
                                )
                            })
                            .size(uipx)
                            .font(uifont),
                            button(text(self.t(editpad_core::Key::ButtonReload)).size(uipx).font(uifont))
                                .padding([4, 12])
                                .style(chrome_button_style)
                                .on_press(Message::ConfirmExternalReload(first)),
                            button(text(self.t(editpad_core::Key::ButtonIgnore)).size(uipx).font(uifont))
                                .padding([4, 12])
                                .style(chrome_button_style)
                                .on_press(Message::IgnoreExternalChange(first)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .padding([6, 10]);
                        if total > 1 {
                            bar = bar.push(
                                button(text(self.t(editpad_core::Key::ButtonIgnoreAll)).size(uipx).font(uifont))
                                    .padding([4, 12])
                                    .style(chrome_button_style)
                                    .on_press(Message::IgnoreAllExternalChanges),
                            );
                        }
                        body = body.push(rule::horizontal(1)).push(bar);
                    }
                }
            }
        }

        // P30 崩溃恢复一次性提示条：上次未正常收尾（崩溃/P31 心跳中间态）。
        // 「恢复」按快照全量重建；「丢弃」连快照一起丢。数据原封留在磁盘，
        // 不裁决就一直挂着——与「未保存确认条」同级的强提醒语义。
        if self.recover_prompt.is_some() {
            body = body.push(rule::horizontal(1)).push(
                row![
                    text(self.t(editpad_core::Key::RestoreTitle))
                        .size(uipx)
                        .font(uifont),
                    button(text(self.t(editpad_core::Key::ButtonRestore)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::SessionRecoverAccepted),
                    button(text(self.t(editpad_core::Key::ButtonDiscard)).size(uipx).font(uifont))
                        .padding([4, 12])
                        .style(chrome_button_style)
                        .on_press(Message::SessionRecoverDiscarded),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .padding([6, 10]),
            );
        }

        if !self.status.is_empty() {
            // 错误（红色 ⚠）与普通信息（主题文字色，无警号）区分渲染：
            // 信息持久化后满屏红字会放大无关紧要的提示
            let (icon, color) = if self.status_is_error {
                ("⚠ ", [0.9, 0.25, 0.25])
            } else {
                let palette = if self.dark_mode {
                    Theme::Dark.palette()
                } else {
                    Theme::Light.palette()
                };
                let Color { r, g, b, .. } = palette.text;
                ("", [r, g, b])
            };
            body = body.push(
                container(
                    text(format!("{icon}{}", self.status))
                        .size(uipx)
                        .font(uifont)
                        .color(color),
                )
                .padding([4, 10]),
            );
        }

        let cursor = self.cur_handle.borrow().cursor;
        // P67：行尾短标签（主导行尾随文档实时读取）
        let eol = eol_label(self.cur_handle.borrow().doc.line_ending());
        // 第 70 轮：状态栏再排——文件路径移到**最左**并定宽截断（超长
        // 加省略号），随后竖线分隔：路径 │ 统计组(弹性吸收) │ 右组
        // （选区定宽/行尾/编码）。两条竖线位置固定，两侧互不影响。
        // ⚠️ 第 71 轮勘误：竖线禁用 rule::vertical——iced 0.14 的
        // vertical rule 在 Row 内长度默认 Fill，会把整条状态栏撑满剩余
        // 高度（用户实测截图暴露）；改为 1px 定宽定高的 container 竖条。
        let palette = if self.dark_mode {
            Theme::Dark.palette()
        } else {
            Theme::Light.palette()
        };
        let sep_color = Color { a: 0.30, ..palette.text };
        // 第 76 轮：状态栏行高与顶部菜单栏一致压缩（竖条 20→14px 随行高
        // 同缩；整行 padding [6,10]→[1,8]）
        let sep_v = move || -> Element<'_, Message> {
            container(text(""))
                .width(1)
                .height(14)
                .style(move |_: &Theme| {
                    container::Style {
                        background: Some(Background::Color(sep_color)),
                        ..container::Style::default()
                    }
                })
                .into()
        };
        let (doc_chars, line_count, sel_chars, cur_off) = {
            let ed = self.cur_handle.borrow();
            (
                ed.doc.text_len(),
                ed.doc.line_count(),
                ed.selection_display_len(),
                ed.cursor_offset(),
            )
        };
        let path_full = self
            .tab()
            .path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("({})", self.tab().base_name_in(self.lang())));
        let path_show = {
            const MAX: usize = 40;
            let chars: Vec<char> = path_full.chars().collect();
            if chars.len() > MAX {
                format!("…{}", chars[chars.len() - MAX..].iter().collect::<String>())
            } else {
                path_full.clone()
            }
        };
        let status_bar = row![
            text(path_show).size(uipx).font(uifont).width(280),
            sep_v(),
            text(format!("{}: {doc_chars}", self.t(editpad_core::Key::StatusLength)))
                .size(uipx)
                .font(uifont),
            text(format!("{}: {line_count}", self.t(editpad_core::Key::StatusLines)))
                .size(uipx)
                .font(uifont),
            text(format!("{}: {}", self.t(editpad_core::Key::StatusLine), cursor.line + 1))
                .size(uipx)
                .font(uifont),
            text(format!("{}: {}", self.t(editpad_core::Key::StatusColumn), cursor.col + 1))
                .size(uipx)
                .font(uifont),
            text(format!("{}: {cur_off}", self.t(editpad_core::Key::StatusPosition)))
                .size(uipx)
                .font(uifont),
            // 弹性段吸收中部余量：左右两组竖线位置恒定
            text("").width(Fill),
            sep_v(),
            // 选区段：定宽占位（无选区显示占位空白），保证右侧组零推移
            container(
                text(match sel_chars {
                    Some(n) => editpad_core::fmt_selection(self.lang(), n),
                    None => String::new(),
                })
                .size(uipx)
                .font(uifont)
            )
            .width(110)
            .align_x(iced::alignment::Horizontal::Right),
            button(text(eol).size(uipx).font(uifont))
                .padding([1, 6])
                .style(chrome_button_style)
                .on_press_maybe((!self.busy).then_some(Message::ToggleEolMenu)),
            button(
                text(if self.tab().encoding_label.is_empty() {
                    "—".to_owned()
                } else {
                    self.tab().encoding_label.clone()
                })
                .size(uipx)
                .font(uifont)
            )
            .padding([1, 6])
            .style(chrome_button_style)
            .on_press_maybe((!self.busy).then_some(Message::ToggleEncodingMenu)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([1, 8]);

        body = body.push(rule::horizontal(1)).push(status_bar);

        // P39/P40：浮层经 Stack 叠加——首层（正文）定尺寸，顶层浮在
        // 其上、不占布局空间（iced_widget-0.14.2 Stack 语义），正文可用
        // 面积不再被菜单/设置弹窗挤占。opaque 背板捕获层内点击，
        // 防止点菜单外的落穿透到正文。
        let base: Element<'_, Message> = container(body).width(Fill).height(Fill).into();
        let mut layered = Stack::new().push(base);
        // 第 69 轮：顶部菜单栏浮层（最上层优先渲染）
        if let Some(idx) = self.menu_bar_open {
            layered = layered.push(self.menubar_overlay(idx));
        }
        if let Some(idx) = self.tab_context_menu {
            if idx < self.tabs.len() {
                layered = layered.push(self.context_menu_overlay(idx));
            }
        }
        // P150：查找/替换轻浮层（居中定宽、无背板、可继续编辑；Esc/× 关闭）
        if self.find_visible {
            layered = layered.push(self.find_overlay(uipx, uifont));
        }
        if self.settings_visible {
            layered = layered.push(self.settings_overlay());
        }
        // P129：命令面板/快速标签切换浮层（顶部居中卡片）
        if self.palette_visible {
            layered = layered.push(self.palette_overlay());
        }
        // B9：列编辑器对话框（居中模态；与设置/面板互斥由 update 保证）
        if self.column_editor_visible {
            layered = layered.push(self.column_editor_overlay());
        }
        // P67：状态栏编码/行尾弹出菜单（互斥，update 层保证）
        if self.encoding_menu {
            layered = layered.push(self.encoding_menu_overlay());
        }
        if self.eol_menu {
            layered = layered.push(self.eol_menu_overlay());
        }
        layered.into()
    }

    // ---------- 浮层弹窗（P39/P40） ----------

    /// P39：标签右键菜单浮层——整窗透明背板（点击即收起）+ 锚在指针
    /// 位置、贴边钳制后的菜单卡片。opaque 双层防穿透：背板捕获菜单外
    /// 点击不落到正文，卡片捕获卡片内空白处点击不触发背板关闭。
    /// P43：卡片高度按窗口钳制（小窗口不溢出、不盖满全屏），内容超高
    /// 时内部滚动——与设置弹窗同款适配策略。
    fn context_menu_overlay(&self, idx: usize) -> Element<'_, Message> {
        let vh = self.viewport_size.1;
        let card_h = ctx_menu_card_h(vh);
        let (ax, ay) = clamp_menu_anchor(
            self.menu_anchor,
            self.viewport_size,
            CTX_MENU_W,
            card_h,
        );
        let card = opaque(
            // P56：宽度必须定宽 CTX_MENU_W——P46 的 Shrink 修法在 iced
            // 布局下不成立（Fill 子控件在 Shrink 测量中会撑到窗口宽，
            // 用户截图实测卡片 ~970px）；定宽与贴边钳制共用同一常量，
            // 估宽即实宽，不再有漂移
            container(
                scrollable(self.tab_context_panel(idx))
                    .height(card_h)
                    .width(CTX_MENU_W),
            )
            .padding(4)
            .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
        )
        .on_press(Message::TabContextMenuClosed)
        .on_right_press(Message::TabContextMenuClosed)
        .into()
    }

    // ---------- 第 69 轮：顶部菜单栏浮层 ----------

    /// 菜单栏浮层：整窗透明背板 + 锚在触发按钮槽位下方的卡片。
    ///
    /// 第 70 轮修订（用户反馈「弹窗会移动」）：
    /// * 锚点读**展开瞬间冻结**的 menubar_anchor，且 x 对齐到按钮槽位
    ///   左缘（menubar_slot_idx 反推）——同一菜单无论点按钮哪个部位、
    ///   展开期间鼠标怎么动，浮层位置恒定；
    /// * 背板 on_move 跟踪指针 + on_press 时若落在菜单栏条带内 → 视为
    ///   点击另一菜单（MenuToggled 切换，主流横移手感），否则收起。
    fn menubar_overlay(&self, idx: usize) -> Element<'_, Message> {
        const W: f32 = 240.0;
        let card_h = ctx_menu_card_h(self.viewport_size.1).min(380.0);
        let slot_x = MENU_BAR_LEFT + menubar_slot_idx(self.menubar_anchor.0) as f32 * MENU_SLOT_W;
        let (ax, ay) = clamp_menu_anchor(
            (slot_x, MENU_BAR_H),
            self.viewport_size,
            W,
            card_h,
        );
        let ay = ay.max(MENU_BAR_H); // 永不遮住菜单栏本身
        let card = opaque(
            container(
                scrollable(self.menubar_panel(idx))
                    .width(W)
                    .height(card_h),
            )
            .padding(4)
            .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
        )
        .on_move(Message::MenubarHovered)
        .on_press(Message::MenubarPressed)
        .into()
    }

    /// 第 `idx` 个菜单的面板内容。全部复用既有消息（零新编辑逻辑）；
    /// 守卫口径与原工具栏按钮一致（busy / dirty / is_markdown）。
    fn menubar_panel(&self, idx: usize) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let interactive = !self.busy;
        let item = |label: String, msg: Option<Message>| {
            button(container(text(label).size(uipx).font(uifont)).width(Fill))
                .width(Fill)
                .padding([5, 10])
                .style(chrome_menu_item_style)
                .on_press_maybe(msg)
        };
        // P155：菜单项文案组装——「（✓ ）<当前语言的主标签>  <键位提示>」。
        //
        // 键位提示（`Ctrl+S` 之类）是**快捷键字面量**，不随界面语言变化，
        // 故与翻译无关；勾选前缀按开关态决定。抽出这一处是为了让菜单项
        // 在两种语言下排版一致（标签与键位之间恒为两个空格）。
        fn item_label(
            lang: editpad_core::Lang,
            key: editpad_core::Key,
            checked: bool,
            combo: Option<&str>,
        ) -> String {
            let mut s = String::with_capacity(48);
            if checked {
                s.push_str("✓ ");
            }
            s.push_str(key.text(lang));
            if let Some(combo) = combo {
                s.push_str("  ");
                s.push_str(combo);
            }
            s
        }
        let lang = self.lang();
        let sep = || rule::horizontal(1);
        let is_markdown = self
            .cur_handle
            .borrow()
            .highlight_syntax_name()
            .as_deref()
            == Some("Markdown");
        let mut panel = column![].spacing(2).padding([4, 6]);
        match idx {
            // ---------- 文件 ----------
            0 => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuOpen, false, Some("Ctrl+O")),
                        interactive.then_some(Message::OpenRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuSave, false, Some("Ctrl+S")),
                        (interactive && self.tab().dirty).then_some(Message::SaveRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuSaveAs, false, None),
                        interactive.then_some(Message::SaveAsRequested),
                    ))
                    .push(sep())
                    .push(item(
                        format!(
                            "{}{}",
                            item_label(
                                lang,
                                editpad_core::Key::MenuReopenClosed,
                                false,
                                Some("Ctrl+Shift+W")
                            ),
                            if self.closed_stack.is_empty() {
                                self.t(editpad_core::Key::RecentsEmpty)
                            } else {
                                ""
                            }
                        ),
                        (!self.busy && !self.closed_stack.is_empty())
                            .then_some(Message::ReopenLastClosedFile),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuRecents, false, None),
                        interactive.then_some(Message::RecentsToggled),
                    ));
            }
            // ---------- 编辑 ----------
            1 => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuUndo, false, Some("Ctrl+Z")),
                        interactive.then_some(Message::Edit(EditOp::Undo)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuRedo, false, Some("Ctrl+Y")),
                        interactive.then_some(Message::Edit(EditOp::Redo)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuCut, false, Some("Ctrl+X")),
                        interactive.then_some(Message::CutRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuCopy, false, Some("Ctrl+C")),
                        interactive.then_some(Message::CopyRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuPaste, false, Some("Ctrl+V")),
                        interactive.then_some(Message::PasteRequested),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuSelectAll, false, Some("Ctrl+A")),
                        interactive.then_some(Message::Edit(EditOp::SelectAll)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuFind, false, Some("Ctrl+F")),
                        interactive.then_some(Message::FindToggled),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuGoto, false, Some("Ctrl+G")),
                        interactive.then_some(Message::GotoToggled),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuInsertDateTime, false, Some("F5")),
                        interactive.then_some(Message::Edit(EditOp::InsertDateTime)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuToggleComment, false, Some("Ctrl+Q")),
                        interactive.then_some(Message::Edit(EditOp::ToggleLineComment)),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuColumnEditor, false, Some("F6")),
                        interactive.then_some(Message::ColumnEditorToggled),
                    ));
            }
            // ---------- 查看 ----------
            2 => {
                let step = editor::FONT_ZOOM_STEP;
                panel = panel
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuZoomIn,
                            false,
                            Some(editpad_core::Key::ComboWheel.text(lang)),
                        ),
                        interactive.then_some(Message::FontSizeDelta(step)),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuZoomOut,
                            false,
                            Some(editpad_core::Key::ComboWheel.text(lang)),
                        ),
                        interactive.then_some(Message::FontSizeDelta(-step)),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuZoomReset, false, None),
                        interactive.then_some(Message::FontSizeDelta(
                            16.0 - self.display_font_size(),
                        )),
                    ))
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuThemeToggle, false, None),
                        interactive.then_some(Message::ThemeToggled),
                    ))
                    .push(sep())
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuShowWhitespace,
                            self.settings.show_whitespace,
                            None,
                        ),
                        interactive.then_some(Message::SettingsShowWhitespaceToggled(
                            !self.settings.show_whitespace,
                        )),
                    ))
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuShowLineEndings,
                            self.settings.show_line_endings,
                            None,
                        ),
                        interactive.then_some(Message::SettingsShowLineEndingsToggled(
                            !self.settings.show_line_endings,
                        )),
                    ))
                    // 第 73 轮 ⑯：自动换行开关（查看菜单入口，与设置页同消息）
                    .push(item(
                        item_label(
                            lang,
                            editpad_core::Key::MenuWordWrap,
                            self.settings.word_wrap,
                            None,
                        ),
                        interactive.then_some(Message::SettingsWordWrapToggled(
                            !self.settings.word_wrap,
                        )),
                    ))
                    // P134（C7）：本页自动换行三态循环——跟随全局/本页开/
                    // 本页关；有覆盖时全局项旁标注（标签条目自身即状态显示）
                    .push(item(
                        match self.tab().wrap_override {
                            None => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapFollow,
                                false,
                                None,
                            ),
                            Some(true) => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapOn,
                                true,
                                None,
                            ),
                            Some(false) => item_label(
                                lang,
                                editpad_core::Key::MenuTabWrapOff,
                                false,
                                None,
                            ),
                        },
                        interactive.then_some(Message::TabWrapOverrideToggled),
                    ))
                    // P134（C7）：本页字号重置——仅在有覆盖时可用
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuTabFontReset, false, None),
                        (interactive && self.tab().font_size_override.is_some())
                            .then_some(Message::TabFontSizeReset),
                    ))
                    // 原「视图」菜单并入：MD 预览按当前语法门控（最近文件
                    // 面板与文件菜单的「最近文件」重复，不再单列）
                    .push(sep())
                    .push(item(
                        if self.preview_visible {
                            item_label(lang, editpad_core::Key::MenuPreviewClose, false, None)
                        } else {
                            item_label(lang, editpad_core::Key::MenuPreview, false, None)
                        },
                        (interactive && is_markdown).then_some(Message::PreviewToggled),
                    ));
            }
            // ---------- 设置 ----------
            _ => {
                panel = panel
                    .push(item(
                        item_label(lang, editpad_core::Key::MenuOpenSettings, false, None),
                        interactive.then_some(Message::SettingsToggled),
                    ))
                    .push(item(
                        format!(
                            "{}：{}",
                            self.t(editpad_core::Key::MenuBackupMode),
                            match self.settings.backup_mode.as_str() {
                                editpad_core::settings::BACKUP_MODE_SIMPLE => {
                                    self.t(editpad_core::Key::BackupSimpleShort)
                                }
                                editpad_core::settings::BACKUP_MODE_TIMESTAMPED => {
                                    self.t(editpad_core::Key::BackupTimestampedShort)
                                }
                                _ => self.t(editpad_core::Key::BackupOff),
                            }
                        ),
                        interactive.then_some(Message::SettingsBackupModeToggled),
                    ));
            }
        }
        panel.into()
    }

    // 第 62 轮：「查找全部」结果面板（停靠式，非浮层）——数据源 =
    // 查找后台扫描的全量命中表，重扫刷新自动跟随。渲染行数封顶
    // FIND_ALL_MAX_ROWS：chrome 行按钮无虚拟化，10 万级命中全量
    // 渲染会拖垮帧率，超出部分明示截断提示。
    //
    // ---------- P129：命令面板 / 快速标签切换 ----------

    /// 面板全部条目（未过滤）：命令模式 = 注册表全量；标签模式 = 当前
    /// 会话全部页。title 参与模糊匹配，detail 仅展示。
    pub(crate) fn palette_all_entries(&self) -> Vec<crate::view::PaletteEntry> {
        match self.palette_mode {
            crate::state::PaletteMode::Commands => palette_commands(self.lang())
                .into_iter()
                .map(|c| PaletteEntry {
                    command_id: Some(c.id),
                    tab_index: None,
                    title: c.title.to_owned(),
                    detail: c.detail,
                })
                .collect(),
            crate::state::PaletteMode::Tabs => self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| PaletteEntry {
                    command_id: None,
                    tab_index: Some(i),
                    title: t.display_name(),
                    detail: t
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| self.t(editpad_core::Key::TabUnsaved).to_owned()),
                })
                .collect(),
        }
    }

    /// 面板过滤条目（fuzzy_filter 稳定降序）。查询串对 title 与 detail
    /// 拼接匹配——命令可用 id 片段检索（如 readonly），标签可用路径检索。
    pub(crate) fn palette_filtered(&self) -> Vec<PaletteEntry> {
        let pairs: Vec<(PaletteEntry, String)> = self
            .palette_all_entries()
            .into_iter()
            .map(|e| {
                let hay = format!("{} {}", e.title, e.detail);
                (e, hay)
            })
            .collect();
        fuzzy_filter(&pairs, &self.palette_input)
            .into_iter()
            .map(|(e, _)| e)
            .collect()
    }

    /// 执行当前选中条目并关闭面板。命令经 dispatch_action 复用既有
    /// 映射（空修饰键——Shift 选区透传不适用面板执行）；标签模式直接
    /// SwitchTab；列表为空时 no-op（面板保持打开）。
    pub(crate) fn palette_execute(&mut self) -> Task<Message> {
        let entries = self.palette_filtered();
        if entries.is_empty() {
            return Task::none();
        }
        let idx = self.palette_idx.min(entries.len() - 1);
        let entry = &entries[idx];
        let msg = match (entry.command_id, entry.tab_index) {
            (Some(id), _) => dispatch_action(id, keyboard::Modifiers::empty()),
            (_, Some(i)) => Some(Message::SwitchTab(i)),
            _ => None,
        };
        self.palette_visible = false;
        // P151：面板关闭即把输入焦点还给正文（否则中文输入法在正文里失效）
        self.focus_editor();
        match msg {
            Some(m) => self.update(m),
            None => Task::none(),
        }
    }

    /// 面板浮层：顶部居中卡片 = 查询输入框 + 过滤结果（选中行 ▶ 标记，
    /// 滑动窗口最多 12 行保证选中可见）。opaque 背板点击即收起，
    /// 卡片自身 opaque 防穿透（右键菜单同款双层）。
    fn palette_overlay(&self) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let entries = self.palette_filtered();
        let total = entries.len();
        let sel = self.palette_idx.min(total.saturating_sub(1));
        let win_start = sel.saturating_sub(11);
        let mut rows = column![].spacing(0).width(Fill);
        if total == 0 {
            rows = rows.push(
                text(self.t(editpad_core::Key::PaletteNoMatch)).size(uipx).font(uifont).width(Fill),
            );
        }
        for (i, e) in entries.iter().enumerate().skip(win_start).take(12) {
            let marker = if i == sel { "▶ " } else { "　 " };
            let detail: String = if e.detail.chars().count() > 48 {
                let t: String = e.detail.chars().take(47).collect();
                format!("{t}…")
            } else {
                e.detail.clone()
            };
            rows = rows.push(
                button(
                    text(format!("{marker}{}　{}", e.title, detail))
                        .size(uipx)
                        .font(uifont)
                        .width(Fill),
                )
                .width(Fill)
                .padding([3, 10])
                .style(chrome_menu_item_style)
                .on_press(Message::PalettePick(i)),
            );
        }
        let card = opaque(container(
            column![
                text_input(self.t(editpad_core::Key::PalettePlaceholder), &self.palette_input)
                    .id(palette_input_id())
                    .size(uipx)
                    .font(uifont)
                    .on_input(Message::PaletteInputChanged)
                    .on_submit(Message::PaletteExecute)
                    .padding([4, 8]),
                rule::horizontal(1),
                scrollable(rows).height(360.0),
            ]
            .spacing(4)
            .padding(6)
            .width(Fill)
        )
        .width(560)
        .style(popup_card_style));
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: 64.0, right: 0.0, bottom: 0.0, left: 0.0 }),
        )
        .on_press(Message::PaletteToggled(self.palette_mode))
            .into()
    }

    /// B9：列编辑器对话框——整窗背板（点击关闭）+ 居中卡片（设置弹窗
    /// P40 同款结构、命令面板同款 opaque 防穿透）。文本/序号两模式；
    /// Enter 确认 / Esc 取消在 KeyPressed 层拦截，输入框字符键不串层
    /// （P129 同口径）。数值输入以字符串承载、确认时统一校验。
    fn column_editor_overlay(&self) -> Element<'_, Message> {
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let d = &self.column_editor;
        let text_input_w = |placeholder: &'static str, value: &str, msg: fn(String) -> Message| {
            text_input(placeholder, value)
                .size(uipx)
                .font(uifont)
                .width(Fill)
                .on_input(msg)
                .padding([3, 8])
        };
        let mode_btn = |label: &str, active: bool| {
            button(
                text(format!("{} {label}", if active { "●" } else { "○" }))
                    .size(uipx)
                    .font(uifont),
            )
            .padding([3, 10])
            .style(chrome_button_style)
            .on_press(Message::ColumnEditorModeToggled)
        };
        fn labeled<'a>(
            label: &str,
            control: impl Into<iced::Element<'a, Message>>,
            uipx: f32,
            uifont: iced::Font,
        ) -> iced::Element<'a, Message> {
            row![
                text(label.to_owned()).size(uipx).font(uifont),
                control.into(),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Fill)
            .into()
        }
        let mut body = column![].spacing(8).width(Fill);
        body = body.push(
            row![
                text(self.t(editpad_core::Key::ColumnEditorTitle)).size(uipx).font(uifont).width(Fill),
                button(text("×").size(uipx).font(uifont))
                    .padding([1, 6])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorToggled),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        body = body.push(
            row![
                mode_btn(self.t(editpad_core::Key::CeModeText), !d.number_mode),
                mode_btn(self.t(editpad_core::Key::CeModeNumber), d.number_mode)
            ]
                .spacing(6),
        );
        if d.number_mode {
            body = body
                .push(labeled(
                    self.t(editpad_core::Key::CeStart),
                    text_input_w(self.t(editpad_core::Key::CeStartHint), &d.start, Message::ColumnEditorStartChanged),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CeStep),
                    text_input_w(self.t(editpad_core::Key::CeStepHint), &d.step, Message::ColumnEditorStepChanged),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CeBase),
                    button(
                        text(match d.base {
                            editor::NumBase::Dec => self.t(editpad_core::Key::CeBaseDec).to_owned(),
                            editor::NumBase::Hex => self.t(editpad_core::Key::CeBaseHex).to_owned(),
                            editor::NumBase::Bin => self.t(editpad_core::Key::CeBaseBin).to_owned(),
                            editor::NumBase::Oct => self.t(editpad_core::Key::CeBaseOct).to_owned(),
                        })
                        .size(uipx)
                        .font(uifont),
                    )
                    .padding([3, 10])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorBaseCycled),
                    uipx,
                    uifont,
                ))
                .push(labeled(
                    self.t(editpad_core::Key::CePadWidth),
                    text_input_w(self.t(editpad_core::Key::CePadHint), &d.pad_width, Message::ColumnEditorWidthChanged),
                    uipx,
                    uifont,
                ));
            if d.base == editor::NumBase::Hex {
                body = body.push(
                    iced::widget::Checkbox::new(d.hex_upper)
                        .label(self.t(editpad_core::Key::CeHexUpper))
                        .font(uifont)
                        .text_size(uipx)
                        .on_toggle(|_| Message::ColumnEditorHexUpperToggled),
                );
            }
        } else {
            body = body.push(labeled(
                self.t(editpad_core::Key::CeModeText),
                text_input_w(
                    self.t(editpad_core::Key::CeTextContent),
                    &d.text,
                    Message::ColumnEditorTextChanged,
                ),
                uipx,
                uifont,
            ));
        }
        // 目标块实时反馈：行数提示替代打开守卫的二次检查
        let rows_info = match self.cur_handle.borrow().active_block() {
            Some((r0, r1, _, _)) => format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::CeTargetBlockPrefix),
                    r1 - r0 + 1,
                    self.t(editpad_core::Key::CeTargetBlockSuffix)
                ),
            None => self.t(editpad_core::Key::CeNoBlockWarn).to_owned(),
        };
        body = body.push(text(rows_info).size(uipx).font(uifont));
        body = body.push(
            row![
                button(text(self.t(editpad_core::Key::ButtonOk)).size(uipx).font(uifont))
                    .padding([3, 14])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorConfirmed),
                button(text(self.t(editpad_core::Key::ButtonCancel)).size(uipx).font(uifont))
                    .padding([3, 14])
                    .style(chrome_button_style)
                    .on_press(Message::ColumnEditorToggled),
            ]
            .spacing(8),
        );
        let card = opaque(container(body).padding(14).width(420).style(popup_card_style));
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(Alignment::Center)
                .padding(16),
        )
        .on_press(Message::ColumnEditorToggled)
        .into()
    }

    fn find_all_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let total = self.matches.len();
        let scanning = self.find_scanning();
        let shown = total.min(FIND_ALL_MAX_ROWS);
        // 标题行：文本 width(Fill) 把关闭按钮推到右缘
        let header = row![
            text(if scanning {
                self.t(editpad_core::Key::FindSearching).to_owned()
            } else if total == 0 {
                self.t(editpad_core::Key::FindNoMatch).to_owned()
            } else {
                editpad_core::fmt_match_total(self.lang(), total)
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::FindAllToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if shown > 0 {
            let editor = self.cur_handle.borrow();
            for i in 0..shown {
                let m = self.matches[i];
                let raw = editor.line_text(m.line);
                let excerpt = match_excerpt(&raw, m.col, FIND_ALL_EXCERPT_COLS);
                rows = rows.push(
                    button(
                        text(format!("{}:{}  {}", m.line + 1, m.col + 1, excerpt))
                            .size(uipx)
                            .font(uifont)
                            .width(Fill),
                    )
                    .width(Fill)
                    .padding([3, 10])
                    .style(chrome_menu_item_style)
                    .on_press(Message::FindAllGoto(i)),
                );
            }
        }
        if total > shown {
            rows = rows.push(
                text(format!(
                    "{}{}{}{}{}",
                    self.t(editpad_core::Key::FindAllShownPrefix),
                    shown,
                    self.t(editpad_core::Key::FindAllShownTotalMiddle),
                    total,
                    self.t(editpad_core::Key::FindAllShownTotalSuffix),
                ))
                .size(uipx)
                .font(uifont),
            );
        }
        // 高度按窗口钳制：小窗不溢出、大窗不占满（浮层卡片同策略）
        let h = (self.viewport_size.1 * 0.35).clamp(120.0, 300.0);
        container(
            column![header, rule::horizontal(1), scrollable(rows)]
                .spacing(0)
                .width(Fill),
        )
        .width(Fill)
        .height(h)
        .into()
    }

    /// A8：「在文件中查找」结果面板——与查找全部面板同槽停靠（互斥
    /// 由装配处的 fif_visible 裁决）。文件头分组行（相对路径 + 命中数）
    /// 与可点命中行（行:列 + 扫描时预计算的摘录）。显示行数与查找全
    /// 部面板同量级封顶，超出明示。
    fn find_in_files_panel(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let scanning = self.fif_scan.is_some();
        let total_files = self.fif_results.len();
        let total_hits: usize = self.fif_results.iter().map(|f| f.hits.len()).sum();
        let title = if scanning {
            format!(
                "{}{}{}",
                self.t(editpad_core::Key::ScanProgressPrefix),
                self.fif_progress.load(std::sync::atomic::Ordering::Relaxed),
                self.t(editpad_core::Key::ScanProgressSuffix)
            )
        } else if total_hits == 0 {
            self.t(editpad_core::Key::DirNoMatch).to_owned()
        } else {
            editpad_core::fmt_fif_panel_title(self.lang(), total_files, total_hits, false)
        };
        let header = row![
            text(if self.fif_truncated {
                format!("{title}{}", self.t(editpad_core::Key::FifReachedCapSuffix))
            } else {
                title
            })
            .size(uipx)
            .font(uifont)
            .width(Fill),
            button(text("×").size(uipx).font(uifont))
                .style(chrome_button_style)
                .on_press(Message::FindInFilesToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([4, 10]);

        let mut rows = column![].spacing(0).width(Fill);
        if !scanning && self.find_query.is_empty() {
            rows = rows.push(text(self.t(editpad_core::Key::FifQueryEmpty)).size(uipx).font(uifont));
        } else if !scanning {
            let mut budget = FIND_ALL_MAX_ROWS;
            'outer: for (fi, fh) in self.fif_results.iter().enumerate() {
                let rel = fh
                    .path
                    .strip_prefix(self.fif_dir.as_deref().unwrap_or(std::path::Path::new("")))
                    .unwrap_or(&fh.path)
                    .display()
                    .to_string();
                rows = rows.push(
                    text(editpad_core::fmt_file_hits(self.lang(), &rel, fh.hits.len()))
                        .size(uipx)
                        .font(uifont),
                );
                for (hi, h) in fh.hits.iter().enumerate() {
                    if budget == 0 {
                        break 'outer;
                    }
                    budget -= 1;
                    rows = rows.push(
                        button(
                            text(format!("{}:{}  {}", h.pos.line + 1, h.pos.col + 1, h.excerpt))
                                .size(uipx)
                                .font(uifont)
                                .width(Fill),
                        )
                        .width(Fill)
                        .padding([3, 10])
                        .style(chrome_menu_item_style)
                        .on_press(Message::FifGoto(fi, hi)),
                    );
                }
            }
            if budget == 0 {
                rows = rows.push(
                    text(format!(
                        "{}{}{}",
                        self.t(editpad_core::Key::FindAllShownPrefix),
                        FIND_ALL_MAX_ROWS,
                        self.t(editpad_core::Key::FindAllShownSuffix)
                    ))
                        .size(uipx)
                        .font(uifont),
                );
            }
        }
        let h = (self.viewport_size.1 * 0.35).clamp(120.0, 300.0);
        container(
            column![header, rule::horizontal(1), scrollable(rows)]
                .spacing(0)
                .width(Fill),
        )
        .width(Fill)
        .height(h)
        .into()
    }

    /// P150：查找/替换**轻浮层**——窗口居中定宽卡片（[`FIND_CARD_W`]），
    /// **无背板**：不遮正文、可继续编辑；Esc 或 × 关闭（用户点单：
    /// 查找 UI 不再占据下方整条，改到窗口中间且不要太宽）。
    ///
    /// 承载全部查找部件：①查询行（输入框带稳定 Id——Ctrl+F 后自动聚焦）；
    /// ②开关与计数行；③替换行（A8 目录模式换为目录/浏览行）；④面板开关行；
    /// ⑤结果面板（查找全部 / 目录命中，同槽互斥）——挂在浮层内下半区、
    /// 面板自身限高滚动。
    fn find_overlay(&self, uipx: f32, uifont: iced::Font) -> Element<'_, Message> {
        let total = self.matches.len();
        // P10：扫描在途时明确显示状态，按钮基于过期结果禁用
        let scanning = self.find_scanning();
        let position_label = if scanning {
            self.t(editpad_core::Key::FindSearching).to_owned()
        } else if total == 0 {
            self.t(editpad_core::Key::FindNoMatch).to_owned()
        } else {
            match self.match_idx {
                Some(i) => editpad_core::fmt_find_counter(self.lang(), Some(i), total),
                None => editpad_core::fmt_find_counter(self.lang(), None, total),
            }
        };
        let has_matches = !scanning && !self.matches.is_empty();
        // A8：FIF 开态下文档导航按钮禁用（命中表是另一套数据源）
        let doc_nav = !self.fif_visible;
        // P153：淡出系数（1.0 = 不淡出）。卡片背板、输入框、按钮、复选框
        // 共用同一系数——控件自带底色（输入框主题底 / 按钮白底 / 勾选点缀色），
        // 只压背板会留下「实心白块浮在淡卡片上」的口径分裂。
        let dim = find_dim_factor(self.find_dimmed);
        // P153：纯文本标签（无底色）的淡出用色——None = 不淡出（不设色，
        // 保持 iced 默认文字样式）。
        let label_color = find_label_color(&self.theme(), dim);

        // 拖动条（用户点单）：按住卡片顶部这条即可拖走浮层，双击复位到
        // 默认位置。`on_move`/`on_release` 挂在整层（见函数尾部）——按下
        // 只有这里捕获，其它位置点击照旧穿透到正文。
        let grip_text = text(self.t(editpad_core::Key::FindGrip))
            .size(uipx * 0.8)
            .font(uifont)
            .width(Fill)
            .align_x(iced::alignment::Horizontal::Center);
        let grip = mouse_area(
            container(match label_color {
                Some(c) => grip_text.color(c).into(),
                None => Element::from(grip_text),
            })
            .width(Fill)
            .padding([0, 0]),
        )
        .interaction(iced::mouse::Interaction::Grab)
        .on_press(Message::FindDragStart)
        .on_double_click(Message::FindDragReset);

        // ①查询行：P150 起输入框带 Id（打开查找栏即聚焦，修前焦点留正文）
        let query_row = row![
            text_input(self.t(editpad_core::Key::FindQueryPlaceholder), &self.find_query)
                .id(find_input_widget_id())
                .size(uipx)
                .font(uifont)
                .on_input(Message::FindQueryChanged)
                .on_submit(Message::FindNext)
                .style(move |theme, status| find_input_style(theme, status, dim))
                .width(Fill),
            button(text(self.t(editpad_core::Key::FindPrev)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(doc_nav.then_some(Message::FindPrev)),
            button(text(self.t(editpad_core::Key::FindNext)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(doc_nav.then_some(Message::FindNext)),
            button(text("×").size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press(Message::FindToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        // ②开关与计数行
        // P153：命中计数是纯文本、无底色可压 alpha——淡出时显式设色
        //（含「无匹配」「查找中…」，用户复报它们不跟着变淡）
        let position_widget = text(position_label).size(uipx).font(uifont);
        let position_text: Element<'_, Message> = match label_color {
            Some(c) => position_widget.color(c).into(),
            None => position_widget.into(),
        };
        let options_row = row![
            checkbox(self.case_sensitive)
                .label(self.t(editpad_core::Key::FindCaseSensitive))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::CaseToggled),
            // P70：正则模式开关（.* 是各编辑器通用的正则图标语义）
            checkbox(self.regex_enabled)
                .label(self.t(editpad_core::Key::FindRegexMode))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::RegexToggled),
            // 整词匹配：命中前后均非词字符（正则模式下不参与——扫描与
            // 替换路径均按 regex 分支先行返回）
            checkbox(self.whole_word)
                .label(self.t(editpad_core::Key::FindWholeWord))
                .text_size(uipx)
                .font(uifont)
                .style(move |theme, status| find_checkbox_style(theme, status, dim))
                .on_toggle(Message::WholeWordToggled),
            // P123：命中计数（扫描在途给动态反馈；0 处也如实显示）
            position_text,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // ③替换行 / A8 目录行（两模式互斥）
        let third_row: Element<'_, Message> = if self.fif_visible {
            let dir_text = match &self.fif_dir {
                Some(d) => d.display().to_string(),
                None => self.t(editpad_core::Key::FifDirUnnamed).to_owned(),
            };
            let dir_label = text(self.t(editpad_core::Key::FindDir).to_owned()).size(uipx).font(uifont);
            let dir_label: Element<'_, Message> = match label_color {
                Some(c) => dir_label.color(c).into(),
                None => dir_label.into(),
            };
            let dir_path = text(dir_text).size(uipx).font(uifont);
            let dir_path: Element<'_, Message> = match label_color {
                Some(c) => dir_path.color(c).into(),
                None => dir_path.into(),
            };
            let scan_label = text(if self.fif_scan.is_some() {
                format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ScanProgressPrefix),
                    self.fif_progress.load(std::sync::atomic::Ordering::Relaxed),
                    self.t(editpad_core::Key::ScanProgressSuffix)
                )
            } else {
                String::new()
            })
            .size(uipx)
            .font(uifont);
            let scan_label: Element<'_, Message> = match label_color {
                Some(c) => scan_label.color(c).into(),
                None => scan_label.into(),
            };
            row![
                dir_label,
                // 长路径裁剪显示：不撑破定宽卡片（取舍：卡片内不可横向滚动）
                container(dir_path).width(Fill).clip(true),
                button(text(self.t(editpad_core::Key::FindBrowse)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press(Message::FifBrowseFolder),
                scan_label,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            row![
                text_input(self.t(editpad_core::Key::ReplaceQueryPlaceholder), &self.replace_query)
                    .size(uipx)
                    .font(uifont)
                    .on_input(Message::ReplaceQueryChanged)
                    .style(move |theme, status| find_input_style(theme, status, dim))
                    .width(Fill),
                // P70：正则模式替换当前 = 对命中做 $1 展开替换
                button(text(self.t(editpad_core::Key::FindReplaceCurrent)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press_maybe(has_matches.then_some(if self.regex_enabled {
                        Message::ReplaceCurrentRegex
                    } else {
                        Message::ReplaceCurrent
                    })),
                // 扫描在途时禁用：此刻的全文快照可能是过期的
                button(text(self.t(editpad_core::Key::FindReplaceAll)).size(uipx).font(uifont))
                    .style(move |theme, status| find_button_style(theme, status, dim))
                    .on_press_maybe((!scanning).then_some(Message::ReplaceAll)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        };

        // ④面板开关行
        let panel_row = row![
            // 第 62 轮：查找全部结果面板开关（扫描在途/无命中时禁用；
            // A8：FIF 开态禁用——两套面板同槽互斥）
            button(text(self.t(editpad_core::Key::FindFindAll)).size(uipx).font(uifont))
                .style(move |theme, status| find_button_style(theme, status, dim))
                .on_press_maybe(
                    (has_matches && doc_nav).then_some(Message::FindAllToggled),
                ),
            // A8：在文件中查找模式开关（F12 同义入口）
            button(text(if self.fif_visible {
                self.t(editpad_core::Key::FindInFilesExit)
            } else {
                self.t(editpad_core::Key::FindInFilesMode)
            })
            .size(uipx)
            .font(uifont))
            .style(move |theme, status| find_button_style(theme, status, dim))
            .on_press(Message::FindInFilesToggled),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut card_body = column![grip, query_row, options_row, third_row, panel_row]
            .spacing(8)
            .width(Fill);
        // ⑤结果面板：查找全部 / 目录命中（同槽互斥，面板自身限高滚动）
        if self.find_all_visible && !self.fif_visible {
            card_body = card_body
                .push(rule::horizontal(1))
                .push(self.find_all_panel(uipx, uifont));
        }
        if self.fif_visible {
            card_body = card_body
                .push(rule::horizontal(1))
                .push(self.find_in_files_panel(uipx, uifont));
        }

        // 定宽 + 小窗钳制；高度按窗口钳制（结果面板内部滚动承接溢出）
        let card_w = FIND_CARD_W.min((self.viewport_size.0 - 32.0).max(320.0));
        // P153：点过正文（find_dimmed）后整框转半透明——背板换淡出色，
        // 卡片内文字/控件沿用原样式按同一 alpha 淡出。
        let card_style = if self.find_dimmed {
            popup_card_dim_style
        } else {
            popup_card_style
        };
        let card = opaque(
            container(card_body)
                .width(card_w)
                .max_height((self.viewport_size.1 - 48.0).max(160.0))
                .padding(10)
                .style(card_style),
        );
        // P153：卡片（含拖动条、空白处，**以及输入框/按钮本身**）接住左键
        // 按下 → 半透明态复位。必须用 [`crate::press_observer::PressObserver`]
        // 而非 `mouse_area`：输入框/按钮会捕获按下事件，`mouse_area` 在内容
        // 已捕获时不再自查（上游 `MouseArea::update` 的 `is_event_captured`
        // 早退），导致「点半透明框里的输入框不恢复」（用户复报）。
        // 观察层同时**消费**该按下——否则会继续下传给正文自绘控件，正文
        // 重新持焦并请求 IME，拼音组字串在查找框与正文各画一份（P151 症状
        // 复现，用户复报「旧 bug 又回来了」）。
        let card =
            PressObserver::new(card, Message::FindBoxPressed).into_element();
        // 位置由应用层持有（用户点单：浮层遮住目标行要能拖开）：
        // 默认 = 窗口中间偏上（名义高度参与居中），拖动后 = 钳制过的左上角。
        // 整层挂 `on_move`/`on_release` 追踪拖动：该层**不设 on_press**，
        // 点击照旧穿透到正文；`ButtonReleased` 上游不捕获（只发消息），
        // 所以正文拖拽/滚动条拖动不受影响。
        let pos = self.find_pos.unwrap_or_else(|| self.default_find_pos());
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: pos.y, right: 0.0, bottom: 0.0, left: pos.x }),
        )
        .on_move(Message::FindCursorMoved)
        .on_release(Message::FindDragEnd)
        .into()
    }

    /// 查找浮层默认位置（窗口中间偏上；卡片高度按名义值 [`FIND_CARD_NOMINAL_H`]
    /// 参与居中——真实高度随结果面板开合变化，取名义值即可，且便于拖动钳制）。
    pub(crate) fn default_find_pos(&self) -> Point {
        let (w, h) = self.viewport_size;
        let card_w = FIND_CARD_W.min((w - 32.0).max(320.0));
        Point::new(
            ((w - card_w) * 0.5).max(8.0),
            ((h - FIND_CARD_NOMINAL_H) * 0.5).max(8.0),
        )
    }

    /// 拖动钳制：左上角留在窗口内（右下按名义尺寸留边，拖不出视野）。
    pub(crate) fn clamp_find_pos(&self, p: Point) -> Point {
        let (w, h) = self.viewport_size;
        let card_w = FIND_CARD_W.min((w - 32.0).max(320.0));
        Point::new(
            p.x.clamp(8.0, (w - card_w - 8.0).max(8.0)),
            p.y.clamp(8.0, (h - FIND_CARD_NOMINAL_H - 8.0).max(8.0)),
        )
    }

    /// P67：状态栏弹出菜单的通用浮层骨架——背板点击关闭 + 右下角贴
    /// 状态栏上缘的定宽卡片（锚点按窗口尺寸现算，无需指针跟踪）。
    fn status_menu_overlay<'a>(
        &self,
        card_w: f32,
        card_h: f32,
        panel: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let ax = (self.viewport_size.0 - card_w - 12.0).max(8.0);
        // 底缘贴状态栏上方（状态栏高 ≈ 32px），小窗口钳到 8px
        let ay = (self.viewport_size.1 - card_h - 36.0).max(8.0);
        let card = opaque(
            container(panel)
                .width(card_w)
                .height(card_h)
                .padding(4)
                .style(popup_card_style),
        );
        mouse_area(
            container(card)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding { top: ay, right: 0.0, bottom: 0.0, left: ax }),
        )
        .on_press(Message::BarsDismissed)
        .into()
    }

    /// P67：编码弹出菜单——三种目标编码，未命名页禁用（无路径可写）。
    fn encoding_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 200.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let named = self.tab().path.is_some();
        let item = |label: &str, enc: editpad_core::SaveEncoding| {
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe(named.then_some(Message::SaveWithEncoding(enc)))
        };
        let panel = column![
            item(&enc_label(self.lang(), "UTF-8"), editpad_core::SaveEncoding::Utf8),
            item(&enc_label(self.lang(), "UTF-8(BOM)"), editpad_core::SaveEncoding::Utf8Bom),
            item(&enc_label(self.lang(), "GBK"), editpad_core::SaveEncoding::Gbk),
            // P127：CJK 传统编码扩展（无法映射字符照旧按数值实体写入）
            item(&enc_label(self.lang(), "Big5"), editpad_core::SaveEncoding::Big5),
            item(&enc_label(self.lang(), "Shift_JIS"), editpad_core::SaveEncoding::ShiftJis),
            item(&enc_label(self.lang(), "EUC-JP"), editpad_core::SaveEncoding::EucJp),
            item(&enc_label(self.lang(), "EUC-KR"), editpad_core::SaveEncoding::EucKr),
        ]
        .spacing(2);
        self.status_menu_overlay(W, ITEM_H * 3.0 + 12.0, panel.into())
    }

    /// P67：行尾弹出菜单——当前主导行尾标头 + 两个转换项（已是目标
    /// 则禁用）。转换为可撤销的文档编辑。
    fn eol_menu_overlay(&self) -> Element<'_, Message> {
        const W: f32 = 220.0;
        const ITEM_H: f32 = 32.0;
        let uipx = editor::ui_font_px();
        let uifont = self.ui_font();
        let current = self.cur_handle.borrow().doc.line_ending();
        let item = |label: &str, target: editpad_core::LineEnding| {
            let disabled = current == target;
            button(
                text(label.to_owned())
                    .size(uipx)
                    .font(uifont)
                    .width(Fill),
            )
            .width(Fill)
            .padding([6, 10])
            .style(chrome_menu_item_style)
            .on_press_maybe((!disabled).then_some(Message::ConvertEol(target)))
        };
        let panel = column![
            text(format!("{}{}", self.t(editpad_core::Key::EolCurrentPrefix), eol_label(current)))
                .size(uipx)
                .font(uifont),
            item(
                &format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ConvertEolPrefix),
                    "CRLF",
                    self.t(editpad_core::Key::ConvertEolCrLfSuffix)
                ),
                editpad_core::LineEnding::CrLf
            ),
            item(
                &format!(
                    "{}{}{}",
                    self.t(editpad_core::Key::ConvertEolPrefix),
                    "LF",
                    self.t(editpad_core::Key::ConvertEolLfSuffix)
                ),
                editpad_core::LineEnding::Lf,
            ),
        ]
        .spacing(4);
        self.status_menu_overlay(W, ITEM_H * 2.0 + 28.0, panel.into())
    }

    // settings_overlay 已迁往 settings_ui.rs（第 82 轮 Phase 2a）

}

// ---------- 「查找全部」结果面板助手（第 62 轮） ----------

/// P155：编码弹出菜单的条目文案（`以 UTF-8 保存` / `Save as UTF-8`）。
/// 编码名本身是事实数据，不翻译；只包装语言相关的动词与语序。
fn enc_label(lang: editpad_core::Lang, enc: &str) -> String {
    format!(
        "{}{}{}",
        editpad_core::Key::SaveAsEncodingPrefix.text(lang),
        enc,
        editpad_core::Key::SaveAsEncodingSuffix.text(lang)
    )
}

/// 结果面板渲染行数封顶：chrome 行按钮无虚拟化，超出部分在面板尾部
/// 明示截断（提示细化关键词），避免超大命中集拖垮每帧构建。
pub(crate) const FIND_ALL_MAX_ROWS: usize = 500;
/// 单条结果的摘录字符数上限（以命中列为窗心向两侧取半）。
const FIND_ALL_EXCERPT_COLS: usize = 96;

// ---------- 查找轻浮层（P150） ----------

/// 查找浮层定宽（px）：用户点单「出现在窗口中间、不要太宽」（小窗自动
/// 钳制到窗口宽 − 32）。修前查找 UI 是整宽停靠在编辑器下方的行。
const FIND_CARD_W: f32 = 560.0;
/// 查找浮层名义高度（px）：仅用于「默认位置取窗口中间」与拖动钳制——
/// 真实高度随结果面板开合变化，用名义值即可，无需回读布局。
const FIND_CARD_NOMINAL_H: f32 = 170.0;
/// 查找输入框的稳定 Id：打开查找栏（Ctrl+F / 菜单 / 工具栏）后据此
/// 自动聚焦——修前打开查找栏不转移焦点，打字直接落进正文文档。
pub(crate) const FIND_INPUT_ID: &str = "editpad-find-input";

/// 查询框的组件 Id（`.id(..)` 与聚焦操作共用同一值；iced 0.14 的
/// [`iced::advanced::widget::Id`] 只能由 `&'static str` 构造）。
pub(crate) fn find_input_widget_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(FIND_INPUT_ID)
}

/// 结果面板行摘录：剥行尾 → 以命中列为窗心取最多 `max_cols` 个字符，
/// 两端截断处补省略号 `…`。纯函数便于单测。
pub(crate) fn match_excerpt(line_text: &str, col: usize, max_cols: usize) -> String {
    let body = line_text.trim_end_matches(['\n', '\r']);
    let chars: Vec<char> = body.chars().collect();
    let col = col.min(chars.len());
    if chars.len() <= max_cols {
        return chars.into_iter().collect();
    }
    // 内窗预算：两端截断各占 1 位省略号（单端截断时另一侧多还 1 位）
    let take = max_cols - 2;
    let mut start = col.saturating_sub(take.div_ceil(2));
    let end = (start + take).min(chars.len());
    if end == chars.len() {
        // 尾部没截：把省下的右侧预算回填给头部
        start = start.saturating_sub(take - (end - start));
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// P129：命令面板条目——命令模式带 command_id（经 dispatch_action
/// 执行），标签模式带 tab_index（SwitchTab 执行）。
#[derive(Clone)]
pub(crate) struct PaletteEntry {
    pub(crate) command_id: Option<&'static str>,
    pub(crate) tab_index: Option<usize>,
    pub(crate) title: String,
    pub(crate) detail: String,
}
