//! 直接透传给 Bridge 的原始 Socket.IO / 文件管理调用。

use std::time::Duration;

use futures_util::future::BoxFuture;
use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use serde_json::json;

use crate::ica::command::emit_ui_event;
use crate::ica::file_manager::call_file_manager;

use super::ack_payload_values;
use super::context::CommandContext;

pub(super) async fn socket_api_call(
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

pub(super) async fn file_manager_call(
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
