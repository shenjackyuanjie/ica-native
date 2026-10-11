#[cfg(test)]
#[path = "tests/http_send.rs"]
mod tests;

use crate::ica::types::message::SendMessage;
use rust_socketio::asynchronous::Client;
use serde_json::Value as JsonValue;
use std::time::Duration;

pub async fn request_send_token(client: &Client) -> Result<String, String> {
    let payload =
        crate::ica::ack::request(client, "requestToken", Vec::new(), Duration::from_secs(10))
            .await
            .map_err(|error| error.to_string())?;
    crate::ica::ack::nonempty_string(&payload, "requestToken")
        .map_err(|_| "requestToken 返回空 token".to_string())
}

pub async fn http_send_message(
    http: &reqwest::Client,
    api_base_url: &str,
    token: &str,
    message: &SendMessage,
) -> Result<(), String> {
    http_send_value(http, api_base_url, token, &message.as_value()).await
}

pub async fn http_send_value(
    http: &reqwest::Client,
    api_base_url: &str,
    token: &str,
    value: &JsonValue,
) -> Result<(), String> {
    let url = format!(
        "{}/api/{}/sendMessage",
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
