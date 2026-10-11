use super::*;
use crate::config::ChatGroups;
use serde_json::json;

#[test]
fn detached_chat_follows_new_messages_and_counts_them_when_scrolled_up() {
    let mut state = BridgeState::new("test".into(), ChatGroups::default());
    state.selected_room_id = Some(-2);
    state.detached_room_ids.insert(-1);
    state.conversation_mut(-1).near_bottom = true;
    apply(
        &mut state,
        "addMessage",
        &json!([{
            "roomId": -1, "message": {"_id": "first", "senderId": 42, "username": "tester", "content": "test"}
        }]),
    );
    assert!(state.conversation(-1).unwrap().scroll_to_bottom);
    let chat = state.conversation_mut(-1);
    chat.near_bottom = false;
    chat.scroll_to_bottom = false;
    apply(
        &mut state,
        "addMessage",
        &json!([{
            "roomId": -1, "message": {"_id": "second", "senderId": 42, "username": "tester", "content": "test"}
        }]),
    );
    let chat = state.conversation(-1).unwrap();
    assert!(!chat.scroll_to_bottom);
    assert_eq!(chat.new_message_count, 1);
    assert_eq!(state.selected_room_id, Some(-2));
}
