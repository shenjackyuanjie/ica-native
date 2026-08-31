//! Socket.IO 连接与鉴权的可展示状态。

use std::fmt::Display;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl Display for SocketState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketState::Connecting => write!(f, "连接中"),
            SocketState::Connected => write!(f, "已连接"),
            SocketState::Disconnected => write!(f, "已断开"),
            SocketState::Failed => write!(f, "连接失败"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    #[default]
    Unknown,
    Pending,
    Succeeded,
    Failed,
}

impl Display for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthState::Unknown => write!(f, "未开始"),
            AuthState::Pending => write!(f, "认证中"),
            AuthState::Succeeded => write!(f, "已认证"),
            AuthState::Failed => write!(f, "认证失败"),
        }
    }
}
