use super::BridgeState;
use crate::config::ChatGroups;

#[test]
fn detached_chat_keeps_history_until_its_window_closes() {
    let mut state = BridgeState::new("test".into(), ChatGroups::default());
    state.detached_room_ids.insert(-1);
    for room_id in [-1, -2] {
        let conversation = state.conversation_mut(room_id);
        conversation.messages = (0..400)
            .map(|index| {
                serde_json::from_value(serde_json::json!({"_id": index.to_string(), "username": "tester", "content": "test"})).unwrap()
            })
            .collect();
        conversation.message_row_heights.insert("row".into(), 40.0);
    }
    state.trim_message_caches(Some(-3));
    assert_eq!(state.conversation(-1).unwrap().messages.len(), 400);
    assert_eq!(state.conversation(-1).unwrap().message_row_heights.len(), 1);
    assert_eq!(state.conversation(-2).unwrap().messages.len(), 300);
    assert!(
        state
            .conversation(-2)
            .unwrap()
            .message_row_heights
            .is_empty()
    );
    state.detached_room_ids.remove(&-1);
    state.trim_message_caches(Some(-3));
    assert_eq!(state.conversation(-1).unwrap().messages.len(), 300);
    assert!(
        state
            .conversation(-1)
            .unwrap()
            .message_row_heights
            .is_empty()
    );
}

#[test]
fn forward_targets_support_multiple_rooms_without_duplicates() {
    let mut state = BridgeState::new("test".to_string(), ChatGroups::default());
    state.replace_forward_selection(-100, "m1".to_string());

    state.add_forward_targets([10001, -200, 10001]);
    assert_eq!(state.forward_target_room_ids, vec![10001, -200]);

    state.set_forward_target_selected(10001, false);
    assert_eq!(state.forward_target_room_ids, vec![-200]);

    state.replace_forward_selection(-300, "m2".to_string());
    assert!(state.forward_target_room_ids.is_empty());
}
