use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;

use futures_util::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde_json::Value as JsonValue;
use serde_json::json;

use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::types::message::{FileAttachment, SendMessage};

use super::client;
use super::command::{IcaCommand, emit_ui_event, json_preview};
use super::file_manager::call_file_manager;

fn ack_payload_values(payload: &Payload) -> Vec<JsonValue> {
    match payload {
        Payload::Text(values) => {
            if let Some(JsonValue::Array(args)) = values.first()
                && values.len() == 1
            {
                return args.clone();
            }
            values.clone()
        }
        Payload::Binary(bytes) => vec![json!(bytes.to_vec())],
        _ => Vec::new(),
    }
}

fn ack_payload_first(payload: &Payload) -> Option<JsonValue> {
    ack_payload_values(payload).into_iter().next()
}

fn push_raw_text_elements(chain: &mut Vec<JsonValue>, content: &str) {
    let mut remaining = content;
    while let Some(start) = remaining.find("[Face: ") {
        if start > 0 {
            chain.push(json!({
                "type": "text",
                "data": { "text": &remaining[..start] },
            }));
        }
        let after = &remaining[start + 7..];
        let Some(end) = after.find(']') else {
            remaining = &remaining[start..];
            break;
        };
        let Ok(face_id) = after[..end].parse::<u16>() else {
            chain.push(json!({
                "type": "text",
                "data": { "text": &remaining[start..start + 8 + end] },
            }));
            remaining = &after[end + 1..];
            continue;
        };
        chain.push(json!({
            "type": "face",
            "data": { "id": face_id },
        }));
        remaining = &after[end + 1..];
    }
    if !remaining.is_empty() {
        chain.push(json!({
            "type": "text",
            "data": { "text": remaining },
        }));
    }
}

fn build_multi_image_raw_payload(
    room_id: i64,
    content: &str,
    reply_to: Option<&crate::ica::types::message::ReplyMessage>,
    images: &[(String, Arc<[u8]>)],
) -> JsonValue {
    use base64::{Engine as _, engine::general_purpose};

    let mut chain = Vec::with_capacity(images.len() + 2);
    if let Some(reply) = reply_to {
        chain.push(json!({
            "type": "reply",
            "data": {
                "id": reply.msg_id,
                "text": reply.content,
            },
        }));
    }
    push_raw_text_elements(&mut chain, content);
    for (_, bytes) in images {
        chain.push(json!({
            "type": "image",
            "data": {
                "file": format!("base64://{}", general_purpose::STANDARD.encode(bytes)),
                "type": "image",
                "sub_type": 0,
            },
        }));
    }

    json!({
        "messageType": "raw",
        "roomId": room_id,
        "content": JsonValue::Array(chain).to_string(),
        "at": [],
        "sticker": false,
    })
}

async fn send_message(
    message: SendMessage,
    client: &Client,
    event_tx: &Option<UnboundedSender<JsonValue>>,
    bridge_key: &str,
    api_base_url: &str,
) {
    let room_id = message.room_id;
    if message.has_b64img() {
        match request_send_token(client).await {
            Ok(token) => {
                if let Err(e) = http_send_message(api_base_url, &token, &message).await {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "sendMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    );
                }
            }
            Err(e) => {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendMessage",
                        "roomId": room_id,
                        "message": e,
                    }),
                );
            }
        }
    } else if !client::send_message(client, &message).await {
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

pub(super) async fn handle_command(
    command: IcaCommand,
    client: &Client,
    event_tx: &Option<UnboundedSender<JsonValue>>,
    bridge_key: &str,
    socket_url: &str,
    api_base_url: &str,
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
                            let ack_values = ack_payload_values(&payload);
                            let messages = ack_values
                                .first()
                                .cloned()
                                .unwrap_or_else(|| JsonValue::Array(Vec::new()));
                            if !messages.is_array() {
                                tracing::warn!(
                                    "fetchMessages ack format unexpected: bridge={} room_id={} raw={}",
                                    bridge_id,
                                    room_id,
                                    json_preview(&JsonValue::Array(ack_values), 512)
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
        IcaCommand::FetchOlderMessages { room_id, offset } => {
            let timeout = Duration::from_secs(10);
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();

            let result = client
                .emit_with_ack(
                    "fetchMessages",
                    vec![json!(room_id), json!(offset)],
                    timeout,
                    move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                        let tx = tx.clone();
                        let bridge_id = bridge_id.clone();
                        Box::pin(async move {
                            let ack_values = ack_payload_values(&payload);
                            let messages = ack_values
                                .first()
                                .cloned()
                                .unwrap_or_else(|| JsonValue::Array(Vec::new()));

                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "appendOlderMessages",
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

            if let Err(e) = result {
                tracing::warn!("fetchOlderMessages failed for {}: {}", bridge_key, e);
                // 通知 UI 加载完成（即使失败也要重置加载状态）
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "appendOlderMessages",
                    json!([
                        {
                            "roomId": room_id,
                            "messages": [],
                        }
                    ]),
                );
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
                                ack_payload_first(&payload).unwrap_or(JsonValue::Null),
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
            send_message(message, client, event_tx, bridge_key, api_base_url).await;
        }
        IcaCommand::SendImageMessage {
            room_id,
            content,
            reply_to,
            image_type,
            image_data,
        } => {
            let encoded_message = tokio::task::spawn_blocking(move || {
                let mut message = SendMessage::new(content, room_id, reply_to);
                message.set_img(image_data.as_ref(), &image_type, false);
                message
            })
            .await;
            match encoded_message {
                Ok(message) => {
                    send_message(message, client, event_tx, bridge_key, api_base_url).await;
                }
                Err(e) => emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendImageMessage",
                        "roomId": room_id,
                        "message": format!("图片编码任务失败: {e}"),
                    }),
                ),
            }
        }
        IcaCommand::SendMultiImageMessage {
            room_id,
            content,
            reply_to,
            images,
        } => {
            let encoded_payload = tokio::task::spawn_blocking(move || {
                build_multi_image_raw_payload(room_id, &content, reply_to.as_ref(), &images)
            })
            .await;
            match encoded_payload {
                Ok(payload) => match request_send_token(client).await {
                    Ok(token) => {
                        if let Err(e) = http_send_value(api_base_url, &token, &payload).await {
                            emit_ui_event(
                                event_tx,
                                bridge_key,
                                "commandFailed",
                                json!({
                                    "kind": "sendMultiImageMessage",
                                    "roomId": room_id,
                                    "message": e,
                                }),
                            );
                        }
                    }
                    Err(e) => emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "sendMultiImageMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    ),
                },
                Err(e) => emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendMultiImageMessage",
                        "roomId": room_id,
                        "message": format!("图片编码任务失败: {e}"),
                    }),
                ),
            }
        }
        IcaCommand::SendRawMessage { room_id, content } => {
            let payload = json!({
                "messageType": "raw",
                "roomId": room_id,
                "content": content.to_string(),
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
        IcaCommand::SocketApiCall {
            event,
            args,
            expect_ack,
        } => {
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
        IcaCommand::FileManagerCall {
            gin,
            event,
            args,
            expect_ack,
        } => {
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
        IcaCommand::PinRoom { room_id, pin } => {
            if let Err(e) = client
                .emit("pinRoom", vec![json!(room_id), json!(pin)])
                .await
            {
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
            if let Err(e) = client
                .emit("ignoreChat", json!({"id": room_id, "name": room_name}))
                .await
            {
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
            if let Err(e) = client
                .emit("setRoomPriority", vec![json!(room_id), json!(priority)])
                .await
            {
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
        IcaCommand::ReportRead {
            room_id,
            message_id,
        } => {
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
        IcaCommand::SetOnlineStatus(status) => {
            if let Err(e) = client.emit("setOnlineStatus", json!(status)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "setOnlineStatus",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SendGroupSign { room_id } => {
            if !client::send_room_sign_in(client, room_id).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendGroupSign",
                        "roomId": room_id,
                        "message": "sendGroupSign failed",
                    }),
                );
            }
        }
        IcaCommand::SendGroupPoke { room_id, target_id } => {
            if !client::send_poke(client, room_id, target_id).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendGroupPoke",
                        "roomId": room_id,
                        "targetId": target_id,
                        "message": "sendGroupPoke failed",
                    }),
                );
            }
        }
        IcaCommand::StopFetchingHistory => {
            if let Err(e) = client.emit("stopFetchingHistory", json!(null)).await {
                tracing::warn!("send stopFetchingHistory failed: {}", e);
            }
        }
        IcaCommand::HideMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "hideMessage",
                    vec![json!(room_id), json!(message_id.clone())],
                )
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
        IcaCommand::RevealMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "revealMessage",
                    vec![json!(room_id), json!(message_id.clone())],
                )
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
        IcaCommand::RenewMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "renewMessage",
                    vec![json!(room_id), json!(message_id.clone()), json!(null)],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "renewMessage",
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
        IcaCommand::AddChatGroup {
            name,
            rooms,
            include_all_personal,
        } => {
            let payload = json!({
                "name": name,
                "rooms": rooms,
                "includeAllPersonal": include_all_personal,
            });
            if let Err(e) = client.emit("addChatGroup", payload).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "addChatGroup",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveChatGroup { name } => {
            if let Err(e) = client.emit("removeChatGroup", json!(name)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeChatGroup",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::UpdateChatGroup {
            name,
            rooms,
            include_all_personal,
        } => {
            let payload = json!({
                "name": name,
                "rooms": rooms,
                "includeAllPersonal": include_all_personal,
            });
            if let Err(e) = client
                .emit("updateChatGroup", vec![json!(name), payload])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "updateChatGroup",
                        "message": e.to_string(),
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
        IcaCommand::SendFileMessage {
            room_id,
            content,
            reply_to,
            file_name,
            file_type,
            file_data,
        } => {
            match upload_and_send_file(
                client, event_tx, bridge_key, room_id, content, reply_to, &file_name, &file_type,
                &file_data,
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
                            "kind": "sendFileMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    );
                }
            }
        }
    }
}

async fn request_send_token(client: &Client) -> Result<String, String> {
    let timeout = Duration::from_secs(30);
    let token = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let token_cb = token.clone();

    let result = client
        .emit_with_ack(
            "requestToken",
            Vec::<JsonValue>::new(),
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let token = token_cb.clone();
                Box::pin(async move {
                    let token_str = ack_payload_first(&payload)
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    *token.lock().await = Some(token_str);
                })
            },
        )
        .await;

    if let Err(e) = result {
        return Err(format!("requestToken 发送失败: {}", e));
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut attempts = 0;
    loop {
        if let Some(token) = token.lock().await.take() {
            if token.is_empty() {
                return Err("requestToken 返回空 token".to_string());
            }
            return Ok(token);
        }
        attempts += 1;
        if attempts > 100 {
            return Err("requestToken 超时".to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn http_send_message(
    api_base_url: &str,
    token: &str,
    message: &SendMessage,
) -> Result<(), String> {
    http_send_value(api_base_url, token, &message.as_value()).await
}

async fn http_send_value(api_base_url: &str, token: &str, value: &JsonValue) -> Result<(), String> {
    let api_base_url = api_base_url.trim_end_matches('/');
    let url = format!("{}/api/{}/sendMessage", api_base_url, token);
    let client = reqwest::Client::new();

    let response = client
        .post(&url)
        .json(value)
        .send()
        .await
        .map_err(|e| format!("HTTP POST 失败: {}", e))?;

    match response.status() {
        reqwest::StatusCode::ACCEPTED => Ok(()),
        reqwest::StatusCode::FORBIDDEN => Err("token 验证失败 (403)".to_string()),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => Err("图片过大，无法发送 (413)".to_string()),
        status => Err(format!("sendMessage HTTP 错误: {}", status)),
    }
}

/// 分块上传文件并发送消息
///
/// 流程参照 ICA++ socketIoAdapter:
/// 1. 计算文件 SHA256 hash
/// 2. requestUpload(fileName, hash, fileSize) → 获取已上传的分块偏移列表
/// 3. 逐块 uploadFile(fileHash, offset, chunk, chunkHash)
/// 4. sendMessage 带 file { type, path: hash, size }
#[allow(clippy::too_many_arguments)]
async fn upload_and_send_file(
    client: &Client,
    _event_tx: &Option<UnboundedSender<JsonValue>>,
    _bridge_key: &str,
    room_id: i64,
    content: String,
    reply_to: Option<crate::ica::types::message::ReplyMessage>,
    file_name: &str,
    file_type: &str,
    file_data: &[u8],
) -> Result<(), String> {
    const CHUNK_SIZE: usize = 512 * 1024; // 512KB

    // 1. 计算文件 SHA256 hash
    let file_hash = {
        let mut hasher = Sha256::new();
        hasher.update(file_data);
        hex::encode(hasher.finalize())
    };

    // 2. requestUpload → 获取已上传的分块列表
    let timeout = Duration::from_secs(30);
    let uploaded_offsets = Arc::new(tokio::sync::Mutex::new(None::<(bool, Vec<usize>)>));
    let uploaded_offsets_cb = uploaded_offsets.clone();

    let result = client
        .emit_with_ack(
            "requestUpload",
            vec![json!(file_name), json!(file_hash), json!(file_data.len())],
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let uploaded_offsets = uploaded_offsets_cb.clone();
                Box::pin(async move {
                    let value = ack_payload_first(&payload).unwrap_or(JsonValue::Null);
                    let all_success = value
                        .get("allSuccess")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let uploaded: Vec<usize> = value
                        .get("uploaded")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_u64().map(|n| n as usize))
                                .collect()
                        })
                        .unwrap_or_default();
                    *uploaded_offsets.lock().await = Some((all_success, uploaded));
                })
            },
        )
        .await;

    if let Err(e) = result {
        return Err(format!("requestUpload 失败: {}", e));
    }

    // 等待 ack 结果
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut attempts = 0;
    let (all_success, uploaded) = loop {
        if let Some(result) = uploaded_offsets.lock().await.take() {
            break result;
        }
        attempts += 1;
        if attempts > 300 {
            return Err("requestUpload 超时".to_string());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // 3. 如果未全部上传, 逐块上传
    if !all_success {
        let chunks: Vec<&[u8]> = file_data.chunks(CHUNK_SIZE).collect();

        for (i, chunk) in chunks.iter().enumerate() {
            let offset = i * CHUNK_SIZE;
            if uploaded.contains(&offset) {
                continue;
            }

            let chunk_hash = {
                let mut hasher = Sha256::new();
                hasher.update(chunk);
                hex::encode(hasher.finalize())
            };

            // 重试最多 3 次
            let mut success = false;
            for retry in 0..3 {
                let upload_result = Arc::new(tokio::sync::Mutex::new(None::<bool>));
                let upload_result_cb = upload_result.clone();
                let chunk_vec = chunk.to_vec();

                let emit_result = client
                    .emit_with_ack(
                        "uploadFile",
                        vec![
                            json!(file_hash),
                            json!(offset),
                            json!(chunk_vec),
                            json!(chunk_hash),
                        ],
                        Duration::from_secs(30),
                        move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                            let upload_result = upload_result_cb.clone();
                            Box::pin(async move {
                                let ok = ack_payload_first(&payload)
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                *upload_result.lock().await = Some(ok);
                            })
                        },
                    )
                    .await;

                if emit_result.is_err() {
                    if retry < 2 {
                        continue;
                    }
                    return Err(format!("uploadFile emit 失败: offset={}", offset));
                }

                // 等待 ack
                let mut ack_attempts = 0;
                let chunk_ok = loop {
                    if let Some(result) = upload_result.lock().await.take() {
                        break result;
                    }
                    ack_attempts += 1;
                    if ack_attempts > 300 {
                        break false;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                };

                if chunk_ok {
                    success = true;
                    break;
                }
            }

            if !success {
                return Err(format!("文件上传失败: offset={}", offset));
            }
        }
    }

    // 4. 发送 sendMessage 带 file 附件
    let mut message = SendMessage::new(content, room_id, reply_to);
    message.file = Some(FileAttachment {
        file_type: file_type.to_string(),
        path: file_hash,
        size: file_data.len(),
    });

    if !client::send_message(client, &message).await {
        return Err("sendMessage 发送失败".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value as JsonValue;

    use super::build_multi_image_raw_payload;

    #[test]
    fn multi_image_payload_contains_one_raw_chain() {
        let payload = build_multi_image_raw_payload(
            -123,
            "文字[Face: 14]",
            None,
            &[
                ("image/png".to_string(), Arc::from([1_u8, 2, 3])),
                ("image/jpeg".to_string(), Arc::from([4_u8, 5, 6])),
            ],
        );

        assert_eq!(payload["messageType"], "raw");
        assert_eq!(payload["roomId"], -123);
        let chain: Vec<JsonValue> =
            serde_json::from_str(payload["content"].as_str().unwrap()).unwrap();
        assert_eq!(chain.len(), 4);
        assert_eq!(chain[0]["type"], "text");
        assert_eq!(chain[1]["type"], "face");
        assert_eq!(chain[2]["type"], "image");
        assert_eq!(chain[3]["type"], "image");
        assert_eq!(chain[2]["data"]["file"], "base64://AQID");
        assert_eq!(chain[3]["data"]["file"], "base64://BAUG");
    }
}
