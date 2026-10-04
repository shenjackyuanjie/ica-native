//! 图片 URL 的 Socket.IO 恢复接口。不等待 ACK，避免堵住其他聊天命令。
use super::{ack_payload_first, context::CommandContext};
use futures_util::FutureExt;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

pub async fn refresh_image_url(
    ctx: CommandContext<'_>,
    file_id: String,
    app_id: String,
    result_tx: oneshot::Sender<Result<String, String>>,
) {
    let sender = Arc::new(Mutex::new(Some(result_tx)));
    let callback_sender = sender.clone();
    let result = ctx
        .client
        .emit_with_ack(
            "getNTPicURLbyFileid",
            vec![json!(file_id), json!(app_id)],
            Duration::from_secs(12),
            move |payload, _| {
                let sender = callback_sender.clone();
                async move {
                    let result = ack_payload_first(&payload)
                        .and_then(|value| {
                            value
                                .as_str()
                                .filter(|url| !url.is_empty())
                                .map(str::to_string)
                        })
                        .ok_or_else(|| "Bridge 未能刷新图片地址".to_string());
                    if let Some(sender) = sender.lock().expect("图片请求回调锁被污染").take()
                    {
                        let _ = sender.send(result);
                    }
                }
                .boxed()
            },
        )
        .await;
    if result.is_err()
        && let Some(sender) = sender.lock().expect("图片请求回调锁被污染").take()
    {
        let _ = sender.send(Err("发送图片地址刷新请求失败".to_string()));
    }
}
