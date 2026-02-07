use ed25519_dalek::{Signature, Signer, SigningKey};
use hex;
use serde_json::Value as JsonValue;
use serde_json::json;
use tracing::{event, Level};

use rust_socketio::asynchronous::Client;
use rust_socketio::Payload;

/// 一些类型别名从 types 模块引入
use crate::ica::types::message::{DeleteMessage, SendMessage};
use crate::ica::types::{RoomId, UserId};

/// 使用指定私钥对服务端的 requireAuth payload 进行签名并发送 auth 事件
pub async fn sign_with_key(payload: Payload, client: Client, private_key_hex: String) {
    // 解析 payload，优先取 Text
    let require_data = match payload {
        Payload::Text(vals) => vals,
        _ => {
            event!(Level::WARN, "sign_with_key: unexpected payload type");
            return;
        }
    };

    if require_data.is_empty() {
        event!(Level::WARN, "sign_with_key: empty payload");
        return;
    }

    // 第一个元素应为 auth_key 字符串
    let auth_key = match &require_data[0] {
        JsonValue::String(s) => s.clone(),
        other => {
            event!(
                Level::WARN,
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
            event!(Level::ERROR, "sign_with_key: invalid auth_key hex: {}", e);
            return;
        }
    };

    // 把私钥 hex 解为 32 字节数组
    let array_key_res: Result<[u8; 32], _> = hex::decode(&private_key_hex)
        .and_then(|v| {
            v.try_into()
                .map_err(|_| hex::FromHexError::InvalidStringLength)
        });

    let array_key = match array_key_res {
        Ok(a) => a,
        Err(_) => {
            event!(
                Level::ERROR,
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
            event!(Level::INFO, "sign_with_key: auth signed & sent");
        }
        Err(e) => {
            event!(Level::ERROR, "sign_with_key: failed to emit auth: {:?}", e);
        }
    }
}

/// 当服务器发来 `requireAuth` 时调用。
/// payload 通常是一个 Text 类型的数组：`[auth_key, version]`
/// 这里我们会从本地配置读取第一个 bridge 的 `private_key`，用它对服务端给出的 salt 签名并发送 `auth` 事件。
///
/// 注意：本实现假设在 native 中只使用单一 bridge（或优先使用配置中第一个 bridge）。
pub async fn sign_callback(payload: Payload, client: Client) {
    // 从配置中读取私钥（取第一个 bridge 的 private_key）
    let cfg = crate::cfg::get_cfg_snapshot();
    let private_key_hex = match cfg.bridges.first() {
        Some(b) => b.private_key.clone(),
        None => {
            event!(Level::WARN, "requireAuth: no bridge configured to sign auth");
            return;
        }
    };

    // 代理到通用的 sign_with_key 实现
    sign_with_key(payload, client, private_key_hex).await;
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
            event!(Level::WARN, "send_message failed: {:?}", e);
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
            event!(Level::WARN, "send_message failed: {:?}", e);
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
            event!(Level::WARN, "delete_message failed: {:?}", e);
            false
        }
    }
}

/// 向群发送签到（仅限群聊，即 room_id.is_room() 为 true）
pub async fn send_room_sign_in(client: &Client, room_id: RoomId) -> bool {
    if room_id.is_positive() {
        event!(Level::WARN, "send_room_sign_in: cannot send sign to private chat");
        return false;
    }
    let data = json!(room_id.abs());
    match client.emit("sendGroupSign", data).await {
        Ok(_) => {
            event!(
Level::INFO, "sent group sign to room {}", room_id);
            true
        }
        Err(e) => {
            event!(Level::ERROR, "send_group_sign failed: {:?}", e);
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
            event!(Level::ERROR, "send_poke failed: {:?}", e);
            false
        }
    }
}
