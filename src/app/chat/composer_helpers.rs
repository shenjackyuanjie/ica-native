#[cfg(test)]
#[path = "tests/composer_helpers.rs"]
mod tests;

use super::format_message_content;
use crate::app::ConversationState;
use crate::config::appearance::MessageSendKey;
use crate::ica::types::message::{Mention, Message, ReplyMessage};

const REPLY_PREVIEW_CHAR_LIMIT: usize = 160;

pub fn reply_preview_text(reply: &ReplyMessage) -> String {
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

#[derive(Clone, Copy)]
pub enum ReplyDirection {
    Previous,
    Next,
}

pub fn move_reply_selection(conversation: &mut ConversationState, direction: ReplyDirection) {
    let messages: Vec<_> = conversation
        .messages
        .iter()
        .filter(|message| !message.system && !message.flash && !message.msg_id.is_empty())
        .collect();
    let selected = conversation
        .reply_to
        .as_ref()
        .map(|reply| reply.msg_id.as_str());
    let current = selected.and_then(|id| messages.iter().position(|message| message.msg_id == id));
    let next = match (direction, selected, current) {
        (ReplyDirection::Previous, None, _) => messages.last().copied(),
        (ReplyDirection::Previous, _, Some(index)) => {
            messages.get(index.saturating_sub(1)).copied()
        }
        (ReplyDirection::Next, _, Some(index)) => messages.get(index + 1).copied(),
        // 已选消息不在本页时保持选择；Ctrl+↑仍可请求定位并拉取历史。
        (ReplyDirection::Previous, Some(_), None) => {
            conversation.scroll_to_message_id = selected.map(str::to_owned);
            conversation.highlight_message_id = selected.map(str::to_owned);
            conversation.scroll_to_message_attempts = 0;
            return;
        }
        (ReplyDirection::Next, _, None) => return,
    };
    conversation.reply_to = next.map(Message::as_reply);
    conversation.highlight_message_id = conversation
        .reply_to
        .as_ref()
        .map(|reply| reply.msg_id.clone());
    conversation.scroll_to_message_id = conversation.highlight_message_id.clone();
    conversation.scroll_to_message_attempts = 0;
}

/// 普通文件、闪照、合并转发等不能无损恢复为输入草稿，必须跳过而不是只恢复文字。
pub fn message_can_be_reedited(message: &Message, self_id: i64) -> bool {
    self_id > 0
        && message.sender_id == self_id
        && !message.msg_id.is_empty()
        && !message.system
        && !message.flash
        && !message.deleted
        && !message.hide
        && message.code.is_null()
        && !message.content.contains("[Forward:")
        && !message.content.contains("[NestedForward:")
        && (!message.content.is_empty() || !message.files.is_empty())
        && message.files.iter().all(|file| {
            super::is_image_file_type(&file.file_type)
                && (file.url.starts_with("https://") || file.url.starts_with("http://"))
        })
}

pub fn restore_edit_mentions(content: &str) -> (String, Vec<Mention>) {
    let mut remaining = content;
    let mut text = String::with_capacity(content.len());
    let mut mentions = Vec::new();
    while let Some((start, open, close)) = [
        ("<IcaAt qq=", "</IcaAt>"),
        ("<IcalinguaAt qq=", "</IcalinguaAt>"),
    ]
    .into_iter()
    .filter_map(|(open, close)| remaining.find(open).map(|start| (start, open, close)))
    .min_by_key(|(start, ..)| *start)
    {
        text.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(head_end) = remaining.find('>') else {
            break;
        };
        let Some(tail) = remaining[head_end + 1..].find(close) else {
            break;
        };
        let end = head_end + 1 + tail + close.len();
        let markup = &remaining[..end];
        let user_id = remaining[open.len()..head_end]
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0);
        if let Some(user_id) = user_id {
            let visible = format_message_content(markup).into_owned();
            text.push_str(&visible);
            if !visible.is_empty() {
                mentions.push(Mention {
                    user_id,
                    text: visible,
                });
            }
        } else {
            text.push_str(markup);
        }
        remaining = &remaining[end..];
    }
    text.push_str(remaining);
    (text, mentions)
}

pub fn edit_last_own_message(conversation: &mut ConversationState, self_id: i64) -> bool {
    if !conversation.can_edit_last_message() {
        return false;
    }
    let Some(message) = conversation
        .messages
        .iter()
        .rev()
        .find(|message| message_can_be_reedited(message, self_id))
    else {
        return false;
    };
    let (draft, mentions) = restore_edit_mentions(&message.content);
    conversation.draft = draft;
    conversation.mentions = mentions;
    conversation.reply_to = message.reply.clone();
    conversation.pending_remote_images = message.files.clone();
    conversation.editing_message_id = Some(message.msg_id.clone());
    true
}
