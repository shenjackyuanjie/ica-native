//! 直接透传给 Bridge 的原始 Socket.IO / 文件管理调用。

use super::context::CommandContext;
use crate::ica::command::emit_ui_event;
use crate::ica::file_manager::call_file_manager;
use crate::ica::{BridgeEvent, ack};
use serde_json::Value as JsonValue;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// 统一交给界面处理本地校验、发送和 ACK 超时，只传错误描述，不传原始请求。
fn socket_api_failure(
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    event: &str,
    message: &str,
) {
    emit_ui_event(
        event_tx,
        bridge_key,
        "commandFailed",
        json!({"kind": "socketApiCall", "event": event, "message": message}),
    );
}

/// 通过当前 Bridge 发送原始事件；等待 ACK 时不阻塞后续聊天命令。
pub async fn socket_api_call(
    ctx: CommandContext<'_>,
    event: String,
    args: Vec<JsonValue>,
    expect_ack: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        ..
    } = ctx;
    // 上游签名为 (request, callback)，不是多个位置参数；没有 callback 不会执行。
    // 只校验公共形状，具体 action 与字段交给 Bridge 校验，避免重复维护动作列表。
    let resource_request = matches!(event.as_str(), "favoritesRequest" | "flashTransferRequest");
    let action = args
        .first()
        .and_then(|request| request.get("action"))
        .and_then(JsonValue::as_str);
    if resource_request
        && (!expect_ack || args.len() != 1 || action.is_none_or(|action| action.trim().is_empty()))
    {
        socket_api_failure(
            event_tx,
            bridge_key,
            &event,
            "需要一个带非空 action 的请求对象，并等待 ACK",
        );
        return;
    }
    if expect_ack {
        // 对齐 Electron：闪传可能递归列目录，收藏 fileUrl 需要额外解析下载凭据。
        // 其他旧事件继续用默认 15 秒，等待超时不重发，避免重复修改/分享。
        let timeout = Duration::from_secs(match (event.as_str(), action) {
            ("flashTransferRequest", _) => 120,
            ("favoritesRequest", Some("fileUrl")) => 35,
            ("favoritesRequest", _) => 20,
            _ => 15,
        });
        match ack::start(client, &event, args, timeout).await {
            Ok(pending) => {
                // 发包仍按主连接命令顺序；只有等回包移到后台，聊天和回执互不阻塞。
                let event_tx = event_tx.clone();
                let bridge_key = bridge_key.to_owned();
                tokio::spawn(async move {
                    match pending.receive().await {
                        Ok(payload) => emit_ui_event(
                            &event_tx,
                            &bridge_key,
                            "socketApiResponse",
                            json!({"event": event, "ack": ack::payload_values(&payload)}),
                        ),
                        Err(error) => {
                            socket_api_failure(&event_tx, &bridge_key, &event, &error.to_string());
                        }
                    }
                });
            }
            Err(error) => socket_api_failure(event_tx, bridge_key, &event, &error.to_string()),
        }
    } else if let Err(error) = client.emit(event.as_str(), args).await {
        socket_api_failure(event_tx, bridge_key, &event, &error.to_string());
    } else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "socketApiResponse",
            json!({"event": event, "sent": true}),
        );
    }
}

/// 向指定 `gin` 的文件管理连接透传调用。
///
/// 结果与错误的投递由 `call_file_manager` 负责，这里只把失败转换成
/// `commandFailed` 事件。
pub async fn file_manager_call(
    ctx: CommandContext<'_>,
    gin: i64,
    event: String,
    args: Vec<JsonValue>,
    expect_ack: bool,
) {
    let CommandContext {
        client,
        event_tx,
        bridge_key,
        socket_url,
        ..
    } = ctx;
    match call_file_manager(
        client, event_tx, bridge_key, socket_url, gin, event, args, expect_ack,
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
                    "kind": "fileManagerCall",
                    "gin": gin,
                    "message": e,
                }),
            );
        }
    }
}
