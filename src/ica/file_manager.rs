use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;
use tokio::sync::{Mutex, mpsc};

use super::command::{emit_ui_event, json_preview};

fn ack_payload_values(payload: &Payload) -> Vec<JsonValue> {
    match payload {
        Payload::Text(values) => {
            if let Some(JsonValue::Array(args)) = values.first()
                && values.len() == 1
            {
                return args.clone();
            }
            values.clone()
        }
        Payload::Binary(bytes) => vec![json!(bytes.to_vec())],
        _ => Vec::new(),
    }
}

fn ack_payload_first(payload: &Payload) -> Option<JsonValue> {
    ack_payload_values(payload).into_iter().next()
}

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

    let auth_result = tokio::time::timeout(Duration::from_secs(10), auth_rx.recv())
        .await
        .map_err(|_| "fileMgr 鉴权超时".to_string())?
        .ok_or_else(|| "fileMgr 鉴权通道关闭".to_string())?;

    let auth_payload = match auth_result {
        Ok(payload) => payload,
        Err(e) => {
            let _ = file_client.disconnect().await;
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

    if let Err(e) = file_client.disconnect().await {
        tracing::warn!(error = %e, "断开 fileMgr 连接失败");
    }
    Ok(())
}

async fn emit_file_manager_with_ack(
    file_client: &Client,
    event: &str,
    args: Vec<JsonValue>,
) -> Result<Vec<JsonValue>, String> {
    let (ack_tx, mut ack_rx) = mpsc::unbounded_channel::<Vec<JsonValue>>();
    let event_for_cb = event.to_string();
    file_client
        .emit_with_ack(
            event,
            args,
            Duration::from_secs(30),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let ack_tx = ack_tx.clone();
                let event = event_for_cb.clone();
                Box::pin(async move {
                    let values = ack_payload_values(&payload);
                    tracing::debug!(
                        "收到 fileMgr ACK: event={} ack={}",
                        event,
                        json_preview(&JsonValue::Array(values.clone()), 512)
                    );
                    let _ = ack_tx.send(values);
                })
            },
        )
        .await
        .map_err(|e| format!("fileMgr {} 发送失败: {}", event, e))?;

    tokio::time::timeout(Duration::from_secs(30), ack_rx.recv())
        .await
        .map_err(|_| format!("fileMgr {} 请求超时", event))?
        .ok_or_else(|| format!("fileMgr {} ack 通道关闭", event))
}

async fn request_gfs_token(client: &Client, gin: i64) -> Result<String, String> {
    let token = Arc::new(Mutex::new(None::<String>));
    let token_cb = token.clone();

    client
        .emit_with_ack(
            "requestGfsToken",
            vec![json!(gin)],
            Duration::from_secs(15),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let token = token_cb.clone();
                Box::pin(async move {
                    let token_str = ack_payload_first(&payload)
                        .and_then(|v| v.as_str().map(ToString::to_string))
                        .unwrap_or_default();
                    *token.lock().await = Some(token_str);
                })
            },
        )
        .await
        .map_err(|e| format!("requestGfsToken 发送失败: {}", e))?;

    let started = std::time::Instant::now();
    loop {
        if let Some(token) = token.lock().await.take() {
            if token.is_empty() {
                return Err("requestGfsToken 返回空 token".to_string());
            }
            return Ok(token);
        }
        if started.elapsed() > Duration::from_secs(15) {
            return Err("requestGfsToken 超时".to_string());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
