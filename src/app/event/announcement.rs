use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::announcement::parse_announcement_list;

fn request_id(payload: &JsonValue) -> u64 {
    payload
        .get("requestId")
        .and_then(JsonValue::as_u64)
        .unwrap_or_default()
}

fn room_id(payload: &JsonValue) -> i64 {
    payload
        .get("roomId")
        .and_then(JsonValue::as_i64)
        .unwrap_or_default()
}

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub(super) fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "groupAnnouncementsResponse" => apply_response(state, payload),
        "groupAnnouncementsFailed" => apply_failure(state, payload),
        _ => return false,
    }
    true
}

/// 处理公告 CGI 的原始响应。
///
/// 传输层只保证拿到了 JSON，`ec` 语义在这里通过 `parse_announcement_list` 判定，
/// 因此接口层面的失败（未登录、无权限）与网络失败会走到同一个错误展示。
fn apply_response(state: &mut BridgeState, payload: &JsonValue) {
    let request_id = request_id(payload);
    let room_id = room_id(payload);
    let viewer = state.group_announcement_viewer.clone();
    let mut viewer = viewer.lock().unwrap();

    // 先留下整份响应：字段名或结构对不上时，界面上仍能整体复制出来排查。
    viewer.set_raw_response(
        request_id,
        room_id,
        serde_json::to_string_pretty(&payload["body"])
            .unwrap_or_else(|_| payload["body"].to_string()),
    );

    match parse_announcement_list(&payload["body"]) {
        Ok(announcements) => {
            let count = announcements.len();
            if viewer.apply_response(request_id, room_id, announcements) {
                tracing::debug!(
                    target: "ica_native::announcement",
                    bridge = %state.bridge_key,
                    request_id,
                    room_id,
                    count,
                    "群公告加载完成"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                target: "ica_native::announcement",
                bridge = %state.bridge_key,
                request_id,
                room_id,
                error = %error,
                "群公告接口返回失败"
            );
            viewer.fail(request_id, room_id, error);
        }
    }
}

fn apply_failure(state: &mut BridgeState, payload: &JsonValue) {
    let request_id = request_id(payload);
    let room_id = room_id(payload);
    let message = payload
        .get("message")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| "拉取群公告失败".to_string());
    let viewer = state.group_announcement_viewer.clone();
    viewer.lock().unwrap().fail(request_id, room_id, message);
}
