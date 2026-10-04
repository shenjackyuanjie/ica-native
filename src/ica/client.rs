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
    _connection_signal_tx: UnboundedSender<ConnectionSignal>,
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
            event!(Level::WARN, bridge = %bridge_key, protocol_version, expected = ICA_PROTOCOL_VERSION, "Bridge 协议版本较旧或不匹配，尝试继续兼容连接");
        } else {
            event!(Level::WARN, bridge = %bridge_key, protocol_version, expected = ICA_PROTOCOL_VERSION, "配置允许连接协议版本不匹配的 Bridge");
        }
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
    _connection_signal_tx: UnboundedSender<ConnectionSignal>,
) -> impl Fn(Payload, Client) -> BoxFuture<'static, ()> + Send + Sync + 'static {
    move |payload: Payload, client: Client| {
        let bridge_key = bridge_key.clone();
        let private_key_hex = private_key_hex.clone();
        let event_tx = event_tx.clone();
        let connection_signal_tx = _connection_signal_tx.clone();
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

/// oicq 的 sendGroupPoke 使用正群号；私聊用对方的正 ID 同时作为两个参数。
/// 不允许把私聊房间中的发送者（例如自己）误当成私聊目标。
pub fn poke_target(room_id: RoomId, target: UserId) -> Option<(i64, UserId)> {
    let conversation_id = room_id.checked_abs().filter(|id| *id > 0)?;
    if target <= 0 {
        return None;
    }
    Some((conversation_id, if room_id > 0 { room_id } else { target }))
}

/// 发送 oicq 群聊或私聊戳一戳。
pub async fn send_poke(client: &Client, room_id: RoomId, target: UserId) -> bool {
    let Some((conversation_id, target)) = poke_target(room_id, target) else {
        event!(Level::WARN, "戳一戳目标无效，未发送请求");
        return false;
    };
    let data = vec![json!(conversation_id), json!(target)];
    match client.emit("sendGroupPoke", data).await {
        Ok(_) => {
            event!(Level::INFO, "戳一戳请求已发送");
            true
        }
        Err(e) => {
            event!(Level::ERROR, "send_poke 失败: {:?}", e);
            false
        }
    }
}

#[cfg(test)]
mod poke_tests {
    use super::poke_target;

    #[test]
    fn group_poke_uses_positive_group_id_without_changing_the_member() {
        assert_eq!(poke_target(-123, 456), Some((123, 456)));
    }

    #[test]
    fn private_poke_uses_the_peer_for_both_oicq_arguments() {
        assert_eq!(poke_target(123, 123), Some((123, 123)));
        assert_eq!(poke_target(123, 456), Some((123, 123)));
    }

    #[test]
    fn invalid_poke_targets_are_rejected_without_overflow() {
        for (room_id, target_id) in [(0, 123), (i64::MIN, 123), (-123, 0), (-123, -1)] {
            assert_eq!(poke_target(room_id, target_id), None);
        }
    }
}
