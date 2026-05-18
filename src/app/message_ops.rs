use crate::cfg::ReEditDraftConflictMode;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    message::{DeleteMessage, ReplyMessage, SendMessage},
};

use super::{IcaApp, PendingImage};

impl IcaApp {
    pub fn send_current_message(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let scroll_to_bottom_after_send = self.scroll_to_bottom_after_send;
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let Some(room_id) = state.selected_room_id else {
            return;
        };

        let draft = state.draft_by_room.entry(room_id).or_default();
        let content = draft.trim().to_string();
        let reply_to = state.reply_to_by_room.remove(&room_id);
        let pending_images = state
            .pending_image_by_room
            .remove(&room_id)
            .unwrap_or_default();
        let pending_file = state.pending_file_by_room.remove(&room_id);
        if content.is_empty() && pending_images.is_empty() && pending_file.is_none() {
            if let Some(reply_to) = reply_to {
                state.reply_to_by_room.insert(room_id, reply_to);
            }
            return;
        }
        draft.clear();

        if let Some(file) = pending_file {
            let command = IcaCommand::SendFileMessage {
                room_id,
                content: content.clone(),
                reply_to: reply_to.clone(),
                file_name: file.name,
                file_type: file.file_type,
                file_data: file.data,
            };
            if let Err(e) = self.ica_clients[bridge_idx].command_tx.send(command) {
                tracing::warn!("send sendFileMessage command failed: {}", e);
                state.draft_by_room.insert(room_id, content);
                if let Some(reply_to) = reply_to {
                    state.reply_to_by_room.insert(room_id, reply_to);
                }
            } else if scroll_to_bottom_after_send {
                state.pending_send_scroll_to_bottom.insert(room_id);
            }
            return;
        }

        let mut outgoing_messages = Vec::new();
        if pending_images.is_empty() {
            outgoing_messages.push(SendMessage::new(content.clone(), room_id, reply_to.clone()));
        } else {
            for (idx, image) in pending_images.iter().enumerate() {
                let mut message = SendMessage::new(
                    if idx == 0 {
                        content.clone()
                    } else {
                        String::new()
                    },
                    room_id,
                    if idx == 0 { reply_to.clone() } else { None },
                );
                message.set_img(&image.data, &image.mime_type, false);
                outgoing_messages.push(message);
            }
        }

        let mut send_failed = None;
        for message in outgoing_messages {
            if let Err(e) = self.ica_clients[bridge_idx]
                .command_tx
                .send(IcaCommand::SendMessage(message))
            {
                send_failed = Some(e);
                break;
            }
        }

        if let Some(e) = send_failed {
            tracing::warn!("send sendMessage command failed: {}", e);
            state.draft_by_room.insert(room_id, content);
            if let Some(reply_to) = reply_to {
                state.reply_to_by_room.insert(room_id, reply_to);
            }
            if !pending_images.is_empty() {
                state.pending_image_by_room.insert(room_id, pending_images);
            }
        } else if scroll_to_bottom_after_send {
            state.pending_send_scroll_to_bottom.insert(room_id);
        }
    }

    pub fn queue_reply(&mut self, room_id: RoomId, reply: ReplyMessage) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.reply_to_by_room.insert(room_id, reply);
        }
    }

    pub fn send_renew_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::RenewMessage {
                room_id,
                message_id,
            })
        {
            tracing::warn!("send renewMessage command failed: {}", e);
        }
    }

    pub fn send_delete_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let message = DeleteMessage::new(room_id, message_id.clone());
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::DeleteMessage(message))
        {
            tracing::warn!("send deleteMessage command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("撤回消息命令发送失败: {}", message_id));
            }
        }
    }

    pub fn set_message_reveal(&mut self, room_id: RoomId, message_id: String, reveal: bool) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let command = if reveal {
            IcaCommand::RevealMessage {
                room_id,
                message_id: message_id.clone(),
            }
        } else {
            IcaCommand::HideMessage {
                room_id,
                message_id: message_id.clone(),
            }
        };

        if let Err(e) = self.ica_clients[bridge_idx].command_tx.send(command) {
            tracing::warn!("send reveal/hide message command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("显示/隐藏消息命令发送失败: {}", message_id));
            }
            return;
        }

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            if reveal {
                state.mark_message_revealed(&message_id);
            } else {
                state.mark_message_hidden(&message_id);
            }
        }
    }

    pub fn restore_deleted_message_to_draft(&mut self, room_id: RoomId, content: String) {
        let mode = self.reedit_draft_conflict_mode;
        if let Some(state) = self.active_bridge_state_mut() {
            let draft = state.draft_by_room.entry(room_id).or_default();
            match mode {
                ReEditDraftConflictMode::Overwrite => {
                    *draft = content;
                }
                ReEditDraftConflictMode::Append => {
                    if draft.trim().is_empty() {
                        *draft = content;
                    } else if !content.trim().is_empty() {
                        if !draft.ends_with('\n') {
                            draft.push('\n');
                        }
                        draft.push_str(&content);
                    }
                }
                ReEditDraftConflictMode::SkipIfNonEmpty => {
                    if draft.trim().is_empty() {
                        *draft = content;
                    }
                }
            }
        }
    }

    pub fn handle_join_request(&mut self, request_type: String, flag: String, accept: bool) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::HandleRequest {
                request_type: request_type.clone(),
                flag: flag.clone(),
                accept,
            })
        {
            tracing::warn!("send handleRequest command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("验证消息操作发送失败: {}", flag));
            }
            return;
        }

        if let Some(state) = self.active_bridge_state_mut() {
            state.join_requests.retain(|request| request.flag != flag);
        }
    }

    pub(super) fn append_pending_images(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        images: impl IntoIterator<Item = PendingImage>,
    ) {
        let entry = self.bridge_states[bridge_idx]
            .pending_image_by_room
            .entry(room_id)
            .or_default();
        entry.extend(images);
    }

    pub(super) fn remove_pending_image_at(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        index: usize,
    ) {
        let mut should_remove_entry = false;
        if let Some(images) = self.bridge_states[bridge_idx]
            .pending_image_by_room
            .get_mut(&room_id)
        {
            if index < images.len() {
                images.remove(index);
            }
            should_remove_entry = images.is_empty();
        }
        if should_remove_entry {
            self.bridge_states[bridge_idx]
                .pending_image_by_room
                .remove(&room_id);
        }
    }

    pub fn pick_image_for_current_room(&mut self) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self.bridge_states[active_bridge_idx].selected_room_id else {
            return;
        };

        let Some(paths) = rfd::FileDialog::new()
            .add_filter("image", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .pick_files()
        else {
            return;
        };

        let mut images = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            match Self::load_pending_image(&path) {
                Ok(image) => images.push(image),
                Err(e) => errors.push(e.to_string()),
            }
        }

        if !images.is_empty() {
            self.append_pending_images(active_bridge_idx, room_id, images);
        }
        if !errors.is_empty() {
            self.bridge_states[active_bridge_idx].last_error = Some(errors.join("；"));
        }
    }

    pub fn pick_file_for_current_room(&mut self) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self.bridge_states[active_bridge_idx].selected_room_id else {
            return;
        };

        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };

        match Self::load_pending_file(&path) {
            Ok(file) => {
                self.bridge_states[active_bridge_idx]
                    .pending_file_by_room
                    .insert(room_id, file);
            }
            Err(e) => {
                self.bridge_states[active_bridge_idx].last_error = Some(e.to_string());
            }
        }
    }
}
