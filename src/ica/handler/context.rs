//! 命令处理共用的上下文。
//!
//! 拆分前每个命令分支都要重复传 client / event_tx / bridge_key / socket_url /
//! api_base_url / HTTP clients 六个参数，改成一个 Copy 的上下文之后，各命令函数的签名里
//! 只保留它自己真正用到的字段。
//!
//! 上下文只借用一个命令的生命周期（`'a`），不持有连接；需要长期持有通道的组件
//! 应改用 `BridgeHandle`。

use crate::ica::event::BridgeEvent;
use rust_socketio::asynchronous::Client;
use tokio::sync::mpsc::UnboundedSender;

/// 一条命令处理期间可用的共享依赖。
///
/// `Copy` 让各命令函数可以按值接收并随意解构，不必为生命周期烦恼。
#[derive(Clone, Copy)]
pub struct CommandContext<'a> {
    /// 主 socket 连接，用于发事件与等 ACK。
    pub client: &'a Client,
    /// 回传 GUI 的事件通道；`None` 表示当前没有界面消费者。
    pub event_tx: &'a Option<UnboundedSender<BridgeEvent>>,
    /// 当前 bridge 标识，用于日志与事件归属。
    pub bridge_key: &'a str,
    /// socket 地址，fileMgr 等短连接复用同一地址。
    pub socket_url: &'a str,
    /// HTTP API 基地址（由 ws/wss 推导而来），大 payload 发送走这里。
    pub api_base_url: &'a str,
    /// 复用的 HTTP 客户端集合。
    pub http: &'a crate::ica::http::BridgeHttpClients,
}
