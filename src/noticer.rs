//! 与 noticer 0.4.2 HTTP API 兼容的内建 webhook 服务。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::config::{NoticerConfig, NoticerRoom};
use crate::ica::{BridgeEvent, BridgeHandle, IcaCommand, NoticerImage, NoticerSendPayload};

pub const API_VERSION: &str = "0.4.2";
pub const HARD_MAX_BODY_SIZE: usize = 512 * 1024 * 1024;
pub const HARD_MAX_IMAGE_SIZE: usize = 64 * 1024 * 1024;
pub const HARD_MAX_IMAGE_COUNT: usize = 32;
pub const HARD_MAX_TOTAL_IMAGE_SIZE: usize = 512 * 1024 * 1024;
pub const HARD_MAX_QUEUED_IMAGE_BYTES: usize = 1024 * 1024 * 1024;
pub const HARD_MAX_QUEUE_CAPACITY: usize = 4096;
pub const HARD_MAX_SEND_TIMEOUT_SECONDS: f64 = 300.0;
pub const HARD_MAX_RETRY_ATTEMPTS: usize = 16;
pub const HARD_MAX_RETRY_DELAY_SECONDS: f64 = 30.0;
pub const HARD_MAX_IDEMPOTENCY_TTL_SECONDS: u64 = 24 * 60 * 60;
pub const HARD_MAX_IDEMPOTENCY_ENTRIES: usize = 65536;
pub const IDEMPOTENCY_KEY_MAX_LENGTH: usize = 128;
pub const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

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

#[derive(Debug, Clone, Default)]
struct BridgeRuntimeState {
    authenticated: bool,
    online: bool,
    rooms_ready: bool,
    rooms: HashSet<i64>,
}

impl BridgeRuntimeState {
    fn ready(&self) -> bool {
        self.authenticated && self.online && self.rooms_ready
    }
}

/// Noticer 与 GUI 共用的 Bridge 就绪及房间目录快照。
#[derive(Debug, Default)]
pub struct BridgeRegistry {
    bridges: RwLock<BTreeMap<String, BridgeRuntimeState>>,
}

impl BridgeRegistry {
    pub fn new(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            bridges: RwLock::new(
                keys.into_iter()
                    .map(|key| (key, BridgeRuntimeState::default()))
                    .collect(),
            ),
        }
    }

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

    fn any_ready(&self) -> bool {
        self.bridges
            .read()
            .expect("Noticer Bridge 状态锁被污染")
            .values()
            .any(BridgeRuntimeState::ready)
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetStatus {
    Ready,
    BridgeMissing,
    NotReady,
    RoomMissing,
}

#[derive(Debug, Clone)]
struct Target {
    bridge: String,
    room_id: i64,
    label: String,
}

#[derive(Debug, Clone)]
struct ParsedSend {
    target: Target,
    payload: NoticerSendPayload,
    image_bytes: usize,
    deprecated_direct: bool,
}

#[derive(Debug, Clone)]
struct JobOutcome {
    status: StatusCode,
    detail: String,
}

#[derive(Debug)]
enum JobPhase {
    Queued,
    Started,
    Completed(JobOutcome),
    Cancelled,
}

#[derive(Debug)]
struct SendJob {
    request_id: String,
    target: Target,
    payload: Mutex<Option<NoticerSendPayload>>,
    image_bytes: usize,
    phase: Mutex<JobPhase>,
    notify: Notify,
}

impl SendJob {
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

#[derive(Debug)]
struct QueueMetrics {
    queued_image_bytes: usize,
}

#[derive(Debug)]
struct Dispatcher {
    tx: mpsc::Sender<Arc<SendJob>>,
    capacity: usize,
    image_byte_capacity: usize,
    metrics: Mutex<QueueMetrics>,
}

impl Dispatcher {
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

    fn release(&self, image_bytes: usize) {
        let mut metrics = self.metrics.lock().expect("Noticer 队列状态锁被污染");
        metrics.queued_image_bytes = metrics.queued_image_bytes.saturating_sub(image_bytes);
    }

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

#[derive(Debug, Clone)]
struct IdempotencyEntry {
    fingerprint: [u8; 32],
    job: Arc<SendJob>,
    expires_at: Instant,
}

#[derive(Clone)]
struct NoticerState {
    config: Arc<NoticerConfig>,
    handles: Arc<HashMap<String, BridgeHandle>>,
    registry: Arc<BridgeRegistry>,
    dispatcher: Arc<Dispatcher>,
    idempotency: Arc<Mutex<HashMap<String, IdempotencyEntry>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendMode {
    Legacy,
    Strict,
    Direct,
}

impl SendMode {
    fn legacy(self) -> bool {
        self == Self::Legacy
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    request_id: Option<String>,
    legacy: bool,
    headers: Box<HeaderMap>,
}

impl ApiError {
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

    fn request(mut self, request_id: &str, legacy: bool) -> Self {
        self.request_id = Some(request_id.to_string());
        self.legacy = legacy;
        self
    }

    fn header(mut self, name: HeaderName, value: &'static str) -> Self {
        self.headers.insert(name, HeaderValue::from_static(value));
        self
    }

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

/// 启动队列 worker 和 HTTP listener。绑定失败只影响 Noticer，不会中止主客户端。
pub fn spawn(
    runtime: &Runtime,
    config: NoticerConfig,
    handles: HashMap<String, BridgeHandle>,
    registry: Arc<BridgeRegistry>,
) {
    if !config.enabled {
        return;
    }
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
    runtime.spawn(run_worker(rx, state.clone()));
    runtime.spawn(async move {
        if let Err(error) = run_server(state).await {
            tracing::error!(error = %error, "内建 Noticer HTTP 服务停止");
        }
    });
}

async fn run_server(state: NoticerState) -> Result<(), String> {
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
        .await
        .map_err(|error| error.to_string())
}

async fn run_worker(mut rx: mpsc::Receiver<Arc<SendJob>>, state: NoticerState) {
    while let Some(job) = rx.recv().await {
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

async fn root() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(ROOT_DOC))
        .expect("构造 Noticer 文档响应失败")
}

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

fn room_status(room: &NoticerRoom, configured: bool) -> JsonValue {
    json!({
        "room_id": room.room_id,
        "description": room.description,
        "configured": configured,
        "bridge": room.bridge,
    })
}

async fn send_legacy(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Legacy).await
}

async fn send_strict(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Strict).await
}

async fn send_direct(State(state): State<NoticerState>, request: Request) -> Response {
    send_handler(state, request, SendMode::Direct).await
}

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

fn require_room_id(value: Option<&JsonValue>) -> Result<i64, ApiError> {
    value.and_then(JsonValue::as_i64).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_room_id",
            "`room_id` must be an integer",
        )
    })
}

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

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

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

fn queue_error(detail: &str, request_id: &str, legacy: bool) -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "queue_unavailable", detail)
        .request(request_id, legacy)
        .header(HeaderName::from_static("retry-after"), "1")
}

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

fn next_request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{now:016x}{counter:016x}")
}

fn json_response(status: StatusCode, value: JsonValue, headers: HeaderMap) -> Response {
    let mut response = (status, axum::Json(value)).into_response();
    response.headers_mut().extend(headers);
    response
}

async fn not_found(request: Request) -> Response {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {}", request.uri().path()),
    )
    .response()
}

async fn method_not_allowed(method: Method) -> Response {
    let _ = method;
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "method not allowed",
    )
    .response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NoticerRoom;
    use axum::http::Request;

    fn config() -> NoticerConfig {
        NoticerConfig {
            enabled: true,
            default_bridge: "main".to_string(),
            rooms: BTreeMap::from([(
                "notice".to_string(),
                NoticerRoom {
                    bridge: "main".to_string(),
                    room_id: -123,
                    description: "提醒".to_string(),
                },
            )]),
            ..NoticerConfig::default()
        }
    }

    fn png_base64() -> String {
        base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n")
    }

    fn state_with_config(config: NoticerConfig) -> (NoticerState, mpsc::Receiver<Arc<SendJob>>) {
        let (tx, rx) = mpsc::channel(config.queue_capacity);
        let dispatcher = Arc::new(Dispatcher {
            tx,
            capacity: config.queue_capacity,
            image_byte_capacity: config.max_queued_image_bytes,
            metrics: Mutex::new(QueueMetrics {
                queued_image_bytes: 0,
            }),
        });
        (
            NoticerState {
                config: Arc::new(config),
                handles: Arc::new(HashMap::new()),
                registry: Arc::new(BridgeRegistry::new(["main".to_string()])),
                dispatcher,
                idempotency: Arc::new(Mutex::new(HashMap::new())),
            },
            rx,
        )
    }

    fn parsed_text(message: &str) -> ParsedSend {
        build_send(
            &config(),
            &json!({"room": "notice", "message": message}),
            SendMode::Strict,
        )
        .unwrap()
    }

    #[test]
    fn named_room_routes_to_configured_bridge() {
        let parsed = build_send(
            &config(),
            &json!({"room": "notice", "message": "ok"}),
            SendMode::Strict,
        )
        .unwrap();
        assert_eq!(parsed.target.bridge, "main");
        assert_eq!(parsed.target.room_id, -123);
    }

    #[test]
    fn direct_route_uses_default_bridge() {
        let parsed = build_send(
            &config(),
            &json!({"room_id": 456, "message": "ok"}),
            SendMode::Direct,
        )
        .unwrap();
        assert_eq!(parsed.target.bridge, "main");
        assert_eq!(parsed.target.room_id, 456);
    }

    #[test]
    fn multi_image_rejects_single_image_fields() {
        let error = build_send(
            &config(),
            &json!({
                "room": "notice",
                "images": [],
                "as_sticker": true,
            }),
            SendMode::Strict,
        )
        .unwrap_err();
        assert_eq!(error.code, "conflicting_image_fields");
    }

    #[test]
    fn configured_image_limits_change_validation() {
        let mut limited = config();
        limited.max_image_count = 1;
        let image = format!("data:IMAGE/PNG;BASE64,{}", png_base64());
        let error = build_send(
            &limited,
            &json!({"room": "notice", "images": [image.clone(), image]}),
            SendMode::Strict,
        )
        .unwrap_err();
        assert!(error.message.contains("max 1"));

        limited.max_image_count = 2;
        limited.max_image_size_bytes = 7;
        let error = build_send(
            &limited,
            &json!({"room": "notice", "image": png_base64()}),
            SendMode::Strict,
        )
        .unwrap_err();
        assert!(error.message.contains("max 7 bytes"));
    }

    #[test]
    fn image_parser_validates_empty_magic_mime_and_legacy_aliases() {
        let cfg = config();
        let empty = build_send(
            &cfg,
            &json!({"room": "notice", "image": ""}),
            SendMode::Legacy,
        )
        .unwrap_err();
        assert_eq!(empty.message, "image is empty");

        let malformed = build_send(
            &cfg,
            &json!({"room": "notice", "image": "aGVsbG8="}),
            SendMode::Legacy,
        )
        .unwrap_err();
        assert_eq!(malformed.message, "unsupported or malformed image data");

        let mismatch = build_send(
            &cfg,
            &json!({
                "room": "notice",
                "image": {"content": png_base64(), "mime": "image/jpeg"}
            }),
            SendMode::Legacy,
        )
        .unwrap_err();
        assert!(mismatch.message.contains("MIME mismatch"));

        let parsed = build_send(
            &cfg,
            &json!({
                "room": "notice",
                "image": {"data": png_base64(), "file_type": "image/png"}
            }),
            SendMode::Legacy,
        )
        .unwrap();
        assert_eq!(parsed.payload.images.len(), 1);
    }

    #[test]
    fn multi_image_requires_items_and_obeys_total_limit() {
        let empty = build_send(
            &config(),
            &json!({"room": "notice", "images": []}),
            SendMode::Strict,
        )
        .unwrap_err();
        assert!(empty.message.contains("at least one"));

        let mut limited = config();
        limited.max_image_count = 2;
        limited.max_image_size_bytes = 8;
        limited.max_total_image_size_bytes = 15;
        let image = png_base64();
        let error = build_send(
            &limited,
            &json!({"room": "notice", "images": [image.clone(), image]}),
            SendMode::Strict,
        )
        .unwrap_err();
        assert!(error.message.contains("max 15 bytes"));
    }

    #[tokio::test]
    async fn configured_body_limit_rejects_request_before_routing() {
        let mut limited = config();
        limited.max_body_size_bytes = 2;
        let (state, _rx) = state_with_config(limited);
        let request = Request::builder()
            .uri("/v1/send")
            .header(CONTENT_TYPE, "application/json")
            .header(CONTENT_LENGTH, "3")
            .body(Body::from("{} "))
            .unwrap();
        let response = send_handler(state, request, SendMode::Strict).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn configured_queue_image_budget_is_enforced() {
        let (tx, _rx) = mpsc::channel(2);
        let dispatcher = Dispatcher {
            tx,
            capacity: 2,
            image_byte_capacity: 7,
            metrics: Mutex::new(QueueMetrics {
                queued_image_bytes: 0,
            }),
        };
        let mut parsed = parsed_text("ok");
        parsed.image_bytes = 8;
        let job = Arc::new(SendJob::new("request".to_string(), parsed));
        assert_eq!(dispatcher.enqueue(job), Err("send queue is full"));
    }

    #[test]
    fn effective_config_exposes_limits_without_tokens() {
        let mut cfg = config();
        cfg.auth_token = "secret-auth-token".to_string();
        cfg.direct_token = "secret-direct-token".to_string();
        cfg.max_image_count = 3;
        let value = effective_config_json(&cfg);
        let serialized = value.to_string();
        assert_eq!(value["limits"]["max_image_count"], 3);
        assert_eq!(
            value["hard_limits"]["max_image_count"],
            HARD_MAX_IMAGE_COUNT
        );
        assert_eq!(value["security"]["tokens_exposed"], false);
        assert!(!serialized.contains("secret-auth-token"));
        assert!(!serialized.contains("secret-direct-token"));
    }

    #[test]
    fn idempotency_key_rejects_spaces() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("has space"));
        assert!(validate_idempotency_key(&headers).is_err());
    }

    #[test]
    fn idempotency_reuses_same_request_and_rejects_conflict() {
        let (state, _rx) = state_with_config(config());
        let first = parsed_text("first");
        let fingerprint = send_fingerprint("/v1/send", &first);
        let first_job = idempotent_job(
            &state,
            "same-key".to_string(),
            fingerprint,
            "request-1",
            first,
        )
        .unwrap();
        let repeated = parsed_text("first");
        let repeated_job = idempotent_job(
            &state,
            "same-key".to_string(),
            fingerprint,
            "request-2",
            repeated,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&first_job, &repeated_job));

        let conflicting = parsed_text("different");
        let conflict_fingerprint = send_fingerprint("/v1/send", &conflicting);
        let error = idempotent_job(
            &state,
            "same-key".to_string(),
            conflict_fingerprint,
            "request-3",
            conflicting,
        )
        .unwrap_err();
        assert_eq!(error.code, "idempotency_conflict");
    }

    #[tokio::test]
    async fn bridge_handle_returns_noticer_submission_result() {
        let mut cfg = config();
        cfg.retry_attempts = 1;
        cfg.send_timeout_seconds = 1.0;
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = Arc::new(BridgeRegistry::new(["main".to_string()]));
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "authSucceed",
            json!([]),
        ));
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "onlineData",
            json!([{}]),
        ));
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "setAllRooms",
            json!([[{"roomId": -123}]]),
        ));
        let (queue_tx, _queue_rx) = mpsc::channel(1);
        let state = NoticerState {
            config: Arc::new(cfg),
            handles: Arc::new(HashMap::from([(
                "main".to_string(),
                BridgeHandle::new("main".to_string(), command_tx),
            )])),
            registry,
            dispatcher: Arc::new(Dispatcher {
                tx: queue_tx,
                capacity: 1,
                image_byte_capacity: 1024,
                metrics: Mutex::new(QueueMetrics {
                    queued_image_bytes: 0,
                }),
            }),
            idempotency: Arc::new(Mutex::new(HashMap::new())),
        };
        let target = Target {
            bridge: "main".to_string(),
            room_id: -123,
            label: "notice".to_string(),
        };
        let payload = parsed_text("ok").payload;
        let dispatch =
            tokio::spawn(async move { dispatch_with_retry(&state, &target, payload).await });
        let command = command_rx.recv().await.unwrap();
        match command {
            IcaCommand::SendNoticerMessage { payload, result_tx } => {
                assert_eq!(payload.room_id, -123);
                result_tx.send(Ok(())).unwrap();
            }
            _ => panic!("收到的不是 Noticer 发送命令"),
        }
        assert_eq!(dispatch.await.unwrap().status, StatusCode::OK);
    }

    #[test]
    fn bridge_registry_requires_auth_online_and_rooms() {
        let registry = BridgeRegistry::new(["main".to_string()]);
        assert_eq!(registry.target_status("main", -123), TargetStatus::NotReady);
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "authSucceed",
            json!([]),
        ));
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "onlineData",
            json!([{}]),
        ));
        registry.observe(&BridgeEvent::from_protocol(
            "main",
            "setAllRooms",
            json!([[{"roomId": -123}]]),
        ));
        assert_eq!(registry.target_status("main", -123), TargetStatus::Ready);
        assert_eq!(
            registry.target_status("main", -456),
            TargetStatus::RoomMissing
        );
    }
}
