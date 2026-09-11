use super::*;

    // ---------- 不变量：tabs 恒非空 ----------

    /// 删页路径（关页 / 批量关 / 恢复占位页回收）之后 `tabs` 必须仍非空，
    /// 且 `active_tab` 保持合法。
    ///
    /// 契约：`set_active_tab` 用 `len() - 1` 钳制、`SwitchTabNext/Prev`
    /// 用 `% len()` 回绕——空集会分别下溢与除零。三处删页点已收口到
    /// [`Editpad::ensure_nonempty_tabs`]，本测试钉住该不变量。
    #[test]
    fn tabs_stay_nonempty_after_closing_last() {
        let mut app = Editpad::default();
        assert_eq!(app.tabs.len(), 1, "前置：默认单页");

        // ① 关闭唯一页 → 补出一个干净页，不变量不破
        dispatch(&mut app, Message::CloseTabAt(0));
        assert!(!app.tabs.is_empty(), "关掉最后一页后必须仍有页");
        assert!(app.active_tab < app.tabs.len(), "active_tab 必须合法");
        assert!(app.cur_handle.borrow().doc.is_empty(), "补出的应是干净页");

        // ② 回绕切换在单页下不得 panic（`% len()` 除零面）
        dispatch(&mut app, Message::SwitchTabNext);
        dispatch(&mut app, Message::SwitchTabPrev);
        assert_eq!(app.active_tab, 0, "单页来回切换仍停在 0");

        // ③ 恢复占位页回收路径同款：删掉唯一占位页不得留下空集合
        app.drop_restore_placeholder(0);
        assert!(!app.tabs.is_empty(), "回收占位页后必须仍有页");
        assert!(app.active_tab < app.tabs.len());
    }

    // ---------- P21 多标签骨架 ----------

    #[test]
    fn new_tab_switch_and_pages_stay_independent() {
        let mut app = Editpad::default();
        assert_eq!(app.tabs.len(), 1);

        // 初始页写内容 → 置脏
        dispatch(&mut app, Message::Edit(EditOp::InsertText("page0".into())));
        assert!(app.tab().dirty);

        // 连开两个新页：自动切换过去，且为空净未命名
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.active_tab, 2);
        assert!(
            app.cur_handle.borrow().doc.is_empty(),
            "新页应为空文档"
        );

        // 第 3 页编辑后切走再切回：内容与置脏状态各自独立
        dispatch(&mut app, Message::Edit(EditOp::InsertText("page2".into())));
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "page0");
        assert!(app.tab().dirty);
        dispatch(&mut app, Message::SwitchTab(2));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "page2");

        // Ctrl+Tab 循环
        dispatch(&mut app, Message::SwitchTabNext);
        assert_eq!(app.active_tab, 0, "从末页循环回首页");
    }

    #[test]
    fn loaded_result_routes_to_origin_tab_after_switch() {
        let mut app = Editpad::default();

        // 打开 A：落入初始空净页
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq_a = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq_a,
                Ok((editpad_core::Document::from_str("content A"), String::new(), "UTF-8".to_owned())),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "content A");

        // 当前页非空 → B 落新页 idx1 并切换过去；加载完成前用户切回页 0
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        assert_eq!(app.active_tab, 1, "非空当前页应新开一页承接打开");
        let seq_b = app.job_seq;
        dispatch(&mut app, Message::SwitchTab(0));

        dispatch(
            &mut app,
            Message::Loaded(
                seq_b,
                Ok((editpad_core::Document::from_str("content B"), String::new(), "UTF-8".to_owned())),
            ),
        );

        assert_eq!(app.active_tab, 0, "切走的用户不被加载结果拉回");
        assert_eq!(app.tabs.len(), 2);
        let origin_doc = app.tabs[1].editor.borrow().doc.to_text();
        assert_eq!(origin_doc, "content B", "迟到结果必须归入发起页");
    }

    #[test]
    fn close_tab_flow_respects_dirty_and_never_empties_tabs() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("dirty page".into())));

        // 置脏页关闭 → 先确认不移除
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.close_tab_confirm, Some(0));
        assert_eq!(app.tabs.len(), 1);

        // 取消：页面原样保留
        dispatch(&mut app, Message::CancelCloseTab);
        assert_eq!(app.close_tab_confirm, None);
        assert_eq!(app.tabs.len(), 1);

        // 确认放弃：清空内容并移除；最后一页被新的空页替代
        dispatch(&mut app, Message::CloseTabRequest);
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
        assert_eq!(app.tabs.len(), 1, "恒保有一个标签页");
        assert!(!app.tab().dirty);
        assert!(app.cur_handle.borrow().doc.is_empty());

        // 干净页关闭即刻生效（两页 → 一页）
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs.len(), 2);
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), 1, "干净页直接关闭");
        assert!(app.close_tab_confirm.is_none());
    }

    #[test]
    fn window_close_confirms_when_any_background_tab_is_dirty() {
        let mut app = Editpad::default();
        // P29 起默认快照直退；本测试钉住「每次询问」模式的聚合确认口径
        app.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("make dirty".into())));
        dispatch(&mut app, Message::NewTab); // 切到干净的新页

        // 活动页干净，但后台页置脏 → 关窗仍需确认（聚合口径）
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(
            app.confirm_visible,
            "任一页置脏都必须弹关窗确认"
        );
    }

    // ---------- P28 标签右键菜单 ----------

    #[test]
    fn batch_close_targets_excludes_pinned_and_handles_out_of_range() {
        // 布局：[0]=固定 [1] [2]=固定 [3]
        let mut tabs = vec![Tab::empty(), Tab::empty(), Tab::empty(), Tab::empty()];
        tabs[0].pinned = true;
        tabs[2].pinned = true;

        // 关闭其他(keep=1)：候选 {0,2,3} 排除固定页 0/2 → 仅剩 3
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(1)),
            vec![3],
            "固定页必须被豁免出批量目标"
        );
        // 关闭其他(keep=3)：其余页里只有非固定的页 1 可关
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(3)),
            vec![1],
        );
        // 关闭其他(keep=3) 但除 keep 外全固定 → 空
        tabs[1].pinned = true;
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(3)),
            Vec::<usize>::new(),
        );
        tabs[1].pinned = false;

        // 关闭右侧：只收 from 右侧的非固定页
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(1)),
            vec![3],
            "右侧的固定页 2 必须被豁免"
        );
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(2)),
            vec![3],
        );

        // 越界安全：keep/from 超出范围一律空表（菜单项据此禁用）
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::Others(9)),
            Vec::<usize>::new()
        );
        assert_eq!(
            batch_close_targets(&tabs, BatchCloseScope::RightOf(9)),
            Vec::<usize>::new()
        );
    }

    // ---------- P39/P40 浮层弹窗 ----------

    /// P43 回归：右键菜单卡片高度随窗口钳制（小窗不溢出、终生不盖满全屏）。
#[test]
fn ctx_menu_card_h_adapts_to_viewport() {
    // 常规窗口（600 高）：内容本身约 260px，卡在常量估高上（上下留边）
    assert_eq!(ctx_menu_card_h(600.0), CTX_MENU_H);
    // 大窗口也不突破估高
    assert_eq!(ctx_menu_card_h(1080.0), CTX_MENU_H);
    // 小窗：高度 = 窗高 − 16 边距（内容超高时内部滚动），不再盖满全屏
    assert_eq!(ctx_menu_card_h(200.0), 184.0);
    assert_eq!(ctx_menu_card_h(100.0), 84.0);
    // 未知/极小窗口：回退保守估高（首帧未收到 Resized 的兜底）
    assert_eq!(ctx_menu_card_h(0.0), CTX_MENU_H);
    assert_eq!(ctx_menu_card_h(40.0), CTX_MENU_H);
    // 与锚点钳制联用：卡片恒在窗口内（小窗下 y 夹 0，不越下缘）
    let (_, ay) = clamp_menu_anchor((100.0, 500.0), (800.0, 120.0), CTX_MENU_W, ctx_menu_card_h(120.0));
    assert!(ay + ctx_menu_card_h(120.0) <= 120.0, "锚点+高度必须落在窗内");
}

#[test]
    fn clamp_menu_anchor_keeps_overlay_inside_window() {
        // 常规位置不钳制
        assert_eq!(
            clamp_menu_anchor((100.0, 100.0), (800.0, 600.0), 200.0, 280.0),
            (100.0, 100.0)
        );
        // 右缘：x 拉回 窗宽−菜单宽（等效于主流编辑器的向左翻开）
        assert_eq!(
            clamp_menu_anchor((750.0, 100.0), (800.0, 600.0), 200.0, 280.0),
            (600.0, 100.0)
        );
        // 下缘：y 拉回 窗高−菜单高
        assert_eq!(
            clamp_menu_anchor((100.0, 500.0), (800.0, 600.0), 200.0, 280.0),
            (100.0, 320.0)
        );
        // 窗口尺寸未知 (0,0) → 不钳制（启动后首个 Resized 事件校准）
        assert_eq!(
            clamp_menu_anchor((750.0, 500.0), (0.0, 0.0), 200.0, 280.0),
            (750.0, 500.0)
        );
        // 小窗整体夹 0；负坐标夹 0
        assert_eq!(
            clamp_menu_anchor((50.0, 40.0), (100.0, 80.0), 200.0, 280.0),
            (0.0, 0.0)
        );
        assert_eq!(
            clamp_menu_anchor((-5.0, -5.0), (800.0, 600.0), 200.0, 280.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn context_menu_anchor_recorded_from_cursor_and_viewport_tracked() {
        let mut app = app_with_tabs(1);
        // 窗口逻辑尺寸经 Resized 事件入账（浮层贴边钳制依据）
        dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
        assert_eq!(app.viewport_size, (1024.0, 768.0));

        // 指针位置先于右键到达 → 菜单锚点 = 打开那一刻的指针位置
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(300.0, 200.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, Some(0));
        assert_eq!(app.menu_anchor, (300.0, 200.0));

        // 进入 busy（对话框/IO 互斥）必须收起已开的浮层：背板挡整窗不能留
        app.enter_busy();
        assert_eq!(app.tab_context_menu, None);

        // busy 期间右键拒开新菜单，锚点也不被改写（下次打开取届时位置）
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(10.0, 10.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, None);
        assert_eq!(app.menu_anchor, (300.0, 200.0));
    }

    #[test]
    fn view_builds_with_floating_overlays_open() {
        // 无头冒烟：菜单浮层、设置浮层、两者叠加时视图树均可构造
        // （Stack + opaque + 背板 mouse_area 的组合在构造期不 panic）
        let mut app = app_with_tabs(2);
        dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
        dispatch(&mut app, Message::CursorMoved(iced::Point::new(40.0, 40.0)));
        dispatch(&mut app, Message::TabContextMenu(0));
        let _ = app.view();
        dispatch(&mut app, Message::SettingsToggled);
        let _ = app.view();
    }

    #[test]
    fn context_menu_opens_closes_and_respects_busy_guard() {
        // 注意：app_with_tabs(2) 的活动页在下标 1（NewTab 会自动切换）
        let mut app = app_with_tabs(2);

        // 右键第 0 页 → 菜单打开并指向该页
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, Some(0));

        // 左键切到另一页（0 ≠ 当前活动页 1）→ 菜单随切换收起
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.tab_context_menu, None);

        // Esc 收起（BarsDismissed 一并清批量确认态）
        dispatch(&mut app, Message::TabContextMenu(1));
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.tab_context_menu, None);

        // busy 中右键不开菜单
        app.busy = true;
        dispatch(&mut app, Message::TabContextMenu(0));
        assert_eq!(app.tab_context_menu, None, "busy 时不得打开右键菜单");
        app.busy = false;

        // × 按钮 / 显式收起消息
        dispatch(&mut app, Message::TabContextMenu(0));
        dispatch(&mut app, Message::TabContextMenuClosed);
        assert_eq!(app.tab_context_menu, None);

        // 已被关掉的页下标：忽略不 panic
        dispatch(&mut app, Message::CloseTabAt(1));
        dispatch(&mut app, Message::TabContextMenu(5));
        assert_eq!(app.tab_context_menu, None);
    }

    #[test]
    fn close_tab_at_routes_clean_dirty_and_pinned_pages() {
        let mut app = app_with_tabs(2);

        // 干净后台页：即刻关闭
        dispatch(&mut app, Message::CloseTabAt(1));
        assert_eq!(app.tabs.len(), 1, "干净页应被立即关闭");

        // 置脏页：转既有单页确认条（含保存并关闭出口）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::SwitchTab(0));
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.close_tab_confirm, Some(0), "置脏页应弹关闭确认条");
        dispatch(&mut app, Message::CancelCloseTab);

        // 固定页：拒绝关闭并留痕状态栏
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(app.tabs[0].pinned, "固定开关应翻转");
        let len_before = app.tabs.len();
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.tabs.len(), len_before, "固定页不得被关闭");
        assert!(
            app.status.contains("取消固定"),
            "拒绝原因应写入状态栏，实际:{}",
            app.status
        );
        // 再翻转一次恢复非固定；Ctrl+W 对固定页同样豁免（键盘路径一致性）
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(!app.tabs[0].pinned);
        dispatch(&mut app, Message::TogglePinTab(0));
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), len_before, "Ctrl+W 不得绕过固定豁免");
        assert!(app.status.contains("取消固定"));
        dispatch(&mut app, Message::TogglePinTab(0));
        assert!(!app.tabs[0].pinned);
    }

    #[test]
    fn close_other_tabs_aggregates_dirty_confirm_and_spares_pinned() {
        // 页 0 固定+置脏；页 1 干净；页 2 置脏。在页 1 上「关闭其他」。
        let mut app = app_with_tabs(3);
        dispatch(&mut app, Message::TogglePinTab(0)); // 页 0 固定（仍置脏）
        dispatch(&mut app, Message::SwitchTab(2));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d2".into())));
        dispatch(&mut app, Message::SwitchTab(1));

        dispatch(&mut app, Message::CloseOtherTabs(1));
        // 目标 = 非固定且非 keep = 仅 {2}（页 0 因固定被豁免）；
        // 含置脏页 → 弹聚合确认而非直接关
        let targets = app.batch_close_confirm.clone().expect("应弹聚合确认");
        assert_eq!(targets, vec![2], "固定页必须被豁免出批量目标");

        // 取消：一切原样
        dispatch(&mut app, Message::CancelBatchCloseTabs);
        assert_eq!(app.tabs.len(), 3);
        assert!(app.tabs[2].dirty);

        // 再次确认放弃：仅移除目标页，固定页与 keep 页幸存
        dispatch(&mut app, Message::CloseOtherTabs(1));
        dispatch(&mut app, Message::ConfirmBatchCloseDiscard);
        assert_eq!(app.tabs.len(), 2, "只有目标页被移除");
        assert!(app.tabs[0].pinned && app.tabs[0].dirty, "固定置脏页原样幸存");
        assert_eq!(app.batch_close_confirm, None);
        assert_eq!(app.tab_context_menu, None, "选中菜单项后菜单应收起");

        // 幸存两页全干净后「关闭其他」= 无置脏目标直接关，不再弹确认
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0)); // 显式放弃固定置脏页
        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert!(app.batch_close_confirm.is_none(), "全部干净不应弹确认");
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn close_right_tabs_only_touches_right_side_non_pinned() {
        // 4 页（页 0 置脏，其余干净）：页 2 固定。「在页 0 上关闭其他」
        // → 目标 = {1, 3} 全干净 → 立即移除，固定页 2 与 keep 页 0 幸存。
        let mut app = app_with_tabs(4);
        dispatch(&mut app, Message::TogglePinTab(2));

        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert!(app.batch_close_confirm.is_none());
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[1].pinned, "原页 2（现下标 1）仍固定且幸存");

        // 重验 CloseTabsRight 的右侧口径与置脏聚合：
        // 4 页，页 0 干净、页 1 置脏。
        let mut app = Editpad::default();
        dispatch(&mut app, Message::NewTab); // 挤掉初始空净页的就地打开优惠
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d1".into())));
        dispatch(&mut app, Message::SwitchTab(0));

        dispatch(&mut app, Message::CloseTabsRight(0));
        let targets = app.batch_close_confirm.clone().expect("含置脏页应弹确认");
        assert_eq!(targets, vec![1, 2, 3], "右侧全部非固定页入列");
        dispatch(&mut app, Message::CancelBatchCloseTabs);
        assert_eq!(app.tabs.len(), 4);

        // 清掉置脏的右侧页后，右侧关闭立即生效
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(1));
        dispatch(&mut app, Message::CloseTabsRight(0));
        assert!(app.batch_close_confirm.is_none());
        assert_eq!(app.tabs.len(), 1, "右侧干净页应被直接移除");
    }

    #[test]
    fn menu_save_and_rename_route_through_active_tab_flow() {
        let mut app = Editpad::default();

        // 页 0 命名并置脏；页 1 新建未命名
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((editpad_core::Document::from_str("A"), String::new(), "UTF-8".to_owned())),
            ),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("change".into())));
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.active_tab, 1);

        // 菜单「保存」后台命名置脏页：先切页再走活动页保存流（busy 进入保存任务）
        dispatch(&mut app, Message::SaveTabFromMenu(0));
        assert_eq!(app.active_tab, 0, "菜单保存应先切到目标页");
        assert!(app.busy, "应进入既有保存流的 busy 守卫");
        app.busy = false;

        // 干净页的菜单保存：no-op（与工具栏「保存」同口径，按钮侧禁用）
        dispatch(&mut app, Message::SaveTabFromMenu(1));
        assert!(!app.busy, "干净页保存应为 no-op");

        // 菜单「重命名」= 切页 + 另存为对话框阶段
        dispatch(&mut app, Message::RenameOrSaveAsTab(1));
        assert_eq!(app.active_tab, 1, "重命名应切到目标页");
        assert!(app.busy, "另存为对话框阶段应置 busy");
        app.busy = false;

        // 未命名置脏页的菜单保存自动落另存为（Ctrl+S 同语义）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().path.is_none() && app.tab().dirty);
        let active = app.active_tab;
        dispatch(&mut app, Message::SaveTabFromMenu(active));
        assert!(app.busy, "未命名页保存应转入另存为对话框");
    }

    #[test]
    fn batch_close_never_empties_tabs_and_keeps_invariants() {
        // 两页全部固定：在页 0 上「关闭其他」→ 无可关目标 no-op。
        let mut app = app_with_tabs(2);
        dispatch(&mut app, Message::TogglePinTab(0));
        dispatch(&mut app, Message::TogglePinTab(1));
        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert_eq!(app.tabs.len(), 2, "无可关目标应保持原样");
        assert!(app.batch_close_confirm.is_none());

        // 单页应用上关闭其他/右侧：同样 no-op
        let mut solo = Editpad::default();
        dispatch(&mut solo, Message::CloseOtherTabs(0));
        dispatch(&mut solo, Message::CloseTabsRight(0));
        assert_eq!(solo.tabs.len(), 1);

        // close_tabs_now 兜底：移除最后一页时重置新空页（tabs 恒非空不变式）
        let mut bare = Editpad::default();
        let removed = bare.close_tabs_now(&[0]);
        assert_eq!(removed, 1);
        assert_eq!(bare.tabs.len(), 1, "移除最后一页应重置新空页");
        assert!(!bare.tab().dirty);
        assert!(bare.cur_handle.borrow().doc.is_empty());
        // 越界/重复下标安全
        assert_eq!(bare.close_tabs_now(&[5]), 0);
        assert_eq!(bare.close_tabs_now(&[0, 0]), 1);
    }

    // ---------- P64/P65 重命名聚焦与双击触发 ----------

    #[test]
    fn p65_double_click_detector_contract() {
        let t0 = std::time::Instant::now();
        // 无记录：单击不算
        assert!(!is_double_click(None, 0, t0));
        // 换页：不算
        assert!(!is_double_click(Some((1, t0)), 0, t0 + std::time::Duration::from_millis(10)));
        // 同页且在窗内：算
        let quick = t0 + std::time::Duration::from_millis(TAB_DOUBLE_CLICK_MS);
        assert!(is_double_click(Some((0, t0)), 0, quick));
        // 同页但超窗（+1ms）：不算
        let late = t0 + std::time::Duration::from_millis(TAB_DOUBLE_CLICK_MS)
            + std::time::Duration::from_millis(1);
        assert!(!is_double_click(Some((0, t0)), 0, late));
    }

    #[test]
    fn p65_second_rapid_click_on_same_tab_enters_rename() {
        let (mut app, _path) = loaded_real_file_app("p65-dblclick");

        // 单击：只切换，不进重命名态
        dispatch(&mut app, Message::SwitchTab(0));
        assert!(app.renaming_tab.is_none(), "单击不得触发重命名");
        assert!(app.last_tab_click.is_some(), "首次点击应记账");

        // 双击：同页在双击窗内再点一次（测试两次派发间隔远小于 500ms）
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.renaming_tab, Some(0), "双击应进入就地重命名");
        assert_eq!(app.rename_input, "note.txt", "预填当前文件名");
        assert!(!app.busy, "双击重命名不进对话框阶段");
    }

    // ---------- 第 76 轮：标签条空白区双击新建 + 窗口标题栏 Logo ----------

    #[test]
    fn p76_blank_strip_double_click_creates_tab_and_busy_ignores() {
        // 双击判定由控件层（strip 外层 mouse_area 的 on_double_click，
        // iced 内核 Click 时间+位置窗口）完成——本消息即「双击空白」的
        // 结果，update 直接建页
        let mut app = Editpad::default();
        let before = app.tabs.len();
        dispatch(&mut app, Message::TabStripBlankPressed);
        assert_eq!(app.tabs.len(), before + 1, "双击空白应新建标签页");
        assert_eq!(app.active_tab, app.tabs.len() - 1, "新页应为活动页");
        // 现有标签自身点击（SwitchTab）绝不产生空白双击消息/新建
        dispatch(&mut app, Message::SwitchTab(0));
        assert_eq!(app.tabs.len(), before + 1, "标签点击不得新建");
        // busy 期间忽略（与标签禁用一致）
        app.busy = true;
        dispatch(&mut app, Message::TabStripBlankPressed);
        assert_eq!(app.tabs.len(), before + 1, "busy 时双击空白不得新建");
        app.busy = false;
        dispatch(&mut app, Message::TabStripBlankPressed);
        assert_eq!(app.tabs.len(), before + 2, "恢复后继续有效");
    }

    // ---------- P100 勘误：P98 空白双击命中面（P100 收口） ----------

    /// P100 勘误回归（全链路真事件）：P98 把空白双击从 Fill 占位按钮改到
    /// strip 外层 `mouse_area.on_double_click` 后，strip Row 默认 Shrink
    /// 宽——外层鼠标区只盖住标签本身，右侧空白不在命中面内，双击永远收不到
    /// `TabStripBlankPressed`。修复 = view.rs 的 strip 行补 `.width(Fill)`。
    ///
    /// 本用例走**生产 view()**（非手工同构控件），经 [`ViewTree`] 夹具布局
    /// 整棵应用树，直接检验两个契约：① 标签条命中面横贯窗口宽（Shrink 时
    /// 只剩 ~标签宽，断言即红）；② 在命中面最右缘空白处连按两次左键
    /// （iced 双击窗 = 300ms 且 <6px，同一棵树状态内连发必然成对）必须发出
    /// `TabStripBlankPressed`。
    #[test]
    fn p100_strip_blank_right_edge_double_click_reaches_outer_area() {
        fn got_new_tab(messages: &[Message]) -> bool {
            messages
                .iter()
                .any(|m| matches!(m, Message::TabStripBlankPressed))
        }

        let app = Editpad::default();
        let mut ui = ViewTree::layout_default(&app);

        // 树形（Stack → container(base) → body column）：[0] 菜单栏
        // [1] 分隔线 [2] 标签条。路径对不上时先 `eprintln!("{}", ui.dump())`
        // 看实际结构；新用例优先改用 `ui.find(谓词)` 免维护。
        let strip = ui.bounds_at(&[0, 0, 2]);
        assert!(
            strip.width > 1000.0,
            "标签条命中面必须横贯窗口（P100 契约：strip 行补 width(Fill)），实际 {}；\n树形：\n{}",
            strip.width,
            ui.dump()
        );

        // 命中面最右缘（空白区）连按两次左键
        let at = iced::Point::new(strip.x + strip.width - 16.0, strip.y + strip.height * 0.5);
        let single = ui.click(at);
        assert!(!got_new_tab(&single), "单击不得触发新建");
        // 双击：同树状态内上一次点击保留，300ms/<6px 窗内成对
        let dbl = ui.double_click(at);
        assert!(
            got_new_tab(&dbl),
            "右侧空白双击必须发出 TabStripBlankPressed（命中面未覆盖空白）"
        );
    }

    // ---------- P55 就地重命名 ----------

    #[test]
    fn rename_target_path_contract() {
        let old = Path::new("C:/dir/旧名.txt");
        // 合法：同目录 + 裁剪首尾空白
        assert_eq!(
            rename_target_path(old, "  新名.md  "),
            Some(PathBuf::from("C:/dir/新名.md"))
        );
        // 空名 / 纯空白 / 路径分隔符 / Windows 非法字符 → None
        assert_eq!(rename_target_path(old, ""), None);
        assert_eq!(rename_target_path(old, "   "), None);
        assert_eq!(rename_target_path(old, "a/b"), None);
        assert_eq!(rename_target_path(old, "a\\b"), None);
        assert_eq!(rename_target_path(old, "a:b"), None);
        assert_eq!(rename_target_path(old, "a*b?"), None);
        assert_eq!(rename_target_path(old, "\"<a>|"), None);
    }

    #[test]
    fn tab_rename_inline_renames_file_and_migrates_recents() {
        let dir = scratch_dir("p55-rename");
        let old_path = dir.join("origin.txt");
        std::fs::write(&old_path, "data").unwrap();
        let config = dir.join("config.toml");

        let mut app = Editpad::default();
        // 提交链路含 persist_settings：必须注入，绝不碰真实 %APPDATA%
        app.settings_path_override = Some(config);
        dispatch(&mut app, Message::FileDropped(old_path.clone()));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("data"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 菜单「重命名」：命名页 → 就地输入框（不进对话框、不置 busy）
        dispatch(&mut app, Message::RenameOrSaveAsTab(0));
        assert_eq!(app.renaming_tab, Some(0));
        assert_eq!(app.rename_input, "origin.txt", "预填当前文件名");
        assert!(!app.busy, "就地重命名不进对话框阶段");
        let _ = app.view(); // 输入框视图可构造

        // 提交：磁盘改名 + 页路径/戳/最近文件迁移
        dispatch(&mut app, Message::TabRenameInputChanged("改名.txt".into()));
        dispatch(&mut app, Message::TabRenameCommitted);
        let new_path = dir.join("改名.txt");
        assert!(!old_path.exists(), "旧文件必须已改名");
        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap(),
            "data",
            "内容原样保留"
        );
        assert_eq!(app.tabs[0].path.as_ref(), Some(&new_path));
        assert!(app.tabs[0].file_stamp.is_some(), "新路径重记比对戳");
        assert!(app.renaming_tab.is_none() && app.rename_input.is_empty());
        assert!(app.session_manifest_stale, "清单记的是旧路径，必须置陈旧");
        // 最近文件迁移
        assert!(
            app.settings.recent_files.iter().any(|p| p.ends_with("改名.txt")),
            "最近文件必须迁移到新路径"
        );
        assert!(
            !app.settings.recent_files.iter().any(|p| p.ends_with("origin.txt")),
            "旧路径条目不得残留"
        );

        // 取消：一切保持原状
        dispatch(&mut app, Message::RenameOrSaveAsTab(0));
        assert_eq!(app.renaming_tab, Some(0));
        dispatch(&mut app, Message::TabRenameInputChanged("whatever.txt".into()));
        dispatch(&mut app, Message::TabRenameCancelled);
        assert!(app.renaming_tab.is_none());
        assert_eq!(app.tabs[0].path.as_ref(), Some(&new_path), "取消不改路径");
        assert!(new_path.exists() && !dir.join("whatever.txt").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_rename_rejects_invalid_and_colliding_names() {
        let dir = scratch_dir("p55-invalid");
        let origin = dir.join("a.txt");
        std::fs::write(&origin, "x").unwrap();
        let collide = dir.join("b.txt");
        std::fs::write(&collide, "y").unwrap();

        let mut app = Editpad::default();
        // 同名收摊分支也会 persist_settings：注入防碰真实 %APPDATA%
        app.settings_path_override = Some(dir.join("config.toml"));
        dispatch(&mut app, Message::FileDropped(origin.clone()));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("x"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 非法字符：报错并保持输入态
        dispatch(&mut app, Message::RenameOrSaveAsTab(0));
        dispatch(&mut app, Message::TabRenameInputChanged("bad:name".into()));
        dispatch(&mut app, Message::TabRenameCommitted);
        assert_eq!(app.renaming_tab, Some(0), "非法名保持输入态");
        assert!(!app.status.is_empty());
        assert!(origin.exists(), "非法名不得动磁盘");

        // 目标已存在：报错并保持输入态，两文件都原样
        dispatch(&mut app, Message::TabRenameInputChanged("b.txt".into()));
        dispatch(&mut app, Message::TabRenameCommitted);
        assert_eq!(app.renaming_tab, Some(0));
        assert!(app.status.contains("已存在"));
        assert!(origin.exists() && collide.exists());

        // 名字没变（裁剪后）：静默收摊不改磁盘
        dispatch(&mut app, Message::TabRenameInputChanged("  a.txt  ".into()));
        dispatch(&mut app, Message::TabRenameCommitted);
        assert!(app.renaming_tab.is_none());
        assert_eq!(app.tabs[0].path.as_ref(), Some(&origin));
        assert!(app.status.is_empty(), "同名收摊不报错");

        // Esc（BarsDismissed）取消
        dispatch(&mut app, Message::RenameOrSaveAsTab(0));
        dispatch(&mut app, Message::BarsDismissed);
        assert!(app.renaming_tab.is_none() && app.rename_input.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P112 页签 × 关闭按钮与悬停态 ----------

    #[test]
    fn tab_hover_state_tracks_clears_on_exit_and_on_close() {
        let mut app = app_with_tabs(2);

        // 进入页 1 → 悬停态置位（纯 UI 态：不改活动页/置脏）
        dispatch(&mut app, Message::TabHovered(Some(1)));
        assert_eq!(app.hovered_tab, Some(1));
        assert_eq!(app.active_tab, 1, "悬停不得改变活动页");
        assert!(!app.tabs[1].dirty, "悬停不得置脏");

        // 越界下标：复位为 None（防陈旧下标把悬停染色挂到错页）
        dispatch(&mut app, Message::TabHovered(Some(9)));
        assert_eq!(app.hovered_tab, None);

        // 离开页签 → None
        dispatch(&mut app, Message::TabHovered(Some(1)));
        dispatch(&mut app, Message::TabHovered(None));
        assert_eq!(app.hovered_tab, None);

        // 单页关闭把悬停页关掉 → 悬停态一并清空
        dispatch(&mut app, Message::TabHovered(Some(1)));
        dispatch(&mut app, Message::CloseTabAt(1));
        assert_eq!(app.hovered_tab, None, "关闭悬停页后悬停态必须清空");

        // 批量关闭含悬停页 → 同样清空
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::TabHovered(Some(1)));
        dispatch(&mut app, Message::CloseOtherTabs(0));
        assert_eq!(app.hovered_tab, None, "批量关闭后悬停态必须清空");
        assert_eq!(app.tabs.len(), 1, "干净页批量关闭直接移除");
    }

    #[test]
    fn view_builds_with_pill_tabs_pinned_and_hovered() {
        // P112 无头冒烟：胶囊页签（文字 + × 双按钮 + 容器悬停底）、
        // 固定页不渲染 ×、悬停染色态下视图树均可构造（container +
        // mouse_area + 双按钮的组合在构造期不 panic；× 走 CloseTabAt
        // 消息已有专用用例覆盖语义）
        let mut app = app_with_tabs(2);
        dispatch(&mut app, Message::TabHovered(Some(0)));
        dispatch(&mut app, Message::TogglePinTab(1));
        let _ = app.view();
        // 悬停离开后重建一遍（悬停态 None 分支）
        dispatch(&mut app, Message::TabHovered(None));
        let _ = app.view();
    }

    // ---------- P134：每页换行/字号独立（路线图 C7） ----------

    #[test]
    fn tab_wrap_override_cycles_three_states_and_gates_global() {
        let mut app = loaded_txt_app();
        assert_eq!(app.tab().wrap_override, None);
        assert!(!app.cur_handle.borrow().wrap_enabled(), "初始跟随全局（关）");
        // 三态循环：跟随全局 → 本页开 → 本页关 → 跟随全局
        dispatch(&mut app, Message::TabWrapOverrideToggled);
        assert_eq!(app.tab().wrap_override, Some(true));
        assert!(app.cur_handle.borrow().wrap_enabled(), "本页开");
        dispatch(&mut app, Message::TabWrapOverrideToggled);
        assert_eq!(app.tab().wrap_override, Some(false));
        // 有覆盖时全局变更不影响本页
        dispatch(&mut app, Message::SettingsWordWrapToggled(true));
        assert!(
            !app.cur_handle.borrow().wrap_enabled(),
            "本页关覆盖住全局开"
        );
        // 清除覆盖后跟随全局（已开）
        dispatch(&mut app, Message::TabWrapOverrideToggled);
        assert_eq!(app.tab().wrap_override, None);
        assert!(app.cur_handle.borrow().wrap_enabled(), "回到跟随全局");
    }

    #[test]
    fn tab_font_zoom_overrides_and_reset_follows_global() {
        let mut app = loaded_txt_app();
        let global = app.settings.font_size;
        // Ctrl+滚轮 = 本页覆盖：全局默认不动
        dispatch(&mut app, Message::TabFontSizeDelta(2.0));
        assert_eq!(app.settings.font_size, global, "Ctrl+滚轮不改全局默认");
        assert_eq!(app.tab().font_size_override, Some(global + 2.0));
        assert!((app.cur_handle.borrow().font_size() - (global + 2.0)).abs() < 0.01);
        // 设置步进器（全局）：覆盖页保持本页字号
        dispatch(&mut app, Message::FontSizeDelta(-2.0));
        assert!((app.settings.font_size - (global - 2.0)).abs() < 0.01);
        assert!(
            (app.cur_handle.borrow().font_size() - (global + 2.0)).abs() < 0.01,
            "有覆盖的页不受全局步进影响"
        );
        // 重置回跟随全局
        dispatch(&mut app, Message::TabFontSizeReset);
        assert_eq!(app.tab().font_size_override, None);
        assert!((app.cur_handle.borrow().font_size() - (global - 2.0)).abs() < 0.01);
    }


    // ---------- P145：关闭确认条下标随页集合变动修正 ----------

    #[test]
    fn close_confirm_bar_index_shifts_when_earlier_tab_removed() {
        // 确认条打开期间其前的页被关 → 确认下标必须随左移平移；旧实现
        // 存陈旧下标，下一帧视图侧 tabs[idx] 越界 panic / 指向错页。
        // 3 页：0 干净、1 置脏（确认条目标）、2 干净。
        let mut app = Editpad::default();
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d1".into())));
        dispatch(&mut app, Message::NewTab);
        // 页1 已置脏，弹确认条
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.close_tab_confirm, Some(1));
        // 关掉其后的页2（干净直关）——确认条不受影响
        dispatch(&mut app, Message::SwitchTab(2));
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.close_tab_confirm, Some(1), "其后的页被关不影响确认下标");
        // 关掉其前的页0（干净直关）——确认下标左移平移
        dispatch(&mut app, Message::SwitchTab(0));
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.close_tab_confirm, Some(0), "确认下标应随左移平移");
        assert_eq!(app.tabs.len(), 1);
        // 确认流走完：置脏页1（现下标0）放弃关闭，确认条清空
        dispatch(&mut app, Message::ConfirmCloseTabDiscard(0));
        assert_eq!(app.close_tab_confirm, None);
    }

    #[test]
    fn batch_close_clears_confirm_bar_when_confirmed_tab_in_targets() {
        // 批量移除把确认页自身也关掉 → 确认条必须清空（防陈旧下标）。
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d0".into())));
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::SwitchTab(0));
        dispatch(&mut app, Message::CloseTabRequest); // 页0 置脏 → 确认条
        assert_eq!(app.close_tab_confirm, Some(0));
        assert_eq!(app.close_tabs_now(&[0]), 1);
        assert_eq!(app.close_tab_confirm, None, "确认页自身被批量移除应清空确认条");
    }
