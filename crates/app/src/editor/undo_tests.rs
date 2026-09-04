use super::*;
use super::tests::*;

    // ---------- P37 打字成组撤销 ----------

    #[test]
    fn typing_run_merges_consecutive_single_chars_into_one_undo() {
        let mut c = core_with("");
        for ch in ["a", "b", "c"] {
            c.insert_str(ch);
        }
        assert_eq!(c.doc.to_text(), "abc");
        // 三次连续单字符输入 = 一个撤销组：一步退回输入前
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");
        assert!(!c.undo(), "组内只有一份快照，一步即到输入前");
        assert!(c.redo());
        assert_eq!(c.doc.to_text(), "abc", "重做整体恢复该组");
    }

    #[test]
    fn typing_run_breaks_on_cursor_motion() {
        let mut c = core_with("");
        c.insert_str("a");
        c.insert_str("b");
        c.apply_motion(Motion::Left, false); // 光标移动打断成组
        c.insert_str("c");
        assert_eq!(c.doc.to_text(), "acb");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "ab", "移动后的输入独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "", "移动前的连续输入是另一组");
    }

    #[test]
    fn typing_run_breaks_on_newline_paste_and_selection_replace() {
        // 换行：归一后即使只有 \n 也不并入组
        let mut c = core_with("");
        c.insert_str("a");
        c.insert_str("\n");
        c.insert_str("b");
        assert_eq!(c.doc.to_text(), "a\nb");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a\n", "b 独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "a");

        // 粘贴（多字符）不参与成组；其后紧邻的单字符输入开新组
        let mut c = core_with("");
        c.insert_str("hello");
        c.insert_str("!");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "hello", "! 是独立组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "");

        // 选区替换：消费选区的插入永远开新组。
        // 状态序列 xyz → x-z → x-!z：撤一次只撤掉 !（独立成组），
        // 再撤才回到原始 xyz
        let mut c = core_with("xyz");
        c.select_span(0, 1, 1); // 选中 y —— 选区变更本身打断组
        c.replace_selection("-");
        c.insert_str("!");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "x-z", "! 独立成组");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "xyz");
    }

    #[test]
    fn typing_run_survives_scrolling_but_not_focus_loss_or_undo() {
        let mut c = core_with("seed\n");
        c.cursor = CursorPos { line: 0, col: 4 }; // 行尾起打
        c.insert_str("a");
        c.scroll_by_lines(2.0); // 滚动不动光标与文档，不打断
        c.insert_str("b");
        assert_eq!(c.doc.to_text(), "seedab\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seed\n", "滚动不打断成组：ab 一起撤销");

        // 失焦打断
        c.insert_str("c");
        c.pointer_focus(false);
        c.insert_str("d");
        assert_eq!(c.doc.to_text(), "seedcd\n");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "失焦后的输入独立成组");

        // 撤销本身打断：ef 连续输入并作一组，一步退净且不混入更早历史；
        // undo 之后的新输入是全新一组——重做链被新快照清空，再退只撤掉自己
        c.insert_str("e");
        c.insert_str("f");
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "ef 同组一起撤销");
        c.insert_str("g");
        assert_eq!(c.doc.to_text(), "seedcg\n", "undo 后的输入开新组");
        assert!(
            c.redo_stack.is_empty(),
            "undo 后的新编辑必须作废被撤销的重做链"
        );
        assert!(c.undo());
        assert_eq!(c.doc.to_text(), "seedc\n", "只撤掉 g 本身");
    }

    #[test]
    fn undo_stack_capacity_bumps_to_512_groups() {
        let mut c = core_with("");
        for i in 0..600 {
            c.insert_str(&format!("{}", i % 10));
            // 每次移动打断成组 → 每个字符各自成组，撑爆容量上限
            c.apply_motion(Motion::Left, false);
            c.apply_motion(Motion::Right, false);
        }
        assert_eq!(c.undo_stack.len(), 512, "容量上限提升到 512 组");
        for _ in 0..512 {
            assert!(c.undo());
        }
        // 容量淘汰的已知取舍：最旧的 88 组（第 1~88 次插入）已被挤出，
        // 能退到的最早状态 = 第 89 次插入前的快照（88 字符），不是初始空文档
        assert_eq!(
            c.doc.to_text(),
            "0123456789".repeat(8) + "01234567",
            "淘汰后最早退到第 89 组快照，而非空文档"
        );
        assert!(!c.undo(), "栈已耗尽");
    }

    // ---------- P38 落盘基线：dirty 如实反映与磁盘的差异 ----------

    #[test]
    fn saved_baseline_tracks_save_and_revert_cycle() {
        let mut c = core_with("");
        // 初始基线 = 空文档（Default 即登记）
        assert!(c.is_at_saved_content(), "初始空内容应在基线上");

        c.insert_str("hello");
        assert!(!c.is_at_saved_content());
        // 模拟落盘成功：当前内容成为新基线
        c.mark_saved();
        assert!(c.is_at_saved_content());

        // 基线之后继续编辑 → 偏离；撤销回基线 → 回到与磁盘一致
        c.insert_str("!");
        assert!(!c.is_at_saved_content());
        assert!(c.undo());
        assert!(c.is_at_saved_content(), "撤销回基线应判定与磁盘一致");
        assert!(c.redo());
        assert!(!c.is_at_saved_content(), "重做离开基线");
    }

    #[test]
    fn reset_document_installs_new_baseline() {
        let mut c = core_with("");
        c.insert_str("stale");
        c.mark_saved();
        // 换文档（加载/放弃重置）：基线必须跟着换，不能沿用旧文档
        c.reset_document(editpad_core::Document::from_str("fresh\r\ndoc"));
        assert!(c.is_at_saved_content());
        c.insert_str("x");
        assert!(!c.is_at_saved_content());
        assert!(c.undo());
        assert!(c.is_at_saved_content());
        // 旧基线不得复活：撤销后内容是新文档原文，不是 "stale"
        assert_eq!(c.doc.to_text(), "fresh\r\ndoc");
    }

    #[test]
    fn cleared_baseline_is_conservative_until_marked_saved_again() {
        let mut c = core_with("");
        c.insert_str("快照恢复出的草稿");
        // P30 恢复路径：快照内容 ≠ 磁盘内容，基线必须清空
        c.clear_saved_baseline();
        assert!(!c.is_at_saved_content(), "无基线时必须保守判定为偏离");

        // 清空基线后撤销/重做往返：一律保守置脏，绝不误判「已与磁盘一致」
        c.insert_str("!");
        assert!(c.undo());
        assert!(!c.is_at_saved_content());
        assert!(c.redo());
        assert!(!c.is_at_saved_content());

        // 真正落盘一次后基线重建，回清能力恢复
        c.mark_saved();
        assert!(c.is_at_saved_content());
        c.insert_str("x");
        assert!(c.undo());
        assert!(c.is_at_saved_content());
    }

    #[test]
    fn select_span_and_replace_selection() {
        let mut c = core_with("你好世界");
        c.select_span(0, 1, 2); // 选中 “好世”
        assert_eq!(c.selected_text().as_deref(), Some("好世"));
        c.replace_selection("-");
        assert_eq!(c.doc.to_text(), "你-界");
        assert_eq!(c.cursor, CursorPos { line: 0, col: 2 });
    }

    #[test]
    fn hit_test_maps_to_visible_columns() {
        let mut c = core_with("abcdef\nxy\n");
        c.set_viewport_height(200.0);
        let gutter = c.gutter_width();

        let hit = c.hit_test(gutter + 1.0, 1.0 * c.line_height());
        assert_eq!(hit, CursorPos { line: 1, col: 0 });

        // 超出该行尾的点击被夹紧到行尾
        let hit = c.hit_test(gutter + 100.0 * c.char_width(), 0.0);
        assert_eq!(hit, CursorPos { line: 0, col: 6 });
    }

    // ---------- 第 58 轮 主线 A 扩容：随机混合操作不变量 + 撤销重放对拍 ----------

    /// XorShift64（与 crates/core/tests/edit_sequence_fuzz.rs 同款零依赖 PRNG，
    /// 固定种子失败可精确复现）。
    struct XorShift64(u64);

    impl XorShift64 {
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                return 0;
            }
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            (x % n as u64) as usize
        }
    }

    /// 剥掉全部换行字符——大小写对拍口径（换行不在大小写映射表内）。
    fn strip_newlines(s: &str) -> String {
        s.chars().filter(|c| *c != '\r' && *c != '\n').collect()
    }

    /// ropey 行界口径切行，返回不含行尾的各行内容。
    fn rope_line_bodies(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\n' => out.push(std::mem::take(&mut cur)),
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(std::mem::take(&mut cur));
                }
                other => cur.push(other),
            }
        }
        out.push(cur);
        out
    }

    /// 每步必跑的结构不变量：光标/锚点在界内、滚动值有限非负、
    /// 书签行号全部落在文档行数域内（第 60 轮）。
    fn assert_structural_invariants(c: &EditorCore) {
        assert!(
            c.cursor.col <= c.line_display_len(c.cursor.line),
            "光标列越界：{:?} 行长 {}",
            c.cursor,
            c.line_display_len(c.cursor.line)
        );
        assert!(c.cursor.line < c.doc.line_count(), "光标行越界");
        if let Some(a) = c.anchor {
            assert!(a.col <= c.line_display_len(a.line), "锚点列越界");
            assert!(a.line < c.doc.line_count(), "锚点行越界");
        }
        assert!(c.scroll_top.is_finite() && c.scroll_top >= 0.0, "垂直滚动非法");
        // 第 60 轮：书签行号界内（任何再映射漂移当场暴露）
        for &l in &c.bookmarks {
            assert!(
                l < c.doc.line_count(),
                "书签行越界：{l} / 行数 {}",
                c.doc.line_count()
            );
        }
    }

    #[test]
    fn random_mixed_ops_preserve_invariants_undo_replays_exactly() {
        // 混入三种行尾、CJK、emoji 与加长映射字符（ß），覆盖新旧行为交互
        const TOKENS: &[&str] = &[
            "a", "Z", "9", "中", "文", "🚀", "ß", " ", "\t", "  ", "\n", "\r\n",
        ];
        let motions = [
            Motion::Left,
            Motion::Right,
            Motion::Up,
            Motion::Down,
            Motion::Home,
            Motion::End,
        ];
        for seed in [1u64, 0xDEAD_BEEF, 0x5EED_1234] {
            let mut rng = XorShift64(seed);
            let mut init = String::new();
            for _ in 0..rng.below(20) + 5 {
                init.push_str(TOKENS[rng.below(TOKENS.len())]);
            }
            let initial = init.clone();
            let mut c = core_with(&init);

            for step in 0..240 {
                let old_text = c.doc.to_text();
                match rng.below(100) {
                    // 插入随机 token（含 CRLF/LF/CJK/emoji/ß）
                    0..=34 => {
                        let text: String = (0..rng.below(3) + 1)
                            .map(|_| TOKENS[rng.below(TOKENS.len())])
                            .collect();
                        c.insert_str(&text);
                    }
                    35..=49 => {
                        c.backspace();
                    }
                    50..=59 => {
                        c.delete_forward();
                    }
                    60..=74 => {
                        let m = motions[rng.below(motions.len())];
                        c.apply_motion(m, rng.below(4) == 0); // 偶尔带 Shift 成选区
                    }
                    75..=79 => c.select_all(),
                    // 大小写转换：先收拢选区 → 全文档口径，oracle 才有全局闭式解
                    80..=87 => {
                        if c.anchor.is_some() {
                            c.apply_motion(Motion::Left, false);
                        }
                        let kind =
                            if rng.below(2) == 0 { CaseKind::Upper } else { CaseKind::Lower };
                        let expected = match kind {
                            CaseKind::Upper => strip_newlines(&old_text).to_uppercase(),
                            CaseKind::Lower => strip_newlines(&old_text).to_lowercase(),
                        };
                        let _changed = c.convert_case(kind);
                        assert_eq!(
                            strip_newlines(&c.doc.to_text()),
                            expected,
                            "seed={seed} step={step} 大小写对拍发散"
                        );
                        assert_eq!(
                            c.doc.line_count(),
                            rope_line_bodies(&old_text).len(),
                            "seed={seed} step={step} 大小写转换改变行数"
                        );
                    }
                    // 行首尾清理：同上收拢为全文档口径，逐行闭式对拍
                    88..=94 => {
                        if c.anchor.is_some() {
                            c.apply_motion(Motion::Left, false);
                        }
                        let mode = [TrimMode::Leading, TrimMode::Trailing, TrimMode::Both]
                            [rng.below(3)];
                        let before = rope_line_bodies(&old_text);
                        let _changed = c.trim_touched_lines(mode);
                        let after = rope_line_bodies(&c.doc.to_text());
                        assert_eq!(
                            after.len(),
                            before.len(),
                            "seed={seed} step={step} 清理改变行数"
                        );
                        for (i, (o, n)) in before.iter().zip(after.iter()).enumerate() {
                            let want = match mode {
                                TrimMode::Leading => o.trim_start(),
                                TrimMode::Trailing => o.trim_end(),
                                TrimMode::Both => o.trim(),
                            };
                            assert_eq!(n, want, "seed={seed} step={step} 第 {i} 行清理结果不符");
                        }
                    }
                    // 撤销/重做交错（空栈返回 false 合法）
                    95..=96 => {
                        let _ = c.undo();
                    }
                    // 书签套件混入（第 60 轮）：开关/跳转/清除随机三选一，
                    // 行号界内由结构不变量把关，回滚一致性由收尾对拍把关。
                    // 第 66 轮起并入括号跳转噪声臂（纯光标移动，零文本变化）。
                    97..=98 => match rng.below(4) {
                        0 => {
                            c.toggle_bookmark();
                        }
                        1 => {
                            let _ = c.next_bookmark(rng.below(2) == 0);
                        }
                        2 => {
                            let _ = c.clear_bookmarks();
                        }
                        _ => {
                            let t0 = c.doc.to_text();
                            let _ = c.jump_to_matching_bracket();
                            assert_eq!(
                                c.doc.to_text(),
                                t0,
                                "seed={seed} step={step} 括号跳转改变了文本"
                            );
                        }
                    },
                    99 => {
                        let _ = c.redo();
                    }
                    _ => {
                        let _ = c.undo();
                    }
                }
                assert_structural_invariants(&c);
            }

            // 撤销到底必须逐字节回到初始文本；重做推进到顶终态一致（可复演性）
            // 第 60 轮起书签同拍对拍：初始无书签 → 撤销到底必空 → 重放到
            // 顶与终态书签集一致
            let final_text = c.doc.to_text();
            let final_marks = c.bookmarked_lines();
            while c.undo() {}
            assert_eq!(c.doc.to_text(), initial, "seed={seed} 撤销到底未回初始态");
            assert!(
                c.bookmarked_lines().is_empty(),
                "seed={seed} 撤销到底书签未随快照清空"
            );
            while c.redo() {}
            assert_eq!(c.doc.to_text(), final_text, "seed={seed} 重放终态发散");
            assert_eq!(
                c.bookmarked_lines(),
                final_marks,
                "seed={seed} 重放终态书签发散"
            );
        }
    }

    /// 第 66 轮 主线 A 手段 2 扩容：P82/P84 行块操作（合并/拆分/删空行/
    /// 注释切换）纳入随机对拍。
    ///
    /// 文档口径 = **LF 纯净**（不含 CRLF/孤立 \r token）：主导行尾恒
    /// `\n`，四个操作的 oracle 均有闭式解；CRLF 混排下这些操作的结构
    /// 不变量已由上方主循环把关。每步先按当前光标行构造期望文本再调
    /// 用实现，逐字节对拍；收尾同样做「撤销到底回初始 / 重放终态一致」。
    #[test]
    fn random_line_block_ops_lf_docs_match_oracles() {
        const TOK: &[&str] = &["a", "Z", "中", " ", "  ", "// x", "# y"];
        for seed in [7u64, 0xBEEF_5EED, 42] {
            let mut rng = XorShift64(seed);
            let rows = rng.below(6) + 2; // 2..=7 行
            let mut init: Vec<String> = Vec::new();
            for _ in 0..rows {
                let mut s = String::new();
                for _ in 0..rng.below(4) {
                    s.push_str(TOK[rng.below(TOK.len())]);
                }
                init.push(s);
            }
            let trailing_nl = rng.below(2) == 0;
            let mut text = init.join("\n");
            if trailing_nl {
                text.push('\n');
            }
            let initial = text.clone();
            let mut c = core_with(&text);

            for step in 0..160 {
                let cur_text = c.doc.to_text();
                let bodies = rope_line_bodies(&cur_text);
                // 幻影末行：以换行收尾时 bodies 多出的最后一个空元素
                let ends_nl = cur_text.ends_with('\n');
                let real: &[String] =
                    if ends_nl && bodies.last().is_some_and(|s| s.is_empty()) {
                        &bodies[..bodies.len() - 1]
                    } else {
                        &bodies[..]
                    };
                let cur = c.cursor.line.min(real.len().saturating_sub(1));
                c.anchor = None; // 本批全部走「无选区」口径，oracle 才闭式
                c.cursor = CursorPos { line: cur, col: 0 };
                let join = |b: &[String]| -> String {
                    let mut t = b.join("\n");
                    if ends_nl && !t.is_empty() {
                        t.push('\n');
                    }
                    t
                };
                match rng.below(100) {
                    0..=24 => {
                        // 合并行：当前行并入下一行（末真实行无从并 → 不变）
                        let expected = if cur + 1 < real.len() {
                            let merged = [real[cur].trim(), real[cur + 1].trim()]
                                .iter()
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                                .collect::<Vec<_>>()
                                .join(" ");
                            let mut nb = real[..cur].to_vec();
                            nb.push(merged);
                            nb.extend_from_slice(&real[cur + 2..]);
                            join(&nb)
                        } else {
                            cur_text.clone()
                        };
                        let _changed = c.merge_lines();
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 合并发散"
                        );
                    }
                    25..=49 => {
                        // 拆分行：光标处断行（列随机）
                        let col = rng.below(real[cur].chars().count() + 1);
                        c.cursor.col = col;
                        let pre: String = real[cur].chars().take(col).collect();
                        let post: String = real[cur].chars().skip(col).collect();
                        let mut nb = real[..cur].to_vec();
                        nb.push(pre);
                        nb.push(post);
                        nb.extend_from_slice(&real[cur + 1..]);
                        let expected = join(&nb);
                        assert!(c.split_line());
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 拆分发散"
                        );
                    }
                    50..=74 => {
                        // 删空行：Empty / Whitespace 两口径（全文档）
                        let kind = if rng.below(2) == 0 {
                            BlankKind::Empty
                        } else {
                            BlankKind::Whitespace
                        };
                        let blank =
                            |s: &str| kind == BlankKind::Whitespace && s.trim().is_empty()
                                || s.is_empty();
                        let kept: Vec<String> = real
                            .iter()
                            .filter(|s| !blank(s))
                            .cloned()
                            .collect();
                        let expected = if kept.len() == real.len() {
                            cur_text.clone() // 无可删 no-op
                        } else if kept.is_empty() {
                            String::new() // 全删空且块在文档头 → 衔接符规则不加尾
                        } else {
                            join(&kept)
                        };
                        let _changed = c.delete_empty_lines(kind);
                        assert_eq!(
                            c.doc.to_text(),
                            expected,
                            "seed={seed} step={step} 删空行发散（kind={kind:?}）"
                        );
                    }
                    75..=94 => {
                        // 行注释切换（无语法 → `//`）：独立按规范重算
                        let nonblank: Vec<bool> =
                            real.iter().map(|s| !s.trim().is_empty()).collect();
                        let all_commented = real
                            .iter()
                            .zip(nonblank.iter())
                            .all(|(s, &b)| !b || s.trim_start().starts_with("//"));
                        let expected_bodies: Vec<String> = real
                            .iter()
                            .enumerate()
                            .map(|(i, s)| {
                                if !nonblank[i] {
                                    return s.clone();
                                }
                                let t = s.trim_start();
                                let ind = &s[..s.len() - t.len()];
                                if all_commented {
                                    let rest = &t["//".len()..];
                                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                                    format!("{ind}{rest}")
                                } else {
                                    format!("{ind}// {t}")
                                }
                            })
                            .collect();
                        let changed_expected = expected_bodies != real;
                        let changed = c.toggle_line_comment();
                        assert_eq!(changed, changed_expected, "seed={seed} step={step} 返回值不符");
                        assert_eq!(
                            c.doc.to_text(),
                            join(&expected_bodies),
                            "seed={seed} step={step} 注释切换发散"
                        );
                    }
                    _ => {
                        // 光标噪声：End/Left 移动，仅不变量把关
                        let m = if rng.below(2) == 0 { Motion::End } else { Motion::Left };
                        c.apply_motion(m, false);
                    }
                }
                assert_structural_invariants(&c);
            }

            let final_text = c.doc.to_text();
            while c.undo() {}
            assert_eq!(c.doc.to_text(), initial, "seed={seed} 撤销到底未回初始态");
            while c.redo() {}
            assert_eq!(c.doc.to_text(), final_text, "seed={seed} 重放终态发散");
        }
    }

