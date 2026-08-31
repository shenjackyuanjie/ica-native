//! 会话与聊天分组管理类命令。

use serde_json::json;

use crate::ica::command::emit_ui_event;
use crate::ica::types::RoomId;
use crate::ica::types::room::Room;

use super::context::CommandContext;

pub async fn add_room(ctx: CommandContext<'_>, room: Room) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let room_id = room.room_id;
    if let Err(error) = client.emit("addRoom", json!(room)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "addRoom",
                "roomId": room_id,
                "message": error.to_string(),
            }),
        );
    }
}

pub async fn pin_room(ctx: CommandContext<'_>, room_id: RoomId, pin: bool) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client
        .emit("pinRoom", vec![json!(room_id), json!(pin)])
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "pinRoom",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn remove_chat(ctx: CommandContext<'_>, room_id: RoomId) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client.emit("removeChat", json!(room_id)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "removeChat",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn ignore_chat(ctx: CommandContext<'_>, room_id: RoomId, room_name: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client
        .emit("ignoreChat", json!({"id": room_id, "name": room_name}))
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "ignoreChat",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn remove_ignored_chat(ctx: CommandContext<'_>, room_id: RoomId) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client.emit("removeIgnoredChat", json!(room_id)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "removeIgnoredChat",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn set_room_priority(ctx: CommandContext<'_>, room_id: RoomId, priority: u8) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client
        .emit("setRoomPriority", vec![json!(room_id), json!(priority)])
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "setRoomPriority",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn report_read(ctx: CommandContext<'_>, room_id: RoomId, message_id: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client.emit("reportRead", json!(message_id)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "reportRead",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn add_chat_group(
    ctx: CommandContext<'_>,
    name: String,
    rooms: Vec<RoomId>,
    include_all_personal: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let payload = json!({
        "name": name,
        "rooms": rooms,
        "includeAllPersonal": include_all_personal,
    });
    if let Err(e) = client.emit("addChatGroup", payload).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "addChatGroup",
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn remove_chat_group(ctx: CommandContext<'_>, name: String) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    if let Err(e) = client.emit("removeChatGroup", json!(name)).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "removeChatGroup",
                "message": e.to_string(),
            }),
        );
    }
}

pub async fn update_chat_group(
    ctx: CommandContext<'_>,
    name: String,
    rooms: Vec<RoomId>,
    include_all_personal: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    let payload = json!({
        "name": name,
        "rooms": rooms,
        "includeAllPersonal": include_all_personal,
    });
    if let Err(e) = client
        .emit("updateChatGroup", vec![json!(name), payload])
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "updateChatGroup",
                "message": e.to_string(),
            }),
        );
    }
}
