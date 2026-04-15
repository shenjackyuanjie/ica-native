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

use sha2::{Sha256, Digest};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::types::message::{FileAttachment, SendMessage};

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
                            let raw_payload = payload_to_json(&payload);
                            let messages = raw_payload
                                .as_array()
                                .and_then(|values| values.first())
                                .cloned()
                                .map(unwrap_singleton_array_layers)
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
        IcaCommand::SendFileMessage {
            room_id,
            content,
            reply_to,
            file_name,
            file_type,
            file_data,
        } => {
            match upload_and_send_file(client, event_tx, bridge_key, room_id, content, reply_to, &file_name, &file_type, &file_data).await {
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
                    let json = payload_to_json(&payload);
                    let value = json
                        .as_array()
                        .and_then(|a| a.first())
                        .cloned()
                        .unwrap_or(json.clone());
                    let all_success = value.get("allSuccess").and_then(|v| v.as_bool()).unwrap_or(false);
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
                                let json = payload_to_json(&payload);
                                let ok = json
                                    .as_array()
                                    .and_then(|a| a.first())
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
