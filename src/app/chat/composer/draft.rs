//! 上一条自有消息的草稿恢复、附件筛选与提及还原。

#[cfg(test)]
#[path = "tests/draft.rs"]
mod tests;

use crate::app::ConversationState;
use crate::app::chat::{format_message_content, is_image_file_type};
use crate::ica::types::message::{Mention, Message};

/// 普通文件、闪照、合并转发等不能无损恢复为输入草稿，必须跳过而不是只恢复文字。
fn message_can_be_reedited(message: &Message, self_id: i64) -> bool {
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
            is_image_file_type(&file.file_type)
                && (file.url.starts_with("https://") || file.url.starts_with("http://"))
        })
}

fn restore_edit_mentions(content: &str) -> (String, Vec<Mention>) {
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
