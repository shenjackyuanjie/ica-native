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
    let response = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(45))
        // token 在路径里且只能消费一次，不允许重定向或自动重试 POST。
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("无法创建发送客户端: {}", error.without_url()))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn submission_requires_202_and_never_follows_redirects_or_retries_post() {
        for status in [
            "202 Accepted",
            "200 OK",
            "403 Forbidden",
            "413 Payload Too Large",
            "307 Temporary Redirect",
            "500 Internal Server Error",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 2048];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                assert!(request.starts_with(b"POST /prefix/api/test-token/sendMessage "));
                socket.write_all(format!("HTTP/1.1 {status}\r\nLocation: http://{address}/unexpected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                drop(socket);
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), listener.accept())
                        .await
                        .is_err(),
                    "不能重复发送或跟随携带一次性 token 的 POST 重定向"
                );
            });
            let result = http_send_value(
                &format!("http://{address}/prefix/"),
                "test-token",
                &serde_json::json!({"content": "测试"}),
            )
            .await;
            assert_eq!(result.is_ok(), status.starts_with("202"), "{status}");
            if let Err(error) = result {
                assert!(!error.contains("test-token"));
            }
            server.await.unwrap();
        }
    }
}
