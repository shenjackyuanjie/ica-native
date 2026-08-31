//! 消息列表与单条消息变更相关的事件。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::message::{Message, NewMessage};

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "setMessages" => {
            if let Some(value) = payload::first_payload_value(payload) {
                let room_id = value["roomId"].as_i64().unwrap_or_default();
                match Vec::<Message>::deserialize(&value["messages"]) {
                    Ok(messages) => {
                        let conversation = state.conversation_mut(room_id);
                        conversation.requested_snapshot = true;
                        if std::mem::take(&mut conversation.pending_message_scroll_to_bottom) {
                            conversation.scroll_to_bottom = true;
                        }
                        conversation.no_more_history = false;
                        conversation.loading_older_messages = false;
                        conversation.new_message_count = 0;
                        conversation.message_row_heights.clear();
                        conversation.message_row_layouts.clear();
                        conversation.message_layout_cache_key = None;
                        conversation.last_content_height = None;
                        conversation.messages = messages;
                        state.trim_message_caches(state.selected_room_id);
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(
                            state,
                            "setMessages",
                            &e.to_string(),
                            value,
                        );
                        state.last_error = Some(format!("setMessages 解析失败: {}", e));
                    }
                }
            }
        }
        "appendOlderMessages" => {
            if let Some(value) = payload::first_payload_value(payload) {
                let room_id = value["roomId"].as_i64().unwrap_or_default();
                // 不管解析成功与否都要重置加载状态
                state.conversation_mut(room_id).loading_older_messages = false;
                match Vec::<Message>::deserialize(&value["messages"]) {
                    Ok(older_messages) => {
                        if older_messages.is_empty() {
                            // 没有更多历史消息
                            state.conversation_mut(room_id).no_more_history = true;
                        } else {
                            // 合并到已有消息列表前面（去重）
                            let existing = &mut state.conversation_mut(room_id).messages;
                            let existing_ids: std::collections::HashSet<&str> =
                                existing.iter().map(|m| m.msg_id.as_str()).collect();
                            let mut new_msgs: Vec<Message> = older_messages
                                .into_iter()
                                .filter(|m| !existing_ids.contains(m.msg_id.as_str()))
                                .collect();
                            // 将旧消息放在前面
                            new_msgs.append(existing);
                            *existing = new_msgs;
                            // 前插消息会改变新旧消息交界处是否需要日期分隔框，
                            // 缓存高度包含分隔框，因此这里必须整体失效。
                            state.conversation_mut(room_id).message_row_heights.clear();
                            state.invalidate_message_rows(room_id);
                            // 标记需要调整 scroll offset
                            state.conversation_mut(room_id).prepend_scroll_fix = true;
                            state.trim_after_history_prepend(room_id);
                        }
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(
                            state,
                            "appendOlderMessages",
                            &e.to_string(),
                            value,
                        );
                    }
                }
            }
        }
        "addMessage" => {
            if let Some(value) = payload::first_payload_value(payload) {
                match NewMessage::deserialize(value) {
                    Ok(new_message) => {
                        let room_id = new_message.room_id;
                        let is_selected_room = state.selected_room_id == Some(room_id);
                        let (pending_send_scroll, near_bottom) = {
                            let conversation = state.conversation_mut(room_id);
                            (
                                std::mem::take(&mut conversation.pending_send_scroll_to_bottom),
                                conversation.near_bottom,
                            )
                        };
                        let should_scroll_to_bottom = new_message.msg.sender_id
                            == state.online_data.qqid
                            && pending_send_scroll;
                        let should_follow_new_message = is_selected_room && near_bottom;
                        // 实时消息可能早于用户首次打开会话到达。它只是一条增量消息，
                        // 不能把房间误标记成“完整历史已经加载”，否则首次点击时不会
                        // 再发送 fetchMessages，界面就可能永远只显示这一两条消息。
                        state.sync_room_preview(room_id, &new_message.msg);
                        let inserted = state.upsert_message(room_id, new_message.msg);
                        state.trim_message_caches(state.selected_room_id);
                        let conversation = state.conversation_mut(room_id);
                        if should_scroll_to_bottom || should_follow_new_message {
                            conversation.scroll_to_bottom = true;
                            conversation.new_message_count = 0;
                        } else if inserted && is_selected_room {
                            conversation.new_message_count =
                                conversation.new_message_count.saturating_add(1);
                        }
                    }
                    Err(e) => {
                        payload::log_event_parse_failure(
                            state,
                            "addMessage",
                            &e.to_string(),
                            value,
                        );
                        state.last_error = Some(format!("addMessage 解析失败: {}", e));
                    }
                }
            }
        }
        "deleteMessage" => {
            if let Some(msg_id) =
                payload::first_payload_value(payload).and_then(|value| value.as_str())
            {
                state.mark_message_deleted(msg_id);
            }
        }
        "hideMessage" => {
            if let Some(msg_id) =
                payload::first_payload_value(payload).and_then(|value| value.as_str())
            {
                state.mark_message_hidden(msg_id);
            }
        }
        "revealMessage" => {
            if let Some(msg_id) =
                payload::first_payload_value(payload).and_then(|value| value.as_str())
            {
                state.mark_message_revealed(msg_id);
            }
        }
        "renewMessage" => {
            if let Some(value) = payload::first_payload_value(payload) {
                let room_id = value["roomId"].as_i64().unwrap_or_default();
                let mut changed_message_id = None;
                if let Some(msg_id) = value["messageId"].as_str()
                    && let Some(messages) = state
                        .conversations
                        .get_mut(&room_id)
                        .map(|conversation| &mut conversation.messages)
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
            if let Some(value) = payload::first_payload_value(payload) {
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
                    return true;
                };
                if message_id.is_empty() {
                    return true;
                }

                for conversation in state.conversations.values_mut() {
                    let messages = &mut conversation.messages;
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
        "messageSuccess" => {
            state.last_notice = payload::first_payload_display_message(payload);
        }
        "messageError" => {
            if let Some(msg) = payload::first_payload_display_message(payload) {
                state.last_error = Some(msg);
            } else {
                state.last_error = Some("消息发送失败".to_string());
            }
        }
        "addMessageText" => {
            if let Some(text) =
                payload::first_payload_value(payload).and_then(|value| value.as_str())
                && let Some(room_id) = state.selected_room_id
            {
                state.conversation_mut(room_id).draft.push_str(text);
            }
        }
        "notifyMessage" => {
            state.last_notice = payload::first_payload_display_message(payload);
        }
        _ => return false,
    }
    true
}
