//! 与 noticer 0.4.2 HTTP API 兼容的内建 webhook 服务。
//!
//! 本模块在客户端进程内启动一个 axum 服务（默认仅监听回环地址），把外部 HTTP
//! 提醒请求转成 `IcaCommand::SendNoticerMessage` 交给对应 Bridge 发送。
//! 接口「成功」只表示消息已提交给 Bridge，不代表 QQ 服务端最终送达。
//!
//! 关键设计：
//! - 三套端点语义：`/send`（旧版，宽容解析）、`/v1/send`（严格模式，按配置的
//!   房间名路由）、`/v1/send/direct`（直接指定 `room_id`，需单独 Token）；
//! - 请求经有界队列交给单个 worker 串行发送，配合重试、超时与幂等键；
//! - Bridge 是否就绪、目标群是否在当前会话中，由 [`BridgeRegistry`] 根据
//!   Bridge 事件实时推断，发送前先做一次预检。

#[cfg(test)]
mod tests;

use crate::config::{NoticerConfig, NoticerRoom};
use crate::ica::{BridgeEvent, BridgeHandle, IcaCommand, NoticerImage, NoticerSendPayload};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use serde_json::{Map, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc, oneshot, watch};

/// 对外声明的 API 版本，出现在 `/status` 与 `/config` 的 `server` 字段。
pub const API_VERSION: &str = "0.4.2";
/// 请求体大小硬上限（512MB）；配置值超过它会被 `NoticerConfig::validate` 拒绝。
pub const HARD_MAX_BODY_SIZE: usize = 512 * 1024 * 1024;
/// 单张图片大小硬上限（64MB）。
pub const HARD_MAX_IMAGE_SIZE: usize = 64 * 1024 * 1024;
/// 单次请求图片数量硬上限。
pub const HARD_MAX_IMAGE_COUNT: usize = 32;
/// 单次请求图片总大小硬上限（512MB）。
pub const HARD_MAX_TOTAL_IMAGE_SIZE: usize = 512 * 1024 * 1024;
/// 队列中待发送图片的字节总量硬上限（1GB）；超出后新请求直接返回 503。
pub const HARD_MAX_QUEUED_IMAGE_BYTES: usize = 1024 * 1024 * 1024;
/// 待发送队列容量硬上限（按任务个数计）。
pub const HARD_MAX_QUEUE_CAPACITY: usize = 4096;
/// 单次发送超时硬上限（秒）。
pub const HARD_MAX_SEND_TIMEOUT_SECONDS: f64 = 300.0;
/// 发送重试次数硬上限。
pub const HARD_MAX_RETRY_ATTEMPTS: usize = 16;
/// 重试间隔硬上限（秒）。
pub const HARD_MAX_RETRY_DELAY_SECONDS: f64 = 30.0;
/// 幂等记录存活时间硬上限（秒，24 小时）。
pub const HARD_MAX_IDEMPOTENCY_TTL_SECONDS: u64 = 24 * 60 * 60;
/// 幂等记录条数硬上限。
pub const HARD_MAX_IDEMPOTENCY_ENTRIES: usize = 65536;
/// `Idempotency-Key` 请求头的最大长度。
pub const IDEMPOTENCY_KEY_MAX_LENGTH: usize = 128;
/// 支持的图片 MIME 白名单；探测出的真实类型必须落在该集合内才允许发送。
pub const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// `GET /` 返回的纯文本文档，列出全部端点与兼容性说明。
const ROOT_DOC: &str = r#"Noticer 本地提醒服务（ica-native 内建）

GET  /health
GET  /ready
GET  /status
GET  /config
POST /send
POST /v1/send
POST /v1/send/direct

HTTP 请求与 noticer 0.4.2 兼容。成功表示消息已提交给 Bridge，不代表 QQ 服务端最终送达。
"#;

/// 单个 Bridge 的运行时状态，全部由 Bridge 事件推断，不主动查询协议端。
#[derive(Debug, Clone, Default)]
struct BridgeRuntimeState {
    /// 是否已完成鉴权（收到 `authSucceed` 后为 true）。
    authenticated: bool,
    /// 是否在线（收到 `onlineData` / `setOnline` 为 true）。
    online: bool,
    /// 房间目录是否已同步（收到过 `setAllRooms`）。
    rooms_ready: bool,
    /// 当前会话中的房间 ID 集合；发送前用它判断机器人是否在目标群。
    rooms: HashSet<i64>,
}

impl BridgeRuntimeState {
    /// 三个条件同时满足才认为该 Bridge 可承担发送任务。
    fn ready(&self) -> bool {
        self.authenticated && self.online && self.rooms_ready
    }
}

/// Noticer 与 GUI 共用的 Bridge 就绪及房间目录快照。
#[derive(Debug, Default)]
pub struct BridgeRegistry {
    /// bridge 标识（`IcaBridge::key`）→ 运行时状态。
    ///
    /// 用 `BTreeMap` 而非 `HashMap`，让 `/status` 输出顺序稳定、便于人工比对。
    bridges: RwLock<BTreeMap<String, BridgeRuntimeState>>,
}

impl BridgeRegistry {
    /// 用给定 bridge 标识集合构造注册表，初始状态均为「未就绪」。
    pub fn new(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            bridges: RwLock::new(
                keys.into_iter()
                    .map(|key| (key, BridgeRuntimeState::default()))
                    .collect(),
            ),
        }
    }

    /// 消费一个 Bridge 事件并更新对应 bridge 的状态。
    ///
    /// 只识别与就绪判定相关的事件名：未知事件被忽略（不视为错误），
    /// 未在注册表中的 bridge 同样安全跳过。
    pub fn observe(&self, event: &BridgeEvent) {
        let mut bridges = self.bridges.write().expect("Noticer Bridge 状态锁被污染");
        let Some(state) = bridges.get_mut(&event.bridge_key) else {
            return;
        };
        match event.name() {
            "authSucceed" => state.authenticated = true,
            "authFailed"
            | "socketDisconnected"
            | "socketConnectFailed"
            | "socketReconnectExhausted" => {
                state.authenticated = false;
                state.online = false;
            }
            "onlineData" | "setOnline" => state.online = true,
            "setOffline" | "requestSetup" | "fatal" => state.online = false,
            "setAllRooms" => {
                let rooms = event
                    .payload()
                    .as_array()
                    .and_then(|values| values.first())
                    .and_then(JsonValue::as_array);
                if let Some(rooms) = rooms {
                    state.rooms = rooms
                        .iter()
                        .filter_map(|room| room.get("roomId").and_then(JsonValue::as_i64))
                        .collect();
                    state.rooms_ready = true;
                }
            }
            "updateRoom" => {
                if let Some(room_id) = event
                    .payload()
                    .as_array()
                    .and_then(|values| values.first())
                    .and_then(|room| room.get("roomId"))
                    .and_then(JsonValue::as_i64)
                {
                    state.rooms.insert(room_id);
                }
            }
            _ => {}
        }
    }

    /// 是否存在任一 bridge 满足就绪条件，供 `/health` 快速探测。
    fn any_ready(&self) -> bool {
        self.bridges
            .read()
            .expect("Noticer Bridge 状态锁被污染")
            .values()
            .any(BridgeRuntimeState::ready)
    }

    /// 查询「某个 bridge 的某个房间」当前是否可发送。
    ///
    /// 依次判断 bridge 是否存在、是否就绪、房间是否在当前会话中；
    /// 调用方按返回的 [`TargetStatus`] 选择 HTTP 状态码。
    fn target_status(&self, bridge: &str, room_id: i64) -> TargetStatus {
        let bridges = self.bridges.read().expect("Noticer Bridge 状态锁被污染");
        let Some(state) = bridges.get(bridge) else {
            return TargetStatus::BridgeMissing;
        };
        if !state.ready() {
            return TargetStatus::NotReady;
        }
        if !state.rooms.contains(&room_id) {
            return TargetStatus::RoomMissing;
        }
        TargetStatus::Ready
    }

    /// 生成 `/status` 中的 `bridges` 段 JSON（按 bridge 标识排序，便于比对）。
    fn status_json(&self) -> JsonValue {
        let bridges = self.bridges.read().expect("Noticer Bridge 状态锁被污染");
        JsonValue::Object(
            bridges
                .iter()
                .map(|(name, state)| {
                    (
                        name.clone(),
                        json!({
                            "authenticated": state.authenticated,
                            "online": state.online,
                            "rooms_ready": state.rooms_ready,
                            "room_count": state.rooms.len(),
                            "ready": state.ready(),
                        }),
                    )
                })
                .collect(),
        )
    }
}

/// 发送目标的就绪状态，决定请求被接受还是快速失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetStatus {
    /// bridge 已就绪且房间在当前会话中，可以发送。
    Ready,
    /// 配置里没有这个 bridge 标识。
    BridgeMissing,
    /// bridge 存在但尚未完成鉴权 / 不在线 / 房间目录未同步。
    NotReady,
    /// bridge 已就绪，但目标房间不在当前会话（机器人可能还没进群）。
    RoomMissing,
}

/// 一条发送请求的目标：哪个 bridge、哪个房间、用什么标签回显。
#[derive(Debug, Clone)]
struct Target {
    /// 目标 bridge 标识，对应 `IcaBridge::key`。
    bridge: String,
    /// 目标房间（群 / 私聊）ID。
    room_id: i64,
    /// 回显用的房间标签：配置房间名或 `room_{id}` 形式。
    label: String,
}

/// 请求体解析成功后的中间结果：目标、待发送 payload 与附加元数据。
#[derive(Debug, Clone)]
struct ParsedSend {
    /// 解析出的发送目标。
    target: Target,
    /// 交给 Bridge 的发送载荷（房间、文本、图片、是否作表情）。
    payload: NoticerSendPayload,
    /// 图片字节总数，入队时计入队列字节预算。
    image_bytes: usize,
    /// 是否走了旧版 `/send` 里已废弃的 `room_id` 直写路径（成功时打废弃响应头）。
    deprecated_direct: bool,
}

/// 一次发送尝试的最终结果：HTTP 状态码 + 面向调用方的细节说明。
#[derive(Debug, Clone)]
struct JobOutcome {
    /// 最终返回给客户端的 HTTP 状态码。
    status: StatusCode,
    /// 人类可读的细节，同时参与错误码推断（如是否队列问题）。
    detail: String,
}

/// 发送任务在队列中的生命周期。
#[derive(Debug)]
enum JobPhase {
    /// 已入队，等待 worker 领取。
    Queued,
    /// worker 已领取并正在发送。
    Started,
    /// 已完成，携带最终结果。
    Completed(JobOutcome),
    /// 排队阶段超时被取消；尚未发送，消息不会发出。
    Cancelled,
}

/// 一个待发送任务，跨 HTTP 处理协程与 worker 共享。
#[derive(Debug)]
struct SendJob {
    /// 请求标识，用于日志关联与幂等命中时复用。
    request_id: String,
    /// 发送目标。
    target: Target,
    /// 待发送载荷；`Option` 因为 worker 取出后需转移所有权，只允许取一次。
    payload: Mutex<Option<NoticerSendPayload>>,
    /// 该任务占用的图片字节数，出队时归还预算。
    image_bytes: usize,
    /// 当前生命周期阶段，驱动等待方的提前返回。
    phase: Mutex<JobPhase>,
    /// 阶段变化通知：worker 每完成一步就唤醒等待中的请求协程。
    notify: Notify,
}

impl SendJob {
    /// 用解析结果构造任务，初始阶段为 `Queued`。
    fn new(request_id: String, parsed: ParsedSend) -> Self {
        Self {
            request_id,
            target: parsed.target,
            payload: Mutex::new(Some(parsed.payload)),
            image_bytes: parsed.image_bytes,
            phase: Mutex::new(JobPhase::Queued),
            notify: Notify::new(),
        }
    }
}

/// 队列的运行时指标（当前只有待发送图片字节数）。
#[derive(Debug)]
struct QueueMetrics {
    /// 已入队但尚未发送完成的图片字节总量。
    queued_image_bytes: usize,
}

/// 有界发送队列：同时限制任务个数与图片字节总量。
#[derive(Debug)]
struct Dispatcher {
    /// 任务通道；容量即配置的 `queue_capacity`。
    tx: mpsc::Sender<Arc<SendJob>>,
    /// 任务个数上限（快照，用于计算已用深度）。
    capacity: usize,
    /// 图片字节总量上限（来自 `max_queued_image_bytes`）。
    image_byte_capacity: usize,
    /// 字节计数与任务计数共享的互斥状态。
    metrics: Mutex<QueueMetrics>,
}

impl Dispatcher {
    /// 尝试入队：先占字节预算，再尝试发送到通道；任一步失败都会回滚计数。
    ///
    /// 返回 `Err` 时消息为英文短语，由调用方转成 503 响应。
    fn enqueue(&self, job: Arc<SendJob>) -> Result<(), &'static str> {
        let mut metrics = self.metrics.lock().expect("Noticer 队列状态锁被污染");
        if metrics.queued_image_bytes + job.image_bytes > self.image_byte_capacity {
            return Err("send queue is full");
        }
        metrics.queued_image_bytes += job.image_bytes;
        if self.tx.try_send(job.clone()).is_err() {
            metrics.queued_image_bytes -= job.image_bytes;
            return Err("send queue is full");
        }
        Ok(())
    }

    /// 任务出队（完成或取消）后归还图片字节预算；用饱和减法防止重复归还冲穿下限。
    fn release(&self, image_bytes: usize) {
        let mut metrics = self.metrics.lock().expect("Noticer 队列状态锁被污染");
        metrics.queued_image_bytes = metrics.queued_image_bytes.saturating_sub(image_bytes);
    }

    /// 返回 `(队列是否可用, 当前深度, 待发送图片字节数)`，供 `/ready` 与 `/status` 使用。
    fn status(&self) -> (bool, usize, usize) {
        let image_bytes = self
            .metrics
            .lock()
            .expect("Noticer 队列状态锁被污染")
            .queued_image_bytes;
        (
            !self.tx.is_closed(),
            self.capacity.saturating_sub(self.tx.capacity()),
            image_bytes,
        )
    }
}

/// 一条幂等记录：请求指纹 → 已创建的任务，附过期时间。
#[derive(Debug, Clone)]
struct IdempotencyEntry {
    /// 请求指纹（路径 + 目标 + 内容 + 图片的 SHA-256），用于识别「同键不同内容」。
    fingerprint: [u8; 32],
    /// 首次请求创建的任务；重放请求直接等待同一任务，不重复发送。
    job: Arc<SendJob>,
    /// 过期时刻；每次命中后续期，惰性清理时移除。
    expires_at: Instant,
}

/// HTTP 处理协程之间共享的只读状态（配置、Bridge 句柄、注册表、队列、幂等表）。
#[derive(Clone)]
struct NoticerState {
    /// 生效配置（快照，运行期内变更需重启服务生效）。
    config: Arc<NoticerConfig>,
    /// bridge 标识 → 命令通道，用于把发送命令投给对应 Bridge。
    handles: Arc<HashMap<String, BridgeHandle>>,
    /// Bridge 就绪与房间目录注册表。
    registry: Arc<BridgeRegistry>,
    /// 有界发送队列。
    dispatcher: Arc<Dispatcher>,
    /// 幂等键 → 记录；受独立互斥锁保护。
    idempotency: Arc<Mutex<HashMap<String, IdempotencyEntry>>>,
}

/// 三种端点语义，决定鉴权方式、字段白名单与响应体结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendMode {
    /// 旧版 `/send`：宽容解析，错误响应为 `{ "error": ... }`，不支持幂等键。
    Legacy,
    /// 严格 `/v1/send`：按配置房间名路由，支持幂等键与结构化错误。
    Strict,
    /// `/v1/send/direct`：直接给 `room_id`，需独立 Token，走默认 bridge。
    Direct,
}

impl SendMode {
    /// 是否为旧版模式（影响错误响应体与幂等键支持）。
    fn legacy(self) -> bool {
        self == Self::Legacy
    }
}

/// 可转换为 HTTP 响应的 API 错误；同时承载旧版与严格两种错误体结构。
#[derive(Debug)]
struct ApiError {
    /// HTTP 状态码。
    status: StatusCode,
    /// 机器可读错误码（严格模式 `error.code`）。
    code: &'static str,
    /// 人类可读信息。
    message: String,
    /// 请求标识；严格模式下回显，便于客户端对照日志。
    request_id: Option<String>,
    /// 是否按旧版结构输出（`{ "error": message }`）。
    legacy: bool,
    /// 需要附加到响应的额外头部（如 `WWW-Authenticate`、`Retry-After`）。
    headers: Box<HeaderMap>,
}

impl ApiError {
    /// 构造基础错误：默认无请求标识、非旧版、无附加头部。
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            request_id: None,
            legacy: false,
            headers: Box::new(HeaderMap::new()),
        }
    }

    /// 绑定请求标识并声明是否按旧版结构输出（建造者模式，便于链式调用）。
    fn request(mut self, request_id: &str, legacy: bool) -> Self {
        self.request_id = Some(request_id.to_string());
        self.legacy = legacy;
        self
    }

    /// 附加一个静态响应头（建造者模式）。
    fn header(mut self, name: HeaderName, value: &'static str) -> Self {
        self.headers.insert(name, HeaderValue::from_static(value));
        self
    }

    /// 把错误转成 JSON 响应：旧版只给 `error` 文本，严格版给 `code` / `message` / `request_id`。
    fn response(self) -> Response {
        let body = if self.legacy {
            json!({ "error": self.message })
        } else {
            let mut body = json!({
                "status": "error",
                "error": { "code": self.code, "message": self.message },
            });
            if let Some(request_id) = self.request_id {
                body["request_id"] = json!(request_id);
            }
            body
        };
        json_response(self.status, body, *self.headers)
    }
}

/// Noticer 控制器：跨线程发送「应用配置」命令，用于运行期启停 HTTP 服务。
#[derive(Clone)]
pub struct NoticerController {
    /// 控制命令通道；`Clone` 出的多个控制器共享同一服务任务。
    command_tx: mpsc::UnboundedSender<NoticerControlMessage>,
}

/// 控制器可下达的命令。
enum NoticerControlMessage {
    /// 用新配置（重新）启动服务：先停旧服务，`enabled` 为 true 时再起新的。
    Apply {
        /// 最新 Noticer 配置。
        config: NoticerConfig,
        /// 最新 bridge 命令通道表。
        handles: HashMap<String, BridgeHandle>,
        /// Bridge 就绪注册表（由 GUI 侧持续更新）。
        registry: Arc<BridgeRegistry>,
    },
}

/// 正在运行的服务句柄：停止信号 + 服务任务，供控制器等待其退出。
struct RunningNoticer {
    /// 发送 `true` 触发优雅关闭。
    stop_tx: watch::Sender<bool>,
    /// 服务任务句柄；`await` 它确保旧端口已释放后再起新服务。
    task: tokio::task::JoinHandle<()>,
}

impl NoticerController {
    /// 应用一份新配置（幂等）：停掉旧服务并按需启动新服务。
    ///
    /// 只发送命令不等待完成，因此调用方（GUI 线程）不会被阻塞。
    pub fn apply(
        &self,
        config: NoticerConfig,
        handles: HashMap<String, BridgeHandle>,
        registry: Arc<BridgeRegistry>,
    ) {
        let _ = self.command_tx.send(NoticerControlMessage::Apply {
            config,
            handles,
            registry,
        });
    }
}

/// 启动 Noticer 控制器。控制器允许在客户端运行期间启停 HTTP 服务。
///
/// 在传入的 tokio [`Runtime`] 上派生一个控制任务：收到 `Apply` 时先等待旧服务
/// 完全退出（避免端口占用），再按 `config.enabled` 决定是否启动新服务。
/// 返回的 [`NoticerController`] 可跨线程克隆，首次配置会立即被应用。
pub fn spawn(
    runtime: &Runtime,
    config: NoticerConfig,
    handles: HashMap<String, BridgeHandle>,
    registry: Arc<BridgeRegistry>,
) -> NoticerController {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel();
    let controller = NoticerController { command_tx };
    runtime.spawn(async move {
        let mut running: Option<RunningNoticer> = None;
        while let Some(message) = command_rx.recv().await {
            match message {
                NoticerControlMessage::Apply {
                    config,
                    handles,
                    registry,
                } => {
                    if let Some(current) = running.take() {
                        let _ = current.stop_tx.send(true);
                        let _ = current.task.await;
                    }
                    if config.enabled {
                        running = Some(start_service(config, handles, registry));
                    }
                }
            }
        }
        if let Some(current) = running {
            let _ = current.stop_tx.send(true);
            let _ = current.task.await;
        }
    });
    controller.apply(config, handles, registry);
    controller
}

/// 派生服务任务并返回其停止句柄（不等待任务真正开始运行）。
fn start_service(
    config: NoticerConfig,
    handles: HashMap<String, BridgeHandle>,
    registry: Arc<BridgeRegistry>,
) -> RunningNoticer {
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = tokio::spawn(run_service(config, handles, registry, stop_rx));
    RunningNoticer { stop_tx, task }
}

/// 组装并运行一次服务实例：队列、状态、worker 与 HTTP server 同生命周期。
///
/// 两个任务用 `tokio::join!` 并发等待；server 出错只记录日志，
/// 停止信号由 `stop_rx` 广播给两侧。
async fn run_service(
    config: NoticerConfig,
    handles: HashMap<String, BridgeHandle>,
    registry: Arc<BridgeRegistry>,
    stop_rx: watch::Receiver<bool>,
) {
    let (tx, rx) = mpsc::channel(config.queue_capacity);
    let dispatcher = Arc::new(Dispatcher {
        tx,
        capacity: config.queue_capacity,
        image_byte_capacity: config.max_queued_image_bytes,
        metrics: Mutex::new(QueueMetrics {
            queued_image_bytes: 0,
        }),
    });
    let state = NoticerState {
        config: Arc::new(config),
        handles: Arc::new(handles),
        registry,
        dispatcher: dispatcher.clone(),
        idempotency: Arc::new(Mutex::new(HashMap::new())),
    };
    let worker_stop = stop_rx.clone();
    let worker = run_worker(rx, state.clone(), worker_stop);
    let server = run_server(state, stop_rx);
    tokio::join!(worker, async {
        if let Err(error) = server.await {
            tracing::error!(error = %error, "内建 Noticer HTTP 服务停止");
        }
    });
}

/// 绑定监听地址并启动 axum 服务，收到停止信号后优雅关闭。
///
/// 路由表在此集中声明；绑定失败（端口占用等）转为 `Err(String)` 上报。
async fn run_server(state: NoticerState, mut stop_rx: watch::Receiver<bool>) -> Result<(), String> {
    let router = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/status", get(status))
        .route("/config", get(effective_config))
        .route("/send", post(send_legacy))
        .route("/v1/send", post(send_strict))
        .route("/v1/send/direct", post(send_direct))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind((state.config.host.as_str(), state.config.port))
        .await
        .map_err(|error| {
            format!(
                "无法监听 {}:{}: {error}",
                state.config.host, state.config.port
            )
        })?;
    tracing::info!(host = %state.config.host, port = state.config.port, "内建 Noticer HTTP 服务已启动");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = stop_rx.wait_for(|stopped| *stopped).await;
        })
        .await
        .map_err(|error| error.to_string())
}

/// 发送 worker：串行领取队列任务并驱动发送，直到收到停止信号。
///
/// 排队阶段被取消（`wait_for_job` 超时）的任务直接跳过并归还字节预算；
/// 每完成一步都通知等待中的请求协程。
async fn run_worker(
    mut rx: mpsc::Receiver<Arc<SendJob>>,
    state: NoticerState,
    mut stop_rx: watch::Receiver<bool>,
) {
    loop {
        let next_job = tokio::select! {
            _ = stop_rx.wait_for(|stopped| *stopped) => None,
            job = rx.recv() => job,
        };
        let Some(job) = next_job else {
            break;
        };
        let should_run = {
            let mut phase = job.phase.lock().expect("Noticer 任务状态锁被污染");
            match &*phase {
                JobPhase::Cancelled => false,
                JobPhase::Queued => {
                    *phase = JobPhase::Started;
                    true
                }
                _ => false,
            }
        };
        job.notify.notify_waiters();
        if !should_run {
            state.dispatcher.release(job.image_bytes);
            continue;
        }
        let payload = job
            .payload
            .lock()
            .expect("Noticer 任务 payload 锁被污染")
            .take();
        let outcome = match payload {
            Some(payload) => dispatch_with_retry(&state, &job.target, payload).await,
            None => JobOutcome {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                detail: "send payload is unavailable".to_string(),
            },
        };
        *job.phase.lock().expect("Noticer 任务状态锁被污染") = JobPhase::Completed(outcome);
        job.notify.notify_waiters();
        state.dispatcher.release(job.image_bytes);
    }
}

/// 带重试的发送：先预检目标状态，再经 Bridge 命令通道发送并等待结果。
///
/// 重试只覆盖「通道发送失败」与「Bridge 返回错误」；房间缺失、桥未就绪、
/// 结果通道断开与超时都属于终态，不再重试。
/// 超时视为结果未知，向客户端返回 504，避免重复发送造成刷屏。
async fn dispatch_with_retry(
    state: &NoticerState,
    target: &Target,
    payload: NoticerSendPayload,
) -> JobOutcome {
    let Some(handle) = state.handles.get(&target.bridge) else {
        return JobOutcome {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: "client not ready yet".to_string(),
        };
    };
    for attempt in 1..=state.config.retry_attempts {
        match state.registry.target_status(&target.bridge, target.room_id) {
            TargetStatus::RoomMissing => {
                return JobOutcome {
                    status: StatusCode::NOT_FOUND,
                    detail: format!(
                        "room {} not found in current session (bot may not have joined this group)",
                        target.room_id
                    ),
                };
            }
            TargetStatus::Ready => {}
            TargetStatus::BridgeMissing | TargetStatus::NotReady => {
                return JobOutcome {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    detail: "client not ready yet".to_string(),
                };
            }
        }

        let (result_tx, result_rx) = oneshot::channel();
        if handle
            .send(IcaCommand::SendNoticerMessage {
                payload: payload.clone(),
                result_tx,
            })
            .is_err()
        {
            if attempt < state.config.retry_attempts {
                tokio::time::sleep(Duration::from_secs_f64(state.config.retry_delay_seconds)).await;
                continue;
            }
            return JobOutcome {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                detail: "send command channel is closed".to_string(),
            };
        }

        match tokio::time::timeout(
            Duration::from_secs_f64(state.config.send_timeout_seconds),
            result_rx,
        )
        .await
        {
            Ok(Ok(Ok(()))) => {
                return JobOutcome {
                    status: StatusCode::OK,
                    detail: "ok".to_string(),
                };
            }
            Ok(Ok(Err(error))) => {
                if attempt < state.config.retry_attempts {
                    tracing::warn!(bridge = %target.bridge, room_id = target.room_id, attempt, error = %error, "Noticer 发送失败，准备重试");
                    tokio::time::sleep(Duration::from_secs_f64(state.config.retry_delay_seconds))
                        .await;
                    continue;
                }
                return JobOutcome {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    detail: error,
                };
            }
            Ok(Err(_)) => {
                return JobOutcome {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    detail: "send result channel is closed".to_string(),
                };
            }
            Err(_) => {
                return JobOutcome {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    detail: "send timeout; outcome unknown".to_string(),
                };
            }
        }
    }
    JobOutcome {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: "send failed".to_string(),
    }
}

/// `GET /`：返回纯文本端点说明（无需鉴权）。
async fn root() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(ROOT_DOC))
        .expect("构造 Noticer 文档响应失败")
}

/// `GET /health`：存活探测，始终 200，附带「是否有 bridge 就绪」。
async fn health(State(state): State<NoticerState>) -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "status": "running",
            "client_ready": state.registry.any_ready(),
        }),
        HeaderMap::new(),
    )
}

/// `GET /ready`：就绪探测，需同时满足「有 bridge 就绪」且「队列未关闭」。
///
/// 未就绪返回 503，便于上游负载均衡或脚本重试。
async fn ready(State(state): State<NoticerState>) -> Response {
    let client_ready = state.registry.any_ready();
    let (queue_ready, _, _) = state.dispatcher.status();
    let ready = client_ready && queue_ready;
    json_response(
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        json!({
            "status": if ready { "ready" } else { "not_ready" },
            "client_ready": client_ready,
            "queue_ready": queue_ready,
        }),
        HeaderMap::new(),
    )
}

/// `GET /status`：运行状态总览（需鉴权）。
///
/// 汇总配置房间（并补齐两个内置房间占位）、bridge 状态、队列深度与限额；
/// 令牌本身永不输出。
async fn status(State(state): State<NoticerState>, request: Request) -> Response {
    if let Err(error) = authorize(request.headers(), &state.config.auth_token, true) {
        return error.response();
    }
    let mut rooms = Map::new();
    for (name, room) in &state.config.rooms {
        rooms.insert(name.clone(), room_status(room, true));
    }
    for (name, description) in [
        ("notice", "提醒房间（用于发送一般性提醒消息）"),
        ("warning", "警告房间（用于发送警告/异常消息）"),
    ] {
        rooms.entry(name.to_string()).or_insert_with(|| {
            json!({
                "room_id": 0,
                "description": description,
                "configured": false,
            })
        });
    }
    let (queue_ready, queue_depth, image_bytes) = state.dispatcher.status();
    json_response(
        StatusCode::OK,
        json!({
            "server": format!("noticer/{API_VERSION}"),
            "status": "running",
            "client_ready": state.registry.any_ready(),
            "rooms": rooms,
            "bridges": state.registry.status_json(),
            "queue": {
                "ready": queue_ready,
                "depth": queue_depth,
                "capacity": state.config.queue_capacity,
                "image_bytes": image_bytes,
                "image_byte_capacity": state.config.max_queued_image_bytes,
            },
            "limits": configured_limits_json(&state.config),
            "config_endpoint": "/config",
        }),
        HeaderMap::new(),
    )
}

/// `GET /config`：返回生效配置与限额（需鉴权），不含任何令牌明文。
async fn effective_config(State(state): State<NoticerState>, request: Request) -> Response {
    if let Err(error) = authorize(request.headers(), &state.config.auth_token, true) {
        return error.response();
    }
    json_response(
        StatusCode::OK,
        effective_config_json(&state.config),
        HeaderMap::new(),
    )
}

/// 生成 `/config` 的 JSON：监听地址、安全开关、投递参数、限额与端点清单。
///
/// `tokens_exposed` 恒为 false，用于向调用方承诺响应不含令牌。
fn effective_config_json(config: &NoticerConfig) -> JsonValue {
    let rooms = config
        .rooms
        .iter()
        .map(|(name, room)| (name.clone(), room_status(room, true)))
        .collect::<Map<String, JsonValue>>();
    json!({
        "server": format!("noticer/{API_VERSION}"),
        "listen": {
            "host": config.host,
            "port": config.port,
        },
        "security": {
            "auth_enabled": !config.auth_token.is_empty(),
            "direct_enabled": !config.direct_token.is_empty(),
            "tokens_exposed": false,
        },
        "delivery": {
            "default_bridge": config.default_bridge,
            "queue_capacity": config.queue_capacity,
            "send_timeout_seconds": config.send_timeout_seconds,
            "retry_attempts": config.retry_attempts,
            "retry_delay_seconds": config.retry_delay_seconds,
        },
        "limits": configured_limits_json(config),
        "hard_limits": hard_limits_json(),
        "supported_image_types": SUPPORTED_IMAGE_TYPES,
        "idempotency_key": {
            "max_length": IDEMPOTENCY_KEY_MAX_LENGTH,
            "printable_ascii_without_spaces": true,
        },
        "rooms": rooms,
        "endpoints": [
            {"method": "GET", "path": "/"},
            {"method": "GET", "path": "/health"},
            {"method": "GET", "path": "/ready"},
            {"method": "GET", "path": "/status"},
            {"method": "GET", "path": "/config"},
            {"method": "POST", "path": "/send"},
            {"method": "POST", "path": "/v1/send"},
            {"method": "POST", "path": "/v1/send/direct"},
        ],
    })
}

/// 把用户可配置的限额序列化为 JSON（与硬上限分开展示，便于排查是谁在限制）。
fn configured_limits_json(config: &NoticerConfig) -> JsonValue {
    json!({
        "max_body_size_bytes": config.max_body_size_bytes,
        "max_image_size_bytes": config.max_image_size_bytes,
        "max_image_count": config.max_image_count,
        "max_total_image_size_bytes": config.max_total_image_size_bytes,
        "max_queued_image_bytes": config.max_queued_image_bytes,
        "idempotency_ttl_seconds": config.idempotency_ttl_seconds,
        "idempotency_max_entries": config.idempotency_max_entries,
    })
}

/// 把编译期硬上限序列化为 JSON，供调用方了解服务的天花板。
fn hard_limits_json() -> JsonValue {
    json!({
        "max_body_size_bytes": HARD_MAX_BODY_SIZE,
        "max_image_size_bytes": HARD_MAX_IMAGE_SIZE,
        "max_image_count": HARD_MAX_IMAGE_COUNT,
        "max_total_image_size_bytes": HARD_MAX_TOTAL_IMAGE_SIZE,
        "max_queued_image_bytes": HARD_MAX_QUEUED_IMAGE_BYTES,
        "max_queue_capacity": HARD_MAX_QUEUE_CAPACITY,
        "max_send_timeout_seconds": HARD_MAX_SEND_TIMEOUT_SECONDS,
        "max_retry_attempts": HARD_MAX_RETRY_ATTEMPTS,
        "max_retry_delay_seconds": HARD_MAX_RETRY_DELAY_SECONDS,
        "max_idempotency_ttl_seconds": HARD_MAX_IDEMPOTENCY_TTL_SECONDS,
        "max_idempotency_entries": HARD_MAX_IDEMPOTENCY_ENTRIES,
    })
}

/// 单个房间在 `/status`、`/config` 中的展示结构。
fn room_status(room: &NoticerRoom, configured: bool) -> JsonValue {
    json!({
        "room_id": room.room_id,
        "description": room.description,
        "configured": configured,
        "bridge": room.bridge,
    })
}

/// `POST /send`：旧版端点，转 [`send_handler`] 的 `Legacy` 模式。
async fn send_legacy(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Legacy).await
}

/// `POST /v1/send`：严格端点，按配置房间名路由，支持幂等键。
async fn send_strict(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Strict).await
}

/// `POST /v1/send/direct`：直接指定 `room_id` 的端点，需 `direct_token` 鉴权。
async fn send_direct(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Direct).await
}

/// 发送端点的统一处理流程：鉴权 → 校验 → 解析 → 预检 → 入队 → 等待结果。
///
/// 顺序即优先级：先拒绝未授权与错误 Content-Type，再按 `Content-Length`
/// 预筛过大请求（省得把 Body 读完），最后才做业务校验与发送。
/// 请求在 `send_timeout_seconds` 内等待；排队阶段超时会取消任务，
/// 发送阶段超时则返回 504 并告知「结果未知」。
async fn send_handler(state: NoticerState, request: Request, mode: SendMode) -> Response {
    let started_at = Instant::now();
    let mut request_id = next_request_id();
    let legacy = mode.legacy();

    if mode == SendMode::Direct {
        if state.config.direct_token.trim().is_empty() {
            return ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "direct_api_disabled",
                "direct API is disabled",
            )
            .request(&request_id, false)
            .response();
        }
        if let Err(error) = authorize(request.headers(), &state.config.direct_token, false) {
            return error.request(&request_id, false).response();
        }
    } else if let Err(error) = authorize(request.headers(), &state.config.auth_token, true) {
        return error.request(&request_id, legacy).response();
    }

    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if content_type != "application/json" {
        return ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        )
        .request(&request_id, legacy)
        .response();
    }
    let content_length = match request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(length) if length > 0 => length,
        _ => {
            return ApiError::new(StatusCode::BAD_REQUEST, "empty_body", "empty body")
                .request(&request_id, legacy)
                .response();
        }
    };
    if content_length > state.config.max_body_size_bytes {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!(
                "request body too large (max {} bytes)",
                state.config.max_body_size_bytes
            ),
        )
        .request(&request_id, legacy)
        .response();
    }
    let idempotency_key = if legacy {
        None
    } else {
        match validate_idempotency_key(request.headers()) {
            Ok(value) => value,
            Err(message) => {
                return ApiError::new(StatusCode::BAD_REQUEST, "invalid_idempotency_key", message)
                    .request(&request_id, false)
                    .response();
            }
        }
    };
    let path = request.uri().path().to_string();
    let body = match to_bytes(request.into_body(), state.config.max_body_size_bytes).await {
        Ok(body) if body.len() == content_length => body,
        Ok(_) => {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                "incomplete_body",
                "incomplete request body",
            )
            .request(&request_id, legacy)
            .response();
        }
        Err(_) => {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "failed to read request body",
            )
            .request(&request_id, legacy)
            .response();
        }
    };
    let data: JsonValue = match serde_json::from_slice(&body) {
        Ok(JsonValue::Object(data)) => JsonValue::Object(data),
        Ok(_) => {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "JSON body must be an object",
            )
            .request(&request_id, legacy)
            .response();
        }
        Err(error) => {
            return ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                format!("invalid JSON: {error}"),
            )
            .request(&request_id, legacy)
            .response();
        }
    };
    let parsed = match build_send(&state.config, &data, mode) {
        Ok(parsed) => parsed,
        Err(error) => return error.request(&request_id, legacy).response(),
    };
    match state
        .registry
        .target_status(&parsed.target.bridge, parsed.target.room_id)
    {
        TargetStatus::Ready => {}
        TargetStatus::RoomMissing => {
            return ApiError::new(
                StatusCode::NOT_FOUND,
                "room_not_found",
                format!(
                    "room {} not found in current session (bot may not have joined this group)",
                    parsed.target.room_id
                ),
            )
            .request(&request_id, legacy)
            .response();
        }
        TargetStatus::BridgeMissing | TargetStatus::NotReady => {
            return ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "client_not_ready",
                "bot client not ready yet",
            )
            .request(&request_id, legacy)
            .response();
        }
    }

    let deprecated_direct = parsed.deprecated_direct;
    let fingerprint = send_fingerprint(&path, &parsed);
    let job = if let Some(key) = idempotency_key {
        match idempotent_job(&state, key, fingerprint, &request_id, parsed) {
            Ok(job) => {
                request_id = job.request_id.clone();
                job
            }
            Err(error) => return error.request(&request_id, false).response(),
        }
    } else {
        let job = Arc::new(SendJob::new(request_id.clone(), parsed));
        if let Err(detail) = state.dispatcher.enqueue(job.clone()) {
            return queue_error(detail, &request_id, legacy).response();
        }
        job
    };

    let outcome = wait_for_job(
        &job,
        Duration::from_secs_f64(state.config.send_timeout_seconds),
    )
    .await;
    tracing::info!(
        request_id,
        path,
        bridge = %job.target.bridge,
        room_id = job.target.room_id,
        status = outcome.status.as_u16(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "Noticer 请求完成"
    );
    if outcome.status == StatusCode::OK {
        let mut headers = HeaderMap::new();
        if deprecated_direct {
            headers.insert(
                HeaderName::from_static("deprecation"),
                HeaderValue::from_static("true"),
            );
            headers.insert(
                HeaderName::from_static("link"),
                HeaderValue::from_static("</v1/send/direct>; rel=\"successor-version\""),
            );
        }
        let response = match mode {
            SendMode::Legacy => json!({ "status": "ok", "room": job.target.label }),
            SendMode::Strict => json!({
                "status": "ok",
                "request_id": request_id,
                "room": job.target.label,
            }),
            SendMode::Direct => json!({
                "status": "ok",
                "request_id": request_id,
                "room_id": job.target.room_id,
            }),
        };
        return json_response(StatusCode::OK, response, headers);
    }
    outcome_error(outcome, &request_id, legacy).response()
}

/// 把已通过「必须是 JSON 对象」校验的请求体解析为 [`ParsedSend`]。
///
/// 非旧版模式先做未知字段白名单校验；目标解析按模式分流：
/// `Direct` 只认 `room_id`，`Legacy` 在带 `room_id` 时走废弃直写路径，
/// 其余按配置房间名查找。文本与图片至少要有一项，否则拒绝。
fn build_send(
    config: &NoticerConfig,
    value: &JsonValue,
    mode: SendMode,
) -> Result<ParsedSend, ApiError> {
    let object = value.as_object().expect("调用方已验证 JSON 对象");
    let common = [
        "message",
        "image",
        "image_base64",
        "images",
        "image_type",
        "as_sticker",
    ];
    if mode != SendMode::Legacy {
        let route_field = if mode == SendMode::Direct {
            "room_id"
        } else {
            "room"
        };
        let unknown = object
            .keys()
            .filter(|key| key.as_str() != route_field && !common.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "unknown_field",
                format!("unknown field(s): {}", unknown.join(", ")),
            ));
        }
    }

    let (target, deprecated_direct) = match mode {
        SendMode::Direct => {
            let room_id = require_room_id(object.get("room_id"))?;
            (
                Target {
                    bridge: config.default_bridge.clone(),
                    room_id,
                    label: format!("room_{room_id}"),
                },
                false,
            )
        }
        SendMode::Legacy if object.contains_key("room_id") => {
            let room_id = require_room_id(object.get("room_id"))?;
            let label = object
                .get("room")
                .and_then(JsonValue::as_str)
                .filter(|room| !room.trim().is_empty())
                .map(str::trim)
                .map(str::to_string)
                .unwrap_or_else(|| format!("room_{room_id}"));
            (
                Target {
                    bridge: config.default_bridge.clone(),
                    room_id,
                    label,
                },
                true,
            )
        }
        SendMode::Legacy | SendMode::Strict => {
            let room_name = object
                .get("room")
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|room| !room.is_empty())
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "room_required",
                        format!(
                            "`room` is required (available rooms: {})",
                            config.rooms.keys().cloned().collect::<Vec<_>>().join(", ")
                        ),
                    )
                })?;
            let room = config.rooms.get(room_name).ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "unknown_room",
                    format!(
                        "unknown room '{room_name}', available: {}",
                        config.rooms.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
            })?;
            (
                Target {
                    bridge: room.bridge.clone(),
                    room_id: room.room_id,
                    label: room_name.to_string(),
                },
                false,
            )
        }
    };

    let message = match object.get("message") {
        None | Some(JsonValue::Null) => String::new(),
        Some(JsonValue::String(message)) => message.clone(),
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_message",
                "`message` must be a string",
            ));
        }
    };
    let (images, as_sticker) = parse_images(config, object, mode != SendMode::Legacy)?;
    if message.is_empty() && images.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "content_required",
            "`message`, `image`, or `images` is required",
        ));
    }
    let image_bytes = images.iter().map(|image| image.data.len()).sum();
    Ok(ParsedSend {
        target: target.clone(),
        payload: NoticerSendPayload {
            room_id: target.room_id,
            content: message,
            images,
            as_sticker,
        },
        image_bytes,
        deprecated_direct,
    })
}

/// 从 JSON 值中取 `room_id`，缺失或不是整数时返回 `invalid_room_id` 错误。
fn require_room_id(value: Option<&JsonValue>) -> Result<i64, ApiError> {
    value.and_then(JsonValue::as_i64).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_room_id",
            "`room_id` must be an integer",
        )
    })
}

/// 解析图片字段，返回 `(图片列表, 是否全部作表情发送)`。
///
/// 两种互斥写法：`images` 数组（严格模式下每项还要过字段白名单）或单个
/// `image` / `image_base64` 字段；两者混用、`image` 与 `image_base64` 并用都会报错。
/// `strict` 为 false 时放宽对象字段与 MIME 别名，保持旧版兼容。
fn parse_images(
    config: &NoticerConfig,
    object: &Map<String, JsonValue>,
    strict: bool,
) -> Result<(Vec<NoticerImage>, bool), ApiError> {
    if let Some(images) = object.get("images") {
        let conflicts = ["image", "image_base64", "image_type", "as_sticker"]
            .into_iter()
            .filter(|field| object.contains_key(*field))
            .collect::<Vec<_>>();
        if !conflicts.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "conflicting_image_fields",
                format!("`images` cannot be combined with: {}", conflicts.join(", ")),
            ));
        }
        let values = images.as_array().ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_images",
                "`images` must be an array",
            )
        })?;
        if values.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_images",
                "`images` must contain at least one image",
            ));
        }
        if values.len() > config.max_image_count {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_images",
                format!("too many images (max {})", config.max_image_count),
            ));
        }
        let mut parsed = Vec::with_capacity(values.len());
        let mut total = 0;
        for (index, value) in values.iter().enumerate() {
            let image = parse_image_value(config, value, None, true, false).map_err(|error| {
                ApiError::new(
                    error.status,
                    error.code,
                    format!("images[{index}]: {}", error.message),
                )
            })?;
            total += image.data.len();
            if total > config.max_total_image_size_bytes {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_images",
                    format!(
                        "images total too large (max {} bytes)",
                        config.max_total_image_size_bytes
                    ),
                ));
            }
            parsed.push(image);
        }
        return Ok((parsed, false));
    }

    if object.contains_key("image") && object.contains_key("image_base64") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            "`image` and `image_base64` cannot be combined",
        ));
    }
    let top_mime = object
        .get("image_type")
        .and_then(JsonValue::as_str)
        .unwrap_or("image/png");
    let top_sticker = match object.get("as_sticker") {
        None => false,
        Some(JsonValue::Bool(value)) => *value,
        Some(_) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_image",
                "`as_sticker` must be a boolean",
            ));
        }
    };
    let value = object.get("image").or_else(|| object.get("image_base64"));
    let Some(value) = value else {
        return Ok((Vec::new(), false));
    };
    let mut image = parse_image_value(config, value, Some(top_mime), strict, true)?;
    let object_sticker = value
        .as_object()
        .and_then(|value| value.get("as_sticker"))
        .and_then(JsonValue::as_bool);
    let as_sticker = object_sticker.unwrap_or(top_sticker);
    if as_sticker {
        image.mime = normalize_image_type(&image.mime)?;
    }
    Ok((vec![image], as_sticker))
}

/// 解析单个图片值：支持 data URL、裸 base64 字符串或对象三种形态。
///
/// 解析出的字节必须通过三道校验：非空、不超过单图上限、
/// 且按文件头探测出的真实类型与声明 MIME 一致（防止改扩展名浑水摸鱼）。
fn parse_image_value(
    config: &NoticerConfig,
    value: &JsonValue,
    fallback_mime: Option<&str>,
    strict: bool,
    allow_sticker: bool,
) -> Result<NoticerImage, ApiError> {
    let (base64_value, mime) = match value {
        JsonValue::String(value) => {
            let trimmed = value.trim();
            if trimmed
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
            {
                let rest = &trimmed[5..];
                let marker = rest.to_ascii_lowercase().find(";base64,").ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_image",
                        "invalid image data URL",
                    )
                })?;
                let mime = &rest[..marker];
                let data = &rest[marker + ";base64,".len()..];
                (data, mime)
            } else {
                (trimmed, fallback_mime.unwrap_or("image/png"))
            }
        }
        JsonValue::Object(object) => {
            if strict {
                let allowed = if allow_sticker {
                    &["base64", "type", "as_sticker"][..]
                } else {
                    &["base64", "type"][..]
                };
                let unknown = object
                    .keys()
                    .filter(|key| !allowed.contains(&key.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                if !unknown.is_empty() {
                    return Err(ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_image",
                        format!("unknown image field(s): {}", unknown.join(", ")),
                    ));
                }
            }
            let data = ["base64", "data", "content"]
                .into_iter()
                .filter(|_| !strict)
                .find_map(|key| object.get(key).and_then(JsonValue::as_str))
                .or_else(|| object.get("base64").and_then(JsonValue::as_str))
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "invalid_image",
                        "image object requires string `base64`",
                    )
                })?;
            let mime = ["type", "mime", "file_type"]
                .into_iter()
                .filter(|key| !strict || *key == "type")
                .find_map(|key| object.get(key).and_then(JsonValue::as_str))
                .or(fallback_mime)
                .unwrap_or("image/png");
            (data, mime)
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_image",
                "image must be a base64 string or object",
            ));
        }
    };
    let mime = normalize_image_type(mime)?;
    let compact_base64 = base64_value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let data = base64::engine::general_purpose::STANDARD
        .decode(compact_base64)
        .map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_image",
                "invalid image base64",
            )
        })?;
    if data.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            "image is empty",
        ));
    }
    if data.len() > config.max_image_size_bytes {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            format!(
                "image too large (max {} bytes)",
                config.max_image_size_bytes
            ),
        ));
    }
    let detected_mime = detect_image_type(&data).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            "unsupported or malformed image data",
        )
    })?;
    if detected_mime != mime {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            format!("image MIME mismatch: declared {mime}, detected {detected_mime}"),
        ));
    }
    Ok(NoticerImage {
        mime,
        data: Arc::from(data),
    })
}

/// 按文件头识别图片类型；无法识别时返回 `None`（调用方按不支持处理）。
///
/// 只认魔数不信任扩展名或声明，与 `parse_image_value` 的 MIME 校验配合。
fn detect_image_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// 规范化 MIME：去空白、转小写，把 `image/jpg` 归一为 `image/jpeg`，
/// 并确保落在 [`SUPPORTED_IMAGE_TYPES`] 白名单内。
fn normalize_image_type(mime: &str) -> Result<String, ApiError> {
    let mime = mime.trim().to_ascii_lowercase();
    let mime = if mime == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        mime
    };
    if !SUPPORTED_IMAGE_TYPES.contains(&mime.as_str()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_image",
            format!("unsupported image type: {mime}"),
        ));
    }
    Ok(mime)
}

/// 校验 `Authorization: Bearer <token>`。
///
/// `optional` 为 true 且服务端未配置令牌时直接放行（关闭鉴权）；
/// 否则用常量时间比较避免时序侧信道，失败统一返回 401 并带 `WWW-Authenticate`。
fn authorize(headers: &HeaderMap, expected: &str, optional: bool) -> Result<(), ApiError> {
    if optional && expected.is_empty() {
        return Ok(());
    }
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty())
        .map(|(_, token)| token.trim());
    if supplied.is_some_and(|supplied| constant_time_eq(supplied.as_bytes(), expected.as_bytes())) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "valid Bearer token required",
    )
    .header(WWW_AUTHENTICATE, "Bearer"))
}

/// 常量时间字节比较：先比长度，再按位累积差异，避免逐字节提前返回泄露信息。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

/// 校验 `Idempotency-Key` 请求头：1..=128 字符、可打印 ASCII 且不含空白。
///
/// 未携带该头时返回 `Ok(None)`（不启用幂等）；格式错误返回中文可读的失败原因。
fn validate_idempotency_key(headers: &HeaderMap) -> Result<Option<String>, String> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| "Idempotency-Key must use printable ASCII without spaces".to_string())?;
    if !(1..=IDEMPOTENCY_KEY_MAX_LENGTH).contains(&value.len()) {
        return Err(format!(
            "Idempotency-Key must contain 1 to {IDEMPOTENCY_KEY_MAX_LENGTH} characters"
        ));
    }
    if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err("Idempotency-Key must use printable ASCII without spaces".to_string());
    }
    Ok(Some(value.to_string()))
}

/// 计算请求指纹（SHA-256）：路径、bridge、房间、文本、表情标志与每张图片。
///
/// 字段间用 `0` 字节分隔，避免拼接歧义；图片只参与摘要不落日志。
/// 同键但指纹不同视为冲突，拒绝复用。
fn send_fingerprint(path: &str, parsed: &ParsedSend) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(path.as_bytes());
    digest.update([0]);
    digest.update(parsed.target.bridge.as_bytes());
    digest.update([0]);
    digest.update(parsed.target.room_id.to_string().as_bytes());
    digest.update([0]);
    digest.update(parsed.payload.content.as_bytes());
    digest.update([0]);
    digest.update([u8::from(parsed.payload.as_sticker)]);
    for image in &parsed.payload.images {
        digest.update(image.mime.as_bytes());
        digest.update([0]);
        digest.update(&image.data);
    }
    digest.finalize().into()
}

/// 幂等入队：命中未过期记录时返回原任务（重放不重复发送），
/// 指纹不同返回 409；未命中则创建任务、入队并记账。
///
/// 每次访问先惰性清理过期记录，超容量时淘汰最早过期的条目。
fn idempotent_job(
    state: &NoticerState,
    key: String,
    fingerprint: [u8; 32],
    request_id: &str,
    parsed: ParsedSend,
) -> Result<Arc<SendJob>, ApiError> {
    let now = Instant::now();
    let mut entries = state.idempotency.lock().expect("Noticer 幂等状态锁被污染");
    entries.retain(|_, entry| entry.expires_at > now);
    if let Some(entry) = entries.get_mut(&key) {
        if entry.fingerprint != fingerprint {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "idempotency key was already used for another request",
            ));
        }
        entry.expires_at = now + Duration::from_secs(state.config.idempotency_ttl_seconds);
        return Ok(entry.job.clone());
    }
    while entries.len() >= state.config.idempotency_max_entries {
        let oldest = entries
            .iter()
            .min_by_key(|(_, entry)| entry.expires_at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            entries.remove(&oldest);
        } else {
            break;
        }
    }
    let job = Arc::new(SendJob::new(request_id.to_string(), parsed));
    state
        .dispatcher
        .enqueue(job.clone())
        .map_err(|detail| queue_error(detail, request_id, false))?;
    entries.insert(
        key,
        IdempotencyEntry {
            fingerprint,
            job: job.clone(),
            expires_at: now + Duration::from_secs(state.config.idempotency_ttl_seconds),
        },
    );
    Ok(job)
}

/// 等待任务完成：轮询阶段状态 + `Notify` 唤醒，直到完成、取消或超时。
///
/// 超时发生在排队阶段会把任务标记为 `Cancelled`（消息不会发出）；
/// 发生在发送阶段则返回「结果未知」，由调用方决定是否重试。
async fn wait_for_job(job: &Arc<SendJob>, timeout: Duration) -> JobOutcome {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let notified = job.notify.notified();
        {
            let phase = job.phase.lock().expect("Noticer 任务状态锁被污染");
            match &*phase {
                JobPhase::Completed(outcome) => return outcome.clone(),
                JobPhase::Cancelled => {
                    return JobOutcome {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        detail: "queue wait timeout; message was not sent".to_string(),
                    };
                }
                JobPhase::Queued | JobPhase::Started => {}
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            let mut phase = job.phase.lock().expect("Noticer 任务状态锁被污染");
            return match &*phase {
                JobPhase::Queued => {
                    *phase = JobPhase::Cancelled;
                    JobOutcome {
                        status: StatusCode::GATEWAY_TIMEOUT,
                        detail: "queue wait timeout; message was not sent".to_string(),
                    }
                }
                JobPhase::Started => JobOutcome {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    detail: "send timeout; outcome unknown".to_string(),
                },
                JobPhase::Completed(outcome) => outcome.clone(),
                JobPhase::Cancelled => JobOutcome {
                    status: StatusCode::GATEWAY_TIMEOUT,
                    detail: "queue wait timeout; message was not sent".to_string(),
                },
            };
        }
    }
}

/// 队列不可用（满或已关闭）时的 503 错误，附带 `Retry-After: 1` 提示稍后重试。
fn queue_error(detail: &str, request_id: &str, legacy: bool) -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "queue_unavailable", detail)
        .request(request_id, legacy)
        .header(HeaderName::from_static("retry-after"), "1")
}

/// 把 worker 的 [`JobOutcome`] 映射为带错误码的 [`ApiError`]。
///
/// 错误码按状态码 + detail 关键词推断，保证客户端拿到的 `code` 稳定可判。
fn outcome_error(outcome: JobOutcome, request_id: &str, legacy: bool) -> ApiError {
    let code = match outcome.status {
        StatusCode::NOT_FOUND => "room_not_found",
        StatusCode::SERVICE_UNAVAILABLE => {
            if outcome.detail.contains("queue") {
                "queue_unavailable"
            } else {
                "client_not_ready"
            }
        }
        StatusCode::GATEWAY_TIMEOUT if outcome.detail.contains("outcome unknown") => {
            "outcome_unknown"
        }
        StatusCode::GATEWAY_TIMEOUT => "queue_timeout",
        _ => "send_failed",
    };
    ApiError::new(outcome.status, code, outcome.detail).request(request_id, legacy)
}

/// 生成请求标识：纳秒时间戳 + 进程内自增计数，保证同一进程内不重复。
fn next_request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{now:016x}{counter:016x}")
}

/// 统一构造 JSON 响应并附加额外头部（`extend` 保留已存在的同名字段）。
fn json_response(status: StatusCode, value: JsonValue, headers: HeaderMap) -> Response {
    let mut response = (status, axum::Json(value)).into_response();
    response.headers_mut().extend(headers);
    response
}

/// 未匹配路由的 fallback：返回 404 并带上请求路径，便于排查拼写。
async fn not_found(request: Request) -> Response {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {}", request.uri().path()),
    )
    .response()
}

/// 方法不允许（如对 `/health` 发 POST）的 fallback。
///
/// `method` 参数仅用于占位以满足 axum 的提取器签名，故意不参与响应。
async fn method_not_allowed(method: Method) -> Response {
    let _ = method;
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "method not allowed",
    )
    .response()
}
