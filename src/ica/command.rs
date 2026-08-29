use rust_socketio::Payload;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use super::event::BridgeEvent;

use crate::ica::types::{
    RoomId,
    message::{DeleteMessage, Mention, ReplyMessage, SendMessage},
    room::Room,
};

/// icalingua 客户端的兼容版本号
pub const ICA_PROTOCOL_VERSION: &str = "2.26.2";
pub const GROUP_BAN_MAX_DURATION: u64 = 30 * 24 * 60 * 60;
/// 自动重连最多尝试 5 次。
pub(super) const MAX_RECONNECT_ATTEMPTS: usize = 5;
/// 指数退避的等待时间上限，避免失败时越等越久。
const MAX_RECONNECT_BACKOFF_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
pub(super) enum ConnectionSignal {
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum IcaCommand {
    FetchMessages(RoomId),
    /// 从 QQ/协议端拉取指定会话的最新漫游历史，而不只是读取 bridge 本地数据库。
    FetchLatestHistory {
        room_id: RoomId,
        current_loaded_messages: usize,
    },
    /// 加载更旧的历史消息（带 offset）
    FetchOlderMessages {
        room_id: RoomId,
        offset: usize,
    },
    FetchGroupMembers {
        room_id: RoomId,
    },
    FetchMessagesBySender {
        request_id: u64,
        room_id: RoomId,
        sender_id: i64,
        offset: usize,
    },
    SetGroupBan {
        room_id: RoomId,
        target_id: i64,
        duration: u64,
    },
    GetSystemMsg,
    FetchContacts {
        request_id: u64,
    },
    AddRoom(Room),
    PinRoom {
        room_id: RoomId,
        pin: bool,
    },
    RemoveChat(RoomId),
    IgnoreChat {
        room_id: RoomId,
        room_name: String,
    },
    RemoveIgnoredChat(RoomId),
    SetRoomPriority {
        room_id: RoomId,
        priority: u8,
    },
    ReportRead {
        room_id: RoomId,
        message_id: String,
    },
    SetOnlineStatus(u8),
    SendGroupSign {
        room_id: RoomId,
    },
    SendGroupPoke {
        room_id: RoomId,
        target_id: i64,
    },
    StopFetchingHistory,
    HideMessage {
        room_id: RoomId,
        message_id: String,
    },
    RevealMessage {
        room_id: RoomId,
        message_id: String,
    },
    SendMessage(SendMessage),
    /// 在后台编码单张图片后发送，避免在 GUI 线程生成大型 Base64 字符串。
    SendImageMessage {
        room_id: RoomId,
        content: String,
        reply_to: Option<ReplyMessage>,
        mentions: Vec<Mention>,
        image_type: String,
        image_data: std::sync::Arc<[u8]>,
    },
    /// 在后台将多张图片编码为 raw 消息链后一次发送，避免阻塞 GUI 线程。
    SendMultiImageMessage {
        room_id: RoomId,
        content: String,
        reply_to: Option<ReplyMessage>,
        mentions: Vec<Mention>,
        images: Vec<(String, std::sync::Arc<[u8]>)>,
    },
    SendRawMessage {
        room_id: RoomId,
        content: JsonValue,
    },
    SearchMessages {
        room_id: RoomId,
        keyword: String,
        offset: usize,
    },
    FetchForwardMessages {
        request_id: u64,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
    },
    SendMergedForward {
        nodes: Vec<JsonValue>,
        direct_message: bool,
        origin: Option<i64>,
        target_room_id: RoomId,
    },
    SocketApiCall {
        event: String,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    FileManagerCall {
        gin: i64,
        event: String,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    /// 上传到群文件目录；数据分块传给 bridge 后由其提交到 QQ 群文件。
    UploadGroupFile {
        group_id: i64,
        parent_id: String,
        file_name: String,
        file_data: std::sync::Arc<[u8]>,
    },
    /// 分块上传文件后发送消息
    SendFileMessage {
        room_id: RoomId,
        content: String,
        reply_to: Option<ReplyMessage>,
        mentions: Vec<Mention>,
        file_name: String,
        file_type: String,
        file_data: std::sync::Arc<[u8]>,
    },
    DeleteMessage(DeleteMessage),
    RenewMessage {
        room_id: RoomId,
        message_id: String,
    },
    HandleRequest {
        request_type: String,
        flag: String,
        accept: bool,
    },
    AddChatGroup {
        name: String,
        rooms: Vec<RoomId>,
        include_all_personal: bool,
    },
    RemoveChatGroup {
        name: String,
    },
    UpdateChatGroup {
        name: String,
        rooms: Vec<RoomId>,
        include_all_personal: bool,
    },
}

#[derive(Debug, Clone)]
pub struct BridgeHandle {
    bridge_key: String,
    command_tx: UnboundedSender<IcaCommand>,
}

impl BridgeHandle {
    pub fn new(bridge_key: String, command_tx: UnboundedSender<IcaCommand>) -> Self {
        Self {
            bridge_key,
            command_tx,
        }
    }

    pub fn bridge_key(&self) -> &str {
        &self.bridge_key
    }

    pub fn send(&self, command: IcaCommand) -> Result<(), String> {
        self.command_tx
            .send(command)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn command_sender(&self) -> UnboundedSender<IcaCommand> {
        self.command_tx.clone()
    }
}

pub(super) fn emit_ui_event(
    tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_id: &str,
    event_name: &'static str,
    payload: JsonValue,
) {
    let event = BridgeEvent::from_protocol(bridge_id, event_name, payload);

    if let Some(tx) = tx {
        let _ = tx.send(event);
    } else {
        tracing::info!("{}: {:?}", event_name, event);
    }
}

pub(super) fn payload_to_json(payload: &Payload) -> JsonValue {
    match payload {
        Payload::Text(values) => JsonValue::Array(values.clone()),
        Payload::Binary(bytes) => json!(bytes.to_vec()),
        _ => JsonValue::Null,
    }
}

pub(super) fn json_preview(value: &JsonValue, max_chars: usize) -> String {
    let raw = value.to_string();
    let mut chars = raw.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

pub(super) fn reconnect_delay(attempt: usize) -> Duration {
    let exp = attempt.saturating_sub(1).min(5) as u32;
    let seconds = (1_u64 << exp).min(MAX_RECONNECT_BACKOFF_SECS);
    Duration::from_secs(seconds)
}
