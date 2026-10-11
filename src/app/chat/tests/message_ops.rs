use super::*;
use crate::app::{BridgeState, test_support::app_with_bridges};
use crate::config::{ChatGroups, IcaCfg};
use tokio::sync::mpsc;

fn test_app() -> (IcaApp, mpsc::UnboundedReceiver<IcaCommand>) {
    let bridge = BridgeState::new("测试".into(), ChatGroups::default());
    let (mut app, mut receivers) = app_with_bridges(IcaCfg::default(), vec![bridge]);
    app.bridge_states[0].selected_room_id = Some(-1);
    app.bridge_states[0].conversation_mut(-2).draft = "另一会话草稿".into();
    (app, receivers.pop().unwrap())
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
    app.bridge_states[0].conversation_mut(-1).pending_file = Some(crate::app::PendingFile::new(
        "新增文件.txt".into(),
        "text/plain".into(),
        vec![3],
    ));
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
