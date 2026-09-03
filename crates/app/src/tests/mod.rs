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

    /// 测试内派发：显式丢弃 Task（update 的返回值仅运行时消费）
    ///（自 editor.rs 迁入：多个域文件共用，见第 85 轮 Phase 3a）
    fn dispatch(app: &mut Editpad, message: Message) {
        let _ = app.update(message);
    }

    /// 测试便捷：默认（无重映射）热键分发（自 settings.rs 迁入，多域共用）。
    fn handle_key_defaults(
        key: keyboard::Key,
        mods: keyboard::Modifiers,
    ) -> Option<Message> {
        handle_key(key, mods, &std::collections::HashMap::new())
    }

    /// 构造一个已按 .txt 加载完成的应用（纯文本路径）（自 file.rs 迁入，多域共用）。
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

    /// 构造「真实落盘文件 + 已按该文件加载完成」的应用：
    /// `file_stamp` 为真实磁盘戳，供外部修改守卫测试使用（自 file.rs 迁入）。
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

    /// 构造 N 页应用：页 0 置脏（内容 "d0"），其余干净；返回应用（自 tabs.rs 迁入）。
    fn app_with_tabs(n: usize) -> Editpad {
        let mut app = Editpad::default();
        dispatch(&mut app, Message::Edit(EditOp::InsertText("d0".into())));
        for _ in 1..n {
            dispatch(&mut app, Message::NewTab);
        }
        app
    }

    /// 把规范组合串解析回 (修饰键, 按键)——热键表与分发一致性的驱动器
    ///（自 settings.rs 迁入，多域共用）。
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

mod tabs;
mod file;
mod find;
mod session;
mod settings;
mod editor;
mod robustness;
