use super::{clear_room_unread, should_request_messages_on_room_select};
use crate::ica::types::{
    message::{At, LastMessage},
    room::Room,
};

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
