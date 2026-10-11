use super::{latest_history_message_id, normalize_ack_list};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

#[test]
fn ack_list_accepts_flat_and_nested_socket_payloads() {
    assert_eq!(
        normalize_ack_list(vec![json!({"id": 1})]),
        json!([{"id": 1}])
    );
    assert_eq!(
        normalize_ack_list(vec![json!([{"id": 1}, {"id": 2}])]),
        json!([{"id": 1}, {"id": 2}])
    );
}

#[test]
fn latest_history_id_uses_legacy_private_and_group_shapes() {
    let private = STANDARD
        .decode(latest_history_message_id(0x0102_0304).unwrap())
        .unwrap();
    let group = STANDARD
        .decode(latest_history_message_id(-0x0102_0304).unwrap())
        .unwrap();

    assert_eq!(private.len(), 17);
    assert_eq!(group.len(), 21);
    assert_eq!(&private[..4], &[1, 2, 3, 4]);
    assert_eq!(&group[..4], &[1, 2, 3, 4]);
    assert!(private[4..].iter().all(|byte| *byte == 0));
    assert!(group[4..].iter().all(|byte| *byte == 0));
}
