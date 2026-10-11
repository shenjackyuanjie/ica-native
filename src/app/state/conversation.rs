//! 单个会话（一个群或一个私聊）拥有的全部可变状态。
//!
//! 每个房间在 `BridgeState::conversations` 中各有一份，按需惰性创建。
//! 这里混合了三类状态：协议数据（消息、群成员）、输入区状态（草稿、回复、
//! 待发送附件、编辑）和渲染状态（滚动、行高与布局缓存）。输入区状态按会话
//! 保存，切换会话再切回来时不会丢失未发送内容。

#[cfg(test)]
#[path = "tests/conversation.rs"]
mod tests;

use super::{GroupMember, MessageLayoutCacheKey, MessageRowLayout, PendingFile, PendingImage};
use crate::ica::types::{
    files::MessageFile,
    message::{Mention, Message, ReplyMessage},
};
use std::collections::HashMap;

/// 单个会话拥有的全部可变状态。
#[derive(Debug, Clone, Default)]
pub struct ConversationState {
    /// 已加载的消息，按时间从旧到新排列。
    pub messages: Vec<Message>,
    /// 群成员列表；私聊为空。
    pub group_members: Vec<GroupMember>,
    /// 群成员列表是否已加载完成。
    pub group_members_loaded: bool,
    /// 是否正在拉取群成员（防止重复请求）。
    pub loading_group_members: bool,
    /// 请求滚动到底部（一次性请求，消费后清除）。
    pub scroll_to_bottom: bool,
    /// 待消息列表更新后滚动到底部（新消息到达时置位）。
    pub pending_message_scroll_to_bottom: bool,
    /// 发送成功后滚动到底部（等待发送结果时置位）。
    pub pending_send_scroll_to_bottom: bool,
    /// 当前滚动位置是否贴近底部；决定新消息是否自动滚动。
    pub near_bottom: bool,
    /// 不在底部期间累积的新消息数，用于“回到底部”按钮上的角标。
    pub new_message_count: usize,
    /// 当前正在回复的消息；`None` 表示不在回复状态。
    pub reply_to: Option<ReplyMessage>,
    /// 编辑重发仅属于当前 bridge 的当前会话，不随窗口焦点迁移。
    pub editing_message_id: Option<String>,
    /// 提交期间保留编辑草稿，失败后可原样重试；不允许重复提交。
    pub editing_send_pending: bool,
    /// 从历史消息恢复的图片引用，不能只恢复正文而丢弃原附件。
    pub pending_remote_images: Vec<MessageFile>,
    /// 本地待发送图片（来自剪贴板、选择框或拖放）。
    pub pending_images: Vec<PendingImage>,
    /// 本地待发送的普通文件；至多一个。
    pub pending_file: Option<PendingFile>,
    /// 输入框正文草稿。
    pub draft: String,
    /// 正文中的 @提及列表，与正文一起发送。
    pub mentions: Vec<Mention>,
    /// 是否已向 Bridge 请求过完整会话快照。
    pub requested_snapshot: bool,
    /// 是否正在向上拉取更早的历史消息。
    pub loading_older_messages: bool,
    /// 是否已经没有更早的历史可拉。
    pub no_more_history: bool,
    /// 向前插入历史后需要做的滚动位置补偿。
    pub prepend_scroll_fix: bool,
    /// 上一帧消息区域的内容高度，用于检测内容增长。
    pub last_content_height: Option<f32>,
    /// 上一帧的滚动偏移，配合阅读锚点恢复位置。
    pub message_scroll_offset: Option<f32>,
    /// 消息 id 到实测行高的缓存，供虚拟列表估算总高度。
    pub message_row_heights: HashMap<String, f32>,
    /// 可见行的布局结果（位置与高度）。
    pub message_row_layouts: Vec<MessageRowLayout>,
    /// 生成上述布局时对应的布局键；变化即整体失效。
    pub message_layout_cache_key: Option<MessageLayoutCacheKey>,
    /// 键盘选择回复后的持续高亮，与一次性的滚动请求分开。
    pub highlight_message_id: Option<String>,
    /// 请求滚动到某条消息（一次性，可能需多帧重试）。
    pub scroll_to_message_id: Option<String>,
    /// 滚动到消息的已尝试次数，避免目标行尚未渲染时无限重试。
    pub scroll_to_message_attempts: u8,
}

impl ConversationState {
    /// 输入区是否有任何待发送内容（正文、本地图片、远程图片或文件）。
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
    ///
    /// 返回是否消费了这次取消；提交进行中（`editing_send_pending`）时拒绝。
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
