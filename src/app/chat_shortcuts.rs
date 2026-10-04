//! 主窗口的会话导航。独立聊天 viewport 固定房间，不消费这里的快捷键。
//!
//! 在更新输入法状态之后、绘制任何控件之前调用 `handle_chat_navigation_shortcuts`。
//! 普通 Tab 只允许主聊天区无控件焦点的情况；编辑器、按钮、搜索框仍使用原有 Tab。

use egui::{Event, Key, ViewportId};

use crate::app::{CompactChatPanel, IcaApp, SelectedChatGroup};
use crate::config::ChatGroups;
use crate::ica::types::{RoomId, room::Room};

/// 会话列表和快捷键共用筛选、排序，避免搜索、分类和置顶规则各自演化。
pub fn filtered_sorted_room_indices(
    rooms: &[Room],
    groups: &ChatGroups,
    selected_group: &SelectedChatGroup,
    disable_groups: bool,
    search: &str,
) -> Vec<usize> {
    let query = search.trim().to_uppercase();
    let mut indices: Vec<_> = rooms
        .iter()
        .enumerate()
        .filter(|(_, room)| {
            disable_groups
                || match selected_group {
                    SelectedChatGroup::All => true,
                    SelectedChatGroup::Group => room.room_id < 0,
                    SelectedChatGroup::Private => room.room_id > 0,
                    SelectedChatGroup::Custom(index) => {
                        groups.groups.get(*index).is_some_and(|group| {
                            group.rooms.contains(&room.room_id)
                                || (group.include_all_personal && room.room_id > 0)
                        })
                    }
                }
        })
        .filter(|(_, room)| {
            query.is_empty()
                || room.room_name.to_uppercase().contains(&query)
                || room.room_id.to_string().contains(&query)
        })
        .map(|(index, _)| index)
        .collect();
    // 保持列表原有的稳定排序：先置顶，再按最后活跃时间倒序，同值保持 Bridge 顺序。
    indices.sort_by(|&a, &b| {
        (rooms[b].index > 0)
            .cmp(&(rooms[a].index > 0))
            .then(rooms[b].utime.cmp(&rooms[a].utime))
    });
    indices
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatShortcut {
    Previous,
    Next,
    Unread,
    Group(SelectedChatGroup),
}

fn chat_shortcut(event: &Event) -> Option<ChatShortcut> {
    let Event::Key {
        key,
        pressed: true,
        repeat: false,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    // 只匹配所声明的 Ctrl/Alt 组合，避免把 AltGr、Ctrl+Alt 或系统 Command 当成导航。
    if modifiers.mac_cmd || (modifiers.command && !modifiers.ctrl) {
        return None;
    }
    if modifiers.ctrl && !modifiers.alt {
        if *key == Key::Tab {
            return Some(if modifiers.shift {
                ChatShortcut::Previous
            } else {
                ChatShortcut::Next
            });
        }
        if modifiers.shift {
            return None;
        }
        let group = match key {
            Key::Num1 => SelectedChatGroup::All,
            Key::Num2 => SelectedChatGroup::Group,
            Key::Num3 => SelectedChatGroup::Private,
            Key::Num4 => SelectedChatGroup::Custom(0),
            Key::Num5 => SelectedChatGroup::Custom(1),
            Key::Num6 => SelectedChatGroup::Custom(2),
            Key::Num7 => SelectedChatGroup::Custom(3),
            Key::Num8 => SelectedChatGroup::Custom(4),
            Key::Num9 => SelectedChatGroup::Custom(5),
            _ => return None,
        };
        return Some(ChatShortcut::Group(group));
    }
    if modifiers.alt && !modifiers.ctrl && !modifiers.shift {
        return match key {
            Key::ArrowUp => Some(ChatShortcut::Previous),
            Key::ArrowDown => Some(ChatShortcut::Next),
            _ => None,
        };
    }
    (*key == Key::Tab && modifiers.is_none()).then_some(ChatShortcut::Unread)
}

fn shortcut_room(
    shortcut: &ChatShortcut,
    rooms: &[Room],
    visible_indices: &[usize],
    selected: Option<RoomId>,
) -> Option<RoomId> {
    match shortcut {
        ChatShortcut::Next | ChatShortcut::Previous => {
            let first = *visible_indices.first()?;
            let current = visible_indices
                .iter()
                .position(|&index| Some(rooms[index].room_id) == selected);
            let index = match (current, shortcut) {
                (Some(index), ChatShortcut::Next) => {
                    visible_indices[(index + 1) % visible_indices.len()]
                }
                (Some(index), _) => {
                    visible_indices[(index + visible_indices.len() - 1) % visible_indices.len()]
                }
                // 尚未选房，或当前房间被筛选掉，两种方向都从当前列表首项开始。
                (None, _) => first,
            };
            Some(rooms[index].room_id)
        }
        ChatShortcut::Unread => {
            // 与 Socket.IO 客户端一致按 5→1 查找；同级按当前列表顺序，绝不跨出空筛选。
            (1..=5).rev().find_map(|priority| {
                visible_indices
                    .iter()
                    .map(|&index| &rooms[index])
                    .find(|room| room.unread_count > 0 && room.priority == priority)
                    .map(|room| room.room_id)
            })
        }
        ChatShortcut::Group(_) => None,
    }
}

impl IcaApp {
    fn chat_navigation_has_overlay(&self) -> bool {
        let page = &self.open_page;
        let page_open = [
            page.verify_message,
            page.about,
            page.settings,
            page.noticer_settings,
            page.notify_level,
            page.custom_chat_ica,
            page.custom_chat_extra,
            page.online_status,
            page.socketio_status,
            page.raw_config,
            page.chat_group_editor,
            page.contacts,
            page.group_tools,
            page.account_tools,
            page.file_tools,
            page.message_tools,
            page.room_tools,
            page.auto_sign,
            page.relation_network,
        ]
        .into_iter()
        .any(|open| open);
        page_open
            || self.image_viewer.is_some()
            || self.show_mention_picker
            || self.show_face_picker
            || self.group_file_panel.open
            || self.group_member_panel.confirmation.is_some()
            || self.active_bridge_state().is_some_and(|state| {
                state.forward_target_picker_open
                    || state.message_search.open
                    || state.member_history.open
                    || state
                        .forward_viewer
                        .lock()
                        .map(|viewer| viewer.open)
                        .unwrap_or(true)
                    || state
                        .group_announcement_viewer
                        .lock()
                        .map(|viewer| viewer.open)
                        .unwrap_or(true)
            })
    }

    /// 必须在本帧控件绘制之前调用，保证消费 Tab 时可同时取消 egui 的焦点移动。
    /// 对独立聊天、设置、图片等非 ROOT viewport 是无副作用的空操作。
    pub fn handle_chat_navigation_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != ViewportId::ROOT
            || !ctx
                .input(|input| input.focused && input.viewport().focused.unwrap_or(input.focused))
            || self.ime_composing
            || self.ime_event_this_frame
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|memory| memory.top_modal_layer().is_some())
            || self.chat_navigation_has_overlay()
        {
            return;
        }
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            return;
        };
        let selected = state.selected_room_id;
        let composer_id =
            selected.map(|room| egui::Id::new(("message_composer", bridge_idx, room)));
        let focused = ctx.memory(|memory| memory.focused());
        let list_row_focused = focused.is_some_and(|focused| {
            state
                .rooms
                .iter()
                .any(|room| focused == egui::Id::new(("chat_list_row", bridge_idx, room.room_id)))
        });
        if focused.is_some() && focused != composer_id && !list_row_focused {
            return;
        }
        let Some((event_index, shortcut)) =
            ctx.input(|input| {
                input.events.iter().enumerate().find_map(|(index, event)| {
                    chat_shortcut(event).map(|shortcut| (index, shortcut))
                })
            })
        else {
            return;
        };
        if shortcut == ChatShortcut::Unread
            && (focused.is_some()
                || selected.is_none()
                || self.active_chat_is_detached()
                || self.group_member_panel.open
                || (self.uses_compact_chat_layout(ctx)
                    && self.compact_chat_panel != CompactChatPanel::Chat))
        {
            return;
        }
        match &shortcut {
            ChatShortcut::Group(group) => {
                if !self.select_active_chat_group(group.clone()) {
                    return;
                }
            }
            _ => {
                let visible = self.visible_room_indices(bridge_idx);
                let Some(room_id) = shortcut_room(
                    &shortcut,
                    &self.bridge_states[bridge_idx].rooms,
                    &visible,
                    selected,
                ) else {
                    return;
                };
                self.select_active_room_preserving_filter(room_id);
                if focused.is_some() && focused == composer_id && !self.active_chat_is_detached() {
                    ctx.memory_mut(|memory| {
                        memory.request_focus(egui::Id::new((
                            "message_composer",
                            bridge_idx,
                            room_id,
                        )))
                    });
                }
            }
        }
        // 只消费确实执行的按下事件，不误吞未匹配的修饰键、重复事件或普通控件 Tab。
        ctx.input_mut(|input| {
            input.events.remove(event_index);
        });
        if shortcut == ChatShortcut::Unread {
            ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        AppState, BridgeSession, BridgeState, runtime::AppRuntime, stickers::StickerStore,
    };
    use crate::config::{ConfigStore, IcaCfg, chat_groups::ChatGroup};
    use crate::ica::{
        BridgeHandle, IcaCommand,
        types::message::{At, LastMessage},
    };
    use egui::{Id, Modifiers, RawInput};
    use tokio::sync::{mpsc, oneshot};

    fn room(id: RoomId, name: &str, pinned: bool, time: i64, priority: u8, unread: u64) -> Room {
        Room {
            room_id: id,
            room_name: name.into(),
            index: i64::from(pinned),
            utime: time,
            priority,
            unread_count: unread,
            users: serde_json::Value::Null,
            at: At::None,
            last_message: LastMessage {
                content: None,
                timestamp: None,
                username: None,
                user_id: None,
            },
        }
    }

    fn rooms() -> Vec<Room> {
        vec![
            room(-10, "Rust alpha", false, 50, 5, 1),
            room(20, "Rust private", true, 60, 4, 2),
            room(-30, "Rust beta", true, 20, 1, 0),
            room(-40, "Other group", false, 90, 5, 3),
            room(-50, "Rust gamma", true, 20, 3, 1),
        ]
    }

    fn ids(rooms: &[Room], indices: &[usize]) -> Vec<RoomId> {
        indices.iter().map(|&index| rooms[index].room_id).collect()
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn input(event: Event, viewport: ViewportId) -> RawInput {
        let mut raw = RawInput {
            viewport_id: viewport,
            events: vec![event],
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            ..Default::default()
        };
        raw.viewports.entry(viewport).or_default().focused = Some(true);
        raw
    }

    fn run_ui(ctx: &egui::Context, input: RawInput, run: impl FnMut(&mut egui::Ui)) {
        let mut output = ctx.run_ui(input, run);
        // 无渲染器的焦点测试不上传纹理，显式释放 egui 的待处理纹理增量。
        output.textures_delta.clear();
    }

    fn test_app() -> (IcaApp, Vec<mpsc::UnboundedReceiver<IcaCommand>>) {
        let config: IcaCfg = toml::from_str("bridges = []").unwrap();
        let store = ConfigStore::from_config(
            config.clone(),
            std::env::temp_dir().join("ica-chat-shortcuts-test.toml"),
        );
        let mut receivers = Vec::new();
        let sessions = ["shortcut-a", "shortcut-b"]
            .into_iter()
            .map(|name| {
                let (tx, rx) = mpsc::unbounded_channel();
                receivers.push(rx);
                let (stop, _) = oneshot::channel();
                let mut state = BridgeState::new(name.into(), ChatGroups::default());
                state.rooms = rooms();
                state.selected_room_id = Some(-30);
                BridgeSession::new(BridgeHandle::new(name.into(), tx), state, stop)
            })
            .collect();
        let state = AppState::new(
            &config,
            &store,
            sessions,
            StickerStore::unavailable(std::env::temp_dir().join("ica-shortcuts-stickers"), "测试"),
        );
        let mut app = IcaApp {
            runtime: AppRuntime::new(&egui::Context::default(), &config),
            config: store,
            state,
            chat_windows: Vec::new(),
        };
        app.custom_chat.disable_chat_group = false;
        app.custom_chat.hide_chat_group_sidebar = false;
        (app, receivers)
    }

    /// 返回事件是否仍留给普通控件，以及处理后的焦点。
    fn dispatch(app: &mut IcaApp, raw: RawInput, focus: Option<Id>) -> (bool, Option<Id>) {
        let ctx = egui::Context::default();
        let mut result = (false, None);
        run_ui(&ctx, raw, |ui| {
            if let Some(focus) = focus {
                ui.memory_mut(|memory| memory.request_focus(focus));
            }
            app.handle_chat_navigation_shortcuts(ui.ctx());
            result = (
                ui.input(|input| !input.events.is_empty()),
                ui.memory(|memory| memory.focused()),
            );
        });
        result
    }

    #[test]
    fn filtering_and_stable_pinned_sort_are_shared_with_room_navigation() {
        let rooms = rooms();
        let visible = filtered_sorted_room_indices(
            &rooms,
            &ChatGroups::default(),
            &SelectedChatGroup::Group,
            false,
            " rUsT ",
        );
        assert_eq!(ids(&rooms, &visible), [-30, -50, -10]);
        assert_eq!(
            shortcut_room(&ChatShortcut::Next, &rooms, &visible, Some(-30)),
            Some(-50)
        );
        let by_id = filtered_sorted_room_indices(
            &rooms,
            &ChatGroups::default(),
            &SelectedChatGroup::All,
            false,
            "40",
        );
        assert_eq!(ids(&rooms, &by_id), [-40]);
        let private = filtered_sorted_room_indices(
            &rooms,
            &ChatGroups::default(),
            &SelectedChatGroup::Private,
            false,
            "",
        );
        assert_eq!(ids(&rooms, &private), [20]);
    }

    #[test]
    fn custom_groups_include_all_personal_but_still_obey_search() {
        let rooms = rooms();
        let groups = ChatGroups {
            groups: vec![ChatGroup {
                name: "分类".into(),
                rooms: vec![-10, -40],
                include_all_personal: true,
            }],
        };
        let visible = filtered_sorted_room_indices(
            &rooms,
            &groups,
            &SelectedChatGroup::Custom(0),
            false,
            "rust",
        );
        assert_eq!(ids(&rooms, &visible), [20, -10]);
        assert!(
            filtered_sorted_room_indices(&rooms, &groups, &SelectedChatGroup::Custom(9), false, "")
                .is_empty()
        );
        let disabled = filtered_sorted_room_indices(
            &rooms,
            &groups,
            &SelectedChatGroup::Custom(9),
            true,
            "private",
        );
        assert_eq!(ids(&rooms, &disabled), [20]);
    }

    #[test]
    fn cycling_wraps_and_missing_selection_starts_at_first_visible_room() {
        let rooms = rooms();
        let visible = [2, 4, 0];
        assert_eq!(
            shortcut_room(&ChatShortcut::Next, &rooms, &visible, Some(-10)),
            Some(-30)
        );
        assert_eq!(
            shortcut_room(&ChatShortcut::Previous, &rooms, &visible, Some(-30)),
            Some(-10)
        );
        for shortcut in [ChatShortcut::Next, ChatShortcut::Previous] {
            for selected in [None, Some(20), Some(-999)] {
                assert_eq!(
                    shortcut_room(&shortcut, &rooms, &visible, selected),
                    Some(-30)
                );
            }
            assert_eq!(shortcut_room(&shortcut, &rooms, &[0], Some(-10)), Some(-10));
            assert_eq!(shortcut_room(&shortcut, &rooms, &[], Some(-30)), None);
        }
    }

    #[test]
    fn unread_priority_beats_pinning_and_ties_follow_visible_order() {
        let rooms = rooms();
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[1, 4, 0, 3], None),
            Some(-10)
        );
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[3, 0, 1], None),
            Some(-40)
        );
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[2, 4, 1], None),
            Some(20)
        );
    }

    #[test]
    fn unread_does_not_escape_filter_or_accept_mentions_without_unread_count() {
        let mut rooms = rooms();
        rooms[2].at = At::All;
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[2], None),
            None
        );
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[], None),
            None
        );
        assert_eq!(shortcut_room(&ChatShortcut::Unread, &[], &[], None), None);
        rooms[0].priority = 6;
        rooms[4].priority = 0;
        assert_eq!(
            shortcut_room(&ChatShortcut::Unread, &rooms, &[0, 4, 1], None),
            Some(20)
        );
    }

    #[test]
    fn declared_keys_match_exact_modifiers_and_ignore_repeat_or_release() {
        for (event, expected) in [
            (key(Key::Tab, Modifiers::CTRL), ChatShortcut::Next),
            (
                key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT),
                ChatShortcut::Previous,
            ),
            (key(Key::ArrowUp, Modifiers::ALT), ChatShortcut::Previous),
            (key(Key::ArrowDown, Modifiers::ALT), ChatShortcut::Next),
            (key(Key::Tab, Modifiers::NONE), ChatShortcut::Unread),
        ] {
            assert_eq!(chat_shortcut(&event), Some(expected));
        }
        for event in [
            key(Key::Tab, Modifiers::SHIFT),
            key(Key::Tab, Modifiers::ALT),
            key(Key::Tab, Modifiers::CTRL | Modifiers::ALT),
            key(Key::ArrowUp, Modifiers::ALT | Modifiers::SHIFT),
            key(Key::Num1, Modifiers::CTRL | Modifiers::SHIFT),
            key(Key::Tab, Modifiers::MAC_CMD),
        ] {
            assert_eq!(chat_shortcut(&event), None);
        }
        let mut repeated = key(Key::Tab, Modifiers::CTRL);
        if let Event::Key { repeat, .. } = &mut repeated {
            *repeat = true;
        }
        assert_eq!(chat_shortcut(&repeated), None);
        let mut released = key(Key::Tab, Modifiers::CTRL);
        if let Event::Key { pressed, .. } = &mut released {
            *pressed = false;
        }
        assert_eq!(chat_shortcut(&released), None);
    }

    #[test]
    fn control_digits_map_to_builtin_and_first_six_custom_categories() {
        for (key_code, group) in [
            (Key::Num1, SelectedChatGroup::All),
            (Key::Num2, SelectedChatGroup::Group),
            (Key::Num3, SelectedChatGroup::Private),
            (Key::Num4, SelectedChatGroup::Custom(0)),
            (Key::Num5, SelectedChatGroup::Custom(1)),
            (Key::Num6, SelectedChatGroup::Custom(2)),
            (Key::Num7, SelectedChatGroup::Custom(3)),
            (Key::Num8, SelectedChatGroup::Custom(4)),
            (Key::Num9, SelectedChatGroup::Custom(5)),
        ] {
            assert_eq!(
                chat_shortcut(&key(key_code, Modifiers::CTRL)),
                Some(ChatShortcut::Group(group))
            );
        }
        assert_eq!(chat_shortcut(&key(Key::Num0, Modifiers::CTRL)), None);
    }

    #[test]
    fn navigation_preserves_search_and_uses_existing_selection_side_effects() {
        let (mut app, mut receivers) = test_app();
        app.clear_search_on_room_select = true;
        app.custom_chat.auto_read_on_select = true;
        app.bridge_states[0].room_search_query = "rust".into();
        app.bridge_states[0].selected_chat_group = SelectedChatGroup::Group;
        let composer = Id::new(("message_composer", 0_usize, -30_i64));
        let (unhandled, focused) = dispatch(
            &mut app,
            input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT),
            Some(composer),
        );
        assert!(!unhandled);
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-50));
        assert_eq!(
            focused,
            Some(Id::new(("message_composer", 0_usize, -50_i64)))
        );
        assert_eq!(app.bridge_states[0].room_search_query, "rust");
        assert_eq!(app.bridge_states[0].rooms[4].unread_count, 0);
        let mut cleared = false;
        let mut fetched = false;
        while let Ok(command) = receivers[0].try_recv() {
            cleared |= matches!(command, IcaCommand::ClearRoomUnread { room_id: -50 });
            fetched |= matches!(command, IcaCommand::FetchMessages(-50));
        }
        assert!(cleared && fetched);
        for expected in [-10, -30] {
            assert!(
                !dispatch(
                    &mut app,
                    input(key(Key::ArrowDown, Modifiers::ALT), ViewportId::ROOT),
                    None
                )
                .0
            );
            assert_eq!(app.bridge_states[0].selected_room_id, Some(expected));
        }
        assert_eq!(app.bridge_states[0].room_search_query, "rust");
        assert!(app.clear_search_on_room_select);
        assert!(receivers[1].try_recv().is_err());
    }

    #[test]
    fn category_selection_validates_bounds_without_changing_room_or_other_bridge() {
        let (mut app, _receivers) = test_app();
        app.bridge_states[0]
            .chat_groups
            .groups
            .push(ChatGroup::new("分类", vec![-10]));
        assert!(
            !dispatch(
                &mut app,
                input(key(Key::Num4, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
        assert_eq!(
            app.bridge_states[0].selected_chat_group,
            SelectedChatGroup::Custom(0)
        );
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-30));
        assert_eq!(app.visible_room_indices(0), [0]);
        assert_eq!(
            app.bridge_states[1].selected_chat_group,
            SelectedChatGroup::All
        );
        assert!(
            dispatch(
                &mut app,
                input(key(Key::Num9, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
        assert_eq!(
            app.bridge_states[0].selected_chat_group,
            SelectedChatGroup::Custom(0)
        );
        app.custom_chat.disable_chat_group = true;
        assert!(
            dispatch(
                &mut app,
                input(key(Key::Num2, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
        app.custom_chat.disable_chat_group = false;
        app.custom_chat.hide_chat_group_sidebar = true;
        assert!(
            dispatch(
                &mut app,
                input(key(Key::Num3, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
    }

    #[test]
    fn detached_and_unrelated_viewports_leave_room_category_and_events_untouched() {
        let (mut app, mut receivers) = test_app();
        app.bridge_states[0].detached_room_ids.insert(-30);
        for viewport in [
            ViewportId::from_hash_of(("chat_window", "shortcut-a", -30_i64)),
            ViewportId::from_hash_of("settings"),
            ViewportId::from_hash_of("image_viewer"),
        ] {
            for event in [
                key(Key::Tab, Modifiers::CTRL),
                key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT),
                key(Key::ArrowUp, Modifiers::ALT),
                key(Key::ArrowDown, Modifiers::ALT),
                key(Key::Tab, Modifiers::NONE),
                key(Key::Num2, Modifiers::CTRL),
                key(Key::Num4, Modifiers::CTRL),
            ] {
                assert!(dispatch(&mut app, input(event, viewport), None).0);
                assert_eq!(app.active_bridge_idx, Some(0));
                for state in &app.bridge_states {
                    assert_eq!(state.selected_room_id, Some(-30));
                    assert_eq!(state.selected_chat_group, SelectedChatGroup::All);
                }
                assert!(app.bridge_states[0].detached_room_ids.contains(&-30));
            }
        }
        assert!(
            receivers
                .iter_mut()
                .all(|receiver| receiver.try_recv().is_err())
        );
    }

    #[test]
    fn plain_tab_preserves_editor_button_and_search_focus_navigation() {
        let (mut app, _receivers) = test_app();
        for focus in [
            Id::new(("message_composer", 0_usize, -30_i64)),
            Id::new("普通按钮"),
            Id::new("成员搜索"),
            Id::new("会话搜索"),
            Id::new(("chat_list_row", 0_usize, -30_i64)),
        ] {
            assert!(
                dispatch(
                    &mut app,
                    input(key(Key::Tab, Modifiers::NONE), ViewportId::ROOT),
                    Some(focus)
                )
                .0
            );
            assert_eq!(app.bridge_states[0].selected_room_id, Some(-30));
        }
        for focus in [Id::new("成员搜索"), Id::new("普通按钮")] {
            for event in [
                key(Key::Tab, Modifiers::CTRL),
                key(Key::ArrowDown, Modifiers::ALT),
                key(Key::Num3, Modifiers::CTRL),
            ] {
                assert!(dispatch(&mut app, input(event, ViewportId::ROOT), Some(focus)).0);
                assert_eq!(app.bridge_states[0].selected_room_id, Some(-30));
                assert_eq!(
                    app.bridge_states[0].selected_chat_group,
                    SelectedChatGroup::All
                );
            }
        }
        let row = Id::new(("chat_list_row", 0_usize, -30_i64));
        assert!(
            !dispatch(
                &mut app,
                input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT),
                Some(row)
            )
            .0
        );
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-50));
    }

    #[test]
    fn handled_plain_tab_cancels_egui_focus_movement_but_no_unread_keeps_it() {
        let (mut app, _receivers) = test_app();
        let ctx = egui::Context::default();
        run_ui(
            &ctx,
            input(key(Key::Tab, Modifiers::NONE), ViewportId::ROOT),
            |ui| {
                app.handle_chat_navigation_shortcuts(ui.ctx());
                assert!(ui.input(|input| input.events.is_empty()));
                let button = ui.button("不应抢焦点");
                assert!(!button.has_focus());
            },
        );
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-40));
        for room in &mut app.bridge_states[0].rooms {
            room.unread_count = 0;
        }
        let ctx = egui::Context::default();
        run_ui(
            &ctx,
            input(key(Key::Tab, Modifiers::NONE), ViewportId::ROOT),
            |ui| {
                app.handle_chat_navigation_shortcuts(ui.ctx());
                assert!(ui.input(|input| !input.events.is_empty()));
                assert!(ui.button("正常 Tab 导航").has_focus());
            },
        );
    }

    #[test]
    fn overlays_ime_and_unfocused_window_do_not_consume_navigation() {
        let (mut app, _receivers) = test_app();
        for blocked in 0..7 {
            app.open_page.settings = blocked == 0;
            app.show_mention_picker = blocked == 1;
            app.ime_composing = blocked == 2;
            app.ime_event_this_frame = blocked == 3;
            app.bridge_states[0].message_search.open = blocked == 4;
            app.bridge_states[0].member_history.open = blocked == 5;
            app.image_viewer = (blocked == 6).then(|| {
                std::sync::Arc::new(std::sync::Mutex::new(crate::app::ImageViewerState::new(
                    "测试图片".into(),
                )))
            });
            assert!(
                dispatch(
                    &mut app,
                    input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT),
                    None
                )
                .0
            );
            assert_eq!(app.bridge_states[0].selected_room_id, Some(-30));
        }
        app.image_viewer = None;
        let mut raw = input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT);
        raw.focused = false;
        assert!(dispatch(&mut app, raw, None).0);
        let mut raw = input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT);
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().focused = Some(false);
        assert!(dispatch(&mut app, raw, None).0);
        let ctx = egui::Context::default();
        run_ui(
            &ctx,
            input(key(Key::Num2, Modifiers::CTRL), ViewportId::ROOT),
            |ui| {
                egui::Popup::open_id(ui.ctx(), Id::new("测试菜单"));
                app.handle_chat_navigation_shortcuts(ui.ctx());
                assert!(ui.input(|input| !input.events.is_empty()));
                assert_eq!(
                    app.bridge_states[0].selected_chat_group,
                    SelectedChatGroup::All
                );
            },
        );
    }

    #[test]
    fn empty_filter_or_missing_bridge_is_a_noop_without_swallowing_tab() {
        let (mut app, _receivers) = test_app();
        app.bridge_states[0].room_search_query = "没有匹配".into();
        for event in [
            key(Key::Tab, Modifiers::CTRL),
            key(Key::Tab, Modifiers::NONE),
            key(Key::ArrowUp, Modifiers::ALT),
        ] {
            assert!(dispatch(&mut app, input(event, ViewportId::ROOT), None).0);
            assert_eq!(app.bridge_states[0].selected_room_id, Some(-30));
        }
        app.active_bridge_idx = None;
        assert!(
            dispatch(
                &mut app,
                input(key(Key::Num2, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
        app.active_bridge_idx = Some(99);
        assert!(
            dispatch(
                &mut app,
                input(key(Key::Tab, Modifiers::CTRL), ViewportId::ROOT),
                None
            )
            .0
        );
    }
}
