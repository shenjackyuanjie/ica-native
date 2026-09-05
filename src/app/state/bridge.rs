//! 单个 bridge 在 GUI 侧维护的完整状态。

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

use crate::config::ChatGroups;
use crate::ica::types::{RoomId, message::Message, room::JoinRequestRoom};

use super::{
    ConversationState, ForwardViewerState, GroupAnnouncementViewerState, GroupMember,
    MemberHistoryState, MessageSearchState, RoomDirectory,
};
use crate::app::SelectedChatGroup;
use crate::app::contacts::ContactDirectory;

#[derive(Debug, Clone)]
pub struct VisibleRoomIndicesCache {
    pub revision: u64,
    pub query: String,
    pub selected_chat_group: SelectedChatGroup,
    pub disable_chat_group: bool,
    pub indices: Vec<usize>,
}

/// Bridge 数据库升级（例如重建消息搜索索引）的当前进度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseUpgradeProgress {
    pub active: bool,
    pub step: u64,
    pub total: u64,
    pub message: String,
}

impl DatabaseUpgradeProgress {
    /// 返回 0 到 1 之间的进度；未知总量时保持在 0，交给界面以不定进度展示。
    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.step as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone)]
/// 单个 bridge 在 GUI 侧维护的完整状态。
///
/// 这里既存连接状态，也存房间列表、消息缓存、验证消息和草稿，
/// 这样切换 bridge 时不需要把全局状态来回覆写。
pub struct BridgeState {
    pub directory: RoomDirectory,
    pub conversations: HashMap<RoomId, ConversationState>,
    pub selected_room_id: Option<RoomId>,
    /// 独立窗口中的会话也需要保留活跃会话的历史缓存。
    pub detached_room_ids: std::collections::HashSet<RoomId>,
    pub forward_room_id: Option<RoomId>,
    pub forward_selected_message_ids: Vec<String>,
    pub forward_target_picker_open: bool,
    pub forward_target_search_query: String,
    pub forward_target_room_ids: Vec<RoomId>,
    pub forward_target_as_merged: bool,
    pub forward_viewer: Arc<Mutex<ForwardViewerState>>,
    /// 当前 bridge 的群公告窗口状态。
    pub group_announcement_viewer: Arc<Mutex<GroupAnnouncementViewerState>>,
    pub room_search_query: String,
    /// 从当前 bridge 获取、用于发起新会话的好友和群列表。
    pub contacts: Arc<Mutex<ContactDirectory>>,
    /// 当前 bridge 的聊天记录搜索窗口状态。
    pub message_search: MessageSearchState,
    pub member_history: MemberHistoryState,
    /// Bridge 数据库升级进度；非活跃时保留最后一次完成状态，界面不显示横幅。
    pub db_upgrade_progress: DatabaseUpgradeProgress,
}

impl Deref for BridgeState {
    type Target = RoomDirectory;

    fn deref(&self) -> &Self::Target {
        &self.directory
    }
}

impl DerefMut for BridgeState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.directory
    }
}

impl BridgeState {
    const ACTIVE_ROOM_MESSAGE_LIMIT: usize = 5_000;
    const BACKGROUND_ROOM_MESSAGE_LIMIT: usize = 300;
    const SEARCH_MESSAGE_LIMIT: usize = 200;

    pub fn new(bridge_key: String, chat_groups: ChatGroups) -> Self {
        Self {
            directory: RoomDirectory::new(bridge_key, chat_groups),
            conversations: HashMap::new(),
            selected_room_id: None,
            detached_room_ids: Default::default(),
            forward_room_id: None,
            forward_selected_message_ids: Vec::new(),
            forward_target_picker_open: false,
            forward_target_search_query: String::new(),
            forward_target_room_ids: Vec::new(),
            forward_target_as_merged: true,
            forward_viewer: Arc::new(Mutex::new(ForwardViewerState::default())),
            group_announcement_viewer: Arc::new(
                Mutex::new(GroupAnnouncementViewerState::default()),
            ),
            room_search_query: String::new(),
            contacts: Arc::new(Mutex::new(ContactDirectory::default())),
            message_search: MessageSearchState::default(),
            member_history: MemberHistoryState::default(),
            db_upgrade_progress: DatabaseUpgradeProgress::default(),
        }
    }

    pub fn conversation(&self, room_id: RoomId) -> Option<&ConversationState> {
        self.conversations.get(&room_id)
    }

    pub fn conversation_mut(&mut self, room_id: RoomId) -> &mut ConversationState {
        self.conversations.entry(room_id).or_default()
    }

    pub fn group_members_snapshot(&self) -> HashMap<RoomId, Vec<GroupMember>> {
        self.conversations
            .iter()
            .filter(|(room_id, conversation)| **room_id < 0 && conversation.group_members_loaded)
            .map(|(room_id, conversation)| (*room_id, conversation.group_members.clone()))
            .collect()
    }

    pub fn group_members_loaded(&self, room_id: RoomId) -> bool {
        self.conversation(room_id)
            .is_some_and(|conversation| conversation.group_members_loaded)
    }

    pub fn group_members_loading(&self, room_id: RoomId) -> bool {
        self.conversation(room_id)
            .is_some_and(|conversation| conversation.loading_group_members)
    }

    pub fn bump_rooms_revision(&mut self) {
        self.rooms_revision = self.rooms_revision.wrapping_add(1);
        if self.rooms_revision == 0 {
            self.rooms_revision = 1;
        }
        self.visible_room_indices_cache = None;
    }

    pub fn invalidate_visible_room_indices(&mut self) {
        self.visible_room_indices_cache = None;
    }

    pub fn invalidate_message_layout(&mut self, room_id: RoomId) {
        if let Some(conversation) = self.conversations.get_mut(&room_id) {
            conversation.message_row_heights.clear();
            conversation.message_row_layouts.clear();
            conversation.message_layout_cache_key = None;
            conversation.last_content_height = None;
        }
    }

    fn trim_room_messages_to_limit(&mut self, room_id: RoomId, limit: usize) -> bool {
        let Some(conversation) = self.conversations.get_mut(&room_id) else {
            return false;
        };
        let messages = &mut conversation.messages;
        if messages.len() <= limit {
            return false;
        }

        let remove_count = messages.len() - limit;
        messages.drain(..remove_count);
        self.invalidate_message_layout(room_id);
        self.conversation_mut(room_id).no_more_history = false;
        true
    }

    fn trim_room_messages_to_limit_keep_oldest(&mut self, room_id: RoomId, limit: usize) -> bool {
        let Some(conversation) = self.conversations.get_mut(&room_id) else {
            return false;
        };
        let messages = &mut conversation.messages;
        if messages.len() <= limit {
            return false;
        }

        messages.truncate(limit);
        self.invalidate_message_layout(room_id);
        self.conversation_mut(room_id).no_more_history = false;
        true
    }

    pub fn trim_after_history_prepend(&mut self, room_id: RoomId) {
        self.trim_room_messages_to_limit_keep_oldest(room_id, Self::ACTIVE_ROOM_MESSAGE_LIMIT);
        self.trim_layout_caches_to_active_room(self.selected_room_id);
        self.trim_message_search_results();
    }

    pub fn trim_message_caches(&mut self, active_room_id: Option<RoomId>) {
        let room_ids = self.conversations.keys().copied().collect::<Vec<_>>();
        for room_id in room_ids {
            let limit =
                if Some(room_id) == active_room_id || self.detached_room_ids.contains(&room_id) {
                    Self::ACTIVE_ROOM_MESSAGE_LIMIT
                } else {
                    Self::BACKGROUND_ROOM_MESSAGE_LIMIT
                };
            self.trim_room_messages_to_limit(room_id, limit);
        }
        self.trim_layout_caches_to_active_room(active_room_id);
        self.trim_message_search_results();
    }

    pub fn trim_layout_caches_to_active_room(&mut self, active_room_id: Option<RoomId>) {
        for (room_id, conversation) in &mut self.conversations {
            if Some(*room_id) != active_room_id && !self.detached_room_ids.contains(room_id) {
                conversation.message_row_heights.clear();
                conversation.message_row_layouts.clear();
                conversation.message_layout_cache_key = None;
                conversation.last_content_height = None;
            }
        }
    }

    pub fn trim_message_search_results(&mut self) {
        if self.message_search.messages.len() <= Self::SEARCH_MESSAGE_LIMIT {
            return;
        }
        let remove_count = self.message_search.messages.len() - Self::SEARCH_MESSAGE_LIMIT;
        self.message_search.messages.drain(..remove_count);
    }

    pub fn invalidate_message_rows(&mut self, room_id: RoomId) {
        if let Some(conversation) = self.conversations.get_mut(&room_id) {
            conversation.message_row_layouts.clear();
        }
    }

    pub fn invalidate_message_height(&mut self, msg_id: &str) {
        for conversation in self.conversations.values_mut() {
            conversation.message_row_heights.remove(msg_id);
            conversation.message_row_layouts.clear();
        }
    }

    fn preview_content(message: &Message) -> String {
        if message.deleted {
            "[已撤回]".to_string()
        } else if !message.content.is_empty() {
            message.content.clone()
        } else if !message.files.is_empty() {
            format!("[{} 个文件]", message.files.len())
        } else if message.system {
            "[系统消息]".to_string()
        } else {
            "[空消息]".to_string()
        }
    }

    pub fn sync_room_preview(&mut self, room_id: RoomId, message: &Message) {
        let Some(room) = self.rooms.iter_mut().find(|room| room.room_id == room_id) else {
            return;
        };

        room.last_message.content = Some(Self::preview_content(message));
        room.last_message.username = Some(message.sender_name.clone());
        room.last_message.user_id = Some(message.sender_id);
        room.last_message.timestamp = Some(message.time_text.clone());
        room.utime = message.time.timestamp_millis();
        self.bump_rooms_revision();
    }

    pub fn upsert_message(&mut self, room_id: RoomId, message: Message) -> bool {
        let msg_id = message.msg_id.clone();
        let conversation = self.conversations.entry(room_id).or_default();
        let messages = &mut conversation.messages;
        let inserted = if let Some(existing) = messages
            .iter_mut()
            .find(|item| item.msg_id == message.msg_id)
        {
            *existing = message;
            false
        } else {
            messages.push(message);
            true
        };
        conversation.message_row_heights.remove(&msg_id);
        self.invalidate_message_rows(room_id);
        self.trim_room_messages_to_limit(room_id, Self::ACTIVE_ROOM_MESSAGE_LIMIT);
        inserted
    }

    pub fn mark_message_deleted(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.deleted = true;
                message.reveal = false;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    pub fn mark_message_hidden(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = true;
                message.reveal = false;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    pub fn mark_message_revealed(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = false;
                message.reveal = true;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    pub fn upsert_join_request(&mut self, request: JoinRequestRoom) {
        if let Some(existing) = self
            .join_requests
            .iter_mut()
            .find(|item| item.flag == request.flag)
        {
            *existing = request;
        } else {
            self.join_requests.insert(0, request);
        }
        self.join_requests
            .sort_by_key(|request| std::cmp::Reverse(request.time));
    }

    pub fn replace_join_requests(&mut self, mut requests: Vec<JoinRequestRoom>) {
        requests.sort_by_key(|request| std::cmp::Reverse(request.time));
        self.join_requests = requests;
    }

    pub fn find_message(&self, room_id: RoomId, message_id: &str) -> Option<&Message> {
        self.conversations
            .get(&room_id)?
            .messages
            .iter()
            .find(|message| message.msg_id == message_id)
    }

    pub fn is_forward_selection_active(&self, room_id: RoomId) -> bool {
        self.forward_room_id == Some(room_id) && !self.forward_selected_message_ids.is_empty()
    }

    pub fn is_forward_selected(&self, room_id: RoomId, message_id: &str) -> bool {
        self.is_forward_selection_active(room_id)
            && self
                .forward_selected_message_ids
                .iter()
                .any(|selected_id| selected_id == message_id)
    }

    pub fn clear_forward_selection(&mut self) {
        self.forward_room_id = None;
        self.forward_selected_message_ids.clear();
        self.forward_target_picker_open = false;
        self.forward_target_search_query.clear();
        self.forward_target_room_ids.clear();
        self.forward_target_as_merged = true;
    }

    pub fn replace_forward_selection(&mut self, room_id: RoomId, message_id: String) {
        self.forward_room_id = Some(room_id);
        self.forward_selected_message_ids.clear();
        self.forward_selected_message_ids.push(message_id);
        self.forward_target_room_ids.clear();
    }

    pub fn set_forward_target_selected(&mut self, room_id: RoomId, selected: bool) {
        if selected {
            if !self.forward_target_room_ids.contains(&room_id) {
                self.forward_target_room_ids.push(room_id);
            }
        } else {
            self.forward_target_room_ids
                .retain(|target_room_id| *target_room_id != room_id);
        }
    }

    pub fn add_forward_targets(&mut self, room_ids: impl IntoIterator<Item = RoomId>) {
        for room_id in room_ids {
            self.set_forward_target_selected(room_id, true);
        }
    }

    pub fn toggle_forward_selection(&mut self, room_id: RoomId, message_id: String) {
        if self.forward_room_id != Some(room_id) {
            self.replace_forward_selection(room_id, message_id);
            return;
        }

        if let Some(index) = self
            .forward_selected_message_ids
            .iter()
            .position(|selected_id| selected_id == &message_id)
        {
            self.forward_selected_message_ids.remove(index);
            if self.forward_selected_message_ids.is_empty() {
                self.clear_forward_selection();
            }
        } else {
            self.forward_selected_message_ids.push(message_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BridgeState;
    use crate::config::ChatGroups;

    #[test]
    fn detached_chat_keeps_history_until_its_window_closes() {
        let mut state = BridgeState::new("test".into(), ChatGroups::default());
        state.detached_room_ids.insert(-1);
        for room_id in [-1, -2] {
            let conversation = state.conversation_mut(room_id);
            conversation.messages = (0..400)
                .map(|index| {
                    serde_json::from_value(serde_json::json!({"_id": index.to_string(), "username": "tester", "content": "test"})).unwrap()
                })
                .collect();
            conversation.message_row_heights.insert("row".into(), 40.0);
        }
        state.trim_message_caches(Some(-3));
        assert_eq!(state.conversation(-1).unwrap().messages.len(), 400);
        assert_eq!(state.conversation(-1).unwrap().message_row_heights.len(), 1);
        assert_eq!(state.conversation(-2).unwrap().messages.len(), 300);
        assert!(
            state
                .conversation(-2)
                .unwrap()
                .message_row_heights
                .is_empty()
        );
        state.detached_room_ids.remove(&-1);
        state.trim_message_caches(Some(-3));
        assert_eq!(state.conversation(-1).unwrap().messages.len(), 300);
        assert!(
            state
                .conversation(-1)
                .unwrap()
                .message_row_heights
                .is_empty()
        );
    }

    #[test]
    fn forward_targets_support_multiple_rooms_without_duplicates() {
        let mut state = BridgeState::new("test".to_string(), ChatGroups::default());
        state.replace_forward_selection(-100, "m1".to_string());

        state.add_forward_targets([10001, -200, 10001]);
        assert_eq!(state.forward_target_room_ids, vec![10001, -200]);

        state.set_forward_target_selected(10001, false);
        assert_eq!(state.forward_target_room_ids, vec![-200]);

        state.replace_forward_selection(-300, "m2".to_string());
        assert!(state.forward_target_room_ids.is_empty());
    }
}
