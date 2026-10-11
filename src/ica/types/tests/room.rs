use super::Room;
use crate::ica::types::message::At;
use serde_json::json;

#[test]
fn missing_optional_at_defaults_to_none() {
    let room: Room = serde_json::from_value(json!({
        "roomId": -123,
        "roomName": "测试会话",
        "index": 0,
        "unreadCount": 1,
        "priority": 2,
        "utime": 1_700_000_000_000_i64,
        "lastMessage": {
            "content": "普通消息",
            "timestamp": "12:00"
        }
    }))
    .expect("bridge 协议中的 Room.at 是可选字段");

    assert_eq!(room.at, At::None);
}
