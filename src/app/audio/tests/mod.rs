use super::*;

fn controller_with_preview() -> (AudioController, RoomKey) {
    let controller = AudioController::default();
    let owner = RoomKey {
        bridge_key: "bridge-a".into(),
        room_id: -7,
    };
    lock(&controller.state).draft = Some(VoiceDraft {
        id: 99,
        owner: owner.clone(),
        stage: DraftStage::Ready,
        duration: Duration::from_secs(1),
        wav: Some(Arc::from([0_u8; 44])),
        request_id: None,
    });
    (controller, owner)
}

fn toolbar_frame(
    controller: &AudioController,
    ctx: &egui::Context,
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, egui::Rect, egui::Pos2) {
    let mut row = egui::Rect::NOTHING;
    let mut microphone = egui::Pos2::ZERO;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 240.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            row = ui
                .horizontal(|ui| {
                    ui.add_sized([width - 90.0, 30.0], egui::Button::new("输入"));
                    microphone = ui.next_widget_position() + egui::vec2(15.0, 15.0);
                    controller.render_audio_toolbar(
                        ui,
                        "bridge-a",
                        -7,
                        false,
                        egui::vec2(30.0, 30.0),
                    );
                    ui.add_sized([30.0, 30.0], egui::Button::new("发送"));
                })
                .response
                .rect;
        },
    );
    output.textures_delta.clear();
    (output, row, microphone)
}

#[test]
fn voice_toolbar_keeps_input_and_send_on_one_row_in_all_draft_stages() {
    for width in [180.0, 640.0] {
        for stage in [
            None,
            Some(DraftStage::Starting),
            Some(DraftStage::Recording),
            Some(DraftStage::Stopping),
            Some(DraftStage::Ready),
            Some(DraftStage::Sending),
        ] {
            let (controller, _) = controller_with_preview();
            if let Some(stage) = stage {
                lock(&controller.state).draft.as_mut().unwrap().stage = stage;
            } else {
                lock(&controller.state).draft = None;
            }
            let (_, row, _) = toolbar_frame(&controller, &egui::Context::default(), width, vec![]);
            assert!(
                row.height() <= 32.0,
                "录音状态不应增加工具栏高度：{stage:?}"
            );
            assert!(row.right() <= width, "录音入口不能挤出发送按钮");
            assert!(
                lock(&controller.worker).is_none(),
                "绘制录音入口不得打开设备"
            );
        }
    }
}

#[test]
fn escape_closes_voice_popup_without_discarding_offline_preview() {
    let (controller, _) = controller_with_preview();
    let ctx = egui::Context::default();
    let (_, _, pos) = toolbar_frame(&controller, &ctx, 640.0, vec![]);
    toolbar_frame(
        &controller,
        &ctx,
        640.0,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert!(
        egui::Popup::is_any_open(&ctx),
        "离线时仍应能打开保留的语音预览"
    );
    toolbar_frame(
        &controller,
        &ctx,
        640.0,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(!egui::Popup::is_any_open(&ctx));
    let state = lock(&controller.state);
    let draft = state.draft.as_ref().expect("收起浮层不能丢弃录音");
    assert_eq!(draft.stage, DraftStage::Ready);
    assert!(draft.wav.is_some());
    assert!(controller.poll_action().is_none(), "Esc 不应发送录音");
    assert!(
        lock(&controller.worker).is_none(),
        "打开与收起预览不能自行访问设备"
    );
}

#[test]
fn switching_bridges_cannot_send_another_conversations_preview() {
    let (controller, owner) = controller_with_preview();
    controller.queue_send(&RoomKey {
        bridge_key: "bridge-b".into(),
        ..owner.clone()
    });
    controller.queue_send(&RoomKey {
        room_id: -8,
        ..owner.clone()
    });
    assert!(controller.poll_action().is_none());
    controller.queue_send(&owner);
    controller.queue_send(&owner);
    assert!(matches!(
        controller.poll_action(),
        Some(AudioAction::SendVoice { room_id: -7, .. })
    ));
    assert!(
        controller.poll_action().is_none(),
        "不能重复提交正在发送的录音"
    );
    assert!(
        lock(&controller.worker).is_none(),
        "状态操作不能打开音频设备"
    );
}

#[test]
fn stale_ack_cannot_clear_new_or_other_bridge_draft_and_failure_allows_retry() {
    let (controller, owner) = controller_with_preview();
    controller.queue_send(&owner);
    let Some(AudioAction::SendVoice { request_id, .. }) = controller.poll_action() else {
        panic!()
    };
    controller.finish_send("bridge-b", -7, request_id, Ok(()));
    controller.finish_send("bridge-a", -8, request_id, Ok(()));
    controller.finish_send("bridge-a", -7, request_id + 1, Ok(()));
    assert_eq!(
        lock(&controller.state).draft.as_ref().unwrap().stage,
        DraftStage::Sending
    );
    controller.finish_send("bridge-a", -7, request_id, Err("发送失败".into()));
    assert_eq!(
        lock(&controller.state).draft.as_ref().unwrap().stage,
        DraftStage::Ready
    );
    controller.queue_send(&owner);
    let Some(AudioAction::SendVoice {
        request_id: retry, ..
    }) = controller.poll_action()
    else {
        panic!()
    };
    controller.finish_send("bridge-a", -7, request_id, Ok(()));
    assert!(lock(&controller.state).draft.is_some());
    controller.finish_send("bridge-a", -7, retry, Ok(()));
    assert!(lock(&controller.state).draft.is_none());
}

#[test]
fn terminal_bridge_failure_preserves_recording_and_rejects_late_ack() {
    let (controller, owner) = controller_with_preview();
    controller.queue_send(&owner);
    let Some(AudioAction::SendVoice {
        request_id,
        audio_data,
        ..
    }) = controller.poll_action()
    else {
        panic!();
    };
    controller.fail_pending_send_for_bridge("bridge-b");
    assert_eq!(
        lock(&controller.state).draft.as_ref().unwrap().stage,
        DraftStage::Sending
    );
    controller.fail_pending_send_for_bridge(&owner.bridge_key);
    controller.finish_send(&owner.bridge_key, owner.room_id, request_id, Ok(()));
    let state = lock(&controller.state);
    let draft = state.draft.as_ref().unwrap();
    assert_eq!(draft.stage, DraftStage::Ready);
    assert!(draft.request_id.is_none());
    assert_eq!(draft.wav.as_ref().unwrap().as_ref(), audio_data.as_ref());
    assert!(state.notice.as_ref().unwrap().1.contains("检查聊天记录"));
}

#[test]
fn record_url_keeps_reverse_proxy_prefix_and_never_treats_fid_as_playable() {
    let mut file = MessageFile {
        file_type: "audio/ogg".into(),
        url: "clip.ogg".into(),
        name: Some("clip.ogg".into()),
        fid: Some("protocol-resource-not-a-url".into()),
        size: None,
    };
    assert_eq!(
        resolve_voice_url("https://bridge.invalid/prefix/", &file).unwrap(),
        "https://bridge.invalid/prefix/records/clip.ogg"
    );
    file.url = "https://media.invalid/audio.ogg?key=example".into();
    assert_eq!(
        resolve_voice_url("https://bridge.invalid", &file).unwrap(),
        file.url
    );
    for invalid in [
        "",
        "decoding",
        "../secret",
        "file:///private/audio.wav",
        "data:audio/wav;base64,AAAA",
    ] {
        file.url = invalid.into();
        assert!(resolve_voice_url("https://bridge.invalid", &file).is_err());
    }
}
