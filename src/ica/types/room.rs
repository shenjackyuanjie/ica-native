use crate::ica::types::{
    RoomId, UserId,
    message::{At, LastMessage},
};
use serde::{Deserialize, Serialize};

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
    #[serde(rename = "roomId")]
    pub room_id: RoomId,
    #[serde(rename = "roomName")]
    pub room_name: String,
    pub index: i64,
    #[serde(rename = "unreadCount")]
    pub unread_count: u64,
    pub priority: u8,
    pub utime: i64,
    /// 我严重怀疑是脱裤子放屁
    /// 历史遗留啊,那没事了()
    // pub users: JsonValue,
    pub at: At,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequestRoom {
    pub comment: String,
    pub group_id: RoomId,
    pub group_name: String,
    pub user_id: UserId,
    pub nickname: String,
    pub request_type: String,
    pub post_type: String,
    pub sub_type: String,
    pub time: i64,
    pub tips: String,
    pub flag: String,
}
