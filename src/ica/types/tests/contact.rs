use super::{FriendContact, GroupContact};
use serde_json::json;

#[test]
fn friend_contract_accepts_oicq_fallback_and_duplicate_field_names() {
    let fallback: FriendContact = serde_json::from_value(json!({
        "uin": 10001,
        "nick": "Alice",
        "remark": "同事"
    }))
    .unwrap();
    let oicq: FriendContact = serde_json::from_value(json!({
        "user_id": "10002",
        "nickname": "Bob",
        "remark": null
    }))
    .unwrap();
    let bridge: FriendContact = serde_json::from_value(json!({
        "uin": 10003,
        "user_id": "99999",
        "nick": "Carol",
        "nickname": "旧昵称",
        "remark": ""
    }))
    .unwrap();

    assert_eq!(fallback.display_name(), "同事");
    assert_eq!(oicq.uin, 10002);
    assert_eq!(oicq.display_name(), "Bob");
    assert_eq!(bridge.uin, 10003);
    assert_eq!(bridge.display_name(), "Carol");
    assert!(fallback.matches_query("ALICE"));
}

#[test]
fn group_contract_uses_negative_room_ids_and_remark_for_display() {
    let group: GroupContact = serde_json::from_value(json!({
        "group_id": "123456",
        "group_name": "开发群",
        "group_remark": "工作"
    }))
    .unwrap();

    assert_eq!(group.room_id(), -123456);
    assert_eq!(group.display_name(), "工作");
    assert_eq!(group.room_name(), "开发群");
    assert!(group.matches_query("1234"));
}
