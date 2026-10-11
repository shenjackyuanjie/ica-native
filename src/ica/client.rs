//! 主 socket 连接的鉴权与消息发送原语。
//!
//! 这里只封装"怎么和 Bridge 说一件事"，不判断该不该说：
//! - `sign_callback` / `sign_with_key`：用 ed25519 私钥应答 Bridge 的 `requireAuth` 挑战；
//! - `send_message` / `send_string_message` / `delete_message`：消息的发出与撤回；
//! - `send_room_sign_in` / `send_poke`：群签到与戳一戳等带参数约定的 oicq 事件。
//!
//! 所有函数都只返回"这次 emit 是否成功"，不包含业务校验；协议版本协商、
//! 权限与重试策略由 `handler` 与 GUI 侧决定。

#[cfg(test)]
#[path = "tests/client.rs"]
mod tests;

use super::command::{ConnectionSignal, emit_ui_event};
// 一些类型别名从 types 模块引入
use crate::ica::types::message::{DeleteMessage, SendMessage};
use crate::ica::types::{RoomId, UserId};
use crate::ica::{BridgeEvent, ICA_PROTOCOL_VERSION};
use ed25519_dalek::{Signature, Signer, SigningKey};
use futures_util::future::BoxFuture;
use hex;
use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{Level, event};

/// 使用指定私钥对服务端的 requireAuth payload 进行签名并发送 auth 事件。
///
/// `requireAuth` 的载荷约定为 `[auth_key, {version, protocolVersion}]`：第一个元素是
/// 服务端下发的十六进制挑战值，第二个元素描述 Bridge 自身与协议版本。本函数据此完成
/// 版本协商事件上报、ed25519 签名与 `auth` 事件发送。
///
/// 任何一步不合预期（载荷类型不对、挑战值或私钥不是合法 hex）都只记录日志后返回，
/// 不 panic：连接会因服务端未收到 `auth` 而自然失败，由上层重连逻辑接手。
async fn sign_with_key(
    payload: Payload,
    client: Client,
    bridge_key: String,
    private_key_hex: String,
    allow_protocol_mismatch: bool,
    event_tx: Option<UnboundedSender<BridgeEvent>>,
    _connection_signal_tx: UnboundedSender<ConnectionSignal>,
) {
    // 解析 payload，优先取 Text：requireAuth 只可能以文本数组下发，
    // 二进制载荷说明协议对不上，直接放弃本次鉴权。
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

    // 载荷第二个元素携带 Bridge 版本信息；缺失时按 unknown 处理，
    // 让 GUI 仍能显示"对端没报版本"，而不是当成版本匹配。
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
        // 版本不一致只上报并打日志，不中断握手：Bridge 的新增事件通常向后兼容，
        // 是否放行由配置 allow_protocol_mismatch 决定，这里只负责把情况讲清楚。
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

    // 把 auth_key 当成 hex 解码成 salt：服务端下发的是十六进制字符串形式的挑战值，
    // ed25519 签名直接作用于这些原始字节。
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

    // 把私钥 hex 解为 32 字节数组；长度不符时统一换算成 FromHexError，
    // 让下面的错误分支只需处理一种错误类型。
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
///
/// 参数：
/// - `bridge_key`：日志与事件里标识这条连接的名字；
/// - `private_key_hex`：该 bridge 的 ed25519 私钥（十六进制，32 字节）；
/// - `allow_protocol_mismatch`：协议版本不一致时是否仅提示而继续连接；
/// - `event_tx`：版本信息与协议不匹配事件的回传通道；
/// - `_connection_signal_tx`：预留的内部控制信号通道，当前握手流程不发出信号。
///
/// 返回的回调可被多次调用（`Fn`），内部每次都克隆一份已捕获的配置。
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
///
/// 只负责把消息序列化后 emit 出去；调用方需要自行判断媒体是否应改走 HTTP 发送
/// （见 `handler::send_message_result`）。
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
///
/// 用于 raw 消息等无法用 `SendMessage` 结构表达的载荷；`message` 会被克隆后发出。
/// 返回是否 emit 成功。
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
///
/// 按 Bridge 约定只传 `[room_id, message_id]` 两个参数，不发送整条消息内容。
/// 返回是否 emit 成功；撤回是否真正生效以 Bridge 回报为准。
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
///
/// `RoomId` 用负数表示群聊，这里直接复用 `i64::is_positive` 判私聊并拒绝，
/// 避免把私聊房间号当成群号发出去。返回是否 emit 成功。
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
///
/// 返回 `(conversation_id, target_user_id)`：
/// - 群聊（`room_id < 0`）取群号绝对值，目标即传入的 `target`；
/// - 私聊（`room_id > 0`）会话 id 就是对方 id，因此两个参数都用 `room_id`；
/// - `room_id == 0`、群号为 0 或 `target <= 0` 时返回 `None`，交由调用方放弃发送。
pub fn poke_target(room_id: RoomId, target: UserId) -> Option<(i64, UserId)> {
    let conversation_id = room_id.checked_abs().filter(|id| *id > 0)?;
    if target <= 0 {
        return None;
    }
    Some((conversation_id, if room_id > 0 { room_id } else { target }))
}

/// 发送 oicq 群聊或私聊戳一戳。
///
/// 先用 `poke_target` 归一化参数，目标无效时记录警告并返回 `false`，
/// 不会向 Bridge 发出请求。返回是否 emit 成功。
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
