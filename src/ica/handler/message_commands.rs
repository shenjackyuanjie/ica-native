//! 消息发送、撤回与可见性变更类命令。

use serde_json::Value as JsonValue;
use serde_json::json;

use crate::ica::client;
use crate::ica::command::emit_ui_event;
use crate::ica::types::RoomId;
use crate::ica::types::message::{DeleteMessage, Mention, ReplyMessage, SendMessage};

use super::context::CommandContext;
use super::{build_multi_image_message, send_message, upload_and_send_file};

pub(super) async fn send_chat_message(ctx: CommandContext<'_>, message: SendMessage) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        api_base_url,
        ..
    } = ctx;
    send_message(message, client, event_tx, bridge_key, api_base_url).await;
}

pub(super) async fn send_image_message(
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

pub(super) async fn send_multi_image_message(
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

pub(super) async fn send_raw_message(ctx: CommandContext<'_>, room_id: RoomId, content: JsonValue) {
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

pub(super) async fn hide_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
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

pub(super) async fn reveal_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
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

pub(super) async fn renew_message(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
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

pub(super) async fn delete_message(ctx: CommandContext<'_>, message: DeleteMessage) {
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
pub(super) struct OutgoingFile {
    pub name: String,
    pub file_type: String,
    pub data: std::sync::Arc<[u8]>,
}

pub(super) async fn send_file_message(
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
