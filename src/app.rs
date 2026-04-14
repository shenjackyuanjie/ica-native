use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    sync::Arc,
};

use eframe::CreationContext;
use eframe::glow;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use serde_json::Value as JsonValue;

use crate::cfg;
use crate::ica;
use crate::{
    assets,
    ica::{IcaClient, IcaCommand},
};

pub mod chat_groups;
pub mod config_editer;
pub mod custom_chat;
pub mod online_mode;
pub mod open_page;
pub mod renders;

use chat_groups::ChatGroups;
use config_editer::ConfigEditer;
use custom_chat::CustomChat;
use online_mode::OnlineMode;
use open_page::AppOpenPage;

use crate::ica::types::{
    RoomId,
    message::{Message, NewMessage, SendMessage},
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};

#[derive(Debug, Default, Clone, Copy)]
pub enum ChatListScrollTarget {
    #[default]
    None,
    Top,
    Bottom,
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
    pub join_requests: Vec<JoinRequestRoom>,
    pub selected_room_id: Option<RoomId>,
    pub draft_by_room: HashMap<RoomId, String>,
    pub requested_rooms: HashSet<RoomId>,
    pub socket_state: SocketState,
    pub auth_state: AuthState,
    pub online_data: OnlineData,
    pub last_error: Option<String>,
    pub last_event: Option<String>,
}

impl BridgeState {
    pub fn new(bridge_key: String) -> Self {
        Self {
            bridge_key,
            rooms: Vec::new(),
            messages_by_room: HashMap::new(),
            join_requests: Vec::new(),
            selected_room_id: None,
            draft_by_room: HashMap::new(),
            requested_rooms: HashSet::new(),
            socket_state: SocketState::Connecting,
            auth_state: AuthState::Unknown,
            online_data: OnlineData::default(),
            last_error: None,
            last_event: None,
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
    }

    pub fn upsert_message(&mut self, room_id: RoomId, message: Message) {
        let messages = self.messages_by_room.entry(room_id).or_default();
        if let Some(existing) = messages.iter_mut().find(|item| item.msg_id == message.msg_id) {
            *existing = message;
        } else {
            messages.push(message);
        }
    }

    pub fn mark_message_deleted(&mut self, msg_id: &str) {
        for messages in self.messages_by_room.values_mut() {
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.deleted = true;
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
    }
}

pub struct IcaApp {
    /// 聊天界面定制选项
    pub custom_chat: CustomChat,
    /// 在线模式
    pub online_mode: OnlineMode,
    /// 打开了什么页面
    pub open_page: AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
    /// 是否选中某个聊天组
    pub chat_group_selected: bool,
    /// 选中了哪个聊天组
    pub chat_group_idx: usize,
    /// 聊天组
    pub chat_groups: ChatGroups,
    /// 配置文件修改
    pub config_editer: ConfigEditer,
    /// 聊天列表滚动目标
    pub chat_list_scroll_target: ChatListScrollTarget,
    /// 当前选中的 bridge
    pub active_bridge_idx: Option<usize>,
    /// 每个 bridge 的界面状态
    pub bridge_states: Vec<BridgeState>,
    /// tokio rt
    /// 用来开 socketio
    pub runtime: Runtime,
    /// Socketio 列表
    /// 一些 Socketio 连接
    pub ica_clients: Vec<IcaClient>,
    /// GUI 侧接收事件的 channel
    pub ui_rx: UnboundedReceiver<JsonValue>,
    /// 发送事件到 GUI 的 channel
    pub ui_tx: UnboundedSender<JsonValue>,
    /// Socketio 停止信号
    pub socketio_stop_senders: Vec<oneshot::Sender<()>>,
}

impl IcaApp {
    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        let font_sy_data = egui::FontData::from_static(assets::fonts::FONT_思源黑体);
        let font_unifont_data = egui::FontData::from_static(assets::fonts::FONT_UNIFONT);

        let sy_font_name = "notosans".to_string();
        let unifont_name = "unifont".to_string();

        fonts
            .font_data
            .insert(sy_font_name.clone(), Arc::new(font_sy_data));

        fonts
            .font_data
            .insert(unifont_name.clone(), Arc::new(font_unifont_data));

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, unifont_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(unifont_name.clone());

        ctx.set_fonts(fonts);
    }

    fn setup_async_rt() -> Runtime {
        let config = crate::cfg::get_cfg_snapshot();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("faild to build tokio rt")
    }

    /// 生成测试用的聊天室数据
    #[allow(unused)]
    fn test_chat_rooms() -> Vec<Room> {
        // 生成随机房间数据
        use rand::Rng;
        use rand::rng;
        use rand::seq::SliceRandom;
        let mut rooms = Vec::with_capacity(50);
        let room_names = vec![
            "测试群聊",
            "开发讨论组",
            "项目协作",
            "闲聊灌水",
            "技术交流",
            "学习小组",
            "游戏开黑",
            "音乐分享",
            "读书会",
            "运动健身",
        ];

        let user_names = vec![
            "张三",
            "李四",
            "王五",
            "赵六",
            "钱七",
            "孙八",
            "周九",
            "吴十",
            "郑十一",
            "王十二",
        ];

        let message_templates = vec![
            "大家好！今天天气不错",
            "有人在线吗？",
            "这个功能什么时候能做完？",
            "晚上一起吃饭吗？",
            "[图片]",
            "我刚刚上传了文件",
            "明天会议几点开始？",
            "这个问题怎么解决？",
            "有人玩{}吗？",
            "推荐一个好看的{}",
        ];

        let mut rng = rng();

        for i in 0..500 {
            let room_name_idx = rng.random_range(0..room_names.len());
            let user_idx = rng.random_range(0..user_names.len());
            let message_idx = rng.random_range(0..message_templates.len());

            // 随机生成消息内容
            let mut message = message_templates[message_idx].to_string();
            if message.contains("{}") {
                let replacements = ["游戏", "电影", "书", "餐厅", "音乐", "软件"];
                let replacement = replacements[rng.random_range(0..replacements.len())];
                message = message.replace("{}", replacement);
            }

            // 随机添加表情或标签
            if rng.random_bool(0.3) {
                message += if rng.random_bool(0.5) {
                    " 😊"
                } else {
                    " #标签"
                };
            }

            rooms.push(Room {
                room_id: if rng.random_bool(0.7) {
                    -rng.random_range(100_000_000..1_000_000_000)
                } else {
                    rng.random_range(100_000_000..1_000_000_000)
                },
                room_name: format!("{} {}", room_names[room_name_idx], rng.random_range(1..100)),
                index: i as i64 + 1,
                unread_count: rng.random_range(0..100),
                priority: rng.random_range(1..4),
                utime: 1700000000 + rng.random_range(0..100000),
                at: match rng.random_range(0..5) {
                    0 => crate::ica::types::message::At::All,
                    1 => crate::ica::types::message::At::Bool(rng.random_bool(0.2)),
                    _ => crate::ica::types::message::At::None,
                },
                last_message: crate::ica::types::message::LastMessage {
                    content: Some(message),
                    timestamp: Some(match rng.random_range(0..4) {
                        0 => "刚刚".to_string(),
                        1 => format!("{}:{}", rng.random_range(0..24), rng.random_range(0..60)),
                        2 => "昨天".to_string(),
                        _ => "前天".to_string(),
                    }),
                    username: Some(user_names[user_idx].to_string()),
                    user_id: Some(rng.random_range(100_000_000..1_000_000_000)),
                },
            });
        }

        // 打乱房间顺序
        rooms.shuffle(&mut rng);
        rooms
    }

    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);

        let (ui_tx, ui_rx) = unbounded_channel::<JsonValue>();

        let config = cfg::get_cfg_snapshot();
        let runtime = Self::setup_async_rt();

        let mut socketio_stop_senders = Vec::new();
        let mut ica_clients = Vec::new();
        let mut bridge_states = Vec::new();

        for bridge in config.bridges.clone() {
            if !bridge.enable {
                continue;
            }
            let (stop_tx, stop_rx) = oneshot::channel();
            socketio_stop_senders.push(stop_tx);

            let bridge_key = if bridge.name.is_empty() {
                bridge.url.clone()
            } else {
                bridge.name.clone()
            };
            let (command_tx, command_rx) = unbounded_channel();
            ica_clients.push(IcaClient {
                bridge_key: bridge_key.clone(),
                command_tx,
            });
            bridge_states.push(BridgeState::new(bridge_key.clone()));

            let ui_tx_clone = ui_tx.clone();
            runtime.spawn(async move {
                if let Err(e) = ica::main(stop_rx, &bridge, Some(ui_tx_clone), command_rx).await {
                    tracing::error!("socketio bridge {} stopped with error: {}", bridge_key, e);
                }
            });
        }

        Self {
            custom_chat: CustomChat::default(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_group_selected: false,
            chat_group_idx: 0,
            chat_groups: ChatGroups::new(),
            config_editer: ConfigEditer::default(),
            chat_list_scroll_target: ChatListScrollTarget::Bottom,
            active_bridge_idx: if bridge_states.is_empty() { None } else { Some(0) },
            bridge_states,
            runtime,
            ica_clients,
            ui_rx,
            ui_tx,
            socketio_stop_senders,
        }
    }

    pub fn active_bridge_state(&self) -> Option<&BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get(idx))
    }

    pub fn active_bridge_state_mut(&mut self) -> Option<&mut BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get_mut(idx))
    }

    fn bridge_state_mut(&mut self, bridge_key: &str) -> Option<&mut BridgeState> {
        self.bridge_states
            .iter_mut()
            .find(|state| state.bridge_key == bridge_key)
    }

    /// socket.io 的事件 payload 基本都是数组包装，真正的数据通常在第一个元素里。
    fn first_payload_value(payload: &JsonValue) -> Option<&JsonValue> {
        payload.as_array().and_then(|values| values.first())
    }

    /// 统一提取事件里常见的 `message` 字段，避免每个分支都手动抄一遍。
    fn payload_message(payload: &JsonValue) -> Option<String> {
        payload
            .get("message")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
    }

    /// 把某个 bridge 发来的事件应用到对应的本地状态上。
    ///
    /// 这里故意不做 UI 逻辑，只做“事件 -> 状态”的映射，方便后续继续补事件类型。
    fn apply_socketio_event(state: &mut BridgeState, event_name: &str, payload: &JsonValue) {
        match event_name {
            "socketConnecting" => {
                state.socket_state = SocketState::Connecting;
                state.auth_state = AuthState::Unknown;
                state.last_error = None;
            }
            "socketReconnecting" => {
                state.socket_state = SocketState::Connecting;
                state.last_error = Self::payload_message(payload);
            }
            "socketConnected" => {
                state.socket_state = SocketState::Connected;
                state.last_error = None;
            }
            "socketDisconnected" => {
                state.socket_state = SocketState::Disconnected;
                state.last_error = Self::payload_message(payload);
            }
            "socketConnectFailed" => {
                state.socket_state = SocketState::Failed;
                state.last_error = Self::payload_message(payload);
            }
            "socketRetryScheduled" => {
                state.socket_state = SocketState::Connecting;
                state.last_error = Self::payload_message(payload);
            }
            "socketReconnectExhausted" => {
                state.socket_state = SocketState::Failed;
                state.last_error = Self::payload_message(payload);
            }
            "requireAuth" => {
                state.auth_state = AuthState::Pending;
            }
            "authSucceed" => {
                state.auth_state = AuthState::Succeeded;
                state.last_error = None;
            }
            "authFailed" => {
                state.auth_state = AuthState::Failed;
                state.last_error = Some("bridge 认证失败".to_string());
            }
            "onlineData" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    state.online_data = OnlineData::new_from_json(value);
                }
            }
            "setAllRooms" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<Vec<Room>>(value.clone()) {
                        Ok(rooms) => state.rooms = rooms,
                        Err(e) => {
                            state.last_error = Some(format!("setAllRooms 解析失败: {}", e));
                        }
                    }
                }
            }
            "setMessages" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    match serde_json::from_value::<Vec<Message>>(value["messages"].clone()) {
                        Ok(messages) => {
                            state.requested_rooms.insert(room_id);
                            state.messages_by_room.insert(room_id, messages);
                        }
                        Err(e) => {
                            state.last_error = Some(format!("setMessages 解析失败: {}", e));
                        }
                    }
                }
            }
            "addMessage" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<NewMessage>(value.clone()) {
                        Ok(new_message) => {
                            let room_id = new_message.room_id;
                            state.requested_rooms.insert(room_id);
                            state.sync_room_preview(room_id, &new_message.msg);
                            state.upsert_message(room_id, new_message.msg);
                        }
                        Err(e) => {
                            state.last_error = Some(format!("addMessage 解析失败: {}", e));
                        }
                    }
                }
            }
            "deleteMessage" => {
                if let Some(msg_id) = Self::first_payload_value(payload).and_then(|value| value.as_str()) {
                    state.mark_message_deleted(msg_id);
                }
            }
            "handleRequest" => {
                if let Some(value) = Self::first_payload_value(payload) {
                    match serde_json::from_value::<JoinRequestRoom>(value.clone()) {
                        Ok(request) => {
                            state.upsert_join_request(request);
                        }
                        Err(e) => {
                            state.last_error = Some(format!("handleRequest 解析失败: {}", e));
                        }
                    }
                }
            }
            "commandFailed" => {
                state.last_error = Self::payload_message(payload);
            }
            _ => {}
        }
    }

    pub fn request_room_messages(&self, bridge_idx: usize, room_id: RoomId) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };

        // 房间历史消息通过 socket 任务去拉，GUI 本身不直接碰底层 client。
        if let Err(e) = client.command_tx.send(IcaCommand::FetchMessages(room_id)) {
            tracing::warn!("send fetchMessages command failed: {}", e);
        }
    }

    pub fn select_active_room(&mut self, room_id: RoomId) {
        let mut should_request = false;
        if let Some(state) = self.active_bridge_state_mut() {
            state.selected_room_id = Some(room_id);
            should_request = state.requested_rooms.insert(room_id);
        }

        if should_request && let Some(bridge_idx) = self.active_bridge_idx {
            self.request_room_messages(bridge_idx, room_id);
        }
    }

    pub fn send_current_message(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let Some(room_id) = state.selected_room_id else {
            return;
        };

        let draft = state.draft_by_room.entry(room_id).or_default();
        let content = draft.trim().to_string();
        if content.is_empty() {
            return;
        }
        draft.clear();

        // 发送动作也统一走命令通道，这样多 bridge 下不会把消息发到错误连接。
        let message = SendMessage::new(content.clone(), room_id, None);
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendMessage(message))
        {
            tracing::warn!("send sendMessage command failed: {}", e);
            state.draft_by_room.insert(room_id, content);
        }
    }

    fn poll_socketio_events(&mut self) {
        while let Ok(event) = self.ui_rx.try_recv() {
            let Some(event_name) = event.get("event").and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(bridge_key) = event.get("bridge").and_then(|value| value.as_str()) else {
                continue;
            };

            let payload = event.get("payload").cloned().unwrap_or(JsonValue::Null);
            let Some(state) = self.bridge_state_mut(bridge_key) else {
                continue;
            };

            state.last_event = Some(event_name.to_string());

            Self::apply_socketio_event(state, event_name, &payload);
        }
    }
}

impl eframe::App for IcaApp {
    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        for sender in self.socketio_stop_senders.drain(..) {
            let _ = sender.send(());
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_socketio_events();

        // 检测 ESC 键取消选择
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.selected_room_id = None;
        }

        // 渲染相关的方法已移到 `renders.rs` 模块
        self.render_top_panel(ctx);
        self.render_left_groups_panel(ctx);
        self.render_chat_list_panel(ctx);
        self.render_central_panel(ctx);
        self.render_windows(ctx);
    }
}
