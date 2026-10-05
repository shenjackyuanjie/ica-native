use std::time::Duration;

use futures_util::future::BoxFuture;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;
use tokio::sync::mpsc;

use super::ack::{self, payload_values as ack_payload_values};
use super::command::emit_ui_event;

#[allow(clippy::too_many_arguments)]
pub async fn call_file_manager(
    main_client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    socket_url: &str,
    gin: i64,
    event: String,
    args: Vec<JsonValue>,
    expect_ack: bool,
) -> Result<(), String> {
    let token = request_gfs_token(main_client, gin).await?;
    let (auth_tx, mut auth_rx) = mpsc::unbounded_channel::<Result<JsonValue, String>>();
    let auth_bridge_key = bridge_key.to_string();

    let mut builder =
        ClientBuilder::new(socket_url.to_string()).transport_type(TransportType::Websocket);
    {
        let token = token.clone();
        let auth_bridge_key = auth_bridge_key.clone();
        builder = builder.on(
            "requireAuth",
            move |_payload: Payload, client: Client| -> BoxFuture<'static, ()> {
                let token = token.clone();
                let auth_bridge_key = auth_bridge_key.clone();
                Box::pin(async move {
                    if let Err(e) = client
                        .emit("auth", vec![json!(token), json!("fileMgr")])
                        .await
                    {
                        tracing::warn!(bridge = %auth_bridge_key, socket = "file_manager", error = %e, "发送 fileMgr 认证事件失败");
                    }
                })
            },
        );
    }
    {
        let auth_tx = auth_tx.clone();
        builder = builder.on(
            "authSucceed",
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let auth_tx = auth_tx.clone();
                Box::pin(async move {
                    let _ = auth_tx.send(Ok(JsonValue::Array(ack_payload_values(&payload))));
                })
            },
        );
    }
    {
        let auth_tx = auth_tx.clone();
        builder = builder.on(
            "authFailed",
            move |_payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let auth_tx = auth_tx.clone();
                Box::pin(async move {
                    let _ = auth_tx.send(Err("fileMgr 鉴权失败".to_string()));
                })
            },
        );
    }

    let file_client = builder
        .connect()
        .await
        .map_err(|e| format!("fileMgr 连接失败: {}", e))?;

    let result = async {
        let auth_result = tokio::time::timeout(Duration::from_secs(10), auth_rx.recv())
            .await
            .map_err(|_| "fileMgr 鉴权超时".to_string())?
            .ok_or_else(|| "fileMgr 鉴权通道关闭".to_string())?;

        let auth_payload = match auth_result {
            Ok(payload) => payload,
            Err(e) => {
                return Err(e);
            }
        };

        if expect_ack {
            let ack = emit_file_manager_with_ack(&file_client, &event, args).await?;
            emit_ui_event(
                event_tx,
                bridge_key,
                "fileManagerResponse",
                json!({
                    "gin": gin,
                    "event": event,
                    "auth": auth_payload,
                    "ack": ack,
                }),
            );
        } else {
            file_client
                .emit(event.as_str(), args)
                .await
                .map_err(|e| format!("fileMgr {} 发送失败: {}", event, e))?;

            emit_ui_event(
                event_tx,
                bridge_key,
                "fileManagerResponse",
                json!({
                    "gin": gin,
                    "event": event,
                    "auth": auth_payload,
                    "sent": true,
                }),
            );
        }

        Ok(())
    }
    .await;

    if let Err(e) = file_client.disconnect().await {
        tracing::warn!(error = %e, "断开 fileMgr 连接失败");
    }
    result
}

async fn emit_file_manager_with_ack(
    file_client: &Client,
    event: &str,
    args: Vec<JsonValue>,
) -> Result<Vec<JsonValue>, String> {
    let payload = ack::request(file_client, event, args, Duration::from_secs(30))
        .await
        .map_err(|error| error.to_string())?;
    // 不再把包含文件信息的原始 ACK 输出到日志。
    Ok(ack_payload_values(&payload))
}

async fn request_gfs_token(client: &Client, gin: i64) -> Result<String, String> {
    let payload = ack::request(
        client,
        "requestGfsToken",
        vec![json!(gin)],
        Duration::from_secs(15),
    )
    .await
    .map_err(|error| error.to_string())?;
    ack::nonempty_string(&payload, "requestGfsToken")
        .map_err(|_| "requestGfsToken 返回空 token".to_string())
}
