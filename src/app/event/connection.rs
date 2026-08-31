//! 连接、鉴权与在线状态相关的事件。

use serde_json::Value as JsonValue;

use crate::app::state::{AuthState, BridgeState, SocketState};
use crate::ica::types::online_data::OnlineData;

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "socketConnecting" => {
            state.socket_state = SocketState::Connecting;
            state.auth_state = AuthState::Unknown;
            state.last_error = None;
        }
        "socketReconnecting" => {
            state.socket_state = SocketState::Connecting;
            state.last_error = payload::payload_message(payload);
        }
        "socketConnected" => {
            state.socket_state = SocketState::Connected;
            state.last_error = None;
        }
        "socketDisconnected" => {
            state.socket_state = SocketState::Disconnected;
            state.last_error = payload::payload_message(payload);
        }
        "socketConnectFailed" => {
            state.socket_state = SocketState::Failed;
            state.last_error = payload::payload_message(payload);
        }
        "socketRetryScheduled" => {
            state.socket_state = SocketState::Connecting;
            state.last_error = payload::payload_message(payload);
        }
        "socketReconnectExhausted" => {
            state.socket_state = SocketState::Failed;
            state.last_error = payload::payload_message(payload);
        }
        "requireAuth" => {
            state.auth_state = AuthState::Pending;
        }
        "authSucceed" => {
            state.auth_state = AuthState::Succeeded;
            state.last_error = None;
        }
        "authFailed" => {
            state.auth_state = AuthState::Failed;
            state.last_error = Some("bridge 认证失败".to_string());
        }
        "message" => {
            if let Some(message) =
                payload::first_payload_value(payload).and_then(|value| value.as_str())
            {
                match message {
                    "authRequired" => {
                        state.auth_state = AuthState::Pending;
                    }
                    "authSucceed" => {
                        state.auth_state = AuthState::Succeeded;
                        state.last_error = None;
                    }
                    "authFailed" => {
                        state.auth_state = AuthState::Failed;
                        state.last_error = Some("bridge 认证失败".to_string());
                    }
                    _ => {}
                }
            }
        }
        "onlineData" => {
            if let Some(value) = payload::first_payload_value(payload) {
                state.online_data = OnlineData::new_from_json(value);
            }
        }
        "setOnline" => {
            state.socket_state = SocketState::Connected;
        }
        "setOffline" => {
            state.socket_state = SocketState::Disconnected;
            if let Some(value) = payload::first_payload_value(payload)
                && let Some(msg) = value.as_str()
            {
                state.last_error = Some(msg.to_string());
            }
        }
        "setShutUp" => {
            state.is_shut_up = payload::first_payload_value(payload)
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
        }
        "requestSetup" => {
            if let Some(value) = payload::first_payload_value(payload) {
                state.setup_requested = Some(payload::json_preview(value, 512));
                state.last_error =
                    Some("bridge 尚未登录，需要先在 Icalingua++/bridge 完成登录".to_string());
            }
        }
        "fatal" => {
            let message = payload::first_payload_display_message(payload)
                .unwrap_or_else(|| "bridge 发生致命错误".to_string());
            state.fatal_error = Some(message.clone());
            state.last_error = Some(message);
            state.socket_state = SocketState::Failed;
        }
        _ => return false,
    }
    true
}
