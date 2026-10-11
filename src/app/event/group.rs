//! 群成员列表与群管理操作相关的事件。
//!
//! 处理群成员响应（`groupMembersResponse`）与群管理请求确认（`groupBanRequested`）。
//! 后者只把“正在刷新成员列表”置位并提示，真正的禁言/踢人等结果再由随后的成员
//! 响应或命令失败事件反映。成员按展示名排序，保证界面顺序稳定。

use crate::app::state::{BridgeState, GroupMember};
use serde::Deserialize;
use serde_json::Value as JsonValue;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
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
            // 群管理操作（禁言/踢人等）会触发成员列表变动，先置位加载中，
            // 随后的 groupMembersResponse 或 commandFailed 再据此收尾。
            state.conversation_mut(room_id).loading_group_members = true;
            state.last_notice = Some("群管理请求已发送，稍后刷新成员列表".to_string());
        }
        _ => return false,
    }
    true
}
