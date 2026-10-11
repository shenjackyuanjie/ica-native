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
    let dispatch = tokio::spawn(async move { dispatch_with_retry(&state, &target, payload).await });
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

#[tokio::test]
async fn noticer_controller_accepts_runtime_enable_and_disable_updates() {
    let registry = Arc::new(BridgeRegistry::default());
    std::thread::spawn(move || {
        let runtime = Runtime::new().unwrap();
        let controller = spawn(
            &runtime,
            NoticerConfig::default(),
            HashMap::new(),
            registry.clone(),
        );

        let enabled = NoticerConfig {
            enabled: true,
            port: 0,
            ..NoticerConfig::default()
        };
        controller.apply(enabled, HashMap::new(), registry.clone());
        std::thread::sleep(Duration::from_millis(80));
        controller.apply(NoticerConfig::default(), HashMap::new(), registry);
        std::thread::sleep(Duration::from_millis(80));
    })
    .join()
    .unwrap();
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
