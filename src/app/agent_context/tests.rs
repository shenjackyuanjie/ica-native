use super::*;
use crate::{
    agent_context::{ReadMode, ReadRequest, server::Lease},
    app::{
        runtime::AppRuntime,
        state::{AppState, BridgeSession, BridgeState},
        stickers::StickerStore,
    },
    config::{ChatGroups, ConfigStore, IcaCfg},
    ica::{BridgeHandle, IcaCommand, types::message::Message},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::sync::{mpsc, oneshot};

fn target(bridge: &str, room_id: i64) -> Target {
    Target {
        bridge: bridge.into(),
        room_id,
    }
}
fn message(id: &str, content: &str) -> Message {
    serde_json::from_value(serde_json::json!({"_id": id, "content": content, "senderId": 1, "username": "测试者", "time": 1_700_000_000_000_i64})).unwrap()
}
fn app(allowlist: Vec<Target>) -> (IcaApp, Vec<mpsc::UnboundedReceiver<IcaCommand>>) {
    // 运行时保持关闭，只有测试状态开启权限规则；不监听真实服务端口。
    let runtime_config = IcaCfg::default();
    let mut config = runtime_config.clone();
    config.agent_context = AgentContextConfig {
        enabled: true,
        auth_token: "fixture-token".into(),
        allowlist,
        ..Default::default()
    };
    let store = ConfigStore::from_config(
        config.clone(),
        std::env::temp_dir().join("ica-agent-context-unused.toml"),
    );
    let mut receivers = Vec::new();
    let sessions = ["a", "b"]
        .into_iter()
        .map(|key| {
            let (tx, rx) = mpsc::unbounded_channel();
            receivers.push(rx);
            let (stop, _) = oneshot::channel();
            let mut state = BridgeState::new(key.into(), ChatGroups::default());
            state.conversation_mut(-10).messages = vec![
                message("first", &format!("{key} 的第一条")),
                message("second", &format!("{key} 的第二条")),
            ];
            state.conversation_mut(-10).draft = "不要改变草稿".into();
            state.conversation_mut(-10).message_scroll_offset = Some(42.0);
            state.conversation_mut(-30).messages = vec![message("background", "未打开的缓存")];
            if key == "a" {
                state.selected_room_id = Some(-10);
            } else {
                state.detached_room_ids.insert(-10);
            }
            BridgeSession::new(BridgeHandle::new(key.into(), tx), state, stop)
        })
        .collect();
    let state = AppState::new(
        &config,
        &store,
        sessions,
        StickerStore::unavailable(
            std::env::temp_dir().join("ica-agent-context-unused-stickers"),
            "fixture",
        ),
    );
    (
        IcaApp {
            runtime: AppRuntime::new(&egui::Context::default(), &runtime_config),
            config: store,
            state,
            chat_windows: Vec::new(),
        },
        receivers,
    )
}
fn request(
    app: &IcaApp,
    operation: Operation,
) -> (GuiRequest, oneshot::Receiver<Result<QueryResult, ApiError>>) {
    let (result_tx, rx) = oneshot::channel();
    (
        GuiRequest {
            id: 1,
            operation,
            config: Arc::new(app.config.snapshot().agent_context),
            result_tx,
            lease: Lease {
                generation: Arc::new(AtomicU64::new(0)),
                expected_generation: 0,
                deadline: Instant::now() + Duration::from_secs(120),
            },
        },
        rx,
    )
}
fn read(
    app: &IcaApp,
    target: Option<Target>,
    mode: ReadMode,
) -> (GuiRequest, oneshot::Receiver<Result<QueryResult, ApiError>>) {
    request(
        app,
        Operation::Read(ReadRequest {
            target,
            mode,
            ..Default::default()
        }),
    )
}
fn no_bridge_commands(receivers: &mut [mpsc::UnboundedReceiver<IcaCommand>]) {
    for receiver in receivers {
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[test]
fn direct_allowlist_read_is_bridge_scoped_and_does_not_change_chat_state() {
    let (mut app, mut receivers) = app(vec![target("a", -10), target("b", -10)]);
    app.bridge_states[1].forward_room_id = Some(-10);
    app.bridge_states[1].forward_selected_message_ids = vec!["second".into()];
    let (request, mut result) = read(&app, Some(target("b", -10)), ReadMode::Selected);
    app.handle_agent_context_request(request);
    let QueryResult::Snapshot(snapshot) = result.try_recv().unwrap().unwrap() else {
        panic!("应返回快照");
    };
    assert_eq!(snapshot.context.target, target("b", -10));
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].content, "b 的第二条");
    assert!(app.agent_context_ui.pending.is_none());
    assert_eq!(app.active_bridge_idx, Some(0));
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-10));
    let conversation = app.bridge_states[1].conversation(-10).unwrap();
    assert_eq!(conversation.draft, "不要改变草稿");
    assert_eq!(conversation.message_scroll_offset, Some(42.0));
    assert_eq!(
        app.bridge_states[1].forward_selected_message_ids,
        ["second"]
    );
    no_bridge_commands(&mut receivers);
}

#[test]
fn directory_only_lists_open_allowlisted_targets_and_does_not_open_cached_rooms() {
    let (mut app, mut receivers) = app(vec![target("b", -10), target("a", -30)]);
    let (request, mut result) = request(&app, Operation::List);
    app.handle_agent_context_request(request);
    let QueryResult::Contexts { contexts } = result.try_recv().unwrap().unwrap() else {
        panic!("应返回目录");
    };
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].target, target("b", -10));
    let (request, mut result) = read(&app, Some(target("a", -30)), ReadMode::Recent);
    app.handle_agent_context_request(request);
    assert_eq!(
        result.try_recv().unwrap().unwrap_err().code,
        "target_not_open"
    );
    assert_eq!(app.bridge_states[0].selected_room_id, Some(-10));
    no_bridge_commands(&mut receivers);
}

#[test]
fn non_allowlisted_read_waits_for_consent_and_returns_the_frozen_snapshot() {
    let (mut app, mut receivers) = app(Vec::new());
    let (request, mut result) = read(&app, Some(target("a", -10)), ReadMode::Recent);
    app.handle_agent_context_request(request);
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let ctx = egui::Context::default();
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.render_agent_context(ui.ctx())
    });
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    app.bridge_states[0].conversation_mut(-10).messages[0].content = "批准前的新内容".into();
    app.bridge_states[0].detached_room_ids.insert(-10);
    app.active_bridge_idx = Some(1);
    app.finish_agent_context_consent(true);
    let QueryResult::Snapshot(snapshot) = result.try_recv().unwrap().unwrap() else {
        panic!("应返回快照");
    };
    assert_eq!(snapshot.context.target, target("a", -10));
    assert_eq!(snapshot.messages[0].content, "a 的第一条");
    assert!(app.config.snapshot().agent_context.allowlist.is_empty());
    no_bridge_commands(&mut receivers);
}

#[test]
fn choosing_target_applies_its_own_policy_and_never_guesses_or_remembers_consent() {
    let (mut app, _) = app(vec![target("b", -10)]);
    let (request, mut result) = read(&app, None, ReadMode::Recent);
    app.handle_agent_context_request(request);
    assert!(
        app.agent_context_ui
            .pending
            .as_ref()
            .unwrap()
            .snapshot
            .is_none()
    );
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let pending = app.agent_context_ui.pending.take().unwrap();
    app.choose_agent_context(pending.request, &target("b", -10));
    assert!(result.try_recv().unwrap().is_ok());
    assert!(app.agent_context_ui.pending.is_none());
    let (request, mut result) = read(&app, None, ReadMode::Recent);
    app.handle_agent_context_request(request);
    let pending = app.agent_context_ui.pending.take().unwrap();
    app.choose_agent_context(pending.request, &target("a", -10));
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    app.finish_agent_context_consent(false);
    assert_eq!(result.try_recv().unwrap().unwrap_err().code, "user_denied");
    assert_eq!(
        app.config.snapshot().agent_context.allowlist,
        [target("b", -10)]
    );
}

#[test]
fn closed_expired_disconnected_and_revoked_requests_cannot_be_approved_late() {
    for scenario in [
        "closed",
        "expired",
        "disconnected",
        "generation",
        "disabled",
        "raw_policy",
    ] {
        let (mut app, mut receivers) = app(Vec::new());
        let (request, mut result) = read(&app, Some(target("a", -10)), ReadMode::Recent);
        app.handle_agent_context_request(request);
        match scenario {
            "closed" => app.bridge_states[0].selected_room_id = None,
            "expired" => {
                app.agent_context_ui
                    .pending
                    .as_mut()
                    .unwrap()
                    .request
                    .lease
                    .deadline = Instant::now() - Duration::from_secs(1)
            }
            "generation" => {
                app.agent_context_ui
                    .pending
                    .as_ref()
                    .unwrap()
                    .request
                    .lease
                    .generation
                    .fetch_add(1, Ordering::SeqCst);
            }
            "disabled" => {
                app.config
                    .update(|config| config.agent_context.enabled = false);
                app.sync_agent_context_config();
            }
            "raw_policy" => {
                // 原始配置编辑器可能在本帧更新策略，不能等下一帧同步才撤销。
                app.config.update(|config| {
                    config.agent_context.auth_token = "rotated-fixture-token".into()
                });
            }
            "disconnected" => {
                result.close();
            }
            _ => unreachable!(),
        }
        app.finish_agent_context_consent(true);
        assert!(app.agent_context_ui.pending.is_none(), "{scenario}");
        if scenario != "disconnected" {
            let error = result.try_recv().unwrap().unwrap_err();
            assert_eq!(
                error.code,
                if scenario == "closed" {
                    "target_not_open"
                } else {
                    "request_cancelled"
                }
            );
        }
        no_bridge_commands(&mut receivers);
    }
}

#[test]
fn selection_from_a_different_room_never_becomes_a_recent_read() {
    let (mut app, mut receivers) = app(vec![target("a", -10)]);
    app.bridge_states[0].forward_room_id = Some(-30);
    app.bridge_states[0].forward_selected_message_ids = vec!["first".into()];
    let (request, mut result) = read(&app, Some(target("a", -10)), ReadMode::Selected);
    app.handle_agent_context_request(request);
    assert_eq!(result.try_recv().unwrap().unwrap_err().code, "no_selection");
    no_bridge_commands(&mut receivers);
}

#[test]
fn real_http_to_gui_flow_covers_direct_read_picker_approval_and_denial() {
    let (mut app, mut receivers) = app(vec![target("b", -10)]);
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    app.config.update(|config| config.agent_context.port = port);
    app.sync_agent_context_config();
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !app
        .runtime
        .agent_context_controller
        .status()
        .starts_with("监听 127.0.0.1:")
    {
        assert!(Instant::now() < deadline, "测试服务未及时启动");
        std::thread::sleep(Duration::from_millis(5));
    }
    for scenario in ["direct", "approve", "deny"] {
        let payload = if scenario == "direct" {
            serde_json::json!({"target":{"bridge":"b","room_id":-10},"mode":"recent","limit":1})
        } else {
            serde_json::json!({"mode":"recent","limit":1})
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        app.runtime.spawn(async move {
            let response = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap()
                .post(format!("http://127.0.0.1:{port}/v1/context"))
                .bearer_auth("fixture-token")
                .json(&payload)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.json::<serde_json::Value>().await.unwrap();
            tx.send((status, body)).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let (status, body) = loop {
            app.poll_socketio_events(&ctx);
            if let Ok(result) = rx.try_recv() {
                break result;
            }
            if scenario != "direct"
                && let Some(pending) = app.agent_context_ui.pending.take()
            {
                assert!(pending.snapshot.is_none(), "HTTP 未指定目标，不应自动猜测");
                app.choose_agent_context(pending.request, &target("a", -10));
                assert!(
                    app.agent_context_ui
                        .pending
                        .as_ref()
                        .unwrap()
                        .snapshot
                        .is_some()
                );
                assert!(rx.try_recv().is_err(), "批准前不可返回正文");
                app.finish_agent_context_consent(scenario == "approve");
            }
            assert!(Instant::now() < deadline, "HTTP 到 GUI 往返超时");
            std::thread::sleep(Duration::from_millis(5));
        };
        if scenario == "deny" {
            assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
            assert!(body.get("data").is_none());
            assert_eq!(body["error"]["code"], "user_denied");
        } else {
            assert_eq!(status, axum::http::StatusCode::OK);
            assert_eq!(
                body["data"]["context"]["bridge"],
                if scenario == "direct" { "b" } else { "a" }
            );
            assert_eq!(body["data"]["messages"][0]["message_id"], "second");
            assert_eq!(body["data"]["source"], "loaded_only");
        }
        assert!(app.agent_context_ui.pending.is_none());
    }
    app.config
        .update(|config| config.agent_context.enabled = false);
    app.sync_agent_context_config();
    no_bridge_commands(&mut receivers);
}
