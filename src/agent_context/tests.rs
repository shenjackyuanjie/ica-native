use super::*;
use serde_json::json;

fn message(index: usize) -> Message {
    serde_json::from_value(json!({"_id": format!("fixture-{index}"), "senderId": 17,
        "username": "测试发送者", "content": format!("正文 {index}"), "time": 1_700_000_000_000_i64 + index as i64})).unwrap()
}
fn context() -> Context {
    Context {
        target: Target {
            bridge: "fixture".into(),
            room_id: -10,
        },
        room_name: "测试会话".into(),
    }
}

#[test]
fn recent_snapshot_limits_loaded_messages_without_reordering_or_mutation() {
    let loaded: Vec<_> = (0..65).map(message).collect();
    let result = snapshot(context(), &ReadRequest::default(), &loaded, &HashSet::new()).unwrap();
    assert_eq!(result.loaded_count, 65);
    assert_eq!(result.messages.len(), 50);
    assert_eq!(result.messages.first().unwrap().message_id, "fixture-15");
    assert_eq!(result.messages.last().unwrap().message_id, "fixture-64");
    assert_eq!(result.source, "loaded_only");
    assert_eq!(loaded[0].content, "正文 0");
}

#[test]
fn selected_snapshot_preserves_chat_order_and_never_falls_back_to_recent() {
    let loaded: Vec<_> = (0..5).map(message).collect();
    let request = ReadRequest {
        mode: ReadMode::Selected,
        ..Default::default()
    };
    let result = snapshot(
        context(),
        &request,
        &loaded,
        &HashSet::from(["fixture-4", "fixture-1"]),
    )
    .unwrap();
    let ids: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.message_id.as_str())
        .collect();
    assert_eq!(ids, ["fixture-1", "fixture-4"]);
    let error = snapshot(context(), &request, &loaded, &HashSet::from(["unknown"])).unwrap_err();
    assert_eq!(error.code, "no_selection");
}

#[test]
fn protected_content_reply_copies_and_download_credentials_do_not_escape() {
    let mut loaded: Vec<_> = (0..4).map(message).collect();
    for m in &mut loaded {
        m.content = "private-body".into();
        m.raw_msg = Some(Box::new(json!({"secret": "raw-protocol"})));
        m.code = json!({"secret": "raw-code"});
        m.reply = Some(crate::ica::types::message::ReplyMessage {
            msg_id: "reply-target".into(),
            content: "reply-private-copy".into(),
            file: None,
            files: Vec::new(),
            sender_name: "被引用者".into(),
        });
        m.files.push(crate::ica::types::files::MessageFile {
            file_type: "image/png".into(),
            name: Some("fixture.png".into()),
            url: "https://example.invalid/?token=download-secret".into(),
            size: None,
            fid: None,
        });
        m.reveal = true;
    }
    loaded[0].deleted = true;
    loaded[1].hide = true;
    loaded[2].flash = true;
    let result = snapshot(context(), &ReadRequest::default(), &loaded, &HashSet::new()).unwrap();
    for m in &result.messages[..3] {
        assert!(!m.content.contains("private-body"));
        assert!(m.attachments.is_empty());
        assert!(m.reply_to.is_none());
    }
    assert_eq!(result.messages[3].content, "private-body");
    assert_eq!(result.messages[3].reply_to.as_deref(), Some("reply-target"));
    assert_eq!(
        result.messages[3].attachments[0].name.as_deref(),
        Some("fixture.png")
    );
    let output = serde_json::to_string(&result).unwrap();
    for forbidden in [
        "reply-private-copy",
        "download-secret",
        "raw-protocol",
        "raw-code",
    ] {
        assert!(!output.contains(forbidden));
    }
}

#[test]
fn selected_and_encoded_output_limits_fail_instead_of_silent_truncation() {
    let loaded: Vec<_> = (0..201).map(message).collect();
    let ids = loaded.iter().map(|m| m.msg_id.as_str()).collect();
    let request = ReadRequest {
        mode: ReadMode::Selected,
        ..Default::default()
    };
    assert_eq!(
        snapshot(context(), &request, &loaded, &ids)
            .unwrap_err()
            .code,
        "too_many_selected"
    );
    let mut large = message(0);
    // JSON 转义后超限，原始字符串仍小于字节上限。
    large.content = "\n".repeat(MAX_RESPONSE_BYTES / 2);
    assert_eq!(
        snapshot(
            context(),
            &ReadRequest::default(),
            &[large],
            &HashSet::new()
        )
        .unwrap_err()
        .code,
        "result_too_large"
    );
}

#[test]
fn validation_enforces_authentication_and_explicit_read_bounds() {
    let mut config = AgentContextConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(config.validate().is_err());
    config.auth_token = "fixture-token".into();
    config.allowlist.push(context().target);
    assert!(config.validate().is_ok());
    assert!(!config.allows(&Target {
        bridge: "other".into(),
        room_id: -10
    }));
    config.auth_token = "token\r\ninjection".into();
    assert!(config.validate().is_err());
    for request in [
        ReadRequest {
            limit: Some(0),
            ..Default::default()
        },
        ReadRequest {
            limit: Some(201),
            ..Default::default()
        },
        ReadRequest {
            mode: ReadMode::Selected,
            limit: Some(1),
            ..Default::default()
        },
        ReadRequest {
            reason: Some("用途\n伪造提示".into()),
            ..Default::default()
        },
    ] {
        assert!(request.validate().is_err());
    }
}

#[test]
fn partially_evicted_selection_is_rejected_instead_of_silently_shrinking() {
    let loaded = vec![message(0)];
    let request = ReadRequest {
        mode: ReadMode::Selected,
        ..Default::default()
    };
    let error = snapshot(
        context(),
        &request,
        &loaded,
        &HashSet::from(["fixture-0", "evicted"]),
    )
    .unwrap_err();
    assert_eq!(error.code, "selection_unavailable");
}

#[test]
fn read_token_cannot_inherit_noticer_send_permissions_by_reuse() {
    let mut config = crate::config::IcaCfg::default();
    config.agent_context.enabled = true;
    config.agent_context.auth_token = "read-fixture-token".into();
    config.noticer.auth_token = "read-fixture-token".into();
    assert!(
        config
            .validate_private_keys()
            .unwrap_err()
            .to_string()
            .contains("不能与 Noticer")
    );
    config.noticer.auth_token = "send-fixture-token".into();
    config.noticer.direct_token = "read-fixture-token".into();
    assert!(
        config
            .validate_private_keys()
            .unwrap_err()
            .to_string()
            .contains("不能与 Noticer")
    );
    config.noticer.direct_token = "direct-fixture-token".into();
    config.validate_private_keys().unwrap();
}
