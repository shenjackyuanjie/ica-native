use super::contact_ack_items;
use rust_socketio::Payload;
use serde_json::json;

#[test]
fn nested_ack_list_is_unwrapped_for_contact_deserialization() {
    let payload = Payload::Text(vec![json!([[{
        "uin": "10001",
        "nick": "Alice",
        "remark": ""
    }]])]);

    assert_eq!(
        contact_ack_items(&payload),
        json!([{
            "uin": "10001",
            "nick": "Alice",
            "remark": ""
        }])
    );
}
