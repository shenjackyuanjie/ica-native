use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub static CONFIG: OnceLock<IcaCfg> = OnceLock::new();

/// 配置文件
///
/// 考虑到允许你同时连接多个 bridge, 所以这玩意做的有点复杂
#[derive(Debug, Serialize, Deserialize)]
pub struct IcaCfg {
    /// bridge 列表
    pub bridges: Vec<IcaBridge>,
}

/// 具体 bridge 的配置
///
/// ## 登录功能
///
/// 理论上应该可以支持你去使用 ica native 让 bridge 登录
///
/// 但是考虑到会需要解析一些网页之类的, 还是请使用 icalingua 本体进行登录
///
/// 因此其实这玩意挺简洁的就是了
#[derive(Debug, Serialize, Deserialize)]
pub struct IcaBridge {
    /// socketio 服务器的 url
    pub url: String,
    /// socketio 的 private key (ed25519)
    pub private_key: String,
}
