//! 聊天界面的一次性交互意图：滚动目标、消息操作与待发送附件。
//!
//! 这些类型都由 UI 层产生、由主循环消费，放在一起便于对照。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ica::types::{
    RoomId,
    message::{Message, ReplyMessage},
};

use super::super::media::ImageAction;

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
