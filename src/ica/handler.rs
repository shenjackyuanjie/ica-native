use rust_socketio::asynchronous::Client;
use rust_socketio::Payload;

use futures_util::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::UnboundedSender;

use super::client;
use super::command::{
    IcaCommand, emit_ui_event, json_preview, payload_to_json, unwrap_singleton_array_layers,
};

pub(super) async fn handle_command(
    command: IcaCommand,
    client: &Client,
    event_tx: &Option<UnboundedSender<JsonValue>>,
    bridge_key: &str,
) {
    match command {
        IcaCommand::FetchMessages(room_id) => {
            let timeout = Duration::from_secs(10);
            let ack_received = Arc::new(AtomicBool::new(false));
            let ack_received_cb = ack_received.clone();
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();

            let result = client
                .emit_with_ack(
                    "fetchMessages",
                    vec![json!(room_id), json!(0)],
                    timeout,
                    move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                        let ack_received = ack_received_cb.clone();
                        let tx = tx.clone();
                        let bridge_id = bridge_id.clone();
                        Box::pin(async move {
                            ack_received.store(true, Ordering::SeqCst);
                            let raw_payload = payload_to_json(&payload);
                            let messages = raw_payload
                                .as_array()
                                .and_then(|values| values.first())
                                .cloned()
                                .map(unwrap_singleton_array_layers)
                                .unwrap_or_else(|| JsonValue::Array(Vec::new()));
                            if !messages.is_array() {
                                tracing::warn!(
                                    "fetchMessages ack format unexpected: bridge={} room_id={} raw={}",
                                    bridge_id,
                                    room_id,
                                    json_preview(&raw_payload, 512)
                                );
                            }

                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "setMessages",
                                json!([
                                    {
                                        "roomId": room_id,
                                        "messages": messages,
                                    }
                                ]),
                            );
                        })
                    },
                )
                .await;

            match result {
                Ok(()) => {
                    let tx = event_tx.clone();
                    let bridge_id = bridge_key.to_string();
                    tokio::spawn(async move {
                        tokio::time::sleep(timeout).await;
                        if !ack_received.load(Ordering::SeqCst) {
                            tracing::warn!(
                                "fetchMessages timeout: bridge={} room_id={}",
                                bridge_id,
                                room_id
                            );
                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "commandFailed",
                                json!({
                                    "kind": "fetchMessages",
                                    "roomId": room_id,
                                    "message": "fetchMessages 请求超时",
                                }),
                            );
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("fetchMessages failed for {}: {}", bridge_key, e);
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "fetchMessages",
                            "roomId": room_id,
                            "message": e.to_string(),
                        }),
                    );
                }
            }
        }
        IcaCommand::GetSystemMsg => {
            let timeout = Duration::from_secs(10);
            let ack_received = Arc::new(AtomicBool::new(false));
            let ack_received_cb = ack_received.clone();
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();

            let result = client
                .emit_with_ack(
                    "getSystemMsg",
                    Vec::<JsonValue>::new(),
                    timeout,
                    move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                        let ack_received = ack_received_cb.clone();
                        let tx = tx.clone();
                        let bridge_id = bridge_id.clone();
                        Box::pin(async move {
                            ack_received.store(true, Ordering::SeqCst);
                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "setSystemMessages",
                                payload_to_json(&payload),
                            );
                        })
                    },
                )
                .await;

            match result {
                Ok(()) => {
                    let tx = event_tx.clone();
                    let bridge_id = bridge_key.to_string();
                    tokio::spawn(async move {
                        tokio::time::sleep(timeout).await;
                        if !ack_received.load(Ordering::SeqCst) {
                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "commandFailed",
                                json!({
                                    "kind": "getSystemMsg",
                                    "message": "getSystemMsg 请求超时",
                                }),
                            );
                        }
                    });
                }
                Err(e) => {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "getSystemMsg",
                            "message": e.to_string(),
                        }),
                    );
                }
            }
        }
        IcaCommand::SendMessage(message) => {
            let room_id = message.room_id;
            if !client::send_message(client, &message).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendMessage",
                        "roomId": room_id,
                        "message": "sendMessage failed",
                    }),
                );
            }
        }
        IcaCommand::SendRawMessage { room_id, content } => {
            let payload = json!({
                "messageType": "raw",
                "roomId": room_id,
                "content": content,
            });
            if !client::send_string_message(client, &payload).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendRawMessage",
                        "roomId": room_id,
                        "message": "sendRawMessage failed",
                    }),
                );
            }
        }
        IcaCommand::PinRoom { room_id, pin } => {
            if let Err(e) = client.emit("pinRoom", vec![json!(room_id), json!(pin)]).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "pinRoom",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveChat(room_id) => {
            if let Err(e) = client.emit("removeChat", json!(room_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::IgnoreChat { room_id, room_name } => {
            if let Err(e) = client.emit("ignoreChat", json!({"id": room_id, "name": room_name})).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "ignoreChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveIgnoredChat(room_id) => {
            if let Err(e) = client.emit("removeIgnoredChat", json!(room_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeIgnoredChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SetRoomPriority { room_id, priority } => {
            if let Err(e) = client.emit("setRoomPriority", vec![json!(room_id), json!(priority)]).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "setRoomPriority",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::ReportRead { room_id, message_id } => {
            if let Err(e) = client.emit("reportRead", json!(message_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "reportRead",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::StopFetchingHistory => {
            if let Err(e) = client.emit("stopFetchingHistory", json!(null)).await {
                tracing::warn!("send stopFetchingHistory failed: {}", e);
            }
        }
        IcaCommand::HideMessage { room_id, message_id } => {
            if let Err(e) = client
                .emit("hideMessage", vec![json!(room_id), json!(message_id.clone())])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "hideMessage",
                        "roomId": room_id,
                        "messageId": message_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RevealMessage { room_id, message_id } => {
            if let Err(e) = client
                .emit("revealMessage", vec![json!(room_id), json!(message_id.clone())])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "revealMessage",
                        "roomId": room_id,
                        "messageId": message_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::DeleteMessage(message) => {
            let message_id = message.message_id.clone();
            if !client::delete_message(client, &message).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "deleteMessage",
                        "messageId": message_id,
                        "message": "deleteMessage failed",
                    }),
                );
            }
        }
        IcaCommand::HandleRequest {
            request_type,
            flag,
            accept,
        } => {
            if let Err(e) = client
                .emit(
                    "handleRequest",
                    vec![json!(request_type), json!(flag), json!(accept)],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "handleRequest",
                        "flag": flag,
                        "message": e.to_string(),
                    }),
                );
            }
        }
    }
}
