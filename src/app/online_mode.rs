use std::fmt::Display;

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

impl Display for OnlineMode {
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
