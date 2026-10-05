//! 每个 Bridge 长期复用的 HTTP 客户端。
use std::time::Duration;

#[derive(Clone)]
pub struct BridgeHttpClients {
    pub send: reqwest::Client,
    pub announcement: reqwest::Client,
}

impl BridgeHttpClients {
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
