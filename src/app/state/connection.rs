//! Socket.IO 连接与鉴权的可展示状态。
//!
//! 这两个枚举是界面上“连接横幅/状态页”的唯一数据来源：协议事件只把它们
//! 推向某个取值，渲染与文案（`Display`）集中在此，避免各处硬编码中文。

use std::fmt::Display;

/// Socket.IO 连接状态。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    /// 正在建立连接（初始状态）。
    #[default]
    Connecting,
    /// 连接已建立。
    Connected,
    /// 连接已断开（可能是主动关闭或被动掉线）。
    Disconnected,
    /// 连接失败（无法连通或协议不兼容）。
    Failed,
}

/// 把连接状态渲染成顶栏/状态页使用的中文文案。
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

/// 登录鉴权状态。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    /// 尚未开始鉴权（初始状态）。
    #[default]
    Unknown,
    /// 鉴权进行中（例如等待扫码、滑块或登录确认）。
    Pending,
    /// 鉴权成功。
    Succeeded,
    /// 鉴权失败。
    Failed,
}

/// 把鉴权状态渲染成顶栏/状态页使用的中文文案。
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
