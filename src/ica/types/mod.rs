//! 以防你好奇为啥这里的代码都像是 copy 过来的
//! 这里给你解答一下: 是的，是 copy 过来的
//! 从我的 shenbot 里 copy 过来的
//!
//! https://github.com/shenjackyuanjie/icalingua-bridge-bot
//!
//! 本模块汇总 ica-native 与 Bridge 通信时用到的协议数据类型与 ID 别名，
//! 并按职责拆分为公告、联系人、文件、消息、在线数据与房间六个子模块。

pub mod announcement;
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
/// Bridge 侧可能是数字或字符串，这里统一成字符串；解析与兼容处理见
/// `message::Message` 的反序列化实现。
pub type MessageId = String;

/// 会话 ID 的正负号约定辅助。
///
/// 群聊 ID 为负、私聊 ID 为正；这些方法用于在两种表示之间换算，避免调用处
/// 直接写 `abs` / 取负时弄错方向。
#[allow(unused)]
pub trait RoomIdTrait {
    /// 判断是否是群聊
    fn is_room(&self) -> bool;
    /// 判断是否是私聊
    fn is_chat(&self) -> bool {
        !self.is_room()
    }
    /// 换算成群聊用的负 ID。
    fn as_room_id(&self) -> RoomId;
    /// 换算成私聊用的正 ID。
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
