use super::parse_database_upgrade_progress;
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
