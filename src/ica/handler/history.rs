use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{ack_payload_first, ack_payload_values, normalize_ack_list};
use crate::ica::command::emit_ui_event;

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

pub(super) async fn fetch_latest_history(
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

pub(super) async fn fetch_messages(
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
            vec![json!(room_id), json!(0)],
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
            tracing::warn!("fetchMessages failed for {}: {}", bridge_key, e);
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

pub(super) async fn fetch_older_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
    offset: usize,
) {
    let timeout = Duration::from_secs(10);
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();

    let result = client
        .emit_with_ack(
            "fetchMessages",
            vec![json!(room_id), json!(offset)],
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
        tracing::warn!("fetchOlderMessages failed for {}: {}", bridge_key, e);
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

pub(super) async fn fetch_group_members(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    room_id: i64,
) {
    let timeout = Duration::from_secs(15);
    let completed = Arc::new(AtomicBool::new(false));
    let completed_callback = completed.clone();
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    let timeout_tx = event_tx.clone();
    let timeout_bridge_id = bridge_key.to_string();
    let started_at = Instant::now();
    tracing::debug!(bridge = bridge_key, room_id, "fetchGroupMembers emitted");
    match client
        .emit_with_ack(
            "getGroupMembers",
            vec![json!(room_id.abs())],
            timeout,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                let completed = completed_callback.clone();
                Box::pin(async move {
                    if completed.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let members = normalize_ack_list(ack_payload_values(&payload));
                    let member_count = members.as_array().map_or(0, Vec::len);
                    tracing::debug!(
                        bridge = bridge_id,
                        room_id,
                        member_count,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "fetchGroupMembers acknowledged"
                    );

                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "groupMembersResponse",
                        json!({
                            "roomId": room_id,
                            "members": members,
                        }),
                    );
                })
            },
        )
        .await
    {
        Ok(()) => {
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                if !completed.swap(true, Ordering::AcqRel) {
                    emit_ui_event(
                        &timeout_tx,
                        &timeout_bridge_id,
                        "commandFailed",
                        json!({
                            "kind": "fetchGroupMembers",
                            "roomId": room_id,
                            "message": "群成员列表 ACK 等待超时",
                        }),
                    );
                }
            });
        }
        Err(e) => {
            completed.store(true, Ordering::Release);
            tracing::warn!(bridge = bridge_key, room_id, error = %e, "fetchGroupMembers failed");
            emit_ui_event(
                event_tx,
                bridge_key,
                "commandFailed",
                json!({
                    "kind": "fetchGroupMembers",
                    "roomId": room_id,
                    "message": e.to_string(),
                }),
            );
        }
    }
}

pub(super) async fn get_system_messages(
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

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::json;

    use super::{latest_history_message_id, normalize_ack_list};

    #[test]
    fn ack_list_accepts_flat_and_nested_socket_payloads() {
        assert_eq!(
            normalize_ack_list(vec![json!({"id": 1})]),
            json!([{"id": 1}])
        );
        assert_eq!(
            normalize_ack_list(vec![json!([{"id": 1}, {"id": 2}])]),
            json!([{"id": 1}, {"id": 2}])
        );
    }

    #[test]
    fn latest_history_id_uses_legacy_private_and_group_shapes() {
        let private = STANDARD
            .decode(latest_history_message_id(0x0102_0304).unwrap())
            .unwrap();
        let group = STANDARD
            .decode(latest_history_message_id(-0x0102_0304).unwrap())
            .unwrap();

        assert_eq!(private.len(), 17);
        assert_eq!(group.len(), 21);
        assert_eq!(&private[..4], &[1, 2, 3, 4]);
        assert_eq!(&group[..4], &[1, 2, 3, 4]);
        assert!(private[4..].iter().all(|byte| *byte == 0));
        assert!(group[4..].iter().all(|byte| *byte == 0));
    }
}
