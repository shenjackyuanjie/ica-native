//! 尚未归入具体领域的通用事件：加载提示、命令失败与调试用的原始响应。

use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub(super) fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "closeLoading" => {}
        "notifyError" => {
            if let Some(msg) = payload::first_payload_display_message(payload) {
                state.last_error = Some(msg);
            }
        }
        "commandFailed" => {
            tracing::warn!(
                target: "ica_native::command",
                bridge = %state.bridge_key,
                command = payload.get("kind").and_then(JsonValue::as_str),
                error = payload.get("message").and_then(JsonValue::as_str),
                "bridge 命令执行失败"
            );
            if payload.get("kind").and_then(JsonValue::as_str) == Some("fetchMessages")
                && let Some(room_id) = payload.get("roomId").and_then(JsonValue::as_i64)
            {
                // 超时或 bridge 拒绝请求后允许用户再次点击重试，不能让请求占位
                // 永久阻止这个房间后续加载。
                let conversation = state.conversation_mut(room_id);
                conversation.requested_snapshot = false;
                conversation.pending_message_scroll_to_bottom = false;
            }
            if payload.get("kind").and_then(JsonValue::as_str) == Some("fetchGroupMembers")
                && let Some(room_id) = payload.get("roomId").and_then(JsonValue::as_i64)
            {
                state.conversation_mut(room_id).loading_group_members = false;
            }
            if payload.get("kind").and_then(JsonValue::as_str) == Some("searchMessages") {
                let message = payload::payload_message(payload)
                    .unwrap_or_else(|| "搜索聊天记录失败".to_string());
                state.message_search.fail(message);
            }
            state.last_error = payload::payload_message(payload);
        }
        "socketApiResponse" | "fileManagerResponse" => {
            let response = payload::json_preview(payload, 1024);
            state.last_socket_api_response = Some(response.clone());
            let label = if event_name == "fileManagerResponse" {
                "文件管理"
            } else {
                "Socket API"
            };
            state.last_notice = Some(format!("{}: {}", label, response));
        }
        _ => return false,
    }
    true
}
