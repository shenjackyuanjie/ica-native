//! 消息发送、撤回与可见性变更类命令。

use serde_json::Value as JsonValue;
use serde_json::json;

use crate::ica::client;
use crate::ica::command::emit_ui_event;
use crate::ica::types::RoomId;
use crate::ica::types::message::{DeleteMessage, Mention, ReplyMessage, SendMessage};

use super::context::CommandContext;
use super::{build_multi_image_message, send_message, upload_and_send_file};

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
        super::http_send_message(ctx.api_base_url, &token, &encoded).await
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

pub async fn send_chat_message(ctx: CommandContext<'_>, message: SendMessage) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        api_base_url,
        ..
    } = ctx;
    send_message(message, client, event_tx, bridge_key, api_base_url).await;
}

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
        ..
    } = ctx;
    let encoded_message = tokio::task::spawn_blocking(move || {
        build_multi_image_message(room_id, &content, reply_to.as_ref(), &mentions, &images)
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
                "kind": "sendMultiImageMessage",
                "roomId": room_id,
                "message": format!("图片编码任务失败: {e}"),
            }),
        ),
    }
}

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

#[cfg(test)]
mod edit_tests {
    use super::submit_edit_then_recall;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn edit_recall_only_runs_after_confirmed_submission() {
        let order = Arc::new(Mutex::new(Vec::new()));
        for accepted in [false, true] {
            order.lock().unwrap().clear();
            let submit_order = order.clone();
            let recall_order = order.clone();
            let result = submit_edit_then_recall(
                async move {
                    submit_order.lock().unwrap().push("submit");
                    if accepted {
                        Ok(())
                    } else {
                        Err("超时".into())
                    }
                },
                || async move {
                    recall_order.lock().unwrap().push("recall");
                    true
                },
            )
            .await;
            if accepted {
                assert_eq!(result, Ok(true));
                assert_eq!(*order.lock().unwrap(), ["submit", "recall"]);
            } else {
                assert!(result.is_err());
                assert_eq!(*order.lock().unwrap(), ["submit"]);
            }
        }
    }

    #[tokio::test]
    async fn recall_failure_does_not_turn_accepted_submission_into_retryable_failure() {
        assert_eq!(
            submit_edit_then_recall(async { Ok(()) }, || async { false }).await,
            Ok(false)
        );
    }
}
