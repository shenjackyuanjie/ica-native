use super::*;
use crate::app::{BridgeState, test_support::app_with_bridges};
use crate::config::{IcaCfg, chat_groups::ChatGroup};
use crate::ica::{
    IcaCommand,
    types::message::{At, LastMessage},
};
use egui::{Id, Modifiers, RawInput};
use tokio::sync::mpsc;

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
    let bridges = ["shortcut-a", "shortcut-b"]
        .into_iter()
        .map(|name| {
            let mut state = BridgeState::new(name.into(), ChatGroups::default());
            state.rooms = rooms();
            state.selected_room_id = Some(-30);
            state
        })
        .collect();
    let (mut app, receivers) = app_with_bridges(IcaCfg::default(), bridges);
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
