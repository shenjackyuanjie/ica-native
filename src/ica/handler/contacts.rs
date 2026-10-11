#[cfg(test)]
#[path = "tests/contacts.rs"]
mod tests;

use super::{ack_payload_values, normalize_ack_list};
use crate::ica::{command::emit_ui_event, event::BridgeEvent};
use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

const CONTACTS_TIMEOUT: Duration = Duration::from_secs(15);

/// 联系人接口的 ACK 可能把列表作为唯一参数再包一层；UI 侧始终接收联系人对象数组。
fn contact_ack_items(payload: &Payload) -> serde_json::Value {
    normalize_ack_list(ack_payload_values(payload))
}

async fn fetch_contact_part(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    part: &'static str,
    event: &'static str,
) {
    let ack_received = Arc::new(AtomicBool::new(false));
    let ack_received_cb = ack_received.clone();
    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();

    let result = client
        .emit_with_ack(
            event,
            Vec::<serde_json::Value>::new(),
            CONTACTS_TIMEOUT,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let ack_received = ack_received_cb.clone();
                let tx = tx.clone();
                let bridge_id = bridge_id.clone();
                Box::pin(async move {
                    ack_received.store(true, Ordering::SeqCst);
                    emit_ui_event(
                        &tx,
                        &bridge_id,
                        "contactsPartResponse",
                        json!({
                            "requestId": request_id,
                            "part": part,
                            "items": contact_ack_items(&payload),
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
            "contactsPartFailed",
            json!({
                "requestId": request_id,
                "part": part,
                "message": error.to_string(),
            }),
        );
        return;
    }

    let tx = event_tx.clone();
    let bridge_id = bridge_key.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(CONTACTS_TIMEOUT).await;
        if !ack_received.load(Ordering::SeqCst) {
            emit_ui_event(
                &tx,
                &bridge_id,
                "contactsPartFailed",
                json!({
                    "requestId": request_id,
                    "part": part,
                    "message": format!("{event} 请求超时"),
                }),
            );
        }
    });
}

pub async fn fetch_contacts(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
) {
    fetch_contact_part(
        client,
        event_tx,
        bridge_key,
        request_id,
        "friends",
        "getFriendsFallback",
    )
    .await;
    fetch_contact_part(
        client,
        event_tx,
        bridge_key,
        request_id,
        "groups",
        "getGroups",
    )
    .await;
}
