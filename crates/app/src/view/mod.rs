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
//! 分域 impl 见子模块：
//! * `overlays`——标签右键菜单、顶部菜单栏与各下拉菜单、命令面板、列编辑器对话框；
//! * `find_panel`——查找/替换浮层、查找全部结果面板、在文件中查找面板；
//! * `lifecycle`——窗口关闭与退出、退出前落盘、窗口标题；
//! * `restore`——会话恢复；`snapshots`——心跳快照；`scans`——扫描调度；
//! * `find_ops`——命中跳转与替换；`tab_menu`——标签右键面板。
//!
//! 约定：子模块里的私有方法经 `pub(super)` 跨文件可见；共享导入在本文件
//! **`pub(crate)` 再导出**（私有 `use` 不跨模块传递）。**`column` 宏是例外**——
//! 宏经 glob 再导出会与 prelude 撞名（E0659：`column` is ambiguous），故它
//! 不进枢纽，由用到它的子模块各自显式 `use iced::widget::column;`。

pub(crate) use super::*;
use iced::widget::column;

// 按域拆分的 impl 块（纯移动零行为变更；私有方法经 pub(super) 跨文件可见）
mod find_ops;
mod find_panel;
mod overlays;
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


    // ---------- 第 69 轮：顶部菜单栏浮层 ----------



    // 第 62 轮：「查找全部」结果面板（停靠式，非浮层）——数据源 =
    // 查找后台扫描的全量命中表，重扫刷新自动跟随。渲染行数封顶
    // FIND_ALL_MAX_ROWS：chrome 行按钮无虚拟化，10 万级命中全量
    // 渲染会拖垮帧率，超出部分明示截断提示。
    //
    // ---------- P129：命令面板 / 快速标签切换 ----------














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
