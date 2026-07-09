use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::ica::types::{
    RoomId,
    message::{Mention, Message, ReplyMessage},
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::{ChatGroups, SelectedChatGroup};

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
}

/// 图片查看器状态（通过 Arc<Mutex<>> 在主窗口和 viewport 间共享）
#[derive(Debug)]
pub struct ImageViewerState {
    /// 当前图片 URL
    pub url: String,
    /// 当前会话中可连续浏览的图片 URL。
    pub images: Vec<String>,
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
        Self {
            url,
            images,
            image_index,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
        }
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
    PreviewImage {
        url: String,
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
/// 单个 bridge 在 GUI 侧维护的完整状态。
///
/// 这里既存连接状态，也存房间列表、消息缓存、验证消息和草稿，
/// 这样切换 bridge 时不需要把全局状态来回覆写。
pub struct BridgeState {
    pub bridge_key: String,
    /// 由当前 bridge 同步下来的聊天分组，避免多连接事件互相覆盖。
    pub chat_groups: ChatGroups,
    /// 每个 bridge 独立保存当前选中的聊天分组。
    pub selected_chat_group: SelectedChatGroup,
    pub rooms: Vec<Room>,
    pub messages_by_room: HashMap<RoomId, Vec<Message>>,
    /// 已按群聊缓存的成员列表，仅在打开 @ 选择器时懒加载。
    pub group_members_by_room: HashMap<RoomId, Vec<GroupMember>>,
    pub loading_group_members: HashSet<RoomId>,
    pub message_scroll_to_bottom: HashSet<RoomId>,
    pub pending_message_scroll_to_bottom: HashSet<RoomId>,
    pub pending_send_scroll_to_bottom: HashSet<RoomId>,
    /// 上一帧位于消息列表底部附近的房间。
    pub message_near_bottom: HashSet<RoomId>,
    /// 用户上滑后当前会话收到的新消息数量。
    pub new_message_counts: HashMap<RoomId, usize>,
    pub join_requests: Vec<JoinRequestRoom>,
    pub reply_to_by_room: HashMap<RoomId, ReplyMessage>,
    pub pending_image_by_room: HashMap<RoomId, Vec<PendingImage>>,
    pub pending_file_by_room: HashMap<RoomId, PendingFile>,
    pub selected_room_id: Option<RoomId>,
    pub draft_by_room: HashMap<RoomId, String>,
    pub mentions_by_room: HashMap<RoomId, Vec<Mention>>,
    pub forward_room_id: Option<RoomId>,
    pub forward_selected_message_ids: Vec<String>,
    pub forward_target_picker_open: bool,
    pub forward_target_search_query: String,
    pub room_search_query: String,
    pub requested_rooms: HashSet<RoomId>,
    pub socket_state: SocketState,
    pub auth_state: AuthState,
    pub online_data: OnlineData,
    /// 当前会话/账号是否被禁言。
    pub is_shut_up: bool,
    pub last_error: Option<String>,
    /// 非错误类的最近提示，例如服务端通知、消息发送成功等。
    pub last_notice: Option<String>,
    /// 高级 Socket API 最近一次响应。
    pub last_socket_api_response: Option<String>,
    /// 当前 bridge 的聊天记录搜索窗口状态。
    pub message_search: MessageSearchState,
    /// Bridge 要求登录/初始化时附带的账号信息。
    pub setup_requested: Option<String>,
    /// 服务端广播的致命错误。
    pub fatal_error: Option<String>,
    pub last_event: Option<String>,
    /// 正在加载更旧历史消息的房间
    pub loading_older_messages: HashSet<RoomId>,
    /// 已经没有更多历史消息的房间
    pub no_more_history: HashSet<RoomId>,
    /// 标记哪些房间刚发生了 prepend（需要调整 scroll offset）
    pub prepend_scroll_fix: HashSet<RoomId>,
    /// 每帧记录每个房间消息列表的 content_size.y
    pub last_content_height: HashMap<RoomId, f32>,
    /// 每个房间的消息列表滚动偏移。
    pub message_scroll_offsets: HashMap<RoomId, f32>,
    /// 每个房间内单条消息渲染后的高度缓存，用于消息列表虚拟化。
    pub message_row_heights: HashMap<RoomId, HashMap<String, f32>>,
    /// 每个房间的消息行位置缓存，避免每次重绘都扫描全部历史消息。
    pub message_row_layouts: HashMap<RoomId, Vec<MessageRowLayout>>,
    /// 消息高度缓存对应的布局参数。布局变化后需要重新测量。
    pub message_layout_cache_keys: HashMap<RoomId, MessageLayoutCacheKey>,
    /// 需要滚动到的目标消息 ID
    pub scroll_to_message_id: Option<String>,
    /// 为定位引用消息自动补拉历史的次数。
    pub scroll_to_message_attempts: u8,
}

impl BridgeState {
    pub fn new(bridge_key: String, chat_groups: ChatGroups) -> Self {
        Self {
            bridge_key,
            chat_groups,
            selected_chat_group: SelectedChatGroup::All,
            rooms: Vec::new(),
            messages_by_room: HashMap::new(),
            group_members_by_room: HashMap::new(),
            loading_group_members: HashSet::new(),
            message_scroll_to_bottom: HashSet::new(),
            pending_message_scroll_to_bottom: HashSet::new(),
            pending_send_scroll_to_bottom: HashSet::new(),
            message_near_bottom: HashSet::new(),
            new_message_counts: HashMap::new(),
            join_requests: Vec::new(),
            reply_to_by_room: HashMap::new(),
            pending_image_by_room: HashMap::new(),
            pending_file_by_room: HashMap::new(),
            selected_room_id: None,
            draft_by_room: HashMap::new(),
            mentions_by_room: HashMap::new(),
            forward_room_id: None,
            forward_selected_message_ids: Vec::new(),
            forward_target_picker_open: false,
            forward_target_search_query: String::new(),
            room_search_query: String::new(),
            requested_rooms: HashSet::new(),
            socket_state: SocketState::Connecting,
            auth_state: AuthState::Unknown,
            online_data: OnlineData::default(),
            is_shut_up: false,
            last_error: None,
            last_notice: None,
            last_socket_api_response: None,
            message_search: MessageSearchState::default(),
            setup_requested: None,
            fatal_error: None,
            last_event: None,
            loading_older_messages: HashSet::new(),
            no_more_history: HashSet::new(),
            prepend_scroll_fix: HashSet::new(),
            last_content_height: HashMap::new(),
            message_scroll_offsets: HashMap::new(),
            message_row_heights: HashMap::new(),
            message_row_layouts: HashMap::new(),
            message_layout_cache_keys: HashMap::new(),
            scroll_to_message_id: None,
            scroll_to_message_attempts: 0,
        }
    }

    pub fn invalidate_message_layout(&mut self, room_id: RoomId) {
        self.message_row_heights.remove(&room_id);
        self.message_row_layouts.remove(&room_id);
        self.message_layout_cache_keys.remove(&room_id);
        self.last_content_height.remove(&room_id);
    }

    pub(super) fn invalidate_message_rows(&mut self, room_id: RoomId) {
        self.message_row_layouts.remove(&room_id);
    }

    pub(super) fn invalidate_message_height(&mut self, msg_id: &str) {
        for heights in self.message_row_heights.values_mut() {
            heights.remove(msg_id);
        }
        self.message_row_layouts.clear();
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
        room.last_message.timestamp = Some(message.time.format("%H:%M:%S").to_string());
        room.utime = message.time.timestamp_millis();
    }

    pub fn upsert_message(&mut self, room_id: RoomId, message: Message) -> bool {
        let msg_id = message.msg_id.clone();
        let messages = self.messages_by_room.entry(room_id).or_default();
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
        if let Some(heights) = self.message_row_heights.get_mut(&room_id) {
            heights.remove(&msg_id);
        }
        self.invalidate_message_rows(room_id);
        inserted
    }

    pub fn mark_message_deleted(&mut self, msg_id: &str) {
        let mut changed = false;
        for messages in self.messages_by_room.values_mut() {
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
        for messages in self.messages_by_room.values_mut() {
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
        for messages in self.messages_by_room.values_mut() {
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
        self.messages_by_room
            .get(&room_id)?
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
    }

    pub fn replace_forward_selection(&mut self, room_id: RoomId, message_id: String) {
        self.forward_room_id = Some(room_id);
        self.forward_selected_message_ids.clear();
        self.forward_selected_message_ids.push(message_id);
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
    use super::ImageViewerState;

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
}
