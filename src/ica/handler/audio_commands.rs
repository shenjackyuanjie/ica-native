//! oicq 语音使用内嵌 media + 一次性 HTTP token，绝不走普通文件分块上传。

use super::{
    context::CommandContext,
    http_send::{http_send_message, request_send_token},
    message_payload::build_voice_message,
};
use crate::ica::command::emit_ui_event;
use serde_json::json;
use std::sync::Arc;

/// 发送语音消息。
///
/// 语音不经过普通文件分块上传：先在阻塞线程上把 WAV 编码成内嵌 media 的
/// 消息体，再申请一次性 HTTP token 提交。`request_id` 与 `room_id` 原样用于
/// 回执匹配；无论成败都会发出 `voiceSendResult`，其中 `accepted` 只表示
/// Bridge 已接收，不等于 QQ 侧最终送达。
pub async fn send_voice_message(
    ctx: CommandContext<'_>,
    request_id: u64,
    room_id: i64,
    audio_data: Arc<[u8]>,
) {
    // WAV 校验及 base64 构造不占用 Socket.IO 异步执行线程。
    let encoded =
        tokio::task::spawn_blocking(move || build_voice_message(room_id, &audio_data)).await;
    let result = match encoded {
        Ok(Ok(message)) => match request_send_token(ctx.client).await {
            Ok(token) => {
                http_send_message(&ctx.http.send, ctx.api_base_url, &token, &message).await
            }
            Err(error) => Err(error),
        },
        Ok(Err(error)) => Err(error),
        Err(_) => Err("语音编码任务失败".to_string()),
    };
    let accepted = result.is_ok();
    let message = match result {
        Ok(()) => "Bridge 已接收语音；最终发送状态以 Bridge 回报为准".to_string(),
        Err(error) => format!("语音提交失败：{error}。若网络超时，请先检查消息记录，避免重复发送"),
    };
    emit_ui_event(
        ctx.event_tx,
        ctx.bridge_key,
        "voiceSendResult",
        json!({
            "requestId": request_id,
            "roomId": room_id,
            "accepted": accepted,
            "message": message,
        }),
    );
}
