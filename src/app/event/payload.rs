//! Bridge 事件 payload 的共享解析辅助。
//!
//! socket.io 的事件参数基本都是数组包装，且同一个字段在不同协议端下
//! 可能是字符串、对象或缺失，这里集中处理，避免每个事件分支各抄一遍。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::room::JoinRequestRoom;

/// socket.io 的事件 payload 基本都是数组包装，真正的数据通常在第一个元素里。
pub fn first_payload_value(payload: &JsonValue) -> Option<&JsonValue> {
    payload.as_array().and_then(|values| values.first())
}

/// 统一提取事件里常见的 `message` 字段，避免每个分支都手动抄一遍。
pub fn payload_message(payload: &JsonValue) -> Option<String> {
    payload
        .get("message")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

pub fn value_to_display_message(value: &JsonValue) -> Option<String> {
    if let Some(msg) = value.as_str() {
        return Some(msg.to_string());
    }

    let title = value.get("title").and_then(|value| value.as_str());
    let message = value.get("message").and_then(|value| value.as_str());
    match (title, message) {
        (Some(title), Some(message)) if !title.is_empty() && !message.is_empty() => {
            Some(format!("{title}: {message}"))
        }
        (_, Some(message)) if !message.is_empty() => Some(message.to_string()),
        (Some(title), _) if !title.is_empty() => Some(title.to_string()),
        _ => None,
    }
}

pub fn first_payload_display_message(payload: &JsonValue) -> Option<String> {
    first_payload_value(payload).and_then(value_to_display_message)
}

pub fn json_preview(value: &JsonValue, max_chars: usize) -> String {
    let raw = value.to_string();
    let mut chars = raw.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

pub fn json_shape(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "空值".to_string(),
        JsonValue::Bool(_) => "布尔值".to_string(),
        JsonValue::Number(_) => "数字".to_string(),
        JsonValue::String(_) => "字符串".to_string(),
        JsonValue::Array(values) => format!("数组(元素数={})", values.len()),
        JsonValue::Object(object) => format!(
            "对象(字段=[{}])",
            object
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

pub fn log_event_parse_failure(state: &BridgeState, event: &str, error: &str, payload: &JsonValue) {
    tracing::warn!(
        target: "ica_native::protocol",
        bridge = %state.bridge_key,
        event,
        error,
        "bridge 事件解析失败"
    );
    tracing::debug!(
        target: "ica_native::protocol",
        bridge = %state.bridge_key,
        event,
        payload = %json_preview(payload, 2048),
        "bridge 事件原始数据"
    );
}

pub fn parse_join_request(
    value: &JsonValue,
    fallback_flag: Option<&str>,
) -> Result<JoinRequestRoom, String> {
    let mut request = JoinRequestRoom::deserialize(value).map_err(|e| e.to_string())?;
    if request.flag.is_empty()
        && let Some(flag) = fallback_flag
    {
        request.flag = flag.to_string();
    }
    request.request_type = request.request_type.trim().to_string();
    request.post_type = request.post_type.trim().to_string();
    request.sub_type = request.sub_type.trim().to_string();
    request.source = request.source.trim().to_string();
    request.group_name = request.group_name.trim().to_string();
    request.nickname = request.nickname.trim().to_string();
    request.comment = request.comment.trim().to_string();
    request.tips = request.tips.trim().to_string();
    Ok(request)
}

pub fn parse_join_requests_snapshot(value: &JsonValue) -> Result<Vec<JoinRequestRoom>, String> {
    match value {
        JsonValue::Array(values) => values
            .iter()
            .map(|item| parse_join_request(item, None))
            .collect(),
        JsonValue::Object(items) => items
            .iter()
            .map(|(flag, item)| parse_join_request(item, Some(flag)))
            .collect(),
        _ => Err("getSystemMsg 返回的不是数组或对象".to_string()),
    }
}
