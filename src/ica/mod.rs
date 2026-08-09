use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Payload, TransportType};

use crate::StopGetter;
use crate::config::IcaBridge;

use futures_util::future::BoxFuture;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub mod client;
mod command;
pub mod event;
mod file_manager;
mod handler;
pub use command::{BridgeHandle, GROUP_BAN_MAX_DURATION, ICA_PROTOCOL_VERSION, IcaCommand};
use command::{
    ConnectionSignal, MAX_RECONNECT_ATTEMPTS, emit_ui_event, payload_to_json, reconnect_delay,
};
pub use event::{BridgeEvent, BridgeEventKind};
pub mod types;

/// 启动 socketio client，并把服务端事件用 unbounded channel 发回 GUI 主线程
///
/// 参数说明：
/*
 - stop_alrm: 停止信号（oneshot receiver 的别名 StopGetter）
 - bridge_cfg: 当前 bridge 的配置（包含 url 与 private_key）
 - event_tx: 类型化 bridge 事件发送端
*/
pub async fn run_bridge(
    stop_alrm: StopGetter,
    bridge_cfg: &IcaBridge,
    event_tx: UnboundedSender<BridgeEvent>,
    mut command_rx: UnboundedReceiver<IcaCommand>,
) -> anyhow::Result<()> {
    if !bridge_cfg.enable {
        return Ok(());
    }
    let event_tx = Some(event_tx);

    // 这里的值都固定绑定到当前 bridge，后面注册回调时直接捕获它们。
    let bridge_key = if bridge_cfg.name.is_empty() {
        bridge_cfg.url.clone()
    } else {
        bridge_cfg.name.clone()
    };
    let http_api_url = {
        if let Some(rest) = bridge_cfg.url.strip_prefix("ws://") {
            format!("http://{}", rest)
        } else if let Some(rest) = bridge_cfg.url.strip_prefix("wss://") {
            format!("https://{}", rest)
        } else {
            bridge_cfg.url.clone()
        }
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
        builder = builder.on("setAllChatGroups", make_event_cb("setAllChatGroups"));
        builder = builder.on("setMessages", make_event_cb("setMessages"));
        builder = builder.on("handleRequest", make_event_cb("handleRequest"));
        builder = builder.on("sendAddRequest", make_event_cb("sendAddRequest"));
        builder = builder.on("updateRoom", make_event_cb("updateRoom"));
        builder = builder.on("syncRead", make_event_cb("syncRead"));
        builder = builder.on("renewMessage", make_event_cb("renewMessage"));
        builder = builder.on("renewMessageURL", make_event_cb("renewMessageURL"));
        builder = builder.on("setOnline", make_event_cb("setOnline"));
        builder = builder.on("setOffline", make_event_cb("setOffline"));
        builder = builder.on("setShutUp", make_event_cb("setShutUp"));
        builder = builder.on("messageSuccess", make_event_cb("messageSuccess"));
        builder = builder.on("messageError", make_event_cb("messageError"));
        builder = builder.on("addMessageText", make_event_cb("addMessageText"));
        builder = builder.on("notifyMessage", make_event_cb("notifyMessage"));
        builder = builder.on("closeLoading", make_event_cb("closeLoading"));
        builder = builder.on("notifyError", make_event_cb("notifyError"));
        builder = builder.on("requestSetup", make_event_cb("requestSetup"));
        builder = builder.on("fatal", make_event_cb("fatal"));
        builder = builder.on("login-verify", make_event_cb("login-verify"));
        builder = builder.on("login-qrcodeLogin", make_event_cb("login-qrcodeLogin"));
        builder = builder.on("login-smsCodeVerify", make_event_cb("login-smsCodeVerify"));
        builder = builder.on("login-error", make_event_cb("login-error"));
        builder = builder.on("login-slider", make_event_cb("login-slider"));

        let client = match builder.connect().await {
            Ok(client) => {
                tracing::info!(
                    "{}",
                    format!("Socket.IO 连接耗时: {:?}", start_connect_time.elapsed())
                );
                reconnect_attempt = 0;
                emit_ui_event(&event_tx, &bridge_key, "socketConnected", JsonValue::Null);
                client
            }
            Err(e) => {
                tracing::error!(error = %e, "Socket.IO 连接失败");
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
                            tracing::info!(bridge = %bridge_key, "Socket.IO bridge 已断开");
                        }
                        Err(e) => {
                            tracing::warn!(bridge = %bridge_key, error = %e, "断开 Socket.IO bridge 失败");
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
                    handler::handle_command(
                        command,
                        &client,
                        &event_tx,
                        &bridge_key,
                        &bridge_cfg.url,
                        &http_api_url,
                    )
                    .await;
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
