//! oicq 语音使用内嵌 media + 一次性 HTTP token，绝不走普通文件分块上传。

use std::sync::Arc;

use serde_json::json;

use crate::ica::command::emit_ui_event;

use super::{
    context::CommandContext,
    http_send::{http_send_message, request_send_token},
    message_payload::build_voice_message,
};

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
