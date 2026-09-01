use std::{sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::ica::command::emit_ui_event;
use crate::ica::event::BridgeEvent;
use crate::ica::types::RoomId;
use crate::ica::types::announcement::{
    ANNOUNCEMENT_COOKIE_DOMAIN, GroupAnnouncementDraft, announcement_delete_form,
    announcement_delete_url, announcement_list_form, announcement_list_url,
    announcement_publish_form, announcement_publish_url, parse_announcement_action, resolve_bkn,
};

use super::ack_payload_first;

const COOKIE_TIMEOUT: Duration = Duration::from_secs(15);
const COOKIE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const COOKIE_POLL_ATTEMPTS: usize = 150;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// 向 Bridge 索取指定域名的 Cookie。
///
/// Bridge 的 `getCookies` 只在 ACK 里回传结果，这里沿用发送 token 时的轮询等待方式。
async fn request_cookies(client: &Client, domain: &str) -> Result<String, String> {
    let cookie = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let cookie_cb = cookie.clone();
    client
        .emit_with_ack(
            "getCookies",
            vec![json!(domain)],
            COOKIE_TIMEOUT,
            move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                let cookie = cookie_cb.clone();
                Box::pin(async move {
                    let value = ack_payload_first(&payload)
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default();
                    *cookie.lock().await = Some(value);
                })
            },
        )
        .await
        .map_err(|error| format!("getCookies 发送失败: {error}"))?;

    for _ in 0..COOKIE_POLL_ATTEMPTS {
        if let Some(cookie) = cookie.lock().await.take() {
            return if cookie.trim().is_empty() {
                Err(format!(
                    "Bridge 返回了空的 {domain} Cookie，请确认账号已登录且协议端支持取 Cookie"
                ))
            } else {
                Ok(cookie)
            };
        }
        tokio::time::sleep(COOKIE_POLL_INTERVAL).await;
    }
    Err("getCookies 超时".to_string())
}

/// 用 Cookie 与 bkn 请求公告列表，返回 CGI 原始响应体。
///
/// CGI 恒定返回 HTTP 200，业务成败由响应体的 `ec` 决定，因此这里只负责传输，
/// 语义判定交给 `types::announcement::parse_announcement_list`。
/// 所有公告接口共用的调用流程：取 Cookie、定 bkn、POST 表单、解析 JSON。
///
/// 表单体由调用方按 `(bkn, 群号)` 拼好，因为各接口的字段差异较大，
/// 与其抽象成参数表，不如把拼装留在协议层。
async fn post_announcement_cgi<F>(
    client: &Client,
    room_id: RoomId,
    online_bkn: i64,
    url: &str,
    build_form: F,
) -> Result<JsonValue, String>
where
    F: FnOnce(i64, i64) -> String,
{
    let Some(group_id) = room_id.checked_neg().filter(|_| room_id < 0) else {
        return Err("只有群聊才有群公告".to_string());
    };

    let cookie = request_cookies(client, ANNOUNCEMENT_COOKIE_DOMAIN).await?;
    let Some(bkn) = resolve_bkn(online_bkn, &cookie) else {
        return Err("无法确定 bkn：onlineData 未下发，且 Cookie 里没有 skey".to_string());
    };

    let http = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败: {error}"))?;
    // 公告 CGI 一律是 POST 表单，且必须带 Cookie 才认登录态。
    let response = http
        .post(url)
        .header(reqwest::header::COOKIE, cookie)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(build_form(bkn, group_id))
        .send()
        .await
        .map_err(|error| format!("请求群公告失败: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("群公告接口返回 HTTP {status}"));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取群公告响应失败: {error}"))?;
    serde_json::from_str::<JsonValue>(&body).map_err(|error| {
        // 未登录或 Cookie 失效时 QQ 会返回 HTML 跳转页，这里给出可操作的提示。
        tracing::debug!(
            target: "ica_native::announcement",
            body = %body.chars().take(256).collect::<String>(),
            "群公告响应不是 JSON"
        );
        format!("群公告响应不是 JSON（可能是登录态失效）: {error}")
    })
}

/// 发布或编辑一条公告；草稿带 fid 即为编辑。
pub async fn publish_group_announcement(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    room_id: RoomId,
    online_bkn: i64,
    draft: GroupAnnouncementDraft,
) {
    let action = if draft.is_edit() { "编辑" } else { "发布" };
    let result = post_announcement_cgi(
        client,
        room_id,
        online_bkn,
        announcement_publish_url(draft.to_new),
        |bkn, group_id| announcement_publish_form(bkn, group_id, &draft),
    )
    .await
    .and_then(|body| parse_announcement_action(&body));
    emit_action_result(event_tx, bridge_key, request_id, room_id, action, result);
}

/// 删除一条公告。
pub async fn delete_group_announcement(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    room_id: RoomId,
    online_bkn: i64,
    fid: String,
) {
    let result = post_announcement_cgi(
        client,
        room_id,
        online_bkn,
        announcement_delete_url(),
        |bkn, group_id| announcement_delete_form(bkn, group_id, &fid),
    )
    .await
    .and_then(|body| parse_announcement_action(&body));
    emit_action_result(event_tx, bridge_key, request_id, room_id, "删除", result);
}

/// 统一上报写操作结果；成功时界面会据此自动刷新列表。
fn emit_action_result(
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    room_id: RoomId,
    action: &str,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => emit_ui_event(
            event_tx,
            bridge_key,
            "groupAnnouncementActionDone",
            json!({
                "requestId": request_id,
                "roomId": room_id,
                "action": action,
            }),
        ),
        Err(message) => {
            tracing::warn!(
                target: "ica_native::announcement",
                bridge = %bridge_key,
                request_id,
                room_id,
                action,
                error = %message,
                "群公告写操作失败"
            );
            emit_ui_event(
                event_tx,
                bridge_key,
                "groupAnnouncementActionFailed",
                json!({
                    "requestId": request_id,
                    "roomId": room_id,
                    "action": action,
                    "message": message,
                }),
            )
        }
    }
}

pub async fn fetch_group_announcements(
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    request_id: u64,
    room_id: RoomId,
    online_bkn: i64,
) {
    match post_announcement_cgi(
        client,
        room_id,
        online_bkn,
        announcement_list_url(),
        announcement_list_form,
    )
    .await
    {
        Ok(body) => emit_ui_event(
            event_tx,
            bridge_key,
            "groupAnnouncementsResponse",
            json!({
                "requestId": request_id,
                "roomId": room_id,
                "body": body,
            }),
        ),
        Err(message) => {
            tracing::warn!(
                target: "ica_native::announcement",
                bridge = %bridge_key,
                request_id,
                room_id,
                error = %message,
                "拉取群公告失败"
            );
            emit_ui_event(
                event_tx,
                bridge_key,
                "groupAnnouncementsFailed",
                json!({
                    "requestId": request_id,
                    "roomId": room_id,
                    "message": message,
                }),
            )
        }
    }
}
