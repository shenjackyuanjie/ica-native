//! Bridge ACK 的一次完成与截止时间。只统一传输，不替协议决定返回值含义。
//!
//! Socket.IO 的 `emit_with_ack` 把"发出请求"和"收到回调"耦在一起，而群成员、
//! 图片地址等查询需要先注册、再把等待挪到后台，主循环才能继续处理聊天命令。
//! 因此这里把一次 ACK 拆成两半：
//! - `start`：发出请求，立刻拿到可转移的 `PendingAck`（注册与等待分离）；
//! - `PendingAck::receive`：在任意位置等待结果，受统一截止时间约束。
//!
//! 传输层只保证"回调被调用过一次或超时/断开"，不解释回调内容的业务含义；
//! 载荷解析与兼容处理见本文件末尾的 `payload_values` / `nonempty_string`。

#[cfg(test)]
#[path = "tests/ack.rs"]
mod tests;

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

/// 一次 ACK 失败的原因。
///
/// 区分"没发出去"（`Send`）、"发出去但没等到"（`Timeout`）、"通道被丢弃"
/// （`Closed`）与"回来了但内容不合法"（`InvalidResponse`），GUI 才能给出准确提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckFailure {
    /// 请求未能通过 socket 发出。
    Send,
    /// 在截止时间前没有收到回调。
    Timeout,
    /// 回调通道已被关闭（通常是等待方被丢弃）。
    Closed,
    /// 收到回调但载荷不符合该事件的约定；`&'static str` 为可直接展示的原因。
    InvalidResponse(&'static str),
}

/// ACK 错误：事件名 + 失败原因。
///
/// 事件名随错误一起传递，调用方无需再知道自己刚才请求的是哪个事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckError {
    /// 出错的事件名。
    pub event: String,
    /// 失败原因。
    pub failure: AckFailure,
}

impl AckError {
    /// 构造一个"响应内容不合法"错误。
    pub fn invalid(event: &str, reason: &'static str) -> Self {
        Self {
            event: event.into(),
            failure: AckFailure::InvalidResponse(reason),
        }
    }
}

impl fmt::Display for AckError {
    /// 把失败原因翻译成中文，附带事件名，便于直接展示给用户。
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

/// 回调通道槽位：`Some(sender)` 表示还没回调过，`None` 表示已被消费或作废。
type ReplySlot = Mutex<Option<oneshot::Sender<Payload>>>;

/// 交给 socket 回调的完成器。
///
/// 回调只持短时同步锁；不在锁内执行用户回调、网络请求或异步等待。
#[derive(Clone)]
pub struct Completion(Arc<ReplySlot>);

impl Completion {
    /// 投递一次回调结果。
    ///
    /// 槽位已被取走（重复回调）或接收端已丢弃时返回 `false`；重复回调不会 panic，
    /// 因为 socket 层理论上可能对同一事件触发多次。
    pub fn complete(&self, payload: Payload) -> bool {
        let sender = self.0.lock().expect("ACK 完成状态锁被污染").take();
        sender.is_some_and(|sender| sender.send(payload).is_ok())
    }
}

/// 单个请求拥有接收端；丢弃它会作废传输层仍保留的迟到回调。
pub struct PendingAck {
    /// 事件名，用于构造错误。
    event: String,
    /// 统一截止时间点（`Instant` 而非 `Duration`，避免等待期间被反复重置）。
    deadline: Instant,
    /// 接收端；`receive` 消费自身时取出，保证只能等待一次。
    receiver: Option<oneshot::Receiver<Payload>>,
    /// 完成器的弱引用：等待方丢弃后不会阻止 `Completion` 被释放。
    completion: Weak<ReplySlot>,
}

impl PendingAck {
    /// 等待回调结果，受 `start` 时确定的截止时间约束。
    ///
    /// 只能调用一次（第二次会因接收端已被取走而 panic）。返回：
    /// - `Ok(payload)`：按时收到回调；
    /// - `Err(Closed)`：完成器被丢弃或发送端关闭；
    /// - `Err(Timeout)`：超过截止时间。
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
    /// 等待方被丢弃时取走槽位，让后续到达的回调无处可投。
    ///
    /// 这一步同时打破 `Completion` 与 socket 回调之间的引用预期：迟到回调只会
    /// 发现槽位已空而返回 `false`，不会写入一个没人读的 channel。
    fn drop(&mut self) {
        if let Some(completion) = self.completion.upgrade() {
            completion.lock().expect("ACK 完成状态锁被污染").take();
        }
    }
}

/// 分开注册与等待，群成员／图片查询可把 receive 移入后台，继续处理聊天命令。
///
/// `emit_with_ack` 本身也受 `timeout` 约束：注册阶段超时同样按 `AckFailure::Timeout`
/// 上报，避免一个卡住的 emit 占住调用方。
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

/// `start` 的可测试内核：把"怎么发请求"抽象成闭包，便于在不构造 socket 的情况下
/// 验证超时、发送失败与迟到回调的处理。
///
/// 截止时间在进入本函数时一次性确定，注册与等待共享同一个 `Instant`。
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

/// 一步到位的 ACK 请求：发出后立刻等待结果。
///
/// 适合结果必须就地使用的调用（如换取 token、取文件管理句柄）；
/// 需要把等待挪到后台的场景请改用 `start`。
pub async fn request(
    client: &Client,
    event: &str,
    args: Vec<JsonValue>,
    timeout: Duration,
) -> Result<Payload, AckError> {
    start(client, event, args, timeout).await?.receive().await
}

/// 保留 Icalingua Bridge 单层 Socket.IO 参数包裹的兼容合同。
///
/// Bridge 的 ACK 回调有时把参数再包一层数组（`[[a, b]]`），有时直接给 `[a, b]`；
/// 这里只剥掉"仅有一个元素且该元素是数组"的情况，其余形状原样返回，
/// 从而让两种历史形态都能按同一套下标读取。
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

/// 取 ACK 回调的第一个参数，没有参数时返回 `None`。
pub fn payload_first(payload: &Payload) -> Option<JsonValue> {
    payload_values(payload).into_iter().next()
}

/// 要求 ACK 回调的第一个参数是非空字符串。
///
/// 用于 token 一类"必须拿到可用值"的返回；空字符串按无效处理，
/// 因为 Bridge 有时用空串表示内部失败而不是报错。
pub fn nonempty_string(payload: &Payload, event: &str) -> Result<String, AckError> {
    payload_first(payload)
        .and_then(|value| value.as_str().filter(|s| !s.is_empty()).map(str::to_string))
        .ok_or_else(|| AckError::invalid(event, "响应必须是非空字符串"))
}
