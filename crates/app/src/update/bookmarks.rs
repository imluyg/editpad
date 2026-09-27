//! P318：书签列表面板的三个动作（开关／逐条跳／逐条摘）。
//!
//! 面板**不持有**行数据：每次建视图现取当前页的书签集
//! （[`Editpad::bookmark_panel_lines`]），所以别处改了书签——清除全部、
//! 删标记行、编辑触发的行号再映射——下一帧自动跟上，不需要第二条刷新通路。
//! 代价是列表可能与当前状态差一帧（例如面板开着时按 Ctrl+Shift+F2），
//! 因此「×」摘一条要能识别"那一行其实已经没有书签"并给对应反馈，
//! 而不是假装摘掉了。

use super::*;

impl Editpad {
    pub(crate) fn update_bookmarks(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::BookmarksToggled => {
                self.bookmarks_visible = !self.bookmarks_visible;
            }
            Message::BookmarkGoto(line) => {
                // 跳转走「转到行」同一条路：它负责记出发点（P312 的历史栈）、
                // 打断组字、清竖向目标，并按现存行域夹紧越界落点
                self.cur_handle
                    .borrow_mut()
                    .jump_to_line(line.saturating_add(1));
            }
            Message::BookmarkRemoveAt(line) => {
                // ⚠️ 先出借再 set_status：本函数的调用方在 update 顶层，
                // 借还活着时再进应用层是 P135 那条调试入口守卫专门拦的形状
                let removed = self.cur_handle.borrow_mut().remove_bookmark_at(line);
                let key = if removed {
                    editpad_core::Key::StBookmarkRemoved
                } else {
                    editpad_core::Key::StBookmarkNotThere
                };
                self.set_status(self.t(key).to_owned());
            }
            _ => {}
        }
        Task::none()
    }
}
