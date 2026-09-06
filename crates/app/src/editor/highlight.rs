use super::*;

impl EditorCore {
    /// 按文件扩展名启用语法高亮（测试辅助；生产路径已改走
    /// [`Self::set_language_by_name`]）。
    #[cfg(test)]
    pub fn set_language(&mut self, extension: Option<&str>) {
        self.highlight = extension.and_then(LazyHighlighter::new).map(RefCell::new);
    }

    /// 按语法名启用高亮（P22）：名字来自 core 的别名层/嗅探层解析；
    /// None 关闭（纯文本快速路径）。
    pub fn set_language_by_name(&mut self, name: Option<&str>) {
        self.highlight = name
            .and_then(LazyHighlighter::new_by_name)
            .map(RefCell::new);
    }

    /// 语法高亮主题随应用明暗档切换（编辑器配色与 syntect 主题都要
    /// 换档；纯文本页无高亮器，no-op）。换主题经 core 整体重建高亮器，
    /// 新代次让在途补建结果自然作废。
    pub fn apply_highlight_theme(&mut self, dark: bool) {
        if let Some(hl) = &self.highlight {
            hl.borrow_mut().set_dark_mode(dark);
        }
    }

    /// 第 `offset` 字符偏移之后的高亮状态失效。
    pub(crate) fn invalidate_highlight_from(&mut self, offset: usize) {
        // 第 61 轮：本函数是全部正文突变路径的唯一汇点——括号匹配缓存
        // 在此统一失效（光标键控的缓存对「同位异文」不可见，必须显式清）
        self.bracket_cache.borrow_mut().take();
        // 第 63 轮：选区跨度缓存同汇点失效（偏移键控对「同位异文」同理）
        self.sel_span_cache.borrow_mut().take();
        // P133：链接悬停跨度同汇点清空——编辑后字符区间可能失配，
        // 下一次鼠标移动重新探测
        self.link_hover = None;
        self.last_link_probe = None;
        // 第 73 轮 ⑯：软换行缓存同汇点失效——行数变化整表重置，
        // 否则只推代次（memo 过期由下次查询懒惰重算）
        self.wrap.borrow_mut().after_edit(self.doc.line_count());
        if let Some(hl) = &self.highlight {
            let line = self.doc.char_to_line(offset.min(self.doc.text_len()));
            hl.borrow_mut().invalidate_from(line);
        }
    }

    /// 计算第 `line_idx` 行的着色片段；未启用高亮时返回空表。
    ///
    /// P12：补建预算受限——缺失检查点档位超过
    /// [`LazyHighlighter::MAX_INLINE_STRIDES`] 时不再于本线程现算
    /// （旧实现会在 UI 线程一次性冻结数十秒），而是返回无色渲染，
    /// 同时经 [`Self::needs_paving`] 由应用层安排后台分批补建；
    /// 铺建推进到该行后自然恢复配色。
    ///
    /// P61 渐进上色：精确预算不足时改走**可视区近似上色**（anchor =
    /// 可视首行的全新语法状态，独立缓存、不污染精确路径）——大跳转后
    /// 视口立即有近似配色，后台精确铺建到达后自然替换。可视区之外的
    /// 行保持无色（反正画不出来）。
    pub(crate) fn highlight_runs(&self, line_idx: usize, target_text: &str) -> Vec<StyledRun> {
        let Some(hl) = &self.highlight else {
            return Vec::new();
        };
        let doc = &self.doc;
        let total = doc.line_count();
        if let Some(runs) = hl.borrow_mut().styled_line_limited(
            line_idx,
            target_text,
            total,
            LazyHighlighter::MAX_INLINE_STRIDES,
            &mut |i| doc.line_str(i).trim_end_matches(['\n', '\r']).to_owned(),
        ) {
            return runs;
        }
        let anchor = self.visible_range().0;
        hl.borrow_mut().styled_line_approx(
            line_idx,
            anchor,
            target_text,
            total,
            &mut |i| doc.line_str(i).trim_end_matches(['\n', '\r']).to_owned(),
        )
    }

    /// 可见区域是否还有「内联预算外」的缺档（需要安排后台铺建）。
    /// 以视口最末行判断：O(1)，每帧在 RedrawRequested 里调用。
    pub fn needs_paving(&self) -> bool {
        let Some(hl) = &self.highlight else {
            return false;
        };
        let last_visible = self.visible_range().1;
        hl.borrow().strides_missing(last_visible) > LazyHighlighter::MAX_INLINE_STRIDES
    }

    /// 当前使用的语法名（None = 纯文本路径）。
    /// 状态栏展示与「按当前语法判断」的功能开关共用。
    pub fn highlight_syntax_name(&self) -> Option<String> {
        self.highlight
            .as_ref()
            .map(|h| h.borrow().syntax_name().to_owned())
    }

    /// 当前高亮器代次；未启用高亮时为 None。
    pub fn highlight_generation(&self) -> Option<u64> {
        self.highlight.as_ref().map(|h| h.borrow().generation())
    }

    /// 检查点数量（测试诊断用；生产路径经 [`Self::needs_paving`] 间接消费）。
    #[cfg(test)]
    pub fn highlight_checkpoints_len(&self) -> Option<usize> {
        self.highlight.as_ref().map(|h| h.borrow().checkpoints_len())
    }

    /// 后台铺建的起点快照：(代次, 高亮器克隆)。文档克隆由调用方另行完成。
    /// 未启用高亮时为 None。
    pub fn highlight_pave_snapshot(&self) -> Option<(u64, LazyHighlighter)> {
        self.highlight.as_ref().map(|h| {
            let hl = h.borrow();
            (hl.generation(), hl.clone())
        })
    }

    /// 安装一份后台铺建成果；仅当其代次与当前高亮器一致时生效（P12）。
    /// 返回是否安装。代次不符 = 期间发生过编辑，成果整体丢弃。
    ///
    /// 整体替换会丢掉铺建期间同步路径攒下的少量行缓存（重新按需推导，
    /// 只影响速度不影响正确性），换来的是无需合并逻辑的简单协议。
    pub fn install_highlighter_if_current(&self, generation: u64, paved: LazyHighlighter) -> bool {
        let Some(hl) = &self.highlight else {
            return false;
        };
        let mut hl = hl.borrow_mut();
        if hl.generation() != generation {
            return false;
        }
        *hl = paved;
        true
    }
}
