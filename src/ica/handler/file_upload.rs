use std::{sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};

use crate::ica::{
    client,
    types::message::{FileAttachment, Mention, ReplyMessage, SendMessage},
};

use super::ack_payload_first;

#[allow(clippy::too_many_arguments)]
pub async fn upload_and_send_file(
    client: &Client,
    room_id: i64,
    content: String,
    reply_to: Option<ReplyMessage>,
    mentions: Vec<Mention>,
    file_name: &str,
    file_type: &str,
    file_data: &[u8],
) -> Result<(), String> {
    const CHUNK_SIZE: usize = 512 * 1024;
    let file_hash = sha256(file_data);
    let uploaded_offsets = Arc::new(tokio::sync::Mutex::new(None::<(bool, Vec<usize>)>));
    let uploaded_offsets_cb = uploaded_offsets.clone();

    client
        .emit_with_ack(
            "requestUpload",
            vec![json!(file_name), json!(file_hash), json!(file_data.len())],
            Duration::from_secs(30),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let uploaded_offsets = uploaded_offsets_cb.clone();
                Box::pin(async move {
                    let value = ack_payload_first(&payload).unwrap_or(JsonValue::Null);
                    let all_success = value
                        .get("allSuccess")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                    let uploaded = value
                        .get("uploaded")
                        .and_then(JsonValue::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|value| value.as_u64().map(|number| number as usize))
                                .collect()
                        })
                        .unwrap_or_default();
                    *uploaded_offsets.lock().await = Some((all_success, uploaded));
                })
            },
        )
        .await
        .map_err(|error| format!("requestUpload 失败: {error}"))?;

    let (all_success, uploaded) = wait_for_result(&uploaded_offsets, 300)
        .await
        .ok_or_else(|| "requestUpload 超时".to_string())?;
    if !all_success {
        for (index, chunk) in file_data.chunks(CHUNK_SIZE).enumerate() {
            let offset = index * CHUNK_SIZE;
            if !uploaded.contains(&offset) {
                upload_chunk(client, &file_hash, offset, chunk).await?;
            }
        }
    }

    let mut message = SendMessage::new(content, room_id, reply_to);
    message.set_mentions(&mentions);
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

pub async fn upload_group_file(
    client: &Client,
    group_id: i64,
    parent_id: &str,
    file_name: &str,
    file_data: &[u8],
) -> Result<(), String> {
    let file_hash = upload_file_to_bridge(client, file_name, file_data).await?;
    let result = Arc::new(tokio::sync::Mutex::new(None::<Result<(), String>>));
    let result_cb = result.clone();
    client
        .emit_with_ack(
            "uploadGroupFile",
            vec![
                json!(file_hash),
                json!(group_id),
                json!(parent_id),
                json!(file_name),
            ],
            Duration::from_secs(10 * 60),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let result = result_cb.clone();
                Box::pin(async move {
                    let response = ack_payload_first(&payload).unwrap_or(JsonValue::Null);
                    let outcome = if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                        Ok(())
                    } else {
                        Err(response
                            .get("error")
                            .and_then(JsonValue::as_str)
                            .unwrap_or("群文件上传失败")
                            .to_string())
                    };
                    *result.lock().await = Some(outcome);
                })
            },
        )
        .await
        .map_err(|error| format!("uploadGroupFile 发送失败: {error}"))?;
    wait_for_result(&result, 6_000)
        .await
        .unwrap_or_else(|| Err("uploadGroupFile 超时".to_string()))
}

async fn upload_file_to_bridge(
    client: &Client,
    file_name: &str,
    file_data: &[u8],
) -> Result<String, String> {
    const CHUNK_SIZE: usize = 512 * 1024;
    let file_hash = sha256(file_data);
    let uploaded_offsets = Arc::new(tokio::sync::Mutex::new(None::<(bool, Vec<usize>)>));
    let uploaded_offsets_cb = uploaded_offsets.clone();
    client
        .emit_with_ack(
            "requestUpload",
            vec![json!(file_name), json!(file_hash), json!(file_data.len())],
            Duration::from_secs(30),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let uploaded_offsets = uploaded_offsets_cb.clone();
                Box::pin(async move {
                    let value = ack_payload_first(&payload).unwrap_or(JsonValue::Null);
                    let all_success = value
                        .get("allSuccess")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                    let uploaded = value
                        .get("uploaded")
                        .and_then(JsonValue::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|value| value.as_u64().map(|number| number as usize))
                                .collect()
                        })
                        .unwrap_or_default();
                    *uploaded_offsets.lock().await = Some((all_success, uploaded));
                })
            },
        )
        .await
        .map_err(|error| format!("requestUpload 失败: {error}"))?;
    let (all_success, uploaded) = wait_for_result(&uploaded_offsets, 300)
        .await
        .ok_or_else(|| "requestUpload 超时".to_string())?;
    if !all_success {
        for (index, chunk) in file_data.chunks(CHUNK_SIZE).enumerate() {
            let offset = index * CHUNK_SIZE;
            if !uploaded.contains(&offset) {
                upload_chunk(client, &file_hash, offset, chunk).await?;
            }
        }
    }
    Ok(file_hash)
}

async fn upload_chunk(
    client: &Client,
    file_hash: &str,
    offset: usize,
    chunk: &[u8],
) -> Result<(), String> {
    let chunk_hash = sha256(chunk);
    for retry in 0..3 {
        let result = Arc::new(tokio::sync::Mutex::new(None::<bool>));
        let result_cb = result.clone();
        let emit = client
            .emit_with_ack(
                "uploadFile",
                vec![
                    json!(file_hash),
                    json!(offset),
                    json!(chunk.to_vec()),
                    json!(chunk_hash),
                ],
                Duration::from_secs(30),
                move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                    let result = result_cb.clone();
                    Box::pin(async move {
                        *result.lock().await = Some(
                            ack_payload_first(&payload)
                                .and_then(|value| value.as_bool())
                                .unwrap_or(false),
                        );
                    })
                },
            )
            .await;
        if emit.is_err() {
            if retry < 2 {
                continue;
            }
            return Err(format!("uploadFile emit 失败: offset={offset}"));
        }
        if wait_for_result(&result, 300).await == Some(true) {
            return Ok(());
        }
    }
    Err(format!("文件上传失败: offset={offset}"))
}

async fn wait_for_result<T: Send>(
    result: &tokio::sync::Mutex<Option<T>>,
    attempts: usize,
) -> Option<T> {
    for _ in 0..attempts {
        if let Some(value) = result.lock().await.take() {
            return Some(value);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
