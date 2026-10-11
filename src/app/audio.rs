//! 本地录音与语音播放。只有用户点击后才启动音频线程、设备或网络请求。
//!
//! 宿主每帧排空 `poll_action`，按 `bridge_key` 路由发送命令，再把带请求编号的
//! HTTP 接收结果交给 `finish_send`。HTTP 接收成功不等于 QQ 已投递成功。
//!
//! 该模块位于 `crate::app` 之下，由两层组成：
//! - 本文件的 `AudioController`：跨会话共享的控制器，持有 `AudioState` 与惰性工作线程句柄，
//!   在 egui 线程上驱动录音草稿、播放视图与工具栏；
//! - `audio::device`：唯一拥有音频设备的工作线程，负责麦克风采集、网络下载与解码播放，
//!   GUI 线程不因这些阻塞操作而卡顿。
//!
//! 关键设计：
//! - 所有跨线程可达的状态集中在 `AudioState`，由 `lock` 统一加锁（含 poison 恢复）；
//! - 后台任务结果通过 `request_id` / `generation` 与当前草稿匹配，避免过期回写；
//! - 录音、下载、播放都遵循“用户点击后才启动”的懒加载原则。

mod device;
#[cfg(test)]
mod tests;
mod wav;

use crate::ica::types::files::MessageFile;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

/// 单次录音的最长秒数（含一点缓冲），到点由工作线程自动收尾。
pub const MAX_RECORD_SECONDS: u64 = 60;

/// 音频操作归属的“会话”标识：由 bridge 标识与会话 ID 共同确定。
///
/// 用 `bridge_key + room_id` 而非仅 room_id，是因为多个 bridge 可能打开同一会话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomKey {
    /// 所属 bridge 的标识。
    pub bridge_key: String,
    /// 会话 ID（roomId）。
    pub room_id: i64,
}

/// 控制器向宿主排出的、需要在主线程执行的动作。
#[derive(Debug)]
pub enum AudioAction {
    /// 请求把一段录音作为语音消息发送到指定 bridge 的指定会话。
    SendVoice {
        /// 请求编号，用于把后续发送回执匹配回对应草稿。
        request_id: u64,
        /// 目标 bridge 标识（使用动作里的标识，而非当前选中会话）。
        bridge_key: String,
        /// 目标会话 ID。
        room_id: i64,
        /// WAV 格式的音频字节。
        audio_data: Arc<[u8]>,
    },
}

/// 录音草稿的生命周期阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftStage {
    /// 已创建，正在等待工作线程打开麦克风。
    Starting,
    /// 麦克风已打开，正在采集。
    Recording,
    /// 用户已点停止，正在生成预览（编码 WAV）。
    Stopping,
    /// 预览已生成，可播放、可发送。
    Ready,
    /// 已提交发送，等待 bridge 回执。
    Sending,
}

/// 一份录音草稿：从开始录音到发送完成前的全过程状态。
#[derive(Clone)]
pub struct VoiceDraft {
    /// 草稿唯一 ID，用于把工作线程事件路由回该草稿。
    pub id: u64,
    /// 归属的会话（bridge + room）。
    pub owner: RoomKey,
    /// 当前生命周期阶段。
    pub stage: DraftStage,
    /// 已录制的时长（采集期间实时更新）。
    pub duration: Duration,
    /// 停止后编码得到的 WAV 字节；录音中为 `None`。
    pub wav: Option<Arc<[u8]>>,
    /// 发送中的请求编号；未发送或已结束时为 `None`。
    pub request_id: Option<u64>,
}

/// 一次播放的目标：既可能是录音预览，也可能是聊天里的语音消息附件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackItem {
    /// 录音草稿预览，值为草稿 ID。
    Preview(u64),
    /// 聊天中的语音消息附件。
    Message {
        /// 消息 ID。
        message_id: String,
        /// 附件在消息中的下标（一条消息可带多个附件）。
        attachment_index: usize,
        /// 解析后的可播放 URL。
        url: String,
    },
}

/// 播放的唯一键：会话 + 播放目标，用于把后台播放进度匹配回对应 UI。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackKey {
    /// 归属的会话。
    pub owner: RoomKey,
    /// 播放目标。
    pub item: PlaybackItem,
}

/// 播放的生命周期阶段。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStage {
    /// 正在下载/解码，尚未开始发声。
    Loading,
    /// 正在播放。
    Playing,
    /// 已暂停。
    Paused,
    /// 播放自然结束。
    Finished,
    /// 失败（附错误信息）。
    Failed,
}

/// 供 UI 读取的播放视图快照。
#[derive(Clone)]
pub struct PlaybackView {
    /// 对应哪一次播放。
    pub key: PlaybackKey,
    /// 当前阶段。
    pub stage: PlaybackStage,
    /// 当前播放位置。
    pub position: Duration,
    /// 总时长（未知时为 `None`，如尚未解码完成）。
    pub duration: Option<Duration>,
    /// 失败原因。
    pub error: Option<String>,
}

/// 跨线程共享的音频状态：同一时刻至多一份录音草稿和一次播放。
#[derive(Default)]
pub struct AudioState {
    /// 当前录音草稿（若有）。
    pub draft: Option<VoiceDraft>,
    /// 当前播放视图（若有）。
    pub playback: Option<PlaybackView>,
    /// 需要提示用户的一次性通知（归属会话 + 文本）。
    pub notice: Option<(RoomKey, String)>,
}

/// 音频控制器：egui 线程侧的唯一入口，跨会话共享。
///
/// 持有共享状态、惰性启动的工作线程句柄、待宿主处理的动作队列与一个自增序号；
/// 自身实现 `Clone`，可被多个视图/会话同时持有同一份状态。
#[derive(Default)]
pub struct AudioController {
    /// 跨线程共享的音频状态。
    state: Arc<Mutex<AudioState>>,
    /// 惰性创建的工作线程命令发送端；`None` 表示尚未启动或已停止。
    worker: Mutex<Option<mpsc::Sender<device::WorkerCommand>>>,
    /// 待宿主在主线程处理的动作队列。
    actions: Mutex<VecDeque<AudioAction>>,
    /// 自增序号，用于分配草稿 ID 与请求编号。
    sequence: AtomicU64,
}

/// 统一的加锁助手：锁被污染（某处 panic）时恢复其内部值而非传播 panic。
///
/// 音频状态一次 panic 不应让整个 GUI 后续所有锁操作都失败，因此这里选择
/// `into_inner` 继续使用。
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `AudioController` 的固有实现：草稿/播放驱动、工具栏渲染与发送生命周期管理。
impl AudioController {
    /// 分配下一个自增序号（从 1 开始），用作草稿 ID 或请求编号。
    fn next_id(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 把一条命令发给工作线程；线程尚未启动时惰性创建，失效时置空并提示失败。
    ///
    /// 这样麦克风/下载/解码都只在真正需要命令时才触达音频线程。
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

    /// 工作线程启动失败或已停止时，把进行中的草稿/播放标记为失败并写入通知。
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
            // 全局只有一份草稿/通知；这里按当前会话过滤出“属于自己的”那份，
            // 其它会话的草稿仅用于给出“请先回到该会话处理”的提示。
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

    /// 取消归属该会话的录音草稿（发送中的草稿不可取消），并通知工作线程停止采集。
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

    /// 把就绪的录音草稿转入发送中，并把 `SendVoice` 动作入队等待宿主处理。
    ///
    /// 仅在草稿归属匹配且阶段为 `Ready` 时生效；WAV 字节被克隆进行动，
    /// 以便发送失败后仍能在草稿里保留重试。
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

    /// 绘制单个播放器（播放/暂停按钮、进度条与时间），并把交互转成工作线程命令。
    ///
    /// 录音进行中禁用播放按钮，避免与麦克风采集争用音频设备。
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

/// 控制器析构时通知工作线程关闭，避免遗留麦克风流或播放线程。
impl Drop for AudioController {
    fn drop(&mut self) {
        if let Some(sender) = lock(&self.worker).take() {
            let _ = sender.send(device::WorkerCommand::Shutdown);
        }
    }
}

/// 把时长格式化为 `mm:ss`，用于录音/播放计时显示。
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
    /// 每帧处理音频控制器排出的发送动作：按动作里的 bridge 标识路由，校验连接与认证后下发。
    ///
    /// 发送通道层面的失败会立即以错误回执收尾，让草稿回到 `Ready` 保留重试。
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

    /// 应用一次语音发送回执（HTTP 202 后的 Bridge 回报）。
    ///
    /// 从 `payload` 读取 `roomId` / `requestId` / `accepted` / `message`，
    /// 匹配对应草稿后交给 `finish_send`：接受则清理草稿，否则保留 WAV 供重试。
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

    /// 在聊天消息里渲染语音附件播放器。
    ///
    /// 返回 `false` 表示该附件不是音频（交由其它渲染路径处理）；
    /// 是音频时把当前 bridge 的 `ws(s)://` 地址换算成 `http(s)://` 后交给控制器渲染。
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
        // 语音走 HTTP 播放，这里把 bridge 的 ws(s):// 地址换算成 http(s)://。
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

/// 预留的固有 impl 块；当前未添加额外方法，`AudioController` 的实现见上方各 `impl`。
impl AudioController {}
