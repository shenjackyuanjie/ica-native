use std::collections::HashMap;

use crate::ica::types::message::{Mention, Message, ReplyMessage};

use super::{GroupMember, MessageLayoutCacheKey, MessageRowLayout, PendingFile, PendingImage};

/// All mutable state owned by a single room.
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
    pub scroll_to_message_id: Option<String>,
    pub scroll_to_message_attempts: u8,
}
