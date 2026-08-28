use crate::ica::types::message::ReplyMessage;

use super::format_message_content;

const REPLY_PREVIEW_CHAR_LIMIT: usize = 160;

pub(super) fn reply_preview_text(reply: &ReplyMessage) -> String {
    if reply.content.contains("[Forward: ") || reply.content.contains("[NestedForward: ") {
        return "[合并转发]".to_string();
    }

    let formatted = format_message_content(&reply.content);
    let normalized = formatted.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return if reply.file.is_some() || !reply.files.is_empty() {
            "[图片或附件]".to_string()
        } else {
            "[空消息]".to_string()
        };
    }

    let mut chars = normalized.chars();
    let mut preview = chars
        .by_ref()
        .take(REPLY_PREVIEW_CHAR_LIMIT)
        .collect::<String>();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

pub(super) fn consume_composer_send_key(
    ui: &egui::Ui,
    composer_id: egui::Id,
    ime_composing: bool,
    ime_event_this_frame: bool,
) -> bool {
    !ime_composing
        && !ime_event_this_frame
        && ui.memory(|memory| memory.has_focus(composer_id))
        && ui.input_mut(|input| {
            !input.modifiers.shift
                && !input.modifiers.ctrl
                && input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        })
}

#[cfg(test)]
mod tests {
    use super::consume_composer_send_key;

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
            enter_pressed = consume_composer_send_key(ui, composer_id, false, false);
            ui.add(egui::TextEdit::multiline(&mut draft).id(composer_id));
        });
        output.textures_delta.clear();

        assert!(enter_pressed);
        assert_eq!(draft, "aaaaaaabaaaaaa");
    }
}
