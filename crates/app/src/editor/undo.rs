use super::*;

impl EditorCore {
    /// 登记落盘基线 = **当前内容**。仅在「当前内容确已写入磁盘」的时刻
    /// 调用（保存/自动保存成功且版本守卫通过）；版本不符时不得调用——
    /// 那时磁盘上是旧快照，基线保持不动才能维持 dirty 的正确性。
    /// rope 克隆是结构共享，O(1)。
    pub fn mark_saved(&mut self) {
        self.saved_baseline = Some(self.doc.clone());
    }

    /// 当前内容是否与最近一次落盘基线一致。撤销/重做后由应用层查询，
    /// 决定 dirty 是否可以回清。无基线时保守返回 false。
    pub fn is_at_saved_content(&self) -> bool {
        self.saved_baseline
            .as_ref()
            .is_some_and(|base| base.content_eq(&self.doc))
    }

    /// 撤销落盘基线（置 None）。用于 P30 会话恢复：恢复出来的置脏页内容
    /// 来自快照而非磁盘，若把快照当基线，「编辑→撤销回快照态」会错误
    /// 清脏——命名页下次启动从磁盘旧内容重载、未命名页直接空白，未存
    /// 改动将静默丢失。清掉后 `is_at_saved_content` 保守返回 false，该页
    /// 在真正落盘一次之前撤销永不回清，宁可不便利也不能丢内容。
    pub fn clear_saved_baseline(&mut self) {
        self.saved_baseline = None;
    }

    pub(crate) fn snapshot(&mut self) {
        // P37：开新快照 = 上一组就此终结（删除类编辑/整体替换/非合并
        // 插入都经此处打断成组；合并插入走的是跳过本函数的路径）
        self.typing_run = None;
        // P45：任何经快照的编辑都可能改变文档宽度结构——高水位可能
        // 过估，标记惰性收敛（合并插入只增不减，无需标记）
        self.max_cols_stale = true;
        self.undo_stack.push(Snapshot {
            doc: self.doc.clone(), // rope 克隆是结构共享，廉价
            cursor: self.cursor,
            anchor: self.anchor,
            bookmarks: self.bookmarks.clone(),
        });
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        self.break_typing(); // P37：撤销本身打断组，防后续输入混入历史组
        self.goal_px = None; // 第 73 轮 ⑯：撤销 = 非竖向操作，清 goal
        self.clear_block(); // 第 67 轮：列块不参与快照回滚，一并清除
        let Some(snap) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
            // 第 60 轮：书签随快照对换回滚（见 Snapshot::bookmarks 注释）
            bookmarks: std::mem::replace(&mut self.bookmarks, snap.bookmarks),
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        // 文档被整体替换，高亮状态全量失效；第 61 轮：经唯一汇点，
        // 括号匹配缓存一并清（撤销落点恰为缓存键时防陈旧命中）
        self.invalidate_highlight_from(0);
        // P45：撤销 = 整个文档替换，宽度结构可能缩短——标记惰性收敛
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.break_typing(); // P37 同上
        self.goal_px = None; // 第 73 轮 ⑯：重做 = 非竖向操作，清 goal
        self.clear_block(); // 第 67 轮：同 undo
        let Some(snap) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(Snapshot {
            doc: std::mem::replace(&mut self.doc, snap.doc),
            cursor: self.cursor,
            anchor: self.anchor,
            // 第 60 轮：重做对称回滚书签（同 undo）
            bookmarks: std::mem::replace(&mut self.bookmarks, snap.bookmarks),
        });
        self.cursor = snap.cursor;
        self.anchor = snap.anchor;
        // 第 61 轮：经唯一汇点失效（同 undo，清括号匹配缓存）
        self.invalidate_highlight_from(0);
        // P45：重做同样整体替换文档——标记惰性收敛（对称 undo）
        self.max_cols_stale = true;
        self.ensure_visible();
        true
    }
}
