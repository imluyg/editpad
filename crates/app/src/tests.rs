    use super::*;
    use super::settings_ui::*;
    use iced::futures::{executor::block_on, StreamExt};

    /// 项目内落盘目录：系统 TEMP 在部分沙箱下不可写。
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("editpad-app-tests")
            .join(format!("{tag}-{}", std::process::id()));
        fs_create_dir_all(&dir);
        dir
    }

    /// std::fs 的薄封装（避免测试里到处 use std::fs）。
    fn fs_create_dir_all(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
    }

    #[test]
    fn panic_message_handles_known_payloads() {
        assert_eq!(panic_message(&"字符串字面量"), "字符串字面量");
        assert_eq!(panic_message(&(String::from("堆上字符串"))), "堆上字符串");
        assert_eq!(panic_message(&42_i32), "未知原因");
    }

    // ---------- P33 字体一致性 ----------

    #[test]
    fn font_pin_smoke_call_twice_is_safe() {
        // P33 冒烟：公开路径 iced::advanced::graphics::text::font_system()
        // 在测试环境真实可用（不只是类型层面编译通过）；AtomicBool 幂等
        // 保证二次调用直接短路。副作用仅为 fontdb 的 Family::Monospace
        // 解析目标被钉到 CJK 等宽候选——测试无渲染断言，不受影响。
        apply_default_cjk_mono_pin();
        apply_default_cjk_mono_pin();
    }

    // ---------- P8 AltGr ----------

    #[test]
    fn altgr_character_falls_through_to_text_insert() {
        use iced::keyboard::{self, key::Named};
        let altgr = keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT;

        // 德语布局 @ = AltGr+Q：winit 上报 Ctrl+Alt + 字符 q/@，
        // 不得落入 Ctrl 快捷键分支被吞，必须按普通文本插入
        let at = keyboard::Key::Character("@".into());
        assert!(matches!(
            handle_key_defaults(at, altgr),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "@"
        ));

        let q = keyboard::Key::Character("q".into());
        assert!(matches!(
            handle_key_defaults(q, altgr),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "q"
        ));

        // 纯 Ctrl 的快捷键行为不受影响
        let ctrl = keyboard::Modifiers::CTRL;
        let o = keyboard::Key::Character("O".into());
        assert!(matches!(
            handle_key_defaults(o, ctrl),
            Some(Message::OpenRequested)
        ));

        // Ctrl+未绑定字母仍返回 None（不插入；注意 x 已是 P4 剪切键）
        let q = keyboard::Key::Character("q".into());
        assert!(handle_key_defaults(q, ctrl).is_none());

        // Shift+字符（无 Ctrl）照常插入
        let bang = keyboard::Key::Character("!".into());
        assert!(matches!(
            handle_key_defaults(bang, keyboard::Modifiers::SHIFT),
            Some(Message::Edit(EditOp::InsertText(t))) if t == "!"
        ));
        let _ = Named::Tab; // 保持 import 使用
    }

    // ---------- P4 剪贴板 ----------

    #[test]
    fn ctrl_c_x_v_map_to_clipboard_messages() {
        use iced::keyboard::{self, key::Named};
        let ctrl = keyboard::Modifiers::CTRL;
        let _ = Named::Tab;

        for (letter, expected) in [
            ("c", "copy"),
            ("x", "cut"),
            ("v", "paste"),
            ("C", "copy"), // 大写（Shift 同按）也走同一快捷键
        ] {
            let key = keyboard::Key::Character(letter.into());
            let message = handle_key_defaults(key, ctrl).expect("应产生消息");
            let ok = match (&message, expected) {
                (Message::CopyRequested, "copy")
                | (Message::CutRequested, "cut")
                | (Message::PasteRequested, "paste") => true,
                _ => false,
            };
            assert!(ok, "{letter} 应映射到剪贴板消息，实际 {message:?}");
        }
    }

    #[test]
    fn pasted_text_enters_document_as_dominant_eol() {
        // P9：粘贴不再在 app 层预归一；insert_str 按文档主导行尾统一改写。
        // CRLF 文档进来什么行尾都落成 \r\n，LF 文档落成 \n（与旧版行为一致）。
        let mut crlf = editor::EditorCore::default();
        crlf.reset_document(editpad_core::Document::from_str("a\r\nb"));
        crlf.cursor = editor::CursorPos { line: 1, col: 1 }; // 文末
        crlf.insert_str("x\r\ny\rz");
        assert_eq!(crlf.doc.to_text(), "a\r\nbx\r\ny\r\nz");

        let mut lf = editor::EditorCore::default();
        lf.insert_str(&"x\r\ny\rz");
        assert_eq!(lf.doc.to_text(), "x\ny\nz");
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

    /// 构造 N 页应用：页 0 置脏（内容 "d0"），其余干净；返回应用。
    fn app_with_tabs(n: usize) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d0".into())));
        for _ in 1..n {
            dispatch(&mut app, Message::NewTab);
        }
        app
    }

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

    // ---------- P32 最近文件光标/滚动记忆 ----------

    #[test]
    fn reopened_recent_file_restores_cursor_and_scroll() {
        let mut app = Editpad::default();
        // 文档要足够大（默认视口 ~400px 高可容纳全部行时，
        // restore_view 的行程钳制会把小滚动值顶到 max+1——P30 设计语义）
        let mut text = String::new();
        for i in 1..=60 {
            text.push_str(&format!("line{i}\n"));
        }
        let doc = || editpad_core::Document::from_str(&text);

        // 首开 C:/a.txt：进最近列表（尚无记忆）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq1, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.settings.recent_view(Path::new("C:/a.txt")).is_none());

        // 种入上次退出时的位置（模拟历史记忆）
        assert!(app.settings.set_recent_view(
            Path::new("C:/a.txt"),
            editpad_core::RecentView { line: 2, col: 1, scroll_top: 5.0 },
        ));

        // 重开同一文件（当前页已命名非空 → 落新页并聚焦）→ 光标滚动就位
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq2, Ok((doc(), String::new(), "UTF-8".to_owned()))),
        );
        let ed = app.cur_handle.borrow();
        assert_eq!(
            ed.cursor,
            editor::CursorPos { line: 2, col: 1 },
            "光标应回到记忆位置"
        );
        assert_eq!(ed.scroll_top, 5.0, "垂直滚动应回到记忆位置");
    }

    #[test]
    fn closing_named_tab_writes_back_cursor_memory_to_disk() {
        let dir = scratch_dir("p32-writeback");
        let mut app = Editpad::default();
        // 注入配置路径：验证回写真的落盘，且绝不碰真实 %APPDATA%
        app.settings_path_override = Some(dir.join("config.toml"));

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("r0\nr1\nr2\nr3"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 光标挪到第 3 行（jump_to_line 1 基 → 0 基 line=2），页面干净可直接关
        app.cur_handle.borrow_mut().jump_to_line(3);
        assert!(!app.tab().dirty);
        dispatch(&mut app, Message::CloseTabAt(0));
        assert_eq!(app.tabs.len(), 1, "干净命名页应被关闭");

        let recorded = app.settings.recent_view(Path::new("C:/b.txt"));
        assert_eq!(
            recorded.map(|v| v.line),
            Some(2),
            "关页必须把光标行（0 基）写回记忆"
        );
        // 注入路径端到端：重载 config.toml 记忆仍在
        let persisted = editpad_core::Settings::load_from(&dir.join("config.toml"));
        assert_eq!(persisted.recent_view(Path::new("C:/b.txt")).map(|v| v.line), Some(2));

        fs_remove_dir_all(&dir);
    }

    #[test]
    fn window_close_writes_back_all_named_tabs_views() {
        let mut app = Editpad::default();
        let dir = scratch_dir("p32-windowclose");
        app.settings_path_override = Some(dir.join("config.toml"));

        // 两页各开一个文件，光标停在不同行
        for (name, jump) in [("C:/w1.txt", 1usize), ("C:/w2.txt", 3usize)] {
            dispatch(&mut app, Message::FileDropped(PathBuf::from(name)));
            let seq = app.job_seq;
            dispatch(
                &mut app,
                Message::Loaded(
                    seq,
                    Ok((
                        editpad_core::Document::from_str("a\nb\nc\nd\ne"),
                        String::new(),
                        "UTF-8".to_owned(),
                    )),
                ),
            );
            if jump > 1 {
                app.cur_handle.borrow_mut().jump_to_line(jump);
            }
        }
        assert_eq!(app.tabs.len(), 2);

        // 关窗请求（快照直退默认路径；目录传 None 走降级不影响断言）：
        // 入口处必须先把两页的光标都记账
        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, None);

        assert_eq!(
            app.settings.recent_view(Path::new("C:/w1.txt")).map(|v| v.line),
            Some(0),
            "未跳转的页记文首"
        );
        assert_eq!(
            app.settings.recent_view(Path::new("C:/w2.txt")).map(|v| v.line),
            Some(2),
            "跳转过光标的页按 0 基记账"
        );

        fs_remove_dir_all(&dir);
    }

    /// std::fs::remove_dir_all 的薄封装（测试尾部清理）。
    fn fs_remove_dir_all(dir: &Path) {
        std::fs::remove_dir_all(dir).ok();
    }

    // ---------- P35 打开落新页的别名失步（P32 本轮发现） ----------

    #[test]
    fn opening_into_new_tab_keeps_editor_alias_in_sync() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/a.txt")));
        let seq_a = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq_a,
                Ok((
                    editpad_core::Document::from_str("content A"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 打开 B：当前页已命名 → 落新页 idx1 并「聚焦」（active_tab 切过去）
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/b.txt")));
        let seq_b = app.job_seq;
        assert_eq!(app.active_tab, 1, "打开即聚焦新页");
        dispatch(
            &mut app,
            Message::Loaded(
                seq_b,
                Ok((
                    editpad_core::Document::from_str("content B"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 加载完成后直接输入（无任何显式切换）：必须落在聚焦的新页。
        // 失步缺陷下 cur_handle 仍指页 0——敲字打进上一个文档。
        // （打开文件后光标在文首 0:0，插入落在开头是既有正确语义；
        //  本测试钉住的是「落进哪一页」，不是行内位置。）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("+X".into())));
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "+Xcontent B",
            "输入必须落在聚焦的新页"
        );
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "content A",
            "旧页不得被误改"
        );
        // 渲染源与活动页一致
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "+Xcontent B");
    }

    #[test]
    fn preview_toggle_only_flips_for_markdown_documents() {
        // .md 扩展名经别名层得到 Markdown 语法 → 开关生效
        let mut md = Editpad::default();
        dispatch(&mut md, Message::FileDropped(PathBuf::from("C:/doc/readme.md")));
        let seq = md.job_seq;
        dispatch(
            &mut md,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("# 标题\n正文"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert!(!md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(md.preview_visible);
        dispatch(&mut md, Message::PreviewToggled);
        assert!(!md.preview_visible);

        // 非 Markdown 页：不翻转并提示
        let mut app = json_app("{}");
        dispatch(&mut app, Message::PreviewToggled);
        assert!(!app.preview_visible);
        assert!(app.status.contains("仅支持 Markdown"), "{:?}", app.status);
    }

    // ---------- P24/P25 手测反馈修复 ----------

    #[test]
    fn untitled_tabs_get_unique_sequential_names() {
        let mut app = Editpad::default();
        // 初始页即「未命名1」
        assert_eq!(app.tab().base_name(), "未命名1");
        assert_eq!(app.title(), "未命名1 - Editpad");

        // 连开两页：未命名2、未命名3，全局唯一
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::NewTab);
        let names: Vec<String> =
            app.tabs.iter().map(|t| t.base_name()).collect();
        assert_eq!(names, vec!["未命名1", "未命名2", "未命名3"]);

        // 置脏前缀进标签名但不进 title 的基础名判断之外重复
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().display_name().starts_with("● 未命名"));
    }

    #[test]
    fn closed_untitled_numbers_are_never_reused_and_saving_clears_them() {
        let mut app = Editpad::default();

        // 关闭「未命名2」（干净直接关）后新建：号码不复用，拿「未命名3」
        dispatch(&mut app, Message::NewTab);
        assert_eq!(app.tabs[1].base_name(), "未命名2");
        app.set_active_tab(1);
        dispatch(&mut app, Message::CloseTabRequest);
        assert_eq!(app.tabs.len(), 1);

        dispatch(&mut app, Message::NewTab);
        assert_eq!(
            app.tabs[1].base_name(),
            "未命名3",
            "单调分配杜绝重名"
        );

        // 另存为转正后序号清除，标签显示真实文件名
        dispatch(
            &mut app,
            Message::SaveTargetChosen(Some(PathBuf::from("C:/x/real.txt"))),
        );
        assert_eq!(
            app.tabs[1].path.as_deref(),
            Some(Path::new("C:/x/real.txt"))
        );
        assert_eq!(app.tabs[1].untitled_num, None);
        assert_eq!(app.tabs[1].base_name(), "real.txt");

        // 加载真实文件同样清除序号（Loaded 路径）
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/r/a.md")));
        let seq = app2.job_seq;
        dispatch(
            &mut app2,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("x"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app2.tabs[0].untitled_num, None);
        assert_eq!(app2.tabs[0].base_name(), "a.md");
    }

    #[test]
    fn tab_keymap_new_next_close() {
        use iced::keyboard::{self};
        let ctrl = keyboard::Modifiers::CTRL;

        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("t".into()), ctrl),
            Some(Message::NewTab)
        ));
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("w".into()), ctrl),
            Some(Message::CloseTabRequest)
        ));
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Named(Named::Tab), ctrl),
            Some(Message::SwitchTabNext)
        ));
    }

    // ---------- 健壮性边界用例批 ----------

    #[test]
    fn duplicate_open_while_loading_is_guards_and_reload_after_done() {
        // 加载进行中再次拖入同一文件：busy 守卫必须拒绝重入（防双任务竞态）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert!(app.active_load.is_some(), "首次拖入应登记加载任务");
        let seq_first = app.job_seq;

        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(app.job_seq, seq_first, "加载中不得排队第二个任务");

        // 加载完成后允许重复打开同一文件（每次新 job，无路径去重）
        let doc = editpad_core::Document::from_str("reloaded\n");
        dispatch(
            &mut app,
            Message::Loaded(seq_first, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none(), "完成后任务应解除");
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/dup.txt")));
        assert_eq!(
            app.job_seq,
            seq_first + 1,
            "完成后的重复打开必须作为新任务重新加载"
        );
    }

    #[test]
    fn close_during_load_exits_without_confirm_bar_or_stuck_state() {
        // 加载中关窗：dirty 必为 false（打开确认已清），应走直接关窗路径，
        // 不弹未保存确认条、不残留 pending 状态。
        // P29：快照直退路径有专门测试（注入目录），此处关掉开关钉住降级行为，
        // 同时避免测试触碰真实 %APPDATA%。
        let mut app = Editpad::default();
        app.settings.enable_snapshots = false;
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/big.log")));
        assert!(app.active_load.is_some());
        assert!(!app.tab().dirty);

        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert_eq!(app.main_window, Some(id), "窗口 id 应被捕获供 close 使用");
        assert!(
            !app.confirm_visible && !app.pending_close,
            "加载中关窗不得触发确认条"
        );
        // 关窗后迟到的加载完成消息照常按 job 过滤消化，不 panic
        let doc = editpad_core::Document::from_str("late arrival");
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        assert!(app.active_load.is_none());
    }

    // ---------- P29 会话快照直退（关窗状态机） ----------

    /// 项目内落盘目录：快照测试绝不触碰真实 %APPDATA%。
    fn snapshot_scratch_dir(tag: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(format!("app-p29-{tag}-{}", std::process::id()))
    }

    #[test]
    fn snapshot_exit_writes_session_and_closes_without_confirm() {
        let dir = snapshot_scratch_dir("exit");
        let mut app = Editpad::default();

        // 页 0：未命名页，已编辑置脏（未命名页正是 auto-save 的盲区）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿内容\r\n第二行".into())));
        // 页 1：命名干净页——只记元数据不写内容文件
        let mut named = Tab::empty();
        named.path = Some(PathBuf::from("C:/work/notes.md"));
        app.tabs.push(named);

        let id = iced::window::Id::unique();
        let _ = app.handle_close_request(id, Some(dir.clone()));

        assert!(!app.confirm_visible, "快照直退零询问");
        assert_eq!(app.main_window, Some(id));

        // 清单完整落盘且可解析
        let manifest = editpad_core::snapshot::read_manifest(&dir)
            .expect("关窗后必须有可解析的会话清单");
        assert!(manifest.clean_exit, "正常退出必须带收尾标记");
        assert_eq!(manifest.tabs.len(), 2);
        assert_eq!(manifest.active, 0);
        assert_eq!(manifest.next_untitled, app.untitled_next);

        // 置脏未命名页：序号延续 + 内容逐字回来
        // （插入文本经 P9 归一为文档主导行尾 LF;CRLF 保真已由 core 测试钉住）
        let t0 = &manifest.tabs[0];
        assert!(t0.dirty);
        assert_eq!(t0.path, None);
        assert_eq!(t0.untitled_num, Some(1));
        let doc = editpad_core::snapshot::read_page(&dir, t0).expect("置脏页必须有快照");
        assert_eq!(doc.to_text(), "草稿内容\n第二行");

        // 干净命名页：路径记住、无内容文件
        let t1 = &manifest.tabs[1];
        assert!(!t1.dirty);
        assert_eq!(t1.path.as_deref(), Some("C:/work/notes.md"));
        assert_eq!(t1.file, None);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn snapshot_exit_captures_cursor_and_scroll_positions() {
        let dir = snapshot_scratch_dir("cursor");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("a\nb\nc\n".into())));
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.jump_to_line(3); // 跳到末行
            ed.scroll_top = 7.5;
            ed.scroll_left = 2.0;
        }
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        let t0 = &manifest.tabs[0];
        assert_eq!(t0.cursor_line, 2, "光标行（0 基）必须被记录");
        assert!((t0.scroll_top - 7.5).abs() < f32::EPSILON);
        assert!((t0.scroll_left - 2.0).abs() < f32::EPSILON);

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn disabled_or_ask_mode_falls_back_to_confirm_bar_without_writes() {
        let dir = snapshot_scratch_dir("fallback");
        fs::create_dir_all(&dir).unwrap();

        // 开关关闭 + 置脏 → 旧确认条，且磁盘上什么都没写
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        dispatch(&mut off, Message::Edit(EditOp::InsertText("x".into())));
        let _ = off.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(off.confirm_visible, "开关关闭必须回退旧确认条");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "降级路径不得写任何快照"
        );

        // 模式为「每次询问」+ 置脏 → 同样回退
        let mut ask = Editpad::default();
        ask.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut ask, Message::Edit(EditOp::InsertText("y".into())));
        let _ = ask.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(ask.confirm_visible);
        assert!(editpad_core::snapshot::read_manifest(&dir).is_none());

        // 目录不可用（None）→ 功能自动降级，同样回退确认条
        let mut nodir = Editpad::default();
        dispatch(&mut nodir, Message::Edit(EditOp::InsertText("z".into())));
        let _ = nodir.handle_close_request(iced::window::Id::unique(), None);
        assert!(nodir.confirm_visible, "无快照目录时必须降级而不是丢数据");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_failure_degrades_to_confirm_bar_with_status_reason() {
        // 用一个普通文件冒充快照目录 → create_dir_all 必败
        let dir = snapshot_scratch_dir("fail");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, "").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText(" precious ".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));

        assert!(app.confirm_visible, "快照失败必须降级回确认条保住数据");
        assert!(
            app.status.contains("会话快照失败"),
            "失败原因要留在状态栏:{:?}",
            app.status
        );
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "失败的提交不得产生清单"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discard_and_close_clears_snapshot_area() {
        let dir = snapshot_scratch_dir("discard");
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("will discard".into())));
        // 先正常快照一次（模拟上一轮退出留下的会话记录）
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(editpad_core::snapshot::read_manifest(&dir).is_some());

        // 新会话里用户选择「放弃更改」→ 快照区必须一并清空，
        // 否则下次启动会把已放弃的内容当会话恢复回来
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("new session".into())));
        let _ = app2.discard_all_and_close(Some(dir.clone()));
        assert!(!app2.confirm_visible && !app2.any_dirty());
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "放弃语义 = 连快照一起丢"
        );
        assert!(!dir.exists(), "清场应删除整个快照目录");

        // 开关关闭时放弃不清场（本来就没有快照承诺）
        let mut off = Editpad::default();
        off.settings.enable_snapshots = false;
        let _ = off.discard_all_and_close(Some(dir.clone()));
        fs::remove_dir_all(&dir).ok();
    }

    // ---------- P30 启动会话恢复 ----------

    /// 构造一个干净命名页的清单元数据（测试辅助）。
    fn clean_named_tab(path: &str) -> editpad_core::snapshot::SessionTab {
        editpad_core::snapshot::SessionTab {
            path: Some(path.to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
        }
    }

    #[test]
    fn session_restore_rebuilds_tabs_content_active_and_pending_loads() {
        let dir = snapshot_scratch_dir("p30-restore");

        // 准备上一轮正常退出留下的会话：置脏未命名页（内容+光标）+ 干净命名页
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(4),
            dirty: true,
            file: None,
            cursor_line: 1,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/notes.md".to_owned()),
            untitled_num: None,
            dirty: false,
            file: None,
            cursor_line: 10,
            cursor_col: 3,
            scroll_top: 50.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("第一行\r\n草稿内容"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            7,
        )
        .expect("写会话应成功");
        assert!(manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 0, "激活页按清单还原");
        assert!(app.recover_prompt.is_none(), "clean_exit=true 静默恢复，不弹提示条");

        // 置脏未命名页：内容逐字还原 + 光标就位 + 编号延续
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "第一行\r\n草稿内容");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(4));
        let ed0 = app.tabs[0].editor.borrow();
        assert_eq!((ed0.cursor.line, ed0.cursor.col), (1, 2), "光标回到退出时位置");
        drop(ed0);
        assert_eq!(app.untitled_next, 7, "未命名编号从清单延续单调性");

        // 干净命名页：占位页已建、加载任务已自动开跑（不抢焦点）
        assert!(app.restore_placeholder_ready(1));
        let job = app.active_load.as_ref().expect("命名页应立即开载").clone();
        assert_eq!(job.tab, 1);
        assert!(app.busy);
        assert_eq!(app.active_tab, 0, "恢复链绝不把用户拽到加载页");

        // 加载完成：内容回填 + 视图回填 + 收尾
        let body = editpad_core::Document::from_str(&"line\n".repeat(30));
        dispatch(
            &mut app,
            Message::Loaded(job.id, Ok((body, String::new(), "UTF-8".to_owned()))),
        );
        assert_eq!(app.tabs[1].path.as_deref(), Some(Path::new("C:/w/notes.md")));
        assert!(!app.tabs[1].dirty);
        let ed1 = app.tabs[1].editor.borrow();
        assert_eq!(
            (ed1.cursor.line, ed1.cursor.col),
            (10, 3),
            "加载完成的命名页同样回填光标"
        );
        assert!(
            ed1.scroll_top > 0.0 && ed1.scroll_top <= 50.0,
            "滚动值被应用且经行程钳制，实际 {}",
            ed1.scroll_top
        );
        drop(ed1);
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0, "恢复链排空");
        assert!(app.status.is_empty(), "全部成功的恢复不打扰状态栏");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// P43 回归：新建标签页唯一出口必须套用全局字号设置
    /// （曾有多处直接 `Tab::empty()`，编辑器默认 16px 脱离设置）。
    #[test]
    fn fresh_tab_carries_global_font_size() {
        let mut app = Editpad::default();
        app.settings.font_size = 22.0;
        let tab = app.fresh_tab();
        assert_eq!(tab.editor.borrow().font_size(), 22.0);

        // 归一规则与 boot 同源：非法（超界/负）值收敛到合法区间
        let mut app = Editpad::default();
        app.settings.font_size = 9999.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            editor::normalize_font_size(9999.0)
        );
        let mut app = Editpad::default();
        app.settings.font_size = -5.0;
        assert_eq!(
            app.fresh_tab().editor.borrow().font_size(),
            editor::normalize_font_size(-5.0)
        );
    }

    /// P43 回归：会话恢复（P30）创建的标签页必须套用全局字号——
    /// 曾恢复路径整体替换 tabs，boot 只给恢复前默认页应用过字号，
    /// 表现为「字号调到 24 生效，重启后字体回到原大小，设置面板仍 24」。
    #[test]
    fn restored_tabs_carry_global_font_size() {
        let dir = snapshot_scratch_dir("p43-restore-font-size");
        let tab = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("待恢复的草稿"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        app.settings.font_size = 24.0;
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().font_size(),
            24.0,
            "恢复页必须应用全局字号设置"
        );
        assert_eq!(app.settings.font_size, 24.0, "设置本身不受影响");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn dirty_named_tab_restores_snapshot_instead_of_disk_reload() {
        let dir = snapshot_scratch_dir("p30-dirty-named");
        let tab = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/doc.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 4,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[editpad_core::snapshot::SessionPage {
                tab,
                doc: editpad_core::Document::from_str("磁盘上没有的未保存内容"),
            }],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].editor.borrow().doc.to_text(),
            "磁盘上没有的未保存内容",
            "v1 一律信快照（所见即所得），绝不从磁盘重载覆盖未存改动"
        );
        assert!(app.tabs[0].dirty, "恢复出的置脏页保持置脏");
        assert_eq!(app.tabs[0].encoding_label, "UTF-8", "快照恒为 UTF-8 落盘");
        assert!(app.active_load.is_none(), "快照同步还原，无需排队加载");
        assert!(app.status.is_empty());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restored_dirty_pages_stay_dirty_when_undo_revisits_snapshot_state() {
        // P38 × P30 交互回归：恢复出来的置脏页不得把快照内容当落盘基线，
        // 否则「编辑 → 撤销回快照态」会错误清脏 → 零询问退出后下次启动
        // 命名页从磁盘旧内容重载、未命名页直接空白——未存改动静默丢失。
        let dir = snapshot_scratch_dir("p38-restore-baseline");
        let untitled = editpad_core::snapshot::SessionTab {
            path: None,
            untitled_num: Some(3),
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 2,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let named = editpad_core::snapshot::SessionTab {
            path: Some("C:/w/report.txt".to_owned()),
            untitled_num: None,
            dirty: true,
            file: None,
            cursor_line: 0,
            cursor_col: 0,
            scroll_top: 0.0,
            scroll_left: 0.0,
        };
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: untitled,
                    doc: editpad_core::Document::from_str("草稿"),
                },
                editpad_core::snapshot::SessionPage {
                    tab: named,
                    doc: editpad_core::Document::from_str("磁盘上没有的修改"),
                },
            ],
            0,
            4,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs[0].dirty && app.tabs[1].dirty);

        // 未命名页：编辑一步再撤销回快照态——必须保持置脏（无基线保守）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        assert!(app.tabs[0].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(
            app.tabs[0].dirty,
            "撤销回快照态不清脏：内容从未落盘（P30 恢复页基线已清空）"
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "草稿");

        // 置脏命名页同理：磁盘还是旧内容，快照态不算「已保存」
        dispatch(&mut app, Message::SwitchTab(1));
        dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
        assert!(app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(app.tabs[1].dirty, "命名恢复页撤销回快照态同样保持置脏");
        assert_eq!(
            app.tabs[1].editor.borrow().doc.to_text(),
            "磁盘上没有的修改"
        );

        // 真正落盘一次后基线重建，撤销回清能力恢复正常语义
        let v = app.tabs[1].version;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                1,
                v,
                PathBuf::from("C:/w/report.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tabs[1].dirty);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.tabs[1].dirty, "落盘之后撤销回基线应正常回清");

        editpad_core::snapshot::clear_session(&dir);
    }

    /// 在指定目录种一个「异常退出中间态」会话（clean_exit=false，
    /// 即 P31 心跳写的清单形态）：手工落一份可解析 TOML + 真实页文件。
    fn plant_orphan_session(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("s7-t0.snap"), "崩溃前的未保存草稿").unwrap();
        fs::write(
            dir.join(editpad_core::snapshot::MANIFEST_NAME),
            concat!(
                "generation = 7\n",
                "active = 0\n",
                "next_untitled = 3\n",
                "clean_exit = false\n",
                "\n",
                "[[tabs]]\n",
                "untitled_num = 2\n",
                "dirty = true\n",
                "file = \"s7-t0.snap\"\n",
                "cursor_line = 0\n",
                "cursor_col = 3\n",
            ),
        )
        .unwrap();
    }

    #[test]
    fn crash_orphan_manifest_prompts_then_restores_or_discards() {
        // 「恢复」路径：弹一次性提示条 → 接受后全量重建（含置脏内容）
        let dir_a = snapshot_scratch_dir("p30-orphan-a");
        plant_orphan_session(&dir_a);
        let manifest = editpad_core::snapshot::read_manifest(&dir_a).unwrap();
        assert!(!manifest.clean_exit);

        let mut app = Editpad::default();
        let _ = app.restore_from_dir(&dir_a);
        assert!(app.recover_prompt.is_some(), "异常退出必须弹一次性恢复条");
        assert_eq!(app.tabs.len(), 1, "未裁决前不得动当前标签");

        let _ = app.accept_session_recover(Some(dir_a.clone()));
        assert!(app.recover_prompt.is_none());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "崩溃前的未保存草稿");
        assert!(app.tabs[0].dirty);
        assert_eq!(app.tabs[0].untitled_num, Some(2), "编号延续");
        assert_eq!(app.untitled_next, 3, "计数器取清单的 next_untitled");
        editpad_core::snapshot::clear_session(&dir_a);

        // 「丢弃」路径：连快照一起丢（P29 放弃语义同族），空白起步
        let dir_b = snapshot_scratch_dir("p30-orphan-b");
        plant_orphan_session(&dir_b);
        let mut app2 = Editpad::default();
        let _ = app2.restore_from_dir(&dir_b);
        assert!(app2.recover_prompt.is_some());
        let _ = app2.discard_session_recover(Some(dir_b.clone()));
        assert!(app2.recover_prompt.is_none());
        assert!(
            editpad_core::snapshot::read_manifest(&dir_b).is_none(),
            "丢弃语义必须清场，防下次启动复活已弃内容"
        );
        fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn remember_session_disabled_means_blank_start_and_ask_style_close() {
        // 总开关判定纯函数：两个开关都开才允许
        assert!(session_restore_allowed(true, true));
        assert!(!session_restore_allowed(false, true), "快照总开关是前提");
        assert!(!session_restore_allowed(true, false), "关闭会话记忆 = 不恢复也不写清单");

        // 退出侧：remember_session 关闭时关窗回退旧确认条，不写任何会话数据
        let dir = snapshot_scratch_dir("p30-gate");
        let mut app = Editpad::default();
        app.settings.remember_session = false;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("keep me".into())));
        let _ = app.handle_close_request(iced::window::Id::unique(), Some(dir.clone()));
        assert!(app.confirm_visible, "回退旧确认条而不是静默丢改动");
        assert!(
            editpad_core::snapshot::read_manifest(&dir).is_none(),
            "记住会话关闭后退出不写清单（§3 P30 第 4 条）"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_skips_unloadable_page_and_keeps_chain_going() {
        let dir = snapshot_scratch_dir("p30-fail");
        let manifest = editpad_core::snapshot::write_session(
            &dir,
            &[
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/gone.txt"),
                    doc: editpad_core::Document::new(),
                },
                editpad_core::snapshot::SessionPage {
                    tab: clean_named_tab("C:/w/stays.txt"),
                    doc: editpad_core::Document::new(),
                },
            ],
            0,
            2,
        )
        .unwrap();

        let mut app = Editpad::default();
        let _ = app.restore_from_manifest(&dir, &manifest);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.restore_pending, 2);
        let seq_a = app.active_load.as_ref().unwrap().id;

        // 第一页加载失败（文件已被删等）：占位移除、失败计数、
        // 续排同步完成——B 已顶上载通道，队列排空
        dispatch(&mut app, Message::Loaded(seq_a, Err("文件不存在".to_owned())));
        assert_eq!(app.tabs.len(), 1, "失败占位页应被移除（单页失败不阻断）");
        assert_eq!(app.restore_failed, 1);
        assert!(
            app.restore_queue.is_empty(),
            "续排是同步的：下一页应已登记为在途任务"
        );

        // 第二页自动续排并成功落地
        let job_b = app.active_load.as_ref().expect("失败后必须续排下一页").clone();
        assert_eq!(job_b.tab, 0, "队列下标必须随页面移除整体左移");
        dispatch(
            &mut app,
            Message::Loaded(
                job_b.id,
                Ok((
                    editpad_core::Document::from_str("b body"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.tabs[0].editor.borrow().doc.to_text(), "b body");
        assert_eq!(app.tabs[0].path.as_deref(), Some(Path::new("C:/w/stays.txt")));
        assert!(app.active_load.is_none());
        assert_eq!(app.restore_pending, 0);
        assert!(
            app.status.contains("会话恢复完成") && app.status.contains("1 页未能恢复"),
            "失败页要有状态栏汇总:{:?}",
            app.status
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn restore_plan_prioritizes_active_page_and_truncates_over_cap() {
        let dir = snapshot_scratch_dir("p30-plan");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f100.snap"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("f200.snap"), vec![0u8; 200]).unwrap();

        // t0 = 干净命名页（估算读其磁盘文件 100B）；t1 = 置脏页（快照 200B）；
        // t2 = 干净未命名页（空文档，0B）
        let manifest = |active: usize| editpad_core::snapshot::SessionManifest {
            generation: 1,
            tabs: vec![
                clean_named_tab(&dir.join("f100.snap").display().to_string()),
                editpad_core::snapshot::SessionTab {
                    file: Some("f200.snap".to_owned()),
                    dirty: true,
                    ..clean_named_tab("")
                },
                editpad_core::snapshot::SessionTab {
                    untitled_num: Some(5),
                    ..clean_named_tab("")
                },
            ],
            active,
            next_untitled: 6,
            clean_exit: true,
        };

        // 宽松上限：全保
        let (kept, dropped) = plan_restore_order(&manifest(0), &dir, u64::MAX);
        assert_eq!((kept, dropped), (vec![0, 1, 2], 0));

        // 250B 上限、激活页是 200B 快照页：先保激活页 → 100B 页出局
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 250);
        assert_eq!((kept, dropped), (vec![1, 2], 1));

        // 150B 上限、激活页是 200B 快照页：激活页自身超限同样出局
        // （「先保激活页」是装填优先级，不是护栏豁免权），
        // 随后 100B 页与空页照常装填
        let (kept, dropped) = plan_restore_order(&manifest(1), &dir, 150);
        assert_eq!((kept, dropped), (vec![0, 2], 1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loaded_language_routes_through_alias_and_sniff() {
        // P22：扩展名别名层 —— .log 命中「Editpad Log」内嵌语法
        // （即使内容长得像 JSON，扩展名可信优先）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/logs/app.log")));
        let doc = editpad_core::Document::from_str("{\"level\":1}");
        let sample = "{\"level\":1}";
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, sample.to_owned(), "UTF-8".to_owned()))),
        );
        assert_eq!(
            app.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Editpad Log")
        );

        // 无扩展名 + shebang 样本 → 内容嗅探接管
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/bin/build")));
        let doc2 = editpad_core::Document::from_str("#!/bin/sh\necho hi\n");
        let seq2 = app2.job_seq;
        dispatch(
            &mut app2,
            Message::Loaded(
                seq2,
                Ok((doc2, "#!/bin/sh\necho hi\n".to_owned(), "UTF-8".to_owned())),
            ),
        );
        assert_eq!(
            app2.cur_handle.borrow().highlight_syntax_name().as_deref(),
            Some("Bourne Again Shell (bash)")
        );
    }

    // ---------- 查找/替换转义解析（\n \r \t \\） ----------

    #[test]
    fn unescape_resolves_known_escapes_and_keeps_unknown_literal() {
        assert_eq!(unescape_query("\\n"), "\n");
        assert_eq!(unescape_query("a\\tb"), "a\tb");
        assert_eq!(unescape_query("\\r"), "\r");
        assert_eq!(unescape_query("a\\\\b"), "a\\b");
        // 未知转义保持原样两个字符；孤立反斜杠保持字面
        assert_eq!(unescape_query("\\q"), "\\q");
        assert_eq!(unescape_query("trailing\\"), "trailing\\");
        // 无转义内容不受影响
        assert_eq!(unescape_query("plain 中文 🚀"), "plain 中文 🚀");
    }

    #[test]
    fn find_and_replace_support_escaped_tab_and_newline() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/a.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("a\tb"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 查找 \t 并替换为换行：输入框里的字面反斜杠序列被解析成真实控制字符
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("\\t".into()));
        dispatch(&mut app, Message::ReplaceQueryChanged("\\n".into()));
        let raw_query_kept = app.find_query.clone();
        assert_eq!(raw_query_kept, "\\t", "输入框保留用户原始输入");
        // 模拟在途扫描已完成（测试中任务被丢弃，不会自动清除）
        app.find_scan = None;

        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\nb",
            "替换文本中的 \\n 转义应成为真实换行"
        );
        assert!(app.tab().dirty);
    }

    // ---------- P22 第二批：格式化 JSON ----------

    /// 构造一个已按 .json 加载完成的应用。
    fn json_app(text: &str) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/x/data.json")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str(text);
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((doc, String::new(), "UTF-8".to_owned())),
            ),
        );
        app
    }

    #[test]
    fn format_json_pretty_prints_and_is_revertible() {
        let mut app = json_app("{\"b\":1,\"a\":[2,3]}");
        dispatch(&mut app, Message::FormatJson);
        let expected = "{\n  \"b\": 1,\n  \"a\": [\n    2,\n    3\n  ]\n}";
        assert_eq!(app.cur_handle.borrow().doc.to_text(), expected);
        assert!(app.tab().dirty, "格式化属于内容修改，必须置脏");
        assert_eq!(app.status, "已格式化 JSON");

        // 可撤销：replace_whole_document 走快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{\"b\":1,\"a\":[2,3]}",
            "撤销应还原到格式化前"
        );
    }

    #[test]
    fn format_json_reports_error_position_and_keeps_document() {
        let bad = "{\"a\": 1,,}";
        let mut app = json_app(bad);
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            bad,
            "校验失败不得改动文档"
        );
        assert!(
            app.status.contains("JSON 格式化失败") && app.status.contains("第"),
            "状态栏应带出错误行列，实际 {:?}",
            app.status
        );
    }

    #[test]
    fn format_json_is_noop_for_non_json_documents() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/notes/plain.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("{not:json,but:plain txt}");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc, String::new(), "UTF-8".to_owned()))),
        );
        dispatch(&mut app, Message::FormatJson);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "{not:json,but:plain txt}",
            "非 JSON 文档不得被改动"
        );
        assert!(app.status.contains("仅对 JSON"), "应提示语法不匹配");
    }

    #[test]
    fn ctrl_shift_f_maps_to_format_json_but_ctrl_f_stays_find() {
        use iced::keyboard::{self};
        let shift_ctrl = keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT;
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("F".into()), shift_ctrl),
            Some(Message::FormatJson)
        ));
        // 普通 Ctrl+F 不受影响
        assert!(matches!(
            handle_key_defaults(keyboard::Key::Character("f".into()), keyboard::Modifiers::CTRL),
            Some(Message::FindToggled)
        ));
    }

    // ---------- P18 即时保存 ----------

    /// 构造一个已按 .txt 加载完成的应用（纯文本路径）。
    fn loaded_txt_app() -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/doc/note.txt")));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((editpad_core::Document::from_str("base"), String::new(), "UTF-8".to_owned())),
            ),
        );
        app
    }

    #[test]
    fn edit_schedules_single_inflight_autosave_and_success_clears_dirty() {
        let mut app = loaded_txt_app();
        // P63：即时保存改为显式选择——本测试验证的是开启后的机制
        assert!(!app.settings.autosave_enabled);
        app.settings.autosave_enabled = true;

        // 编辑置脏并派发防抖任务（版本号在页上推进）
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(
            app.tabs[0].autosave_inflight,
            "首次编辑应排队本页防抖任务"
        );
        let scheduled_version = app.tab().version;

        // 连续再编辑：inflight 去重不重复排队；版本继续推进
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        assert!(app.tabs[0].autosave_inflight);
        assert_eq!(app.tab().version, scheduled_version + 1);

        // 任务回报且版本一致 → 清脏解除挂起
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                scheduled_version + 1,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tab().dirty, "版本一致时落盘应清脏");
        assert!(!app.tabs[0].autosave_inflight);
    }

    #[test]
    fn autosave_stale_version_keeps_dirty() {
        // 快照之后又有编辑：迟到的「保存成功」不得清脏（否则丢改动标记）
        let mut app = loaded_txt_app();
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let stale = app.tab().version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        dispatch(
            &mut app,
            Message::TabAutosaved(0, stale, PathBuf::from("C:/doc/note.txt"), AutosaveOutcome::Written),
        );

        assert!(
            app.tab().dirty && !app.tabs[0].autosave_inflight,
            "版本不符应保持置脏并解除挂起"
        );
    }

    #[test]
    fn autosave_failure_traces_status_keeps_dirty_and_allows_requeue() {
        let mut app = loaded_txt_app();
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tabs[0].autosave_inflight);

        let version = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                version,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Failed("disk full".into()),
            ),
        );

        assert!(!app.tabs[0].autosave_inflight, "失败也要解除挂起");
        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(
            app.status.contains("自动保存失败") && app.status.contains("disk full"),
            "失败必须留痕不能无声吞掉，实际 {:?}",
            app.status
        );

        // 失败解除挂起后，下一次编辑仍可重新排队
        dispatch(&mut app, Message::Edit(EditOp::InsertText("z".into())));
        assert!(app.tabs[0].autosave_inflight, "新编辑应重新排队");
    }

    #[test]
    fn autosave_skipped_for_untitled_or_disabled() {
        // 未命名文档（path == None）：绝不自动落盘
        let mut untitled = Editpad::default();
        dispatch(&mut untitled, Message::Edit(EditOp::InsertText("x".into())));
        assert!(untitled.tab().dirty);
        assert!(
            !untitled.tabs[0].autosave_inflight,
            "未命名文档不参与自动保存"
        );

        // 开关关闭（P63 起即默认状态）：已命名页同样不排队
        let mut app = loaded_txt_app();
        assert!(!app.settings.autosave_enabled, "P63：默认关闭");
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(!app.tabs[0].autosave_inflight, "开关关闭时不排队");

        // busy（手动 IO 进行中）时也跳过——显式开启后守卫仍生效
        let mut busy_app = loaded_txt_app();
        busy_app.settings.autosave_enabled = true;
        busy_app.busy = true;
        dispatch(&mut busy_app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(!busy_app.tabs[0].autosave_inflight, "busy 时不得排队自动保存");
    }

    // ---------- P63 保存策略对标主流编辑器 ----------

    /// 构造「真实落盘文件 + 已按该文件加载完成」的应用：
    /// `file_stamp` 为真实磁盘戳，供外部修改守卫测试使用。
    fn loaded_real_file_app(tag: &str) -> (Editpad, PathBuf) {
        let path = scratch_dir(tag).join("note.txt");
        std::fs::write(&path, "base").unwrap();
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("base"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        (app, path)
    }

    #[test]
    fn p63_default_settings_never_autowrite_existing_files() {
        // 用户点名（对标主流编辑器）：默认设置下编辑已有文件只置脏，
        // 绝不悄悄排队写盘；● 标记与关窗确认照旧兜底
        let (mut app, path) = loaded_real_file_app("p63-default-off");

        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);
        assert!(
            !app.tabs[0].autosave_inflight,
            "默认关闭时编辑不得触发写盘任务"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "base",
            "默认设置下磁盘必须原封不动"
        );
    }

    #[test]
    fn p63_autosave_skip_on_external_change_queues_prompt_and_keeps_dirty() {
        // 防抖睡眠期间文件被外部改动 → 写前校验拒写：保持置脏、送入
        // P52 提示条队列、留痕状态栏，未裁决前不重记戳
        let (mut app, path) = loaded_real_file_app("p63-skip-ext");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tabs[0].autosave_inflight);
        let old_stamp = app.tabs[0].file_stamp;
        let version = app.tab().version;

        fs::write(&path, "external edit made this longer").unwrap();

        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                version,
                path.clone(),
                AutosaveOutcome::SkippedExternalChange,
            ),
        );
        assert!(app.tab().dirty, "拒写必须保持置脏");
        assert!(!app.tabs[0].autosave_inflight, "挂起照常解除，可重新排队");
        assert_eq!(
            app.external_change,
            Some(vec![0]),
            "拒写应把页送进 P52 提示条队列"
        );
        assert!(app.status.contains("外部修改"), "实际 {:?}", app.status);
        assert_eq!(
            app.tabs[0].file_stamp, old_stamp,
            "用户裁决前不得按磁盘现状重记戳"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "external edit made this longer",
            "拒写后磁盘内容必须还是外部版本"
        );
    }

    #[test]
    fn p63_written_outcome_clears_dirty_and_mismatched_path_report_is_dropped() {
        // 正常路径：路径匹配 + 版本一致 → 清脏（新载荷下的既有语义）
        let (mut app, path) = loaded_real_file_app("p63-written");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let v = app.tab().version;

        // 先来一条路径不符的回报（下标漂移竞态）——必须整条丢弃，
        // 不得清脏/解除挂起；随后正确路径的同版本回报照常生效
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                v,
                PathBuf::from("C:/other/renamed.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(
            app.tab().dirty && app.tabs[0].autosave_inflight,
            "路径不符的回报必须整条丢弃"
        );

        dispatch(
            &mut app,
            Message::TabAutosaved(0, v, path, AutosaveOutcome::Written),
        );
        assert!(!app.tab().dirty && !app.tabs[0].autosave_inflight);
    }

    #[test]
    fn p63_manual_save_blocked_on_external_change_until_acknowledged() {
        // 应用聚焦期间文件被外部改（无焦点事件 → P50 巡检不触发），
        // Ctrl+S 必须拦截而不是无声覆盖；〔忽略〕重记戳后再存 = 有意覆盖
        let (mut app, path) = loaded_real_file_app("p63-manual-guard");
        std::fs::write(&path, "externally replaced").unwrap();

        dispatch(&mut app, Message::SaveRequested);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "externally replaced",
            "拦截期间原文件绝不能被覆盖"
        );
        assert!(!app.busy, "拦截不是进入保存流程");
        assert_eq!(app.external_change, Some(vec![0]), "应弹 P52 提示条交裁决");
        assert!(app.status.contains("外部修改"), "实际 {:?}", app.status);

        // 〔忽略〕= 按磁盘现状重记戳并收条；随后 Ctrl+S 守卫放行进入保存管线
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::SaveRequested);
        assert!(app.busy, "有意覆盖的第二步应正常走保存");
    }

    #[test]
    fn p63_save_as_target_is_restamped_not_blocked() {
        // 另存为：对话框里显式选中的目标（可能已存在且内容不同）不该被
        // 自家外部修改守卫拦下——选定目标即按其磁盘现状记戳
        let target = scratch_dir("p63-saveas").join("existing-target.txt");
        std::fs::write(&target, "old content on disk").unwrap();

        let mut app = Editpad::default(); // 未命名页，file_stamp = None
        dispatch(&mut app, Message::Edit(EditOp::InsertText("draft".into())));
        dispatch(&mut app, Message::SaveTargetChosen(Some(target.clone())));

        assert_eq!(app.tabs[0].path, Some(target.clone()));
        assert_eq!(
            app.tabs[0].file_stamp,
            file_stamp(&target),
            "选定目标即按其磁盘现状重记戳"
        );
        assert!(app.busy, "另存为应直接进入保存管线而不被拦");
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

    #[test]
    fn p64_rename_input_id_is_stable_for_focus_operation() {
        // 聚焦操作按 id 查找控件：每次调用必须产出相等的 id（逐帧稳定），
        // 且与视图侧 text_input 挂载的是同一个值（同源常量函数）
        let a = rename_input_id();
        let b = rename_input_id();
        assert_eq!(a, b, "id 必须逐帧稳定，聚焦操作才能命中输入框");
    }

    #[test]
    fn p63_late_skip_after_own_manual_save_is_silently_dropped() {
        // 误报抑制：编辑→Ctrl+S 手动落盘（磁盘与记录戳同步刷新）→ 在途
        // 防抖线程拿着旧期望戳迟到拒写——这是自己的保存不是外部改动，
        // 必须静默丢弃，不得弹提示条
        let (mut app, path) = loaded_real_file_app("p63-late-skip");
        app.settings.autosave_enabled = true;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let v = app.tab().version;

        // 手动保存走完整链路：进入 busy → Saved 回报清脏并按新磁盘重记戳
        dispatch(&mut app, Message::SaveRequested);
        assert!(app.busy);
        std::fs::write(&path, "our own newer content").unwrap();
        dispatch(
            &mut app,
            Message::Saved(v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert!(!app.tab().dirty && !app.busy);
        let fresh_stamp = app.tabs[0].file_stamp;

        // 迟到的拒写回报（线程的期望戳仍是载入时刻的旧戳）
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                v,
                path.clone(),
                AutosaveOutcome::SkippedExternalChange,
            ),
        );
        assert!(
            app.external_change.is_none(),
            "自己的手动保存不得触发外部修改提示条"
        );
        assert!(!app.status.contains("外部修改"), "实际 {:?}", app.status);
        assert_eq!(app.tabs[0].file_stamp, fresh_stamp, "记录戳保持不动");

        // 对照：真外部改动后磁盘 ≠ 记录戳，同样的迟到回报必须入队
        std::fs::write(&path, "genuinely external").unwrap();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        let v2 = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(0, v2, path, AutosaveOutcome::SkippedExternalChange),
        );
        assert_eq!(app.external_change, Some(vec![0]), "真外部改动必须入队裁决");
    }

    // ---------- P67 编码与行尾控制 ----------

    #[test]
    fn p67_status_menus_toggle_mutually_and_esc_closes() {
        let mut app = loaded_txt_app();

        dispatch(&mut app, Message::ToggleEncodingMenu);
        eprintln!("[P67] after enc-toggle: enc={} eol={}", app.encoding_menu, app.eol_menu);
        assert!(app.encoding_menu && !app.eol_menu);

        // 互斥：开行尾关编码
        dispatch(&mut app, Message::ToggleEolMenu);
        assert!(!app.encoding_menu && app.eol_menu);

        // busy 时不得「打开」菜单（已开的菜单由各项自身守卫兜底）
        dispatch(&mut app, Message::ToggleEolMenu); // 再 toggle 一次 = 收起
        assert!(!app.encoding_menu && !app.eol_menu, "两菜单此时应全关");
        app.busy = true;
        dispatch(&mut app, Message::ToggleEncodingMenu);
        assert!(!app.encoding_menu, "busy 时不得开菜单");
        app.busy = false;

        // Esc（BarsDismissed）一并收起
        dispatch(&mut app, Message::ToggleEncodingMenu);
        assert!(app.encoding_menu);
        dispatch(&mut app, Message::BarsDismissed);
        assert!(!app.encoding_menu && !app.eol_menu);
    }

    #[test]
    fn p67_save_with_encoding_sets_pref_and_enters_save_pipeline() {
        let (mut app, _path) = loaded_real_file_app("p67-enc-pref");

        dispatch(&mut app, Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk));
        assert_eq!(
            app.tabs[0].save_encoding,
            Some(editpad_core::SaveEncoding::Gbk),
            "偏好必须记住（此后每次保存沿用）"
        );
        assert!(app.busy, "选择编码后应立即走保存管线");
    }

    #[test]
    fn p67_save_with_encoding_rejected_for_untitled() {
        let mut app = Editpad::default(); // 未命名页
        dispatch(&mut app, Message::SaveWithEncoding(editpad_core::SaveEncoding::Gbk));
        assert!(app.tabs[0].save_encoding.is_none(), "无路径不得记偏好");
        assert!(!app.busy);
        assert!(app.status.contains("另存为"), "应提示先另存为");
    }

    #[test]
    fn p67_saved_reflects_target_label_and_unmappable_warning() {
        let (mut app, _path) = loaded_real_file_app("p67-saved-label");
        app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);
        // 模拟载入自 GBK 文件（标签为 GBK）
        app.tabs[0].encoding_label = "GBK".to_owned();
        let v = app.tab().version;

        // 同编码保存：无转码提示
        dispatch(
            &mut app,
            Message::Saved(v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert_eq!(app.tabs[0].encoding_label, "GBK", "标签反映实际落盘编码");
        assert!(app.status.is_empty(), "同编码不得提示转码，实际 {:?}", app.status);

        // 不可映射：优先告警
        app.tabs[0].encoding_label = "GBK".to_owned();
        dispatch(
            &mut app,
            Message::Saved(
                v,
                Ok(editpad_core::EncodeNotice { unmappable: true }),
            ),
        );
        assert!(app.status.contains("&#"), "应提示数值实体写入：{:?}", app.status);

        // 默认（无偏好）保存 GBK 载入的文件 → 转码提示（P6 语义保持）
        app.tabs[0].save_encoding = None;
        app.tabs[0].encoding_label = "GBK".to_owned();
        dispatch(
            &mut app,
            Message::Saved(v, Ok(editpad_core::EncodeNotice::default())),
        );
        assert_eq!(app.tabs[0].encoding_label, "UTF-8");
        assert!(app.status.contains("GBK"), "应提示原编码：{:?}", app.status);
    }

    #[test]
    fn p67_convert_eol_rewrites_document_undoably() {
        let (mut app, _path) = loaded_real_file_app("p67-eol");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("a\nb\nc\n"));
        }
        let v0 = app.tab().version;

        // LF → CRLF：内容改写、置脏、版本推进
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::CrLf));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\r\nb\r\nc\r\n",
            "行尾应统一为 CRLF"
        );
        assert!(app.tab().dirty, "行尾转换是真实文档编辑");
        assert_eq!(app.tab().version, v0 + 1, "版本应推进（自动保存触发依据）");
        assert_eq!(
            app.cur_handle.borrow().doc.line_ending(),
            editpad_core::LineEnding::CrLf
        );

        // 已是目标：提示且不动文档
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::CrLf));
        assert!(app.status.contains("已是"), "实际 {:?}", app.status);
        assert_eq!(app.tab().version, v0 + 1, "无变化不得推进版本");

        // CRLF → LF：可撤销（撤销回 LF 原文）
        dispatch(&mut app, Message::ConvertEol(editpad_core::LineEnding::Lf));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "a\nb\nc\n");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "a\r\nb\r\nc\r\n",
            "行尾转换必须可撤销"
        );
    }

    #[test]
    fn p67_loaded_and_save_as_reset_encoding_preference() {
        // 重新载入同一文件：编码偏好重置为默认 UTF-8
        // （先派发 FileDropped 建新加载任务，否则 Loaded 会被过期守卫丢弃；
        // 当前页非空净 → 打开落在新页，断言按路径定位而非固定下标）
        let (mut app, path) = loaded_real_file_app("p67-pref-reset");
        app.tabs[0].save_encoding = Some(editpad_core::SaveEncoding::Gbk);

        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq,
                Ok((
                    editpad_core::Document::from_str("base"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        // start_loading「打开即聚焦」：活动页 = 刚重载完成的那一页
        assert_eq!(
            app.tab().path.as_deref(),
            Some(path.as_path()),
            "活动页应为重载的文件"
        );
        assert!(
            app.tab().save_encoding.is_none(),
            "重载必须重置编码偏好"
        );

        // 另存为新路径：同样重置（旧偏好属于旧路径）——先给活动页设偏好
        app.tab_mut().save_encoding = Some(editpad_core::SaveEncoding::Utf8Bom);
        let target = scratch_dir("p67-pref-reset2").join("new.txt");
        dispatch(&mut app, Message::SaveTargetChosen(Some(target)));
        assert!(
            app.tab().save_encoding.is_none(),
            "另存为必须重置编码偏好"
        );
    }

    // ---------- P70 正则查找/替换 ----------

    #[test]
    fn p70_regex_toggle_rescans_and_invalid_pattern_surfaces_error() {
        let mut app = loaded_txt_app();
        dispatch(&mut app, Message::FindToggled); // 打开查找栏（校验在排扫描时进行）
        dispatch(
            &mut app,
            Message::FindQueryChanged("foo".to_owned()),
        );
        dispatch(&mut app, Message::RegexToggled(true));
        assert!(app.regex_enabled);
        assert!(app.status.contains("$1"), "开启正则应提示替换语法：{:?}", app.status);

        // 无效正则：状态栏报错、不排扫描任务
        dispatch(&mut app, Message::FindQueryChanged("(unclosed".to_owned()));
        assert!(
            app.status.contains("正则无效"),
            "非法模式应立即报错：{:?}",
            app.status
        );
        assert!(app.find_scan.is_none(), "无效模式不得排队扫描");

        // 关闭开关恢复字面模式
        dispatch(&mut app, Message::RegexToggled(false));
        assert!(!app.regex_enabled);
    }

    #[test]
    fn p70_regex_replace_all_end_to_end_with_captures() {
        let (mut app, _path) = loaded_real_file_app("p70-replace-all");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str(
                "a1 b2\na3 c\nend a4",
            ));
        }
        app.regex_enabled = true;
        app.find_query = r"a(\d)".to_owned();
        app.replace_query = "x$1".to_owned();

        dispatch(&mut app, Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "x1 b2\nx3 c\nend x4",
            "正则替换应展开 $1 组引用"
        );
        assert!(app.tab().dirty);
        assert!(app.status.contains("3"), "应报告替换 3 处：{:?}", app.status);

        // 非法模式：文档原封不动
        app.find_query = "[".to_owned();
        dispatch(&mut app, Message::ReplaceAll);
        assert!(
            app.status.contains("正则无效"),
            "非法模式应报错：{:?}",
            app.status
        );
        assert!(app.tab().dirty, "文档保持上次替换后的置脏状态");
    }

    #[test]
    fn p70_regex_replace_current_expands_single_match() {
        let (mut app, _path) = loaded_real_file_app("p70-replace-current");
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str("a1 b2 a3"));
        }
        app.regex_enabled = true;
        app.find_query = r"a(\d)".to_owned();
        app.replace_query = "<$1>".to_owned();

        // 命中表直接注入（模拟后台扫描完成）：3 处命中
        let hits = vec![
            editpad_core::MatchPos { line: 0, col: 0, len_chars: 2 },
            editpad_core::MatchPos { line: 0, col: 4, len_chars: 2 },
            editpad_core::MatchPos { line: 0, col: 7, len_chars: 2 },
        ];
        let seq = app.find_seq;
        app.find_scan = Some(seq); // 登记「在途扫描」：FindScanDone 的守卫条件
        dispatch(&mut app, Message::FindScanDone(seq, hits));

        // 无当前命中：先跳到第一个（step_match 语义）
        dispatch(&mut app, Message::ReplaceCurrentRegex);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "<1> b2 a3",
            "仅当前命中被展开替换"
        );
        assert!(app.tab().dirty, "替换当前是真实文档编辑");
    }

    #[test]
    fn p63_autosave_must_skip_contract() {
        // 写前判定纯函数契约：期望戳缺失不拦截；期望已知时以差异为准
        let t0 = std::time::SystemTime::UNIX_EPOCH;
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert!(!autosave_must_skip(None, Some((t1, 5))), "从未记录无从比对");
        assert!(!autosave_must_skip(Some((t0, 5)), Some((t0, 5))), "一致放行");
        assert!(autosave_must_skip(Some((t0, 5)), Some((t1, 5))), "mtime 变化拒写");
        assert!(autosave_must_skip(Some((t0, 5)), Some((t0, 6))), "size 变化拒写");
        assert!(
            autosave_must_skip(Some((t0, 5)), None),
            "文件被外部删除同样拒写（提示条会给出明确失败提示）"
        );
    }

    // ---------- P38 撤销回基线清脏 ----------

    #[test]
    fn undo_back_to_saved_content_clears_dirty_and_redo_restores() {
        let mut app = loaded_txt_app();
        assert!(!app.tab().dirty);

        // 基线之后编辑置脏 → 自动保存成功落盘 → 当前内容成为磁盘内容
        dispatch(&mut app, Message::Edit(EditOp::InsertText("!".into())));
        assert!(app.tab().dirty);
        let v = app.tab().version;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                v,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tab().dirty);

        // 再编辑后撤销：内容恰好回到磁盘版本 → dirty 如实回清。
        // 注意先移动光标打断 P37 打字组——否则「?」并入上一组，
        // 一次撤销会直接退回基线之前（那时保持置脏才是正确行为）
        dispatch(
            &mut app,
            Message::Edit(EditOp::Motion(Motion::Right, false)),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("?".into())));
        assert!(app.tab().dirty);
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(
            !app.tab().dirty,
            "撤销回已保存内容必须回清 dirty（旧实现保持置脏）"
        );
        assert!(!app.any_dirty(), "单页应用：聚合口径同样干净");

        // 重做离开基线 → 重新置脏；再撤又回清
        dispatch(&mut app, Message::Edit(EditOp::Redo));
        assert!(app.tab().dirty, "重做到未保存状态必须重新置脏");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.tab().dirty);
    }

    #[test]
    fn untitled_page_undo_to_blank_clears_dirty() {
        let mut app = Editpad::default();
        // 未命名页的基线 = 初始空白：打字置脏，退净即安全可关
        dispatch(&mut app, Message::Edit(EditOp::InsertText("草稿".into())));
        assert!(app.any_dirty());
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert!(!app.any_dirty(), "撤销回空白的未命名页不应再拦截关窗");
        assert_eq!(app.tabs.len(), 1);

        // 部分撤销仍视为有未保存内容：两步输入只退一步
        let mut app2 = Editpad::default();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("a".into())));
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("\n".into()))); // 换行独立成组
        dispatch(&mut app2, Message::Edit(EditOp::Undo));
        assert_eq!(app2.cur_handle.borrow().doc.to_text(), "a");
        assert!(app2.any_dirty(), "\"a\" 从未落盘，必须保持置脏");
    }

    #[test]
    fn tab_key_inserts_literal_tab() {
        // P14：Tab 不再被吞——插入真实制表符（显示层展开，见 editor.rs）
        use iced::keyboard::{self, key::Named};
        let message =
            handle_key_defaults(keyboard::Key::Named(Named::Tab), keyboard::Modifiers::empty())
                .expect("Tab 应产生编辑消息");
        assert!(matches!(
            message,
            Message::Edit(EditOp::InsertText(ref t)) if t == "\t"
        ));
    }

    #[test]
    fn line_op_hotkeys_dispatch_to_edit_ops() {
        // 第 57 轮行操作：组合键 → EditOp 映射抽样（全量覆盖见注册表
        // 分发一致性测试 hotkey_actions_all_dispatch_through_handle_key）
        use iced::keyboard::{self, Modifiers};
        let msg = handle_key_defaults(keyboard::Key::Character("d".into()), Modifiers::CTRL)
            .expect("Ctrl+D 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::DuplicateLines)));
        let msg = handle_key_defaults(keyboard::Key::Character("l".into()), Modifiers::CTRL)
            .expect("Ctrl+L 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::DeleteLines)));
        let msg = handle_key_defaults(
            keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+Up 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::MoveLinesUp)));
        let msg = handle_key_defaults(
            keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+Down 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::MoveLinesDown)));
        // 第 58 轮：大小写转换与行首尾清理（默认键与主流编辑器对齐的 U 系 +
        // 助记 T/L/B；全量覆盖仍由 hotkey_actions_all_dispatch_through_handle_key）
        let msg = handle_key_defaults(
            keyboard::Key::Character("u".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+U 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::ConvertCase(CaseKind::Upper))));
        let msg = handle_key_defaults(keyboard::Key::Character("u".into()), Modifiers::CTRL)
            .expect("Ctrl+U 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::ConvertCase(CaseKind::Lower))));
        for (ch, mode, combo) in [
            ("t", TrimMode::Trailing, "Ctrl+Shift+T"),
            ("l", TrimMode::Leading, "Ctrl+Shift+L"),
            ("b", TrimMode::Both, "Ctrl+Shift+B"),
        ] {
            let msg = handle_key_defaults(
                keyboard::Key::Character(ch.into()),
                Modifiers::CTRL | Modifiers::SHIFT,
            )
            .unwrap_or_else(|| panic!("{combo} 应产生编辑消息"));
            assert!(
                matches!(msg, Message::Edit(EditOp::TrimLines(m)) if m == mode),
                "{combo} 映射到错误的清理模式"
            );
        }
        // 第 59 轮：行排序与去重（S/D=升/降序助记，K=去重；全量覆盖仍由
        // hotkey_actions_all_dispatch_through_handle_key）
        for (ch, order, combo) in [
            ("s", SortOrder::Ascending, "Ctrl+Shift+S"),
            ("d", SortOrder::Descending, "Ctrl+Shift+D"),
        ] {
            let msg = handle_key_defaults(
                keyboard::Key::Character(ch.into()),
                Modifiers::CTRL | Modifiers::SHIFT,
            )
            .unwrap_or_else(|| panic!("{combo} 应产生编辑消息"));
            assert!(
                matches!(msg, Message::Edit(EditOp::SortLines(o)) if o == order),
                "{combo} 映射到错误的排序方向"
            );
        }
        let msg = handle_key_defaults(
            keyboard::Key::Character("k".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+K 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::RemoveDuplicateLines)));
        // 第 60 轮：书签套件（F2 家族 = 主流编辑器同默认键；裸功能键经
        // 第 60 轮扩约的 combo_string 进入热键系统；全量覆盖仍由
        // hotkey_actions_all_dispatch_through_handle_key）
        use iced::keyboard::key::Named as N;
        let msg = handle_key_defaults(
            keyboard::Key::Named(N::F2),
            Modifiers::CTRL,
        )
        .expect("Ctrl+F2 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::ToggleBookmark)));
        let msg = handle_key_defaults(keyboard::Key::Named(N::F2), Modifiers::empty())
            .expect("裸 F2 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::BookmarkNext)));
        let msg = handle_key_defaults(
            keyboard::Key::Named(N::F2),
            Modifiers::SHIFT,
        )
        .expect("Shift+F2 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::BookmarkPrev)));
        let msg = handle_key_defaults(
            keyboard::Key::Named(N::F2),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+F2 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::BookmarksClearAll)));
        let msg = handle_key_defaults(
            keyboard::Key::Character("c".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+C 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::CopyBookmarkedLines)));
        let msg = handle_key_defaults(
            keyboard::Key::Character("x".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
        )
        .expect("Ctrl+Shift+X 应产生编辑消息");
        assert!(matches!(msg, Message::Edit(EditOp::RemoveBookmarkedLines)));
    }

    #[test]
    fn line_ops_flow_through_app_update_marking_dirty() {
        // 行操作经统一编辑入口：置脏 + 可撤销语义与普通编辑完全一致
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("甲\n乙\n丙丁".into())));
        assert!(app.any_dirty());
        // 当前行（第 3 行）上移
        dispatch(&mut app, Message::Edit(EditOp::MoveLinesUp));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n乙");
        // 复制当前行（第 2 行）到其下方
        dispatch(&mut app, Message::Edit(EditOp::DuplicateLines));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n丙丁\n乙");
        // 删除当前行（副本）
        dispatch(&mut app, Message::Edit(EditOp::DeleteLines));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n乙");
        // 行删除可经 Ctrl+Z 撤销
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "甲\n丙丁\n丙丁\n乙");
    }

    #[test]
    fn case_convert_and_trim_flow_through_app_update_marking_dirty() {
        // 第 58 轮：大小写/清理经统一编辑入口——置脏、可撤销与普通编辑一致
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("  hello world  ".into())));
        dispatch(&mut app, Message::Edit(EditOp::ConvertCase(CaseKind::Upper)));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "  HELLO WORLD  ",
            "无选区=整个文档转换"
        );
        assert!(app.any_dirty());
        dispatch(&mut app, Message::Edit(EditOp::TrimLines(TrimMode::Both)));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "HELLO WORLD");
        // 每步都可撤销，且回退精确
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "  HELLO WORLD  ");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "  hello world  ");
    }

    #[test]
    fn sort_and_dedupe_flow_through_app_update_marking_dirty() {
        // 第 59 轮：排序/去重经统一编辑入口——置脏、可撤销与普通编辑一致
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("梨\n苹果\nApple\n苹果".into())));
        assert!(app.any_dirty());
        // 码点序：'A'(0x41) < 梨(U+68A8) < 苹(U+82F9)
        dispatch(&mut app, Message::Edit(EditOp::SortLines(SortOrder::Ascending)));
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "Apple\n梨\n苹果\n苹果",
            "无选区=全文档排序"
        );
        dispatch(&mut app, Message::Edit(EditOp::RemoveDuplicateLines));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "Apple\n梨\n苹果");
        assert!(app.any_dirty(), "去重改动必须保持置脏");
        // 逐步撤销，回退精确
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "Apple\n梨\n苹果\n苹果");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "梨\n苹果\nApple\n苹果");
    }

    #[test]
    fn bookmark_ops_flow_through_app_update_without_dirtying() {
        // 第 60 轮：书签开关/跳转/清除是纯状态操作——不置脏、不排自动保存；
        // 删除标记行是真编辑——置脏 + 可撤销（文本与书签一并找回）
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("甲\n乙\n丙".into())));
        let dirty_after_typing = app.any_dirty();
        assert!(dirty_after_typing);

        // 开关当前行（第 3 行）：不置脏
        dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark));
        assert_eq!(app.cur_handle.borrow().bookmarked_lines(), vec![2]);
        // 唯一书签就是当前行：跳转原地不动（主流口径）
        dispatch(&mut app, Message::Edit(EditOp::BookmarkPrev));
        assert_eq!(app.cur_handle.borrow().cursor.line, 2);
        // 移到第 1 行再标记，从第 3 行向上跳 → 落第 1 行
        dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::DocStart, false)));
        dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行0
        dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::Down, false)));
        dispatch(&mut app, Message::Edit(EditOp::BookmarkPrev));
        assert_eq!(app.cur_handle.borrow().cursor.line, 0);
        assert_eq!(app.any_dirty(), dirty_after_typing, "书签操作不得改变置脏态");
        // 清除全部：同样不置脏
        dispatch(&mut app, Message::Edit(EditOp::BookmarksClearAll));
        assert!(app.cur_handle.borrow().bookmarked_lines().is_empty());

        // 标记首尾两行后删除标记行：置脏 + 撤销找回
        dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::DocStart, false)));
        dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行0
        dispatch(&mut app, Message::Edit(EditOp::Motion(Motion::DocEnd, false)));
        dispatch(&mut app, Message::Edit(EditOp::ToggleBookmark)); // 行2
        dispatch(&mut app, Message::Edit(EditOp::RemoveBookmarkedLines));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "乙\n", "只删标记行，中间行连同其行尾保留");
        assert!(app.any_dirty(), "删除标记行是真编辑");
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        {
            let ed = app.cur_handle.borrow();
            assert_eq!(ed.doc.to_text(), "甲\n乙\n丙");
            assert_eq!(ed.bookmarked_lines(), vec![0, 2], "撤销一并找回书签");
        }
    }

    #[test]
    fn combo_string_contract_includes_bare_function_keys() {
        // 第 60 轮扩约：无 Ctrl 仅放行 F1~F12（至多带 Shift）；Alt 恒拒；
        // 裸字符键绝不参与热键（打字正文保护）
        use iced::keyboard::{self, key::Named, Modifiers};
        let f2 = || keyboard::Key::Named(Named::F2);
        assert_eq!(
            combo_string(Modifiers::empty(), &f2()),
            Some("F2".to_owned())
        );
        assert_eq!(
            combo_string(Modifiers::SHIFT, &f2()),
            Some("Shift+F2".to_owned())
        );
        assert_eq!(
            combo_string(Modifiers::CTRL, &f2()),
            Some("Ctrl+F2".to_owned())
        );
        assert_eq!(
            combo_string(Modifiers::CTRL | Modifiers::SHIFT, &f2()),
            Some("Ctrl+Shift+F2".to_owned())
        );
        // Alt 恒拒（AltGr 保护）
        assert_eq!(combo_string(Modifiers::ALT, &f2()), None);
        assert_eq!(combo_string(Modifiers::CTRL | Modifiers::ALT, &f2()), None);
        // 无 Ctrl 的非功能键一律拒绝
        let home = keyboard::Key::Named(Named::Home);
        assert_eq!(combo_string(Modifiers::empty(), &home), None);
        let letter = keyboard::Key::Character("a".into());
        assert_eq!(combo_string(Modifiers::empty(), &letter), None);
        assert_eq!(combo_string(Modifiers::SHIFT, &letter), None);
    }

    // ---------- P6 编码知情权 ----------

    #[test]
    fn transcode_notice_covers_all_encoding_labels() {
        use editpad_core::SaveEncoding;
        // P67 口径：提示按「原标签 vs 实际目标」判定
        // 纯 UTF-8 → UTF-8 / 未打开：无需提示
        assert_eq!(transcode_notice("UTF-8", "UTF-8", false), None);
        assert_eq!(transcode_notice("", "UTF-8", false), None);

        // 用户显式选择 GBK 且原文件就是 GBK：不提示（非意外转码）
        assert_eq!(
            transcode_notice("GBK", SaveEncoding::Gbk.label(), false),
            None
        );

        // 不可映射字符优先告警
        let m = transcode_notice("UTF-8", "GBK", true).expect("应有告警");
        assert!(m.contains("&#"), "应说明数值实体写入：{m}");

        // BOM 丢失要提示
        let bom = transcode_notice("UTF-8(BOM)", "UTF-8", false).expect("BOM 丢失应有提示");
        assert!(bom.contains("BOM"));

        // 转码要提示且带出原编码名与目标
        for label in ["GBK", "UTF-16LE", "UTF-16BE"] {
            let notice =
                transcode_notice(label, "UTF-8", false).expect("转码应有提示");
            assert!(notice.contains(label), "提示需含原编码 {label}: {notice}");
            assert!(notice.contains("UTF-8"));
        }

        // 反向：UTF-8 → GBK 同样提示
        let back = transcode_notice("UTF-8", SaveEncoding::Gbk.label(), false)
            .expect("反向转码应有提示");
        assert!(back.contains("GBK") && back.contains("UTF-8"));
    }

    #[test]
    fn load_stream_happy_path_emits_progress_then_done() {
        let dir = scratch_dir("happy");
        let path = dir.join("note.txt");
        // 超过单块（64KB）以触发多次进度回调
        let content = "Editpad 加载流测试\n".repeat(8_000);
        std::fs::write(&path, &content).unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                7,
                path.clone(),
                |p, cb| editpad_core::load_document_streaming(p, cb),
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let progress_count = messages
            .iter()
            .filter(|m| matches!(m, Message::LoadProgress(7, _, _)))
            .count();
        assert!(progress_count >= 2, "应有多条进度消息，实际 {progress_count}");

        let dones: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(id, result) => Some((*id, result)),
                _ => None,
            })
            .collect();
        assert_eq!(dones.len(), 1, "恰好一条 Loaded");
        let (id, Ok((doc, _sample, encoding))) = dones[0] else {
            panic!("应为成功加载，实际 {:?}", dones[0]);
        };
        assert_eq!(id, 7);
        // P19：消息携带的是 rope 文档本体
        assert_eq!(doc.to_text(), content);
        assert_eq!(encoding.as_str(), "UTF-8");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_stream_reports_failure_when_loader_panics() {
        // P5 回归：加载线程崩溃也必须让 UI 收到 Loaded(Err)，busy 才能解除
        let dir = scratch_dir("panic");
        let path = dir.join("boom.txt");
        std::fs::write(&path, b"data").unwrap();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_load(
                9,
                path.clone(),
                |_p, _cb| -> Result<editpad_core::LoadedDocument, editpad_core::CoreError> {
                    panic!("模拟加载线程崩溃");
                },
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        let failures: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Loaded(_, Err(text)) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1, "panic 后应恰好一条失败 Loaded，实际 {messages:?}");
        assert!(failures[0].contains("加载线程崩溃"), "错误需含兜底前缀: {}", failures[0]);
        assert!(failures[0].contains("模拟加载线程崩溃"), "错误需含 panic 信息: {}", failures[0]);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P10 后台查找扫描 ----------

    #[test]
    fn find_scan_stream_emits_exactly_one_done_with_matches() {
        let payload = FindScanPayload {
            seq: 42,
            doc: editpad_core::Document::from_str("foo\nbar foo\n"),
            query: "foo".to_owned(),
            case_sensitive: true,
            regex: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        // 恰好一条完成消息（函数直接返回它）
        let message = block_on(drive_find_scan(payload, |doc, q, cs, _rx| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 42);
                assert_eq!(
                    hits,
                    &vec![
                        editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 },
                        editpad_core::MatchPos { line: 1, col: 4, len_chars: 3 },
                    ]
                );
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_cancelled_task_skips_scanning_and_replies_empty() {
        // 新输入排队时会把上一代的取消标志置位；被作废的任务醒来即退出，
        // 不浪费一次全文扫描，但仍回一条空结果消息保持「恰好一条」语义
        let flag = Arc::new(AtomicBool::new(false));
        flag.store(true, Ordering::Relaxed);
        let payload = FindScanPayload {
            seq: 3,
            doc: editpad_core::Document::from_str("target target"),
            query: "target".to_owned(),
            case_sensitive: true,
            regex: false,
            cancelled: flag,
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(payload, |doc, q, cs, _rx| {
            editpad_core::find_all_document(doc, q, cs)
        }));
        match message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(seq, 3);
                assert!(hits.is_empty(), "被取消的任务不得产出命中");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_survives_panic_and_still_replies() {
        // 扫描函数崩溃也必须回消息（空表），否则查找栏永久停在「查找中…」
        let payload = FindScanPayload {
            seq: 7,
            doc: editpad_core::Document::new(),
            query: "x".to_owned(),
            case_sensitive: false,
            regex: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            debounce_ms: 10,
        };
        let message = block_on(drive_find_scan(
            payload,
            |_doc, _q, _cs, _rx| -> Vec<editpad_core::MatchPos> { panic!("模拟扫描崩溃") },
        ));
        match &message {
            Message::FindScanDone(seq, hits) => {
                assert_eq!(*seq, 7);
                assert!(hits.is_empty(), "panic 兜底应回空命中表");
            }
            other => panic!("应为 FindScanDone，实际 {other:?}"),
        }
    }

    #[test]
    fn find_scan_results_are_filtered_by_sequence_number() {
        let mut app = Editpad::default();
        // 两份可区分的命中表：过期投递不得覆盖已采纳/待采纳的状态
        let hit_a = || vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 1 }];
        let hit_b = || vec![editpad_core::MatchPos { line: 1, col: 4, len_chars: 1 }];
        /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
        fn dispatch(app: &mut Editpad, message: Message) {
            let _ = app.update(message);
        }

        app.find_visible = true;

        // 排队 → 采纳当前序号的结果
        dispatch(&mut app, Message::FindQueryChanged("a".into()));
        assert_eq!(app.find_scan, Some(1), "查询变化应排队一次后台扫描");
        dispatch(&mut app, Message::FindScanDone(1, hit_a()));
        assert_eq!(app.matches, hit_a());
        assert_eq!(app.find_scan, None, "结果采纳后在途标记应清除");
        assert_eq!(app.match_idx, None, "新结果后跳转游标复位");

        // 已消费的序号再回来（重复投递）不得二次生效
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a());

        // 新输入换新序号；重扫期间保留旧命中（防闪烁），但旧序号的迟到结果不许覆盖
        dispatch(&mut app, Message::FindQueryChanged("ab".into()));
        assert_eq!(app.find_scan, Some(2));
        dispatch(&mut app, Message::FindScanDone(1, hit_b()));
        assert_eq!(app.matches, hit_a(), "过期结果必须被丢弃、不得覆盖");
        dispatch(&mut app, Message::FindScanDone(2, hit_b()));
        assert_eq!(app.matches, hit_b());
        assert_eq!(app.find_scan, None);

        // 大小写切换同样触发重扫
        dispatch(&mut app, Message::CaseToggled(true));
        assert_eq!(app.find_scan, Some(3));

        // 文档编辑（查找栏开着时）触发重扫
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert_eq!(app.find_scan, Some(4));

        // 关闭查找栏 = 取消：清结果且在途结果作废
        dispatch(&mut app, Message::FindToggled);
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(4, hit_a()));
        assert!(app.matches.is_empty(), "取消后的迟到结果必须被丢弃");

        // Esc 关栏同样取消（注意：上一步关栏的取消已把序号推到 5，本次排队为 6）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("abc".into()));
        assert_eq!(app.find_scan, Some(6));
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.find_scan, None);
        dispatch(&mut app, Message::FindScanDone(6, hit_a()));
        assert!(app.matches.is_empty());

        // 清空查询 = 取消而非空表扫描（BarsDismissed 取消后序号为 7，本次排队 8）
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("q".into()));
        assert_eq!(app.find_scan, Some(8));
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert_eq!(app.matches, hit_a());
        dispatch(&mut app, Message::FindQueryChanged(String::new()));
        assert_eq!(app.find_scan, None);
        assert!(app.matches.is_empty());
        dispatch(&mut app, Message::FindScanDone(8, hit_a()));
        assert!(app.matches.is_empty(), "清空查询取消后，同号迟到结果也必须被丢弃");
    }

    #[test]
    fn find_next_while_scanning_reports_progress_not_stale_jump() {
        let mut app = Editpad::default();
        app.find_visible = true;
        app.find_query = "zzz".into();

        // 有结果在途：不基于过期命中表跳转
        app.find_scan = Some(9);
        app.matches = vec![editpad_core::MatchPos { line: 0, col: 0, len_chars: 3 }];
        let _ = app.update(Message::FindNext);
        assert_eq!(app.status, "查找中…", "扫描在途时 Enter 应提示进度");

        // 无结果且无在途扫描：懒补排一次扫描而不是误报「无匹配」
        let mut fresh = Editpad::default();
        fresh.find_visible = true;
        fresh.find_query = "zzz".into();
        let _ = fresh.update(Message::FindNext);
        assert!(fresh.find_scan.is_some(), "Enter 应懒触发一次后台扫描");
    }

    // ---------- P26 跨行查询（CRLF 文档上的选区还原与替换当前） ----------

    /// 构造一个已加载 CRLF 文档的应用，返回 (应用, 文档快照)。
    fn crlf_find_app() -> (Editpad, editpad_core::Document) {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/t/crlf.txt")));
        let seq = app.job_seq;
        let doc = editpad_core::Document::from_str("first\r\nsecond\r\nthird");
        dispatch(
            &mut app,
            Message::Loaded(seq, Ok((doc.clone(), String::new(), "UTF-8".to_owned()))),
        );
        (app, doc)
    }

    #[test]
    fn multiline_match_selects_across_lines_on_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        // 用户输入转义序列：\n 解析为真实换行 → 查询含换行、跨行匹配
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        assert!(app.find_scan.is_some(), "查询变化应排队后台扫描");

        // 用与后台线程相同的核心函数产出命中表，再按协议回填
        let query = unescape_query("st\\nse");
        let hits = editpad_core::find_all_document(&doc, &query, true);
        assert_eq!(
            hits,
            vec![editpad_core::MatchPos { line: 0, col: 3, len_chars: 5 }],
            "命中跨度应为显示口径 5（2 字符 + 1 次跨行 + 2 字符），不是原始字符数 6"
        );
        let seq = app.find_scan.unwrap();
        dispatch(&mut app, Message::FindScanDone(seq, hits));
        assert!(app.matches.len() == 1);

        // 跳到该命中：选区必须跨行且恰好覆盖「st\r\nse」（\r\n 只算一格）
        dispatch(&mut app, Message::FindNext);
        assert_eq!(app.status, "第 1/1 处匹配");
        let selected = app.cur_handle.borrow().selected_text();
        assert_eq!(
            selected.as_deref(),
            Some("st\r\nse"),
            "跨行选区应包含文档真实的 \\r\\n，而不是多走/少走一格"
        );
    }

    #[test]
    fn replace_current_replaces_multiline_match_in_crlf_document() {
        let (mut app, doc) = crlf_find_app();
        app.find_visible = true;
        dispatch(&mut app, Message::FindQueryChanged("st\\nse".into()));
        let query = unescape_query("st\\nse");
        let seq = {
            let hits = editpad_core::find_all_document(&doc, &query, true);
            let s = app.find_scan.unwrap();
            dispatch(&mut app, Message::FindScanDone(s, hits));
            s
        };
        let _ = seq;
        dispatch(&mut app, Message::FindNext);
        dispatch(&mut app, Message::ReplaceQueryChanged("-".into()));

        // 替换当前：选区文本 st\r\nse 与查询 st\nse 行尾归一后判等才可替换。
        // （P26 前的行为：字面比较判不等 → 误跳下一个、文档不动。）
        dispatch(&mut app, Message::ReplaceCurrent);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            // "second" 被吃掉开头 "se" 后剩 "cond"，不是 "ond"
            "fir-cond\r\nthird",
            "跨行命中应被整体替换为替换文本"
        );
        assert!(app.tab().dirty);

        // 可撤销：替换走 insert_str 快照链
        dispatch(&mut app, Message::Edit(EditOp::Undo));
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "first\r\nsecond\r\nthird");
    }

    // ---------- P11 全部替换（rope 流式路径） ----------

    #[test]
    fn replace_all_swaps_document_and_sets_dirty() {
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("foo bar foo\nfoo"));
        app.find_visible = true;
        app.find_query = "foo".into();
        app.replace_query = "baz".into();

        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "baz bar baz\nbaz",
            "全部替换应改写文档内容"
        );
        assert!(app.tab().dirty, "全部替换后必须置脏");
        assert_eq!(app.status, "已替换 3 处");

        // 无命中时不改文档也不置脏
        let mut app2 = Editpad::default();
        app2.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("untouched"));
        app2.find_query = "zzz".into();
        app2.replace_query = "x".into();
        let _ = app2.update(Message::ReplaceAll);
        assert_eq!(app2.cur_handle.borrow().doc.to_text(), "untouched");
        assert!(!app2.tab().dirty);
        assert_eq!(app2.status, "已替换 0 处");
    }

    #[test]
    fn replace_all_is_blocked_while_scan_in_flight() {
        // P10 守卫在 P11 新路径上仍然生效：扫描在途时的全文快照可能过期
        let mut app = Editpad::default();
        app.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("keep me"));
        app.find_query = "me".into();
        app.replace_query = "you".into();
        app.find_scan = Some(11);
        let _ = app.update(Message::ReplaceAll);
        assert_eq!(
            app.cur_handle.borrow().doc.to_text(),
            "keep me",
            "后台扫描在途时不得执行全部替换"
        );
        assert!(!app.tab().dirty);
    }

    // ---------- P12 高亮后台分批补建 ----------

    /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
    fn dispatch(app: &mut Editpad, message: Message) {
        let _ = app.update(message);
    }

    /// 构造一个启用了 Rust 语法高亮的 N 行文档应用，返回 (状态, 当时代次)。
    fn app_with_rs_doc(lines: usize) -> (Editpad, u64) {
        let app = Editpad::default();
        let text = "fn f(x: f64) -> f64 { x /* 注释 */ }\n".repeat(lines);
        {
            let mut ed = app.cur_handle.borrow_mut();
            ed.reset_document(editpad_core::Document::from_str(&text));
            ed.set_language(Some("rs"));
        }
        let gen = app
            .cur_handle
            .borrow()
            .highlight_generation()
            .expect("已启用高亮");
        (app, gen)
    }

    fn pave_payload(app: &Editpad, batch: usize) -> (HlPavePayload, u64) {
        let (gen, highlighter) = app
            .cur_handle
            .borrow()
            .highlight_pave_snapshot()
            .expect("已启用高亮");
        let doc = app.cur_handle.borrow().doc.clone();
        (
            HlPavePayload {
                gen,
                doc,
                highlighter,
                total_lines: app.cur_handle.borrow().doc.line_count(),
                cancelled: Arc::new(AtomicBool::new(false)),
                batch_strides: batch,
            },
            gen,
        )
    }

    #[test]
    fn hl_pave_stream_reports_progress_then_installs_final_state() {
        let (mut app, _) = app_with_rs_doc(600);
        let (payload, gen) = pave_payload(&app, 1);

        // 调度登记：HighlightPaveNeeded 应把任务记入在途（幂等性另有专测）
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(app.hl_paving.is_some(), "应登记在途铺建任务");

        // 直接驱动一次完整流并逐条喂给 update（模拟运行时投递）
        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(payload, pave_run, &mut tx).await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        // 600 行 → 3 个完整档位，batch=1 → 3 条进度 + 恰好 1 条完成
        let progresses: Vec<u64> = messages
            .iter()
            .filter_map(|m| match m {
                Message::HlPaveProgress(g, done) => {
                    assert_eq!(*g, gen, "进度消息必须携带调度代次");
                    Some(*done)
                }
                _ => None,
            })
            .collect();
        assert_eq!(progresses, vec![1, 2, 3], "进度应为累计档位数");
        let dones: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::HlPaved(g, _) => Some(*g),
                _ => None,
            })
            .collect();
        assert_eq!(dones, vec![gen], "恰好一条完成消息且代次一致");

        // 进度消息刷新状态栏；完成消息安装成果并解除登记
        for message in messages {
            dispatch(&mut app, message);
        }
        assert!(matches!(
            app.status.as_str(),
            "语法分析中…100%（后台）" | ""
        ), "铺建完成后状态应归位，实际 {:?}", app.status);
        assert_eq!(app.hl_paving, None, "完成后必须解除在途登记");
        assert_eq!(
            app.cur_handle.borrow().highlight_checkpoints_len(),
            Some(1 + 3),
            "初始检查点 + 3 个后台档位"
        );
    }

    #[test]
    fn stale_hl_pave_result_is_dropped_after_generation_change() {
        let (mut app, gen0) = app_with_rs_doc(600);
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert_eq!(app.hl_paving, Some(gen0));

        // 编辑换代：真实编辑触发 invalidate_from → 换代
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert_ne!(
            app.cur_handle.borrow().highlight_generation(),
            Some(gen0),
            "编辑必须换代"
        );
        let len_now = app.cur_handle.borrow().highlight_checkpoints_len();

        // 迟到的旧代成果（内容无关紧要，代次闸门负责拒收）
        let (stale_payload, _) = pave_payload(&app, 32);
        let mut enriched_hl = stale_payload.highlighter.clone();
        enriched_hl.advance_checkpoints(4, 600, &mut |i| format!("let e{i} = {i};"));
        dispatch(&mut app, Message::HlPaved(gen0, enriched_hl));

        assert_eq!(app.hl_paving, None, "过期任务的登记必须解除");
        assert_eq!(
            app.cur_handle.borrow().highlight_checkpoints_len(),
            len_now,
            "换代后的迟到成果不得覆盖当前高亮器"
        );
    }

    #[test]
    fn cancelled_hl_pave_skips_work_but_still_replies_done() {
        let (app, _) = app_with_rs_doc(600);
        let (payload, _gen) = pave_payload(&app, 1);
        payload.cancelled.store(true, Ordering::Relaxed);
        let base_len = payload.highlighter.checkpoints_len();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(payload, pave_run, &mut tx).await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        assert!(
            messages.iter().all(|m| !matches!(m, Message::HlPaveProgress(..))),
            "被取消的任务不得产出任何推进进度"
        );
        let dones = messages
            .iter()
            .filter(|m| matches!(m, Message::HlPaved(..)))
            .count();
        assert_eq!(dones, 1, "取消也必须回一条完成消息保持「恰好一条」语义");

        // 兜底/取消路径送回的是起点克隆：安装它等于无变化，UI 不受损
        if let Some(Message::HlPaved(_, hl)) = messages.into_iter().next() {
            assert_eq!(hl.checkpoints_len(), base_len);
        }
    }

    #[test]
    fn hl_pave_runner_panic_still_replies_done() {
        // 推进函数崩溃也必须回完成消息（起点克隆兜底），
        // 否则「语法分析中…」永不解除——P5/P10 同款契约
        let (mut app, _) = app_with_rs_doc(600);
        let (payload, gen) = pave_payload(&app, 1);
        let base_len = payload.highlighter.checkpoints_len();

        let messages = block_on(async {
            let (mut tx, mut rx) = iced::futures::channel::mpsc::channel::<Message>(64);
            drive_hl_pave(
                payload,
                |_payload, _report| -> editpad_core::LazyHighlighter { panic!("模拟铺建崩溃") },
                &mut tx,
            )
            .await;
            drop(tx);
            let mut collected = Vec::new();
            while let Some(message) = rx.next().await {
                collected.push(message);
            }
            collected
        });

        assert_eq!(messages.len(), 1, "panic 后只应有兜底完成消息");
        match &messages[0] {
            Message::HlPaved(g, hl) => {
                assert_eq!(*g, gen);
                assert_eq!(hl.checkpoints_len(), base_len, "兜底必须是未推进的起点");
            }
            other => panic!("应为 HlPaved，实际 {other:?}"),
        }

        // 喂给 update 后登记解除、状态归零（不卡死）
        dispatch(&mut app, messages.into_iter().next().unwrap());
        assert_eq!(app.hl_paving, None);
    }

    #[test]
    fn schedule_highlight_pave_is_idempotent_until_generation_changes() {
        let (mut app, _) = app_with_rs_doc(600);

        // 首次调度 + 同代重发：不得作废旧任务（取消标志不变、代次不变）
        dispatch(&mut app, Message::HighlightPaveNeeded);
        let flag_first = app.hl_pave_cancel.clone();
        let slot_first = app.hl_paving;
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(
            Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
            "同代重复请求必须幂等跳过"
        );
        assert_eq!(app.hl_paving, slot_first);

        // 编辑换代后再发：旧任务作废（换新取消标志）、新代次重新出发
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));
        dispatch(&mut app, Message::HighlightPaveNeeded);
        assert!(
            !Arc::ptr_eq(&flag_first, &app.hl_pave_cancel),
            "换代后必须重排新任务"
        );
        assert_ne!(app.hl_paving, slot_first, "新任务应携带换代后的代次");

        // 纯文本文档（无高亮器）：请求直接清空登记不派发
        let mut plain = Editpad::default();
        plain.cur_handle
            .borrow_mut()
            .reset_document(editpad_core::Document::from_str("plain text only\n"));
        dispatch(&mut plain, Message::HighlightPaveNeeded);
        assert_eq!(plain.hl_paving, None);
    }

    // ---------- P21 内存护栏 ----------

    #[test]
    fn mem_guard_rejects_when_estimate_exceeds_cap() {
        let cap = 1_000u64;
        // 现有 100 字符 ×3 字节估算 = 300；再开 500 字节文件 → 800 ≤ 1000 允许
        assert!(mem_guard_allows(100, 500, cap));
        // 再大一点就超
        assert!(!mem_guard_allows(100, 900, cap));
        // 极端值不 panic（饱和运算）
        assert!(mem_guard_allows(usize::MAX, u64::MAX, u64::MAX));
        assert!(!mem_guard_allows(usize::MAX, u64::MAX, 0));
    }

    #[test]
    fn start_loading_enforces_mem_guard() {
        let mut app = Editpad::default();
        // 预置一个超大字符量的当前页（直接改 doc 以绕过真实大文件）
        let huge = editpad_core::Document::from_str(&"x".repeat(200_000_000));
        app.cur_handle.borrow_mut().reset_document(huge);
        // 当前页非空 → 打开会走新页，但护栏按全页合计判定
        let big_path = PathBuf::from("C:/definitely/too/big.bin");
        let task = app.start_loading(big_path.clone(), app.tabs.len());
        let _ = task;
        assert!(
            app.status.contains("内存保护"),
            "超限打开应被拒绝并提示，实际 {:?}",
            app.status
        );
        assert!(app.active_load.is_none(), "被拒的打开不得登记任务");
        assert_eq!(app.tabs.len(), 1, "拒绝时不得占位新页");
    }

    // ---------- P31 周期快照心跳 ----------

    /// 构造已加载命名文档、注入快照目录的心跳测试应用。
    fn heartbeat_app(dir: &Path) -> Editpad {
        let mut app = loaded_txt_app();
        app.snapshot_dir_override = Some(dir.to_path_buf());
        app
    }

    #[test]
    fn heartbeat_writes_changed_dirty_page_and_marks_intermediate_manifest() {
        let dir = scratch_dir("hb-e2e");
        let mut app = heartbeat_app(&dir);

        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("有变化的置脏页应触发一次提交");
        assert!(outcome.result.is_ok(), "提交应成功，实际 {:?}", outcome.result);

        // 磁盘形态：clean_exit=false 的中间态清单 + 内容文件（未命名页
        // 与命名页同规则收录——P18 的盲区由此获得兜底）
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!manifest.clean_exit, "心跳写的必须是中间态清单");
        assert!(manifest.tabs[0].dirty);
        let doc = editpad_core::snapshot::read_page(&dir, &manifest.tabs[0]).unwrap();
        assert_eq!(doc.to_text(), "xbase", "快照必须反映当前内容（光标在行首插入）");

        // 账目回填：版本一致 → 记录 (版本, 文件名)
        assert_eq!(
            app.tabs[0].heartbeat_snap,
            Some((app.tabs[0].version, manifest.tabs[0].file.clone().unwrap()))
        );
        assert!(!app.heartbeat_inflight, "回报落地后必须解除在途标记");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_commit_entirely_when_nothing_changed() {
        let dir = scratch_dir("hb-dedupe");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));

        let first = app.run_heartbeat_cycle(&dir).expect("首次应有提交");
        let manifest_before = editpad_core::snapshot::read_manifest(&dir).unwrap();

        // 无新编辑：代次去重命中 → 零 IO，磁盘原封不动
        let second = app.run_heartbeat_cycle(&dir);
        assert!(second.is_none(), "无变化且清单不过期时不得产生任何写盘");
        assert_eq!(
            editpad_core::snapshot::read_manifest(&dir),
            Some(manifest_before),
            "清单必须保持原样（含代次号）"
        );
        assert_eq!(
            first.plan,
            vec![(0usize, app.tabs[0].version)],
            "计划只含变化页"
        );

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_refreshes_manifest_after_autosave_cleans_tab() {
        let dir = scratch_dir("hb-stale");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let _ = app.run_heartbeat_cycle(&dir).unwrap();
        let dirty_manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(dirty_manifest.tabs[0].dirty);

        // 模拟 auto-save 成功清脏：内存比已提交清单「更干净」→ 过期标记
        let version = app.tabs[0].version;
        dispatch(
            &mut app,
            Message::TabAutosaved(
                0,
                version,
                PathBuf::from("C:/doc/note.txt"),
                AutosaveOutcome::Written,
            ),
        );
        assert!(!app.tab().dirty);
        assert!(app.session_manifest_stale, "清脏后下一拍必须重写清单");

        // 下一拍即使无页被选中也要提交：清单转为「干净页纯元数据」，
        // 否则崩溃恢复会把已落盘内容按旧快照复活成置脏页
        app.run_heartbeat_cycle(&dir).expect("过期清单应触发重写");
        let refreshed = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(!refreshed.tabs[0].dirty && refreshed.tabs[0].file.is_none());
        assert!(!app.session_manifest_stale, "成功提交后过期标记归零");

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_skips_oversized_page_content_but_still_records_it_dirty() {
        let dir = scratch_dir("hb-oversize");
        let mut app = heartbeat_app(&dir);
        // 17MB 文档 > HEARTBEAT_MAX_PAGE_BYTES(16MB)：不进心跳重写，
        // 只在退出时随全量提交落盘（§3 P29 第 5 条的 IO 保护）
        let big = editpad_core::Document::from_str(&"a".repeat(17_000_000));
        {
            let tab = &mut app.tabs[0];
            tab.editor.borrow_mut().reset_document(big);
            tab.dirty = true;
            tab.version += 1;
        }

        // 首拍：Loaded 留下的清单过期标记驱动一次提交——超限页被记账
        // （保持置脏）但绝不产生内容文件
        let outcome = app
            .run_heartbeat_cycle(&dir)
            .expect("过期清单 + 置脏页应触发一次记账提交");
        assert!(outcome.result.is_ok());
        assert!(
            outcome.plan.is_empty(),
            "超限页不得进入重写计划，实际 {:?}",
            outcome.plan
        );
        let manifest = editpad_core::snapshot::read_manifest(&dir).unwrap();
        assert!(manifest.tabs[0].dirty, "超限页保持置脏记账");
        assert_eq!(manifest.tabs[0].file, None, "超限页不得落内容文件");

        let snap_files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".snap"))
            .collect();
        assert!(snap_files.is_empty(), "磁盘上不得出现任何内容快照");

        // 次拍：过期标记已清、计划仍为空 → 零 IO（节流的常态形态）
        assert!(app.run_heartbeat_cycle(&dir).is_none());

        editpad_core::snapshot::clear_session(&dir);
    }

    #[test]
    fn heartbeat_failure_clears_accounts_traces_status_and_keeps_dirty() {
        let dir = scratch_dir("hb-fail");
        // 注入目录指向一个普通文件 → create_dir_all 必败 → 提交必败
        let blocker = dir.join("occupied-as-file");
        std::fs::write(&blocker, b"not a dir").unwrap();

        let mut app = heartbeat_app(&blocker);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        assert!(app.tab().dirty);

        let outcome = app
            .run_heartbeat_cycle(&blocker)
            .expect("有置脏页就应有提交尝试");
        assert!(outcome.result.is_err(), "目录不可用时应失败");

        assert!(app.tab().dirty, "失败必须保持置脏");
        assert!(app.tabs[0].heartbeat_snap.is_none(), "失败账目作废");
        assert!(!app.heartbeat_inflight, "失败也解除在途（允许下拍重试）");
        assert!(
            app.status.contains("快照心跳失败"),
            "失败必须留痕不无声吞掉，实际 {:?}",
            app.status
        );

        // 账目作废后下一拍会全量重试（此处仅验证状态复位，不再造真实目录）
        let recovery_dir = scratch_dir("hb-fail-recovery");
        let mut recovered = app.clone();
        recovered.snapshot_dir_override = Some(recovery_dir.clone());
        let retry = recovered.run_heartbeat_cycle(&recovery_dir);
        assert!(retry.is_some_and(|o| o.result.is_ok()), "复位后重试应成功");

        editpad_core::snapshot::clear_session(&dir);
        editpad_core::snapshot::clear_session(&recovery_dir);
    }

    #[test]
    fn heartbeat_result_with_stale_version_does_not_record_account() {
        // 回报落地时的版本守卫：派发后页面又编辑过 → 迟到的快照不得入账
        // （否则账目声称「版本 v 已快照」，实际盘上是更旧的内容）
        let dir = scratch_dir("hb-stale-version");
        let mut app = heartbeat_app(&dir);
        dispatch(&mut app, Message::Edit(EditOp::InsertText("x".into())));
        let planned_version = app.tabs[0].version;
        dispatch(&mut app, Message::Edit(EditOp::InsertText("y".into())));

        let outcome = HeartbeatOutcome {
            plan: vec![(0, planned_version)],
            result: Ok(editpad_core::snapshot::SessionManifest {
                generation: 1,
                tabs: vec![editpad_core::snapshot::SessionTab {
                    path: None,
                    untitled_num: None,
                    dirty: true,
                    file: Some("s1-t0.snap".to_owned()),
                    cursor_line: 0,
                    cursor_col: 0,
                    scroll_top: 0.0,
                    scroll_left: 0.0,
                }],
                active: 0,
                next_untitled: 2,
                clean_exit: false,
            }),
        };
        app.heartbeat_apply(outcome);

        assert!(
            app.tabs[0].heartbeat_snap.is_none(),
            "版本不符的迟到成果不得记账"
        );
        assert!(!app.session_manifest_stale, "成功路径仍应清过期标记");
        assert!(!app.heartbeat_inflight);

        editpad_core::snapshot::clear_session(&dir);
    }

    // ---------- P27 设置按钮 + 设置弹窗 + P62 热键系统 ----------

    /// 测试便捷：默认（无重映射）热键分发。
    fn handle_key_defaults(
        key: keyboard::Key,
        mods: keyboard::Modifiers,
    ) -> Option<Message> {
        handle_key(key, mods, &std::collections::HashMap::new())
    }

    /// 把规范组合串解析回 (修饰键, 按键)——热键表与分发一致性的驱动器。
    fn parse_combo_for_test(combo: &str) -> (keyboard::Modifiers, keyboard::Key) {
        use iced::keyboard::{self, key::Named};
        let mut mods = keyboard::Modifiers::empty();
        let mut tokens = combo.split('+').map(str::trim).rev();
        let key_name = tokens
            .next()
            .unwrap_or_else(|| panic!("空组合键 {combo:?}"));
        for tok in tokens {
            mods |= match tok {
                "Ctrl" => keyboard::Modifiers::CTRL,
                "Shift" => keyboard::Modifiers::SHIFT,
                other => panic!("未知修饰键 {other:?}"),
            };
        }
        let key = match key_name {
            "Tab" => keyboard::Key::Named(Named::Tab),
            "Home" => keyboard::Key::Named(Named::Home),
            "End" => keyboard::Key::Named(Named::End),
            // 第 57 轮行操作热键引入方向键组合
            "Up" => keyboard::Key::Named(Named::ArrowUp),
            "Down" => keyboard::Key::Named(Named::ArrowDown),
            "Left" => keyboard::Key::Named(Named::ArrowLeft),
            "Right" => keyboard::Key::Named(Named::ArrowRight),
            "PageUp" => keyboard::Key::Named(Named::PageUp),
            "PageDown" => keyboard::Key::Named(Named::PageDown),
            "Insert" => keyboard::Key::Named(Named::Insert),
            "Delete" => keyboard::Key::Named(Named::Delete),
            // 第 60 轮：书签导航引入裸/Shift 功能键组合
            name if name.len() >= 2 && name.starts_with('F') => {
                let n: u8 = name[1..]
                    .parse()
                    .unwrap_or_else(|_| panic!("测试解析器未支持的键名 {name:?}"));
                assert!((1..=12).contains(&n), "测试解析器未支持的键名 {name:?}");
                keyboard::Key::Named(match n {
                    1 => Named::F1,
                    2 => Named::F2,
                    3 => Named::F3,
                    4 => Named::F4,
                    5 => Named::F5,
                    6 => Named::F6,
                    7 => Named::F7,
                    8 => Named::F8,
                    9 => Named::F9,
                    10 => Named::F10,
                    11 => Named::F11,
                    _ => Named::F12,
                })
            }
            single if single.chars().count() == 1 => {
                keyboard::Key::Character(single.to_ascii_lowercase().into())
            }
            other => panic!("测试解析器未支持的键名 {other:?}"),
        };
        (mods, key)
    }

    #[test]
    fn hotkey_actions_are_well_formed() {
        assert!(!HOTKEY_ACTIONS.is_empty(), "热键动作注册表不得为空");
        // id 唯一且非空（持久化主键）
        let mut ids: Vec<_> = HOTKEY_ACTIONS.iter().map(|a| a.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "动作 id 存在重复");
        // 默认组合全部可归一（格式合法）且互不冲突
        let mut combos: Vec<_> = HOTKEY_ACTIONS
            .iter()
            .map(|a| editpad_core::normalize_combo(a.default_combo).expect("默认组合必须合法"))
            .collect();
        let combo_total = combos.len();
        combos.sort_unstable();
        combos.dedup();
        assert_eq!(combos.len(), combo_total, "默认组合存在冲突");
        for a in HOTKEY_ACTIONS {
            assert!(!a.desc.trim().is_empty(), "动作 {} 说明不得为空", a.id);
        }
    }

    #[test]
    fn hotkey_actions_all_dispatch_through_handle_key() {
        // 注册表的每个默认组合都必须经 handle_key（默认表）产生消息——
        // 注册表与分发器任何一侧改动漏同步，本测试当场暴露（防漂移）
        for action in HOTKEY_ACTIONS {
            let (mods, key) = parse_combo_for_test(action.default_combo);
            let dispatched = handle_key_defaults(key, mods);
            assert!(
                dispatched.is_some(),
                "动作 {}（{}）在 handle_key 中无对应分支",
                action.id,
                action.default_combo
            );
        }
    }

    #[test]
    fn handle_key_remap_overrides_and_frees_default() {
        // 把「保存」重映射到 Ctrl+Shift+S：新组合生效、旧组合让出
        let mut remap = std::collections::HashMap::new();
        remap.insert("save".to_owned(), "Ctrl+Shift+S".to_owned());

        let (mods, key) = parse_combo_for_test("Ctrl+Shift+S");
        assert!(matches!(
            handle_key(key, mods, &remap),
            Some(Message::SaveRequested)
        ));
        let (mods, key) = parse_combo_for_test("Ctrl+S");
        assert!(
            handle_key(key, mods, &remap).is_none(),
            "被挪走的默认组合必须让出（用户显式放弃）"
        );
        // 未重映射的动作不受影响
        let (mods, key) = parse_combo_for_test("Ctrl+O");
        assert!(matches!(
            handle_key(key, mods, &remap),
            Some(Message::OpenRequested)
        ));
    }

    #[test]
    fn settings_modal_open_close_and_busy_guard() {
        let mut app = Editpad::default();
        assert!(!app.settings_visible);

        // 工具栏「设置」开 ↔ 再点关
        dispatch(&mut app, Message::SettingsToggled);
        assert!(app.settings_visible);
        dispatch(&mut app, Message::SettingsToggled);
        assert!(!app.settings_visible);

        // busy（加载/保存中）禁开——update 层守卫，与工具栏按钮禁用双保险
        app.busy = true;
        dispatch(&mut app, Message::SettingsToggled);
        assert!(!app.settings_visible, "busy 时不得打开设置弹窗");
        app.busy = false;

        // Esc 统一收口所有面板：设置弹窗一并关闭
        dispatch(&mut app, Message::SettingsToggled);
        assert!(app.settings_visible);
        dispatch(&mut app, Message::BarsDismissed);
        assert!(!app.settings_visible, "Esc 必须关闭设置弹窗");
    }

    #[test]
    fn settings_modal_actions_write_back_and_persist_to_injected_path() {
        use editpad_core::settings::{
            EXIT_MODE_ASK, EXIT_MODE_SNAPSHOT, MAX_AUTOSAVE_DELAY_SECS,
            MAX_SNAPSHOT_INTERVAL_SECS, MIN_AUTOSAVE_DELAY_SECS, MIN_SNAPSHOT_INTERVAL_SECS,
        };
        let dir = scratch_dir("p27-settings");
        let config = dir.join("config.toml");
        let mut app = Editpad::default();
        // 注入配置路径：弹窗的每次改动都落到这里，绝不碰真实 %APPDATA%
        app.settings_path_override = Some(config.clone());

        // 即时保存开关写回
        dispatch(&mut app, Message::SettingsAutosaveToggled(false));
        assert!(!app.settings.autosave_enabled);

        // 防抖秒数 ±1 步进，clamp [1,60]
        dispatch(&mut app, Message::SettingsAutosaveDelayDelta(-1));
        assert_eq!(app.settings.autosave_delay_secs, 1, "默认 2s 减 1 → 1s");
        dispatch(&mut app, Message::SettingsAutosaveDelayDelta(-10));
        assert_eq!(app.settings.autosave_delay_secs, MIN_AUTOSAVE_DELAY_SECS);
        dispatch(&mut app, Message::SettingsAutosaveDelayDelta(500));
        assert_eq!(app.settings.autosave_delay_secs, MAX_AUTOSAVE_DELAY_SECS);

        // P20 口径随开关收编：关闭「记住最近文件」→ 存量列表一并清空
        app.settings.recent_files.push("C:/old.txt".to_owned());
        dispatch(&mut app, Message::SettingsRememberRecentToggled(false));
        assert!(
            app.settings.recent_files.is_empty(),
            "只关开关不清数据等于没关"
        );

        // 快照与会话两个开关
        dispatch(&mut app, Message::SettingsSnapshotsToggled(false));
        assert!(!app.settings.enable_snapshots);
        dispatch(&mut app, Message::SettingsRememberSessionToggled(false));
        assert!(!app.settings.remember_session);

        // 关窗行为快照直退 ↔ 每次询问往返切换
        dispatch(&mut app, Message::SettingsExitModeToggled);
        assert_eq!(app.settings.exit_mode, EXIT_MODE_ASK);
        dispatch(&mut app, Message::SettingsExitModeToggled);
        assert_eq!(app.settings.exit_mode, EXIT_MODE_SNAPSHOT);

        // 心跳间隔 ±5 步进，clamp [5,120]：越界方向被夹在边界上
        dispatch(&mut app, Message::SettingsIntervalDelta(-50));
        assert_eq!(
            app.settings.snapshot_interval_secs,
            MIN_SNAPSHOT_INTERVAL_SECS
        );
        dispatch(&mut app, Message::SettingsIntervalDelta(500));
        assert_eq!(
            app.settings.snapshot_interval_secs,
            MAX_SNAPSHOT_INTERVAL_SECS
        );

        // 汇总断言：以上每次改动都已即时持久化到注入路径
        assert_eq!(
            editpad_core::Settings::load_from(&config),
            app.settings,
            "弹窗改动必须即时写回 config.toml"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P47 设置弹窗分类导航（侧栏分类风格） ----------

    #[test]
    fn settings_page_nav_selects_and_clears_search() {
        let mut app = app_with_tabs(2);
        // P51 起点分类即落盘：注入配置路径，测试绝不碰真实 %APPDATA%
        let dir = scratch_dir("p47-nav");
        app.settings_path_override = Some(dir.join("config.toml"));
        dispatch(&mut app, Message::ViewportResized(1024.0, 768.0));
        assert_eq!(app.settings_page, SettingsPage::default(), "默认落在第一分类");

        // 打开弹窗 → 每个分类都可选中，且选中后视图树可构造
        dispatch(&mut app, Message::SettingsToggled);
        for page in SettingsPage::ALL {
            dispatch(&mut app, Message::SettingsPageSelected(page));
            assert_eq!(app.settings_page, page);
            let _ = app.view();
        }

        // 搜索态：输入词入账、视图可构造（跨分类过滤分支）
        dispatch(&mut app, Message::SettingsSearchChanged("快照".to_owned()));
        assert_eq!(app.settings_search, "快照");
        let _ = app.view();

        // 点导航 = 离开搜索态（同款语义：搜索词被清空）
        dispatch(&mut app, Message::SettingsPageSelected(SettingsPage::Save));
        assert!(app.settings_search.is_empty(), "点分类必须清空搜索词");
        assert_eq!(app.settings_page, SettingsPage::Save);

        // 关闭弹窗：搜索词清空、分类位置保留（重开回到上次分类）
        dispatch(&mut app, Message::SettingsToggled);
        assert!(app.settings_search.is_empty(), "关弹窗必须清空搜索词");
        dispatch(&mut app, Message::SettingsToggled);
        assert_eq!(app.settings_page, SettingsPage::Save, "分类位置跨开合保留");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P51 分类页记忆持久化 ----------

    #[test]
    fn settings_nav_keys_match_core_registry() {
        // app 侧键映射与 core 注册表逐一对上（防两套字符串漂移）：
        // 每个分类的 key 必须经 core 归一后原样返回（= 注册键合法）
        for page in SettingsPage::ALL {
            assert_eq!(
                editpad_core::normalize_settings_page(page.key()),
                page.key(),
                "分类 {} 的持久化键不在 core 注册表中",
                page.title()
            );
        }
        // 顺序一致：ALL 的展示顺序 = 注册表顺序
        for (page, key) in SettingsPage::ALL.iter().zip(editpad_core::SETTINGS_PAGES) {
            assert_eq!(page.key(), key);
        }
        // 未知键经 core 归一 → 默认页
        assert_eq!(SettingsPage::from_key("hacked"), SettingsPage::Appearance);
        assert_eq!(SettingsPage::from_key("hotkeys"), SettingsPage::Hotkeys);
    }

    #[test]
    fn settings_page_selection_persists_and_restores() {
        let dir = scratch_dir("p51-restore");
        let config = dir.join("config.toml");

        // 选中分类 → 即时落盘
        let mut app = Editpad::default();
        app.settings_path_override = Some(config.clone());
        dispatch(&mut app, Message::SettingsPageSelected(SettingsPage::Hotkeys));
        assert_eq!(app.settings.settings_page, "hotkeys");
        assert_eq!(
            editpad_core::Settings::load_from(&config).settings_page,
            "hotkeys",
            "分类选择必须即时写回注入路径的 config.toml"
        );

        // 重启恢复：boot 同款逻辑（from_key 读配置键）
        let restored = editpad_core::Settings::load_from(&config);
        assert_eq!(
            SettingsPage::from_key(&restored.settings_page),
            SettingsPage::Hotkeys,
            "重启后必须回到上次浏览的分类页"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn settings_search_hit_rules() {
        // 空词 / 纯空白 = 不过滤（全部命中）
        assert!(settings_search_hit("", "任意", "任意"));
        assert!(settings_search_hit("   ", "任意", "任意"));

        // 大小写不敏感：desc 里的 JSON 命中 "json" 查询（热键行描述）
        let (_, json_desc) = HOTKEY_ACTIONS
            .iter()
            .find(|a| a.desc.contains("JSON"))
            .map(|a| (a.id, a.desc))
            .expect("热键动作必有 JSON 格式化条目");
        assert!(settings_search_hit("json", "Ctrl+Shift+F", json_desc));

        // 中文子串：标题命中（会话快照）与描述命中（关窗行为含「快照直退」）
        assert!(settings_search_hit("快照", "会话快照", "关窗时自动保存未存内容"));
        assert!(settings_search_hit("快照", "关窗行为", "「快照直退」不打断"));

        // 未命中
        assert!(!settings_search_hit("打印机", "主题", "切换深色 / 浅色主题"));
    }

    #[test]
    fn settings_row_catalog_covers_pages_and_controls() {
        let rows: Vec<SettingsRow> = settings_rows().collect();

        // 行元数据完整：键/标题/描述非空，键全清单唯一（控件匹配的依据）
        let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "设置行键存在重复：{keys:?}");
        for r in &rows {
            assert!(!r.key.trim().is_empty() && !r.title.trim().is_empty());
            assert!(!r.desc.trim().is_empty(), "行 {} 描述不得为空", r.key);
        }

        // 静态目录覆盖非热键分类页（P62 起热键页为动态行，由 rows_for 构建）
        for page in SettingsPage::ALL {
            if page == SettingsPage::Hotkeys {
                continue;
            }
            assert!(
                rows.iter().any(|r| r.page == page),
                "分类 {} 在行清单中没有条目",
                page.title()
            );
        }
        assert!(
            !rows.iter().any(|r| r.page == SettingsPage::Hotkeys),
            "热键行已改为动态构建，静态目录不得再包含"
        );

        // 动态热键行：每个注册动作一行，标题 = 说明、描述 = 生效组合
        let app = Editpad::default();
        let hotkey_rows = app.rows_for(SettingsPage::Hotkeys);
        assert_eq!(hotkey_rows.len(), HOTKEY_ACTIONS.len());
        for (action, r) in HOTKEY_ACTIONS.iter().zip(&hotkey_rows) {
            assert_eq!(r.key, action.id, "热键行键必须与动作 id 一致");
            assert_eq!(r.title, action.desc);
            assert_eq!(r.desc, action.default_combo, "未重映射时描述 = 默认组合");
        }

        // 控件覆盖：功能行必有控件；热键行有「修改」控件、关于为纯展示行
        for r in &rows {
            let control = app.settings_row_control(&r.key);
            match r.page {
                SettingsPage::About => {
                    assert!(control.is_none(), "展示行 {} 不应有控件", r.key);
                }
                _ => {
                    assert!(control.is_some(), "设置行 {} 缺右侧控件", r.key);
                }
            }
        }
        for r in &hotkey_rows {
            assert!(
                app.settings_row_control(&r.key).is_some(),
                "热键行 {} 缺「修改」控件",
                r.key
            );
        }

        // 字体挑选块挂载点：FONT_ROW_KEY 行存在于字体页（键与标题同源）
        assert!(rows.iter().any(|r| {
            r.page == SettingsPage::Font && r.key == FONT_ROW_KEY && r.title == FONT_ROW_KEY
        }));
    }

    #[test]
    fn hotkey_capture_flow_persists_rejects_conflict_and_resets() {
        let dir = scratch_dir("p62-capture");
        let config = dir.join("config.toml");
        let mut app = Editpad::default();
        app.settings_path_override = Some(config.clone());

        // 进入捕获 → 提交无冲突组合 → 写映射 + 持久化 + 退出捕获
        // （第 59 轮注：Ctrl+Shift+S/D/K 已成为排序/去重默认键，本测试
        //   改用仍无主的 Ctrl+Shift+E 演练重映射流程）
        dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
        assert_eq!(app.hotkey_capture, Some("save"));
        dispatch(&mut app, Message::HotkeyCaptureKey("Ctrl+Shift+E".into()));
        assert!(app.hotkey_capture.is_none());
        assert_eq!(
            app.settings.hotkeys.get("save").map(String::as_str),
            Some("Ctrl+Shift+E")
        );
        assert_eq!(
            editpad_core::Settings::load_from(&config)
                .hotkeys
                .get("save")
                .map(String::as_str),
            Some("Ctrl+Shift+E"),
            "重映射必须即时落盘"
        );

        // 重映射后分发走新组合（save → Ctrl+Shift+E；分发读活重映射表）
        let (mods, key) = parse_combo_for_test("Ctrl+Shift+E");
        assert!(matches!(
            handle_key(key, mods, &app.settings.hotkeys),
            Some(Message::SaveRequested)
        ));
        let (mods, key) = parse_combo_for_test("Ctrl+S");
        assert!(
            handle_key(key, mods, &app.settings.hotkeys).is_none(),
            "旧组合已让出"
        );

        // 冲突：把「打开」绑到已被 save 占用的组合 → 拒绝并保持捕获态
        dispatch(&mut app, Message::HotkeyCaptureStarted("open"));
        dispatch(&mut app, Message::HotkeyCaptureKey("Ctrl+Shift+E".into()));
        assert_eq!(app.hotkey_capture, Some("open"), "冲突保持捕获态");
        assert!(app.status.contains("占用"));
        assert!(
            app.settings.hotkeys.get("open").is_none(),
            "被拒动作不得写入映射"
        );
        dispatch(&mut app, Message::HotkeyCaptureCancel);
        assert!(app.hotkey_capture.is_none());

        // 全部恢复默认：清空映射 + 持久化（旧组合回归默认、新组合失效）
        dispatch(&mut app, Message::HotkeysResetAll);
        assert!(app.settings.hotkeys.is_empty());
        assert!(
            editpad_core::Settings::load_from(&config)
                .hotkeys
                .is_empty()
        );
        let (mods, key) = parse_combo_for_test("Ctrl+S");
        assert!(matches!(
            handle_key_defaults(key, mods),
            Some(Message::SaveRequested)
        ), "恢复默认后 Ctrl+S 回归保存");
        let (mods, key) = parse_combo_for_test("Ctrl+Shift+E");
        assert!(
            handle_key_defaults(key, mods).is_none(),
            "恢复默认后 Ctrl+Shift+E 不再触发保存"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hotkey_capture_rejects_non_hotkey_keys_and_esc_cancels() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::HotkeyCaptureStarted("save"));

        // 无 Ctrl 的按键不可作热键：忽略（保持捕获态）
        dispatch(
            &mut app,
            Message::KeyPressed(
                keyboard::Key::Character("a".into()),
                keyboard::Modifiers::empty(),
            ),
        );
        assert_eq!(app.hotkey_capture, Some("save"));

        // Esc 取消
        dispatch(
            &mut app,
            Message::KeyPressed(
                keyboard::Key::Named(keyboard::key::Named::Escape),
                keyboard::Modifiers::empty(),
            ),
        );
        assert!(app.hotkey_capture.is_none());

        // 非法组合串（core 归一兜底）：捕获态提交非法串 → 不写入
        dispatch(&mut app, Message::HotkeyCaptureStarted("save"));
        dispatch(&mut app, Message::HotkeyCaptureKey("alt+f4".into()));
        assert!(
            !app.settings.hotkeys.contains_key("save"),
            "非法组合不得写入映射"
        );
        dispatch(&mut app, Message::HotkeyCaptureCancel);
    }

    #[test]
    fn font_size_delta_clamps_and_persists_to_injected_path() {
        // P48：工具栏 A-/A+ 移除后，FontSizeDelta 的入口 = 设置面板步进
        // 与 Ctrl+滚轮（共用 FONT_ZOOM_STEP）——clamp 与落盘契约不变
        let dir = scratch_dir("p48-zoom");
        let config = dir.join("config.toml");
        let mut app = Editpad::default();
        app.settings_path_override = Some(config.clone());

        dispatch(&mut app, Message::FontSizeDelta(editor::FONT_ZOOM_STEP));
        assert_eq!(app.settings.font_size, 18.0);
        assert_eq!(
            app.cur_handle.borrow().font_size(),
            18.0,
            "当前页必须立即生效（P36 口径：只调正文）"
        );

        // 越界方向被夹在边界上（Ctrl+滚轮连滚不越界）
        for _ in 0..80 {
            dispatch(&mut app, Message::FontSizeDelta(editor::FONT_ZOOM_STEP));
        }
        assert_eq!(app.settings.font_size, editpad_core::settings::MAX_FONT_SIZE);
        for _ in 0..300 {
            dispatch(&mut app, Message::FontSizeDelta(-editor::FONT_ZOOM_STEP));
        }
        assert_eq!(app.settings.font_size, editpad_core::settings::MIN_FONT_SIZE);

        assert_eq!(
            editpad_core::Settings::load_from(&config).font_size,
            app.settings.font_size,
            "缩放必须即时落盘"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P49 健壮性边界输入（app 层；core 层见 boundary_inputs.rs） ----------

    #[test]
    fn goto_line_beyond_document_and_invalid_input_are_clamped() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("a\nb\nc".into())));
        assert_eq!(app.cur_handle.borrow().doc.line_count(), 3);

        // 超大行号：夹到末行行首，不 panic
        dispatch(&mut app, Message::GotoInputChanged("999999999".into()));
        dispatch(&mut app, Message::GotoSubmit);
        assert_eq!(app.cur_handle.borrow().cursor.line, 2);
        assert_eq!(app.cur_handle.borrow().cursor.col, 0);
        assert!(!app.goto_visible, "有效跳转后收起输入条");

        // 前导/尾随空白容忍（输入框手滑）
        dispatch(&mut app, Message::GotoInputChanged("  2  ".into()));
        dispatch(&mut app, Message::GotoSubmit);
        assert_eq!(app.cur_handle.borrow().cursor.line, 1);

        // 0 / 非数字 / 负数形态：报错提示、输入条保持、光标不动
        //（先重开输入条——上面的有效跳转已把它收起）
        dispatch(&mut app, Message::GotoToggled);
        assert!(app.goto_visible);
        for bad in ["0", "abc", "-3"] {
            dispatch(&mut app, Message::GotoInputChanged(bad.to_owned()));
            dispatch(&mut app, Message::GotoSubmit);
            assert!(!app.status.is_empty(), "非法行号 {bad} 必须有提示");
            assert!(app.goto_visible, "非法行号 {bad} 不收起输入条");
        }
        assert_eq!(app.cur_handle.borrow().cursor.line, 1, "非法输入不动光标");
    }

    #[test]
    fn reopen_same_file_twice_lands_consistently() {
        let mut app = Editpad::default();
        let path = PathBuf::from("C:/same.txt");

        // 第一次打开：落入初始空净页
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("v1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].path.as_ref(), Some(&path));

        // 重复打开同一路径：当前页非空 → 新开一页承接，不覆盖、不 panic
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq2 = app.job_seq;
        assert_eq!(app.active_tab, 1, "重复打开落新页");
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("v1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        assert_eq!(app.tabs.len(), 2);
        for tab in &app.tabs {
            assert_eq!(tab.path.as_ref(), Some(&path));
            assert_eq!(tab.editor.borrow().doc.to_text(), "v1");
        }
    }

    #[test]
    fn close_window_during_active_load_degrades_safely() {
        // 「每次询问」模式避免触碰真实 %APPDATA%（既有关窗测试同款守则）
        let mut app = Editpad::default();
        app.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();

        // 加载进行中：busy + 在途任务 + 占位页干净
        dispatch(&mut app, Message::FileDropped(PathBuf::from("C:/loading.txt")));
        assert!(app.busy && app.active_load.is_some());

        // 此刻关窗：占位页干净 → 直接关窗路径，不 panic、不弹确认
        let id = iced::window::Id::unique();
        let _ = app.update(Message::CloseRequested(id));
        assert!(!app.confirm_visible, "加载中关窗（干净页）不弹确认");
        assert_eq!(app.main_window, Some(id), "关窗流必须捕获窗口 id");

        // 变体：加载中且后台有置脏页 → 确认条兜底（数据不丢）。
        // 布局 = 页0 置脏、页1 干净且活动（加载落活动页，脏页在后台）
        let mut app2 = Editpad::default();
        app2.settings.exit_mode = editpad_core::EXIT_MODE_ASK.to_owned();
        dispatch(&mut app2, Message::Edit(EditOp::InsertText("dirty".into())));
        dispatch(&mut app2, Message::NewTab);
        dispatch(&mut app2, Message::FileDropped(PathBuf::from("C:/loading2.txt")));
        assert!(app2.active_load.is_some());
        let id2 = iced::window::Id::unique();
        let _ = app2.update(Message::CloseRequested(id2));
        assert!(app2.confirm_visible, "加载中关窗（置脏页）必须弹确认");

        // 快照直退模式 + 注入目录：加载中关窗照走快照直退，不卡死
        let dir = scratch_dir("p49-close-during-load");
        let mut app3 = Editpad::default();
        dispatch(&mut app3, Message::FileDropped(PathBuf::from("C:/loading3.txt")));
        let id3 = iced::window::Id::unique();
        let _ = app3.handle_close_request(id3, Some(dir.clone()));
        assert!(!app3.confirm_visible);
        // 占位页（干净未命名）也入清单——下次启动恢复为空页，无数据丢失
        assert!(editpad_core::read_manifest(&dir).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------- P50 外部修改检测 ----------

    #[test]
    fn file_changed_externally_contract() {
        let t1 = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(100);
        let t2 = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(200);

        // 记录缺失（未落地页）= 不判定
        assert!(!file_changed_externally(None, Some((t1, 10))));
        // 完全一致 = 未修改
        assert!(!file_changed_externally(Some((t1, 10)), Some((t1, 10))));
        // mtime 变 / size 变（FAT 系 2s 粒度盲区的兜底）/ 双变 = 修改
        assert!(file_changed_externally(Some((t1, 10)), Some((t2, 10))));
        assert!(file_changed_externally(Some((t1, 10)), Some((t1, 11))));
        assert!(file_changed_externally(Some((t1, 10)), Some((t2, 11))));
        // 当前缺失 = 文件被外部删除 = 视为修改（提示用户，重载得明确报错）
        assert!(file_changed_externally(Some((t1, 10)), None));
    }

    #[test]
    fn window_focus_reloads_clean_active_tab_silently() {
        let dir = scratch_dir("p50-focus");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "v1").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("v1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert!(app.tabs[0].file_stamp.is_some(), "载入成功即记比对戳");

        // 无外部变化：聚焦不动作
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.active_load.is_none());
        assert!(app.external_change.is_none());

        // 外部修改（内容+大小都变）：聚焦 → 干净活动页静默重载
        std::fs::write(&path, "v2-changed").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.active_load.is_some(), "干净活动页应静默重载");
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("v2-changed"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "v2-changed");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn window_focus_prompts_for_dirty_tab_and_respects_decision() {
        let dir = scratch_dir("p50-dirty");
        let path = dir.join("dirty.txt");
        std::fs::write(&path, "disk").unwrap();

        let mut app = Editpad::default();
        dispatch(&mut app, Message::FileDropped(path.clone()));
        let seq1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq1,
                Ok((
                    editpad_core::Document::from_str("disk"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 用户编辑置脏；期间文件被外部改
        dispatch(&mut app, Message::Edit(EditOp::InsertText("local edit".into())));
        std::fs::write(&path, "disk changed").unwrap();

        // 聚焦：置脏页绝不静默重载，弹提示条
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        let _ = app.view(); // 提示条视图可构造

        // 忽略 → 以磁盘现状重记戳，再次聚焦不再提示
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none(), "忽略后同状态不再提示");

        // 文件再次变化 → 又提示；这次选重载 → 放弃本地编辑取磁盘内容
        std::fs::write(&path, "disk v3").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        dispatch(&mut app, Message::ConfirmExternalReload(0));
        let seq2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                seq2,
                Ok((
                    editpad_core::Document::from_str("disk v3"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.cur_handle.borrow().doc.to_text(), "disk v3");
        assert!(!app.tabs[0].dirty, "重载完成后回净");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn external_change_prompt_queues_multiple_tabs() {
        // P52 聚合：两个置脏后台页同被外部修改 → 队列按序聚合；
        // 忽略首页自动切下一页；全部忽略一次清队
        let dir = scratch_dir("p52-queue");
        let (p0, p1) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&p0, "a1").unwrap();
        std::fs::write(&p1, "b1").unwrap();

        let mut app = Editpad::default();
        // 页0：加载后置脏；页1：新开加载（保持干净，但属后台页）
        dispatch(&mut app, Message::FileDropped(p0.clone()));
        let s1 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s1,
                Ok((
                    editpad_core::Document::from_str("a1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        dispatch(&mut app, Message::Edit(EditOp::InsertText("local".into())));
        dispatch(&mut app, Message::NewTab);
        dispatch(&mut app, Message::FileDropped(p1.clone()));
        let s2 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s2,
                Ok((
                    editpad_core::Document::from_str("b1"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );
        assert_eq!(app.active_tab, 1, "第二个打开落新页并聚焦");

        // 两页同被外部修改 → 聚焦后队列 [0, 1]（活动页1干净本可静默重载，
        // 但页0置脏在先——巡检按序扫描，页0先命中进队列；页1是干净活动页
        // 走静默重载分支并提前返回，队列只收页0。此处钉住该优先级语义。）
        std::fs::write(&p0, "a2").unwrap();
        std::fs::write(&p1, "b2").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert!(
            app.active_load.is_some(),
            "干净活动页(1)优先静默重载并提前返回"
        );
        let s3 = app.job_seq;
        dispatch(
            &mut app,
            Message::Loaded(
                s3,
                Ok((
                    editpad_core::Document::from_str("b2"),
                    String::new(),
                    "UTF-8".to_owned(),
                )),
            ),
        );

        // 重载完成后再次聚焦：页1已重记戳不再命中，队列只剩页0
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(app.external_change, Some(vec![0]));
        let _ = app.view();

        // 忽略页0 → 队列清空；再改两页且都置脏 → 聚合 [0,1]
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert!(app.external_change.is_none());

        dispatch(&mut app, Message::Edit(EditOp::InsertText("more".into())));
        std::fs::write(&p0, "a3").unwrap();
        std::fs::write(&p1, "b3").unwrap();
        dispatch(&mut app, Message::WindowFocused);
        assert_eq!(
            app.external_change,
            Some(vec![0, 1]),
            "两页同变必须聚合成队列"
        );

        // 忽略首页 → 队列切到下一页（条不消失）
        dispatch(&mut app, Message::IgnoreExternalChange(0));
        assert_eq!(app.external_change, Some(vec![1]));
        let _ = app.view();

        // 全部忽略 → 清队且两页都重记戳（再次聚焦不再提示）
        dispatch(&mut app, Message::IgnoreAllExternalChanges);
        assert!(app.external_change.is_none());
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none(), "全部忽略后同状态不再提示");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn focus_check_skips_busy_and_unnamed_tabs() {
        let mut app = Editpad::default();
        // 未命名页（无路径无戳）：聚焦不动作不 panic
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none());

        // busy 中聚焦：跳过巡检（在途任务的完成回调会刷新戳）
        app.busy = true;
        dispatch(&mut app, Message::WindowFocused);
        assert!(app.external_change.is_none() && app.active_load.is_none());
        app.busy = false;
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

    // ---------- P56/P57 全局中性样式 ----------

    #[test]
    fn chrome_styles_are_neutral_and_status_aware() {
        let bg_of = |s: &button::Style| match s.background {
            Some(Background::Color(c)) => Some(c),
            _ => None,
        };
        for (label, theme) in [("浅色", Theme::Light), ("深色", Theme::Dark)] {
            // 按钮：悬停必须有视觉反馈、禁用必须降灰（不再是实心蓝底白字）
            let active = chrome_button_style(&theme, button::Status::Active);
            let hovered = chrome_button_style(&theme, button::Status::Hovered);
            let disabled = chrome_button_style(&theme, button::Status::Disabled);
            assert_ne!(
                bg_of(&active),
                bg_of(&hovered),
                "{label}: 悬停必须有视觉反馈"
            );
            assert_ne!(active.text_color, disabled.text_color, "{label}: 禁用必须降灰");
            assert_ne!(
                active.border.color, disabled.border.color,
                "{label}: 禁用描边应弱化"
            );

            // 菜单项：常态透明融入卡片、悬停淡染（不再像全选中的高亮条）
            let item = chrome_menu_item_style(&theme, button::Status::Active);
            let item_hover = chrome_menu_item_style(&theme, button::Status::Hovered);
            assert_eq!(bg_of(&item), Some(Color::TRANSPARENT), "{label}: 菜单项常态透明");
            assert_ne!(
                bg_of(&item),
                bg_of(&item_hover),
                "{label}: 菜单项悬停必须有淡染"
            );
            let item_disabled =
                chrome_menu_item_style(&theme, button::Status::Disabled);
            assert_ne!(
                item.text_color, item_disabled.text_color,
                "{label}: 菜单项禁用必须降灰"
            );

            // 页签：活动页淡底 + 描边，非活动透明无边
            let tab_active = chrome_nav_button_style(&theme, button::Status::Active, true);
            let tab_idle = chrome_nav_button_style(&theme, button::Status::Active, false);
            assert!(bg_of(&tab_active).is_some(), "{label}: 活动页签应有底色");
            assert!(bg_of(&tab_idle).is_none(), "{label}: 非活动页签应透明");
            assert_ne!(
                tab_active.border.color, tab_idle.border.color,
                "{label}: 活动页签应有描边指示"
            );
        }
    }

    #[test]
    fn settings_card_size_adapts_to_viewport() {
        // 大窗：宽封顶 720，内容高封顶 680
        assert_eq!(settings_card_size(1600.0, 1000.0), (720.0, 680.0));
        // 中窗：高度跟随 vh−96
        assert_eq!(settings_card_size(1000.0, 700.0), (720.0, 604.0));
        // 窄窗：宽度收缩到 vw−32，永不出窗
        assert_eq!(settings_card_size(500.0, 700.0), (468.0, 604.0));
        // 窗口尺寸未知：保守默认
        assert_eq!(settings_card_size(0.0, 0.0), (720.0, 560.0));
        // 极小窗：高度走保守默认，宽度仍收缩
        assert_eq!(settings_card_size(300.0, 100.0), (268.0, 560.0));

        // 不变式：任意尺寸下卡片宽 ≤ 视口宽（已知时）、内容高 ≤ vh−96
        for (vw, vh) in [(800.0, 600.0), (1024.0, 768.0), (1920.0, 1080.0), (360.0, 500.0)] {
            let (w, h) = settings_card_size(vw, vh);
            assert!(w <= vw - 32.0 + 0.5, "{vw}x{vh}: 宽 {w} 出窗");
            assert!(h <= vh - 96.0 + 0.5, "{vw}x{vh}: 高 {h} 出窗");
        }
    }

    // ---------- P34 字体选择 ----------

    #[test]
    fn effective_font_family_resolution_contract() {
        let available = vec![
            "Microsoft YaHei UI".to_owned(),
            "NSimSun".to_owned(),
            "Sarasa Mono SC".to_owned(),
        ];

        // 未配置 → 默认等宽（None）
        assert_eq!(effective_font_family(None, &available), None);

        // 精确命中 → 原样生效
        assert_eq!(
            effective_font_family(Some("NSimSun"), &available),
            Some("NSimSun".to_owned())
        );

        // 大小写/空白变体 → 宽松命中并矫正为清单规范名
        assert_eq!(
            effective_font_family(Some("nsimsun"), &available),
            Some("NSimSun".to_owned())
        );
        assert_eq!(
            effective_font_family(Some(" SarasaMono sc "), &available),
            Some("Sarasa Mono SC".to_owned())
        );

        // 未安装 → 回退默认，不臆造名字
        assert_eq!(effective_font_family(Some("不存在的字体"), &available), None);
        // 空清单（枚举失败的降级环境）→ 一律默认
        assert_eq!(effective_font_family(Some("NSimSun"), &[]), None);
    }

    #[test]
    fn resolve_startup_font_prompts_only_for_configured_but_missing() {
        let available = vec!["NSimSun".to_owned()];

        // 未配置：无提示
        assert_eq!(resolve_startup_font(None, &available), (None, false));
        // 配置且可用（含变体矫正）：生效但不提示
        assert_eq!(
            resolve_startup_font(Some("nsimsun"), &available),
            (Some("NSimSun".to_owned()), false)
        );
        // 配置了未安装的字体：回退 + 提示一次
        assert_eq!(resolve_startup_font(Some("Foo"), &available), (None, true));
        // 空清单降级环境：配置了就提示
        assert_eq!(resolve_startup_font(Some("Foo"), &[]), (None, true));
    }

    #[test]
    fn font_selection_updates_active_font_and_persists_to_injected_path() {
        let dir = scratch_dir("p34-app");
        let config = dir.join("config.toml");
        let mut app = Editpad::default();
        app.settings_path_override = Some(config.clone());
        app.available_fonts =
            vec!["Arial".to_owned(), "Sarasa Mono SC".to_owned()];

        // 未选择时 = 默认等宽（P33 钉字语义不变）
        assert_eq!(app.body_font(), editor::BODY_FONT);

        // 选择系统字体：设置与生效族名同步更新并落盘
        dispatch(&mut app, Message::SettingsFontSelected("Arial".to_owned()));
        assert_eq!(app.settings.font_family.as_deref(), Some("Arial"));
        assert_eq!(app.active_font_family.as_deref(), Some("Arial"));
        assert_eq!(
            app.body_font().family,
            iced::font::Family::Name("Arial"),
            "正文与 UI 的换装点必须切到所选族"
        );
        assert_eq!(
            editpad_core::Settings::load_from(&config)
                .font_family
                .as_deref(),
            Some("Arial"),
            "选择必须即时写回注入路径的 config.toml"
        );

        // 回退默认：设置清空、渲染回到 BODY_FONT、落盘为空
        dispatch(&mut app, Message::SettingsFontReset);
        assert_eq!(app.settings.font_family, None);
        assert_eq!(app.active_font_family, None);
        assert_eq!(app.body_font(), editor::BODY_FONT);
        assert_eq!(
            editpad_core::Settings::load_from(&config).font_family,
            None,
            "回退默认后 config.toml 不应再留字体名"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn font_selection_via_loose_match_canonicalizes_name() {
        // 手改 config 的变体名经统一解析收敛到规范名（与启动期同一条路径）
        let mut app = Editpad::default();
        app.available_fonts = vec!["Noto Sans Mono CJK SC".to_owned()];
        dispatch(
            &mut app,
            Message::SettingsFontSelected("noto sans mono cjk sc".to_owned()),
        );
        assert_eq!(
            app.active_font_family.as_deref(),
            Some("Noto Sans Mono CJK SC"),
            "生效值必须是清单里的规范族名"
        );
        assert_eq!(
            app.settings.font_family.as_deref(),
            Some("noto sans mono cjk sc"),
            "core 层只做损坏值裁剪，不负责大小写矫正"
        );
    }

    #[test]
    fn font_filter_is_pure_ui_state_and_never_touches_settings() {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::FontFilterChanged("mono".to_owned()));
        assert_eq!(app.font_filter, "mono");
        assert_eq!(app.settings.font_family, None, "过滤词不落盘不改配置");

        // Esc 关浮动栏不应清掉弹窗内输入态（重开时保留上次过滤词属可接受行为，
        // 这里钉住的是「过滤不影响设置」这一核心契约）
        dispatch(&mut app, Message::BarsDismissed);
        assert_eq!(app.settings.font_family, None);
    }
