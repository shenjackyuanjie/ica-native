//! 好友与群联系人列表相关的事件。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::BridgeState;
use crate::ica::types::contact::{FriendContact, GroupContact};

use super::payload;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "contactsPartResponse" => {
            let request_id = payload
                .get("requestId")
                .and_then(JsonValue::as_u64)
                .unwrap_or_default();
            let part = payload
                .get("part")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            let Some(items) = payload.get("items") else {
                state.contacts.lock().unwrap().fail_part(
                    request_id,
                    part,
                    "联系人响应缺少 items".to_string(),
                );
                return true;
            };

            match part {
                "friends" => match Vec::<FriendContact>::deserialize(items) {
                    Ok(mut friends) => {
                        friends.sort_by_key(|friend| friend.uin);
                        friends.dedup_by_key(|friend| friend.uin);
                        friends.sort_by(|left, right| {
                            left.display_name()
                                .to_lowercase()
                                .cmp(&right.display_name().to_lowercase())
                                .then(left.uin.cmp(&right.uin))
                        });
                        state
                            .contacts
                            .lock()
                            .unwrap()
                            .apply_friends(request_id, friends);
                    }
                    Err(error) => {
                        payload::log_event_parse_failure(
                            state,
                            "contactsPartResponse/friends",
                            &error.to_string(),
                            items,
                        );
                        let message = format!("好友列表解析失败: {error}");
                        if state.contacts.lock().unwrap().fail_part(
                            request_id,
                            part,
                            message.clone(),
                        ) {
                            state.last_error = Some(message);
                        }
                    }
                },
                "groups" => match Vec::<GroupContact>::deserialize(items) {
                    Ok(mut groups) => {
                        groups.sort_by_key(|group| group.group_id);
                        groups.dedup_by_key(|group| group.group_id);
                        groups.sort_by(|left, right| {
                            left.display_name()
                                .to_lowercase()
                                .cmp(&right.display_name().to_lowercase())
                                .then(left.group_id.cmp(&right.group_id))
                        });
                        state
                            .contacts
                            .lock()
                            .unwrap()
                            .apply_groups(request_id, groups);
                    }
                    Err(error) => {
                        payload::log_event_parse_failure(
                            state,
                            "contactsPartResponse/groups",
                            &error.to_string(),
                            items,
                        );
                        let message = format!("群列表解析失败: {error}");
                        if state.contacts.lock().unwrap().fail_part(
                            request_id,
                            part,
                            message.clone(),
                        ) {
                            state.last_error = Some(message);
                        }
                    }
                },
                _ => {}
            }
        }
        "contactsPartFailed" => {
            let request_id = payload
                .get("requestId")
                .and_then(JsonValue::as_u64)
                .unwrap_or_default();
            let part = payload
                .get("part")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();
            let message =
                payload::payload_message(payload).unwrap_or_else(|| "联系人请求失败".to_string());
            if state
                .contacts
                .lock()
                .unwrap()
                .fail_part(request_id, part, message.clone())
            {
                state.last_error = Some(message);
            }
        }
        _ => return false,
    }
    true
}
