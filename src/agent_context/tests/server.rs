use super::*;
use crate::agent_context::{Context, Target};

struct Harness {
    url: String,
    client: reqwest::Client,
    events: mpsc::UnboundedReceiver<AppEvent>,
    stop: watch::Sender<bool>,
    generation: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn new(timeout: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (event_tx, events) = mpsc::unbounded_channel();
        let (stop, stop_rx) = watch::channel(false);
        let generation = Arc::new(AtomicU64::new(0));
        let state = ServiceState {
            config: Arc::new(AgentContextConfig {
                enabled: true,
                auth_token: "fixture-token".into(),
                ..Default::default()
            }),
            generation: generation.clone(),
            expected_generation: 0,
            event_tx,
            ctx: egui::Context::default(),
            stop_rx: stop_rx.clone(),
            slot: Arc::new(Semaphore::new(1)),
            timeout,
        };
        let mut shutdown = stop_rx;
        let task = tokio::spawn(async move {
            axum::serve(listener, router(state))
                .with_graceful_shutdown(async move {
                    let _ = shutdown.changed().await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            events,
            stop,
            generation,
            task,
        }
    }
    fn read(&self) -> tokio::task::JoinHandle<reqwest::Response> {
        let request = self
            .client
            .post(format!("{}/v1/context", self.url))
            .bearer_auth("fixture-token")
            .json(&json!({"mode":"recent"}));
        tokio::spawn(async move { request.send().await.unwrap() })
    }
    async fn next_request(&mut self) -> GuiRequest {
        let event = tokio::time::timeout(Duration::from_secs(2), self.events.recv())
            .await
            .unwrap()
            .unwrap();
        let AppEvent::AgentContext(request) = event else {
            panic!("仅应收到上下文请求");
        };
        request
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.task.abort();
    }
}

#[tokio::test]
async fn unauthorized_browser_and_oversized_requests_never_reach_gui() {
    let mut h = Harness::new(Duration::from_secs(2)).await;
    for (token, origin, expected) in [
        ("", None, StatusCode::UNAUTHORIZED),
        ("wrong", None, StatusCode::UNAUTHORIZED),
        ("fixture-token", Some("null"), StatusCode::FORBIDDEN),
    ] {
        let mut request = h
            .client
            .post(format!("{}/v1/context", h.url))
            .bearer_auth(token)
            .json(&json!({}));
        if let Some(origin) = origin {
            request = request.header("Origin", origin);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
    }
    let response = h
        .client
        .post(format!("{}/v1/context", h.url))
        .bearer_auth("fixture-token")
        .body(" ".repeat(4097))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(matches!(
        h.events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn invalid_query_bounds_and_partial_targets_are_rejected_before_gui() {
    let mut h = Harness::new(Duration::from_secs(2)).await;
    for body in [
        json!({"limit":201}),
        json!({"mode":"selected","limit":1}),
        json!({"target":{"bridge":"fixture"}}),
        json!({"unknown": true}),
    ] {
        let response = h
            .client
            .post(format!("{}/v1/context", h.url))
            .bearer_auth("fixture-token")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
            "invalid_request"
        );
    }
    assert!(matches!(
        h.events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn http_response_waits_for_gui_result_and_enforces_single_flight() {
    let mut h = Harness::new(Duration::from_secs(2)).await;
    let first = h.read();
    let pending = h.next_request().await;
    assert!(!first.is_finished());
    let busy = h.read().await.unwrap();
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(matches!(
        h.events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    pending.finish(Err(ApiError::denied()));
    let response = first.await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["error"]["code"], "user_denied");
    assert!(body.get("data").is_none());
}

#[tokio::test]
async fn expiry_closes_gui_reply_and_late_approval_cannot_deliver() {
    let mut h = Harness::new(Duration::from_millis(100)).await;
    let first = h.read();
    let pending = h.next_request().await;
    let response = first.await.unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(!pending.valid());
    pending.finish(Ok(QueryResult::Contexts {
        contexts: Vec::new(),
    }));
    assert!(matches!(
        h.events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn stopping_listener_cancels_pending_request_and_generation_rejects_old_result() {
    let mut h = Harness::new(Duration::from_secs(2)).await;
    let first = h.read();
    let pending = h.next_request().await;
    h.generation.fetch_add(1, Ordering::SeqCst);
    pending.finish(Ok(QueryResult::Contexts {
        contexts: Vec::new(),
    }));
    assert_eq!(
        first.await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut h = Harness::new(Duration::from_secs(2)).await;
    let first = h.read();
    let pending = h.next_request().await;
    h.stop.send(true).unwrap();
    assert_eq!(
        first.await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!pending.valid());
}

#[tokio::test]
async fn authenticated_directory_uses_gui_projection_without_requesting_bridge() {
    let mut h = Harness::new(Duration::from_secs(2)).await;
    let request = h
        .client
        .get(format!("{}/v1/contexts", h.url))
        .bearer_auth("fixture-token");
    let http = tokio::spawn(async move { request.send().await.unwrap() });
    let pending = h.next_request().await;
    assert!(matches!(pending.operation, Operation::List));
    pending.finish(Ok(QueryResult::Contexts {
        contexts: vec![Context {
            target: Target {
                bridge: "fixture".into(),
                room_id: -10,
            },
            room_name: "测试会话".into(),
        }],
    }));
    let response = http.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["data"]["contexts"][0]["bridge"], "fixture");
    assert!(body["data"]["contexts"][0].get("messages").is_none());
}

#[tokio::test]
async fn bind_failure_is_visible_without_queuing_gui_requests() {
    let occupied = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let config = AgentContextConfig {
        enabled: true,
        port: occupied.local_addr().unwrap().port(),
        auth_token: "fixture-token".into(),
        ..Default::default()
    };
    let (event_tx, mut events) = mpsc::unbounded_channel();
    let (stop, stop_rx) = watch::channel(false);
    let status = Arc::new(RwLock::new(String::new()));
    let state = ServiceState {
        config: Arc::new(config),
        generation: Arc::new(AtomicU64::new(0)),
        expected_generation: 0,
        event_tx,
        ctx: egui::Context::default(),
        stop_rx,
        slot: Arc::new(Semaphore::new(1)),
        timeout: Duration::from_secs(2),
    };
    serve(state, status.clone()).await;
    assert!(status.read().unwrap().starts_with("监听失败"));
    assert!(events.try_recv().is_err());
    drop(stop);
}
