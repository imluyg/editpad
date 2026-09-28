//! B12「变更历史行边条」的数据侧：把"相对落盘基线哪几行改了"算成一页一份、
//! 按纪元缓存的标记表。渲染只问 [`EditorCore::change_mark_at`]，不认识差分本身。
//!
//! 三条刻意的口径，都各有用例盯着：
//!
//! - **新鲜度是两个键**：正文纪元（[`EditorCore::content_epoch`]）+ 基线代次
//!   （[`EditorCore::baseline_epoch`]）。只盯正文会漏掉"保存成功"那一刻——
//!   基线换了、正文没换，缓存继续把已存的行画成没存，正是边条最难查的一种错。
//! - **无基线就不画**：`saved_baseline` 为 `None`（P30 会话恢复的保守路径）时
//!   宁可不显示，也不拿"空基线"当"全文都是新写的"。
//! - **大文档直接不算**：core 那边超字节预算返回 [`editpad_core::DiffQuality::Skipped`]，
//!   这里就一张空表——被动显示不值得为 50 MB 日志每次按键扫全文（P298 那笔账）。

use super::*;

/// 一次边条计算的缓存。
pub(crate) struct ChangeMemo {
    /// 算的时候正文是哪个纪元。
    content_epoch: u64,
    /// 算的时候落盘基线是哪个代次。
    baseline_epoch: u64,
    /// 行号升序的标记表（按行二分查找，与书签 `BTreeSet` 同口径的 O(log n)/行）。
    marks: Vec<(usize, editpad_core::MarkKind)>,
}

impl EditorCore {
    /// 应用层从 Settings 下发开关。关掉时顺手把缓存放掉——只是**内存回收**，
    /// 不承担正确性（"关→开之间基线动了"那格由双键比较兜，见
    /// [`Self::bump_baseline_epoch`]）。
    pub(crate) fn set_change_strip(&mut self, on: bool) {
        self.change_strip = on;
        if !on {
            self.change_memo.replace(None);
        }
    }

    /// 第 `line` 行的边条种类；`None` = 不画。
    ///
    /// 只在两个键都过期时才算一次，所以"每帧一次、每次扫全文"不会发生
    /// （`take_change_diffs` / `take_change_keys` 两个仪表就是钉这个的）。
    pub(crate) fn change_mark_at(&self, line: usize) -> Option<editpad_core::MarkKind> {
        if !self.change_strip {
            return None;
        }
        let base = self.saved_baseline.as_ref()?;
        let epoch = self.content_epoch;
        let bep = self.baseline_epoch;
        let mut memo = self.change_memo.borrow_mut();
        let fresh = memo
            .as_ref()
            .is_some_and(|m| m.content_epoch == epoch && m.baseline_epoch == bep);
        if !fresh {
            let diff = editpad_core::changed_lines(base, &self.doc);
            #[cfg(test)]
            {
                self.change_diffs.set(self.change_diffs.get() + 1);
                self.change_keys
                    .set(self.change_keys.get() + diff.keys_examined as u64);
            }
            *memo = Some(ChangeMemo {
                content_epoch: epoch,
                baseline_epoch: bep,
                marks: editpad_core::classify_changed_lines(&diff.changed),
            });
        }
        let m = memo.as_ref()?;
        let i = m.marks.binary_search_by_key(&line, |(l, _)| *l).ok()?;
        Some(m.marks[i].1)
    }

    /// 测试钩子：读出并清零"重算了几次边条"。生产构建整字段不参与编译。
    #[cfg(test)]
    pub(crate) fn take_change_diffs(&self) -> u64 {
        self.change_diffs.replace(0)
    }

    /// 测试钩子：读出并清零"累计做了几次行内容哈希"——成本护栏的读数。
    #[cfg(test)]
    pub(crate) fn take_change_keys(&self) -> u64 {
        self.change_keys.replace(0)
    }

    /// 基线换过一次（保存成功 / 撤销基线 / 换文档）：代次 +1。
    ///
    /// ⚠️ 这里**故意不**清缓存——失效只由 [`Self::change_mark_at`] 的双键比较
    /// 负责。两处都做同一件事的话，键就没牙齿了（第 234 轮的变异探针把键短路掉
    /// 仍全绿，正是被这种"双保险"喂出来的假绿）。
    pub(crate) fn bump_baseline_epoch(&mut self) {
        self.baseline_epoch += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with(text: &str) -> EditorCore {
        let mut c = EditorCore::default();
        c.reset_document(editpad_core::Document::from_str(text));
        c.set_change_strip(true);
        c
    }

    /// 在第 `line` 行行首插 `t`（走真实编辑入口，不手改文档）。
    fn insert_at(c: &mut EditorCore, line: usize, t: &str) {
        let off = c.doc.line_to_char(line);
        c.snapshot();
        c.doc.insert(off, t);
        c.invalidate_highlight_from(off);
    }

    #[test]
    fn marks_follow_the_saved_baseline_and_clear_on_save() {
        let mut c = core_with("one\ntwo\nthree\n");
        assert!(c.change_mark_at(0).is_none(), "刚加载完不该有边条");
        insert_at(&mut c, 1, "INSERTED\n");
        assert_eq!(
            c.change_mark_at(1),
            Some(editpad_core::MarkKind::Changed),
            "新插入的那行要亮"
        );
        assert!(
            c.change_mark_at(0).is_none() && c.change_mark_at(2).is_none(),
            "位移不影响别的行：0 与 2 都该干净"
        );
        // 落盘：基线抬到当前内容 ⇒ 边条整片清空（这一步只动基线、不动正文，
        // 所以它必须能看见缓存作废）
        c.mark_saved();
        for line in 0..4 {
            assert!(
                c.change_mark_at(line).is_none(),
                "保存后第 {line} 行不该还亮"
            );
        }
    }

    #[test]
    fn reverted_line_becomes_a_gap_inside_the_block_not_a_blank() {
        // 连续改两行、再把其中一行撤销回去：回到的那行走"块内空隙"色，
        // 而不是干脆没标记——这才是用户看得见的"这一段动过"
        let mut c = core_with("a\nb\nc\nd\n");
        c.mark_saved();
        insert_at(&mut c, 1, "B1\n");
        insert_at(&mut c, 2, "C1\n");
        let kinds: Vec<(usize, editpad_core::MarkKind)> = (0..6)
            .filter_map(|l| c.change_mark_at(l).map(|k| (l, k)))
            .collect();
        assert!(
            kinds
                .iter()
                .any(|(_, k)| *k == editpad_core::MarkKind::Changed),
            "至少要有实色格，实得 {kinds:?}"
        );
        // 撤销回基线：改动全消，边条整片归零（撤销链本身由别的用例钉）
        let base_text = c.saved_baseline.as_ref().unwrap().to_text();
        c.reset_document(editpad_core::Document::from_str(&base_text));
        c.set_change_strip(true);
        c.mark_saved();
        for l in 0..5 {
            assert!(
                c.change_mark_at(l).is_none(),
                "实得 {:?}",
                c.change_mark_at(l)
            );
        }
    }

    #[test]
    fn the_cache_is_keyed_on_both_content_and_baseline() {
        // 一次编辑 = 一次重算；随后连问 N 次都不再重算（这才叫缓存，不是每帧全文）
        let mut c = core_with("x1\nx2\nx3\nx4\nx5\n");
        insert_at(&mut c, 0, "top\n");
        assert_eq!(c.take_change_diffs(), 0, "夹具自证：读之前不该算");
        let _ = c.change_mark_at(0);
        for line in 0..6 {
            let _ = c.change_mark_at(line);
        }
        assert_eq!(c.take_change_diffs(), 1, "同一纪元内六次读只该算一次");
        // 再改一行：只改正文、还没读 ⇒ 不该有计算
        insert_at(&mut c, 2, "mid\n");
        assert_eq!(c.take_change_diffs(), 0, "改了正文但没读，不该提前算");
        let _ = c.change_mark_at(2);
        assert_eq!(c.take_change_diffs(), 1, "纪元变了要重算一次");
        let _ = c.change_mark_at(0);
        assert_eq!(c.take_change_diffs(), 0, "又落回缓存");
        // 基线换了（保存）也要重算——这一格就是"两个键缺一不可"的读数
        c.mark_saved();
        assert_eq!(c.take_change_diffs(), 0, "只动基线、还没读");
        let _ = c.change_mark_at(0);
        assert_eq!(
            c.take_change_diffs(),
            1,
            "正文没变、只有基线变 ⇒ 仍须重算（单键缓存会在这里漏）"
        );
        let _ = c.change_mark_at(1);
        assert_eq!(c.take_change_diffs(), 0, "基线代次落回缓存");
    }

    #[test]
    fn a_mid_document_edit_does_not_hash_the_whole_file() {
        // 成本护栏（app 侧）：2 万行改中间一行，累计的行内容哈希次数只该与
        // 改动窗口同量级。core 那边有同一格的纯函数版；这一格钉的是"缓存
        // 真的把它挡住了"（每帧问一遍不该变成每帧扫一遍）
        let text: String = (0..20_000)
            .map(|i| format!("row {i} filler filler\n"))
            .collect();
        let mut c = core_with(&text);
        c.take_change_keys();
        insert_at(&mut c, 10_000, "EDITED\n");
        for l in [9_999usize, 10_000, 10_001] {
            let _ = c.change_mark_at(l);
        }
        let keys = c.take_change_keys();
        assert!(keys <= 16, "改一行不该扫全文：本次累计哈希了 {keys} 行");
        assert!(
            c.change_mark_at(10_000).is_some(),
            "夹具自证：这一格确实有标记"
        );
        // ⚠️ 光看次数会漏：退化到保守档时 `keys` 直接归零，"≤16" 就白给了
        // （第 234 轮的变异探针把公共前缀裁剪拆掉，这一格照样绿）。所以再钉
        // 一条**结果形状**：窗口之外的行不得亮——保守档会把 0..=10000 全标上。
        assert!(
            c.change_mark_at(0).is_none(),
            "改动在第 10000 行，第一行不该亮（退化到保守档的迹象）"
        );
        assert!(
            c.change_mark_at(15_000).is_none(),
            "同理：后一半的行也不该亮"
        );
    }

    #[test]
    fn off_or_baseline_less_pages_draw_nothing() {
        // 开关关着：一切为 None，且不重算
        let mut c = core_with("a\nb\n");
        insert_at(&mut c, 0, "X\n");
        c.set_change_strip(false);
        assert!(c.change_mark_at(0).is_none());
        assert_eq!(c.take_change_diffs(), 0, "关着的开关不该驱动任何计算");
        // 无基线（P30 会话恢复那条保守路径）：宁可不显示
        let mut d = core_with("a\nb\n");
        d.clear_saved_baseline();
        insert_at(&mut d, 0, "X\n");
        assert!(
            d.change_mark_at(0).is_none(),
            "没有基线时不许把全文当「全新」涂满边条"
        );
        // 开关关掉再打开，不得吐上一次缓存的旧标记
        let mut e = core_with("a\nb\n");
        insert_at(&mut e, 0, "X\n");
        assert!(e.change_mark_at(0).is_some());
        e.set_change_strip(false);
        e.mark_saved();
        e.set_change_strip(true);
        assert!(
            e.change_mark_at(0).is_none(),
            "关→开之间基线动了，重开必须按新基线算"
        );
    }
}
