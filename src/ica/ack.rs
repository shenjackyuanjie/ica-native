//! Bridge ACK 的一次完成与截止时间。只统一传输，不替协议决定返回值含义。
use futures_util::FutureExt;
use rust_socketio::{Payload, asynchronous::Client};
use serde_json::{Value as JsonValue, json};
use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckFailure {
    Send,
    Timeout,
    Closed,
    InvalidResponse(&'static str),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckError {
    pub event: String,
    pub failure: AckFailure,
}
impl AckError {
    pub fn invalid(event: &str, reason: &'static str) -> Self {
        Self {
            event: event.into(),
            failure: AckFailure::InvalidResponse(reason),
        }
    }
}
impl fmt::Display for AckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self.failure {
            AckFailure::Send => "请求发送失败",
            AckFailure::Timeout => "ACK 等待超时",
            AckFailure::Closed => "ACK 回调通道已关闭",
            AckFailure::InvalidResponse(reason) => reason,
        };
        write!(f, "{}: {}", self.event, detail)
    }
}
impl std::error::Error for AckError {}

type ReplySlot = Mutex<Option<oneshot::Sender<Payload>>>;
/// 回调只持短时同步锁；不在锁内执行用户回调、网络请求或异步等待。
#[derive(Clone)]
pub struct Completion(Arc<ReplySlot>);
impl Completion {
    pub fn complete(&self, payload: Payload) -> bool {
        let sender = self.0.lock().expect("ACK 完成状态锁被污染").take();
        sender.is_some_and(|sender| sender.send(payload).is_ok())
    }
}
/// 单个请求拥有接收端；丢弃它会作废传输层仍保留的迟到回调。
pub struct PendingAck {
    event: String,
    deadline: Instant,
    receiver: Option<oneshot::Receiver<Payload>>,
    completion: Weak<ReplySlot>,
}
impl PendingAck {
    pub async fn receive(mut self) -> Result<Payload, AckError> {
        let receiver = self.receiver.take().expect("ACK 只能等待一次");
        match tokio::time::timeout_at(self.deadline, receiver).await {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(_)) => Err(AckError {
                event: self.event.clone(),
                failure: AckFailure::Closed,
            }),
            Err(_) => Err(AckError {
                event: self.event.clone(),
                failure: AckFailure::Timeout,
            }),
        }
    }
}
impl Drop for PendingAck {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.upgrade() {
            completion.lock().expect("ACK 完成状态锁被污染").take();
        }
    }
}

/// 分开注册与等待，群成员／图片查询可把 receive 移入后台，继续处理聊天命令。
pub async fn start(
    client: &Client,
    event: &str,
    args: Vec<JsonValue>,
    timeout: Duration,
) -> Result<PendingAck, AckError> {
    start_with(event, timeout, |completion| async move {
        client
            .emit_with_ack(event, args, timeout, move |payload, _| {
                completion.complete(payload);
                async {}.boxed()
            })
            .await
            .map_err(|_| ())
    })
    .await
}

async fn start_with<F, Fut>(event: &str, timeout: Duration, emit: F) -> Result<PendingAck, AckError>
where
    F: FnOnce(Completion) -> Fut,
    Fut: Future<Output = Result<(), ()>>,
{
    let (sender, receiver) = oneshot::channel();
    let completion = Completion(Arc::new(Mutex::new(Some(sender))));
    let pending = PendingAck {
        event: event.into(),
        deadline: Instant::now() + timeout,
        receiver: Some(receiver),
        completion: Arc::downgrade(&completion.0),
    };
    match tokio::time::timeout_at(pending.deadline, emit(completion)).await {
        Ok(Ok(())) => Ok(pending),
        Ok(Err(())) => Err(AckError {
            event: event.into(),
            failure: AckFailure::Send,
        }),
        Err(_) => Err(AckError {
            event: event.into(),
            failure: AckFailure::Timeout,
        }),
    }
}

pub async fn request(
    client: &Client,
    event: &str,
    args: Vec<JsonValue>,
    timeout: Duration,
) -> Result<Payload, AckError> {
    start(client, event, args, timeout).await?.receive().await
}

/// 保留 Icalingua Bridge 单层 Socket.IO 参数包裹的兼容合同。
pub fn payload_values(payload: &Payload) -> Vec<JsonValue> {
    match payload {
        Payload::Text(values) => {
            if let Some(JsonValue::Array(args)) = values.first()
                && values.len() == 1
            {
                return args.clone();
            }
            values.clone()
        }
        Payload::Binary(bytes) => vec![json!(bytes.to_vec())],
        _ => Vec::new(),
    }
}
pub fn payload_first(payload: &Payload) -> Option<JsonValue> {
    payload_values(payload).into_iter().next()
}
pub fn nonempty_string(payload: &Payload, event: &str) -> Result<String, AckError> {
    payload_first(payload)
        .and_then(|value| value.as_str().filter(|s| !s.is_empty()).map(str::to_string))
        .ok_or_else(|| AckError::invalid(event, "响应必须是非空字符串"))
}

#[cfg(test)]
mod tests;
