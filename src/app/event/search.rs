//! 聊天记录搜索与成员发言记录分页相关的事件。
//!
//! 处理两类分页拉取：全局聊天记录搜索（`searchMessagesResponse`）与单个群成员的
//! 发言记录（`memberHistoryResponse`）。两者都按 `offset` 决定覆盖首页还是向前追加，
//! 并用请求序号与 msg_id 去重防止迟到响应造成重复或串页。

use super::payload;
use crate::app::state::BridgeState;
use crate::ica::types::message::Message;
use serde::Deserialize;
use serde_json::Value as JsonValue;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
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
                    state.trim_message_search_results();
                }
                Some(Err(e)) => {
                    payload::log_event_parse_failure(
                        state,
                        "searchMessagesResponse",
                        &e.to_string(),
                        payload,
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
        "memberHistoryResponse" => {
            let request_id = payload
                .get("requestId")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            let offset = payload
                .get("offset")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as usize;
            // 请求序号不匹配说明是过期分页的迟到响应，认领但不改动状态。
            if request_id != state.member_history.request_id {
                return true;
            }
            state.member_history.loading = false;
            match payload.get("messages").map(Vec::<Message>::deserialize) {
                Some(Ok(messages)) => {
                    // 返回不足一页即认为已到底；随后按 msg_id 去重，
                    // offset==0 覆盖首页，否则把新页拼到已有结果前面（按时间倒序翻页）。
                    const MEMBER_HISTORY_PAGE_SIZE: usize = 20;
                    state.member_history.exhausted = messages.len() < MEMBER_HISTORY_PAGE_SIZE;
                    let mut existing_ids = state
                        .member_history
                        .messages
                        .iter()
                        .map(|message| message.msg_id.clone())
                        .collect::<std::collections::HashSet<_>>();
                    let fresh = messages
                        .into_iter()
                        .filter(|message| existing_ids.insert(message.msg_id.clone()))
                        .collect::<Vec<_>>();
                    if offset == 0 {
                        state.member_history.messages = fresh;
                    } else if fresh.is_empty() {
                        state.member_history.exhausted = true;
                    } else {
                        let mut combined = fresh;
                        combined.append(&mut state.member_history.messages);
                        state.member_history.messages = combined;
                    }
                }
                Some(Err(error)) => {
                    state.last_error = Some(format!("成员发言记录解析失败: {error}"))
                }
                None => state.last_error = Some("成员发言记录响应缺少 messages".to_string()),
            }
        }
        _ => return false,
    }
    true
}
