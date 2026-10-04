use crate::config::ReEditDraftConflictMode;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    files::MessageFile,
    message::{DeleteMessage, ImageAttachment, Mention, ReplyMessage, SendMessage},
};

use crate::app::{IcaApp, PendingImage};

impl IcaApp {
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

    pub fn remove_pending_image_at(&mut self, bridge_idx: usize, room_id: RoomId, index: usize) {
        let images = &mut self.bridge_states[bridge_idx]
            .conversation_mut(room_id)
            .pending_images;
        if index < images.len() {
            images.remove(index);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        AppState, BridgeSession, BridgeState, runtime::AppRuntime, stickers::StickerStore,
    };
    use crate::config::{ChatGroups, ConfigStore, IcaCfg};
    use crate::ica::BridgeHandle;
    use tokio::sync::{mpsc, oneshot};

    fn test_app() -> (IcaApp, mpsc::UnboundedReceiver<IcaCommand>) {
        let config: IcaCfg = toml::from_str("bridges = []").unwrap();
        let store = ConfigStore::from_config(
            config.clone(),
            std::env::temp_dir().join("ica-composer-test.toml"),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let (stop, _) = oneshot::channel();
        let sessions = vec![BridgeSession::new(
            BridgeHandle::new("测试".into(), tx),
            BridgeState::new("测试".into(), ChatGroups::default()),
            stop,
        )];
        let state = AppState::new(
            &config,
            &store,
            sessions,
            StickerStore::unavailable(
                std::env::temp_dir().join("ica-composer-test-stickers"),
                "测试",
            ),
        );
        let mut app = IcaApp {
            runtime: AppRuntime::new(&egui::Context::default(), &config),
            config: store,
            state,
            chat_windows: Vec::new(),
        };
        app.bridge_states[0].selected_room_id = Some(-1);
        app.bridge_states[0].conversation_mut(-2).draft = "另一会话草稿".into();
        (app, rx)
    }

    fn prepare_edit(app: &mut IcaApp) {
        app.bridge_states[0].socket_state = crate::app::SocketState::Connected;
        app.bridge_states[0].auth_state = crate::app::AuthState::Succeeded;
        let conversation = app.bridge_states[0].conversation_mut(-1);
        conversation.draft = "  编辑后的正文  ".into();
        conversation.editing_message_id = Some("原消息".into());
        conversation.pending_remote_images = ["1", "2"]
            .into_iter()
            .map(|id| MessageFile {
                file_type: "image/png".into(),
                url: format!("https://example.invalid/{id}.png"),
                size: None,
                name: None,
                fid: None,
            })
            .collect();
        conversation.pending_images.push(PendingImage::new(
            "新增.png".into(),
            "image/png".into(),
            vec![1, 2],
        ));
    }

    #[test]
    fn edit_send_enqueues_one_transaction_with_all_old_and_new_images() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.send_current_message();
        let IcaCommand::EditAndResendMessage {
            message,
            images,
            message_id,
        } = rx.try_recv().unwrap()
        else {
            panic!("编辑重发必须交给同一条后台命令，不能在 GUI 提前撤回");
        };
        assert_eq!(message_id, "原消息");
        assert_eq!(message.content, "编辑后的正文");
        assert_eq!(message.room_id, -1);
        assert_eq!(message.media.len(), 2);
        assert_eq!(
            message.media[1].url.as_deref(),
            Some("https://example.invalid/2.png")
        );
        assert_eq!(message.media[1].order, Some(1));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].1.as_ref(), &[1, 2]);
        assert!(rx.try_recv().is_err());
        assert!(
            app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .editing_send_pending
        );
        assert_eq!(
            app.bridge_states[0].conversation(-1).unwrap().draft,
            "  编辑后的正文  "
        );
        app.send_current_message();
        assert!(rx.try_recv().is_err(), "接收回执之前不能重复提交");
        assert_eq!(
            app.bridge_states[0].conversation(-2).unwrap().draft,
            "另一会话草稿"
        );
    }

    #[test]
    fn offline_or_unauthenticated_edit_does_not_enter_submission_queue() {
        for authenticated in [false, true] {
            let (mut app, mut rx) = test_app();
            prepare_edit(&mut app);
            if authenticated {
                app.bridge_states[0].socket_state = crate::app::SocketState::Disconnected;
            } else {
                app.bridge_states[0].auth_state = crate::app::AuthState::Pending;
            }
            app.send_current_message();
            assert!(rx.try_recv().is_err());
            let conversation = app.bridge_states[0].conversation(-1).unwrap();
            assert!(!conversation.editing_send_pending);
            assert_eq!(conversation.draft, "  编辑后的正文  ");
            assert_eq!(conversation.pending_remote_images.len(), 2);
            assert_eq!(conversation.pending_images[0].data.as_ref(), &[1, 2]);
        }
    }

    #[test]
    fn exhausted_reconnect_releases_pending_edit_without_losing_draft() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.send_current_message();
        rx.try_recv().unwrap();
        IcaApp::apply_bridge_event(
            &mut app.bridge_states[0],
            &crate::ica::BridgeEventKind::SocketDisconnected(serde_json::Value::Null),
        );
        assert!(
            app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .editing_send_pending,
            "重连尚在继续时不能允许重复提交已经排队的消息"
        );
        IcaApp::apply_bridge_event(
            &mut app.bridge_states[0],
            &crate::ica::BridgeEventKind::SocketReconnectExhausted(serde_json::Value::Null),
        );
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert!(!conversation.editing_send_pending);
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert_eq!(conversation.pending_remote_images.len(), 2);
        assert_eq!(conversation.pending_images[0].data.as_ref(), &[1, 2]);
        assert!(!conversation.pending_send_scroll_to_bottom);
        assert!(
            app.bridge_states[0]
                .last_notice
                .as_deref()
                .unwrap()
                .contains("检查聊天记录")
        );
        IcaApp::apply_bridge_event(
            &mut app.bridge_states[0],
            &crate::ica::BridgeEventKind::EditSendResult(serde_json::json!({
                "roomId": -1, "messageId": "原消息", "accepted": true,
            })),
        );
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert_eq!(conversation.pending_remote_images.len(), 2);
        assert_eq!(conversation.pending_images[0].data.as_ref(), &[1, 2]);
    }

    #[test]
    fn disconnected_send_restores_exact_draft_edit_target_and_all_attachments() {
        let (mut app, rx) = test_app();
        drop(rx);
        prepare_edit(&mut app);
        app.send_current_message();
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert_eq!(conversation.editing_message_id.as_deref(), Some("原消息"));
        assert_eq!(conversation.pending_remote_images.len(), 2);
        assert_eq!(conversation.pending_images[0].data.as_ref(), &[1, 2]);
        assert_eq!(
            app.bridge_states[0].conversation(-2).unwrap().draft,
            "另一会话草稿"
        );
    }

    #[test]
    fn edit_network_failure_keeps_draft_and_images_and_allows_manual_retry() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.send_current_message();
        rx.try_recv().unwrap();
        IcaApp::apply_bridge_event(
            &mut app.bridge_states[0],
            &crate::ica::BridgeEventKind::EditSendResult(
                serde_json::json!({ "roomId": -1, "messageId": "原消息", "accepted": false, "error": "超时" }),
            ),
        );
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert!(!conversation.editing_send_pending);
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert_eq!(conversation.pending_remote_images.len(), 2);
        assert_eq!(conversation.pending_images[0].data.as_ref(), &[1, 2]);
        assert!(rx.try_recv().is_err(), "失败回执不能自动重发");
        app.send_current_message();
        assert!(matches!(
            rx.try_recv().unwrap(),
            IcaCommand::EditAndResendMessage { .. }
        ));
    }

    #[test]
    fn accepted_edit_result_clears_only_matching_room_after_navigation() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.send_current_message();
        rx.try_recv().unwrap();
        app.bridge_states[0].selected_room_id = Some(-2);
        for target in ["过时消息", "原消息"] {
            IcaApp::apply_bridge_event(
                &mut app.bridge_states[0],
                &crate::ica::BridgeEventKind::EditSendResult(
                    serde_json::json!({ "roomId": -1, "messageId": target, "accepted": true }),
                ),
            );
            assert_eq!(
                app.bridge_states[0]
                    .conversation(-1)
                    .unwrap()
                    .editing_send_pending,
                target != "原消息"
            );
        }
        assert!(
            !app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .has_composer_content()
        );
        assert!(
            app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .editing_message_id
                .is_none()
        );
        assert_eq!(
            app.bridge_states[0].conversation(-2).unwrap().draft,
            "另一会话草稿"
        );
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-2));
    }

    #[test]
    fn pending_edit_is_not_cancelled_or_changed_by_escape_reply_copy_or_mention() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.send_current_message();
        rx.try_recv().unwrap();
        let ctx = egui::Context::default();
        escape_frame(&mut app, &ctx);
        app.restore_deleted_message_to_draft(-1, "覆盖正文".into());
        app.mention_avatar_sender(&ctx, 0, -1, 42, "成员".into());
        app.queue_reply(
            -1,
            ReplyMessage {
                msg_id: "其他消息".into(),
                content: "其他正文".into(),
                sender_name: "成员".into(),
                file: None,
                files: vec![],
            },
        );
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert!(conversation.reply_to.is_none());
        assert!(conversation.editing_send_pending);
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-1));
    }

    #[test]
    fn empty_send_preserves_reply_and_edit_state() {
        let (mut app, mut rx) = test_app();
        let conversation = app.bridge_states[0].conversation_mut(-1);
        conversation.editing_message_id = Some("旧消息".into());
        conversation.reply_to = Some(ReplyMessage {
            msg_id: "引用".into(),
            content: "正文".into(),
            sender_name: "测试成员".into(),
            file: None,
            files: vec![],
        });
        app.send_current_message();
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert!(conversation.reply_to.is_some());
        assert_eq!(conversation.editing_message_id.as_deref(), Some("旧消息"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn editing_with_new_file_is_rejected_without_clearing_draft_or_images() {
        let (mut app, mut rx) = test_app();
        prepare_edit(&mut app);
        app.bridge_states[0].conversation_mut(-1).pending_file = Some(
            crate::app::PendingFile::new("新增文件.txt".into(), "text/plain".into(), vec![3]),
        );
        app.send_current_message();
        let conversation = app.bridge_states[0].conversation(-1).unwrap();
        assert_eq!(conversation.draft, "  编辑后的正文  ");
        assert!(conversation.pending_file.is_some());
        assert_eq!(conversation.pending_remote_images.len(), 2);
        assert!(rx.try_recv().is_err());
        assert!(app.bridge_states[0].last_error.is_some());
    }

    fn escape_frame(app: &mut IcaApp, ctx: &egui::Context) {
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            app.handle_chat_escape(ui.ctx());
        });
        output.textures_delta.clear();
    }

    #[test]
    fn escape_cancels_popups_draft_attachments_forward_selection_then_current_room() {
        let (mut app, _rx) = test_app();
        prepare_edit(&mut app);
        app.show_face_picker = true;
        app.bridge_states[0].forward_room_id = Some(-1);
        app.bridge_states[0].forward_selected_message_ids = vec!["转发消息".into()];
        let ctx = egui::Context::default();
        escape_frame(&mut app, &ctx);
        assert!(!app.show_face_picker);
        assert!(
            !app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .draft
                .is_empty()
        );
        escape_frame(&mut app, &ctx);
        assert!(
            app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .draft
                .is_empty()
        );
        assert!(
            app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .has_composer_content()
        );
        escape_frame(&mut app, &ctx);
        assert!(
            !app.bridge_states[0]
                .conversation(-1)
                .unwrap()
                .has_composer_content()
        );
        assert!(app.bridge_states[0].is_forward_selection_active(-1));
        escape_frame(&mut app, &ctx);
        assert!(!app.bridge_states[0].is_forward_selection_active(-1));
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-1));
        escape_frame(&mut app, &ctx);
        assert_eq!(app.bridge_states[0].selected_room_id, None);
        assert_eq!(
            app.bridge_states[0].conversation(-2).unwrap().draft,
            "另一会话草稿"
        );
    }

    #[test]
    fn main_window_escape_does_not_mutate_a_detached_room_or_ime_draft() {
        let (mut app, _rx) = test_app();
        prepare_edit(&mut app);
        app.bridge_states[0].detached_room_ids.insert(-1);
        let ctx = egui::Context::default();
        escape_frame(&mut app, &ctx);
        assert_eq!(
            app.bridge_states[0].conversation(-1).unwrap().draft,
            "  编辑后的正文  "
        );
        app.bridge_states[0].detached_room_ids.clear();
        app.ime_composing = true;
        escape_frame(&mut app, &ctx);
        assert_eq!(
            app.bridge_states[0].conversation(-1).unwrap().draft,
            "  编辑后的正文  "
        );
    }

    #[test]
    fn escape_does_not_cancel_chat_under_another_window_or_modal() {
        for modal in [false, true] {
            let (mut app, _rx) = test_app();
            prepare_edit(&mut app);
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                if modal {
                    egui::Modal::new(egui::Id::new("测试确认框")).show(ui.ctx(), |ui| {
                        ui.label("确认");
                    });
                } else {
                    egui::Window::new("测试窗口").show(ui.ctx(), |ui| {
                        ui.label("其他操作");
                    });
                }
            });
            output.textures_delta.clear();
            escape_frame(&mut app, &ctx);
            assert_eq!(
                app.bridge_states[0].conversation(-1).unwrap().draft,
                "  编辑后的正文  "
            );
            assert_eq!(app.bridge_states[0].selected_room_id, Some(-1));
        }
    }
}
