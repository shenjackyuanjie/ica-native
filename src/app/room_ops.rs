use crate::cfg::{self, ReEditDraftConflictMode};
use crate::ica::IcaCommand;
use crate::ica::types::RoomId;

use super::{IcaApp, SelectedChatGroup, VisibleRoomIndicesCache};

impl IcaApp {
    pub fn request_group_members(&mut self, bridge_idx: usize, room_id: RoomId, force: bool) {
        if room_id >= 0 {
            return;
        }
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        if state.loading_group_members.contains(&room_id)
            || (!force && state.group_members_by_room.contains_key(&room_id))
        {
            return;
        }
        state.loading_group_members.insert(room_id);
        if force {
            state.group_members_by_room.remove(&room_id);
        }
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::FetchGroupMembers { room_id })
        {
            self.bridge_states[bridge_idx]
                .loading_group_members
                .remove(&room_id);
            self.bridge_states[bridge_idx].last_error = Some(format!("群成员列表请求失败: {e}"));
        }
    }

    pub fn request_room_messages(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        scroll_to_bottom: bool,
    ) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            if scroll_to_bottom {
                state.pending_message_scroll_to_bottom.insert(room_id);
            } else {
                state.pending_message_scroll_to_bottom.remove(&room_id);
                state.message_scroll_to_bottom.remove(&room_id);
            }
        }

        if let Err(e) = client.command_tx.send(IcaCommand::FetchMessages(room_id)) {
            tracing::warn!("send fetchMessages command failed: {}", e);
        }
    }

    pub fn request_older_messages(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        if state.loading_older_messages.contains(&room_id)
            || state.no_more_history.contains(&room_id)
        {
            return;
        }
        let offset = state.messages_by_room.get(&room_id).map_or(0, |m| m.len());
        state.loading_older_messages.insert(room_id);

        if let Err(e) = client
            .command_tx
            .send(IcaCommand::FetchOlderMessages { room_id, offset })
        {
            tracing::warn!("send fetchOlderMessages command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.loading_older_messages.remove(&room_id);
            }
        }
    }

    pub fn request_system_messages(&self, bridge_idx: usize) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };

        if let Err(e) = client.command_tx.send(IcaCommand::GetSystemMsg) {
            tracing::warn!("send getSystemMsg command failed: {}", e);
        }
    }

    pub fn visible_room_indices(&mut self, bridge_idx: usize) -> Vec<usize> {
        let disable_chat_group = self.custom_chat.disable_chat_group;
        let selected_chat_group = self.selected_chat_group.clone();
        let selected_group = if !disable_chat_group {
            match selected_chat_group {
                SelectedChatGroup::Custom(idx) => self
                    .chat_groups
                    .groups
                    .get(idx)
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
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.clear_search_on_room_select = enabled;
        });
        if let Some(state) = self.active_bridge_state_mut() {
            state.invalidate_visible_room_indices();
        }
    }

    pub fn set_scroll_to_bottom_after_send(&mut self, enabled: bool) {
        self.scroll_to_bottom_after_send = enabled;
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.scroll_to_bottom_after_send = enabled;
        });
    }

    pub fn set_reedit_draft_conflict_mode(&mut self, mode: ReEditDraftConflictMode) {
        self.reedit_draft_conflict_mode = mode;
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.reedit_draft_conflict_mode = mode;
        });
    }

    pub fn select_active_room(&mut self, room_id: RoomId) {
        self.show_face_picker = false;
        let mut should_request = false;
        let clear_search_on_room_select = self.clear_search_on_room_select;
        let auto_read = self.custom_chat.auto_read_on_select;
        let mut last_msg_id: Option<String> = None;
        if let Some(state) = self.active_bridge_state_mut() {
            state.selected_room_id = Some(room_id);
            state.scroll_to_message_id = None;
            state.scroll_to_message_attempts = 0;
            state.trim_message_caches(Some(room_id));
            if clear_search_on_room_select {
                state.room_search_query.clear();
                state.invalidate_visible_room_indices();
            }
            should_request = state.requested_rooms.insert(room_id);
            if auto_read {
                last_msg_id = state
                    .messages_by_room
                    .get(&room_id)
                    .and_then(|msgs| msgs.last())
                    .map(|m| m.msg_id.clone());
            }
        }

        if should_request && let Some(bridge_idx) = self.active_bridge_idx {
            self.request_room_messages(bridge_idx, room_id, true);
        }

        if auto_read
            && let Some(msg_id) = last_msg_id
            && let Some(bridge_idx) = self.active_bridge_idx
            && let Some(client) = self.ica_clients.get(bridge_idx)
        {
            let _ = client.command_tx.send(IcaCommand::ReportRead {
                room_id,
                message_id: msg_id,
            });
        }
    }
}
