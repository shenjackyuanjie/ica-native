//! 聊天界面的一次性交互意图：滚动目标、消息操作与待发送附件。
//!
//! 这些类型都由 UI 层产生、由主循环消费，放在一起便于对照。
//! 共同约定：UI 在 egui 闭包内只记录意图（枚举值或置位标志），不在闭包里
//! 直接改状态或发命令，绘制结束后统一执行，避免借用与重入问题。

use super::super::media::ImageAction;
use crate::ica::types::{
    RoomId,
    message::{Message, ReplyMessage},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 会话列表的滚动目标；一次性请求，消费后即清除。
#[derive(Debug, Default, Clone, Copy)]
pub enum ChatListScrollTarget {
    /// 不滚动。
    #[default]
    None,
    /// 滚动到顶部。
    Top,
    /// 滚动到底部。
    Bottom,
}

/// 右键菜单/头像菜单产生的消息级操作，由主循环分发执行。
#[derive(Debug, Clone)]
pub enum MessageAction {
    /// 回复某条消息。
    Reply {
        /// 目标会话。
        room_id: RoomId,
        /// 被回复消息的摘要。
        reply: ReplyMessage,
    },
    /// 撤回自己发送的消息。
    Delete {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 重新编辑已发送消息（把内容写回输入框）。
    ReEdit {
        /// 目标会话。
        room_id: RoomId,
        /// 待编辑的正文。
        content: String,
    },
    /// 显示/隐藏消息（“不感兴趣”与撤销）。
    SetReveal {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
        /// `true` 为显示，`false` 为隐藏。
        reveal: bool,
    },
    /// 复制消息内容到输入框草稿。
    CopyToDraft {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 对消息发一个 +1。
    PlusOne {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 切换某条消息的转发勾选状态。
    ToggleForwardSelection {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 以某条消息为起点进入多选转发。
    StartForward {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 打开一个合并转发查看器。
    OpenForward {
        /// 合并转发资源 id。
        res_id: String,
        /// 合并转发标题。
        file_name: Option<String>,
        /// 拉取失败时的备用资源 id。
        fallback_res_id: Option<String>,
        /// 已随消息下发的内嵌内容。
        inline_messages: Option<Vec<Message>>,
    },
    /// 滚动定位到某条消息。
    ScrollToMessage {
        /// 消息 id。
        msg_id: String,
    },
    /// 重新发送（续传）一条发送失败的消息。
    RenewMessage {
        /// 目标会话。
        room_id: RoomId,
        /// 消息 id。
        message_id: String,
    },
    /// 戳一戳某个用户。
    Poke {
        /// 目标会话。
        room_id: RoomId,
        /// 被戳用户 QQ 号。
        target_id: i64,
    },
    /// 在输入框 @ 某个用户。
    MentionSender {
        /// 目标会话。
        room_id: RoomId,
        /// 被 @ 用户 QQ 号。
        target_id: i64,
        /// 被 @ 用户昵称。
        name: String,
    },
    /// 与该用户发起私聊。
    StartPrivateChat {
        /// 用户 QQ 号。
        target_id: i64,
        /// 用户昵称。
        name: String,
    },
    /// 查看某成员在本群的发言记录。
    MemberHistory {
        /// 目标会话。
        room_id: RoomId,
        /// 成员 QQ 号。
        target_id: i64,
        /// 成员昵称。
        name: String,
    },
    /// 打开成员管理（禁言/踢人等）。
    ManageMember {
        /// 目标会话。
        room_id: RoomId,
        /// 成员 QQ 号。
        target_id: i64,
    },
    /// 图片相关操作（查看、保存、复制等）。
    Image(ImageAction),
}

/// 输入区中待发送的一张本地图片。
#[derive(Debug, Clone)]
pub struct PendingImage {
    /// 预览用的稳定编号（进程内自增），用于 egui id 与去重。
    pub preview_id: u64,
    /// 文件名（发送时作为文件名）。
    pub name: String,
    /// MIME 类型（按扩展名推断）。
    pub mime_type: String,
    /// 图片字节；`Arc` 共享，避免预览与发送重复拷贝。
    pub data: Arc<[u8]>,
}

impl PendingImage {
    /// 用字节构造待发送图片，并分配预览编号。
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

/// 输入区中待发送的一个普通文件；同一时刻至多一个。
#[derive(Debug, Clone)]
pub struct PendingFile {
    /// 文件名。
    pub name: String,
    /// 文件类型描述（按扩展名推断）。
    pub file_type: String,
    /// 文件字节；`Arc` 共享。
    pub data: Arc<[u8]>,
}

impl PendingFile {
    /// 用字节构造待发送文件。
    pub fn new(name: String, file_type: String, data: Vec<u8>) -> Self {
        Self {
            name,
            file_type,
            data: data.into(),
        }
    }
}
