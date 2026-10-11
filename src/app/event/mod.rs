//! 应用事件总线：把 Bridge 协议事件、媒体后台任务结果与 Agent 上下文请求
//! 统一成一种在 Tokio 侧与 egui 主线程之间传递的消息类型。
//!
//! 所有跨线程投递都经由 [`AppEvent`]：后台任务完成后通过 `event_tx` 发送，
//! 主线程在 `poll_socketio_events` 中 `try_recv` 非阻塞消费，从而避免在 egui
//! 更新闭包里直接触碰 Tokio 状态。`reducer` 与各领域子模块负责把 `Bridge`
//! 变体翻译成本地状态变更。

mod announcement;
mod connection;
mod contacts;
mod forward;
mod group;
mod login;
mod message;
mod misc;
mod payload;
mod reducer;
mod room;
mod search;

use super::media::MediaEvent;
use crate::ica::BridgeEvent;

/// 从后台线程投递到 egui 主线程的应用级事件。
///
/// 三个变体分别对应三类来源，主线程按需分发：
/// - [`AppEvent::AgentContext`]：本地 Agent 上下文服务发来的 GUI 请求。
/// - [`AppEvent::Bridge`]：socket.io Bridge 的协议事件（含 `bridge_key`，用于定位连接）。
/// - [`AppEvent::Media`]：媒体/表情后台任务完成后回传的结果。
#[derive(Debug)]
pub enum AppEvent {
    /// Agent 上下文服务请求主线程执行某个 GUI 操作。
    AgentContext(crate::agent_context::server::GuiRequest),
    /// 某个 Bridge 连接产生的协议事件。
    Bridge(BridgeEvent),
    /// 媒体后台任务（复制/保存图片、刷新表情等）完成后的结果。
    Media(MediaEvent),
}
