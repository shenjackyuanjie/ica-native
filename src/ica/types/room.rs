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
    #[serde(rename = "roomId")]
    pub room_id: RoomId,
    #[serde(rename = "roomName")]
    pub room_name: String,
    pub index: i64,
    #[serde(rename = "unreadCount")]
    pub unread_count: u64,
    pub priority: u8,
    pub utime: i64,
    /// StorageProvider 添加会话时仍会持久化这个旧字段。
    #[serde(default)]
    pub users: JsonValue,
    #[serde(default)]
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Room;
    use crate::ica::types::message::At;

    #[test]
    fn missing_optional_at_defaults_to_none() {
        let room: Room = serde_json::from_value(json!({
            "roomId": -123,
            "roomName": "测试会话",
            "index": 0,
            "unreadCount": 1,
            "priority": 2,
            "utime": 1_700_000_000_000_i64,
            "lastMessage": {
                "content": "普通消息",
                "timestamp": "12:00"
            }
        }))
        .expect("bridge 协议中的 Room.at 是可选字段");

        assert_eq!(room.at, At::None);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JoinRequestRoom {
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub group_id: Option<RoomId>,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub user_id: UserId,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub request_type: String,
    #[serde(default)]
    pub post_type: String,
    #[serde(default)]
    pub sub_type: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default)]
    pub tips: String,
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub source: String,
}
