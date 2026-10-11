//! GUI 与 IO 线程之间的命令契约，以及命令相关的共享工具。
//!
//! 原生客户端把界面操作收敛成 `IcaCommand`，经 unbounded channel 交给运行中的
//! `run_bridge` 任务处理；本模块定义：
//! - `IcaCommand`：一条命令的完整参数（含请求 id 与回传 oneshot）；
//! - `BridgeHandle`：GUI 侧持有并发送命令的句柄，隐藏 channel 细节；
//! - `ConnectionSignal`：从事件回调回流到主循环的内部控制信号；
//! - `NoticerSendPayload` / `NoticerImage`：内建 Noticer 发送请求的载荷；
//! - `emit_ui_event` / `payload_to_json` / `json_preview` / `reconnect_delay`：
//!   事件回传与 socket 载荷处理的公共函数。
//!
//! 命令一旦入队就由 IO 线程独占执行，GUI 线程不直接接触 socket。

use super::event::BridgeEvent;
use crate::ica::types::{
    RoomId,
    announcement::GroupAnnouncementDraft,
    message::{DeleteMessage, Mention, ReplyMessage, SendMessage},
    room::Room,
};
use rust_socketio::Payload;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// icalingua 客户端的兼容版本号
///
/// 与 Bridge 的 `protocolVersion` 比对，不一致时由 `client::sign_with_key` 上报
/// `bridgeProtocolMismatch`，是否继续连接取决于配置里的容错开关。
pub const ICA_PROTOCOL_VERSION: &str = "2.26.7";
/// 群禁言时长上限：30 天（秒）。
///
/// Bridge 侧对超过该时长的请求会拒绝，GUI 需要在提交前就按此钳制。
pub const GROUP_BAN_MAX_DURATION: u64 = 30 * 24 * 60 * 60;
/// 自动重连最多尝试 5 次。
///
/// 连接失败与已连接后断线共用这一上限，用尽后发 `socketReconnectExhausted` 并结束任务。
pub const MAX_RECONNECT_ATTEMPTS: usize = 5;
/// 指数退避的等待时间上限，避免失败时越等越久。
///
/// 与 `reconnect_delay` 配合：退避到 32 秒后不再继续翻倍。
const MAX_RECONNECT_BACKOFF_SECS: u64 = 30;

/// 从事件 callback 回流到 `run_bridge` 主循环的内部控制信号。
///
/// 与 `BridgeEvent` 分开：这些信号不面向 GUI，只用于驱动主循环的状态机。
#[derive(Debug, Clone, Copy)]
pub enum ConnectionSignal {
    /// 主连接已断开，主循环应退出内层循环并安排重连。
    Disconnected,
}

/// GUI 侧发起、由 IO 线程执行的一条命令。
///
/// 约定：
/// - 带 `request_id` 的变体用于需要把结果对应回请求方的查询类操作；
/// - 带 `oneshot::Sender` 的变体用于 GUI 需要同步等待结果的调用；
/// - 其余变体是"发出去即可"的写入类操作，结果通过 `commandFailed` 等事件异步回报。
///
/// 新增变体时必须在 `handler::handle_command` 中补上分支（那里是穷尽匹配）。
#[derive(Debug)]
pub enum IcaCommand {
    /// 发送语音消息：`audio_data` 为录音器产出的 WAV 字节。
    SendVoiceMessage {
        /// GUI 侧生成的请求 id，用于把 `voiceSendResult` 对应回调用处。
        request_id: u64,
        /// 目标会话；负数表示群聊，正数表示私聊。
        room_id: RoomId,
        /// WAV 音频原始字节。
        audio_data: std::sync::Arc<[u8]>,
    },
    /// 通过当前消息所属 Bridge 刷新 QQ NT 图片地址。
    RefreshImageUrl {
        /// 图片 file id（QQ NT 的 fileId）。
        file_id: String,
        /// 图片 app id，与 file_id 共同决定取回地址的接口。
        app_id: String,
        /// 结果回传通道：成功为新的图片 URL，失败为可展示的错误文本。
        result_tx: oneshot::Sender<Result<String, String>>,
    },
    /// 拉取会话已有消息（首次进入房间时的本地数据加载）。
    FetchMessages(RoomId),
    /// 从 QQ/协议端拉取指定会话的最新漫游历史，而不只是读取 bridge 本地数据库。
    FetchLatestHistory {
        /// 目标会话。
        room_id: RoomId,
        /// 当前已加载的消息条数，Bridge 据此决定补齐哪些新消息。
        current_loaded_messages: usize,
    },
    /// 按 Bridge 的时间与消息 ID 游标加载更旧的历史消息。
    FetchOlderMessages {
        /// 目标会话。
        room_id: RoomId,
        /// 游标时间（毫秒时间戳），只取早于它的消息。
        before_time: i64,
        /// 游标消息 id，与 `before_time` 共同定位分页起点。
        before_id: String,
    },
    /// 拉取群成员列表。
    FetchGroupMembers {
        /// 目标群（负数群号）。
        room_id: RoomId,
    },
    /// 拉取群公告列表。
    ///
    /// Bridge 没有公告接口，实际是取 `qun.qq.com` 的 Cookie 后直连 QQ 的 Web CGI；
    /// `bkn` 由 GUI 侧从 `onlineData` 带入，为 0 或负数时在 IO 线程按 Cookie 现算。
    FetchGroupAnnouncements {
        /// 请求 id，用于把响应对应回界面。
        request_id: u64,
        /// 目标群（负数群号）。
        room_id: RoomId,
        /// QQ Web 接口的 CSRF 令牌；0 或负数表示需要在 IO 线程现算。
        bkn: i64,
    },
    /// 发布或编辑群公告；草稿带 fid 即为编辑已有公告。
    PublishGroupAnnouncement {
        /// 请求 id，用于把操作结果对应回界面。
        request_id: u64,
        /// 目标群（负数群号）。
        room_id: RoomId,
        /// QQ Web 接口的 CSRF 令牌。
        bkn: i64,
        /// 公告草稿；带 fid 时是编辑，否则是新建。
        draft: GroupAnnouncementDraft,
    },
    /// 删除指定群公告。
    DeleteGroupAnnouncement {
        /// 请求 id，用于把操作结果对应回界面。
        request_id: u64,
        /// 目标群（负数群号）。
        room_id: RoomId,
        /// QQ Web 接口的 CSRF 令牌。
        bkn: i64,
        /// 要删除的公告 fid。
        fid: String,
    },
    /// 按发送者检索会话内的历史消息。
    FetchMessagesBySender {
        /// 请求 id，用于把检索结果对应回界面。
        request_id: u64,
        /// 目标会话。
        room_id: RoomId,
        /// 发送者 QQ 号。
        sender_id: i64,
        /// 分页偏移（跳过多少条）。
        offset: usize,
        /// 检索快照时间，用于让结果稳定对应某一次查询。
        snapshot_time: i64,
    },
    /// 对群成员禁言。
    SetGroupBan {
        /// 目标群（负数群号）。
        room_id: RoomId,
        /// 被禁言的成员 QQ 号。
        target_id: i64,
        /// 禁言时长（秒），上限见 `GROUP_BAN_MAX_DURATION`。
        duration: u64,
    },
    /// 拉取系统消息（好友申请、群邀请等）。
    GetSystemMsg,
    /// 拉取联系人列表（好友与群）。
    FetchContacts {
        /// 请求 id，用于把分片响应对应回界面。
        request_id: u64,
    },
    /// 新增一个会话（本地 room 元数据写入 Bridge）。
    AddRoom(Room),
    /// 置顶或取消置顶会话。
    PinRoom {
        /// 目标会话。
        room_id: RoomId,
        /// `true` 置顶，`false` 取消置顶。
        pin: bool,
    },
    /// 删除会话。
    RemoveChat(RoomId),
    /// 忽略会话（不再接收其消息）。
    IgnoreChat {
        /// 目标会话。
        room_id: RoomId,
        /// 会话名称，用于在忽略列表里展示。
        room_name: String,
    },
    /// 取消忽略会话。
    RemoveIgnoredChat(RoomId),
    /// 设置会话排序优先级。
    SetRoomPriority {
        /// 目标会话。
        room_id: RoomId,
        /// 优先级数值，越大越靠前。
        priority: u8,
    },
    /// 持久化清除会话未读数和 @ 标记。
    ClearRoomUnread {
        /// 目标会话。
        room_id: RoomId,
    },
    /// 上报已读位置。
    ReportRead {
        /// 目标会话。
        room_id: RoomId,
        /// 已读到的消息 id。
        message_id: String,
    },
    /// 设置在线状态（对应 oicq 的 `setOnlineStatus`）。
    SetOnlineStatus(u8),
    /// 发起群签到。
    SendGroupSign {
        /// 目标群（负数群号）。
        room_id: RoomId,
    },
    /// 对群成员戳一戳。
    SendGroupPoke {
        /// 目标群（负数群号）。
        room_id: RoomId,
        /// 被戳的成员 QQ 号。
        target_id: i64,
    },
    /// 中止正在进行的漫游历史拉取。
    StopFetchingHistory,
    /// 隐藏一条消息。
    HideMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 目标消息 id。
        message_id: String,
    },
    /// 恢复一条被隐藏的消息。
    RevealMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 目标消息 id。
        message_id: String,
    },
    /// 发送一条消息。
    SendMessage(SendMessage),
    /// 新消息经 HTTP 202 接收后才请求撤回旧消息；禁止自动重试。
    EditAndResendMessage {
        /// 携带原消息媒体、并追加了新图片的待发送消息。
        message: SendMessage,
        /// 需要追加的图片 `(MIME, 字节)`。
        images: Vec<(String, std::sync::Arc<[u8]>)>,
        /// 新消息被接收后要撤回的旧消息 id。
        message_id: String,
    },
    /// 在后台编码单张图片后发送，避免在 GUI 线程生成大型 Base64 字符串。
    SendImageMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 文字内容。
        content: String,
        /// 回复目标。
        reply_to: Option<ReplyMessage>,
        /// @ 列表。
        mentions: Vec<Mention>,
        /// 图片 MIME 类型。
        image_type: String,
        /// 图片原始字节。
        image_data: std::sync::Arc<[u8]>,
    },
    /// 在后台将多张图片编码为 raw 消息链后一次发送，避免阻塞 GUI 线程。
    SendMultiImageMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 文字内容。
        content: String,
        /// 回复目标。
        reply_to: Option<ReplyMessage>,
        /// @ 列表。
        mentions: Vec<Mention>,
        /// 多张图片的 `(MIME, 字节)`。
        images: Vec<(String, std::sync::Arc<[u8]>)>,
    },
    /// 由内建 Noticer 发起并等待本地 Bridge 提交结果的发送请求。
    SendNoticerMessage {
        /// Noticer 的发送载荷（内容、图片与是否按表情发送）。
        payload: NoticerSendPayload,
        /// 结果回传通道：成功表示 Bridge 已接收，失败为错误文本。
        result_tx: oneshot::Sender<Result<(), String>>,
    },
    /// 发送 raw 消息：`content` 由 GUI 直接给出，客户端不解释其结构。
    SendRawMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 原始内容，按字符串塞进 `sendMessage` 的 `content` 字段。
        content: JsonValue,
    },
    /// 在会话内按关键词检索消息。
    SearchMessages {
        /// 目标会话。
        room_id: RoomId,
        /// 关键词。
        keyword: String,
        /// 分页偏移（跳过多少条）。
        offset: usize,
    },
    /// 拉取合并转发消息的节点内容。
    FetchForwardMessages {
        /// 请求 id，用于把响应对应回界面。
        request_id: u64,
        /// 转发资源 id。
        res_id: String,
        /// 转发文件名，部分协议端需要它才能定位资源。
        file_name: Option<String>,
        /// 取不到 `res_id` 时使用的备用资源 id。
        fallback_res_id: Option<String>,
    },
    /// 发送合并转发消息。
    SendMergedForward {
        /// 转发节点列表（已按 Bridge 约定构造）。
        nodes: Vec<JsonValue>,
        /// 是否以私聊形式直接发出（而不是转发卡片）。
        direct_message: bool,
        /// 转发来源会话，部分实现需要它来拼接展示。
        origin: Option<i64>,
        /// 目标会话。
        target_room_id: RoomId,
    },
    /// 直接调用 Bridge 的任意 socket 事件，供插件/调试使用。
    SocketApiCall {
        /// 事件名。
        event: String,
        /// 事件参数。
        args: Vec<JsonValue>,
        /// 是否需要等待 ACK 并把结果回传 GUI。
        expect_ack: bool,
    },
    /// 调用 fileMgr 连接上的文件管理事件（群文件、目录操作等）。
    FileManagerCall {
        /// 目标群号（正数）。
        gin: i64,
        /// fileMgr 事件名。
        event: String,
        /// 事件参数。
        args: Vec<JsonValue>,
        /// 是否需要等待 ACK 并把结果回传 GUI。
        expect_ack: bool,
    },
    /// 上传到群文件目录；数据分块传给 bridge 后由其提交到 QQ 群文件。
    UploadGroupFile {
        /// 目标群号（正数）。
        group_id: i64,
        /// 目标目录 id，空字符串表示群文件根目录。
        parent_id: String,
        /// 文件名（含扩展名）。
        file_name: String,
        /// 文件内容。
        file_data: std::sync::Arc<[u8]>,
    },
    /// 分块上传文件后发送消息
    SendFileMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 文字内容。
        content: String,
        /// 回复目标。
        reply_to: Option<ReplyMessage>,
        /// @ 列表。
        mentions: Vec<Mention>,
        /// 文件名（含扩展名）。
        file_name: String,
        /// 文件类型标识（Bridge 侧据此判定展示方式）。
        file_type: String,
        /// 文件内容。
        file_data: std::sync::Arc<[u8]>,
    },
    /// 删除一条消息。
    DeleteMessage(DeleteMessage),
    /// 重新拉取一条消息的内容（用于刷新可能已变化的图片地址等）。
    RenewMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 目标消息 id。
        message_id: String,
    },
    /// 处理好友申请或群邀请。
    HandleRequest {
        /// 申请类型（friend / group）。
        request_type: String,
        /// 申请 flag，Bridge 用它定位待处理请求。
        flag: String,
        /// `true` 同意，`false` 拒绝。
        accept: bool,
    },
    /// 新建会话分组。
    AddChatGroup {
        /// 分组名。
        name: String,
        /// 归入该分组的会话。
        rooms: Vec<RoomId>,
        /// 是否自动包含全部私聊会话。
        include_all_personal: bool,
    },
    /// 删除会话分组。
    RemoveChatGroup {
        /// 分组名。
        name: String,
    },
    /// 更新会话分组的成员构成。
    UpdateChatGroup {
        /// 分组名。
        name: String,
        /// 该分组应包含的会话。
        rooms: Vec<RoomId>,
        /// 是否自动包含全部私聊会话。
        include_all_personal: bool,
    },
}

/// 内建 Noticer 的一条待发送消息。
///
/// 由 Noticer 侧构造后经 `IcaCommand::SendNoticerMessage` 进入 IO 线程；
/// 图片以字节形式传递，避免跨线程传递临时文件路径。
#[derive(Debug, Clone)]
pub struct NoticerSendPayload {
    /// 目标会话；负数群聊，正数私聊。
    pub room_id: RoomId,
    /// 文字内容。
    pub content: String,
    /// 随消息发送的图片列表。
    pub images: Vec<NoticerImage>,
    /// 是否按表情（sticker）发送：为 `true` 时只取第一张图片。
    pub as_sticker: bool,
}

/// Noticer 消息中的一张图片。
#[derive(Debug, Clone)]
pub struct NoticerImage {
    /// 图片 MIME 类型。
    pub mime: String,
    /// 图片原始字节。
    pub data: std::sync::Arc<[u8]>,
}

/// GUI 侧持有的一条 bridge 命令通道。
///
/// 每个 bridge 一个实例：`bridge_key` 用于在日志与事件里标识归属，
/// `command_tx` 指向该 bridge 的 `run_bridge` 任务。克隆句柄只会克隆发送端，
/// 不会新建连接。
#[derive(Debug, Clone)]
pub struct BridgeHandle {
    /// 该句柄所属 bridge 的标识（配置名，未配置时退回 url）。
    bridge_key: String,
    /// 命令通道发送端。
    command_tx: UnboundedSender<IcaCommand>,
}

impl BridgeHandle {
    /// 用 bridge 标识与命令发送端构造句柄。
    pub fn new(bridge_key: String, command_tx: UnboundedSender<IcaCommand>) -> Self {
        Self {
            bridge_key,
            command_tx,
        }
    }

    /// 返回该句柄归属的 bridge 标识。
    pub fn bridge_key(&self) -> &str {
        &self.bridge_key
    }

    /// 发送一条命令。
    ///
    /// 只在 IO 任务已经退出（channel 关闭）时返回 `Err`；入队成功不代表命令执行成功，
    /// 执行结果通过 `commandFailed` 等事件异步回报。
    pub fn send(&self, command: IcaCommand) -> Result<(), String> {
        self.command_tx
            .send(command)
            .map_err(|error| error.to_string())
    }

    /// 克隆命令发送端，供需要长期持有通道的组件（如图片加载、Noticer）使用。
    pub fn command_sender(&self) -> UnboundedSender<IcaCommand> {
        self.command_tx.clone()
    }
}

/// 把一条协议事件转发给 GUI，channel 未建立时退化为日志。
///
/// `event_tx` 为 `None` 表示当前没有 GUI 消费者（例如尚未启动界面），
/// 此时只打印日志，避免事件被静默丢弃而无法排查。
pub fn emit_ui_event(
    tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_id: &str,
    event_name: &'static str,
    payload: JsonValue,
) {
    let event = BridgeEvent::from_protocol(bridge_id, event_name, payload);

    if let Some(tx) = tx {
        // 接收端已退出时忽略发送错误：此时界面已经不需要这些事件。
        let _ = tx.send(event);
    } else {
        tracing::info!("{}: {:?}", event_name, event);
    }
}

/// 把 socket 载荷统一转成 JSON。
///
/// 文本载荷保持数组形状；二进制载荷按字节数组存放，与 Bridge 的二进制事件约定一致；
/// 其余类型（如空载荷）归一为 `Null`，让上层可以用同一种方式取值。
pub fn payload_to_json(payload: &Payload) -> JsonValue {
    match payload {
        Payload::Text(values) => JsonValue::Array(values.clone()),
        Payload::Binary(bytes) => json!(bytes.to_vec()),
        _ => JsonValue::Null,
    }
}

/// 截取 JSON 的文本预览，用于日志。
///
/// 按字符而非字节截断，避免把多字节字符切开产生乱码；超长时追加 `...`。
pub fn json_preview(value: &JsonValue, max_chars: usize) -> String {
    let raw = value.to_string();
    let mut chars = raw.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

/// 计算第 `attempt` 次重连前的等待时间（指数退避）。
///
/// `attempt` 从 1 开始计数，退避序列为 1、2、4、8、16、32 秒，之后保持 32 秒；
/// `attempt` 为 0 或极大值时都不会溢出或 panic。
pub fn reconnect_delay(attempt: usize) -> Duration {
    let exp = attempt.saturating_sub(1).min(5) as u32;
    let seconds = (1_u64 << exp).min(MAX_RECONNECT_BACKOFF_SECS);
    Duration::from_secs(seconds)
}
