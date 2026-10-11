//! 好友与群联系人列表相关的事件。
//!
//! 处理 `contactsPartResponse` / `contactsPartFailed`：Bridge 把好友与群分成两批
//! 异步返回，这里按 `requestId` 归属写入对应的 `ContactDirectory`，经去重、排序后
//! 供联系人窗口渲染。`requestId` 不匹配的迟到响应会被静默丢弃，避免覆盖更新结果。

use super::payload;
use crate::app::state::BridgeState;
use crate::ica::types::contact::{FriendContact, GroupContact};
use serde::Deserialize;
use serde_json::Value as JsonValue;

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
                        // 先按 uin 排序去重（保留首个），再按展示名排序；
                        // 保证 Bridge 端重复项不会渲染成两行，同名时顺序稳定。
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
                        // 与好友一致：先按 group_id 去重，再按展示名排序。
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
