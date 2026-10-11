//! 历史消息、群成员与消息检索类命令。

use super::context::CommandContext;
use super::{ack_payload_values, history, normalize_ack_list};
use crate::ica::command::emit_ui_event;
use crate::ica::types::RoomId;
use futures_util::future::BoxFuture;
use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::time::Duration;

/// 拉取会话当前片段消息，转交 `history` 模块。
pub async fn fetch_messages(ctx: CommandContext<'_>, room_id: RoomId) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::fetch_messages(client, event_tx, bridge_key, room_id).await
}

/// 从最新位置拉取漫游历史，转交 `history` 模块。
pub async fn fetch_latest_history(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    current_loaded_messages: usize,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::fetch_latest_history(
        client,
        event_tx,
        bridge_key,
        room_id,
        current_loaded_messages,
    )
    .await
}

/// 向前分页拉取更早消息，转交 `history` 模块。
pub async fn fetch_older_messages(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    before_time: i64,
    before_id: String,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::fetch_older_messages(
        client,
        event_tx,
        bridge_key,
        room_id,
        before_time,
        before_id,
    )
    .await
}

/// 拉取群成员列表，转交 `history` 模块。
pub async fn fetch_group_members(ctx: CommandContext<'_>, room_id: RoomId) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::fetch_group_members(client, event_tx, bridge_key, room_id).await
}

/// 分页检索指定发送者的发言，转交 `history` 模块。
pub async fn fetch_messages_by_sender(
    ctx: CommandContext<'_>,
    request_id: u64,
    room_id: RoomId,
    sender_id: i64,
    offset: usize,
    snapshot_time: i64,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::fetch_messages_by_sender(
        client,
        event_tx,
        bridge_key,
        history::FetchMessagesBySenderRequest {
            request_id,
            room_id,
            sender_id,
            offset,
            snapshot_time,
        },
    )
    .await
}

/// 在会话内按关键词检索消息（15 秒 ACK），结果以 `searchMessagesResponse` 返回。
pub async fn search_messages(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    keyword: String,
    offset: usize,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    let keyword_for_event = keyword.clone();
    if let Err(e) = client
        .emit_with_ack(
            "searchMessages",
            vec![
                json!(room_id),
                json!(keyword),
                json!(offset),
                JsonValue::Null,
                JsonValue::Null,
                JsonValue::Null,
            ],
            Duration::from_secs(15),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                let keyword = keyword_for_event.clone();
                Box::pin(async move {
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "searchMessagesResponse",
                        json!({
                            "roomId": room_id,
                            "keyword": keyword,
                            "offset": offset,
                            "messages": normalize_ack_list(ack_payload_values(&payload)),
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
                "kind": "searchMessages",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

/// 通知 Bridge 停止拉取历史；失败只记警告，不影响界面。
pub async fn stop_fetching_history(ctx: CommandContext<'_>) {
    let CommandContext { client, .. } = ctx;
    if let Err(e) = client.emit("stopFetchingHistory", json!(null)).await {
        tracing::warn!(error = %e, "发送 stopFetchingHistory 事件失败");
    }
}
