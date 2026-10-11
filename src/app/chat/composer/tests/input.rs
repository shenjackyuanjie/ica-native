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
