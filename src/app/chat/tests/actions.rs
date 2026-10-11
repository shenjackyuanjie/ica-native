use super::*;
use crate::app::test_support::test_app;

#[test]
fn avatar_mention_replaces_unicode_selection_and_keeps_one_protocol_mention() {
    let (mut app, _rx) = test_app();
    let ctx = egui::Context::default();
    let id = egui::Id::new(("message_composer", 0_usize, -123_i64));
    app.bridge_states[0].conversation_mut(-123).draft = "甲待替换乙".into();
    let mut edit = egui::widgets::text_edit::TextEditState::default();
    edit.cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(1),
            egui::text::CCursor::new(4),
        )));
    edit.store(&ctx, id);
    app.mention_avatar_sender(&ctx, 0, -123, 456, "测试成员".into());
    assert_eq!(
        app.bridge_states[0].conversation(-123).unwrap().draft,
        "甲@测试成员 乙"
    );
    app.mention_avatar_sender(&ctx, 0, -123, 456, "测试成员".into());
    let conversation = app.bridge_states[0].conversation(-123).unwrap();
    assert_eq!(conversation.draft, "甲@测试成员 @测试成员 乙");
    assert_eq!(
        conversation.mentions,
        vec![crate::ica::types::message::Mention {
            user_id: 456,
            text: "@测试成员".into(),
        }]
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
}

#[test]
fn avatar_mention_sanitizes_controls_and_rejects_private_or_virtual_targets() {
    let (mut app, _rx) = test_app();
    let ctx = egui::Context::default();
    app.mention_avatar_sender(&ctx, 0, -123, 456, "\n\u{202E}".into());
    app.mention_avatar_sender(&ctx, 0, 123, 456, "不可提及".into());
    app.mention_avatar_sender(&ctx, 0, -123, -1, "虚拟用户".into());
    let conversation = app.bridge_states[0].conversation(-123).unwrap();
    assert_eq!(conversation.draft, "@456 ");
    assert_eq!(conversation.mentions.len(), 1);
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-123));
    assert!(app.bridge_states[0].conversation(123).is_none());
}

#[test]
fn avatar_management_loads_the_target_group_without_sending_a_ban() {
    let (mut app, mut rx) = test_app();
    app.group_member_panel.filter = GroupMemberFilter::Muted;
    app.group_member_panel.error = Some("旧错误".into());
    app.open_avatar_member_management(0, -123, 456);
    assert!(app.group_member_panel.open);
    assert_eq!(app.group_member_panel.search_query, "456");
    assert_eq!(app.group_member_panel.filter, GroupMemberFilter::All);
    assert!(app.group_member_panel.error.is_none());
    let mut requested_members = false;
    while let Ok(command) = rx.try_recv() {
        assert!(!matches!(command, IcaCommand::SetGroupBan { .. }));
        if let IcaCommand::FetchGroupMembers { room_id } = command {
            assert_eq!(room_id, -123);
            requested_members = true;
        }
    }
    assert!(requested_members);
}

#[test]
fn invalid_avatar_poke_is_not_queued() {
    let (mut app, mut rx) = test_app();
    app.send_group_poke(i64::MIN, 456);
    assert!(rx.try_recv().is_err());
    assert!(app.bridge_states[0].last_error.is_some());
}
