use std::collections::HashMap;
use std::fmt::Display;
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::ica::types::{
    RoomId,
    message::{Message, ReplyMessage},
    room::JoinRequestRoom,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::config::ChatGroups;

use super::SelectedChatGroup;
use super::contacts::ContactDirectory;
use super::media::{ImageAction, ImageSource};

mod conversation;
mod session;
mod ui;

pub use conversation::ConversationState;
pub use session::{
    BridgeSession, ConnectionState, RoomDirectory, StatusMessage, StatusMessageKind,
};
pub use ui::{AppState, GroupBanConfirmation, GroupMemberFilter};

fn deserialize_string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(String::new());
    };

    Ok(match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value,
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            serde_json::to_string(&value).unwrap_or_else(|_| String::new())
        }
    })
}

fn deserialize_i64_or_default<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(0);
    };
    match value {
        JsonValue::Null => Ok(0),
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .ok_or_else(|| serde::de::Error::custom("integer is outside i64 range")),
        JsonValue::String(value) if value.trim().is_empty() => Ok(0),
        JsonValue::String(value) => value.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("应为整数、整数字符串或 null")),
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GroupMember {
    pub user_id: i64,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub nickname: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub card: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub remark: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub level: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub role: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_default")]
    pub shutup_time: i64,
}

impl GroupMember {
    pub fn display_name(&self) -> &str {
        if self.card.trim().is_empty() {
            &self.nickname
        } else {
            &self.card
        }
    }

    pub fn matches_search(&self, query: &str) -> bool {
        query.is_empty()
            || self.user_id.to_string().contains(query)
            || [
                self.display_name(),
                self.nickname.as_str(),
                self.card.as_str(),
                self.remark.as_str(),
                self.title.as_str(),
                self.level.as_str(),
                self.role.as_str(),
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(query))
    }

    pub fn is_muted_at(&self, timestamp: i64) -> bool {
        self.shutup_time > timestamp
    }

    pub fn remaining_mute_seconds_at(&self, timestamp: i64) -> u64 {
        u64::try_from(self.shutup_time.saturating_sub(timestamp)).unwrap_or(0)
    }

    pub fn role_rank(&self) -> u8 {
        match self.role.trim().to_ascii_lowercase().as_str() {
            "owner" => 2,
            "admin" | "administrator" => 1,
            _ => 0,
        }
    }

    pub fn role_label(&self) -> Option<&'static str> {
        match self.role_rank() {
            2 => Some("群主"),
            1 => Some("管理员"),
            _ => None,
        }
    }

    pub fn moderation_denial_reason(
        actor: Option<&GroupMember>,
        target: &GroupMember,
        self_id: i64,
    ) -> Option<&'static str> {
        if target.user_id == self_id {
            return Some("不能管理自己");
        }
        let Some(actor) = actor else {
            return Some("成员列表中没有当前账号的权限信息");
        };
        if actor.role_rank() == 0 {
            return Some("普通成员只能查看群成员");
        }
        if target.role_rank() >= actor.role_rank() {
            return Some("不能管理同级或更高权限成员");
        }
        None
    }
}

#[cfg(test)]
mod group_member_tests {
    use serde_json::json;

    use super::GroupMember;

    fn member(user_id: i64, role: &str) -> GroupMember {
        serde_json::from_value(json!({
            "user_id": user_id,
            "nickname": user_id.to_string(),
            "role": role,
            "shutup_time": 100,
        }))
        .unwrap()
    }

    #[test]
    fn mute_boundary_and_moderation_permissions_match_group_roles() {
        let owner = member(1, "owner");
        let admin = member(2, "admin");
        let regular = member(3, "member");

        assert!(regular.is_muted_at(99));
        assert!(!regular.is_muted_at(100));
        assert_eq!(regular.remaining_mute_seconds_at(98), 2);
        assert!(GroupMember::moderation_denial_reason(Some(&owner), &admin, 1).is_none());
        assert!(GroupMember::moderation_denial_reason(Some(&admin), &regular, 2).is_none());
        assert!(GroupMember::moderation_denial_reason(Some(&admin), &owner, 2).is_some());
        assert!(GroupMember::moderation_denial_reason(Some(&regular), &admin, 3).is_some());
        assert!(GroupMember::moderation_denial_reason(Some(&owner), &owner, 1).is_some());
    }
}

/// 图片查看器状态（通过 Arc<Mutex<>> 在主窗口和 viewport 间共享）
#[derive(Debug)]
pub struct ImageViewerState {
    /// 当前图片 URL
    pub url: String,
    /// 当前会话中可连续浏览的图片 URL。
    pub images: Vec<String>,
    /// 与 `images` 对齐的来源信息，用于复制、保存和在聊天中定位。
    pub sources: Vec<ImageSource>,
    /// 当前图片在 images 中的位置。
    pub image_index: usize,
    /// 缩放比例 (1.0 = 适应窗口)
    pub zoom: f32,
    /// 平移偏移量（像素）
    pub pan_offset: egui::Vec2,
    /// 窗口已关闭
    pub closed: AtomicBool,
    /// 适应窗口的基础缩放比例（渲染时更新）
    pub base_scale: f32,
    /// 是否请求 1:1 原始尺寸
    pub request_original_size: bool,
    /// viewport 只写入动作，主应用在下一帧统一处理副作用。
    pub pending_action: Option<ImageAction>,
}

impl ImageViewerState {
    pub fn new(url: String) -> Self {
        Self::with_images(url.clone(), vec![url])
    }

    pub fn with_images(url: String, mut images: Vec<String>) -> Self {
        if images.is_empty() {
            images.push(url.clone());
        }
        let image_index = images.iter().position(|item| item == &url).unwrap_or(0);
        let url = images[image_index].clone();
        let sources = images.iter().cloned().map(ImageSource::url).collect();
        Self {
            url,
            images,
            sources,
            image_index,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
            pending_action: None,
        }
    }

    pub fn with_sources(current: ImageSource, mut sources: Vec<ImageSource>) -> Self {
        if sources.is_empty() {
            sources.push(current.clone());
        }
        let image_index = sources
            .iter()
            .position(|source| source == &current)
            .or_else(|| sources.iter().position(|source| source.url == current.url))
            .unwrap_or(0);
        let images = sources.iter().map(|source| source.url.clone()).collect();
        let url = sources[image_index].url.clone();
        Self {
            url,
            images,
            sources,
            image_index,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
            pending_action: None,
        }
    }

    pub fn current_source(&self) -> ImageSource {
        self.sources
            .get(self.image_index)
            .cloned()
            .unwrap_or_else(|| ImageSource::url(self.url.clone()))
    }

    pub fn navigate(&mut self, offset: isize) -> bool {
        let next_index = self.image_index as isize + offset;
        if !(0..self.images.len() as isize).contains(&next_index) {
            return false;
        }
        self.image_index = next_index as usize;
        self.url = self.images[self.image_index].clone();
        self.fit_to_window();
        self.request_original_size = false;
        true
    }

    /// 适应窗口大小（重置缩放和偏移）
    pub fn fit_to_window(&mut self) {
        self.zoom = 1.0;
        self.pan_offset = egui::Vec2::ZERO;
    }

    /// 放大 20%
    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * 1.2).min(20.0);
    }

    /// 缩小 20%
    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / 1.2).max(0.05);
    }

    /// 缩放百分比文本（相对于原始像素大小）
    pub fn zoom_percent_text(&self) -> String {
        format!("{:.0}%", self.base_scale * self.zoom * 100.0)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum ChatListScrollTarget {
    #[default]
    None,
    Top,
    Bottom,
}

#[derive(Debug, Clone)]
pub enum MessageAction {
    Reply {
        room_id: RoomId,
        reply: ReplyMessage,
    },
    Delete {
        room_id: RoomId,
        message_id: String,
    },
    ReEdit {
        room_id: RoomId,
        content: String,
    },
    SetReveal {
        room_id: RoomId,
        message_id: String,
        reveal: bool,
    },
    CopyToDraft {
        room_id: RoomId,
        message_id: String,
    },
    PlusOne {
        room_id: RoomId,
        message_id: String,
    },
    ToggleForwardSelection {
        room_id: RoomId,
        message_id: String,
    },
    StartForward {
        room_id: RoomId,
        message_id: String,
    },
    OpenForward {
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
        inline_messages: Option<Vec<Message>>,
    },
    ScrollToMessage {
        msg_id: String,
    },
    RenewMessage {
        room_id: RoomId,
        message_id: String,
    },
    Poke {
        room_id: RoomId,
        target_id: i64,
    },
    Image(ImageAction),
}

#[derive(Debug, Clone)]
pub struct PendingImage {
    pub preview_id: u64,
    pub name: String,
    pub mime_type: String,
    pub data: Arc<[u8]>,
}

impl PendingImage {
    pub fn new(name: String, mime_type: String, data: Vec<u8>) -> Self {
        static NEXT_PREVIEW_ID: AtomicU64 = AtomicU64::new(1);

        Self {
            preview_id: NEXT_PREVIEW_ID.fetch_add(1, Ordering::Relaxed),
            name,
            mime_type,
            data: data.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingFile {
    pub name: String,
    pub file_type: String,
    pub data: Arc<[u8]>,
}

impl PendingFile {
    pub fn new(name: String, file_type: String, data: Vec<u8>) -> Self {
        Self {
            name,
            file_type,
            data: data.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageSearchState {
    pub open: bool,
    pub room_id: Option<RoomId>,
    pub room_name: String,
    pub keyword: String,
    pub searched_keyword: String,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub has_more: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ForwardViewerState {
    pub open: bool,
    pub res_id: String,
    pub file_name: String,
    pub fallback_res_id: Option<String>,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub last_error: Option<String>,
    request_id: u64,
}

impl ForwardViewerState {
    pub fn begin_request(
        &mut self,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.open = true;
        self.res_id = res_id;
        self.file_name = file_name.unwrap_or_default();
        self.fallback_res_id = fallback_res_id;
        self.messages.clear();
        self.loading = true;
        self.last_error = None;
        self.request_id
    }

    pub fn apply_response(
        &mut self,
        request_id: u64,
        res_id: Option<String>,
        messages: Vec<Message>,
    ) {
        if request_id != self.request_id {
            return;
        }
        if let Some(res_id) = res_id {
            self.res_id = res_id;
        }
        self.fallback_res_id = None;
        self.messages = messages;
        self.loading = false;
        self.last_error = None;
    }

    pub fn open_inline(
        &mut self,
        res_id: String,
        file_name: Option<String>,
        messages: Vec<Message>,
    ) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.open = true;
        self.res_id = res_id;
        self.file_name = file_name.unwrap_or_default();
        self.fallback_res_id = None;
        self.messages = messages;
        self.loading = false;
        self.last_error = None;
    }

    pub fn fail(&mut self, request_id: u64, error: String) {
        if request_id != self.request_id {
            return;
        }
        self.loading = false;
        self.last_error = Some(error);
    }
}

impl Default for MessageSearchState {
    fn default() -> Self {
        Self {
            open: false,
            room_id: None,
            room_name: String::new(),
            keyword: String::new(),
            searched_keyword: String::new(),
            messages: Vec::new(),
            loading: false,
            has_more: true,
            last_error: None,
        }
    }
}

impl MessageSearchState {
    pub fn open_for_room(&mut self, room_id: RoomId, room_name: String) {
        if self.room_id != Some(room_id) {
            self.keyword.clear();
            self.searched_keyword.clear();
            self.messages.clear();
            self.has_more = true;
            self.loading = false;
            self.last_error = None;
        }
        self.open = true;
        self.room_id = Some(room_id);
        self.room_name = room_name;
    }

    pub fn start_request(&mut self, keyword: String, offset: usize) {
        if offset == 0 || self.searched_keyword != keyword {
            self.messages.clear();
            self.has_more = true;
        }
        self.searched_keyword = keyword;
        self.loading = true;
        self.last_error = None;
    }

    pub fn apply_response(
        &mut self,
        room_id: RoomId,
        keyword: String,
        offset: usize,
        messages: Vec<Message>,
    ) {
        if self.room_id != Some(room_id) || self.searched_keyword != keyword {
            return;
        }

        self.loading = false;
        self.last_error = None;
        self.has_more = messages.len() >= 20;

        if offset == 0 {
            self.messages = messages;
            return;
        }

        for message in messages {
            if !self
                .messages
                .iter()
                .any(|existing| existing.msg_id == message.msg_id)
            {
                self.messages.push(message);
            }
        }
    }

    pub fn fail(&mut self, error: String) {
        self.loading = false;
        self.last_error = Some(error);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl Display for SocketState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketState::Connecting => write!(f, "连接中"),
            SocketState::Connected => write!(f, "已连接"),
            SocketState::Disconnected => write!(f, "已断开"),
            SocketState::Failed => write!(f, "连接失败"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    #[default]
    Unknown,
    Pending,
    Succeeded,
    Failed,
}

impl Display for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthState::Unknown => write!(f, "未开始"),
            AuthState::Pending => write!(f, "认证中"),
            AuthState::Succeeded => write!(f, "已认证"),
            AuthState::Failed => write!(f, "认证失败"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageLayoutCacheKey {
    pub width: f32,
    pub pure_text_mode: bool,
    pub forward_mode_active: bool,
}

impl MessageLayoutCacheKey {
    pub fn matches(self, other: Self) -> bool {
        (self.width - other.width).abs() <= 8.0
            && self.pure_text_mode == other.pure_text_mode
            && self.forward_mode_active == other.forward_mode_active
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageRowLayout {
    pub top: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct VisibleRoomIndicesCache {
    pub revision: u64,
    pub query: String,
    pub selected_chat_group: SelectedChatGroup,
    pub disable_chat_group: bool,
    pub indices: Vec<usize>,
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
    pub forward_room_id: Option<RoomId>,
    pub forward_selected_message_ids: Vec<String>,
    pub forward_target_picker_open: bool,
    pub forward_target_search_query: String,
    pub forward_target_room_ids: Vec<RoomId>,
    pub forward_target_as_merged: bool,
    pub forward_viewer: ForwardViewerState,
    pub room_search_query: String,
    /// Friends and groups fetched from this bridge for starting new chats.
    pub contacts: ContactDirectory,
    /// 当前 bridge 的聊天记录搜索窗口状态。
    pub message_search: MessageSearchState,
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
            forward_room_id: None,
            forward_selected_message_ids: Vec::new(),
            forward_target_picker_open: false,
            forward_target_search_query: String::new(),
            forward_target_room_ids: Vec::new(),
            forward_target_as_merged: true,
            forward_viewer: ForwardViewerState::default(),
            room_search_query: String::new(),
            contacts: ContactDirectory::default(),
            message_search: MessageSearchState::default(),
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
            let limit = if Some(room_id) == active_room_id {
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
            if Some(*room_id) != active_room_id {
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

    pub(super) fn invalidate_message_rows(&mut self, room_id: RoomId) {
        if let Some(conversation) = self.conversations.get_mut(&room_id) {
            conversation.message_row_layouts.clear();
        }
    }

    pub(super) fn invalidate_message_height(&mut self, msg_id: &str) {
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
    use super::{BridgeState, ImageViewerState};
    use crate::app::media::ImageSource;
    use crate::config::ChatGroups;

    #[test]
    fn image_viewer_navigates_within_gallery_and_resets_transform() {
        let mut viewer = ImageViewerState::with_images(
            "second".to_string(),
            vec!["first".to_string(), "second".to_string()],
        );
        viewer.zoom = 3.0;
        viewer.pan_offset = egui::vec2(12.0, 8.0);

        assert!(viewer.navigate(-1));
        assert_eq!(viewer.url, "first");
        assert_eq!(viewer.zoom, 1.0);
        assert_eq!(viewer.pan_offset, egui::Vec2::ZERO);
        assert!(!viewer.navigate(-1));
    }

    #[test]
    fn image_viewer_keeps_message_location_aligned_while_navigating() {
        let first = ImageSource::message("first".to_string(), -42, "m1".to_string());
        let second = ImageSource::message("second".to_string(), -42, "m2".to_string());
        let mut viewer =
            ImageViewerState::with_sources(second.clone(), vec![first.clone(), second.clone()]);

        assert_eq!(viewer.current_source(), second);
        assert!(viewer.navigate(-1));
        assert_eq!(viewer.current_source(), first);
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
