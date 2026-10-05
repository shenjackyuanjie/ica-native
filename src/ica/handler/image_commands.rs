//! 图片 URL 恢复只等待注册 ACK，避免阻塞其他聊天命令。
use super::context::CommandContext;
use crate::ica::ack;
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

pub async fn refresh_image_url(
    ctx: CommandContext<'_>,
    file_id: String,
    app_id: String,
    result_tx: oneshot::Sender<Result<String, String>>,
) {
    let pending = ack::start(
        ctx.client,
        "getNTPicURLbyFileid",
        vec![json!(file_id), json!(app_id)],
        Duration::from_secs(12),
    )
    .await;
    tokio::spawn(async move {
        let result = match pending {
            Ok(pending) => pending
                .receive()
                .await
                .map_err(|error| error.to_string())
                .and_then(|payload| {
                    ack::nonempty_string(&payload, "getNTPicURLbyFileid")
                        .map_err(|_| "Bridge 未能刷新图片地址".to_string())
                }),
            Err(error) => Err(error.to_string()),
        };
        let _ = result_tx.send(result);
    });
}
