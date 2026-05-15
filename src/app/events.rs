use serde_json::Value as JsonValue;

use crate::ica::types::{
    message::{Message, NewMessage},
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};

use super::IcaApp;
use super::state::{AuthState, BridgeState, SocketState};

impl IcaApp {
    /// socket.io 的事件 payload 基本都是数组包装，真正的数据通常在第一个元素里。
    fn first_payload_value(payload: &JsonValue) -> Option<&JsonValue> {
        payload.as_array().and_then(|values| values.first())
    }

    /// 统一提取事件里常见的 `message` 字段，避免每个分支都手动抄一遍。
    fn payload_message(payload: &JsonValue) -> Option<String> {
        payload
            .get("message")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
    }

    fn json_preview(value: &JsonValue, max_chars: usize) -> String {
        let raw = value.to_string();
        if raw.len() > max_chars {
            format!("{}...", &raw[..max_chars])
        } else {
            raw
        }
    }

    fn parse_join_request(
        value: JsonValue,
        fallback_flag: Option<&str>,
    ) -> Result<JoinRequestRoom, String> {
        let mut request =
            serde_json::from_value::<JoinRequestRoom>(value).map_err(|e| e.to_string())?;
        if request.flag.is_empty()
            && let Some(flag) = fallback_flag
        {
            request.flag = flag.to_string();
        }
        request.request_type = request.request_type.trim().to_string();
        request.post_type = request.post_type.trim().to_string();
        request.sub_type = request.sub_type.trim().to_string();
        request.source = request.source.trim().to_string();
        request.group_name = request.group_name.trim().to_string();
        request.nickname = request.nickname.trim().to_string();
        request.comment = request.comment.trim().to_string();
        request.tips = request.tips.trim().to_string();
        Ok(request)
    }

    fn parse_join_requests_snapshot(value: &JsonValue) -> Result<Vec<JoinRequestRoom>, String> {
        match value {
            JsonValue::Array(values) => values
                .iter()
                .cloned()
                .map(|item| Self::parse_join_request(item, None))
                .collect(),
            JsonValue::Object(items) => items
                .iter()
                .map(|(flag, item)| Self::parse_join_request(item.clone(), Some(flag)))
                .collect(),
            _ => Err("getSystemMsg 返回的不是数组或对象".to_string()),
        }
    }

    /// 把某个 bridge 发来的事件应用到对应的本地状态上。
    ///
    /// 这里故意不做 UI 逻辑，只做"事件 -> 状态"的映射，方便后续继续补事件类型。
    pub(super) fn apply_socketio_event(
        state: &mut BridgeState,
        event_name: &str,
        payload: &JsonValue,
    ) {
        match event_name {
            "socketConnecting" => {
                state.socket_state = SocketState::Connecting;
                state.auth_state = AuthState::Unknown;
                state.last_error = None;
            }
            "socketReconnecting" => {
                state.socket_state = SocketState::Connecting;
                state.last_error = Self::payload_message(payload);
            }
            "socketConnected" => {
                state.socket_state = SocketState::Connected;
                state.last_error = None;
            }
            "socketDisconnected" => {
                state.socket_state = SocketState::Disconnected;
                state.last_error = Self::payload_message(payload);
            }
            "socketConnectFailed" => {
                state.socket_state = SocketState::Failed;
                state.last_error = Self::payload_message(payload);
            }
            "socketRetryScheduled" => {
                state.socket_state = SocketState::Connecting;
                state.last_error = Self::payload_message(payload);
            }
            "socketReconnectExhausted" => {
                state.socket_state = SocketState::Failed;
                state.last_error = Self::payload_message(payload);
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
                    Self::first_payload_value(payload).and_then(|value| value.as_str())
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
                if let Some(value) = Self::first_payload_value(payload) {
                    state.online_data = OnlineData::new_from_json(value);
                }
            }
            "setAllRooms" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<Vec<Room>>(value.clone()) {
                        Ok(rooms) => state.rooms = rooms,
                        Err(e) => {
                            state.last_error = Some(format!("setAllRooms 解析失败: {}", e));
                        }
                    }
                }
            }
            "setMessages" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    match serde_json::from_value::<Vec<Message>>(value["messages"].clone()) {
                        Ok(messages) => {
                            state.requested_rooms.insert(room_id);
                            if state.pending_message_scroll_to_bottom.remove(&room_id) {
                                state.message_scroll_to_bottom.insert(room_id);
                            }
                            // 重置历史加载状态（新的 setMessages 意味着全量刷新）
                            state.no_more_history.remove(&room_id);
                            state.loading_older_messages.remove(&room_id);
                            state.messages_by_room.insert(room_id, messages);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "setMessages parse failed: bridge={} room_id={} err={} raw={}",
                                state.bridge_key,
                                room_id,
                                e,
                                Self::json_preview(&value["messages"], 512)
                            );
                            state.last_error = Some(format!("setMessages 解析失败: {}", e));
                        }
                    }
                }
            }
            "appendOlderMessages" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    // 不管解析成功与否都要重置加载状态
                    state.loading_older_messages.remove(&room_id);
                    match serde_json::from_value::<Vec<Message>>(value["messages"].clone()) {
                        Ok(older_messages) => {
                            if older_messages.is_empty() {
                                // 没有更多历史消息
                                state.no_more_history.insert(room_id);
                            } else {
                                // 合并到已有消息列表前面（去重）
                                let existing = state.messages_by_room.entry(room_id).or_default();
                                let existing_ids: std::collections::HashSet<&str> =
                                    existing.iter().map(|m| m.msg_id.as_str()).collect();
                                let mut new_msgs: Vec<Message> = older_messages
                                    .into_iter()
                                    .filter(|m| !existing_ids.contains(m.msg_id.as_str()))
                                    .collect();
                                // 将旧消息放在前面
                                new_msgs.append(existing);
                                *existing = new_msgs;
                                // 标记需要调整 scroll offset
                                state.prepend_scroll_fix.insert(room_id);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "appendOlderMessages parse failed: bridge={} room_id={} err={}",
                                state.bridge_key,
                                room_id,
                                e,
                            );
                        }
                    }
                }
            }
            "addMessage" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<NewMessage>(value.clone()) {
                        Ok(new_message) => {
                            let room_id = new_message.room_id;
                            let should_scroll_to_bottom = new_message.msg.sender_id
                                == state.online_data.qqid
                                && state.pending_send_scroll_to_bottom.remove(&room_id);
                            state.requested_rooms.insert(room_id);
                            state.sync_room_preview(room_id, &new_message.msg);
                            state.upsert_message(room_id, new_message.msg);
                            if should_scroll_to_bottom {
                                state.message_scroll_to_bottom.insert(room_id);
                            }
                        }
                        Err(e) => {
                            state.last_error = Some(format!("addMessage 解析失败: {}", e));
                        }
                    }
                }
            }
            "deleteMessage" => {
                if let Some(msg_id) =
                    Self::first_payload_value(payload).and_then(|value| value.as_str())
                {
                    state.mark_message_deleted(msg_id);
                }
            }
            "hideMessage" => {
                if let Some(msg_id) =
                    Self::first_payload_value(payload).and_then(|value| value.as_str())
                {
                    state.mark_message_hidden(msg_id);
                }
            }
            "revealMessage" => {
                if let Some(msg_id) =
                    Self::first_payload_value(payload).and_then(|value| value.as_str())
                {
                    state.mark_message_revealed(msg_id);
                }
            }
            "handleRequest" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match Self::parse_join_request(value.clone(), None) {
                        Ok(request) => {
                            state.upsert_join_request(request);
                        }
                        Err(e) => {
                            state.last_error = Some(format!("handleRequest 解析失败: {}", e));
                        }
                    }
                }
            }
            "updateRoom" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<Room>(value.clone()) {
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
                        }
                        Err(e) => {
                            state.last_error = Some(format!("updateRoom 解析失败: {}", e));
                        }
                    }
                }
            }
            "syncRead" => {
                if let Some(room_id) = Self::first_payload_value(payload).and_then(|v| v.as_i64())
                    && let Some(room) = state.rooms.iter_mut().find(|r| r.room_id == room_id)
                {
                    room.unread_count = 0;
                    room.at = crate::ica::types::message::At::Bool(false);
                }
            }
            "renewMessage" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    if let Some(msg_id) = value["messageId"].as_str()
                        && let Some(messages) = state.messages_by_room.get_mut(&room_id)
                        && let Some(existing) = messages.iter_mut().find(|m| m.msg_id == msg_id)
                        && let Some(msg_update) = value.get("message")
                    {
                        if let Some(content) = msg_update.get("content").and_then(|c| c.as_str()) {
                            existing.content = content.to_string();
                        }
                        if let Some(deleted) = msg_update.get("deleted").and_then(|d| d.as_bool()) {
                            existing.deleted = deleted;
                        }
                        if let Some(hide) = msg_update.get("hide").and_then(|h| h.as_bool()) {
                            existing.hide = hide;
                        }
                        if let Some(reveal) = msg_update.get("reveal").and_then(|r| r.as_bool()) {
                            existing.reveal = reveal;
                        }
                    }
                }
            }
            "setOnline" => {
                state.socket_state = SocketState::Connected;
            }
            "setOffline" => {
                state.socket_state = SocketState::Disconnected;
                if let Some(value) = Self::first_payload_value(payload)
                    && let Some(msg) = value.as_str()
                {
                    state.last_error = Some(msg.to_string());
                }
            }
            "messageSuccess" => {}
            "messageError" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let msg = value.as_str().unwrap_or("消息发送失败");
                    state.last_error = Some(msg.to_string());
                }
            }
            "closeLoading" => {}
            "notifyError" => {
                if let Some(value) = Self::first_payload_value(payload)
                    && let Some(msg) = value.as_str()
                {
                    state.last_error = Some(msg.to_string());
                }
            }
            "setSystemMessages" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match Self::parse_join_requests_snapshot(value) {
                        Ok(requests) => {
                            state.replace_join_requests(requests);
                            state.last_error = None;
                        }
                        Err(e) => {
                            state.last_error = Some(format!("setSystemMessages 解析失败: {}", e));
                        }
                    }
                }
            }
            "commandFailed" => {
                state.last_error = Self::payload_message(payload);
            }
            _ => {}
        }
    }
}
