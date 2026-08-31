//! 合并转发的查看与发送类命令。

use serde_json::Value as JsonValue;

use crate::ica::types::RoomId;

use super::context::CommandContext;
use super::forward;

pub async fn fetch_forward_messages(
    ctx: CommandContext<'_>,
    request_id: u64,
    res_id: String,
    file_name: Option<String>,
    fallback_res_id: Option<String>,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    forward::fetch_forward_messages(
        client,
        event_tx,
        bridge_key,
        request_id,
        res_id,
        file_name,
        fallback_res_id,
    )
    .await;
}

pub async fn send_merged_forward(
    ctx: CommandContext<'_>,
    nodes: Vec<JsonValue>,
    direct_message: bool,
    origin: Option<i64>,
    target_room_id: RoomId,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    forward::send_merged_forward(
        client,
        event_tx,
        bridge_key,
        nodes,
        direct_message,
        origin,
        target_room_id,
    )
    .await;
}
