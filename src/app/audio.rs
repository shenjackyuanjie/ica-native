//! 本地录音与语音播放。只有用户点击后才启动音频线程、设备或网络请求。
//!
//! 宿主每帧排空 `poll_action`，按 `bridge_key` 路由发送命令，再把带请求编号的
//! HTTP 接收结果交给 `finish_send`。HTTP 接收成功不等于 QQ 已投递成功。

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use crate::ica::types::files::MessageFile;

mod device;
mod wav;

pub const MAX_RECORD_SECONDS: u64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomKey {
    pub bridge_key: String,
    pub room_id: i64,
}

#[derive(Debug)]
pub enum AudioAction {
    SendVoice {
        request_id: u64,
        bridge_key: String,
        room_id: i64,
        audio_data: Arc<[u8]>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftStage {
    Starting,
    Recording,
    Stopping,
    Ready,
    Sending,
}

#[derive(Clone)]
pub struct VoiceDraft {
    pub id: u64,
    pub owner: RoomKey,
    pub stage: DraftStage,
    pub duration: Duration,
    pub wav: Option<Arc<[u8]>>,
    pub request_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackItem {
    Preview(u64),
    Message {
        message_id: String,
        attachment_index: usize,
        url: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackKey {
    pub owner: RoomKey,
    pub item: PlaybackItem,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStage {
    Loading,
    Playing,
    Paused,
    Finished,
    Failed,
}

#[derive(Clone)]
pub struct PlaybackView {
    pub key: PlaybackKey,
    pub stage: PlaybackStage,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct AudioState {
    pub draft: Option<VoiceDraft>,
    pub playback: Option<PlaybackView>,
    pub notice: Option<(RoomKey, String)>,
}

#[derive(Default)]
pub struct AudioController {
    state: Arc<Mutex<AudioState>>,
    worker: Mutex<Option<mpsc::Sender<device::WorkerCommand>>>,
    actions: Mutex<VecDeque<AudioAction>>,
    sequence: AtomicU64,
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AudioController {
    fn next_id(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn dispatch(&self, ctx: &egui::Context, command: device::WorkerCommand) {
        let mut worker = lock(&self.worker);
        if worker.is_none() {
            match device::spawn(self.state.clone(), ctx.clone()) {
                Ok(sender) => *worker = Some(sender),
                Err(error) => {
                    self.worker_failed(error);
                    return;
                }
            }
        }
        if worker
            .as_ref()
            .is_some_and(|sender| sender.send(command).is_err())
        {
            *worker = None;
            self.worker_failed("音频线程已停止，请重试".to_string());
        }
    }

    fn worker_failed(&self, message: String) {
        let mut state = lock(&self.state);
        if let Some(draft) = state.draft.take() {
            state.notice = Some((draft.owner, message.clone()));
        }
        if let Some(playback) = state.playback.as_mut() {
            playback.stage = PlaybackStage::Failed;
            playback.error = Some(message);
        }
    }

    /// 录音入口与输入框工具栏共用一行，状态与预览浮层不挤占聊天区。
    pub fn render_audio_toolbar(
        &self,
        ui: &mut egui::Ui,
        bridge_key: &str,
        room_id: i64,
        can_send: bool,
        button_size: egui::Vec2,
    ) {
        let owner = RoomKey {
            bridge_key: bridge_key.to_string(),
            room_id,
        };
        let (draft, notice) = {
            let state = lock(&self.state);
            (state.draft.clone(), state.notice.clone())
        };
        ui.push_id(("voice_composer", bridge_key, room_id), |ui| {
            let other_room = draft.as_ref().is_some_and(|draft| draft.owner != owner);
            let owned_draft = draft.filter(|draft| draft.owner == owner);
            let owned_notice = notice.filter(|(notice_owner, _)| *notice_owner == owner);
            let recording = owned_draft.as_ref().is_some_and(|draft| {
                matches!(
                    draft.stage,
                    DraftStage::Starting | DraftStage::Recording | DraftStage::Stopping
                )
            });
            let tooltip = if other_room {
                "另一个会话有录音或待发送预览，请先回到该会话处理"
            } else if owned_draft.is_some() {
                "打开录音面板；关闭面板不会取消录音或清除预览"
            } else {
                "录音：点击后才访问麦克风，最长 60 秒，停止后预览并确认发送"
            };
            let response = ui
                .add_enabled_ui(!other_room && (can_send || owned_draft.is_some()), |ui| {
                    ui.add_sized(
                        button_size,
                        egui::Button::new(
                            egui::RichText::new(if recording { "●" } else { "🎙" })
                                .size(15.0)
                                .color(if recording {
                                    egui::Color32::LIGHT_RED
                                } else {
                                    ui.visuals().text_color()
                                }),
                        )
                        .selected(owned_draft.is_some()),
                    )
                })
                .inner
                .on_hover_text(tooltip);
            if response.clicked() && owned_draft.is_none() {
                let id = self.next_id();
                let mut state = lock(&self.state);
                state.notice = None;
                state.draft = Some(VoiceDraft {
                    id,
                    owner: owner.clone(),
                    stage: DraftStage::Starting,
                    duration: Duration::ZERO,
                    wav: None,
                    request_id: None,
                });
                drop(state);
                self.dispatch(ui.ctx(), device::WorkerCommand::StartRecording(id));
            }
            egui::Popup::from_toggle_button_response(&response)
                .align(egui::RectAlign::TOP_END)
                .width(360.0)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong("语音");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("收起").clicked() {
                                ui.close();
                            }
                        });
                    });
                    ui.separator();
                    if let Some(draft) = owned_draft {
                        ui.horizontal_wrapped(|ui| {
                            match draft.stage {
                                DraftStage::Starting => {
                                    ui.spinner();
                                    ui.label("正在打开麦克风…");
                                }
                                DraftStage::Recording => {
                                    ui.label(format!("录音 {} / 01:00", time_text(draft.duration)));
                                    if ui.button("停止").clicked() {
                                        if let Some(current) = lock(&self.state).draft.as_mut() {
                                            current.stage = DraftStage::Stopping;
                                        }
                                        self.dispatch(
                                            ui.ctx(),
                                            device::WorkerCommand::StopRecording(draft.id),
                                        );
                                    }
                                }
                                DraftStage::Stopping => {
                                    ui.spinner();
                                    ui.label("正在生成预览…");
                                }
                                DraftStage::Ready => {
                                    ui.label(format!("语音 {}", time_text(draft.duration)));
                                    if let Some(bytes) = draft.wav.as_ref() {
                                        self.render_player(
                                            ui,
                                            PlaybackKey {
                                                owner: owner.clone(),
                                                item: PlaybackItem::Preview(draft.id),
                                            },
                                            device::AudioSource::Memory(bytes.clone()),
                                        );
                                        if ui
                                            .add_enabled(can_send, egui::Button::new("发送语音"))
                                            .clicked()
                                        {
                                            self.queue_send(&owner);
                                        }
                                    }
                                }
                                DraftStage::Sending => {
                                    ui.spinner();
                                    ui.label("正在提交 Bridge…");
                                }
                            }
                            if draft.stage != DraftStage::Sending && ui.button("取消").clicked() {
                                self.cancel_draft(ui.ctx(), &owner);
                                ui.close();
                            }
                        });
                    } else if response.clicked() && owned_notice.is_none() {
                        ui.label("正在打开麦克风…");
                    }
                    if let Some((_, message)) = owned_notice {
                        ui.label(egui::RichText::new(message).small());
                        if ui.small_button("知道了").clicked() {
                            let mut state = lock(&self.state);
                            if state
                                .notice
                                .as_ref()
                                .is_some_and(|(notice_owner, _)| *notice_owner == owner)
                            {
                                state.notice = None;
                            }
                            ui.close();
                        }
                    }
                });
            if recording {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        });
    }

    fn cancel_draft(&self, ctx: &egui::Context, owner: &RoomKey) {
        let mut state = lock(&self.state);
        if let Some(draft) = &state.draft
            && &draft.owner == owner
            && draft.stage != DraftStage::Sending
        {
            let id = draft.id;
            state.draft = None;
            state.notice = None;
            drop(state);
            self.dispatch(ctx, device::WorkerCommand::CancelRecording(id));
        }
    }

    fn queue_send(&self, owner: &RoomKey) {
        let mut state = lock(&self.state);
        let Some(draft) = state.draft.as_mut() else {
            return;
        };
        if &draft.owner != owner || draft.stage != DraftStage::Ready {
            return;
        }
        let Some(audio_data) = draft.wav.clone() else {
            return;
        };
        let request_id = self.next_id();
        draft.stage = DraftStage::Sending;
        draft.request_id = Some(request_id);
        lock(&self.actions).push_back(AudioAction::SendVoice {
            request_id,
            bridge_key: owner.bridge_key.clone(),
            room_id: owner.room_id,
            audio_data,
        });
    }

    /// 每帧在主窗口处理一次；发送目标使用动作中的 Bridge 标识，不使用当前选中会话。
    pub fn poll_action(&self) -> Option<AudioAction> {
        lock(&self.actions).pop_front()
    }

    /// HTTP 202 后仅清理匹配的草稿。超时、断线或拒收保留 WAV，允许用户手动重试。
    pub fn finish_send(
        &self,
        bridge_key: &str,
        room_id: i64,
        request_id: u64,
        result: Result<(), String>,
    ) {
        let mut state = lock(&self.state);
        let Some(draft) = state.draft.as_ref() else {
            return;
        };
        if draft.owner.bridge_key != bridge_key
            || draft.owner.room_id != room_id
            || draft.request_id != Some(request_id)
            || draft.stage != DraftStage::Sending
        {
            return;
        }
        let owner = draft.owner.clone();
        let message = match result {
            Ok(()) => {
                state.draft = None;
                "Bridge 已接收语音；最终发送状态以 Bridge 回报为准".to_string()
            }
            Err(error) => {
                if let Some(draft) = state.draft.as_mut() {
                    draft.stage = DraftStage::Ready;
                    draft.request_id = None;
                }
                error
            }
        };
        state.notice = Some((owner, message));
    }

    /// Bridge 停止重连后，队列可能再也不会返回发送回执；保留 WAV，禁止旧回执清理草稿。
    pub fn fail_pending_send_for_bridge(&self, bridge_key: &str) {
        let mut state = lock(&self.state);
        let Some(draft) = state.draft.as_mut() else {
            return;
        };
        if draft.owner.bridge_key != bridge_key || draft.stage != DraftStage::Sending {
            return;
        }
        draft.stage = DraftStage::Ready;
        draft.request_id = None;
        let owner = draft.owner.clone();
        state.notice = Some((
            owner,
            "Bridge 已停止重连，录音已保留；请先检查聊天记录，确认发送状态后再手动处理".into(),
        ));
    }

    /// 返回 false 表示不是音频。支持 &self 消息卡片，无需改宿主的借用结构。
    #[allow(clippy::too_many_arguments)]
    pub fn render_voice_message(
        &self,
        ui: &mut egui::Ui,
        bridge_key: &str,
        room_id: i64,
        message_id: &str,
        attachment_index: usize,
        api_base_url: &str,
        file: &MessageFile,
    ) -> bool {
        if !file.file_type.starts_with("audio/") {
            return false;
        }
        ui.push_id(
            (
                "voice_message",
                bridge_key,
                room_id,
                message_id,
                attachment_index,
            ),
            |ui| {
                if file.name.as_deref() == Some("decoding") || file.url == "decoding" {
                    ui.label("语音正在由 Bridge 解码…");
                    return;
                }
                match resolve_voice_url(api_base_url, file) {
                    Ok(url) => self.render_player(
                        ui,
                        PlaybackKey {
                            owner: RoomKey {
                                bridge_key: bridge_key.to_string(),
                                room_id,
                            },
                            item: PlaybackItem::Message {
                                message_id: message_id.to_string(),
                                attachment_index,
                                url: url.clone(),
                            },
                        },
                        device::AudioSource::Remote(url),
                    ),
                    Err(error) => {
                        ui.label(error);
                    }
                }
            },
        );
        true
    }

    fn render_player(&self, ui: &mut egui::Ui, key: PlaybackKey, source: device::AudioSource) {
        let (view, recording) = {
            let state = lock(&self.state);
            (
                state
                    .playback
                    .as_ref()
                    .filter(|view| view.key == key)
                    .cloned(),
                state.draft.as_ref().is_some_and(|draft| {
                    matches!(
                        draft.stage,
                        DraftStage::Starting | DraftStage::Recording | DraftStage::Stopping
                    )
                }),
            )
        };
        ui.horizontal(|ui| {
            let label = match view.as_ref().map(|view| view.stage) {
                Some(PlaybackStage::Loading) => "取消加载",
                Some(PlaybackStage::Playing) => "暂停",
                Some(PlaybackStage::Paused) => "继续",
                Some(PlaybackStage::Failed) => "重试播放",
                _ => "播放",
            };
            if ui
                .add_enabled(!recording, egui::Button::new(label))
                .clicked()
            {
                self.dispatch(
                    ui.ctx(),
                    device::WorkerCommand::Play {
                        key: key.clone(),
                        source,
                    },
                );
            }
            if let Some(view) = view {
                if let Some(error) = view.error {
                    ui.label(error);
                    return;
                }
                let text = format!(
                    "{} / {}",
                    time_text(view.position),
                    view.duration.map_or_else(|| "--:--".to_string(), time_text)
                );
                if let Some(duration) = view.duration.filter(|duration| !duration.is_zero()) {
                    let mut position = view.position.min(duration).as_secs_f64();
                    let response = ui.add_enabled(
                        matches!(view.stage, PlaybackStage::Playing | PlaybackStage::Paused),
                        egui::Slider::new(&mut position, 0.0..=duration.as_secs_f64())
                            .show_value(false),
                    );
                    if response.changed() {
                        self.dispatch(
                            ui.ctx(),
                            device::WorkerCommand::Seek {
                                key,
                                position: Duration::from_secs_f64(position),
                            },
                        );
                    }
                }
                ui.label(text);
                if matches!(view.stage, PlaybackStage::Playing | PlaybackStage::Loading) {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                }
            } else {
                ui.label("语音");
            }
        });
    }
}

impl Drop for AudioController {
    fn drop(&mut self) {
        if let Some(sender) = lock(&self.worker).take() {
            let _ = sender.send(device::WorkerCommand::Shutdown);
        }
    }
}

fn time_text(duration: Duration) -> String {
    format!(
        "{:02}:{:02}",
        duration.as_secs() / 60,
        duration.as_secs() % 60
    )
}

/// 与 Electron 的 audioPath 一致：Bridge 缓存名映射到 /records，其余只允许 HTTP(S)。
pub fn resolve_voice_url(api_base_url: &str, file: &MessageFile) -> Result<String, String> {
    if file.url == "decoding" || file.name.as_deref() == Some("decoding") {
        return Err("语音正在由 Bridge 解码…".to_string());
    }
    let url = file.url.trim();
    if url.is_empty() {
        return Err("语音暂无可播放地址".to_string());
    }
    if file.name.as_deref() == Some(url)
        && !url.contains(['/', '\\', ':'])
        && url != "."
        && url != ".."
    {
        let mut base =
            reqwest::Url::parse(api_base_url).map_err(|_| "Bridge HTTP 地址无效".to_string())?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err("Bridge 必须使用 HTTP(S) 地址".to_string());
        }
        base.set_query(None);
        base.set_fragment(None);
        base.path_segments_mut()
            .map_err(|_| "Bridge HTTP 地址无效".to_string())?
            .pop_if_empty()
            .push("records")
            .push(url);
        return Ok(base.to_string());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| "语音地址无效，请刷新消息".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("不支持此语音地址，仅播放 HTTP(S) 音频".to_string());
    }
    Ok(parsed.to_string())
}

impl crate::app::IcaApp {
    pub fn process_audio_actions(&mut self) {
        while let Some(action) = self.audio.poll_action() {
            let AudioAction::SendVoice {
                request_id,
                bridge_key,
                room_id,
                audio_data,
            } = action;
            let result = match self
                .bridge_states
                .iter()
                .find(|session| session.bridge_key == bridge_key)
            {
                Some(session)
                    if session.socket_state == crate::app::SocketState::Connected
                        && session.auth_state == crate::app::AuthState::Succeeded =>
                {
                    session.send(crate::ica::IcaCommand::SendVoiceMessage {
                        request_id,
                        room_id,
                        audio_data,
                    })
                }
                Some(_) => Err("Bridge 未连接或认证未完成，录音已保留".to_string()),
                None => Err("录音所属 Bridge 已关闭，录音已保留".to_string()),
            };
            if let Err(error) = result {
                self.audio
                    .finish_send(&bridge_key, room_id, request_id, Err(error));
            }
        }
    }

    pub fn apply_voice_send_result(&self, bridge_key: &str, payload: &serde_json::Value) {
        let (Some(room_id), Some(request_id)) = (
            payload.get("roomId").and_then(serde_json::Value::as_i64),
            payload.get("requestId").and_then(serde_json::Value::as_u64),
        ) else {
            return;
        };
        let result = if payload.get("accepted").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("语音提交失败，录音已保留")
                .to_string())
        };
        self.audio
            .finish_send(bridge_key, room_id, request_id, result);
    }

    pub fn render_chat_voice(
        &self,
        ui: &mut egui::Ui,
        room_id: i64,
        message_id: &str,
        attachment_index: usize,
        file: &MessageFile,
    ) -> bool {
        if !file.file_type.starts_with("audio/") {
            return false;
        }
        let Some(session) = self.active_bridge_state() else {
            return false;
        };
        let config = self.config.snapshot();
        let url = config
            .bridges
            .iter()
            .find(|bridge| bridge.key() == session.bridge_key)
            .map(|bridge| bridge.url.as_str())
            .unwrap_or("");
        let http_url = if let Some(rest) = url.strip_prefix("ws://") {
            format!("http://{rest}")
        } else if let Some(rest) = url.strip_prefix("wss://") {
            format!("https://{rest}")
        } else {
            url.to_string()
        };
        self.audio.render_voice_message(
            ui,
            &session.bridge_key,
            room_id,
            message_id,
            attachment_index,
            &http_url,
            file,
        )
    }
}

impl AudioController {}

#[cfg(test)]
mod tests {
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
                let (_, row, _) =
                    toolbar_frame(&controller, &egui::Context::default(), width, vec![]);
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
}
