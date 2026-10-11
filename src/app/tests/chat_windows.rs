use super::*;
use crate::app::{BridgeState, test_support::app_with_bridges};
use crate::config::{ChatGroups, IcaCfg};
use crate::ica::IcaCommand;
use tokio::sync::mpsc;

fn test_app() -> (IcaApp, Vec<mpsc::UnboundedReceiver<IcaCommand>>) {
    let bridges = ["test-a", "test-b"]
        .into_iter()
        .map(|key| BridgeState::new(key.into(), ChatGroups::default()))
        .collect();
    app_with_bridges(IcaCfg::default(), bridges)
}

#[test]
fn opening_windows_deduplicates_per_bridge_and_preserves_main_selection() {
    let (mut app, _receivers) = test_app();
    app.bridge_states[0].selected_room_id = Some(-2);
    let ctx = egui::Context::default();
    app.open_chat_window(&ctx, 0, -1);
    app.open_chat_window(&ctx, 0, -1);
    assert_eq!(app.chat_windows.len(), 1);
    app.open_chat_window(&ctx, 1, -1);
    assert_eq!(app.chat_windows.len(), 2);
    assert_eq!(app.active_bridge_idx, Some(0));
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-2));
    assert_eq!(app.bridge_states[1].selected_room_id, None);
}

#[test]
fn detached_send_uses_its_own_bridge_and_room_and_restores_main_input() {
    let (mut app, mut receivers) = test_app();
    app.bridge_states[0].selected_room_id = Some(-2);
    app.bridge_states[0].conversation_mut(-2).draft = "main draft".into();
    app.bridge_states[1].selected_room_id = Some(-3);
    app.bridge_states[1].conversation_mut(-1).draft = "child draft".into();
    app.mention_search_query = "main search".into();
    let mut window = ChatWindow {
        bridge_key: "test-b".into(),
        room_id: -1,
        ui: ChatWindowUiState::default(),
    };
    app.with_chat_window(1, &mut window, |app| {
        app.mention_search_query = "child search".into();
        app.send_current_message();
    });
    assert!(receivers[0].try_recv().is_err());
    let IcaCommand::SendMessage(message) = receivers[1].try_recv().unwrap() else {
        panic!("expected send message")
    };
    assert_eq!(message.room_id, -1);
    assert_eq!(message.content, "child draft");
    assert_eq!(app.active_bridge_idx, Some(0));
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-2));
    assert_eq!(app.bridge_states[1].selected_room_id, Some(-3));
    assert_eq!(
        app.bridge_states[0].conversation(-2).unwrap().draft,
        "main draft"
    );
    assert_eq!(app.mention_search_query, "main search");
    assert_eq!(window.ui.mention_search_query, "child search");
}
