//! 查找操作：命中跳转、逐处步进、替换当前、复制标签路径。
//!
//! （P158 自 view.rs 拆出，纯移动零行为变更。）

use super::*;

impl Editpad {
    /// 第 62 轮：按索引跳到「查找全部」结果面板中的某一条命中。
    /// 与 [`Self::step_match`] 同一 select_span 口径（len_chars 自带
    /// 选区跨度），只是定位方式从光标相对序改为面板行号直选。
    pub(crate) fn goto_match_index(&mut self, index: usize) {
        let Some(pos) = self.matches.get(index).copied() else {
            return;
        };
        self.match_idx = Some(index);
        self.cur_handle
            .borrow_mut()
            .select_span(pos.line, pos.col, pos.len_chars);
        self.set_find_status(editpad_core::fmt_match_counter(self.lang(), index + 1, self.matches.len()));
    }

    /// 第 63 轮：复制某页的完整路径或文件名到剪贴板。
    /// `target`：None = 活动页（热键），Some(i) = 指定页（右键菜单）；
    /// 未命名页无路径可写，给状态栏提示不产生剪贴板写入。
    pub(crate) fn copy_tab_ident(&mut self, target: Option<usize>, full_path: bool) -> Task<Message> {
        let idx = target.unwrap_or(self.active_tab);
        let Some(tab) = self.tabs.get(idx) else {
            return Task::none();
        };
        let Some(path) = tab.path.clone() else {
            self.set_status(self.t(editpad_core::Key::StCopyPathUntitled).to_owned());
            return Task::none();
        };
        let payload = if full_path {
            path.display().to_string()
        } else {
            path.file_name()
                .map(|n| n.display().to_string())
                .unwrap_or_else(|| path.display().to_string())
        };
        self.set_status(editpad_core::fmt_suffix(self.lang(), editpad_core::Key::StCopied, &payload));
        iced::clipboard::write(payload)
    }

    pub(crate) fn step_match(&mut self, forward: bool) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        // 扫描在途：不基于过期命中表跳转
        if self.find_scanning() {
            self.set_status(self.t(editpad_core::Key::FindSearching).to_owned());
            return Task::none();
        }
        if self.matches.is_empty() {
            // 懒触发：开栏即按 Enter 而扫描还没排队过时，先补一次扫描
            return self.schedule_find_scan();
        }

        // 「上一个」以选区起点为原点：光标/选区正落在某命中上时一步跳到
        // 上一处——以命中末尾为原点会让第一按重新选中当前命中（需两按）；
        // 「下一个」维持光标（命中末尾）原点不变
        let (origin_line, origin_col) = {
            let ed = self.cur_handle.borrow();
            if forward {
                (ed.cursor.line, ed.cursor.col)
            } else {
                let (start, _) = ed.ordered_selection().unwrap_or((ed.cursor, ed.cursor));
                (start.line, start.col)
            }
        };
        let index = if forward {
            editpad_core::next_from(&self.matches, origin_line, origin_col)
        } else {
            editpad_core::prev_from(&self.matches, origin_line, origin_col)
        };
        self.match_idx = index;

        if let (Some(i), Some(pos)) =
            (index, index.and_then(|i| self.matches.get(i).copied()))
        {
            // P26：选区跨度直接用命中自带的 len_chars（扫描器产出的
            // 「选区显示跨度」口径），不再按当前输入现算查询长度——
            // 单行命中两者相等，跨行命中的正确性由数据自身保证，
            // 不依赖「命中表与输入框同步」这条时序假设
            self.cur_handle
                .borrow_mut()
                .select_span(pos.line, pos.col, pos.len_chars);
            self.set_find_status(editpad_core::fmt_match_counter(self.lang(), i + 1, self.matches.len()));
        }
        Task::none()
    }

    pub(crate) fn replace_current(&mut self) -> Task<Message> {
        if self.busy || self.find_query.is_empty() {
            return Task::none();
        }
        let effective_query = unescape_query(&self.find_query);
        let effective_replacement = unescape_query(&self.replace_query);
        let hit_selected = {
            let editor = self.cur_handle.borrow();
            let eol = editor.doc.line_ending();
            editor
                .selected_text()
                .is_some_and(|selected| {
                    // P26：两侧行尾归一后再比（复用 P9 口径）——CRLF 文档上
                    // 跨行命中的选区文本含 \r\n，而查询是 \n；不归一会让
                    // 「替换当前」永远判不等、退化为「跳下一个」
                    strings_equal(
                        &eol.normalize(&selected),
                        &eol.normalize(&effective_query),
                        self.case_sensitive,
                    )
                })
        };

        if hit_selected {
            self.cur_handle
                .borrow_mut()
                .replace_selection(&effective_replacement);
            self.tab_mut().dirty = true;
            // P10：替换后命中表已过期，排队后台重扫；「跳到下一个」等重扫完成
            // 后由用户再按（旧行为是同步重扫后立即跳，会卡大文档 UI）
            return self.schedule_find_scan();
        }
        // 没有可替换的选区：行为不变——跳到下一个匹配
        self.step_match(true)
    }

}
