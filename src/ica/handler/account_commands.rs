//! 账号、验证消息与联系人类命令。

use serde_json::json;

use crate::ica::command::emit_ui_event;

use super::context::CommandContext;
use super::{contacts, history};

pub async fn get_system_msg(ctx: CommandContext<'_>) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::get_system_messages(client, event_tx, bridge_key).await
}

pub async fn fetch_contacts(ctx: CommandContext<'_>, request_id: u64) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    contacts::fetch_contacts(client, event_tx, bridge_key, request_id).await
}

pub async fn set_online_status(ctx: CommandContext<'_>, status: u8) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client.emit("setOnlineStatus", json!(status)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "setOnlineStatus",
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn handle_request(
    ctx: CommandContext<'_>,
    request_type: String,
    flag: String,
    accept: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client
        .emit(
            "handleRequest",
            vec![json!(request_type), json!(flag), json!(accept)],
        )
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "handleRequest",
                "flag": flag,
                "message": e.to_string(),
            }),
        );
    }
}
