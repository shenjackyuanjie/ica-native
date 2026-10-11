//! 回复预览与已加载消息间的回复导航。

#[cfg(test)]
#[path = "tests/reply.rs"]
mod tests;

use crate::app::ConversationState;
use crate::app::chat::format_message_content;
use crate::ica::types::message::{Message, ReplyMessage};

/// 回复预览文本的最大字符数，超出部分截断并加省略号。
const REPLY_PREVIEW_CHAR_LIMIT: usize = 160;

/// 生成回复预览的一行文本。
///
/// 合并转发、空消息与图片/附件有固定占位符；其余正文按空白折叠为单行后，
/// 截断到 `REPLY_PREVIEW_CHAR_LIMIT` 个字符，超出时末尾补省略号。
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

/// 回复导航方向：在已加载消息里向上/向下移动回复选择。
#[derive(Clone, Copy)]
pub enum ReplyDirection {
    /// 向更早的消息移动。
    Previous,
    /// 向更新的消息移动。
    Next,
}

/// 在当前会话已加载的消息间移动回复选择，并同步高亮与滚动目标。
///
/// 只统计非系统、非闪照且有 msg_id 的消息；尚未选择时 Previous 落到最后一条。
/// 已选消息不在本页时不清除选择，改为请求定位滚动并重置尝试计数，由历史
/// 拉取到位后继续导航。
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
