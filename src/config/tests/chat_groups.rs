use super::ChatGroup;
use serde_json::json;

#[test]
fn deserializes_protocol_include_all_personal() {
    let group: ChatGroup = serde_json::from_value(json!({
        "name": "全部私聊",
        "index": 0,
        "rooms": [],
        "includeAllPersonal": true,
    }))
    .expect("协议聊天分组应当可以反序列化");

    assert!(group.include_all_personal);
}

#[test]
fn deserializes_legacy_snake_case_include_all_personal() {
    let group: ChatGroup = serde_json::from_value(json!({
        "name": "全部私聊",
        "rooms": [],
        "include_all_personal": true,
    }))
    .expect("旧版本地聊天分组应当可以反序列化");

    assert!(group.include_all_personal);
}
