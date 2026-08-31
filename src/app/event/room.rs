//! 会话列表、入群/加好友请求与系统消息相关的事件。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::room::Room;

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "setAllRooms" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match Vec::<Room>::deserialize(value) {
                    Ok(rooms) => {
                        state.rooms = rooms;
                        state.bump_rooms_revision();
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(
                            state,
                            "setAllRooms",
                            &e.to_string(),
                            value,
                        );
                        state.last_error = Some(format!("setAllRooms 解析失败: {}", e));
                    }
                }
            }
        }
        "handleRequest" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match payload::parse_join_request(value, None) {
                    Ok(request) => {
                        state.upsert_join_request(request);
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(state, "handleRequest", &e, value);
                        state.last_error = Some(format!("handleRequest 解析失败: {}", e));
                    }
                }
            }
        }
        "sendAddRequest" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match payload::parse_join_request(value, None) {
                    Ok(request) => {
                        state.upsert_join_request(request);
                        state.last_notice = Some("收到新的验证消息".to_string());
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(state, "sendAddRequest", &e, value);
                        state.last_error = Some(format!("sendAddRequest 解析失败: {}", e));
                    }
                }
            }
        }
        "updateRoom" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match Room::deserialize(value) {
                    Ok(updated_room) => {
                        if let Some(existing) = state
                            .rooms
                            .iter_mut()
                            .find(|r| r.room_id == updated_room.room_id)
                        {
                            *existing = updated_room;
                        } else {
                            state.rooms.push(updated_room);
                        }
                        state.bump_rooms_revision();
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(
                            state,
                            "updateRoom",
                            &e.to_string(),
                            value,
                        );
                        state.last_error = Some(format!("updateRoom 解析失败: {}", e));
                    }
                }
            }
        }
        "syncRead" => {
            if let Some(room_id) = payload::first_payload_value(payload).and_then(|v| v.as_i64())
                && let Some(room) = state.rooms.iter_mut().find(|r| r.room_id == room_id)
            {
                room.unread_count = 0;
                room.at = crate::ica::types::message::At::Bool(false);
                state.bump_rooms_revision();
            }
        }
        "setSystemMessages" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match payload::parse_join_requests_snapshot(value) {
                    Ok(requests) => {
                        state.replace_join_requests(requests);
                        state.last_error = None;
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(state, "setSystemMessages", &e, value);
                        state.last_error = Some(format!("setSystemMessages 解析失败: {}", e));
                    }
                }
            }
        }
        _ => return false,
    }
    true
}
