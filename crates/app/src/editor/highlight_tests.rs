use super::*;
use super::tests::*;

    // ---------- P12 高亮后台分批补建 ----------

    #[test]
    fn needs_paving_tracks_visible_deficit() {
        let mut c = core_with(&(0..600).map(|i| format!("fn f{i}() {{}}\n")).collect::<String>());
        c.set_language(Some("rs"));
        c.set_viewport_height(400.0);
        c.clamp_scroll();
        assert!(
            !c.needs_paving(),
            "顶部可见区落在初始检查点预算内，无需后台铺建"
        );

        // 跳到文末：可见区末行 599 需 5 个档位、只有初始 1 个 → 必须请求铺建
        c.scroll_top = 100_000.0;
        c.clamp_scroll();
        assert!(c.needs_paving(), "大跳转后的可见区缺档必须被识别");

        // 纯文本路径（无高亮器）恒不需要
        let plain = core_with(&"x\n".repeat(600));
        assert!(!plain.needs_paving());
    }

    #[test]
    fn pave_snapshot_and_conditional_install_flow() {
        let mut c = core_with(&(0..600).map(|i| format!("let a{i} = {i};\n")).collect::<String>());
        c.set_language(Some("rs"));

        // 起点快照 + 后台式推进
        let (gen, snapshot) = c.highlight_pave_snapshot().expect("已启用高亮");
        assert_eq!(c.highlight_generation(), Some(gen));
        let mut worker = snapshot.clone();
        let built = worker.advance_checkpoints(32, 600, &mut |i| format!("let w{i} = {i};"));
        // P147 检查点覆盖约定：k 覆盖 [(k-1)*STRIDE, k*STRIDE)——600 行的
        // 完整档位为 [0,128)/[128,256)/[256,384)/[384,512)，共 4 个
        assert_eq!(built, 4, "600 行的完整档位起点为 0/128/256/384，共 4 个");
        assert!(worker.checkpoints_len() > snapshot.checkpoints_len());

        // 代次一致 → 安装成功且状态生效
        assert!(c.install_highlighter_if_current(gen, worker.clone()));
        assert_eq!(
            c.highlight_checkpoints_len(),
            Some(worker.checkpoints_len()),
            "安装后检查点应与成果一致"
        );

        // 代次不符（模拟安装前发生过编辑）→ 整体拒绝
        c.cursor = CursorPos { line: 599, col: 0 };
        c.insert_str("x"); // 真实编辑：内部 invalidate_from → 换代
        let stale_gen = gen;
        assert_ne!(c.highlight_generation(), Some(stale_gen), "编辑必须换代");
        assert!(
            !c.install_highlighter_if_current(stale_gen, worker),
            "换代后的迟到成果不得覆盖当前高亮器"
        );
    }

