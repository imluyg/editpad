use super::*;

/// 热键动作：持久化主键 `id` + 默认组合（可多个）+ 说明。
///
/// `id` 是 config.toml `hotkeys` 映射的键——**改 id = 破坏用户配置**，
/// 禁止；改默认组合只影响未重映射的用户。新增动作 = 追加一行 +
/// [`dispatch_action`] 加一个分支（测试清单自动覆盖）。
pub(crate) struct HotkeyAction {
    pub(crate) id: &'static str,
    /// 默认组合列表：首个是设置页展示的主键位，其余为同义附加默认
    /// （如重做的 `Ctrl+Y` 与主流的 `Ctrl+Shift+Z`）。
    pub(crate) default_combos: &'static [&'static str],
    pub(crate) desc: &'static str,
}

/// 键盘热键动作注册表（展示顺序 = 设置热键页顺序）。
/// Shift+滚轮 / Ctrl+滚轮为鼠标动作，不可重映射，仅 README 展示。
pub(crate) const HOTKEY_ACTIONS: &[HotkeyAction] = &[
    HotkeyAction {
        id: "open",
        default_combos: &["Ctrl+O"],
        desc: "打开文件",
    },
    HotkeyAction {
        id: "save",
        default_combos: &["Ctrl+S"],
        desc: "保存",
    },
    HotkeyAction {
        id: "select_all",
        default_combos: &["Ctrl+A"],
        desc: "全选",
    },
    HotkeyAction {
        id: "find",
        default_combos: &["Ctrl+F"],
        desc: "查找/替换栏",
    },
    HotkeyAction {
        id: "goto",
        default_combos: &["Ctrl+G"],
        desc: "跳转到行",
    },
    HotkeyAction {
        id: "undo",
        default_combos: &["Ctrl+Z"],
        desc: "撤销",
    },
    HotkeyAction {
        id: "redo",
        default_combos: &["Ctrl+Y", "Ctrl+Shift+Z"],
        desc: "重做（Ctrl+Y 或主流的 Ctrl+Shift+Z）",
    },
    HotkeyAction {
        id: "copy",
        default_combos: &["Ctrl+C"],
        desc: "复制选区",
    },
    HotkeyAction {
        id: "cut",
        default_combos: &["Ctrl+X"],
        desc: "剪切选区",
    },
    HotkeyAction {
        id: "paste",
        default_combos: &["Ctrl+V"],
        desc: "粘贴",
    },
    // 行操作套件（第 57 轮，仿主流编辑器编辑菜单）
    HotkeyAction {
        id: "dup_line",
        default_combos: &["Ctrl+D"],
        desc: "在下方复制当前行",
    },
    HotkeyAction {
        id: "del_line",
        default_combos: &["Ctrl+L"],
        desc: "删除当前行",
    },
    HotkeyAction {
        id: "move_line_up",
        default_combos: &["Ctrl+Shift+Up"],
        desc: "当前行上移",
    },
    HotkeyAction {
        id: "move_line_down",
        default_combos: &["Ctrl+Shift+Down"],
        desc: "当前行下移",
    },
    // 大小写转换与行首尾清理（第 58 轮；大小写默认键与主流编辑器一致，
    // Trim 在主流编辑器无默认键——取 Ctrl+Shift+T/L/B 助记：Trim/Leading/Both）
    HotkeyAction {
        id: "to_uppercase",
        default_combos: &["Ctrl+Shift+U"],
        desc: "转为大写（选区/全文）",
    },
    HotkeyAction {
        id: "to_lowercase",
        default_combos: &["Ctrl+U"],
        desc: "转为小写（选区/全文）",
    },
    HotkeyAction {
        id: "trim_trailing",
        default_combos: &["Ctrl+Shift+T"],
        desc: "去除行尾空白（选区行/全文）",
    },
    HotkeyAction {
        id: "trim_leading",
        default_combos: &["Ctrl+Shift+L"],
        desc: "去除行首空白（选区行/全文）",
    },
    HotkeyAction {
        id: "trim_both",
        default_combos: &["Ctrl+Shift+B"],
        desc: "去除行首尾空白（选区行/全文）",
    },
    // 行排序与去重（第 59 轮，仿主流编辑器行操作菜单；S/D=升/降序助记，K=去重）
    HotkeyAction {
        id: "sort_lines_asc",
        default_combos: &["Ctrl+Shift+S"],
        desc: "行升序排序（选区行/全文）",
    },
    HotkeyAction {
        id: "sort_lines_desc",
        default_combos: &["Ctrl+Shift+D"],
        desc: "行降序排序（选区行/全文）",
    },
    HotkeyAction {
        id: "dedupe_lines",
        default_combos: &["Ctrl+Shift+K"],
        desc: "去除重复行（选区行/全文）",
    },
    // 书签套件（第 60 轮，仿主流编辑器书签导航）。F2 家族与主流编辑器
    // 同默认键；Alt 系键位因 AltGr 保护不可用，清除全部取 Ctrl+Shift+F2；
    // 标记行批量操作无主流默认键，取 C/X 助记 = 复制/删除
    HotkeyAction {
        id: "bookmark_toggle",
        default_combos: &["Ctrl+F2"],
        desc: "切换当前行书签",
    },
    HotkeyAction {
        id: "bookmark_next",
        default_combos: &["F2"],
        desc: "跳到下一个书签",
    },
    HotkeyAction {
        id: "bookmark_prev",
        default_combos: &["Shift+F2"],
        desc: "跳到上一个书签",
    },
    HotkeyAction {
        id: "bookmark_clear_all",
        default_combos: &["Ctrl+Shift+F2"],
        desc: "清除全部书签",
    },
    HotkeyAction {
        id: "copy_marked_lines",
        default_combos: &["Ctrl+Shift+C"],
        desc: "复制全部标记行",
    },
    HotkeyAction {
        id: "del_marked_lines",
        default_combos: &["Ctrl+Shift+X"],
        desc: "删除全部标记行",
    },
    // 括号匹配（第 61 轮，仿主流编辑器；M=Matching 助记——主流的 Ctrl+\
    // 因反斜杠非字母/数字不入组合键白名单）
    HotkeyAction {
        id: "jump_bracket",
        default_combos: &["Ctrl+Shift+M"],
        desc: "跳到配对括号",
    },
    // 行操作扩充（第 62 轮；空闲字母键取 I/O/P/J/H/N/R，均可在设置页重映射）
    HotkeyAction {
        id: "tabs_to_spaces_leading",
        default_combos: &["Ctrl+Shift+I"],
        desc: "行首制表符转空格（选区行/全文）",
    },
    HotkeyAction {
        id: "tabs_to_spaces_all",
        default_combos: &["Ctrl+Shift+O"],
        desc: "全部制表符转空格（选区行/全文）",
    },
    HotkeyAction {
        id: "spaces_to_tabs_leading",
        default_combos: &["Ctrl+Shift+P"],
        desc: "行首空格转制表符（选区行/全文）",
    },
    HotkeyAction {
        id: "merge_lines",
        default_combos: &["Ctrl+Shift+J"],
        desc: "合并行（触及块合成一行/并入下一行）",
    },
    HotkeyAction {
        id: "split_line",
        default_combos: &["Ctrl+Shift+H"],
        desc: "拆分行（光标处断行/选区独立成行）",
    },
    HotkeyAction {
        id: "del_empty_lines",
        default_combos: &["Ctrl+Shift+N"],
        desc: "删除空行（选区行/全文）",
    },
    HotkeyAction {
        id: "del_blank_lines",
        default_combos: &["Ctrl+Shift+R"],
        desc: "删除空白行（含纯空白行）",
    },
    // 查找全部结果面板（A = All matches 助记；数据源复用查找栏的后台扫描命中表）
    HotkeyAction {
        id: "find_all_panel",
        default_combos: &["Ctrl+Shift+A"],
        desc: "查找全部结果面板",
    },
    // 第 63 轮：F5 插入日期时间（记事本同款裸功能键）+ 复制路径/文件名
    // （G/Q 为空闲字母，弱助记但 desc 明示；均可在设置页重映射）
    HotkeyAction {
        id: "insert_date_time",
        default_combos: &["F5"],
        desc: "插入日期时间（YYYY-MM-DD HH:MM）",
    },
    HotkeyAction {
        id: "copy_file_path",
        default_combos: &["Ctrl+Shift+G"],
        desc: "复制完整路径（当前页）",
    },
    HotkeyAction {
        id: "copy_file_name",
        default_combos: &["Ctrl+Shift+Q"],
        desc: "复制文件名（当前页）",
    },
    // 第 64 轮：行注释切换（Ctrl+Q，多编辑器通用的注释切换键位）+
    // 恢复上次关闭的标签页（W=与 Ctrl+W 关页互逆的助记）
    HotkeyAction {
        id: "toggle_line_comment",
        default_combos: &["Ctrl+Q"],
        desc: "切换行注释（按语法选 // # -- 等）",
    },
    HotkeyAction {
        id: "reopen_closed_tab",
        default_combos: &["Ctrl+Shift+W"],
        desc: "恢复上次关闭的标签页",
    },
    HotkeyAction {
        id: "new_tab",
        default_combos: &["Ctrl+T"],
        desc: "新建标签页",
    },
    HotkeyAction {
        id: "close_tab",
        default_combos: &["Ctrl+W"],
        desc: "关闭当前标签页",
    },
    HotkeyAction {
        id: "next_tab",
        default_combos: &["Ctrl+Tab"],
        desc: "循环切换标签页",
    },
    HotkeyAction {
        id: "format_json",
        default_combos: &["Ctrl+Shift+F"],
        desc: "格式化 JSON（仅 JSON 文件）",
    },
    HotkeyAction {
        id: "doc_start",
        default_combos: &["Ctrl+Home"],
        desc: "跳到文档首",
    },
    HotkeyAction {
        id: "doc_end",
        default_combos: &["Ctrl+End"],
        desc: "跳到文档尾",
    },
    // 查找导航（F3 家族与主流编辑器同默认键；Shift+F3 走裸功能键通道）+
    // 上一标签页（Ctrl+Shift+Tab 与 next_tab 互逆）
    HotkeyAction {
        id: "find_next",
        default_combos: &["F3"],
        desc: "查找下一个",
    },
    HotkeyAction {
        id: "find_prev",
        default_combos: &["Shift+F3"],
        desc: "查找上一个",
    },
    HotkeyAction {
        id: "prev_tab",
        default_combos: &["Ctrl+Shift+Tab"],
        desc: "循环切换标签页（反向）",
    },
    // P122：词级导航与删词（Ctrl+←/→ 与主流编辑器同默认键；
    // Ctrl+Backspace/Delete 删到词首/词尾，Backspace 补入组合键白名单）
    HotkeyAction {
        id: "word_left",
        default_combos: &["Ctrl+Left"],
        desc: "跳到上一个词首",
    },
    HotkeyAction {
        id: "word_right",
        default_combos: &["Ctrl+Right"],
        desc: "跳到下一个词尾",
    },
    HotkeyAction {
        id: "del_word_left",
        default_combos: &["Ctrl+Backspace"],
        desc: "删除到词首",
    },
    HotkeyAction {
        id: "del_word_right",
        default_combos: &["Ctrl+Delete"],
        desc: "删除到词尾",
    },
];

/// 动作 id 的首个默认组合（未重映射时的展示主键位）。
pub(crate) fn default_combo_of(id: &str) -> Option<&'static str> {
    HOTKEY_ACTIONS
        .iter()
        .find(|a| a.id == id)
        .and_then(|a| a.default_combos.first())
        .copied()
}

/// 过滤用户重映射表：动作 id 不在注册表中的条目删除（启动加载时调用）。
pub(crate) fn sanitize_hotkeys(settings: &mut editpad_core::Settings) {
    settings
        .hotkeys
        .retain(|id, _| HOTKEY_ACTIONS.iter().any(|a| a.id == *id));
}

/// 按键 → 规范组合串（P62；第 60 轮放宽功能键）：`Ctrl [+Shift] +键名`。
///
/// 契约（与 core `normalize_combo` 同步）：不得含 Alt（AltGr 保护，P8
/// 同口径——AltGr 在 Windows 上报为 Ctrl+Alt）；
/// - 含 Ctrl：键名 = 单个字母/数字（大写化）或白名单命名键，形如
///   `Ctrl+F2`；
/// - 无 Ctrl：仅放行功能键 F1~F12（修饰键至多 Shift），形如 `F2` /
///   `Shift+F2`——书签导航的主流默认键位所需；其余无 Ctrl 按键返回
///   None（该按键不参与热键系统，交回普通编辑路径）。
pub(crate) fn combo_string(mods: keyboard::Modifiers, key: &keyboard::Key) -> Option<String> {
    use keyboard::Key;
    if mods.alt() {
        return None;
    }
    let key_name = match key {
        Key::Character(chars) => {
            // 无 Ctrl 的字符键是打字正文，绝不参与热键
            if !mods.control() {
                return None;
            }
            let mut it = chars.chars();
            match (it.next(), it.next()) {
                (Some(ch), None) if ch.is_ascii_alphanumeric() => {
                    ch.to_ascii_uppercase().to_string()
                }
                _ => return None,
            }
        }
        Key::Named(named) => {
            let name = match named {
                Named::Home => "Home",
                Named::End => "End",
                Named::PageUp => "PageUp",
                Named::PageDown => "PageDown",
                Named::Tab => "Tab",
                Named::Insert => "Insert",
                Named::Delete => "Delete",
                // P122：Ctrl+Backspace 删到词首（Delete 原在白名单）
                Named::Backspace => "Backspace",
                Named::ArrowUp => "Up",
                Named::ArrowDown => "Down",
                Named::ArrowLeft => "Left",
                Named::ArrowRight => "Right",
                Named::F1 => "F1",
                Named::F2 => "F2",
                Named::F3 => "F3",
                Named::F4 => "F4",
                Named::F5 => "F5",
                Named::F6 => "F6",
                Named::F7 => "F7",
                Named::F8 => "F8",
                Named::F9 => "F9",
                Named::F10 => "F10",
                Named::F11 => "F11",
                Named::F12 => "F12",
                _ => return None,
            };
            name.to_string()
        }
        _ => return None,
    };
    if mods.control() {
        let mut combo = String::from("Ctrl");
        if mods.shift() {
            combo.push_str("+Shift");
        }
        combo.push('+');
        combo.push_str(&key_name);
        Some(combo)
    } else if is_function_key_name(&key_name) {
        // 第 60 轮：无 Ctrl 仅放行 F1~F12（至多带 Shift）
        let mut combo = String::new();
        if mods.shift() {
            combo.push_str("Shift+");
        }
        combo.push_str(&key_name);
        Some(combo)
    } else {
        None
    }
}

/// 键名是否为功能键 `F1`~`F12`（第 60 轮：无 Ctrl 组合的白名单）。
pub(crate) fn is_function_key_name(name: &str) -> bool {
    name.len() >= 2
        && name.starts_with('F')
        && name[1..]
            .parse::<u8>()
            .map(|n| (1..=12).contains(&n))
            .unwrap_or(false)
}

/// 生效动作查询：精确组合 → 用户重映射优先，其次**未重映射**动作的默认
/// 组合（已重映射的动作让出默认键位——用户显式挪走即视为放弃旧键位）。
pub(crate) fn effective_action(
    combo: &str,
    remap: &HashMap<String, String>,
) -> Option<&'static str> {
    for action in HOTKEY_ACTIONS {
        if remap.get(action.id).map(String::as_str) == Some(combo) {
            return Some(action.id);
        }
    }
    for action in HOTKEY_ACTIONS {
        // 未重映射的动作按全部默认组合匹配（多默认同义键位）
        if !remap.contains_key(action.id) && action.default_combos.contains(&combo) {
            return Some(action.id);
        }
    }
    None
}

/// 去掉组合串中的 Shift 修饰（文档导航的选区变体：Ctrl+Shift+Home 与
/// Ctrl+Home 同指 doc_start，Shift 经由 dispatch 透传给 Motion）。
pub(crate) fn shiftless(combo: &str) -> String {
    combo.replace("Shift+", "")
}

/// 动作 id → 消息。`mods` 供文档导航动作透传 Shift（选区语义）。
pub(crate) fn dispatch_action(id: &str, mods: keyboard::Modifiers) -> Option<Message> {
    let edit = |op| Some(Message::Edit(op));
    match id {
        "open" => Some(Message::OpenRequested),
        "save" => Some(Message::SaveRequested),
        "select_all" => Some(Message::Edit(EditOp::SelectAll)),
        "find" => Some(Message::FindToggled),
        "goto" => Some(Message::GotoToggled),
        "undo" => Some(Message::Edit(EditOp::Undo)),
        "redo" => Some(Message::Edit(EditOp::Redo)),
        "copy" => Some(Message::CopyRequested),
        "cut" => Some(Message::CutRequested),
        "paste" => Some(Message::PasteRequested),
        // 行操作套件（第 57 轮）
        "dup_line" => edit(EditOp::DuplicateLines),
        "del_line" => edit(EditOp::DeleteLines),
        "move_line_up" => edit(EditOp::MoveLinesUp),
        "move_line_down" => edit(EditOp::MoveLinesDown),
        // 大小写转换与行首尾清理（第 58 轮）
        "to_uppercase" => edit(EditOp::ConvertCase(CaseKind::Upper)),
        "to_lowercase" => edit(EditOp::ConvertCase(CaseKind::Lower)),
        "trim_leading" => edit(EditOp::TrimLines(TrimMode::Leading)),
        "trim_trailing" => edit(EditOp::TrimLines(TrimMode::Trailing)),
        "trim_both" => edit(EditOp::TrimLines(TrimMode::Both)),
        // 行排序与去重（第 59 轮）
        "sort_lines_asc" => edit(EditOp::SortLines(SortOrder::Ascending)),
        "sort_lines_desc" => edit(EditOp::SortLines(SortOrder::Descending)),
        "dedupe_lines" => edit(EditOp::RemoveDuplicateLines),
        // 书签套件（第 60 轮；copy_marked_lines 的剪贴板写入在消息层拦截）
        "bookmark_toggle" => edit(EditOp::ToggleBookmark),
        "bookmark_next" => edit(EditOp::BookmarkNext),
        "bookmark_prev" => edit(EditOp::BookmarkPrev),
        "bookmark_clear_all" => edit(EditOp::BookmarksClearAll),
        "copy_marked_lines" => edit(EditOp::CopyBookmarkedLines),
        "del_marked_lines" => edit(EditOp::RemoveBookmarkedLines),
        // 括号匹配（第 61 轮）
        "jump_bracket" => edit(EditOp::JumpToMatchingBracket),
        // 行操作扩充（第 62 轮）
        "tabs_to_spaces_leading" => {
            edit(EditOp::ConvertTabsSpaces(TabSpaceKind::LeadingTabsToSpaces))
        }
        "tabs_to_spaces_all" => edit(EditOp::ConvertTabsSpaces(TabSpaceKind::AllTabsToSpaces)),
        "spaces_to_tabs_leading" => {
            edit(EditOp::ConvertTabsSpaces(TabSpaceKind::LeadingSpacesToTabs))
        }
        "merge_lines" => edit(EditOp::MergeLines),
        "split_line" => edit(EditOp::SplitLine),
        "del_empty_lines" => edit(EditOp::DeleteEmptyLines(BlankKind::Empty)),
        "del_blank_lines" => edit(EditOp::DeleteEmptyLines(BlankKind::Whitespace)),
        // 查找全部结果面板（第 62 轮）
        "find_all_panel" => Some(Message::FindAllToggled),
        // 第 63 轮：热键版复制路径/文件名以活动页为目标（None）；右键
        // 菜单走同一消息的 Some(idx) 形态
        "insert_date_time" => edit(EditOp::InsertDateTime),
        "copy_file_path" => Some(Message::CopyFilePath(None)),
        "copy_file_name" => Some(Message::CopyFileName(None)),
        // 第 64 轮
        "toggle_line_comment" => edit(EditOp::ToggleLineComment),
        "reopen_closed_tab" => Some(Message::ReopenLastClosedFile),
        "new_tab" => Some(Message::NewTab),
        "close_tab" => Some(Message::CloseTabRequest),
        "next_tab" => Some(Message::SwitchTabNext),
        "prev_tab" => Some(Message::SwitchTabPrev),
        "find_next" => Some(Message::FindNext),
        "find_prev" => Some(Message::FindPrev),
        "format_json" => Some(Message::FormatJson),
        "doc_start" => edit(EditOp::Motion(Motion::DocStart, mods.shift())),
        "doc_end" => edit(EditOp::Motion(Motion::DocEnd, mods.shift())),
        // P122：词级导航与删词（词边界与删词共用 word_neighbor 口径；
        // Shift 透传 = 从光标扩展选区到词边界）
        "word_left" => edit(EditOp::Motion(Motion::WordLeft, mods.shift())),
        "word_right" => edit(EditOp::Motion(Motion::WordRight, mods.shift())),
        "del_word_left" => edit(EditOp::DeleteWordLeft),
        "del_word_right" => edit(EditOp::DeleteWordRight),
        _ => None,
    }
}

/// 全局按键分发（P62 可重映射）。
///
/// 顺序：① Ctrl 组合（无 Alt，P8 AltGr 保护）→ 规范组合串 → 生效动作表
/// （用户重映射优先，其次默认）→ 分发；Ctrl+Shift+Home/End 经 shiftless
/// 回退命中 doc_start/doc_end 并透传 Shift（选区语义）。② 其余按键
/// （字符输入/光标移动/回车退格/Esc）走既有编辑路径。
pub(crate) fn handle_key(
    key: keyboard::Key,
    mods: keyboard::Modifiers,
    remap: &HashMap<String, String>,
) -> Option<Message> {
    use keyboard::Key;
    let edit = |op| Some(Message::Edit(op));

    if mods.control() && !mods.alt() {
        if let Some(combo) = combo_string(mods, &key) {
            let id = effective_action(&combo, remap).or_else(|| {
                let stripped = shiftless(&combo);
                if stripped == combo {
                    return None;
                }
                // 仅文档导航接受 Shift 变体回退（其余动作 Shift 组合是
                // 独立动作，如 Ctrl+Shift+F ≠ Ctrl+F）
                effective_action(&stripped, remap)
                    .filter(|id| matches!(*id, "doc_start" | "doc_end"))
            });
            if let Some(id) = id {
                return dispatch_action(id, mods);
            }
        }
        return None;
    }

    // 第 60 轮：裸功能键热键（书签导航 F2 家族）。combo_string 在无 Ctrl
    // 时只对 F1~F12 产出组合串，未注册动作的功能键放行（F 键本无编辑
    // 语义，落到末尾返回 None 与既有行为一致）
    if let Some(combo) = combo_string(mods, &key) {
        if let Some(id) = effective_action(&combo, remap) {
            return dispatch_action(id, mods);
        }
    }

    match &key {
        Key::Character(chars) => edit(EditOp::InsertText(chars.to_string())),
        Key::Named(Named::Backspace) => edit(EditOp::Backspace),
        Key::Named(Named::Delete) => edit(EditOp::Delete),
        // P121：回车改走智能缩进（插入换行 + 当前行行首空白）；换行仍由
        // insert_str 归一为文档主导行尾（CRLF 文档得 \r\n）
        Key::Named(Named::Enter) => edit(EditOp::Enter),
        // P121：Tab 有选区 = 触及行整块缩进（Shift+Tab 反缩进），无选区
        // 仍插真实制表符；显示层由 editor::char_cols 展开到制表位，
        // 文档字节保持原样（保存往返不失真）
        Key::Named(Named::Tab) => edit(EditOp::TabKey(mods.shift())),
        Key::Named(Named::Escape) => Some(Message::BarsDismissed),

        Key::Named(Named::ArrowLeft) => edit(EditOp::Motion(Motion::Left, mods.shift())),
        Key::Named(Named::ArrowRight) => edit(EditOp::Motion(Motion::Right, mods.shift())),
        Key::Named(Named::ArrowUp) => edit(EditOp::Motion(Motion::Up, mods.shift())),
        Key::Named(Named::ArrowDown) => edit(EditOp::Motion(Motion::Down, mods.shift())),
        Key::Named(Named::Home) => edit(EditOp::Motion(Motion::Home, mods.shift())),
        Key::Named(Named::End) => edit(EditOp::Motion(Motion::End, mods.shift())),
        Key::Named(Named::PageUp) => edit(EditOp::Motion(Motion::PageUp, mods.shift())),
        Key::Named(Named::PageDown) => edit(EditOp::Motion(Motion::PageDown, mods.shift())),

        _ => None,
    }
}
