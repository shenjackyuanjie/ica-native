//! 合并转发消息的拉取与发送。
//!
//! 拉取要兼容多家 Bridge 的 ACK 形状：正常应为消息对象数组，部分实现会在
//! 外层多包几层单元素数组，也有把列表作为唯一参数再包一层的情况；主请求
//! 返回特定“错误响应”时，可按调用方提供的 `fallback_res_id` 用回退资源
//! 重试。发送则是普通的 `makeForward` 事件，Bridge 接收后即通知界面，
//! 由界面续走后续发送流程。

#[cfg(test)]
#[path = "tests/forward.rs"]
mod tests;

use super::ack_payload_values;
use crate::ica::{command::emit_ui_event, event::BridgeEvent};
use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

/// `getForwardMsg` 的 ACK 等待上限；主请求与回退请求共用同一上限。
const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

/// 判断 ACK 是否为合并转发的“错误响应”。
///
/// 个别 Bridge 拉不到内容时会回一条 `_id` 与 `senderId` 均为 0 的占位对象；
/// 只有看到它才值得用回退资源重试，避免把正常消息误判成失败。
fn is_forward_error_response(values: &[JsonValue]) -> bool {
    values.len() == 1
        && values[0].get("_id").and_then(JsonValue::as_i64) == Some(0)
        && values[0].get("senderId").and_then(JsonValue::as_i64) == Some(0)
}

/// 合并转发消息应当是对象数组；部分 bridge 会在 ACK 外额外套一层或多层单元素数组。
fn normalize_forward_values(mut values: Vec<JsonValue>) -> Vec<JsonValue> {
    loop {
        if values.len() != 1 {
            return values;
        }

        match values.into_iter().next().expect("已确认数组中仅有一个元素") {
            JsonValue::Array(nested) => values = nested,
            value => return vec![value],
        }
    }
}

/// 统计 ACK 值外层嵌套的单元素数组层数，仅用于日志定位兼容问题。
fn forward_wrapper_depth(mut values: &[JsonValue]) -> usize {
    let mut depth = 0;
    while let [JsonValue::Array(nested)] = values {
        depth += 1;
        values = nested;
    }
    depth
}

/// 把 JSON 值类型转成中文短名，供日志摘要使用。
fn json_value_kind(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "空值",
        JsonValue::Bool(_) => "布尔值",
        JsonValue::Number(_) => "数字",
        JsonValue::String(_) => "字符串",
        JsonValue::Array(_) => "数组",
        JsonValue::Object(_) => "对象",
    }
}

/// 生成 ACK 值的中文摘要（最多列 8 项）。
///
/// 只记录类型与关键字段形态，不输出消息正文，避免把聊天内容写进日志。
fn forward_values_summary(values: &[JsonValue]) -> String {
    const MAX_ITEMS: usize = 8;
    let mut items = values
        .iter()
        .take(MAX_ITEMS)
        .map(|value| match value {
            JsonValue::Object(object) => {
                let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
                let id_kind = object.get("_id").map(json_value_kind).unwrap_or("缺失");
                let sender_id_kind = object
                    .get("senderId")
                    .map(json_value_kind)
                    .unwrap_or("缺失");
                format!(
                    "对象(字段=[{}], _id={id_kind}, senderId={sender_id_kind})",
                    keys.join(",")
                )
            }
            JsonValue::Array(items) => format!("数组(元素数={})", items.len()),
            value => json_value_kind(value).to_string(),
        })
        .collect::<Vec<_>>();
    if values.len() > MAX_ITEMS {
        items.push(format!("另有 {} 项", values.len() - MAX_ITEMS));
    }
    format!("[{}]", items.join(", "))
}

/// 拉取合并转发消息，结果经 `forwardMessagesResponse` 事件返回。
///
/// `file_name` 为空串时按未提供处理；`fallback_res_id` 存在且主请求返回
/// 错误响应时，会自动用回退资源再请求一次。请求发送失败或超时都会发
/// `forwardMessagesFailed`。
pub async fn fetch_forward_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    res_id: String,
    file_name: Option<String>,
    fallback_res_id: Option<String>,
) {
    tracing::debug!(
        target: "ica_native::forward",
        bridge = bridge_key,
        request_id,
        res_id_length = res_id.chars().count(),
        file_name_present = file_name.is_some(),
        fallback_res_id_length = fallback_res_id.as_ref().map(|value| value.chars().count()),
        "正在请求合并转发消息"
    );
    let ack_received = Arc::new(AtomicBool::new(false));
    let ack_received_cb = ack_received.clone();
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    let res_id_for_event = res_id.clone();
    let file_name_for_event = file_name.clone();
    let fallback_for_callback = fallback_res_id.clone();
    let args = vec![
        json!(res_id),
        file_name
            .filter(|value| !value.trim().is_empty())
            .map_or(JsonValue::Null, JsonValue::String),
    ];

    let result = client
        .emit_with_ack(
            "getForwardMsg",
            args,
            FORWARD_TIMEOUT,
            move |payload: Payload, callback_client: Client| -> BoxFuture<'static, ()> {
                let ack_received = ack_received_cb.clone();
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                let res_id = res_id_for_event.clone();
                let file_name = file_name_for_event.clone();
                let fallback_res_id = fallback_for_callback.clone();
                Box::pin(async move {
                    let raw_values = ack_payload_values(&payload);
                    let extra_wrapper_depth = forward_wrapper_depth(&raw_values);
                    let values = normalize_forward_values(raw_values);
                    tracing::debug!(
                        target: "ica_native::forward",
                        bridge = %bridge_id,
                        request_id,
                        res_id_length = res_id.chars().count(),
                        file_name_present = file_name.is_some(),
                        extra_wrapper_depth,
                        value_count = values.len(),
                        value_summary = %forward_values_summary(&values),
                        "收到 getForwardMsg 主请求 ACK"
                    );
                    if let Some(fallback_res_id) = fallback_res_id
                        && is_forward_error_response(&values)
                    {
                        tracing::debug!(
                            target: "ica_native::forward",
                            bridge = %bridge_id,
                            request_id,
                            primary_res_id_length = res_id.chars().count(),
                            fallback_res_id_length = fallback_res_id.chars().count(),
                            "主请求返回合并转发错误，准备使用回退资源"
                        );
                        let fallback_tx = tx.clone();
                        let fallback_bridge_id = bridge_id.clone();
                        let fallback_ack_received = ack_received.clone();
                        let fallback_result = callback_client
                            .emit_with_ack(
                                "getForwardMsg",
                                vec![json!(fallback_res_id.clone()), JsonValue::Null],
                                FORWARD_TIMEOUT,
                                move |payload: Payload, _client: Client| {
                                    let tx = fallback_tx.clone();
                                    let bridge_id = fallback_bridge_id.clone();
                                    let ack_received = fallback_ack_received.clone();
                                    let res_id = fallback_res_id.clone();
                                    Box::pin(async move {
                                        let raw_values = ack_payload_values(&payload);
                                        let extra_wrapper_depth =
                                            forward_wrapper_depth(&raw_values);
                                        let values = normalize_forward_values(raw_values);
                                        tracing::debug!(
                                            target: "ica_native::forward",
                                            bridge = %bridge_id,
                                            request_id,
                                            res_id_length = res_id.chars().count(),
                                            extra_wrapper_depth,
                                            value_count = values.len(),
                                            value_summary = %forward_values_summary(&values),
                                            "收到 getForwardMsg 回退请求 ACK"
                                        );
                                        ack_received.store(true, Ordering::SeqCst);
                                        emit_ui_event(
                                            &tx,
                                            &bridge_id,
                                            "forwardMessagesResponse",
                                            json!({
                                                "requestId": request_id,
                                                "resId": res_id,
                                                "fileName": JsonValue::Null,
                                                "messages": values,
                                            }),
                                        );
                                    })
                                },
                            )
                            .await;
                        if let Err(error) = fallback_result {
                            tracing::warn!(
                                target: "ica_native::forward",
                                bridge = %bridge_id,
                                request_id,
                                error = %error,
                                "发送 getForwardMsg 回退请求失败"
                            );
                            ack_received.store(true, Ordering::SeqCst);
                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "forwardMessagesFailed",
                                json!({
                                    "requestId": request_id,
                                    "message": error.to_string(),
                                }),
                            );
                        }
                        return;
                    }
                    ack_received.store(true, Ordering::SeqCst);
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "forwardMessagesResponse",
                        json!({
                            "requestId": request_id,
                            "resId": res_id,
                            "fileName": file_name,
                            "messages": values,
                        }),
                    );
                })
            },
        )
        .await;

    if let Err(error) = result {
        tracing::warn!(
            target: "ica_native::forward",
            bridge = bridge_key,
            request_id,
            error = %error,
            "发送 getForwardMsg 请求失败"
        );
        emit_ui_event(
            event_tx,
            bridge_key,
            "forwardMessagesFailed",
            json!({
                "requestId": request_id,
                "message": error.to_string(),
            }),
        );
        return;
    }

    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(FORWARD_TIMEOUT).await;
        if !ack_received.load(Ordering::SeqCst) {
            tracing::warn!(
                target: "ica_native::forward",
                bridge = %bridge_id,
                request_id,
                timeout_seconds = FORWARD_TIMEOUT.as_secs(),
                "getForwardMsg 请求超时"
            );
            emit_ui_event(
                &tx,
                &bridge_id,
                "forwardMessagesFailed",
                json!({
                    "requestId": request_id,
                    "message": "getForwardMsg 请求超时",
                }),
            );
        }
    });
}

/// 发送合并转发。
///
/// `nodes` 为待转发消息节点，`direct_message` 表示按私聊发送，`origin` 为
/// 来源会话（群聊转发时透传），`target_room_id` 为目标会话。Bridge 接收后
/// 发 `forwardSendRequested`，实际发送结果由后续消息事件驱动。
pub async fn send_merged_forward(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    nodes: Vec<JsonValue>,
    direct_message: bool,
    origin: Option<i64>,
    target_room_id: i64,
) {
    let node_count = nodes.len();
    let args = vec![
        JsonValue::Array(nodes),
        json!(direct_message),
        origin.map_or(JsonValue::Null, |value| json!(value)),
        json!(target_room_id),
    ];
    if let Err(error) = client.emit("makeForward", args).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "makeForward",
                "roomId": target_room_id,
                "message": error.to_string(),
            }),
        );
        return;
    }

    emit_ui_event(
        event_tx,
        bridge_key,
        "forwardSendRequested",
        json!({
            "roomId": target_room_id,
            "count": node_count,
        }),
    );
}
