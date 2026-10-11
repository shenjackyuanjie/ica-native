//! 历史消息、群成员与系统消息的拉取。
//!
//! 与联系人、转发一样采用“带 ACK 的 Socket.IO 请求 + UI 事件回报”的模式；
//! 部分接口（如群成员）只等待 ACK 注册、把实际等待放进后台任务，避免阻塞
//! 同一 Bridge 上的后续聊天命令。

#[cfg(test)]
#[path = "tests/history.rs"]
mod tests;

use super::{ack_payload_first, ack_payload_values, normalize_ack_list};
use crate::ica::command::emit_ui_event;
use crate::ica::event::BridgeEvent;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

/// “按发送者检索消息”的请求参数。
///
/// 打包成结构体是为了避免长参数表在调用处传错顺序；`snapshot_time` 为界面
/// 发起检索时的快照时间，`offset` 用于分页。
pub struct FetchMessagesBySenderRequest {
    /// 界面发起的请求 ID，结果原样回传用于匹配。
    pub request_id: u64,
    /// 目标会话；群聊为负群号，私聊为正 QQ 号。
    pub room_id: i64,
    /// 要检索的发送者 QQ 号。
    pub sender_id: i64,
    /// 分页偏移，跳过已有条数。
    pub offset: usize,
    /// 检索快照时间（毫秒时间戳），作为跨请求一致的分页基线。
    pub snapshot_time: i64,
}

/// 构造 Icalingua++ 用于“从最新位置拉取历史”的占位消息 ID。
///
/// bridge 的 `fetchHistory` 接口沿用了旧客户端协议：私聊 ID 是 17 字节，群聊 ID
/// 是 21 字节，前四个字节按大端序写入 QQ 号或群号，其余字段保持为零。bridge 会
/// 以该位置为起点向协议端请求漫游记录，完成后再广播一份新的 `setMessages`。
fn latest_history_message_id(room_id: i64) -> Option<String> {
    let target_id = u32::try_from(room_id.unsigned_abs()).ok()?;
    let mut bytes = vec![0_u8; if room_id < 0 { 21 } else { 17 }];
    bytes[..4].copy_from_slice(&target_id.to_be_bytes());
    Some(STANDARD.encode(bytes))
}

/// 从最新位置拉取漫游历史。
///
/// 用占位消息 ID 告诉 Bridge 起点，`current_loaded_messages` 为当前已加载
/// 条数；协议端完成后会广播新的 `setMessages`。
pub async fn fetch_latest_history(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
    current_loaded_messages: usize,
) {
    let Some(message_id) = latest_history_message_id(room_id) else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "fetchHistory",
                "roomId": room_id,
                "message": "房间 ID 超出 fetchHistory 协议支持范围",
            }),
        );
        return;
    };

    if let Err(e) = client
        .emit(
            "fetchHistory",
            vec![
                json!(message_id),
                json!(room_id),
                json!(current_loaded_messages),
            ],
        )
        .await
    {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "fetchHistory",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

/// 拉取会话当前片段的消息；结果以 `setMessages` 事件整体替换界面列表。
///
/// ACK 超时（10 秒）未回会补发 `commandFailed`。
pub async fn fetch_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
) {
    let timeout = Duration::from_secs(10);
    let ack_received = Arc::new(AtomicBool::new(false));
    let ack_received_cb = ack_received.clone();
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();

    let result = client
        .emit_with_ack(
            "fetchMessages",
            vec![json!(room_id), json!({})],
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let ack_received = ack_received_cb.clone();
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    ack_received.store(true, Ordering::SeqCst);
                    let messages = normalize_ack_list(ack_payload_values(&payload));

                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "setMessages",
                        json!([
                            {
                                "roomId": room_id,
                                "messages": messages,
                            }
                        ]),
                    );
                })
            },
        )
        .await;

    match result {
        Ok(()) => {
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                if !ack_received.load(Ordering::SeqCst) {
                    tracing::warn!(
                        "fetchMessages timeout: bridge={} room_id={}",
                        bridge_id,
                        room_id
                    );
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "commandFailed",
                        json!({
                            "kind": "fetchMessages",
                            "roomId": room_id,
                            "message": "fetchMessages 请求超时",
                        }),
                    );
                }
            });
        }
        Err(e) => {
            tracing::warn!(bridge = bridge_key, error = %e, "发送 fetchMessages 请求失败");
            emit_ui_event(
                event_tx,
                bridge_key,
                "commandFailed",
                json!({
                    "kind": "fetchMessages",
                    "roomId": room_id,
                    "message": e.to_string(),
                }),
            );
        }
    }
}

/// 按 `before_time` / `before_id` 向前分页拉取更早的消息。
///
/// 结果以 `appendOlderMessages` 前插；即使请求失败也发空列表，让界面重置
/// “加载中”状态。
pub async fn fetch_older_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
    before_time: i64,
    before_id: String,
) {
    let timeout = Duration::from_secs(10);
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();

    let result = client
        .emit_with_ack(
            "fetchMessages",
            vec![
                json!(room_id),
                json!({ "before": { "time": before_time, "id": before_id } }),
            ],
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    let messages = normalize_ack_list(ack_payload_values(&payload));

                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "appendOlderMessages",
                        json!([
                            {
                                "roomId": room_id,
                                "messages": messages,
                            }
                        ]),
                    );
                })
            },
        )
        .await;

    if let Err(e) = result {
        tracing::warn!(bridge = bridge_key, error = %e, "发送 fetchOlderMessages 请求失败");
        // 通知 UI 加载完成（即使失败也要重置加载状态）
        emit_ui_event(
            event_tx,
            bridge_key,
            "appendOlderMessages",
            json!([
                {
                    "roomId": room_id,
                    "messages": [],
                }
            ]),
        );
    }
}

/// 拉取群成员列表。
///
/// ACK 等待放在后台任务里，不阻塞同一 Bridge 的后续聊天命令。
pub async fn fetch_group_members(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
) {
    let Some(group_id) = room_id.checked_abs() else {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({"kind":"fetchGroupMembers", "roomId":room_id, "message":"群号超出支持范围"}),
        );
        return;
    };
    let pending = crate::ica::ack::start(
        client,
        "getGroupMembers",
        vec![json!(group_id)],
        Duration::from_secs(15),
    )
    .await;
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    // 只等待注册，不把 15 秒 ACK 等待放进串行聊天命令分派。
    tokio::spawn(async move {
        let result = match pending {
            Ok(pending) => pending.receive().await,
            Err(error) => Err(error),
        };
        match result {
            Ok(payload) => emit_ui_event(
                &tx,
                &bridge_id,
                "groupMembersResponse",
                json!({
                    "roomId":room_id, "members":normalize_ack_list(ack_payload_values(&payload)),
                }),
            ),
            Err(error) => emit_ui_event(
                &tx,
                &bridge_id,
                "commandFailed",
                json!({
                    "kind":"fetchGroupMembers", "roomId":room_id, "message":error.to_string(),
                }),
            ),
        }
    });
}

/// 分页检索指定发送者在会话内的发言，结果以 `memberHistoryResponse` 返回。
pub async fn fetch_messages_by_sender(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request: FetchMessagesBySenderRequest,
) {
    let FetchMessagesBySenderRequest {
        request_id,
        room_id,
        sender_id,
        offset,
        snapshot_time,
    } = request;
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    let result = client
        .emit_with_ack(
            "fetchMessagesBySender",
            vec![
                json!(room_id),
                json!(sender_id),
                json!(offset),
                json!(snapshot_time),
            ],
            Duration::from_secs(20),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "memberHistoryResponse",
                        json!({
                            "requestId": request_id,
                            "roomId": room_id,
                            "senderId": sender_id,
                            "offset": offset,
                            "messages": normalize_ack_list(ack_payload_values(&payload)),
                        }),
                    );
                })
            },
        )
        .await;
    if let Err(error) = result {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "fetchMessagesBySender",
                "roomId": room_id,
                "message": error.to_string(),
            }),
        );
    }
}

/// 拉取系统验证消息；结果以 `setSystemMessages` 交给界面，超时会上报失败。
pub async fn get_system_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
) {
    let timeout = Duration::from_secs(10);
    let ack_received = Arc::new(AtomicBool::new(false));
    let ack_received_cb = ack_received.clone();
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();

    let result = client
        .emit_with_ack(
            "getSystemMsg",
            Vec::<JsonValue>::new(),
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let ack_received = ack_received_cb.clone();
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    ack_received.store(true, Ordering::SeqCst);
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "setSystemMessages",
                        ack_payload_first(&payload).unwrap_or(JsonValue::Null),
                    );
                })
            },
        )
        .await;

    match result {
        Ok(()) => {
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                if !ack_received.load(Ordering::SeqCst) {
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "commandFailed",
                        json!({
                            "kind": "getSystemMsg",
                            "message": "getSystemMsg 请求超时",
                        }),
                    );
                }
            });
        }
        Err(e) => {
            emit_ui_event(
                event_tx,
                bridge_key,
                "commandFailed",
                json!({
                    "kind": "getSystemMsg",
                    "message": e.to_string(),
                }),
            );
        }
    }
}
