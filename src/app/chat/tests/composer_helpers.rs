use super::*;

#[test]
fn plain_enter_is_consumed_before_multiline_editor_can_insert_a_newline() {
    let ctx = egui::Context::default();
    let composer_id = egui::Id::new("composer_enter_regression");
    let mut draft = "aaaaaaabaaaaaa".to_string();

    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let response = ui.add(egui::TextEdit::multiline(&mut draft).id(composer_id));
        response.request_focus();
    });
    output.textures_delta.clear();

    let mut input = egui::RawInput::default();
    input.events.push(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let mut enter_pressed = false;
    let mut output = ctx.run_ui(input, |ui| {
        enter_pressed =
            consume_composer_send_key(ui, composer_id, false, false, MessageSendKey::Enter);
        ui.add(egui::TextEdit::multiline(&mut draft).id(composer_id));
    });
    output.textures_delta.clear();

    assert!(enter_pressed);
    assert_eq!(draft, "aaaaaaabaaaaaa");
}
fn message(id: &str, sender_id: i64, content: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "_id": id, "senderId": sender_id, "username": "测试成员", "content": content,
    }))
    .unwrap()
}

fn image(url: &str) -> crate::ica::types::files::MessageFile {
    crate::ica::types::files::MessageFile {
        file_type: "image/png".into(),
        url: url.into(),
        size: None,
        name: None,
        fid: None,
    }
}

#[test]
fn reply_navigation_skips_system_and_flash_and_clears_after_latest() {
    let first = message("first", 2, "第一条");
    let mut system = message("system", 2, "系统提示");
    system.system = true;
    let mut flash = message("flash", 2, "闪照");
    flash.flash = true;
    let latest = message("latest", 2, "最新消息");
    let mut conversation = ConversationState {
        messages: vec![first, system, flash, latest],
        ..Default::default()
    };
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert!(conversation.reply_to.is_none());
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "latest");
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "first");
    assert_eq!(conversation.scroll_to_message_id.as_deref(), Some("first"));
    assert_eq!(conversation.highlight_message_id.as_deref(), Some("first"));
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "first");
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert!(conversation.reply_to.is_none());
    assert!(conversation.highlight_message_id.is_none());
    assert!(conversation.scroll_to_message_id.is_none());
}

#[test]
fn reply_outside_loaded_history_is_located_without_switching_to_another_message() {
    let mut conversation = ConversationState {
        messages: vec![message("latest", 2, "最新消息")],
        reply_to: Some(message("older", 2, "旧消息").as_reply()),
        ..Default::default()
    };
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "older");
    assert!(conversation.scroll_to_message_id.is_none());
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.scroll_to_message_id.as_deref(), Some("older"));
}

#[test]
fn reedit_skips_unresendable_messages_and_restores_all_images_reply_and_mentions() {
    let mut own = message("own", 42, "你好 <IcaAt qq=7>@A&amp;B</IcaAt>");
    own.files = vec![
        image("https://example.invalid/1.png"),
        image("https://example.invalid/2.png"),
    ];
    own.reply = Some(message("reply", 7, "被引用内容").as_reply());
    let mut file = message("file", 42, "不能只恢复这段正文");
    file.files = vec![crate::ica::types::files::MessageFile {
        file_type: "application/zip".into(),
        ..image("https://example.invalid/a.zip")
    }];
    let mut flash = message("flash", 42, "闪照");
    flash.flash = true;
    let mut conversation = ConversationState {
        messages: vec![own, file, flash, message("other", 7, "别人的消息")],
        ..Default::default()
    };
    assert!(edit_last_own_message(&mut conversation, 42));
    assert_eq!(conversation.editing_message_id.as_deref(), Some("own"));
    assert_eq!(conversation.draft, "你好 @A&B");
    assert_eq!(conversation.mentions[0].user_id, 7);
    assert_eq!(conversation.mentions[0].text, "@A&B");
    assert_eq!(conversation.pending_remote_images.len(), 2);
    assert_eq!(
        conversation.pending_remote_images[1].url,
        "https://example.invalid/2.png"
    );
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "reply");
}

#[test]
fn up_never_overwrites_draft_attachments_or_reply() {
    let base = ConversationState {
        messages: vec![message("own", 42, "旧消息")],
        ..Default::default()
    };
    let mut cases = [base.clone(), base.clone(), base.clone(), base];
    cases[0].draft = " ".into();
    cases[1].pending_images.push(crate::app::PendingImage::new(
        "测试.png".into(),
        "image/png".into(),
        vec![1],
    ));
    cases[2].reply_to = Some(message("reply", 7, "引用").as_reply());
    cases[3]
        .pending_remote_images
        .push(image("https://example.invalid/image.png"));
    for conversation in &mut cases {
        assert!(!edit_last_own_message(conversation, 42));
        assert!(conversation.editing_message_id.is_none());
    }
    assert_eq!(cases[0].draft, " ");
    assert_eq!(cases[1].pending_images.len(), 1);
    assert!(cases[2].reply_to.is_some());
    assert_eq!(cases[3].pending_remote_images.len(), 1);
}

#[test]
fn reedit_preserves_legacy_and_malformed_at_markup() {
    let (text, mentions) = restore_edit_mentions(
        "<IcalinguaAt qq=7>%40A%20B</IcalinguaAt> <IcaAt qq=1>@全体成员</IcaAt> <IcaAt qq=bad>@无效</IcaAt> <IcaAt qq=2>未闭合",
    );
    assert_eq!(
        text,
        "@A B @全体成员 <IcaAt qq=bad>@无效</IcaAt> <IcaAt qq=2>未闭合"
    );
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].user_id, 7);
    assert_eq!(mentions[1].user_id, 1);
}

fn enter_result(
    mode: MessageSendKey,
    modifiers: egui::Modifiers,
    ime: bool,
    ime_event: bool,
    focused: bool,
    repeat: bool,
) -> (bool, String) {
    let ctx = egui::Context::default();
    let id = egui::Id::new("测试发送键");
    let mut draft = "甲乙".to_string();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.add(egui::TextEdit::multiline(&mut draft).id(id))
            .request_focus();
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(1),
            )));
        state.store(ui.ctx(), id);
    });
    output.textures_delta.clear();
    if repeat {
        // egui 根据 keys_down 重算 repeat；必须模拟已经按住 Enter 的上一帧。
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ui| {
                assert!(consume_composer_send_key(ui, id, false, false, mode));
                ui.add(egui::TextEdit::multiline(&mut draft).id(id));
            },
        );
        output.textures_delta.clear();
    }
    let input = egui::RawInput {
        focused,
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }],
        ..Default::default()
    };
    let mut sent = false;
    let mut output = ctx.run_ui(input, |ui| {
        sent = consume_composer_send_key(ui, id, ime, ime_event, mode);
        ui.add(egui::TextEdit::multiline(&mut draft).id(id));
    });
    output.textures_delta.clear();
    (sent, draft)
}

#[test]
fn configured_enter_sends_only_selected_chord_and_other_chords_insert_one_newline() {
    for mode in [
        MessageSendKey::Enter,
        MessageSendKey::CtrlEnter,
        MessageSendKey::ShiftEnter,
    ] {
        for (modifiers, ctrl, shift) in [
            (egui::Modifiers::NONE, false, false),
            (egui::Modifiers::CTRL, true, false),
            (egui::Modifiers::SHIFT, false, true),
            (egui::Modifiers::CTRL | egui::Modifiers::SHIFT, true, true),
        ] {
            let expected = match mode {
                MessageSendKey::Enter => !ctrl && !shift,
                MessageSendKey::CtrlEnter => ctrl,
                MessageSendKey::ShiftEnter => !ctrl && shift,
            };
            let (sent, draft) = enter_result(mode, modifiers, false, false, true, false);
            assert_eq!(sent, expected, "{mode:?}, {modifiers:?}");
            assert_eq!(
                draft,
                if expected { "甲乙" } else { "甲\n乙" },
                "{mode:?}, {modifiers:?}"
            );
        }
    }
}

#[test]
fn enter_does_not_send_during_ime_commit_unfocused_viewport_or_key_repeat() {
    for (ime, event, focused, repeat) in [
        (true, false, true, false),
        (false, true, true, false),
        (false, false, false, false),
        (false, false, true, true),
    ] {
        assert!(
            !enter_result(
                MessageSendKey::Enter,
                egui::Modifiers::NONE,
                ime,
                event,
                focused,
                repeat
            )
            .0,
            "ime={ime}, event={event}, focused={focused}, repeat={repeat}"
        );
    }
    assert!(
        !enter_result(
            MessageSendKey::Enter,
            egui::Modifiers::ALT,
            false,
            false,
            true,
            false
        )
        .0
    );
}
