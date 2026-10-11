//! 消息发送、撤回与可见性变更类命令。

#[cfg(test)]
#[path = "tests/message_commands.rs"]
mod tests;

use super::context::CommandContext;
use super::{build_multi_image_message, send_message, upload_and_send_file};
use crate::ica::client;
use crate::ica::command::emit_ui_event;
use crate::ica::types::RoomId;
use crate::ica::types::message::{DeleteMessage, Mention, ReplyMessage, SendMessage};
use serde_json::Value as JsonValue;
use serde_json::json;

/// 撤回必须发生在新消息收到 HTTP 202 之后，失败或结果不确定时不撤回、不重试。
async fn submit_edit_then_recall<S, R, F>(submit: S, recall: F) -> Result<bool, String>
where
    S: std::future::Future<Output = Result<(), String>>,
    F: FnOnce() -> R,
    R: std::future::Future<Output = bool>,
{
    submit.await?;
    Ok(recall().await)
}

/// 编辑重发一条自己发送的消息。
///
/// 先提交新消息（保留原消息的 media，新增图片只能追加），Bridge 经 HTTP
/// 接收后再请求撤回旧消息；提交失败不撤回、不重试，草稿由界面保留。结果以
/// `editSendResult` 上报，`accepted` 只表示 Bridge 已接收，不等于 QQ 已送达。
pub async fn edit_and_resend_message(
    ctx: CommandContext<'_>,
    message: SendMessage,
    images: Vec<(String, std::sync::Arc<[u8]>)>,
    message_id: String,
) {
    let room_id = message.room_id;
    let submit = async {
        let encoded = tokio::task::spawn_blocking(move || {
            let mut message = message;
            // 保留原消息的 media，新增图片只能追加，不能 set_img 覆盖。
            for (mime, bytes) in images {
                message.add_img(&bytes, &mime);
            }
            message
        })
        .await
        .map_err(|_| "编辑消息编码失败".to_string())?;
        // 纯文本也走 HTTP，以获得明确的 Bridge 接收确认。
        let token = super::request_send_token(ctx.client).await?;
        super::http_send_message(&ctx.http.send, ctx.api_base_url, &token, &encoded).await
    };
    let result = submit_edit_then_recall(submit, || async {
        client::delete_message(ctx.client, &DeleteMessage::new(room_id, message_id.clone())).await
    })
    .await;
    let (accepted, error) = match result {
        Ok(true) => (true, None),
        Ok(false) => (
            true,
            Some("Bridge 已接收新消息，但旧消息撤回请求失败，请检查记录后手动撤回".to_string()),
        ),
        Err(error) => (
            false,
            Some(format!(
                "编辑重发提交失败：{error}。旧消息未请求撤回，草稿已保留；网络超时时请先检查记录，避免重复发送"
            )),
        ),
    };
    emit_ui_event(
        ctx.event_tx,
        ctx.bridge_key,
        "editSendResult",
        json!({
            "roomId": room_id, "messageId": message_id, "accepted": accepted, "error": error,
            "message": if accepted { "Bridge 已接收新消息，并已尝试请求撤回旧消息；接收不等于 QQ 已送达，撤回也以 Bridge 回报为准" } else { "编辑重发草稿已保留" },
        }),
    );
}

/// 发送普通聊天消息，转交 `send_message` 统一走 HTTP 提交流程。
pub async fn send_chat_message(ctx: CommandContext<'_>, message: SendMessage) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        api_base_url,
        http,
        ..
    } = ctx;
    send_message(message, client, event_tx, bridge_key, api_base_url, http).await;
}

/// 发送单图消息；图片编码在阻塞线程上完成，失败时上报 `commandFailed`。
pub async fn send_image_message(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    content: String,
    reply_to: Option<ReplyMessage>,
    mentions: Vec<Mention>,
    image_type: String,
    image_data: std::sync::Arc<[u8]>,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        api_base_url,
        http,
        ..
    } = ctx;
    let encoded_message = tokio::task::spawn_blocking(move || {
        let mut message = SendMessage::new(content, room_id, reply_to);
        message.set_mentions(&mentions);
        message.set_img(image_data.as_ref(), &image_type, false);
        message
    })
    .await;
    match encoded_message {
        Ok(message) => {
            send_message(message, client, event_tx, bridge_key, api_base_url, http).await;
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

/// 发送多图消息；图片编码在阻塞线程上完成，失败时上报 `commandFailed`。
pub async fn send_multi_image_message(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    content: String,
    reply_to: Option<ReplyMessage>,
    mentions: Vec<Mention>,
    images: Vec<(String, std::sync::Arc<[u8]>)>,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        api_base_url,
        http,
        ..
    } = ctx;
    let encoded_message = tokio::task::spawn_blocking(move || {
        build_multi_image_message(room_id, &content, reply_to.as_ref(), &mentions, &images)
    })
    .await;
    match encoded_message {
        Ok(message) => {
            send_message(message, client, event_tx, bridge_key, api_base_url, http).await;
        }
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

/// 发送原始（raw）消息，内容由调用方按 Bridge 协议自行拼装。
pub async fn send_raw_message(ctx: CommandContext<'_>, room_id: RoomId, content: JsonValue) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
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
                "message": "sendRawMessage 失败",
            }),
        );
    }
}

/// 对一条消息执行隐藏操作。
pub async fn hide_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
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

/// 取消一条消息的隐藏。
pub async fn reveal_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
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

/// 请求 Bridge 重发（renew）一条消息。
pub async fn renew_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
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

/// 删除（撤回）一条消息；失败时上报 `commandFailed`。
pub async fn delete_message(ctx: CommandContext<'_>, message: DeleteMessage) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let message_id = message.message_id.clone();
    if !client::delete_message(client, &message).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "deleteMessage",
                "messageId": message_id,
                "message": "deleteMessage 失败",
            }),
        );
    }
}

/// 随消息一起发送的文件附件。
///
/// 文件名、类型与内容总是同进同退，单独铺开会让参数表超过 clippy 的上限，
/// 打包成结构体后调用处也更难传错顺序。
pub struct OutgoingFile {
    pub name: String,
    pub file_type: String,
    pub data: std::sync::Arc<[u8]>,
}

/// 发送文件消息；先上传再发送，失败时上报 `commandFailed`。
pub async fn send_file_message(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    content: String,
    reply_to: Option<ReplyMessage>,
    mentions: Vec<Mention>,
    file: OutgoingFile,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    match upload_and_send_file(
        client,
        room_id,
        content,
        reply_to,
        mentions,
        &file.name,
        &file.file_type,
        &file.data,
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
