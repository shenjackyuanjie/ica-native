//! 账号、验证消息与联系人类命令。
//!
//! 本模块属于 Bridge 命令处理层（`ica::handler`），把界面发来的账号类请求
//! 转成 Socket.IO 调用：多数命令只是拆箱 `CommandContext` 后委托给
//! `contacts` / `history` 子模块，失败时通过 `commandFailed` 事件统一上报，
//! 不在本模块内重试。

use super::context::CommandContext;
use super::{contacts, history};
use crate::ica::command::emit_ui_event;
use serde_json::json;

/// 拉取系统验证消息（如加好友申请），实际拉取由 `history` 模块完成。
pub async fn get_system_msg(ctx: CommandContext<'_>) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    history::get_system_messages(client, event_tx, bridge_key).await
}

/// 拉取联系人列表；`request_id` 原样回传，供界面匹配自己发起的请求。
pub async fn fetch_contacts(ctx: CommandContext<'_>, request_id: u64) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    contacts::fetch_contacts(client, event_tx, bridge_key, request_id).await
}

/// 设置账号在线状态；失败时上报 `commandFailed`，不向调用方抛错。
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

/// 处理加好友、加群等验证请求。
///
/// `request_type` 与 `flag` 标识待处理的请求，`accept` 表示同意还是拒绝；
/// 失败时上报 `commandFailed` 并带上 `flag` 便于界面定位。
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
