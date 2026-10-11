use super::{BridgeEvent, BridgeEventKind};
use serde_json::json;

#[test]
fn known_wire_event_becomes_typed_variant() {
    let event = BridgeEvent::from_wire_value(json!({
        "bridge": "primary",
        "event": "setAllRooms",
        "payload": [[{"roomId": 1}]],
    }))
    .unwrap();
    assert_eq!(event.bridge_key, "primary");
    assert!(matches!(event.kind, BridgeEventKind::SetAllRooms(_)));
}

#[test]
fn unknown_wire_event_keeps_name_and_payload() {
    let payload = json!({"future": true});
    let event = BridgeEvent::from_protocol("primary", "futureEvent", payload.clone());
    assert!(matches!(
        event.kind,
        BridgeEventKind::Unknown { ref name, payload: ref kept }
            if name == "futureEvent" && kept == &payload
    ));
}
