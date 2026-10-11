//! 经 Bridge HTTP API 发送消息。
//!
//! 含 base64 媒体的消息不走 socket：payload 体积大，且 Bridge 的 HTTP 通道会用
//! 202 明确回报"已接收"，比 socket 的单向 emit 更可靠。流程是先在 socket 上
//! 用 `requestToken` 换取一次性 token，再 `POST {api_base_url}/api/{token}/sendMessage`。
//!
//! token 只出现在 URL 路径里，因此 HTTP 客户端禁用了重定向与自动重试
//! （见 `ica::http`），避免同一条消息被重复提交。

#[cfg(test)]
#[path = "tests/http_send.rs"]
mod tests;

use crate::ica::types::message::SendMessage;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use std::time::Duration;

/// 换取一次性的消息发送 token。
///
/// 10 秒超时；Bridge 返回空 token 时按失败处理，因为空 token 无法用于后续请求。
pub async fn request_send_token(client: &Client) -> Result<String, String> {
    let payload =
        crate::ica::ack::request(client, "requestToken", Vec::new(), Duration::from_secs(10))
            .await
            .map_err(|error| error.to_string())?;
    crate::ica::ack::nonempty_string(&payload, "requestToken")
        .map_err(|_| "requestToken 返回空 token".to_string())
}

/// 发送一条 `SendMessage`（内部先序列化成协议 JSON）。
pub async fn http_send_message(
    http: &reqwest::Client,
    api_base_url: &str,
    token: &str,
    message: &SendMessage,
) -> Result<(), String> {
    http_send_value(http, api_base_url, token, &message.as_value()).await
}

/// 发送任意协议 JSON 到 Bridge 的 `sendMessage` 接口。
///
/// 返回 `Ok(())` 仅表示 Bridge 已接收（HTTP 202），不等于 QQ 侧已送达。
/// 403 视为 token 失效、413 视为附件过大，其余状态码原样回报便于排查。
pub async fn http_send_value(
    http: &reqwest::Client,
    api_base_url: &str,
    token: &str,
    value: &JsonValue,
) -> Result<(), String> {
    let url = format!(
        "{}/api/{}/sendMessage",
        // 基地址可能带尾斜杠，去掉以免出现双斜杠路径。
        api_base_url.trim_end_matches('/'),
        token
    );
    let response = http
        .post(url)
        .json(value)
        .send()
        .await
        .map_err(|error| format!("HTTP POST 失败: {}", error.without_url()))?;
    match response.status() {
        reqwest::StatusCode::ACCEPTED => Ok(()),
        reqwest::StatusCode::FORBIDDEN => Err("token 验证失败 (403)".to_string()),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => Err("附件过大，无法发送 (413)".to_string()),
        status => Err(format!("sendMessage HTTP 错误: {status}")),
    }
}
