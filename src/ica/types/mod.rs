//! 以防你好奇为啥这里的代码都像是 copy 过来的
//! 这里给你解答一下: 是的，是 copy 过来的
//! 从我的 shenbot 里 copy 过来的
//!
//! https://github.com/shenjackyuanjie/icalingua-bridge-bot

pub mod contact;
pub mod files;
pub mod message;
pub mod online_data;
pub mod room;

/// 房间 id
/// 群聊 < 0
/// 私聊 > 0
pub type RoomId = i64;
/// 用户 id
pub type UserId = i64;
/// 消息 id
///
/// 解析方案:
pub type MessageId = String;

#[allow(unused)]
pub trait RoomIdTrait {
    /// 判断是否是群聊
    fn is_room(&self) -> bool;
    /// 判断是否是私聊
    fn is_chat(&self) -> bool {
        !self.is_room()
    }
    fn as_room_id(&self) -> RoomId;
    fn as_chat_id(&self) -> RoomId;
}

impl RoomIdTrait for RoomId {
    fn is_room(&self) -> bool {
        (*self).is_negative()
    }
    fn as_room_id(&self) -> RoomId {
        -(*self).abs()
    }
    fn as_chat_id(&self) -> RoomId {
        (*self).abs()
    }
}
