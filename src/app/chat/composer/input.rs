//! 输入框焦点、IME 与键盘事件消费规则。

#[cfg(test)]
#[path = "tests/input.rs"]
mod tests;

use crate::config::appearance::MessageSendKey;

/// 输入框快捷键只能作用于当前视口的焦点，不能穿透 IME 或模态弹层。
pub fn composer_keyboard_active(
    ctx: &egui::Context,
    composer_id: egui::Id,
    ime_composing: bool,
    ime_event_this_frame: bool,
) -> bool {
    !ime_composing
        && !ime_event_this_frame
        && ctx.input(|input| input.focused && input.viewport().focused.unwrap_or(true))
        && !egui::Popup::is_any_open(ctx)
        && ctx.memory(|memory| memory.has_focus(composer_id) && memory.top_modal_layer().is_none())
}

pub fn consume_composer_send_key(
    ui: &egui::Ui,
    composer_id: egui::Id,
    ime_composing: bool,
    ime_event_this_frame: bool,
    send_key: MessageSendKey,
) -> bool {
    if !composer_keyboard_active(ui.ctx(), composer_id, ime_composing, ime_event_this_frame) {
        return false;
    }
    ui.input_mut(|input| {
        let mut send = false;
        input.events.retain_mut(|event| {
            let egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                modifiers,
                repeat,
                ..
            } = event
            else {
                return true;
            };
            if modifiers.alt || modifiers.mac_cmd {
                return true;
            }
            let should_send = match send_key {
                MessageSendKey::Enter => !modifiers.ctrl && !modifiers.shift,
                MessageSendKey::CtrlEnter => modifiers.ctrl,
                MessageSendKey::ShiftEnter => !modifiers.ctrl && modifiers.shift,
            };
            if should_send {
                // 必须先消费，再交给多行编辑器，避免正文中间被插入多余换行。
                send |= !*repeat;
                false
            } else {
                // egui 默认不把 Ctrl+Enter 当换行，统一交由 TextEdit 插入，保留撤销/选区。
                *modifiers = egui::Modifiers::NONE;
                true
            }
        });
        send
    })
}

pub fn consume_composer_shortcut(ui: &egui::Ui, key: egui::Key, ctrl: bool) -> bool {
    ui.input_mut(|input| {
        let mut consumed = false;
        input.events.retain(|event| {
            if let egui::Event::Key {
                key: event_key,
                pressed: true,
                modifiers,
                ..
            } = event
                && *event_key == key
                && modifiers.ctrl == ctrl
                && !modifiers.shift
                && !modifiers.alt
                && !modifiers.mac_cmd
            {
                consumed = true;
                false
            } else {
                true
            }
        });
        consumed
    })
}
