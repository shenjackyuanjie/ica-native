//! 尚未归入具体领域的通用事件：加载提示、命令失败与调试用的原始响应。
//!
//! 这些事件横跨多个功能，或暂无法归入单一领域。其中 `commandFailed` 需按 `kind`
//! 分发到不同子系统收回“进行中”状态，是让界面退出无响应加载态的关键补偿路径。

#[cfg(test)]
#[path = "tests/misc.rs"]
mod tests;

use super::payload;
use crate::app::state::{BridgeState, DatabaseUpgradeProgress};
use serde::Deserialize;
use serde_json::Value as JsonValue;

/// `dbUpgradeProgress` 事件的反序列化载体。
///
/// Bridge 端字段为 camelCase，这里用 `rename_all` 对齐；所有字段都设 `default`，
/// 兼容旧版 Bridge 只上报部分字段的情况。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseUpgradeProgressPayload {
    /// 是否正在进行数据库升级。
    #[serde(default)]
    active: bool,
    /// 当前步骤序号。
    #[serde(default)]
    step: u64,
    /// 总步骤数。
    #[serde(default)]
    total: u64,
    /// 供界面展示的进度描述文本。
    #[serde(default)]
    message: String,
}

/// 从事件 payload 解析数据库升级进度。
///
/// payload 仍是数组包装，取第一个元素后按 camelCase 反序列化；空参数或结构不符时
/// 返回错误字符串，交由调用方记录并提示。
fn parse_database_upgrade_progress(payload: &JsonValue) -> Result<DatabaseUpgradeProgress, String> {
    let value = payload::first_payload_value(payload)
        .ok_or_else(|| "事件参数为空或不是数组".to_string())?;
    let parsed =
        DatabaseUpgradeProgressPayload::deserialize(value).map_err(|error| error.to_string())?;
    Ok(DatabaseUpgradeProgress {
        active: parsed.active,
        step: parsed.step,
        total: parsed.total,
        message: parsed.message,
    })
}

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "closeLoading" => {}
        "notifyError" => {
            if let Some(msg) = payload::first_payload_display_message(payload) {
                state.last_error = Some(msg);
            }
        }
        "dbUpgradeProgress" => match parse_database_upgrade_progress(payload) {
            Ok(progress) => state.db_upgrade_progress = progress,
            Err(error) => {
                payload::log_event_parse_failure(state, event_name, &error, payload);
                state.last_error = Some(format!("数据库升级进度解析失败: {error}"));
            }
        },
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
