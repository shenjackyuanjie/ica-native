use ed25519_dalek::{Signature, Signer, SigningKey};
use futures_util::future::BoxFuture;
use hex;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{Level, event};

use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;

/// 一些类型别名从 types 模块引入
use crate::ica::types::message::{DeleteMessage, SendMessage};
use crate::ica::types::{RoomId, UserId};
use crate::ica::{BridgeEvent, ICA_PROTOCOL_VERSION};

use super::command::{ConnectionSignal, emit_ui_event};

/// 使用指定私钥对服务端的 requireAuth payload 进行签名并发送 auth 事件
async fn sign_with_key(
    payload: Payload,
    client: Client,
    bridge_key: String,
    private_key_hex: String,
    allow_protocol_mismatch: bool,
    event_tx: Option<UnboundedSender<BridgeEvent>>,
    connection_signal_tx: UnboundedSender<ConnectionSignal>,
) {
    // 解析 payload，优先取 Text
    let require_data = match payload {
        Payload::Text(vals) => vals,
        _ => {
            event!(Level::WARN, bridge = %bridge_key, socket = "main", "sign_with_key: unexpected payload type");
            return;
        }
    };

    if require_data.is_empty() {
        event!(Level::WARN, bridge = %bridge_key, socket = "main", "sign_with_key: empty payload");
        return;
    }

    let version_info = require_data.get(1).and_then(JsonValue::as_object);
    let bridge_version = version_info
        .and_then(|value| value.get("version"))
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    let protocol_version = version_info
        .and_then(|value| value.get("protocolVersion"))
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    emit_ui_event(
        &event_tx,
        &bridge_key,
        "bridgeVersionInfo",
        json!({
            "version": bridge_version,
            "protocolVersion": protocol_version,
            "expectedProtocolVersion": ICA_PROTOCOL_VERSION,
        }),
    );
    if protocol_version != ICA_PROTOCOL_VERSION {
        let message = format!(
            "Bridge 协议版本不匹配：客户端要求 {ICA_PROTOCOL_VERSION}，服务器为 {protocol_version}"
        );
        emit_ui_event(
            &event_tx,
            &bridge_key,
            "bridgeProtocolMismatch",
            json!({
                "message": message,
                "allowed": allow_protocol_mismatch,
                "protocolVersion": protocol_version,
                "expectedProtocolVersion": ICA_PROTOCOL_VERSION,
            }),
        );
        if !allow_protocol_mismatch {
            event!(Level::ERROR, bridge = %bridge_key, protocol_version, expected = ICA_PROTOCOL_VERSION, "拒绝认证协议版本不匹配的 Bridge");
            let _ = connection_signal_tx.send(ConnectionSignal::Stop);
            let _ = client.disconnect().await;
            return;
        }
        event!(Level::WARN, bridge = %bridge_key, protocol_version, expected = ICA_PROTOCOL_VERSION, "配置允许连接协议版本不匹配的 Bridge");
    }

    // 第一个元素应为 auth_key 字符串
    let auth_key = match &require_data[0] {
        JsonValue::String(s) => s.clone(),
        other => {
            event!(
                Level::WARN,
                bridge = %bridge_key,
                socket = "main",
                "sign_with_key: auth_key is not string: {:?}",
                other
            );
            return;
        }
    };

    // 把 auth_key 当成 hex 解码成 salt
    let salt = match hex::decode(&auth_key) {
        Ok(s) => s,
        Err(e) => {
            event!(
                Level::ERROR,
                bridge = %bridge_key,
                socket = "main",
                "sign_with_key: auth_key 不是有效的十六进制: {}",
                e
            );
            return;
        }
    };

    // 把私钥 hex 解为 32 字节数组
    let array_key_res: Result<[u8; 32], _> = hex::decode(&private_key_hex).and_then(|v| {
        v.try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)
    });

    let array_key = match array_key_res {
        Ok(a) => a,
        Err(_) => {
            event!(
                Level::ERROR,
                bridge = %bridge_key,
                socket = "main",
                "sign_with_key: private key not valid 32-bytes hex"
            );
            return;
        }
    };

    // 使用 ed25519 签名
    let signing_key = SigningKey::from_bytes(&array_key);
    let signature: Signature = signing_key.sign(salt.as_slice());
    let sign_bytes = signature.to_bytes().to_vec();

    // 发送签名到服务端 (auth)
    match client.emit("auth", sign_bytes).await {
        Ok(_) => {
            event!(Level::INFO, bridge = %bridge_key, socket = "main", "sign_with_key: auth signed & sent");
        }
        Err(e) => {
            event!(Level::ERROR, bridge = %bridge_key, socket = "main", error = ?e, "sign_with_key: 发送 auth 事件失败");
        }
    }
}

/// 为某个 bridge 构造专用的 requireAuth 回调。
///
/// 多 bridge 场景下，每个 socket 连接都必须固定使用自己的私钥，
/// 因此这里不再从全局配置里“猜”第一个 bridge，而是在注册事件时直接把 key 封进回调。
pub fn sign_callback(
    bridge_key: String,
    private_key_hex: String,
    allow_protocol_mismatch: bool,
    event_tx: Option<UnboundedSender<BridgeEvent>>,
    connection_signal_tx: UnboundedSender<ConnectionSignal>,
) -> impl Fn(Payload, Client) -> BoxFuture<'static, ()> + Send + Sync + 'static {
    move |payload: Payload, client: Client| {
        let bridge_key = bridge_key.clone();
        let private_key_hex = private_key_hex.clone();
        let event_tx = event_tx.clone();
        let connection_signal_tx = connection_signal_tx.clone();
        Box::pin(async move {
            sign_with_key(
                payload,
                client,
                bridge_key,
                private_key_hex,
                allow_protocol_mismatch,
                event_tx,
                connection_signal_tx,
            )
            .await;
        })
    }
}

/// 发送一条 SendMessage（安全封装）
/// 返回是否发送成功
pub async fn send_message(client: &Client, message: &SendMessage) -> bool {
    let value = message.as_value();
    match client.emit("sendMessage", value).await {
        Ok(_) => {
            event!(Level::DEBUG, "send_message {}", format!("{message:?}"));
            true
        }
        Err(e) => {
            event!(Level::WARN, "send_message 失败: {:?}", e);
            false
        }
    }
}

/// 发送任意 JSON 格式的消息（sendMessage）
pub async fn send_string_message(client: &Client, message: &JsonValue) -> bool {
    match client.emit("sendMessage", message.clone()).await {
        Ok(_) => {
            event!(Level::INFO, "send_message {}", format!("{message:#?}"));
            true
        }
        Err(e) => {
            event!(Level::WARN, "send_message 失败: {:?}", e);
            false
        }
    }
}

/// 删除一条消息
pub async fn delete_message(client: &Client, message: &DeleteMessage) -> bool {
    match client
        .emit(
            "deleteMessage",
            vec![json!(message.room_id), json!(message.message_id)],
        )
        .await
    {
        Ok(_) => {
            event!(Level::DEBUG, "delete_message {:?}", message);
            true
        }
        Err(e) => {
            event!(Level::WARN, "delete_message 失败: {:?}", e);
            false
        }
    }
}

/// 向群发送签到（仅限群聊，即 room_id.is_room() 为 true）
pub async fn send_room_sign_in(client: &Client, room_id: RoomId) -> bool {
    if room_id.is_positive() {
        event!(
            Level::WARN,
            "send_room_sign_in: cannot send sign to private chat"
        );
        return false;
    }
    let data = json!(room_id.abs());
    match client.emit("sendGroupSign", data).await {
        Ok(_) => {
            event!(Level::INFO, "sent group sign to room {}", room_id);
            true
        }
        Err(e) => {
            event!(Level::ERROR, "send_group_sign 失败: {:?}", e);
            false
        }
    }
}

/// 发送戳一戳给某人（群聊或私聊皆可，视服务端实现）
pub async fn send_poke(client: &Client, room_id: RoomId, target: UserId) -> bool {
    let data = vec![json!(room_id), json!(target)];
    match client.emit("sendGroupPoke", data).await {
        Ok(_) => {
            event!(Level::INFO, "sent poke to {} in {}", target, room_id);
            true
        }
        Err(e) => {
            event!(Level::ERROR, "send_poke 失败: {:?}", e);
            false
        }
    }
}
