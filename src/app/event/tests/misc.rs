use super::{apply, parse_database_upgrade_progress};
use crate::{app::state::BridgeState, config::ChatGroups};
use serde_json::json;

#[test]
fn database_upgrade_progress_reads_socketio_argument_wrapper() {
    let progress = parse_database_upgrade_progress(&json!([{
        "active": true,
        "message": "正在建立消息搜索索引...",
        "step": 3_887_893,
        "total": 66_011_987,
    }]))
    .unwrap();

    assert!(progress.active);
    assert_eq!(progress.message, "正在建立消息搜索索引...");
    assert_eq!(progress.step, 3_887_893);
    assert_eq!(progress.total, 66_011_987);
    assert!(progress.ratio() > 0.05 && progress.ratio() < 0.06);
}

#[test]
fn database_upgrade_progress_rejects_non_object_argument() {
    let error = parse_database_upgrade_progress(&json!(["正在建立索引"])).unwrap_err();

    assert!(error.contains("invalid type"));
}

#[test]
fn resource_ack_keeps_download_credentials_out_of_status_and_other_tools() {
    let mut state = BridgeState::new("test".into(), ChatGroups::default());
    let response = json!({
        "event": "favoritesRequest",
        "ack": [{"ok": true, "data": {"url": "https://example.invalid/download?ticket=fake"}}],
    });
    assert!(apply(&mut state, "socketApiResponse", &response));
    assert!(
        state
            .last_resource_api_response
            .as_deref()
            .unwrap()
            .contains("ticket=fake")
    );
    assert!(state.last_socket_api_response.is_none());
    let notice = state.last_notice.as_deref().unwrap();
    assert!(notice.contains("成功"));
    assert!(!notice.contains("ticket="));
}
