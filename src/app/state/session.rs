//! 单个 bridge 的会话装配：连接状态、房间目录与命令句柄。
//!
//! `BridgeSession` 把一个 bridge 的命令通道、停止信号与全部 UI 状态捆在
//! 一起，应用层只持有它的列表；`RoomDirectory` 是其中面向界面的大部分数据
//! （连接、房间、验证消息），并通过 `Deref` 把连接状态直接暴露出来。
//! 状态历史（错误/提示）在这里按“观测值变化”入列，界面只读历史列表。

use super::{AuthState, BridgeState, SocketState, VisibleRoomIndicesCache};
use crate::app::SelectedChatGroup;
use crate::config::ChatGroups;
use crate::ica::types::{
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};
use crate::ica::{BridgeHandle, IcaCommand};
use std::ops::{Deref, DerefMut};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// 状态历史保留的条数上限；超出后从最旧一端丢弃。
const STATUS_HISTORY_LIMIT: usize = 100;

/// 状态历史中一条消息的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMessageKind {
    /// 错误。
    Error,
    /// 普通提示/通知。
    Notice,
}

impl StatusMessageKind {
    /// 历史列表分类标签的中文文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "错误",
            Self::Notice => "消息",
        }
    }
}

/// 一条带时间戳的状态历史记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    /// 本地时间戳文本（`YYYY-MM-DD HH:MM:SS`）。
    pub timestamp: String,
    /// 类别（错误/提示）。
    pub kind: StatusMessageKind,
    /// 记录正文。
    pub text: String,
}

/// 单个 bridge 的连接与账号状态。
#[derive(Debug, Clone)]
pub struct ConnectionState {
    /// bridge 配置 key，用于跨模块定位同一条连接。
    pub bridge_key: String,
    /// Socket.IO 连接状态。
    pub socket_state: SocketState,
    /// 登录鉴权状态。
    pub auth_state: AuthState,
    /// Bridge 版本号。
    pub bridge_version: Option<String>,
    /// Bridge 协议版本号。
    pub bridge_protocol_version: Option<String>,
    /// 当前账号的在线信息（QQ 号、昵称、在线状态）。
    pub online_data: OnlineData,
    /// 当前账号是否被全群禁言。
    pub is_shut_up: bool,
    /// 最近一条错误；同步进历史列表后由 `clear_error` 清除。
    pub last_error: Option<String>,
    /// 最近一条提示；同步进历史列表后由 `clear_notice` 清除。
    pub last_notice: Option<String>,
    /// 状态历史（错误与提示各按发生顺序入列）。
    pub status_history: Vec<StatusMessage>,
    /// 已入列的错误内容，用于检测 `last_error` 的新变化。
    observed_error: Option<String>,
    /// 已入列的提示内容，用于检测 `last_notice` 的新变化。
    observed_notice: Option<String>,
    /// 最近一次高级 Socket API 调用的响应原文。
    pub last_socket_api_response: Option<String>,
    /// Bridge 请求登录/初始化的提示内容。
    pub setup_requested: Option<String>,
    /// 致命错误；出现后连接不再可用。
    pub fatal_error: Option<String>,
    /// 最近一次协议事件名，供状态页排查。
    pub last_event: Option<String>,
}

impl ConnectionState {
    /// 以“连接中、未认证”创建一份初始连接状态。
    pub fn new(bridge_key: String) -> Self {
        Self {
            bridge_key,
            socket_state: SocketState::Connecting,
            auth_state: AuthState::Unknown,
            bridge_version: None,
            bridge_protocol_version: None,
            online_data: OnlineData::default(),
            is_shut_up: false,
            last_error: None,
            last_notice: None,
            status_history: Vec::new(),
            observed_error: None,
            observed_notice: None,
            last_socket_api_response: None,
            setup_requested: None,
            fatal_error: None,
            last_event: None,
        }
    }

    /// 把 `last_error` / `last_notice` 的新变化追加进状态历史。
    ///
    /// 用“上一次观测到的内容”做差：同一提示被反复赋值不会重复入列，
    /// 清空（置 `None`）也不会产生历史记录。
    pub fn sync_status_history(&mut self) {
        if self.last_error != self.observed_error {
            self.observed_error = self.last_error.clone();
            if let Some(message) = self.last_error.clone() {
                self.push_status(StatusMessageKind::Error, message);
            }
        }
        if self.last_notice != self.observed_notice {
            self.observed_notice = self.last_notice.clone();
            if let Some(message) = self.last_notice.clone() {
                self.push_status(StatusMessageKind::Notice, message);
            }
        }
    }

    /// 清除当前错误及其观测值（用户在界面上点了关闭）。
    pub fn clear_error(&mut self) {
        self.last_error = None;
        self.observed_error = None;
    }

    /// 清除当前提示及其观测值。
    pub fn clear_notice(&mut self) {
        self.last_notice = None;
        self.observed_notice = None;
    }

    /// 清空整段状态历史。
    pub fn clear_status_history(&mut self) {
        self.status_history.clear();
    }

    /// 追加一条状态历史，超过 `STATUS_HISTORY_LIMIT` 时从最旧一端丢弃。
    fn push_status(&mut self, kind: StatusMessageKind, text: String) {
        self.status_history.push(StatusMessage {
            timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            kind,
            text,
        });
        if self.status_history.len() > STATUS_HISTORY_LIMIT {
            let overflow = self.status_history.len() - STATUS_HISTORY_LIMIT;
            self.status_history.drain(..overflow);
        }
    }
}

/// 单个 bridge 的房间目录：界面直接读取的房间列表与过滤状态。
#[derive(Debug, Clone)]
pub struct RoomDirectory {
    /// 该 bridge 的连接与账号状态。
    pub connection: ConnectionState,
    /// 聊天分组配置（按 bridge 保存，切换 bridge 时整体替换）。
    pub chat_groups: ChatGroups,
    /// 当前选中的聊天分组筛选。
    pub selected_chat_group: SelectedChatGroup,
    /// 房间列表；顺序即会话列表展示顺序（置顶在前）。
    pub rooms: Vec<Room>,
    /// 房间列表修订号；任何变化都递增，用于失效可见房间缓存。
    pub rooms_revision: u64,
    /// 可见房间下标缓存；过滤条件或修订号变化时失效。
    pub visible_room_indices_cache: Option<VisibleRoomIndicesCache>,
    /// 加群/好友验证消息，按时间倒序。
    pub join_requests: Vec<JoinRequestRoom>,
}

impl RoomDirectory {
    /// 用 bridge key 与分组配置创建空目录；默认展示“所有聊天”。
    pub fn new(bridge_key: String, chat_groups: ChatGroups) -> Self {
        Self {
            connection: ConnectionState::new(bridge_key),
            chat_groups,
            selected_chat_group: SelectedChatGroup::All,
            rooms: Vec::new(),
            rooms_revision: 1,
            visible_room_indices_cache: None,
            join_requests: Vec::new(),
        }
    }
}

/// 让房间目录可以直接访问连接状态字段（`socket_state`、`last_error` 等）。
impl Deref for RoomDirectory {
    type Target = ConnectionState;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for RoomDirectory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

/// bridge 命令句柄及属于同一连接的全部状态。
/// 将它们放在一起可以避免多个并行 Vec 之间的索引漂移。
pub struct BridgeSession {
    /// 向 Bridge 发送命令的句柄。
    handle: BridgeHandle,
    /// 该连接的完整 UI 状态。
    state: BridgeState,
    /// 停止信号发送端；`stop` 时取出发送一次。
    stop_sender: Option<oneshot::Sender<()>>,
}

impl BridgeSession {
    /// 用命令句柄、初始状态与停止信号装配一个会话。
    pub fn new(handle: BridgeHandle, state: BridgeState, stop_sender: oneshot::Sender<()>) -> Self {
        Self {
            handle,
            state,
            stop_sender: Some(stop_sender),
        }
    }

    /// 取命令句柄的只读引用。
    pub fn handle(&self) -> &BridgeHandle {
        &self.handle
    }

    /// 取 UI 状态的只读引用。
    pub fn state(&self) -> &BridgeState {
        &self.state
    }

    /// 取 UI 状态的可变引用。
    pub fn state_mut(&mut self) -> &mut BridgeState {
        &mut self.state
    }

    /// 发送一条命令；通道已断开时返回错误文本。
    pub fn send(&self, command: IcaCommand) -> Result<(), String> {
        self.handle.send(command)
    }

    /// 克隆命令发送端（后台任务需要长期持有时使用）。
    pub fn command_sender(&self) -> UnboundedSender<IcaCommand> {
        self.handle.command_sender()
    }

    /// 请求停止该 bridge 的后台任务；重复调用是安全的（只发一次）。
    pub fn stop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
    }
}

/// 让会话对象可以直接访问 `BridgeState` 的字段与方法。
impl Deref for BridgeSession {
    type Target = BridgeState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for BridgeSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}
