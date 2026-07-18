use crate::config::ReEditDraftConflictMode;
use crate::ica::IcaCommand;
use crate::ica::types::RoomId;

use crate::app::{IcaApp, SelectedChatGroup, VisibleRoomIndicesCache};

/// 判断切换会话时是否需要请求一份完整的消息快照。
///
/// 实时收到的 `addMessage` 只能证明本地缓存里存在零散消息，不能证明已经请求过该
/// 会话的历史快照。因此这个标记只应在主动发起请求或收到 `setMessages` 时维护。
fn should_request_messages_on_room_select(
    has_requested_snapshot: bool,
    room_changed: bool,
    auto_fetch_history_on_select: bool,
) -> bool {
    !has_requested_snapshot || (room_changed && auto_fetch_history_on_select)
}

impl IcaApp {
    pub fn request_group_members(&mut self, bridge_idx: usize, room_id: RoomId, force: bool) {
        if room_id >= 0 {
            return;
        }
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let conversation = state.conversation_mut(room_id);
        if conversation.loading_group_members || (!force && conversation.group_members_loaded) {
            return;
        }
        conversation.loading_group_members = true;
        if let Err(e) =
            self.bridge_states[bridge_idx].send(IcaCommand::FetchGroupMembers { room_id })
        {
            self.bridge_states[bridge_idx]
                .conversation_mut(room_id)
                .loading_group_members = false;
            self.bridge_states[bridge_idx].last_error = Some(format!("群成员列表请求失败: {e}"));
        }
    }

    pub fn request_room_messages(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        scroll_to_bottom: bool,
    ) {
        let Some(command_tx) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| session.command_sender())
        else {
            return;
        };

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            let conversation = state.conversation_mut(room_id);
            if scroll_to_bottom {
                conversation.pending_message_scroll_to_bottom = true;
            } else {
                conversation.pending_message_scroll_to_bottom = false;
                conversation.scroll_to_bottom = false;
            }
        }

        if let Err(e) = command_tx.send(IcaCommand::FetchMessages(room_id)) {
            tracing::warn!("send fetchMessages command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                // 命令没有进入后台任务时必须撤销占位，否则关闭自动刷新后，后续点击
                // 会误以为该房间已经请求过，从而失去重试机会。
                let conversation = state.conversation_mut(room_id);
                conversation.requested_snapshot = false;
                conversation.pending_message_scroll_to_bottom = false;
                state.last_error = Some(format!("历史消息请求发送失败: {e}"));
            }
        }
    }

    fn request_latest_room_history(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        current_loaded_messages: usize,
    ) {
        let Some(session) = self.bridge_states.get(bridge_idx) else {
            return;
        };
        if let Err(e) = session.send(IcaCommand::FetchLatestHistory {
            room_id,
            current_loaded_messages,
        }) {
            tracing::warn!("send fetchHistory command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("最新历史拉取命令发送失败: {e}"));
            }
        }
    }

    pub fn request_older_messages(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(command_tx) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| session.command_sender())
        else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let conversation = state.conversation_mut(room_id);
        if conversation.loading_older_messages || conversation.no_more_history {
            return;
        }
        let offset = conversation.messages.len();
        conversation.loading_older_messages = true;

        if let Err(e) = command_tx.send(IcaCommand::FetchOlderMessages { room_id, offset }) {
            tracing::warn!("send fetchOlderMessages command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.conversation_mut(room_id).loading_older_messages = false;
            }
        }
    }

    pub fn request_system_messages(&self, bridge_idx: usize) {
        let Some(session) = self.bridge_states.get(bridge_idx) else {
            return;
        };

        if let Err(e) = session.send(IcaCommand::GetSystemMsg) {
            tracing::warn!("send getSystemMsg command failed: {}", e);
        }
    }

    pub fn visible_room_indices(&mut self, bridge_idx: usize) -> Vec<usize> {
        let disable_chat_group = self.custom_chat.disable_chat_group;
        let Some(current_state) = self.bridge_states.get(bridge_idx) else {
            return Vec::new();
        };
        let selected_chat_group = current_state.selected_chat_group.clone();
        let selected_group = if !disable_chat_group {
            match &selected_chat_group {
                SelectedChatGroup::Custom(idx) => current_state
                    .chat_groups
                    .groups
                    .get(*idx)
                    .map(|group| (group.rooms.clone(), group.include_all_personal)),
                _ => None,
            }
        } else {
            None
        };

        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return Vec::new();
        };

        if let Some(cache) = &state.visible_room_indices_cache
            && cache.revision == state.rooms_revision
            && cache.query == state.room_search_query
            && cache.selected_chat_group == selected_chat_group
            && cache.disable_chat_group == disable_chat_group
        {
            return cache.indices.clone();
        }

        let query = state.room_search_query.trim().to_uppercase();
        let mut room_indices: Vec<_> = state
            .rooms
            .iter()
            .enumerate()
            .filter(|(_, room)| {
                if disable_chat_group {
                    return true;
                }
                match &selected_chat_group {
                    SelectedChatGroup::All => true,
                    SelectedChatGroup::Private => room.room_id > 0,
                    SelectedChatGroup::Custom(_) => {
                        selected_group
                            .as_ref()
                            .is_some_and(|(rooms, include_all_personal)| {
                                rooms.contains(&room.room_id)
                                    || (*include_all_personal && room.room_id > 0)
                            })
                    }
                }
            })
            .filter(|(_, room)| {
                query.is_empty()
                    || room.room_name.to_uppercase().contains(&query)
                    || room.room_id.to_string().contains(query.as_str())
            })
            .map(|(idx, _)| idx)
            .collect();

        room_indices.sort_by(|&a_idx, &b_idx| {
            let a = &state.rooms[a_idx];
            let b = &state.rooms[b_idx];
            let pinned_a = a.index > 0;
            let pinned_b = b.index > 0;
            pinned_b.cmp(&pinned_a).then(b.utime.cmp(&a.utime))
        });
        state.visible_room_indices_cache = Some(VisibleRoomIndicesCache {
            revision: state.rooms_revision,
            query: state.room_search_query.clone(),
            selected_chat_group,
            disable_chat_group,
            indices: room_indices,
        });
        state
            .visible_room_indices_cache
            .as_ref()
            .map(|cache| cache.indices.clone())
            .unwrap_or_default()
    }

    pub fn set_clear_search_on_room_select(&mut self, enabled: bool) {
        self.clear_search_on_room_select = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.clear_search_on_room_select = enabled;
        });
        if let Some(state) = self.active_bridge_state_mut() {
            state.invalidate_visible_room_indices();
        }
    }

    pub fn set_scroll_to_bottom_after_send(&mut self, enabled: bool) {
        self.scroll_to_bottom_after_send = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.scroll_to_bottom_after_send = enabled;
        });
    }

    pub fn set_auto_fetch_history_on_room_select(&mut self, enabled: bool) {
        self.auto_fetch_history_on_room_select = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.auto_fetch_history_on_room_select = enabled;
        });
    }

    pub fn set_reedit_draft_conflict_mode(&mut self, mode: ReEditDraftConflictMode) {
        self.reedit_draft_conflict_mode = mode;
        self.update_config(|cfg| {
            cfg.ui_setting.reedit_draft_conflict_mode = mode;
        });
    }

    pub fn select_active_room(&mut self, room_id: RoomId) {
        self.show_face_picker = false;
        let selected_room_changed = self
            .active_bridge_state()
            .is_none_or(|state| state.selected_room_id != Some(room_id));
        if selected_room_changed {
            self.group_member_panel.confirmation = None;
        }
        if room_id > 0 {
            self.group_member_panel.open = false;
            self.group_member_panel.confirmation = None;
        }
        let mut should_request = false;
        let clear_search_on_room_select = self.clear_search_on_room_select;
        let auto_fetch_history_on_select = self.auto_fetch_history_on_room_select;
        let auto_read = self.custom_chat.auto_read_on_select;
        let mut last_msg_id: Option<String> = None;
        let mut current_loaded_messages = 0;
        if let Some(state) = self.active_bridge_state_mut() {
            let room_changed = state.selected_room_id != Some(room_id);
            state.selected_room_id = Some(room_id);
            state.trim_message_caches(Some(room_id));
            if clear_search_on_room_select {
                state.room_search_query.clear();
                state.invalidate_visible_room_indices();
            }
            // `requested_rooms` 表示已经发起过完整快照请求，而不是“缓存里碰巧有消息”。
            // 首次打开始终请求；开关启用后，切换回来也刷新。
            let conversation = state.conversation_mut(room_id);
            conversation.scroll_to_message_id = None;
            conversation.scroll_to_message_attempts = 0;
            should_request = should_request_messages_on_room_select(
                conversation.requested_snapshot,
                room_changed,
                auto_fetch_history_on_select,
            );
            if should_request {
                // 先占位可以避免响应返回前连续重绘或重复点击产生并发请求。
                conversation.requested_snapshot = true;
                current_loaded_messages = conversation.messages.len();
            }
            if auto_read {
                last_msg_id = conversation.messages.last().map(|m| m.msg_id.clone());
            }
        }

        if should_request && let Some(bridge_idx) = self.active_bridge_idx {
            if auto_fetch_history_on_select {
                // 与 Icalingua++ 的 fetchHistoryOnChatOpen 行为保持一致：先请求协议端
                // 漫游历史，再读取 bridge 当前缓存。漫游拉取完成后，bridge 还会广播
                // 新的 setMessages，届时界面会自动替换为更新后的完整列表。
                self.request_latest_room_history(bridge_idx, room_id, current_loaded_messages);
            }
            self.request_room_messages(bridge_idx, room_id, true);
        }

        if auto_read
            && let Some(msg_id) = last_msg_id
            && let Some(bridge_idx) = self.active_bridge_idx
            && let Some(session) = self.bridge_states.get(bridge_idx)
        {
            let _ = session.send(IcaCommand::ReportRead {
                room_id,
                message_id: msg_id,
            });
        }

        if selected_room_changed
            && room_id < 0
            && self.group_member_panel.open
            && let Some(bridge_idx) = self.active_bridge_idx
        {
            self.request_group_members(bridge_idx, room_id, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_request_messages_on_room_select;

    #[test]
    fn first_room_open_always_requests_complete_snapshot() {
        assert!(should_request_messages_on_room_select(false, true, false));
    }

    #[test]
    fn loaded_room_only_refreshes_after_switch_when_option_is_enabled() {
        assert!(!should_request_messages_on_room_select(true, true, false));
        assert!(should_request_messages_on_room_select(true, true, true));
        assert!(!should_request_messages_on_room_select(true, false, true));
    }
}
