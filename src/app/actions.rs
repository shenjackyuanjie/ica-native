use std::collections::HashSet;

use serde_json::Value as JsonValue;

use crate::cfg;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    message::{Message, SendMessage},
};

use super::{ChatGroups, IcaApp, OnlineMode, SelectedChatGroup};

impl IcaApp {
    fn extract_raw_chain(message: &Message) -> Option<JsonValue> {
        match &message.raw_msg {
            JsonValue::Array(values) if !values.is_empty() => {
                Some(JsonValue::Array(values.clone()))
            }
            JsonValue::Object(map) if map.contains_key("type") => {
                Some(JsonValue::Array(vec![message.raw_msg.clone()]))
            }
            _ => None,
        }
    }

    fn send_raw_chain(&mut self, room_id: RoomId, chain: JsonValue) -> bool {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendRawMessage {
                room_id,
                content: chain,
            })
        {
            tracing::warn!("send raw sendMessage command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("原样发送命令发送失败: {}", room_id));
            }
            return false;
        }

        if self.scroll_to_bottom_after_send
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.pending_send_scroll_to_bottom.insert(room_id);
        }
        true
    }

    fn clone_message_from_active_bridge(
        &self,
        room_id: RoomId,
        message_id: &str,
    ) -> Option<Message> {
        let bridge_idx = self.active_bridge_idx?;
        self.bridge_states
            .get(bridge_idx)?
            .find_message(room_id, message_id)
            .cloned()
    }

    fn selected_forward_messages(&self, bridge_idx: usize, room_id: RoomId) -> Vec<Message> {
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            return Vec::new();
        };
        if state.forward_room_id != Some(room_id) {
            return Vec::new();
        }

        let selected_ids: HashSet<&str> = state
            .forward_selected_message_ids
            .iter()
            .map(String::as_str)
            .collect();

        state
            .messages_by_room
            .get(&room_id)
            .into_iter()
            .flatten()
            .filter(|message| selected_ids.contains(message.msg_id.as_str()))
            .cloned()
            .collect()
    }

    fn send_message_clone_to_room(&mut self, target_room_id: RoomId, message: &Message) -> bool {
        if let Some(chain) = Self::extract_raw_chain(message) {
            return self.send_raw_chain(target_room_id, chain);
        }

        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };

        if message.content.trim().is_empty() {
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("该消息缺少可复用的原始节点，无法原样发送".to_string());
            }
            return false;
        }

        let outgoing = SendMessage::new(
            message.content.clone(),
            target_room_id,
            message.reply.clone(),
        );

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendMessage(outgoing))
        {
            tracing::warn!("send cloned sendMessage command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("消息发送失败: {}", target_room_id));
            }
            return false;
        }

        if !message.files.is_empty()
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.last_error = Some("部分附件消息缺少原始节点，已退化为纯文本发送".to_string());
        }

        if self.scroll_to_bottom_after_send
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.pending_send_scroll_to_bottom.insert(target_room_id);
        }
        true
    }

    pub fn copy_message_to_draft(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };

        if let Some(state) = self.active_bridge_state_mut() {
            state.draft_by_room.insert(room_id, message.content.clone());
            if let Some(reply) = message.reply.clone() {
                state.reply_to_by_room.insert(room_id, reply);
            } else {
                state.reply_to_by_room.remove(&room_id);
            }
            if !message.files.is_empty() {
                state.last_error =
                    Some("复制到编辑区暂不恢复附件，如需原样发送请使用 +1 或 转发".to_string());
            }
        }
    }

    pub fn plus_one_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };
        let _ = self.send_message_clone_to_room(room_id, &message);
    }

    pub fn begin_forward_selection(
        &mut self,
        room_id: RoomId,
        message_id: String,
        open_picker: bool,
    ) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.replace_forward_selection(room_id, message_id);
            state.forward_target_picker_open = open_picker;
            if open_picker {
                state.forward_target_search_query.clear();
            }
        }
    }

    pub fn toggle_forward_message_selection(&mut self, room_id: RoomId, message_id: String) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.toggle_forward_selection(room_id, message_id);
        }
    }

    pub fn plus_one_forward_selection(&mut self, room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let messages = self.selected_forward_messages(bridge_idx, room_id);
        if messages.is_empty() {
            return;
        }

        let mut failed = 0_usize;
        for message in &messages {
            if !self.send_message_clone_to_room(room_id, message) {
                failed += 1;
            }
        }

        if failed > 0
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.last_error = Some(format!("有 {} 条消息无法完整 +1", failed));
        }
    }

    pub fn open_forward_target_picker(&mut self, room_id: RoomId) {
        if let Some(state) = self.active_bridge_state_mut()
            && state.is_forward_selection_active(room_id)
        {
            state.forward_target_picker_open = true;
            state.forward_target_search_query.clear();
        }
    }

    pub fn clear_forward_selection(&mut self) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.clear_forward_selection();
        }
    }

    pub fn forward_selected_messages_to_room(&mut self, target_room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(source_room_id) = self.bridge_states[bridge_idx].forward_room_id else {
            return;
        };
        let messages = self.selected_forward_messages(bridge_idx, source_room_id);
        if messages.is_empty() {
            self.bridge_states[bridge_idx].clear_forward_selection();
            return;
        }

        let mut failed = 0_usize;
        for message in &messages {
            if !self.send_message_clone_to_room(target_room_id, message) {
                failed += 1;
            }
        }

        if failed > 0 {
            self.bridge_states[bridge_idx].last_error =
                Some(format!("有 {} 条消息无法完整转发", failed));
        }
        self.bridge_states[bridge_idx].clear_forward_selection();
    }

    pub fn save_chat_groups(&mut self) {
        let groups = self.chat_groups.clone();
        if let Some(state) = self.active_bridge_state_mut() {
            state.chat_groups = groups.clone();
        }
        cfg::update_and_save_cfg(|cfg| {
            cfg.chat_groups = groups;
        });
    }

    pub(super) fn send_add_chat_group(
        &self,
        bridge_idx: usize,
        name: &str,
        rooms: &[RoomId],
        include_all_personal: bool,
    ) {
        if let Some(client) = self.ica_clients.get(bridge_idx) {
            let _ = client.command_tx.send(IcaCommand::AddChatGroup {
                name: name.to_string(),
                rooms: rooms.to_vec(),
                include_all_personal,
            });
        }
    }

    pub(super) fn send_remove_chat_group(&self, bridge_idx: usize, name: &str) {
        if let Some(client) = self.ica_clients.get(bridge_idx) {
            let _ = client.command_tx.send(IcaCommand::RemoveChatGroup {
                name: name.to_string(),
            });
        }
    }

    pub(super) fn send_update_chat_group(
        &self,
        bridge_idx: usize,
        name: &str,
        rooms: &[RoomId],
        include_all_personal: bool,
    ) {
        if let Some(client) = self.ica_clients.get(bridge_idx) {
            let _ = client.command_tx.send(IcaCommand::UpdateChatGroup {
                name: name.to_string(),
                rooms: rooms.to_vec(),
                include_all_personal,
            });
        }
    }

    pub(super) fn sync_chat_groups_to_bridge(&self, bridge_idx: usize, old: &ChatGroups) {
        let new = &self.chat_groups;

        for old_group in &old.groups {
            if let Some(new_group) = new.groups.iter().find(|g| g.name == old_group.name) {
                if new_group.rooms != old_group.rooms
                    || new_group.include_all_personal != old_group.include_all_personal
                {
                    self.send_update_chat_group(
                        bridge_idx,
                        &new_group.name,
                        &new_group.rooms,
                        new_group.include_all_personal,
                    );
                }
            } else {
                self.send_remove_chat_group(bridge_idx, &old_group.name);
            }
        }

        for new_group in &new.groups {
            if !old.groups.iter().any(|g| g.name == new_group.name) {
                self.send_add_chat_group(
                    bridge_idx,
                    &new_group.name,
                    &new_group.rooms,
                    new_group.include_all_personal,
                );
            }
        }
    }

    pub fn set_room_pinned(&mut self, bridge_idx: usize, room_id: RoomId, pin: bool) {
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };

        let previous_index = state
            .rooms
            .iter()
            .find(|room| room.room_id == room_id)
            .map(|room| room.index)
            .unwrap_or_default();

        if let Some(room) = state.rooms.iter_mut().find(|room| room.room_id == room_id) {
            room.index = if pin { 1 } else { 0 };
        }

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::PinRoom { room_id, pin })
        {
            tracing::warn!("send pinRoom command failed: {}", e);
            if let Some(room) = self.bridge_states[bridge_idx]
                .rooms
                .iter_mut()
                .find(|room| room.room_id == room_id)
            {
                room.index = previous_index;
            }
            self.bridge_states[bridge_idx].last_error =
                Some(format!("置顶命令发送失败: {}", room_id));
        }
    }

    pub fn remove_chat(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        state.rooms.retain(|room| room.room_id != room_id);
        if state.selected_room_id == Some(room_id) {
            state.selected_room_id = None;
        }
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::RemoveChat(room_id))
        {
            tracing::warn!("send removeChat command failed: {}", e);
        }
    }

    pub fn ignore_chat(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::IgnoreChat { room_id, room_name })
        {
            tracing::warn!("send ignoreChat command failed: {}", e);
        }
    }

    pub fn apply_online_status(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let status = match self.online_mode {
            OnlineMode::Online => 11,
            OnlineMode::Left => 31,
            OnlineMode::Hidden => 41,
            OnlineMode::Busy => 50,
            OnlineMode::PingMe => 60,
            OnlineMode::DoNotDisturb => 70,
        };

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SetOnlineStatus(status))
        {
            tracing::warn!("send setOnlineStatus command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("在线状态命令发送失败".to_string());
            }
        } else if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.last_notice = Some(format!("已请求切换在线状态为 {}", self.online_mode));
        }
    }

    pub fn send_group_sign(&mut self, room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendGroupSign { room_id })
        {
            tracing::warn!("send sendGroupSign command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("群签到命令发送失败".to_string());
            }
        }
    }

    pub fn send_group_poke(&mut self, room_id: RoomId, target_id: i64) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendGroupPoke { room_id, target_id })
        {
            tracing::warn!("send sendGroupPoke command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("戳一戳命令发送失败".to_string());
            }
        }
    }

    pub fn ensure_selected_chat_group_valid(&mut self) {
        if let SelectedChatGroup::Custom(idx) = &self.selected_chat_group
            && *idx >= self.chat_groups.groups.len()
        {
            self.selected_chat_group = SelectedChatGroup::All;
        }
    }
}
