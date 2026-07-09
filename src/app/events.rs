use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::ica::types::{
    message::{Message, NewMessage},
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};

use super::IcaApp;
use super::state::{AuthState, BridgeState, GroupMember, SocketState};

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

    fn value_to_display_message(value: &JsonValue) -> Option<String> {
        if let Some(msg) = value.as_str() {
            return Some(msg.to_string());
        }

        let title = value.get("title").and_then(|value| value.as_str());
        let message = value.get("message").and_then(|value| value.as_str());
        match (title, message) {
            (Some(title), Some(message)) if !title.is_empty() && !message.is_empty() => {
                Some(format!("{title}: {message}"))
            }
            (_, Some(message)) if !message.is_empty() => Some(message.to_string()),
            (Some(title), _) if !title.is_empty() => Some(title.to_string()),
            _ => None,
        }
    }

    fn first_payload_display_message(payload: &JsonValue) -> Option<String> {
        Self::first_payload_value(payload).and_then(Self::value_to_display_message)
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
        value: &JsonValue,
        fallback_flag: Option<&str>,
    ) -> Result<JoinRequestRoom, String> {
        let mut request = JoinRequestRoom::deserialize(value).map_err(|e| e.to_string())?;
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
                .map(|item| Self::parse_join_request(item, None))
                .collect(),
            JsonValue::Object(items) => items
                .iter()
                .map(|(flag, item)| Self::parse_join_request(item, Some(flag)))
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
                    match Vec::<Room>::deserialize(value) {
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
                    match Vec::<Message>::deserialize(&value["messages"]) {
                        Ok(messages) => {
                            state.requested_rooms.insert(room_id);
                            if state.pending_message_scroll_to_bottom.remove(&room_id) {
                                state.message_scroll_to_bottom.insert(room_id);
                            }
                            // 重置历史加载状态（新的 setMessages 意味着全量刷新）
                            state.no_more_history.remove(&room_id);
                            state.loading_older_messages.remove(&room_id);
                            state.new_message_counts.remove(&room_id);
                            state.invalidate_message_layout(room_id);
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
                    match Vec::<Message>::deserialize(&value["messages"]) {
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
                                state.invalidate_message_rows(room_id);
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
                    match NewMessage::deserialize(value) {
                        Ok(new_message) => {
                            let room_id = new_message.room_id;
                            let should_scroll_to_bottom = new_message.msg.sender_id
                                == state.online_data.qqid
                                && state.pending_send_scroll_to_bottom.remove(&room_id);
                            let is_selected_room = state.selected_room_id == Some(room_id);
                            let should_follow_new_message =
                                is_selected_room && state.message_near_bottom.contains(&room_id);
                            state.requested_rooms.insert(room_id);
                            state.sync_room_preview(room_id, &new_message.msg);
                            let inserted = state.upsert_message(room_id, new_message.msg);
                            if should_scroll_to_bottom || should_follow_new_message {
                                state.message_scroll_to_bottom.insert(room_id);
                                state.new_message_counts.remove(&room_id);
                            } else if inserted && is_selected_room {
                                let count = state.new_message_counts.entry(room_id).or_default();
                                *count = count.saturating_add(1);
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
                    match Self::parse_join_request(value, None) {
                        Ok(request) => {
                            state.upsert_join_request(request);
                        }
                        Err(e) => {
                            state.last_error = Some(format!("handleRequest 解析失败: {}", e));
                        }
                    }
                }
            }
            "sendAddRequest" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match Self::parse_join_request(value, None) {
                        Ok(request) => {
                            state.upsert_join_request(request);
                            state.last_notice = Some("收到新的验证消息".to_string());
                        }
                        Err(e) => {
                            state.last_error = Some(format!("sendAddRequest 解析失败: {}", e));
                        }
                    }
                }
            }
            "updateRoom" => {
                if let Some(value) = Self::first_payload_value(payload) {
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
                    let mut changed_message_id = None;
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
                        changed_message_id = Some(msg_id.to_string());
                    }
                    if let Some(msg_id) = changed_message_id {
                        state.invalidate_message_height(&msg_id);
                    }
                }
            }
            "renewMessageURL" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let message_id = value
                        .get("messageId")
                        .and_then(|value| {
                            value
                                .as_str()
                                .map(ToString::to_string)
                                .or_else(|| value.as_i64().map(|id| id.to_string()))
                        })
                        .unwrap_or_default();
                    let Some(url) = value.get("URL").and_then(|value| value.as_str()) else {
                        return;
                    };
                    if message_id.is_empty() {
                        return;
                    }

                    for messages in state.messages_by_room.values_mut() {
                        if let Some(message) = messages
                            .iter_mut()
                            .find(|message| message.msg_id == message_id)
                        {
                            for file in &mut message.files {
                                if file.file_type.eq_ignore_ascii_case("image")
                                    || file.file_type.starts_with("image/")
                                {
                                    file.url = url.to_string();
                                }
                            }
                            state.invalidate_message_height(&message_id);
                            break;
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
            "setShutUp" => {
                state.is_shut_up = Self::first_payload_value(payload)
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
            }
            "messageSuccess" => {
                state.last_notice = Self::first_payload_display_message(payload);
            }
            "messageError" => {
                if let Some(msg) = Self::first_payload_display_message(payload) {
                    state.last_error = Some(msg);
                } else {
                    state.last_error = Some("消息发送失败".to_string());
                }
            }
            "addMessageText" => {
                if let Some(text) =
                    Self::first_payload_value(payload).and_then(|value| value.as_str())
                    && let Some(room_id) = state.selected_room_id
                {
                    let draft = state.draft_by_room.entry(room_id).or_default();
                    draft.push_str(text);
                }
            }
            "notifyMessage" => {
                state.last_notice = Self::first_payload_display_message(payload);
            }
            "closeLoading" => {}
            "notifyError" => {
                if let Some(msg) = Self::first_payload_display_message(payload) {
                    state.last_error = Some(msg);
                }
            }
            "requestSetup" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    state.setup_requested = Some(Self::json_preview(value, 512));
                    state.last_error =
                        Some("bridge 尚未登录，需要先在 Icalingua++/bridge 完成登录".to_string());
                }
            }
            "fatal" => {
                let message = Self::first_payload_display_message(payload)
                    .unwrap_or_else(|| "bridge 发生致命错误".to_string());
                state.fatal_error = Some(message.clone());
                state.last_error = Some(message);
                state.socket_state = SocketState::Failed;
            }
            "login-verify" => {
                state.last_error = Some(
                    "bridge 请求网页登录验证；可在“账号/登录设备”窗口重试或完成验证".to_string(),
                );
            }
            "login-qrcodeLogin" => {
                state.last_error = Some(
                    "bridge 请求扫码登录；请查看 bridge 日志/二维码输出后在“账号/登录设备”继续"
                        .to_string(),
                );
            }
            "login-smsCodeVerify" => {
                state.last_error =
                    Some("bridge 请求短信验证码；可在“账号/登录设备”填写验证码".to_string());
            }
            "login-error" => {
                state.last_error = Self::first_payload_display_message(payload)
                    .or_else(|| Some("bridge 登录失败".to_string()));
            }
            "login-slider" => {
                state.last_error =
                    Some("bridge 请求滑块验证；可在“账号/登录设备”填写滑块 ticket".to_string());
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
                if payload.get("kind").and_then(JsonValue::as_str) == Some("fetchGroupMembers")
                    && let Some(room_id) = payload.get("roomId").and_then(JsonValue::as_i64)
                {
                    state.loading_group_members.remove(&room_id);
                }
                if payload.get("kind").and_then(JsonValue::as_str) == Some("searchMessages") {
                    let message = Self::payload_message(payload)
                        .unwrap_or_else(|| "搜索聊天记录失败".to_string());
                    state.message_search.fail(message);
                }
                state.last_error = Self::payload_message(payload);
            }
            "searchMessagesResponse" => {
                let room_id = payload
                    .get("roomId")
                    .and_then(JsonValue::as_i64)
                    .unwrap_or_default();
                let keyword = payload
                    .get("keyword")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                let offset = payload
                    .get("offset")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or_default() as usize;

                match payload.get("messages").map(Vec::<Message>::deserialize) {
                    Some(Ok(messages)) => {
                        state
                            .message_search
                            .apply_response(room_id, keyword, offset, messages);
                    }
                    Some(Err(e)) => {
                        tracing::warn!(
                            "searchMessages parse failed: bridge={} room_id={} err={} raw={}",
                            state.bridge_key,
                            room_id,
                            e,
                            Self::json_preview(&payload["messages"], 512)
                        );
                        state
                            .message_search
                            .fail(format!("搜索结果解析失败: {}", e));
                    }
                    None => {
                        state
                            .message_search
                            .fail("搜索结果响应缺少 messages".to_string());
                    }
                }
            }
            "groupMembersResponse" => {
                let room_id = payload
                    .get("roomId")
                    .and_then(JsonValue::as_i64)
                    .unwrap_or_default();
                state.loading_group_members.remove(&room_id);
                match payload.get("members").map(Vec::<GroupMember>::deserialize) {
                    Some(Ok(mut members)) => {
                        members.sort_by(|left, right| {
                            left.display_name()
                                .cmp(right.display_name())
                                .then(left.user_id.cmp(&right.user_id))
                        });
                        state.group_members_by_room.insert(room_id, members);
                    }
                    Some(Err(e)) => {
                        state.last_error = Some(format!("群成员列表解析失败: {e}"));
                    }
                    None => {
                        state.last_error = Some("群成员列表响应缺少 members".to_string());
                    }
                }
            }
            "socketApiResponse" | "fileManagerResponse" => {
                let response = Self::json_preview(payload, 1024);
                state.last_socket_api_response = Some(response.clone());
                let label = if event_name == "fileManagerResponse" {
                    "文件管理"
                } else {
                    "Socket API"
                };
                state.last_notice = Some(format!("{}: {}", label, response));
            }
            _ => {}
        }
    }
}
