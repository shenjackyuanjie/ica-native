//! 直接透传给 Bridge 的原始 Socket.IO / 文件管理调用。

use super::ack_payload_values;
use super::context::CommandContext;
use crate::ica::command::emit_ui_event;
use crate::ica::file_manager::call_file_manager;
use futures_util::future::BoxFuture;
use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::time::Duration;

/// 把任意 Socket.IO 事件透传给 Bridge。
///
/// `expect_ack` 为 true 时使用 15 秒 ACK 等待，回执经 `socketApiResponse`
/// 事件返回；为 false 时只发送，成功后同样以 `socketApiResponse`（带 `sent`）
/// 确认。发送或等待失败统一上报 `commandFailed`。
pub async fn socket_api_call(
    ctx: CommandContext<'_>,
    event: String,
    args: Vec<JsonValue>,
    expect_ack: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if expect_ack {
        let event_for_cb = event.clone();
        let tx = event_tx.clone();
        let bridge_id = bridge_key.to_string();
        if let Err(e) = client
            .emit_with_ack(
                event.as_str(),
                args,
                Duration::from_secs(15),
                move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                    let tx = tx.clone();
                    let bridge_id = bridge_id.clone();
                    let event = event_for_cb.clone();
                    Box::pin(async move {
                        emit_ui_event(
                            &tx,
                            &bridge_id,
                            "socketApiResponse",
                            json!({
                                "event": event,
                                "ack": ack_payload_values(&payload),
                            }),
                        );
                    })
                },
            )
            .await
        {
            emit_ui_event(
                event_tx,
                bridge_key,
                "commandFailed",
                json!({
                    "kind": "socketApiCall",
                    "event": event,
                    "message": e.to_string(),
                }),
            );
        }
    } else if let Err(e) = client.emit(event.as_str(), args).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "socketApiCall",
                "event": event,
                "message": e.to_string(),
            }),
        );
    } else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "socketApiResponse",
            json!({
                "event": event,
                "sent": true,
            }),
        );
    }
}

/// 向指定 `gin` 的文件管理连接透传调用。
///
/// 结果与错误的投递由 `call_file_manager` 负责，这里只把失败转换成
/// `commandFailed` 事件。
pub async fn file_manager_call(
    ctx: CommandContext<'_>,
    gin: i64,
    event: String,
    args: Vec<JsonValue>,
    expect_ack: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        socket_url,
        ..
    } = ctx;
    match call_file_manager(
        client, event_tx, bridge_key, socket_url, gin, event, args, expect_ack,
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            emit_ui_event(
                event_tx,
                bridge_key,
                "commandFailed",
                json!({
                    "kind": "fileManagerCall",
                    "gin": gin,
                    "message": e,
                }),
            );
        }
    }
}
