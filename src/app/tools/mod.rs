//! 账号、群、文件、消息、会话和 Socket 管理工具。
//!
//! 每项工具把状态、校验和 egui 视图放在一起，因为它们共同组成一个完整的
//! 底层 bridge API 操作界面。
//!
//! 该模块位于 `crate::app` 之下，属于面向高级/调试场景的工具面板聚合点。
//! 每个子模块负责一类 bridge Socket API：把该类操作的输入状态（`*ToolsState`）、
//! 参数校验与 egui 窗口收拢到一处，避免散落到各个视图里。各窗口由 `IcaApp` 上的
//! `render_*_tools_window` 方法在每帧驱动，向用户呈现按钮、输入框与“最近响应”。
//!
//! 设计要点：
//! - 每个面板只通过既有的 Socket API 通道与 bridge 通信，不持有独立的网络栈；
//! - 输入在 UI 线程先做一次解析/校验，失败仅写入错误提示，不触发任何请求；
//! - 真正要下发的动作在窗口闭包结束后统一 flush，避免在渲染 borrow 期间改状态。

mod account;
mod file;
mod group;
mod message;
mod room;
mod socket;

// 仅对外再导出各面板的输入状态类型，供应用状态聚合与视图层引用；
// 渲染入口以 `IcaApp::render_*_tools_window` 固有方法形式提供，无需在此重复导出。
pub use account::AccountToolsState;
pub use file::FileToolsState;
pub use group::GroupToolsState;
pub use message::MessageToolsState;
pub use room::RoomToolsState;
