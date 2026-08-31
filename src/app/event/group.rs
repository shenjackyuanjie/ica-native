//! 群成员列表与群管理操作相关的事件。

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::app::state::{BridgeState, GroupMember};

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub(super) fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "groupMembersResponse" => {
            let room_id = payload
                .get("roomId")
                .and_then(JsonValue::as_i64)
                .unwrap_or_default();
            state.conversation_mut(room_id).loading_group_members = false;
            match payload.get("members").map(Vec::<GroupMember>::deserialize) {
                Some(Ok(mut members)) => {
                    members.sort_by(|left, right| {
                        left.display_name()
                            .cmp(right.display_name())
                            .then(left.user_id.cmp(&right.user_id))
                    });
                    let conversation = state.conversation_mut(room_id);
                    conversation.group_members = members;
                    conversation.group_members_loaded = true;
                }
                Some(Err(e)) => {
                    state.last_error = Some(format!("群成员列表解析失败: {e}"));
                }
                None => {
                    state.last_error = Some("群成员列表响应缺少 members".to_string());
                }
            }
        }
        "groupBanRequested" => {
            let room_id = payload
                .get("roomId")
                .and_then(JsonValue::as_i64)
                .unwrap_or_default();
            state.conversation_mut(room_id).loading_group_members = true;
            state.last_notice = Some("群管理请求已发送，稍后刷新成员列表".to_string());
        }
        _ => return false,
    }
    true
}
