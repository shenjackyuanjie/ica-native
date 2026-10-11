//! 群文件 / 文件管理（fileMgr）专用连接。
//!
//! fileMgr 事件不走主 socket：Bridge 要求先用主连接上的 `requestGfsToken` 换取
//! 一次性 token，再以该 token 建立独立 socket 并在 `requireAuth` 里声明
//! `["<token>", "fileMgr"]` 完成鉴权。因此每次调用都是一条"取 token -> 建连 ->
//! 鉴权 -> 发事件 -> 断开"的短生命周期连接，不长期持有。
//!
//! 结果统一以 `fileManagerResponse` 事件回传 GUI，字段按是否需要 ACK 二选一：
//! `expect_ack` 为真时带 `ack`，否则带 `sent: true`。

use super::ack::{self, payload_values as ack_payload_values};
use super::command::emit_ui_event;
use crate::ica::event::BridgeEvent;
use futures_util::future::BoxFuture;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;

/// 在 fileMgr 连接上执行一次文件管理事件。
///
/// 参数：
/// - `main_client`：主 socket，仅用于换取 gfs token；
/// - `socket_url`：fileMgr 连接的地址（与主连接同源）；
/// - `gin`：目标群号，随响应回传以便 GUI 对应；
/// - `expect_ack`：是否需要 Bridge 回报执行结果。
///
/// 成功时返回 `Ok(())`，实际结果已通过 `fileManagerResponse` 发出；
/// 取 token、建连、鉴权或发送任一环节失败时返回错误文本，且同样会断开连接。
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
    // 鉴权结果用 channel 从事件回调传到本函数：回调是 'static 的，
    // 无法直接把结果交还给这里的栈帧，只能借 channel 中转。
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
        // 鉴权单独设 10 秒超时：它只验证 token，比真正的文件操作短得多，
        // 卡住时能更快把错误暴露给 GUI。
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

    // 无论成功失败都断开这条一次性连接；断开失败只记警告，
    // 不影响已经拿到的结果，因此不覆盖 result。
    if let Err(e) = file_client.disconnect().await {
        tracing::warn!(error = %e, "断开 fileMgr 连接失败");
    }
    result
}

/// 在 fileMgr 连接上发事件并等待 ACK，返回剥掉包裹后的参数数组。
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

/// 从主连接换取 fileMgr 的一次性 token。
///
/// 15 秒超时足以覆盖一次普通 ACK；Bridge 返回空 token 时按失败处理，
/// 因为空 token 无法用于后续鉴权。
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
