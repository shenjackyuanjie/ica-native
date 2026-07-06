use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use super::{ack_payload_first, ack_payload_values};
use crate::ica::command::{emit_ui_event, json_preview};

pub(super) async fn fetch_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<JsonValue>>,
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
                    let ack_values = ack_payload_values(&payload);
                    let messages = ack_values
                        .first()
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
                    if !messages.is_array() {
                        tracing::warn!(
                            "fetchMessages ack format unexpected: bridge={} room_id={} raw={}",
                            bridge_id,
                            room_id,
                            json_preview(&JsonValue::Array(ack_values), 512)
                        );
                    }

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
    event_tx: &Option<UnboundedSender<JsonValue>>,
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
                    let ack_values = ack_payload_values(&payload);
                    let messages = ack_values
                        .first()
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new()));

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
    event_tx: &Option<UnboundedSender<JsonValue>>,
    bridge_key: &str,
    room_id: i64,
) {
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    if let Err(e) = client
        .emit_with_ack(
            "getGroupMembers",
            vec![json!(room_id.abs())],
            Duration::from_secs(15),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "groupMembersResponse",
                        json!({
                            "roomId": room_id,
                            "members": ack_payload_values(&payload),
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
                "kind": "fetchGroupMembers",
                "roomId": room_id,
                "message": e.to_string(),
            }),
        );
    }
}

pub(super) async fn get_system_messages(
    client: &Client,
    event_tx: &Option<UnboundedSender<JsonValue>>,
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
