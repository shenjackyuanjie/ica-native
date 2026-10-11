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
        let http = crate::ica::http::BridgeHttpClients::new().unwrap();
        let result = http_send_value(
            &http.send,
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

#[tokio::test]
async fn shared_client_reuses_keep_alive_connection_between_submissions() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        for request_index in 0..2 {
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
            assert!(request.starts_with(b"POST /api/shared-token/sendMessage "));
            let connection = if request_index == 0 {
                "keep-alive"
            } else {
                "close"
            };
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: {connection}\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "同一个 Bridge 的连续发送应复用连接池"
        );
    });
    let http = crate::ica::http::BridgeHttpClients::new().unwrap();
    for value in [1, 2] {
        http_send_value(
            &http.send,
            &format!("http://{address}"),
            "shared-token",
            &serde_json::json!({"content": value}),
        )
        .await
        .unwrap();
    }
    server.await.unwrap();
}
