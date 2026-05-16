use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::atomic::AtomicBool;

use crate::ica::types::{
    RoomId,
    message::{Message, ReplyMessage},
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};

/// 图片查看器状态（通过 Arc<Mutex<>> 在主窗口和 viewport 间共享）
#[derive(Debug)]
pub struct ImageViewerState {
    /// 图片 URL
    pub url: String,
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
        Self {
            url,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
        }
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
}

#[derive(Debug, Clone)]
pub struct PendingImage {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PendingFile {
    pub name: String,
    pub file_type: String,
    pub data: Vec<u8>,
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

#[derive(Debug, Clone)]
/// 单个 bridge 在 GUI 侧维护的完整状态。
///
/// 这里既存连接状态，也存房间列表、消息缓存、验证消息和草稿，
/// 这样切换 bridge 时不需要把全局状态来回覆写。
pub struct BridgeState {
    pub bridge_key: String,
    pub rooms: Vec<Room>,
    pub messages_by_room: HashMap<RoomId, Vec<Message>>,
    pub message_scroll_to_bottom: HashSet<RoomId>,
    pub pending_message_scroll_to_bottom: HashSet<RoomId>,
    pub pending_send_scroll_to_bottom: HashSet<RoomId>,
    pub join_requests: Vec<JoinRequestRoom>,
    pub reply_to_by_room: HashMap<RoomId, ReplyMessage>,
    pub pending_image_by_room: HashMap<RoomId, PendingImage>,
    pub pending_file_by_room: HashMap<RoomId, PendingFile>,
    pub selected_room_id: Option<RoomId>,
    pub draft_by_room: HashMap<RoomId, String>,
    pub forward_room_id: Option<RoomId>,
    pub forward_selected_message_ids: Vec<String>,
    pub forward_target_picker_open: bool,
    pub forward_target_search_query: String,
    pub room_search_query: String,
    pub requested_rooms: HashSet<RoomId>,
    pub socket_state: SocketState,
    pub auth_state: AuthState,
    pub online_data: OnlineData,
    pub last_error: Option<String>,
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
    /// 需要滚动到的目标消息 ID
    pub scroll_to_message_id: Option<String>,
}

impl BridgeState {
    pub fn new(bridge_key: String) -> Self {
        Self {
            bridge_key,
            rooms: Vec::new(),
            messages_by_room: HashMap::new(),
            message_scroll_to_bottom: HashSet::new(),
            pending_message_scroll_to_bottom: HashSet::new(),
            pending_send_scroll_to_bottom: HashSet::new(),
            join_requests: Vec::new(),
            reply_to_by_room: HashMap::new(),
            pending_image_by_room: HashMap::new(),
            pending_file_by_room: HashMap::new(),
            selected_room_id: None,
            draft_by_room: HashMap::new(),
            forward_room_id: None,
            forward_selected_message_ids: Vec::new(),
            forward_target_picker_open: false,
            forward_target_search_query: String::new(),
            room_search_query: String::new(),
            requested_rooms: HashSet::new(),
            socket_state: SocketState::Connecting,
            auth_state: AuthState::Unknown,
            online_data: OnlineData::default(),
            last_error: None,
            last_event: None,
            loading_older_messages: HashSet::new(),
            no_more_history: HashSet::new(),
            prepend_scroll_fix: HashSet::new(),
            last_content_height: HashMap::new(),
            message_scroll_offsets: HashMap::new(),
            scroll_to_message_id: None,
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
        room.last_message.timestamp = Some(message.time.format("%H:%M:%S").to_string());
        room.utime = message.time.timestamp_millis();
    }

    pub fn upsert_message(&mut self, room_id: RoomId, message: Message) {
        let messages = self.messages_by_room.entry(room_id).or_default();
        if let Some(existing) = messages
            .iter_mut()
            .find(|item| item.msg_id == message.msg_id)
        {
            *existing = message;
        } else {
            messages.push(message);
        }
    }

    pub fn mark_message_deleted(&mut self, msg_id: &str) {
        for messages in self.messages_by_room.values_mut() {
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.deleted = true;
                message.reveal = false;
                break;
            }
        }
    }

    pub fn mark_message_hidden(&mut self, msg_id: &str) {
        for messages in self.messages_by_room.values_mut() {
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = true;
                message.reveal = false;
                break;
            }
        }
    }

    pub fn mark_message_revealed(&mut self, msg_id: &str) {
        for messages in self.messages_by_room.values_mut() {
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = false;
                message.reveal = true;
                break;
            }
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
            .sort_by(|lhs, rhs| rhs.time.cmp(&lhs.time));
    }

    pub fn replace_join_requests(&mut self, mut requests: Vec<JoinRequestRoom>) {
        requests.sort_by(|lhs, rhs| rhs.time.cmp(&lhs.time));
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
