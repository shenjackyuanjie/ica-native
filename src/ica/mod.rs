use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};

use crate::StopGetter;
use crate::cfg::IcaBridge;
use crate::ica::types::{RoomId, message::{DeleteMessage, SendMessage}};

use futures_util::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub mod client;
pub mod types;

/// icalingua 客户端的兼容版本号
pub const ICA_PROTOCOL_VERSION: &str = "2.12.28";
/// 自动重连最多尝试 5 次。
const MAX_RECONNECT_ATTEMPTS: usize = 5;
/// 指数退避的等待时间上限，避免失败时越等越久。
const MAX_RECONNECT_BACKOFF_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
enum ConnectionSignal {
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum IcaCommand {
    FetchMessages(RoomId),
    GetSystemMsg,
    PinRoom { room_id: RoomId, pin: bool },
    HideMessage { room_id: RoomId, message_id: String },
    RevealMessage { room_id: RoomId, message_id: String },
    SendMessage(SendMessage),
    SendRawMessage { room_id: RoomId, content: JsonValue },
    DeleteMessage(DeleteMessage),
    HandleRequest {
        request_type: String,
        flag: String,
        accept: bool,
    },
}

#[derive(Debug, Clone)]
pub struct IcaClient {
    pub bridge_key: String,
    pub command_tx: UnboundedSender<IcaCommand>,
}

fn emit_ui_event(
    tx: &Option<UnboundedSender<JsonValue>>,
    bridge_id: &str,
    event_name: &'static str,
    payload: JsonValue,
) {
    let obj = json!({
        "bridge": bridge_id,
        "event": event_name,
        "payload": payload,
    });

    if let Some(tx) = tx {
        let _ = tx.send(obj);
    } else {
        tracing::info!("{}: {}", event_name, obj);
    }
}

fn payload_to_json(payload: &Payload) -> JsonValue {
    match payload {
        Payload::Text(values) => JsonValue::Array(values.clone()),
        Payload::Binary(bytes) => json!(bytes.to_vec()),
        _ => JsonValue::Null,
    }
}

fn json_preview(value: &JsonValue, max_chars: usize) -> String {
    let raw = value.to_string();
    if raw.len() > max_chars {
        format!("{}...", &raw[..max_chars])
    } else {
        raw
    }
}

fn unwrap_singleton_array_layers(mut value: JsonValue) -> JsonValue {
    loop {
        match value {
            JsonValue::Array(mut values)
                if values.len() == 1 && matches!(values.first(), Some(JsonValue::Array(_))) =>
            {
                value = values.remove(0);
            }
            _ => return value,
        }
    }
}

fn reconnect_delay(attempt: usize) -> Duration {
    let exp = attempt.saturating_sub(1).min(5) as u32;
    let seconds = (1_u64 << exp).min(MAX_RECONNECT_BACKOFF_SECS);
    Duration::from_secs(seconds)
}

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
    event_tx: Option<UnboundedSender<JsonValue>>,
    mut command_rx: UnboundedReceiver<IcaCommand>,
) -> anyhow::Result<()> {
    if !bridge_cfg.enable {
        return Ok(());
    }

    // 这里的值都固定绑定到当前 bridge，后面注册回调时直接捕获它们。
    let bridge_key = if bridge_cfg.name.is_empty() {
        bridge_cfg.url.clone()
    } else {
        bridge_cfg.name.clone()
    };
    let private_key = bridge_cfg.private_key.clone();
    let mut stop_alrm = stop_alrm;
    let mut reconnect_attempt = 0_usize;

    // 外层循环负责“连接 -> 断线/失败 -> 等待 -> 重连”。
    'connect: loop {
        let connect_event = if reconnect_attempt == 0 {
            "socketConnecting"
        } else {
            "socketReconnecting"
        };
        emit_ui_event(
            &event_tx,
            &bridge_key,
            connect_event,
            json!({
                "attempt": reconnect_attempt,
                "maxAttempts": MAX_RECONNECT_ATTEMPTS,
            }),
        );

        let start_connect_time = std::time::Instant::now();
        // 只把“连接断了，需要重连”这一类内部控制信号发回主循环。
        let (connection_signal_tx, mut connection_signal_rx) =
            tokio::sync::mpsc::unbounded_channel::<ConnectionSignal>();

        let mut builder =
            ClientBuilder::new(bridge_cfg.url.clone()).transport_type(TransportType::Websocket);

        {
            // 鉴权回调在注册时就和当前 bridge 的私钥绑死，避免串 key。
            let sign_callback = client::sign_callback(private_key.clone());
            let bridge_id = bridge_key.clone();
            let tx = event_tx.clone();
            builder = builder.on(
                "requireAuth",
                move |payload: Payload, client: Client| -> BoxFuture<'static, ()> {
                    let bridge_id = bridge_id.clone();
                    let tx = tx.clone();
                    let sign_future = sign_callback(payload, client);
                    Box::pin(async move {
                        emit_ui_event(&tx, &bridge_id, "requireAuth", JsonValue::Null);
                        sign_future.await;
                    })
                },
            );
        }

        // 普通事件统一转成 GUI 侧能消费的 JSON 包。
        let make_event_cb = |event_name: &'static str| {
            let bridge_id = bridge_key.clone();
            let tx = event_tx.clone();
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let bridge_id = bridge_id.clone();
                let tx = tx.clone();
                Box::pin(async move {
                    let payload_json = payload_to_json(&payload);
                    emit_ui_event(&tx, &bridge_id, event_name, payload_json);
                })
            }
        };

        {
            // disconnect 只负责通知主循环“这条连接已经断了”，
            // 真正的 UI 状态更新和重连调度由主循环统一处理。
            let signal_tx = connection_signal_tx.clone();
            builder = builder.on(
                "disconnect",
                move |_payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                    let signal_tx = signal_tx.clone();
                    Box::pin(async move {
                        let _ = signal_tx.send(ConnectionSignal::Disconnected);
                    })
                },
            );
        }

        builder = builder.on("authSucceed", make_event_cb("authSucceed"));
        builder = builder.on("authFailed", make_event_cb("authFailed"));
        builder = builder.on("message", make_event_cb("message"));
        builder = builder.on("onlineData", make_event_cb("onlineData"));
        builder = builder.on("addMessage", make_event_cb("addMessage"));
        builder = builder.on("deleteMessage", make_event_cb("deleteMessage"));
        builder = builder.on("hideMessage", make_event_cb("hideMessage"));
        builder = builder.on("revealMessage", make_event_cb("revealMessage"));
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
                reconnect_attempt = 0;
                emit_ui_event(&event_tx, &bridge_key, "socketConnected", JsonValue::Null);
                client
            }
            Err(e) => {
                tracing::error!("socketio connect failed: {}", e);
                emit_ui_event(
                    &event_tx,
                    &bridge_key,
                    "socketConnectFailed",
                    json!({ "message": e.to_string() }),
                );

                reconnect_attempt += 1;
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    emit_ui_event(
                        &event_tx,
                        &bridge_key,
                        "socketReconnectExhausted",
                        json!({
                            "message": format!(
                                "连接失败次数已达到上限({})，停止重试",
                                MAX_RECONNECT_ATTEMPTS
                            ),
                        }),
                    );
                    return Ok(());
                }

                let delay = reconnect_delay(reconnect_attempt);
                emit_ui_event(
                    &event_tx,
                    &bridge_key,
                    "socketRetryScheduled",
                    json!({
                        "message": format!(
                            "{} 秒后进行第 {}/{} 次重连",
                            delay.as_secs(),
                            reconnect_attempt,
                            MAX_RECONNECT_ATTEMPTS
                        ),
                    }),
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {
                        continue 'connect;
                    }
                    _ = &mut stop_alrm => {
                        return Ok(());
                    }
                }
            }
        };

        // 内层循环只负责“已连接状态下”的命令处理和断线监听。
        let should_reconnect = loop {
            tokio::select! {
                _ = &mut stop_alrm => {
                    match client.disconnect().await {
                        Ok(_) => {
                            tracing::info!("Disconnected: {}", bridge_key);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to disconnect {}: {}", bridge_key, e);
                        }
                    }
                    emit_ui_event(&event_tx, &bridge_key, "socketDisconnected", JsonValue::Null);
                    return Ok(());
                }
                Some(signal) = connection_signal_rx.recv() => {
                    match signal {
                        ConnectionSignal::Disconnected => {
                            break true;
                        }
                    }
                }
                Some(command) = command_rx.recv() => {
                    match command {
                        IcaCommand::FetchMessages(room_id) => {
                            let timeout = Duration::from_secs(10);
                            let ack_received = Arc::new(AtomicBool::new(false));
                            let ack_received_cb = ack_received.clone();
                            let tx = event_tx.clone();
                            let bridge_id = bridge_key.clone();

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
                                            let raw_payload = payload_to_json(&payload);
                                            let messages = raw_payload
                                                .as_array()
                                                .and_then(|values| values.first())
                                                .cloned()
                                                .map(unwrap_singleton_array_layers)
                                                .unwrap_or_else(|| JsonValue::Array(Vec::new()));
                                            if !messages.is_array() {
                                                tracing::warn!(
                                                    "fetchMessages ack format unexpected: bridge={} room_id={} raw={}",
                                                    bridge_id,
                                                    room_id,
                                                    json_preview(&raw_payload, 512)
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
                                    let bridge_id = bridge_key.clone();
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
                                        &event_tx,
                                        &bridge_key,
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
                        IcaCommand::GetSystemMsg => {
                            let timeout = Duration::from_secs(10);
                            let ack_received = Arc::new(AtomicBool::new(false));
                            let ack_received_cb = ack_received.clone();
                            let tx = event_tx.clone();
                            let bridge_id = bridge_key.clone();

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
                                                payload_to_json(&payload),
                                            );
                                        })
                                    },
                                )
                                .await;

                            match result {
                                Ok(()) => {
                                    let tx = event_tx.clone();
                                    let bridge_id = bridge_key.clone();
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
                                        &event_tx,
                                        &bridge_key,
                                        "commandFailed",
                                        json!({
                                            "kind": "getSystemMsg",
                                            "message": e.to_string(),
                                        }),
                                    );
                                }
                            }
                        }
                        IcaCommand::SendMessage(message) => {
                            let room_id = message.room_id;
                            if !client::send_message(&client, &message).await {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "sendMessage",
                                        "roomId": room_id,
                                        "message": "sendMessage failed",
                                    }),
                                );
                            }
                        }
                        IcaCommand::SendRawMessage { room_id, content } => {
                            let payload = json!({
                                "messageType": "raw",
                                "roomId": room_id,
                                "content": content,
                            });
                            if !client::send_string_message(&client, &payload).await {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "sendRawMessage",
                                        "roomId": room_id,
                                        "message": "sendRawMessage failed",
                                    }),
                                );
                            }
                        }
                        IcaCommand::PinRoom { room_id, pin } => {
                            if let Err(e) = client.emit("pinRoom", vec![json!(room_id), json!(pin)]).await {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "pinRoom",
                                        "roomId": room_id,
                                        "message": e.to_string(),
                                    }),
                                );
                            }
                        }
                        IcaCommand::HideMessage { room_id, message_id } => {
                            if let Err(e) = client
                                .emit("hideMessage", vec![json!(room_id), json!(message_id.clone())])
                                .await
                            {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "hideMessage",
                                        "roomId": room_id,
                                        "messageId": message_id,
                                        "message": e.to_string(),
                                    }),
                                );
                            }
                        }
                        IcaCommand::RevealMessage { room_id, message_id } => {
                            if let Err(e) = client
                                .emit("revealMessage", vec![json!(room_id), json!(message_id.clone())])
                                .await
                            {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "revealMessage",
                                        "roomId": room_id,
                                        "messageId": message_id,
                                        "message": e.to_string(),
                                    }),
                                );
                            }
                        }
                        IcaCommand::DeleteMessage(message) => {
                            let message_id = message.message_id.clone();
                            if !client::delete_message(&client, &message).await {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "deleteMessage",
                                        "messageId": message_id,
                                        "message": "deleteMessage failed",
                                    }),
                                );
                            }
                        }
                        IcaCommand::HandleRequest {
                            request_type,
                            flag,
                            accept,
                        } => {
                            if let Err(e) = client
                                .emit(
                                    "handleRequest",
                                    vec![json!(request_type), json!(flag), json!(accept)],
                                )
                                .await
                            {
                                emit_ui_event(
                                    &event_tx,
                                    &bridge_key,
                                    "commandFailed",
                                    json!({
                                        "kind": "handleRequest",
                                        "flag": flag,
                                        "message": e.to_string(),
                                    }),
                                );
                            }
                        }
                    }
                }
            }
        };

        if should_reconnect {
            reconnect_attempt += 1;
            emit_ui_event(
                &event_tx,
                &bridge_key,
                "socketDisconnected",
                json!({ "message": "连接已断开" }),
            );

            if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                emit_ui_event(
                    &event_tx,
                    &bridge_key,
                    "socketReconnectExhausted",
                    json!({
                        "message": format!(
                            "断线重试次数已达到上限({})，停止重连",
                            MAX_RECONNECT_ATTEMPTS
                        ),
                    }),
                );
                return Ok(());
            }

            let delay = reconnect_delay(reconnect_attempt);
            emit_ui_event(
                &event_tx,
                &bridge_key,
                "socketRetryScheduled",
                json!({
                    "message": format!(
                        "{} 秒后进行第 {}/{} 次重连",
                        delay.as_secs(),
                        reconnect_attempt,
                        MAX_RECONNECT_ATTEMPTS
                    ),
                }),
            );

            let _ = client.disconnect().await;

            tokio::select! {
                _ = tokio::time::sleep(delay) => {
                    continue 'connect;
                }
                _ = &mut stop_alrm => {
                    return Ok(());
                }
            }
        }
    }
}
