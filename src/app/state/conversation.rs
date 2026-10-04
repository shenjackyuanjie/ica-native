use std::collections::HashMap;

use crate::ica::types::{
    files::MessageFile,
    message::{Mention, Message, ReplyMessage},
};

use super::{GroupMember, MessageLayoutCacheKey, MessageRowLayout, PendingFile, PendingImage};

/// 单个会话拥有的全部可变状态。
#[derive(Debug, Clone, Default)]
pub struct ConversationState {
    pub messages: Vec<Message>,
    pub group_members: Vec<GroupMember>,
    pub group_members_loaded: bool,
    pub loading_group_members: bool,
    pub scroll_to_bottom: bool,
    pub pending_message_scroll_to_bottom: bool,
    pub pending_send_scroll_to_bottom: bool,
    pub near_bottom: bool,
    pub new_message_count: usize,
    pub reply_to: Option<ReplyMessage>,
    /// 编辑重发仅属于当前 bridge 的当前会话，不随窗口焦点迁移。
    pub editing_message_id: Option<String>,
    /// 提交期间保留编辑草稿，失败后可原样重试；不允许重复提交。
    pub editing_send_pending: bool,
    /// 从历史消息恢复的图片引用，不能只恢复正文而丢弃原附件。
    pub pending_remote_images: Vec<MessageFile>,
    pub pending_images: Vec<PendingImage>,
    pub pending_file: Option<PendingFile>,
    pub draft: String,
    pub mentions: Vec<Mention>,
    pub requested_snapshot: bool,
    pub loading_older_messages: bool,
    pub no_more_history: bool,
    pub prepend_scroll_fix: bool,
    pub last_content_height: Option<f32>,
    pub message_scroll_offset: Option<f32>,
    pub message_row_heights: HashMap<String, f32>,
    pub message_row_layouts: Vec<MessageRowLayout>,
    pub message_layout_cache_key: Option<MessageLayoutCacheKey>,
    /// 键盘选择回复后的持续高亮，与一次性的滚动请求分开。
    pub highlight_message_id: Option<String>,
    pub scroll_to_message_id: Option<String>,
    pub scroll_to_message_attempts: u8,
}

impl ConversationState {
    pub fn has_composer_content(&self) -> bool {
        !self.draft.trim().is_empty()
            || !self.pending_images.is_empty()
            || !self.pending_remote_images.is_empty()
            || self.pending_file.is_some()
    }

    /// ↑ 不得覆盖尚未发送的附件、回复或编辑状态；空白字符也是用户草稿。
    pub fn can_edit_last_message(&self) -> bool {
        self.draft.is_empty()
            && self.pending_images.is_empty()
            && self.pending_remote_images.is_empty()
            && self.pending_file.is_none()
            && self.reply_to.is_none()
            && self.editing_message_id.is_none()
    }

    /// Esc 每次只取消一层：正文/回复/编辑，然后是附件。
    pub fn cancel_composer_layer(&mut self) -> bool {
        if self.editing_send_pending {
            return false;
        }
        if self.reply_to.is_some()
            || self.editing_message_id.is_some()
            || !self.draft.is_empty()
            || !self.mentions.is_empty()
        {
            self.reply_to = None;
            self.editing_message_id = None;
            self.draft.clear();
            self.mentions.clear();
            self.highlight_message_id = None;
            self.scroll_to_message_id = None;
            self.scroll_to_message_attempts = 0;
            return true;
        }
        if !self.pending_images.is_empty()
            || !self.pending_remote_images.is_empty()
            || self.pending_file.is_some()
        {
            self.pending_images.clear();
            self.pending_remote_images.clear();
            self.pending_file = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_cancels_text_reply_and_edit_before_removing_attachments() {
        let reply: ReplyMessage = serde_json::from_value(serde_json::json!({
            "_id": "引用", "content": "被引用消息", "username": "测试成员"
        }))
        .unwrap();
        let mut conversation = ConversationState {
            draft: "草稿".into(),
            reply_to: Some(reply),
            editing_message_id: Some("旧消息".into()),
            highlight_message_id: Some("引用".into()),
            scroll_to_message_id: Some("引用".into()),
            pending_file: Some(PendingFile::new(
                "测试.txt".into(),
                "text/plain".into(),
                vec![1],
            )),
            pending_images: vec![PendingImage::new(
                "测试.png".into(),
                "image/png".into(),
                vec![2],
            )],
            pending_remote_images: vec![MessageFile {
                file_type: "image/png".into(),
                url: "https://example.invalid/image.png".into(),
                size: None,
                name: None,
                fid: None,
            }],
            ..Default::default()
        };
        assert!(conversation.cancel_composer_layer());
        assert!(conversation.draft.is_empty());
        assert!(conversation.reply_to.is_none());
        assert!(conversation.editing_message_id.is_none());
        assert!(conversation.highlight_message_id.is_none());
        assert!(conversation.scroll_to_message_id.is_none());
        assert!(conversation.pending_file.is_some());
        assert_eq!(conversation.pending_images.len(), 1);
        assert_eq!(conversation.pending_remote_images.len(), 1);
        assert!(conversation.cancel_composer_layer());
        assert!(!conversation.has_composer_content());
        assert!(!conversation.cancel_composer_layer());
    }
}
