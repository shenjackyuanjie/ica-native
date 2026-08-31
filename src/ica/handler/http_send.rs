use std::{sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::Value as JsonValue;

use crate::ica::types::message::SendMessage;

use super::ack_payload_first;

pub async fn request_send_token(client: &Client) -> Result<String, String> {
    let token = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let token_cb = token.clone();
    client
        .emit_with_ack(
            "requestToken",
            Vec::<JsonValue>::new(),
            Duration::from_secs(30),
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let token = token_cb.clone();
                Box::pin(async move {
                    let value = ack_payload_first(&payload)
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default();
                    *token.lock().await = Some(value);
                })
            },
        )
        .await
        .map_err(|error| format!("requestToken 发送失败: {error}"))?;

    for _ in 0..100 {
        if let Some(token) = token.lock().await.take() {
            return if token.is_empty() {
                Err("requestToken 返回空 token".to_string())
            } else {
                Ok(token)
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("requestToken 超时".to_string())
}

pub async fn http_send_message(
    api_base_url: &str,
    token: &str,
    message: &SendMessage,
) -> Result<(), String> {
    http_send_value(api_base_url, token, &message.as_value()).await
}

pub async fn http_send_value(
    api_base_url: &str,
    token: &str,
    value: &JsonValue,
) -> Result<(), String> {
    let url = format!(
        "{}/api/{}/sendMessage",
        api_base_url.trim_end_matches('/'),
        token
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(value)
        .send()
        .await
        .map_err(|error| format!("HTTP POST 失败: {error}"))?;
    match response.status() {
        reqwest::StatusCode::ACCEPTED => Ok(()),
        reqwest::StatusCode::FORBIDDEN => Err("token 验证失败 (403)".to_string()),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE => Err("图片过大，无法发送 (413)".to_string()),
        status => Err(format!("sendMessage HTTP 错误: {status}")),
    }
}
