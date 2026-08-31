//! 命令处理共用的上下文。
//!
//! 拆分前每个命令分支都要重复传 client / event_tx / bridge_key / socket_url /
//! api_base_url 五个参数，改成一个 Copy 的上下文之后，各命令函数的签名里
//! 只保留它自己真正用到的字段。

use rust_socketio::asynchronous::Client;
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;

#[derive(Clone, Copy)]
pub struct CommandContext<'a> {
    pub client: &'a Client,
    pub event_tx: &'a Option<UnboundedSender<BridgeEvent>>,
    pub bridge_key: &'a str,
    pub socket_url: &'a str,
    pub api_base_url: &'a str,
}
