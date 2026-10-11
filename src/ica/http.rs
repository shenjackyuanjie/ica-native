//! 每个 Bridge 长期复用的 HTTP 客户端。
//!
//! Bridge 的 socket 主连接不适合传输大 payload（含 base64 的图片、长文本），
//! 因此消息发送另有一条 HTTP 通道：`POST {api_base_url}/api/{token}/sendMessage`，
//! token 由 socket 上的 `requestToken` 事件换取，见 `handler::http_send`。
//!
//! 这里按用途拆成两个客户端并长期复用连接池：
//! - `send`：消息发送，超时较长、严格禁止隐式重试与重定向；
//! - `announcement`：群公告 CGI 访问，只设总超时。
//!
//! 两个客户端都在 `run_bridge` 启动时创建一次，连接与各 bridge 一一对应。

use std::time::Duration;

/// 按用途拆分的 Bridge HTTP 客户端集合。
///
/// 之所以不用单个客户端：消息发送与群公告对超时、重定向和重试的要求完全不同，
/// 混在一个 `reqwest::Client` 里只能取最保守的设置。
#[derive(Clone)]
pub struct BridgeHttpClients {
    /// 消息发送客户端，用于 `sendMessage` HTTP 通道。
    pub send: reqwest::Client,
    /// 群公告客户端，用于直连 QQ Web CGI。
    pub announcement: reqwest::Client,
}

impl BridgeHttpClients {
    /// 构造客户端集合。
    ///
    /// `send` 客户端显式关闭重定向与自动重试：一次性 token 位于 URL 路径中，
    /// 任何隐式重试都可能把同一条消息重复提交。`announcement` 只限制总超时，
    /// 因为群公告 CGI 可能需要跟随跳转。
    ///
    /// 返回 `Err` 时仅可能是 TLS/代理等本地环境问题，错误文本已去掉 URL 以免泄露 token。
    pub fn new() -> Result<Self, String> {
        let send = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .redirect(reqwest::redirect::Policy::none())
            // 一次性 token 在路径中，任何隐式重试都可能造成重复提交。
            .retry(reqwest::retry::never());
        // 单元测试只连接随机本地端口，不能受宿主 HTTP_PROXY 干扰。
        #[cfg(test)]
        let send = send.no_proxy();
        let send = send
            .build()
            .map_err(|error| format!("无法创建消息发送客户端: {}", error.without_url()))?;
        let announcement = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("无法创建群公告客户端: {}", error.without_url()))?;
        Ok(Self { send, announcement })
    }
}
