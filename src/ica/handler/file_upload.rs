use crate::ica::{
    ack::{self, AckError},
    client,
    types::message::{FileAttachment, Mention, ReplyMessage, SendMessage},
};
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

const CHUNK_SIZE: usize = 512 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

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
    let file_hash = upload_file_to_bridge(client, file_name, file_data).await?;
    let mut message = SendMessage::new(content, room_id, reply_to);
    message.set_mentions(&mentions);
    message.file = Some(FileAttachment {
        file_type: file_type.into(),
        path: file_hash,
        size: file_data.len(),
    });
    if !client::send_message(client, &message).await {
        return Err("sendMessage 发送失败".into());
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
    let payload = ack::request(
        client,
        "uploadGroupFile",
        vec![
            json!(file_hash),
            json!(group_id),
            json!(parent_id),
            json!(file_name),
        ],
        Duration::from_secs(10 * 60),
    )
    .await
    .map_err(|error| error.to_string())?;
    let response = ack::payload_first(&payload).unwrap_or(JsonValue::Null);
    if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(response
            .get("error")
            .and_then(JsonValue::as_str)
            .unwrap_or("群文件上传失败")
            .into())
    }
}

async fn upload_file_to_bridge(
    client: &Client,
    file_name: &str,
    file_data: &[u8],
) -> Result<String, String> {
    upload_file_with(file_name, file_data, |event, args, timeout| {
        ack::request(client, event, args, timeout)
    })
    .await
}

/// 普通附件与群文件共享相同的断点续传规则，只在全部分片成功后交出文件引用。
async fn upload_file_with<F, Fut>(
    file_name: &str,
    file_data: &[u8],
    mut request: F,
) -> Result<String, String>
where
    F: FnMut(&'static str, Vec<JsonValue>, Duration) -> Fut,
    Fut: Future<Output = Result<Payload, AckError>>,
{
    let file_hash = sha256(file_data);
    let payload = request(
        "requestUpload",
        vec![json!(file_name), json!(file_hash), json!(file_data.len())],
        UPLOAD_TIMEOUT,
    )
    .await
    .map_err(|error| error.to_string())?;
    let (all_success, uploaded) =
        parse_upload_status(&payload).map_err(|error| error.to_string())?;
    if !all_success {
        for (index, chunk) in file_data.chunks(CHUNK_SIZE).enumerate() {
            let offset = index * CHUNK_SIZE;
            if !uploaded.contains(&offset) {
                upload_chunk_with(&file_hash, offset, chunk, &mut request).await?;
            }
        }
    }
    Ok(file_hash)
}

fn parse_upload_status(payload: &Payload) -> Result<(bool, Vec<usize>), AckError> {
    let value = ack::payload_first(payload).unwrap_or(JsonValue::Null);
    let all_success = value
        .get("allSuccess")
        .and_then(JsonValue::as_bool)
        .ok_or_else(|| AckError::invalid("requestUpload", "响应缺少布尔 allSuccess 字段"))?;
    let uploaded = match value.get("uploaded") {
        None => Vec::new(),
        Some(JsonValue::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| {
                        AckError::invalid("requestUpload", "uploaded 包含无效的分片偏移")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(AckError::invalid(
                "requestUpload",
                "uploaded 必须是分片偏移数组",
            ));
        }
    };
    Ok((all_success, uploaded))
}

async fn upload_chunk_with<F, Fut>(
    file_hash: &str,
    offset: usize,
    chunk: &[u8],
    request: &mut F,
) -> Result<(), String>
where
    F: FnMut(&'static str, Vec<JsonValue>, Duration) -> Fut,
    Fut: Future<Output = Result<Payload, AckError>>,
{
    let chunk_hash = sha256(chunk);
    let mut last_error = String::new();
    // 保留既有的幂等分片重试次数；最终消息提交不在此重试循环中。
    for _ in 0..3 {
        let result = request(
            "uploadFile",
            vec![
                json!(file_hash),
                json!(offset),
                json!(chunk.to_vec()),
                json!(chunk_hash),
            ],
            UPLOAD_TIMEOUT,
        )
        .await;
        match result {
            Ok(payload) => match ack::payload_first(&payload).and_then(|v| v.as_bool()) {
                Some(true) => return Ok(()),
                Some(false) => last_error = "Bridge 拒绝分片".into(),
                None => {
                    last_error = AckError::invalid("uploadFile", "分片响应必须是布尔值").to_string()
                }
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(format!("文件分片上传失败: offset={offset}, {last_error}"))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
