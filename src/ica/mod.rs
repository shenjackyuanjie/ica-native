use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};

use crate::StopGetter;
use crate::cfg::IcaBridge;

use futures_util::future::BoxFuture;
use std::sync::OnceLock;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::UnboundedSender;

pub mod client;
pub mod events;
pub mod types;

/// icalingua 客户端的兼容版本号
pub const ICA_PROTOCOL_VERSION: &str = "2.12.28";

#[derive(Debug, Clone)]
pub struct IcaClient {
    pub bridge_key: String,
}

/// 全局兼容的 UI sender（保持向后兼容，events 模块可能会使用）
/// 每个 bridge 也会把它自己的 sender 通过 closure 捕获并优先使用
pub static UI_SENDER: OnceLock<UnboundedSender<JsonValue>> = OnceLock::new();

/// 启动 socketio client，并把服务端事件用 unbounded channel 发回 GUI 主线程
///
/// 参数说明：
/*
 - stop_alrm: 停止信号（oneshot receiver 的别名 StopGetter）
 - bridge_cfg: 当前 bridge 的配置（包含 url 与 private_key）
 - ui_sender: 可选的 mpsc 发送者，用于把收到的事件发回 GUI（发送 serde_json::Value）
*/
pub async fn main(
    stop_alrm: StopGetter,
    bridge_cfg: &IcaBridge,
    ui_sender: Option<UnboundedSender<JsonValue>>,
) -> anyhow::Result<()> {
    if !bridge_cfg.enable {
        return Ok(());
    }

    // per-bridge local values
    let bridge_key = if bridge_cfg.name.is_empty() {
        bridge_cfg.url.clone()
    } else {
        bridge_cfg.name.clone()
    };
    let private_key = bridge_cfg.private_key.clone();
    let local_tx = ui_sender.clone(); // Option<UnboundedSender<JsonValue>>

    let start_connect_time = std::time::Instant::now();

    // build client with per-bridge closures that capture private_key and local_tx
    let mut builder =
        ClientBuilder::new(bridge_cfg.url.clone()).transport_type(TransportType::Websocket);

    // requireAuth: use sign_with_key implemented in client module (accepts key param)
    {
        let pk = private_key.clone();
        builder = builder.on(
            "requireAuth",
            move |payload: Payload, client: Client| -> BoxFuture<'static, ()> {
                let pk = pk.clone();
                Box::pin(async move {
                    // client::sign_with_key should be implemented to accept (Payload, Client, String)
                    client::sign_with_key(payload, client, pk).await;
                })
            },
        );
    }

    // helper to create per-bridge event callback closures
    let make_event_cb = |event_name: &'static str| {
        let bridge_id = bridge_key.clone();
        let tx = local_tx.clone();
        move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
            let bridge_id = bridge_id.clone();
            let tx = tx.clone();
            Box::pin(async move {
                let payload_json = match &payload {
                    Payload::Text(vs) => serde_json::Value::Array(vs.clone()),
                    Payload::Binary(b) => json!(b.to_vec()),
                    _ => serde_json::Value::Null,
                };
                let obj = json!({
                    "bridge": bridge_id,
                    "event": event_name,
                    "payload": payload_json,
                });
                if let Some(tx) = tx {
                    // ignore send error (UI might be closed)
                    let _ = tx.send(obj);
                } else {
                    tracing::info!("{}: {}", event_name, obj);
                }
            })
        }
    };

    // register commonly used events
    builder = builder.on("authSucceed", make_event_cb("authSucceed"));
    builder = builder.on("authFailed", make_event_cb("authFailed"));
    builder = builder.on("onlineData", make_event_cb("onlineData"));
    builder = builder.on("addMessage", make_event_cb("addMessage"));
    builder = builder.on("deleteMessage", make_event_cb("deleteMessage"));
    builder = builder.on("setAllRooms", make_event_cb("setAllRooms"));
    builder = builder.on("setMessages", make_event_cb("setMessages"));
    builder = builder.on("handleRequest", make_event_cb("handleRequest"));

    let client = match builder.connect().await {
        Ok(client) => {
            tracing::info!(
                "{}",
                format!(
                    "socketio connected time: {:?}",
                    start_connect_time.elapsed()
                )
            );
            client
        }
        Err(e) => {
            tracing::error!("socketio connect failed: {}", e);
            return Err(e.into());
        }
    };

    // 等待停止信号
    stop_alrm.await.ok();

    match client.disconnect().await {
        Ok(_) => {
            tracing::info!("Disconnected: {}", bridge_key);
        }
        Err(e) => {
            tracing::warn!("Failed to disconnect {}: {}", bridge_key, e);
        }
    }

    Ok(())
}
