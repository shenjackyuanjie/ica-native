//! 在线状态模式枚举。
//!
//! 对应 QQ 的几种在线状态，供用户在“在线状态”窗口切换，并通过 socket.io 的
//! `setOnline` 等事件驱动 Bridge 设置远端在线状态。集中在此处便于界面下拉框、
//! 配置持久化与 Bridge 指令之间复用同一组取值。

use std::fmt::Display;

/// 用户的在线状态模式。
///
/// 变体顺序与 `#[default]` 共同决定未配置时的默认值（在线）；各变体的中文标签
/// 由 [`Display`] 实现给出，界面与配置持久化都直接复用该枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OnlineMode {
    /// 在线
    #[default]
    Online,
    /// 离开
    Left,
    /// 隐身
    Hidden,
    /// 忙碌
    Busy,
    /// Q我吧
    PingMe,
    /// 请勿打扰
    DoNotDisturb,
}

/// 把在线状态格式化为界面直接展示的中文标签。
impl Display for OnlineMode {
    /// 按当前变体写出对应中文，例如在线、隐身、忙碌、请勿打扰等。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnlineMode::Online => write!(f, "在线"),
            OnlineMode::Left => write!(f, "离开"),
            OnlineMode::Hidden => write!(f, "隐身"),
            OnlineMode::Busy => write!(f, "忙碌"),
            OnlineMode::PingMe => write!(f, "Q我吧"),
            OnlineMode::DoNotDisturb => write!(f, "请勿打扰"),
        }
    }
}
