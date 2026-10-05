//! 独立本机监听器。鉴权、容量和超时在进入 GUI 事件队列前执行。
use super::{
    AgentContextConfig, ApiError, MAX_RESPONSE_BYTES, QueryResult, REQUEST_TIMEOUT_SECONDS,
    ReadRequest,
};
use crate::app::event::AppEvent;
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    runtime::Runtime,
    sync::{Semaphore, mpsc, oneshot, watch},
};

#[derive(Debug)]
pub enum Operation {
    List,
    Read(ReadRequest),
}
#[derive(Debug, Clone)]
pub struct Lease {
    pub generation: Arc<AtomicU64>,
    pub expected_generation: u64,
    pub deadline: Instant,
}
impl Lease {
    pub fn valid(&self) -> bool {
        self.generation.load(Ordering::SeqCst) == self.expected_generation
            && Instant::now() < self.deadline
    }
}
pub struct GuiRequest {
    pub id: u64,
    pub operation: Operation,
    pub config: Arc<AgentContextConfig>,
    pub lease: Lease,
    pub result_tx: oneshot::Sender<Result<QueryResult, ApiError>>,
}
impl fmt::Debug for GuiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuiRequest")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl GuiRequest {
    pub fn valid(&self) -> bool {
        self.lease.valid() && !self.result_tx.is_closed()
    }
    pub fn finish(self, result: Result<QueryResult, ApiError>) {
        let result = if self.valid() {
            result
        } else {
            Err(ApiError::cancelled())
        };
        let _ = self.result_tx.send(result);
    }
}

pub struct Controller {
    config_tx: watch::Sender<AgentContextConfig>,
    generation: Arc<AtomicU64>,
    status: Arc<RwLock<String>>,
}
impl Controller {
    pub fn apply(&self, config: AgentContextConfig) {
        if *self.config_tx.borrow() != config {
            // 同步作废旧 GUI 请求，不等待后台服务重建。
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.config_tx.send_replace(config);
        }
    }
    pub fn status(&self) -> String {
        self.status.read().expect("Agent 服务状态锁被污染").clone()
    }
}
#[derive(Clone)]
struct ServiceState {
    config: Arc<AgentContextConfig>,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    ctx: egui::Context,
    stop_rx: watch::Receiver<bool>,
    // 包括目录请求也限流，不能通过只读目录接口淹没 GUI 队列。
    slot: Arc<Semaphore>,
    timeout: Duration,
}
fn set_status(status: &RwLock<String>, ctx: &egui::Context, value: String) {
    *status.write().expect("Agent 服务状态锁被污染") = value;
    ctx.request_repaint();
}
pub fn spawn(
    runtime: &Runtime,
    config: AgentContextConfig,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    ctx: egui::Context,
) -> Controller {
    let (config_tx, mut config_rx) = watch::channel(config);
    let generation = Arc::new(AtomicU64::new(0));
    let status = Arc::new(RwLock::new("未启用".into()));
    let controller = Controller {
        config_tx,
        generation: generation.clone(),
        status: status.clone(),
    };
    runtime.spawn(async move {
        loop {
            let config = config_rx.borrow_and_update().clone();
            let expected_generation = generation.load(Ordering::SeqCst);
            let (stop_tx, stop_rx) = watch::channel(false);
            let running = if config.enabled {
                let state = ServiceState {
                    config: Arc::new(config),
                    generation: generation.clone(),
                    expected_generation,
                    event_tx: event_tx.clone(),
                    ctx: ctx.clone(),
                    stop_rx,
                    slot: Arc::new(Semaphore::new(1)),
                    timeout: Duration::from_secs(REQUEST_TIMEOUT_SECONDS),
                };
                let status = status.clone();
                Some(tokio::spawn(async move { serve(state, status).await }))
            } else {
                set_status(&status, &ctx, "未启用".into());
                None
            };
            let closed = config_rx.changed().await.is_err();
            let _ = stop_tx.send(true);
            if let Some(mut task) = running
                && tokio::time::timeout(Duration::from_secs(2), &mut task)
                    .await
                    .is_err()
            {
                task.abort();
                let _ = task.await;
            }
            if closed {
                break;
            }
        }
    });
    controller
}
async fn serve(state: ServiceState, status: Arc<RwLock<String>>) {
    if let Err(error) = state.config.validate() {
        set_status(&status, &state.ctx, error.to_string());
        return;
    }
    let listener =
        match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, state.config.port))
            .await
        {
            Ok(listener) => listener,
            Err(error) => {
                set_status(&status, &state.ctx, format!("监听失败：{error}"));
                return;
            }
        };
    set_status(
        &status,
        &state.ctx,
        format!("监听 127.0.0.1:{}（仅本机）", state.config.port),
    );
    let mut stop = state.stop_rx.clone();
    let result = axum::serve(listener, router(state.clone()))
        .with_graceful_shutdown(async move {
            if !*stop.borrow() {
                let _ = stop.changed().await;
            }
        })
        .await;
    if result.is_err() {
        set_status(&status, &state.ctx, "监听服务异常停止".into());
    }
}
fn router(state: ServiceState) -> Router {
    Router::new()
        .route("/v1/contexts", get(query))
        .route("/v1/context", post(query))
        .fallback(|| async {
            respond(
                0,
                Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "接口不存在",
                )),
            )
        })
        .method_not_allowed_fallback(|| async {
            respond(
                0,
                Err(ApiError::new(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "method_not_allowed",
                    "不支持的请求方法",
                )),
            )
        })
        .with_state(state)
}
fn authorize(request: &Request, token: &str) -> Result<(), ApiError> {
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "browser_origin_forbidden",
            "不允许浏览器跨域请求",
        ));
    }
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let left = Sha256::digest(provided.as_bytes());
    let right = Sha256::digest(token.as_bytes());
    let mismatch = left
        .iter()
        .zip(right.iter())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b));
    if token.is_empty() || mismatch != 0 {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "需要独立的 Agent 上下文 Token",
        ));
    }
    Ok(())
}
async fn query(State(state): State<ServiceState>, request: Request) -> Response {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if let Err(error) = authorize(&request, &state.config.auth_token) {
        return respond(id, Err(error));
    }
    let Ok(_permit) = state.slot.clone().try_acquire_owned() else {
        return respond(
            id,
            Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "已有读取请求，请完成或取消后再试",
            )),
        );
    };
    let mut stop = state.stop_rx.clone();
    if *stop.borrow() || state.generation.load(Ordering::SeqCst) != state.expected_generation {
        return respond(id, Err(ApiError::cancelled()));
    }
    let started = Instant::now();
    // 接收请求体也受同一超时约束，慢连接不能永久占用唯一读取名额。
    let result = tokio::select! {
        biased;
        _ = stop.changed() => Err(ApiError::cancelled()),
        result = tokio::time::timeout(state.timeout, dispatch(&state, id, request, started)) => {
            result.unwrap_or_else(|_| Err(ApiError::new(StatusCode::REQUEST_TIMEOUT, "request_timeout", "等待选择或授权超时，请由用户决定是否重试")))
        }
    };
    let result = if state.generation.load(Ordering::SeqCst) == state.expected_generation {
        result
    } else {
        Err(ApiError::cancelled())
    };
    let (outcome, count) = match &result {
        Ok(QueryResult::Snapshot(snapshot)) => ("ok", snapshot.returned_count),
        Ok(_) => ("ok", 0),
        Err(error) => (error.code, 0),
    };
    tracing::info!(
        request_id = id,
        outcome,
        count,
        elapsed_ms = started.elapsed().as_millis(),
        "Agent 上下文请求结束"
    );
    state.ctx.request_repaint();
    respond(id, result)
}
async fn dispatch(
    state: &ServiceState,
    id: u64,
    request: Request,
    started: Instant,
) -> Result<QueryResult, ApiError> {
    let operation = if request.uri().path() == "/v1/contexts" {
        Operation::List
    } else {
        let bytes = to_bytes(request.into_body(), 4096).await.map_err(|_| {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "请求体最多 4 KiB",
            )
        })?;
        let params: ReadRequest = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::invalid("请求必须是合法的上下文查询 JSON"))?;
        params.validate()?;
        Operation::Read(params)
    };
    let (result_tx, result_rx) = oneshot::channel();
    let request = GuiRequest {
        id,
        operation,
        config: state.config.clone(),
        result_tx,
        lease: Lease {
            generation: state.generation.clone(),
            expected_generation: state.expected_generation,
            deadline: started + state.timeout,
        },
    };
    state
        .event_tx
        .send(AppEvent::AgentContext(request))
        .map_err(|_| ApiError::cancelled())?;
    state.ctx.request_repaint();
    result_rx.await.map_err(|_| ApiError::cancelled())?
}
fn respond(id: u64, result: Result<QueryResult, ApiError>) -> Response {
    let (status, body) = match result {
        Ok(data) => (StatusCode::OK, json!({"request_id": id, "data": data})),
        Err(error) => (error.status, json!({"request_id": id, "error": error})),
    };
    let bytes = serde_json::to_vec(&body).expect("专用上下文 DTO 必须可序列化");
    if bytes.len() > MAX_RESPONSE_BYTES {
        return respond(id, Err(ApiError::too_large()));
    }
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
