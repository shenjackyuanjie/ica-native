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

/// 把消息正文里的 @ 标记还原成“可见文本 + Mention 列表”。
///
/// 兼容 `<IcaAt qq=..>` 与 `<IcalinguaAt qq=..>` 两种历史标记写法：能解析出
/// 正数 user_id 时替换为 @ 的可见文本并收集一条 Mention，解析失败则原样保留
/// 标记文本；剩余内容整体拼到末尾。
fn restore_edit_mentions(content: &str) -> (String, Vec<Mention>) {
    let mut remaining = content;
    let mut text = String::with_capacity(content.len());
    let mut mentions = Vec::new();
    // 两种历史 @ 标记都可能出现，每轮取剩余内容中位置最靠前的一个处理。
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

/// 把“上一条可重发的自有消息”恢复进输入区，返回是否成功进入编辑状态。
///
/// 依次恢复正文草稿、@ 提及、回复关系与待发送的远端图片，并记录
/// `editing_message_id`；提交成功后再由调用方请求撤回原消息。
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
