use super::Message;
use serde_json::json;

#[test]
fn message_time_accepts_bridge_millisecond_timestamps() {
    let message: Message = serde_json::from_value(json!({
        "_id": "m1",
        "senderId": 10001,
        "username": "Alice",
        "content": "hello",
        "time": 1_710_000_000_000_i64,
        "files": [],
    }))
    .unwrap();

    assert_eq!(message.time.timestamp(), 1_710_000_000);
}
