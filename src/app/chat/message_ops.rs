//! 消息操作：发送当前输入区内容、回复/重发/撤回/显隐、加群验证处理与附件挑选。
//!
//! 发送按内容形态拆成多条命令（文件、远程图片、本地单图/多图、纯文本），
//! 任一条发送失败即整体回滚草稿与附件；编辑重发是单命令原子提交，提交期间
//! 锁定输入区。远程图片（已存在于服务器的图片）以 URL 形式附加，不重新上传。

#[cfg(test)]
#[path = "tests/message_ops.rs"]
mod tests;

use crate::app::{IcaApp, PendingImage};
use crate::config::ReEditDraftConflictMode;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    files::MessageFile,
    message::{DeleteMessage, ImageAttachment, Mention, ReplyMessage, SendMessage},
};

impl IcaApp {
    /// 发送当前会话输入区的内容（编辑重发/新消息两条路径）。
    ///
    /// 新消息按"文件 → 远程图片 → 本地图片/纯文本"拆成多条命令，正文只
    /// 挂在第一条上；任一条发送失败时完整还原草稿、回复、提及与附件。
    /// 编辑重发要求 bridge 就绪且不带普通文件，单命令提交并锁定输入区。
    pub fn send_current_message(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let scroll_to_bottom_after_send = self.scroll_to_bottom_after_send;
        let Some(command_tx) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| session.command_sender())
        else {
            return;
        };
        let Some(state) = self
            .bridge_states
            .get_mut(bridge_idx)
            .map(|session| session.state_mut())
        else {
            return;
        };
        let Some(room_id) = state.selected_room_id else {
            return;
        };

        let bridge_ready = state.socket_state == crate::app::SocketState::Connected
            && state.auth_state == crate::app::AuthState::Succeeded;
        let conversation = state.conversation_mut(room_id);
        if conversation.editing_send_pending || !conversation.has_composer_content() {
            return;
        }
        if conversation.editing_message_id.is_some() && !bridge_ready {
            state.last_error = Some("Bridge 未连接或认证未完成，编辑草稿已保留".into());
            return;
        }
        // 编辑重发不能拆成“先发文件再发正文”：失败时无法安全决定是否撤回旧消息。
        if conversation.editing_message_id.is_some() && conversation.pending_file.is_some() {
            state.last_error =
                Some("编辑重发不能附加普通文件；请先取消编辑，再单独发送文件".into());
            return;
        }
        if let Some(message_id) = conversation.editing_message_id.clone() {
            let content = conversation.draft.trim().to_string();
            let mentions = conversation
                .mentions
                .iter()
                .filter(|mention| content.contains(&mention.text))
                .cloned()
                .collect::<Vec<_>>();
            let command = IcaCommand::EditAndResendMessage {
                message: message_with_remote_images(
                    room_id,
                    content,
                    conversation.reply_to.clone(),
                    &mentions,
                    &conversation.pending_remote_images,
                ),
                images: conversation
                    .pending_images
                    .iter()
                    .map(|image| (image.mime_type.clone(), image.data.clone()))
                    .collect(),
                message_id,
            };
            match command_tx.send(command) {
                Ok(()) => {
                    conversation.editing_send_pending = true;
                    if scroll_to_bottom_after_send {
                        conversation.pending_send_scroll_to_bottom = true;
                    }
                }
                Err(_) => state.last_error = Some("编辑重发提交失败，草稿已保留".into()),
            }
            return;
        }
        let original_draft = std::mem::take(&mut conversation.draft);
        let content = original_draft.trim().to_string();
        let pending_remote_images = std::mem::take(&mut conversation.pending_remote_images);
        let reply_to = conversation.reply_to.take();
        let mut mentions = std::mem::take(&mut conversation.mentions);
        mentions.retain(|mention| content.contains(&mention.text));
        let pending_images = std::mem::take(&mut conversation.pending_images);
        let pending_file = conversation.pending_file.take();
        let mut outgoing_commands = Vec::new();
        let mut content_attached = false;
        if let Some(file) = &pending_file {
            outgoing_commands.push(IcaCommand::SendFileMessage {
                room_id,
                content: content.clone(),
                reply_to: reply_to.clone(),
                mentions: mentions.clone(),
                file_name: file.name.clone(),
                file_type: file.file_type.clone(),
                file_data: file.data.clone(),
            });
            content_attached = true;
        }

        if !pending_remote_images.is_empty() {
            outgoing_commands.push(IcaCommand::SendMessage(message_with_remote_images(
                room_id,
                if content_attached {
                    String::new()
                } else {
                    content.clone()
                },
                if content_attached {
                    None
                } else {
                    reply_to.clone()
                },
                &mentions,
                &pending_remote_images,
            )));
            content_attached = true;
        }

        if pending_images.is_empty() {
            if !content_attached {
                let mut message = SendMessage::new(content.clone(), room_id, reply_to.clone());
                message.set_mentions(&mentions);
                outgoing_commands.push(IcaCommand::SendMessage(message));
            }
        } else {
            let image_content = if content_attached {
                String::new()
            } else {
                content.clone()
            };
            let image_reply = if content_attached {
                None
            } else {
                reply_to.clone()
            };
            if pending_images.len() == 1 {
                let image = &pending_images[0];
                outgoing_commands.push(IcaCommand::SendImageMessage {
                    room_id,
                    content: image_content,
                    reply_to: image_reply,
                    mentions: mentions.clone(),
                    image_type: image.mime_type.clone(),
                    image_data: image.data.clone(),
                });
            } else {
                outgoing_commands.push(IcaCommand::SendMultiImageMessage {
                    room_id,
                    content: image_content,
                    reply_to: image_reply,
                    mentions: mentions.clone(),
                    images: pending_images
                        .iter()
                        .map(|image| (image.mime_type.clone(), image.data.clone()))
                        .collect(),
                });
            }
        }

        let mut send_failed = None;
        for command in outgoing_commands {
            if let Err(e) = command_tx.send(command) {
                send_failed = Some(e);
                break;
            }
        }

        if let Some(e) = send_failed {
            tracing::warn!(error = %e, room_id, "发送 sendMessage 命令失败");
            conversation.draft = original_draft;
            conversation.pending_remote_images = pending_remote_images;
            conversation.reply_to = reply_to;
            conversation.mentions = mentions;
            conversation.pending_images = pending_images;
            conversation.pending_file = pending_file;
        } else {
            conversation.highlight_message_id = None;
            if scroll_to_bottom_after_send {
                conversation.pending_send_scroll_to_bottom = true;
            }
        }
    }

    /// 把一条消息设为当前回复对象，并高亮被回复的消息。
    ///
    /// 编辑重发提交期间忽略，避免覆盖待撤回的草稿状态。
    pub fn queue_reply(&mut self, room_id: RoomId, reply: ReplyMessage) {
        if let Some(state) = self.active_bridge_state_mut() {
            let conversation = state.conversation_mut(room_id);
            if conversation.editing_send_pending {
                return;
            }
            conversation.highlight_message_id = Some(reply.msg_id.clone());
            conversation.reply_to = Some(reply);
        }
    }

    /// 请求 Bridge 重发（刷新）一条消息，用于图片/文件加载失败后重试。
    pub fn send_renew_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::RenewMessage {
            room_id,
            message_id: message_id.clone(),
        }) {
            tracing::warn!(error = %e, room_id, message_id, "发送 renewMessage 命令失败");
        }
    }

    /// 撤回一条消息；发送失败记 `last_error`。
    pub fn send_delete_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let message = DeleteMessage::new(room_id, message_id.clone());
        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::DeleteMessage(message)) {
            tracing::warn!(error = %e, room_id, message_id, "发送 deleteMessage 命令失败");
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("撤回消息命令发送失败: {}", message_id));
            }
        }
    }

    /// 显示或隐藏一条消息（对所有人可见或仅自己可见）。
    ///
    /// 命令发送成功后本地立即标记，失败则提示且不改本地状态。
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

        if let Err(e) = self.bridge_states[bridge_idx].send(command) {
            tracing::warn!(error = %e, room_id, message_id, "发送显示或隐藏消息命令失败");
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

    /// 把已撤回/删除的消息内容恢复到编辑区草稿。
    ///
    /// 按 `reedit_draft_conflict_mode` 处理草稿冲突：覆盖、追加或仅在草稿
    /// 为空时写入；编辑重发提交期间忽略。
    pub fn restore_deleted_message_to_draft(&mut self, room_id: RoomId, content: String) {
        let mode = self.reedit_draft_conflict_mode;
        if let Some(state) = self.active_bridge_state_mut() {
            if state
                .conversation(room_id)
                .is_some_and(|conversation| conversation.editing_send_pending)
            {
                return;
            }
            let draft = &mut state.conversation_mut(room_id).draft;
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

    /// 处理一条加群/好友验证消息（同意或拒绝）。
    ///
    /// 成功后从待处理列表移除；失败保留并提示。
    pub fn handle_join_request(&mut self, request_type: String, flag: String, accept: bool) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::HandleRequest {
            request_type: request_type.clone(),
            flag: flag.clone(),
            accept,
        }) {
            tracing::warn!(error = %e, request_type, flag = %flag, "发送 handleRequest 命令失败");
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("验证消息操作发送失败: {}", flag));
            }
            return;
        }

        if let Some(state) = self.active_bridge_state_mut() {
            state.join_requests.retain(|request| request.flag != flag);
        }
    }

    /// 向当前会话追加待发送图片；编辑重发提交期间忽略。
    pub fn append_pending_images(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        images: impl IntoIterator<Item = PendingImage>,
    ) {
        if self.bridge_states[bridge_idx]
            .conversation(room_id)
            .is_some_and(|conversation| conversation.editing_send_pending)
        {
            return;
        }
        let entry = &mut self.bridge_states[bridge_idx]
            .conversation_mut(room_id)
            .pending_images;
        entry.extend(images);
    }

    /// 按下标移除一张待发送图片；越界下标忽略。
    pub fn remove_pending_image_at(&mut self, bridge_idx: usize, room_id: RoomId, index: usize) {
        let images = &mut self.bridge_states[bridge_idx]
            .conversation_mut(room_id)
            .pending_images;
        if index < images.len() {
            images.remove(index);
        }
    }

    /// 打开文件对话框为当前会话挑选图片（可多选）。
    ///
    /// 单个文件读取失败不阻塞其他图片，错误汇总后一次性提示。
    pub fn pick_image_for_current_room(&mut self) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self.bridge_states[active_bridge_idx].selected_room_id else {
            return;
        };

        let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "image",
                &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"],
            )
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

    /// 打开文件对话框为当前会话挑选一个普通附件文件。
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
                    .conversation_mut(room_id)
                    .pending_file = Some(file);
            }
            Err(e) => {
                self.bridge_states[active_bridge_idx].last_error = Some(e.to_string());
            }
        }
    }
}

/// 保留所有原图片及其顺序，不把恢复后的图文消息降级成纯文本。
fn message_with_remote_images(
    room_id: RoomId,
    content: String,
    reply_to: Option<ReplyMessage>,
    mentions: &[Mention],
    images: &[MessageFile],
) -> SendMessage {
    let mut message = SendMessage::new(content, room_id, reply_to);
    message.set_mentions(mentions);
    message.media = images
        .iter()
        .enumerate()
        .map(|(order, file)| ImageAttachment {
            b64: None,
            url: Some(file.url.clone()),
            file_type: Some(file.file_type.clone()),
            fid: None,
            order: Some(order),
        })
        .collect();
    message
}
