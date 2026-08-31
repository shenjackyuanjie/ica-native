//! 合并转发查看与转发发送相关的事件。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::message::Message;

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub(super) fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "forwardMessagesResponse" => {
            let request_id = payload
                .get("requestId")
                .and_then(JsonValue::as_u64)
                .unwrap_or_default();
            let res_id = payload
                .get("resId")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            let file_name = payload.get("fileName").and_then(JsonValue::as_str);
            let messages = payload.get("messages");
            tracing::debug!(
                target: "ica_native::forward",
                bridge = %state.bridge_key,
                request_id,
                res_id_present = res_id.is_some(),
                res_id_length = res_id.as_ref().map(|value| value.chars().count()),
                file_name_present = file_name.is_some(),
                message_count = messages.and_then(JsonValue::as_array).map(Vec::len),
                "收到合并转发消息响应"
            );
            match messages.map(Vec::<Message>::deserialize) {
                Some(Ok(messages)) => {
                    tracing::debug!(
                        target: "ica_native::forward",
                        bridge = %state.bridge_key,
                        request_id,
                        message_count = messages.len(),
                        "合并转发消息响应解析成功"
                    );
                    state
                        .forward_viewer
                        .lock()
                        .unwrap()
                        .apply_response(request_id, res_id, messages);
                }
                Some(Err(error)) => {
                    let invalid_message = payload["messages"].as_array().and_then(|messages| {
                        messages.iter().enumerate().find_map(|(index, value)| {
                            serde_json::from_value::<Message>(value.clone())
                                .err()
                                .map(|error| (index, error, payload::json_shape(value)))
                        })
                    });
                    if let Some((index, item_error, shape)) = &invalid_message {
                        tracing::debug!(
                            target: "ica_native::forward",
                            bridge = %state.bridge_key,
                            request_id,
                            message_index = index,
                            error = %item_error,
                            message_shape = %shape,
                            "已定位无法解析的合并转发消息"
                        );
                    }
                    tracing::warn!(
                        target: "ica_native::forward",
                        bridge = %state.bridge_key,
                        request_id,
                        error = %error,
                        invalid_message_index = invalid_message.as_ref().map(|value| value.0),
                        "合并转发消息响应解析失败"
                    );
                    state
                        .forward_viewer
                        .lock()
                        .unwrap()
                        .fail(request_id, format!("合并转发内容解析失败: {error}"));
                }
                None => {
                    tracing::warn!(
                        target: "ica_native::forward",
                        bridge = %state.bridge_key,
                        request_id,
                        "合并转发消息响应缺少 messages 字段"
                    );
                    state
                        .forward_viewer
                        .lock()
                        .unwrap()
                        .fail(request_id, "合并转发响应缺少 messages".to_string());
                }
            }
        }
        "forwardMessagesFailed" => {
            let request_id = payload
                .get("requestId")
                .and_then(JsonValue::as_u64)
                .unwrap_or_default();
            let message =
                payload::payload_message(payload).unwrap_or_else(|| "查看合并转发失败".to_string());
            tracing::warn!(
                target: "ica_native::forward",
                bridge = %state.bridge_key,
                request_id,
                error = %message,
                "合并转发消息请求失败"
            );
            state
                .forward_viewer
                .lock()
                .unwrap()
                .fail(request_id, message);
        }
        "forwardSendRequested" => {
            let count = payload
                .get("count")
                .and_then(JsonValue::as_u64)
                .unwrap_or_default();
            state.last_notice = Some(format!("已请求发送 {} 条消息的合并转发", count));
        }
        _ => return false,
    }
    true
}
