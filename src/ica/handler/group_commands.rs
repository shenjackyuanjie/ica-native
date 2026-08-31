//! 群管理、群公告与群文件类命令。

use std::time::Duration;

use serde_json::json;

use crate::ica::client;
use crate::ica::command::{GROUP_BAN_MAX_DURATION, emit_ui_event};
use crate::ica::types::RoomId;

use super::context::CommandContext;
use super::{announcement, file_upload, history};

pub(super) async fn fetch_group_announcements(
    ctx: CommandContext<'_>,
    request_id: u64,
    room_id: RoomId,
    bkn: i64,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    announcement::fetch_group_announcements(client, event_tx, bridge_key, request_id, room_id, bkn)
        .await
}

pub(super) async fn set_group_ban(
    ctx: CommandContext<'_>,
    room_id: RoomId,
    target_id: i64,
    duration: u64,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let Some(group_id) = room_id
        .checked_abs()
        .filter(|_| room_id < 0 && duration <= GROUP_BAN_MAX_DURATION)
    else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "setGroupBan",
                "roomId": room_id,
                "message": "群禁言参数无效",
            }),
        );
        return;
    };

    if let Err(error) = client
        .emit(
            "setGroupBan",
            vec![json!(group_id), json!(target_id), json!(duration)],
        )
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "setGroupBan",
                "roomId": room_id,
                "message": error.to_string(),
            }),
        );
    } else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "groupBanRequested",
            json!({
                "roomId": room_id,
                "targetId": target_id,
                "duration": duration,
            }),
        );

        let client = client.clone();
        let event_tx = event_tx.clone();
        let bridge_key = bridge_key.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            history::fetch_group_members(&client, &event_tx, &bridge_key, room_id).await;
        });
    }
}

pub(super) async fn upload_group_file(
    ctx: CommandContext<'_>,
    group_id: i64,
    parent_id: String,
    file_name: String,
    file_data: std::sync::Arc<[u8]>,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    match file_upload::upload_group_file(client, group_id, &parent_id, &file_name, &file_data).await
    {
        Ok(()) => emit_ui_event(
            event_tx,
            bridge_key,
            "groupFileUploadCompleted",
            json!({ "groupId": group_id, "fileName": file_name }),
        ),
        Err(error) => emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({ "kind": "uploadGroupFile", "roomId": -group_id, "message": error }),
        ),
    }
}

pub(super) async fn send_group_sign(ctx: CommandContext<'_>, room_id: RoomId) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if !client::send_room_sign_in(client, room_id).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "sendGroupSign",
                "roomId": room_id,
                "message": "sendGroupSign 失败",
            }),
        );
    }
}

pub(super) async fn send_group_poke(ctx: CommandContext<'_>, room_id: RoomId, target_id: i64) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if !client::send_poke(client, room_id, target_id).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "sendGroupPoke",
                "roomId": room_id,
                "targetId": target_id,
                "message": "sendGroupPoke 失败",
            }),
        );
    }
}
