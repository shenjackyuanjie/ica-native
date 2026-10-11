//! ica 协议客户端核心：与 Icalingua Bridge 之间的 Socket.IO 主连接。
//!
//! 每个启用的 bridge 由 `app::runtime` 各起一个 `run_bridge` 任务，职责是：
//! - 按 `ws://` / `wss://` 连接 Bridge，注册协议事件回调，把服务端事件转成 `BridgeEvent`
//!   通过 unbounded channel 发回 GUI 主线程；
//! - 从另一个 unbounded channel 接收 GUI 侧产生的 `IcaCommand`，交给 `handler` 分发执行；
//! - 维护"连接 -> 断线 -> 退避等待 -> 重连"的外层循环，重连次数用尽后通知 GUI 并退出。
//!
//! 关键设计：
//! - 主连接只承载协议事件。鉴权在 `requireAuth` 回调里用 ed25519 私钥对服务端下发的
//!   `auth_key` 签名完成，私钥在注册回调时就与当前 bridge 绑定，多 bridge 不会串 key；
//! - 文件管理（fileMgr）不复用主连接，而是另建一次性 socket，见 `file_manager`；
//! - 未知协议事件不静默丢弃：`on_any` 兜底会打一条日志，便于排查 Bridge 升级后
//!   客户端"无响应"的兼容问题；确实不消费的事件则显式列入忽略清单；
//! - 重连只由主循环调度，断开回调只负责发送内部控制信号，避免多处同时决定重连。

pub mod ack;
pub mod client;
mod command;
pub mod event;
mod file_manager;
mod handler;
pub mod http;
pub mod types;

use crate::StopGetter;
use crate::config::IcaBridge;
pub use command::{
    BridgeHandle, GROUP_BAN_MAX_DURATION, ICA_PROTOCOL_VERSION, IcaCommand, NoticerImage,
    NoticerSendPayload,
};
use command::{
    ConnectionSignal, MAX_RECONNECT_ATTEMPTS, emit_ui_event, json_preview, payload_to_json,
    reconnect_delay,
};
pub use event::{BridgeEvent, BridgeEventKind};
use futures_util::future::BoxFuture;
use rust_socketio::asynchronous::{Client, ClientBuilder};
use rust_socketio::{Event, Payload, TransportType};
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// 主 socket 连接已注册专用回调的事件名；`on_any` 兜底里命中这些名字的事件视为已处理，
/// 不再打日志。新增 `.on(...)` 注册时记得同步维护这份清单。
const KNOWN_SOCKET_EVENTS: &[&str] = &[
    "requireAuth",
    "disconnect",
    "message",
    "authSucceed",
    "authFailed",
    "onlineData",
    "addMessage",
    "deleteMessage",
    "hideMessage",
    "revealMessage",
    "setAllRooms",
    "setAllChatGroups",
    "setMessages",
    "handleRequest",
    "sendAddRequest",
    "updateRoom",
    "syncRead",
    "renewMessage",
    "renewMessageURL",
    "setOnline",
    "setOffline",
    "setShutUp",
    "messageSuccess",
    "messageError",
    "addMessageText",
    "notifyMessage",
    "closeLoading",
    "notifyError",
    "dbUpgradeProgress",
    "requestSetup",
    "fatal",
    "login-verify",
    "login-qrcodeLogin",
    "login-smsCodeVerify",
    "login-error",
    "login-slider",
];

/// Bridge 会向 GUI 客户端广播、但当前原生客户端不消费的事件。
///
/// 这些事件参考 `ica-bot` 的 `any_event` 忽略列表维护：它们不是兼容性未知事件，
/// 因此不应触发兜底日志。`notify` 是 Bridge 为远端 Electron 客户端准备的桌面通知
/// 请求；原生端尚未实现系统通知，普通消息仍会由 `addMessage`、`updateRoom` 等事件更新。
const IGNORED_SOCKET_EVENTS: &[&str] = &[
    "authRequired", // 旧版 Bridge 的认证请求
    "login",        // Milky adapter 的额外登录成功通知
    "notify",       // 远端客户端桌面通知请求；原生端暂不支持系统通知
];

/// 启动 socketio client，并把服务端事件用 unbounded channel 发回 GUI 主线程。
///
/// 该函数会一直运行到收到停止信号、重连次数耗尽或 bridge 未启用时返回：
/// - `stop_alrm`：停止信号（oneshot receiver 的别名 `StopGetter`），触发后主动断开连接并返回；
/// - `bridge_cfg`：当前 bridge 的配置（包含 url、private_key 与协议版本容错开关）；
/// - `event_tx`：类型化 bridge 事件发送端，所有协议事件与连接状态变化都经它回传 GUI；
/// - `command_rx`：GUI 侧命令接收端，收到的每条 `IcaCommand` 在此循环内被处理。
///
/// 返回 `Ok(())` 表示正常结束（未启用、收到停止信号或重连耗尽）；只有内部错误才会返回 `Err`。
pub async fn run_bridge(
    stop_alrm: StopGetter,
    bridge_cfg: &IcaBridge,
    event_tx: UnboundedSender<BridgeEvent>,
    mut command_rx: UnboundedReceiver<IcaCommand>,
) -> anyhow::Result<()> {
    // 未启用的 bridge 直接返回，避免为一个不会连接的配置占用任务与 channel。
    if !bridge_cfg.enable {
        return Ok(());
    }
    // 包装成 Option 是为了和 emit_ui_event 的签名对齐：channel 尚未建立时只打日志。
    let event_tx = Some(event_tx);

    // 这里的值都固定绑定到当前 bridge，后面注册回调时直接捕获它们。
    // bridge_key 作为多 bridge 场景下的连接标识，配置里没写名字时退回用 url。
    let bridge_key = if bridge_cfg.name.is_empty() {
        bridge_cfg.url.clone()
    } else {
        bridge_cfg.name.clone()
    };
    // Bridge 的 HTTP API 与 socket 同源同端口，只是把 ws/wss 换成 http/https，
    // 大 payload 发送（含 base64 图片）会走这个地址，见 handler::http_send。
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
    let http_clients = http::BridgeHttpClients::new().map_err(anyhow::Error::msg)?;
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
            let sign_callback = client::sign_callback(
                bridge_key.clone(),
                private_key.clone(),
                bridge_cfg.allow_protocol_mismatch,
                event_tx.clone(),
                connection_signal_tx.clone(),
            );
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
            // 断开回调只负责通知主循环“这条连接已经断了”，
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

        {
            // 兜底回调：`on_any` 会收到所有 Message/Custom 事件（含已注册专用回调的），
            // 这里只对没有专用回调的未知事件打一条 info 日志，便于排查 Bridge 新增事件后
            // 客户端静默无响应的兼容问题。
            let bridge_id = bridge_key.clone();
            builder = builder.on_any(
                move |event: Event, payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                    let bridge_id = bridge_id.clone();
                    Box::pin(async move {
                        let event_name = event.as_str().to_string();
                        if KNOWN_SOCKET_EVENTS.contains(&event_name.as_str())
                            || IGNORED_SOCKET_EVENTS.contains(&event_name.as_str())
                        {
                            return;
                        }
                        let payload_json = payload_to_json(&payload);
                        tracing::info!(
                            bridge = %bridge_id,
                            socket = "main",
                            event = %event_name,
                            payload = %json_preview(&payload_json, 512),
                            "收到未处理的 Socket.IO 事件"
                        );
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
        builder = builder.on("dbUpgradeProgress", make_event_cb("dbUpgradeProgress"));
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
                    bridge = %bridge_key,
                    socket = "main",
                    elapsed = ?start_connect_time.elapsed(),
                    "Socket.IO 已连接"
                );
                reconnect_attempt = 0;
                emit_ui_event(&event_tx, &bridge_key, "socketConnected", JsonValue::Null);
                client
            }
            Err(e) => {
                tracing::error!(bridge = %bridge_key, socket = "main", error = %e, "Socket.IO 连接失败");
                emit_ui_event(
                    &event_tx,
                    &bridge_key,
                    "socketConnectFailed",
                    json!({ "message": e.to_string() }),
                );

                reconnect_attempt += 1;
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    // 达到上限属于预期的停止条件而不是故障：正常返回，
                    // 具体原因已经通过 socketReconnectExhausted 告知 GUI。
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
                    // 主动断开只做尽力尝试：失败也不影响流程走向，
                    // 真正重要的是通知 GUI 本次连接已经结束。
                    match client.disconnect().await {
                        Ok(_) => {
                            tracing::info!(bridge = %bridge_key, socket = "main", "Socket.IO 已断开");
                        }
                        Err(e) => {
                            tracing::warn!(bridge = %bridge_key, socket = "main", error = %e, "断开 Socket.IO 失败");
                        }
                    }
                    emit_ui_event(&event_tx, &bridge_key, "socketDisconnected", JsonValue::Null);
                    return Ok(());
                }
                Some(signal) = connection_signal_rx.recv() => {
                    match signal {
                        // 只有这一种内部控制信号；保留枚举是为了将来扩展其它信号时有编译提示。
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
                        &http_clients,
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
                // 同上：断线重连达到上限后正常收尾，不再向 GUI 以外抛错。
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
