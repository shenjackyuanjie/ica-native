use super::{clear_room_unread, should_request_messages_on_room_select};
use crate::app::{BridgeState, IcaApp, test_support::app_with_bridges};
use crate::config::{ChatGroups, IcaCfg};
use crate::ica::types::{
    message::{At, LastMessage},
    room::Room,
};
use crate::ica::{BridgeEventKind, IcaCommand};
use serde_json::json;

fn room(room_id: i64, unread_count: u64, at: At) -> Room {
    Room {
        room_id,
        room_name: format!("会话 {room_id}"),
        index: 0,
        unread_count,
        priority: 1,
        utime: 0,
        users: serde_json::Value::Null,
        at,
        last_message: LastMessage {
            content: None,
            timestamp: None,
            username: None,
            user_id: None,
        },
    }
}

#[test]
fn selecting_room_clears_only_its_unread_and_mention_state() {
    let mut rooms = vec![room(-100, 3, At::All), room(200, 2, At::Bool(true))];

    assert!(clear_room_unread(&mut rooms, -100));
    assert_eq!(rooms[0].unread_count, 0);
    assert_eq!(rooms[0].at, At::Bool(false));
    assert_eq!(rooms[1].unread_count, 2);
    assert_eq!(rooms[1].at, At::Bool(true));
}

#[test]
fn first_room_open_always_requests_complete_snapshot() {
    assert!(should_request_messages_on_room_select(false, true, false));
}

#[test]
fn loaded_room_only_refreshes_after_switch_when_option_is_enabled() {
    assert!(!should_request_messages_on_room_select(true, true, false));
    assert!(should_request_messages_on_room_select(true, true, true));
    assert!(!should_request_messages_on_room_select(true, false, true));
}

#[test]
fn opening_unread_survives_auto_read_live_messages_and_paginated_history() {
    const ROOM: i64 = -100;
    // 正常 Bridge 按“秒 * 1000 + 同秒偏移”写 room.utime 和 message.time。
    const TAIL_TIME: i64 = 1_760_000_003_037;
    let bridges = ["a", "b"]
        .into_iter()
        .map(|key| {
            let mut bridge = BridgeState::new(key.into(), ChatGroups::default());
            bridge.selected_room_id = Some(-200);
            let mut chat = room(ROOM, 2, At::Bool(false));
            chat.utime = TAIL_TIME;
            bridge.rooms.push(chat);
            bridge
        })
        .collect();
    let (mut app, mut receivers) = app_with_bridges(IcaCfg::default(), bridges);
    app.custom_chat.auto_read_on_select = true;
    app.auto_fetch_history_on_room_select = false;
    let message = |id, time, system| {
        json!({
            "_id": id, "senderId": 42, "username": "甲", "content": id, "time": time, "system": system
        })
    };
    let apply = |app: &mut IcaApp, event: &str, data| {
        IcaApp::apply_bridge_event(
            &mut app.bridge_states[0],
            &BridgeEventKind::from_protocol(event, json!([data])),
        );
    };
    let newer = message("较新", TAIL_TIME + 1, false);
    // 实时预览先于 updateRoom，预览时间和当时的未读数不属于同一条消息。
    apply(
        &mut app,
        "addMessage",
        json!({"roomId": ROOM, "message": newer}),
    );
    app.open_chat_window(&egui::Context::default(), 0, ROOM);
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-200));
    assert_eq!(app.bridge_states[0].rooms[0].unread_count, 0);
    assert_eq!(app.bridge_states[1].rooms[0].unread_count, 2);
    assert!(app.bridge_states[1].conversation(ROOM).is_none());
    assert!(matches!(
        receivers[0].try_recv().unwrap(),
        IcaCommand::ClearRoomUnread { room_id: ROOM }
    ));
    assert!(matches!(
        receivers[0].try_recv().unwrap(),
        IcaCommand::FetchMessages(ROOM)
    ));
    assert!(matches!(
        receivers[0].try_recv().unwrap(),
        IcaCommand::ReportRead { room_id: ROOM, .. }
    ));

    let same_time_live = message("开房后同时间", TAIL_TIME, false);
    apply(
        &mut app,
        "addMessage",
        json!({"roomId": ROOM, "message": same_time_live}),
    );
    assert_eq!(
        app.bridge_states[0]
            .conversation(ROOM)
            .unwrap()
            .new_message_count,
        1
    );
    let tail = message("开房时末条", TAIL_TIME, false);
    apply(
        &mut app,
        "setMessages",
        json!({"roomId": ROOM, "messages": [tail, same_time_live, newer]}),
    );
    let unread = app.bridge_states[0]
        .conversation_mut(ROOM)
        .opening_unread
        .as_mut()
        .unwrap();
    assert_eq!(unread.anchor_id(), None);
    unread.jump_pages = Some(0);
    app.advance_opening_unread_jump(0, ROOM);
    assert!(matches!(
        receivers[0].try_recv().unwrap(),
        IcaCommand::FetchOlderMessages { room_id: ROOM, .. }
    ));

    let first = message("首条未读", TAIL_TIME - 2, false);
    let system = message("系统消息", TAIL_TIME - 1, true);
    apply(
        &mut app,
        "appendOlderMessages",
        json!({"roomId": ROOM, "messages": [first, system]}),
    );
    app.advance_opening_unread_jump(0, ROOM);
    let chat = app.bridge_states[0].conversation(ROOM).unwrap();
    assert_eq!(chat.scroll_to_message_id.as_deref(), Some("首条未读"));
    assert!(!chat.scroll_to_bottom);

    // 刷新时补齐一条开房前的消息，也不能重算已确定的锚点。
    let backfilled = message("后来补齐", TAIL_TIME - 1, false);
    apply(
        &mut app,
        "setMessages",
        json!({"roomId": ROOM, "messages": [first, system, backfilled, tail, same_time_live, newer]}),
    );
    app.select_active_room(ROOM);
    let chat = app.bridge_states[0].conversation(ROOM).unwrap();
    assert_eq!(
        chat.opening_unread.as_ref().unwrap().anchor_id(),
        Some("首条未读")
    );
    assert_eq!(chat.scroll_to_message_id.as_deref(), Some("首条未读"));
    assert!(matches!(
        receivers[0].try_recv().unwrap(),
        IcaCommand::ReportRead { room_id: ROOM, .. }
    ));
    assert!(receivers[0].try_recv().is_err());
    assert!(receivers[1].try_recv().is_err());
}
