//! 文件分块上传与群文件上传。
//!
//! 大文件不整包发送：先算 SHA-256 并调 `requestUpload` 询问 Bridge 已有哪些分片，
//! 只补传缺失部分（断点续传），全部就绪后 Bridge 凭文件 hash 引用内容。
//! 之后按用途分两条路径：
//! - 随消息发送：把 hash 写进 `SendMessage.file` 再走普通发送；
//! - 群文件：再调一次 `uploadGroupFile` 提交到指定目录。
//!
//! 分片大小、重试次数与超时集中在本文件顶部的常量里。

#[cfg(test)]
#[path = "tests/file_upload.rs"]
mod tests;

use crate::ica::{
    ack::{self, AckError},
    client,
    types::message::{FileAttachment, Mention, ReplyMessage, SendMessage},
};
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

/// 单个分片的大小：512 KiB。
///
/// 与 Bridge 侧约定一致；偏移量按该值整除计算，改动会让已上传文件无法续传。
const CHUNK_SIZE: usize = 512 * 1024;
/// 分片相关请求（requestUpload / uploadFile）的超时。
///
/// 比群文件提交短：分片小、失败可以重传；群文件提交见 `upload_group_file` 的独立超时。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// 上传文件并把引用作为附件随消息发出。
///
/// 先分块上传拿到 hash，再构造带 `file` 附件的消息走普通发送通道；
/// 任一步失败都会返回错误文本，由调用方转成 `commandFailed` 事件。
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

/// 上传文件到群文件目录。
///
/// `parent_id` 为空字符串表示群文件根目录。Bridge 的提交可能较慢，
/// 因此单独给了 10 分钟超时；结果里的 `ok` 为 `false` 时取 `error` 文本回报。
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
    // 只认布尔 true；缺字段或返回 false 都算失败，并优先带回 Bridge 的错误文本。
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

/// 把"发 ACK 请求"固定成闭包后转交 `upload_file_with`。
///
/// 这样续传逻辑本身不依赖具体 socket，测试可以注入假请求。
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
///
/// 流程：算整包 hash -> `requestUpload` 询问进度 -> 只补传缺失分片 -> 返回 hash。
/// Bridge 报告 `allSuccess` 时跳过全部上传，此时返回的 hash 已指向服务端已有内容。
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
        // 参数顺序是 Bridge 的约定：文件名、整包 hash、总长度。
        vec![json!(file_name), json!(file_hash), json!(file_data.len())],
        UPLOAD_TIMEOUT,
    )
    .await
    .map_err(|error| error.to_string())?;
    let (all_success, uploaded) =
        parse_upload_status(&payload).map_err(|error| error.to_string())?;
    if !all_success {
        // 按偏移量而不是下标比对：Bridge 只报"已有分片的偏移"，
        // 中途改过 CHUNK_SIZE 时下标会对不上，偏移不会。
        for (index, chunk) in file_data.chunks(CHUNK_SIZE).enumerate() {
            let offset = index * CHUNK_SIZE;
            if !uploaded.contains(&offset) {
                upload_chunk_with(&file_hash, offset, chunk, &mut request).await?;
            }
        }
    }
    Ok(file_hash)
}

/// 解析 `requestUpload` 的响应：是否已全部就绪 + 已有分片的偏移列表。
///
/// `allSuccess` 缺失即视为协议不符；`uploaded` 可以省略（等价于空列表），
/// 但出现了就必须是偏移数组，否则无法判断哪些分片还要传。
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

/// 上传单个分片，失败最多重试 3 次。
///
/// 分片上传是幂等的（Bridge 按 hash + offset 去重），因此可以安全重试；
/// 成功条件是回调第一个参数为布尔 `true`。最终消息提交不在此重试范围内，
/// 避免同一条消息被重复投递。
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

/// 计算十六进制 SHA-256，用作文件与分片的引用标识。
fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
