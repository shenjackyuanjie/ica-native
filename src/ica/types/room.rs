//! 会话（房间）与加群／加好友申请的类型。

use crate::ica::types::{
    RoomId, UserId,
    message::{At, LastMessage},
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// export default interface Room {
///     roomId: number
///     roomName: string
///     index: number
///     unreadCount: number
///     priority: 1 | 2 | 3 | 4 | 5
///     utime: number
///     users:
///         | [{ _id: 1; username: '1' }, { _id: 2; username: '2' }]
///         | [{ _id: 1; username: '1' }, { _id: 2; username: '2' }, { _id: 3; username: '3' }]
///     at?: boolean | 'all'
///     lastMessage: LastMessage
///     autoDownload?: boolean
///     downloadPath?: string
/// }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    /// 会话 ID；群聊为负群号，私聊为正 QQ 号。
    #[serde(rename = "roomId")]
    pub room_id: RoomId,
    /// 会话名称。
    #[serde(rename = "roomName")]
    pub room_name: String,
    /// 排序下标。
    pub index: i64,
    /// 未读消息数。
    #[serde(rename = "unreadCount")]
    pub unread_count: u64,
    /// 优先级（1-5，越大越靠前）。
    pub priority: u8,
    /// 最后活跃时间（毫秒时间戳）。
    pub utime: i64,
    /// StorageProvider 添加会话时仍会持久化这个旧字段。
    #[serde(default)]
    pub users: JsonValue,
    /// @ 状态。
    #[serde(default)]
    pub at: At,
    /// 最后一条消息摘要。
    #[serde(rename = "lastMessage")]
    pub last_message: LastMessage,
}

impl Room {
    /// 获取头像 URL
    /// 群聊: https://p.qlogo.cn/gh/{abs(room_id)}/{abs(room_id)}/0
    /// 私聊: https://q1.qlogo.cn/g?b=qq&nk={room_id}&s=140
    pub fn avatar_url(&self) -> String {
        if self.room_id < 0 {
            // 群聊
            let abs_id = self.room_id.abs();
            format!("https://p.qlogo.cn/gh/{abs_id}/{abs_id}/0")
        } else {
            // 私聊
            format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", self.room_id)
        }
    }
}

/// 一条待处理的加群／加好友申请。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JoinRequestRoom {
    /// 申请说明（验证信息）。
    #[serde(default)]
    pub comment: String,
    /// 目标群（加群申请时有值）。
    #[serde(default)]
    pub group_id: Option<RoomId>,
    /// 群名称。
    #[serde(default)]
    pub group_name: String,
    /// 申请人 QQ。
    #[serde(default)]
    pub user_id: UserId,
    /// 申请人昵称。
    #[serde(default)]
    pub nickname: String,
    /// 请求类型。
    #[serde(default)]
    pub request_type: String,
    /// 上报类型。
    #[serde(default)]
    pub post_type: String,
    /// 子类型。
    #[serde(default)]
    pub sub_type: String,
    /// 申请时间（毫秒时间戳）。
    #[serde(default)]
    pub time: i64,
    /// 提示文本。
    #[serde(default)]
    pub tips: String,
    /// 处理凭证，同意／拒绝时原样回传。
    #[serde(default)]
    pub flag: String,
    /// 来源。
    #[serde(default)]
    pub source: String,
}
