//! 应用根模块：装配 `IcaApp`、egui 生命周期与每帧更新流程。
//!
//! `IcaApp` 是 eframe 应用入口，持有运行时、配置与全局状态，并在 `ui` 中按固定顺序
//! 驱动输入处理、事件轮询、各功能视图与独立窗口的绘制。子模块按职责拆分：`runtime`
//! 管 Tokio/Bridge 装配，`event` 把协议事件映射为状态，其余模块各自负责一类界面功能。

mod agent_context;
pub mod audio;
pub mod auto_sign;
mod chat;
pub mod chat_groups;
mod chat_shortcuts;
mod chat_windows;
mod contacts;
pub mod event;
mod media;
pub mod online_mode;
pub mod open_page;
mod relation_network;
mod runtime;
pub mod settings;
mod shell;
pub mod state;
pub mod stickers;
#[cfg(test)]
pub mod test_support;
#[cfg(test)]
mod tests;
mod tools;

use crate::config::ConfigStore;
use crate::ica::BridgeEventKind;
use eframe::CreationContext;
use media::{MediaEvent, MediaTask};
use runtime::AppRuntime;
pub use state::*;
use std::ops::{Deref, DerefMut};

/// 判断本帧是否应把一次粘贴当作“粘贴图片”处理。
///
/// 仅当窗口与视口均聚焦、没有文字粘贴事件、且按下了粘贴快捷键（egui 捕获到的 Ctrl+V
/// 或系统按键状态）时成立——此时剪贴板里更可能是图片而非文本。
fn clipboard_image_paste_requested(
    raw_input: &egui::RawInput,
    system_paste_shortcut_pressed: bool,
) -> bool {
    let viewport_focused = raw_input.viewport().focused.unwrap_or(raw_input.focused);
    if !raw_input.focused || !viewport_focused {
        return false;
    }

    let has_text_paste = raw_input
        .events
        .iter()
        .any(|event| matches!(event, egui::Event::Paste(_)));
    let has_paste_shortcut = raw_input.events.iter().any(|event| {
        if let egui::Event::Key {
            key: egui::Key::V,
            pressed: true,
            modifiers,
            ..
        } = event
        {
            modifiers.command
        } else {
            false
        }
    });

    !has_text_paste && (has_paste_shortcut || system_paste_shortcut_pressed)
}

/// 左侧会话列表的筛选分组。
///
/// `Custom(index)` 指向用户在“聊天分组编辑器”里自定义的分组下标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedChatGroup {
    /// 显示全部会话。
    All,
    /// 仅群（room_id < 0）。
    Group,
    /// 仅私聊/好友（room_id > 0）。
    Private,
    /// 用户自定义分组，值为分组下标。
    Custom(usize),
}

/// eframe 应用主体：持有运行时、配置存储、全局状态与独立聊天窗口集合。
pub struct IcaApp {
    /// Tokio/Bridge 运行时与跨线程事件通道。
    runtime: AppRuntime,
    /// 配置存储，负责读写 `ica_native*.toml`。
    config: ConfigStore,
    /// 全局应用状态（各 bridge 状态、UI 状态等）。
    state: state::AppState,
    /// 已拆分的独立聊天窗口列表。
    chat_windows: Vec<chat_windows::ChatWindow>,
}

/// 让 `IcaApp` 可直接访问内部 `AppState` 的字段/方法，省去到处写 `self.state.`。
impl Deref for IcaApp {
    /// 解引用目标为全局状态 `AppState`。
    type Target = state::AppState;

    /// 不可变借用全局状态。
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// 对应的可变解引用，便于直接修改全局状态。
impl DerefMut for IcaApp {
    /// 可变借用全局状态。
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

/// `IcaApp` 的应用级行为：配置更新、快捷键/输入处理、字体与样式装配、每帧更新等。
impl IcaApp {
    /// 修改配置并立即持久化；保存失败仅记日志，不中断当前操作。
    pub fn update_config(&self, updater: impl FnOnce(&mut crate::config::IcaCfg)) {
        self.config.update(updater);
        if let Err(error) = self.config.save() {
            tracing::error!("保存配置失败: {error}");
        }
    }

    /// Esc 不能穿透本视口里的其他浮动窗口、上下文菜单或模态框。
    pub fn chat_escape_blocked_by_window(
        ctx: &egui::Context,
        allowed_layer: Option<egui::LayerId>,
    ) -> bool {
        egui::Popup::is_any_open(ctx)
            || ctx.memory(|memory| {
                memory.top_modal_layer().is_some()
                    || memory.areas().visible_layer_ids().iter().any(|layer| {
                        matches!(layer.order, egui::Order::Middle | egui::Order::Foreground)
                            && Some(*layer) != allowed_layer
                    })
            })
    }

    /// 处理聊天界面的 Esc：按优先级取消浮层/编辑/转发选择，或在独立窗口关闭视口。
    ///
    /// IME 合成中、有模态框/浮层、焦点不在输入框等情况下直接放过，避免误吞 Esc。
    pub fn handle_chat_escape(&mut self, ctx: &egui::Context) {
        if self.ime_composing
            || self.ime_event_this_frame
            || !ctx.input(|input| {
                input.focused
                    && input.viewport().focused.unwrap_or(true)
                    && input.key_pressed(egui::Key::Escape)
            })
            || self.group_member_panel.confirmation.is_some()
        {
            return;
        }
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self
            .bridge_states
            .get(bridge_idx)
            .and_then(|state| state.selected_room_id)
        else {
            return;
        };
        // 主窗口占位页不得操作另一个独立聊天窗口的草稿或选择。
        if self.bridge_states[bridge_idx]
            .conversation(room_id)
            .is_some_and(|conversation| conversation.editing_send_pending)
        {
            return;
        }
        if ctx.viewport_id() == egui::ViewportId::ROOT && self.active_chat_is_detached() {
            return;
        }
        if ctx.viewport_id() == egui::ViewportId::ROOT
            && self
                .bridge_states
                .iter()
                .any(|state| state.forward_target_picker_open)
        {
            // 目标选择窗自行处理 Esc，不允许它穿透到草稿或转发选区。
            return;
        }
        let composer_id = egui::Id::new(("message_composer", bridge_idx, room_id));
        let mention_id = egui::Id::new(("mention_search", bridge_idx, room_id));
        let allowed_layer = self.show_mention_picker.then(|| {
            egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("mention_picker_overlay", bridge_idx, room_id)),
            )
        });
        if Self::chat_escape_blocked_by_window(ctx, allowed_layer)
            || ctx.memory(|memory| {
                memory.focused().is_some_and(|id| {
                    id != composer_id && !(self.show_mention_picker && id == mention_id)
                })
            })
        {
            return;
        }
        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return;
        }
        if self.cancel_composer_popup() {
            ctx.memory_mut(|memory| memory.request_focus(composer_id));
            return;
        }
        let state = &mut self.bridge_states[bridge_idx];
        if state.conversation_mut(room_id).cancel_composer_layer() {
            ctx.memory_mut(|memory| memory.request_focus(composer_id));
        } else if state.is_forward_selection_active(room_id) {
            state.clear_forward_selection();
        } else if ctx.viewport_id() == egui::ViewportId::ROOT {
            state.selected_room_id = None;
            ctx.memory_mut(|memory| memory.surrender_focus(composer_id));
        } else {
            // 独立聊天窗口只关闭自己的视口，主窗口选中会话由窗口适配层恢复。
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// 每帧更新输入法状态，并识别“Ctrl+V 但无文字粘贴”以支持粘贴图片。
    pub fn update_chat_input(&mut self, ctx: &egui::Context) {
        let raw_input = ctx.input(|input| input.raw.clone());
        self.ime_event_this_frame = false;
        let viewport_focused = raw_input.focused && raw_input.viewport().focused.unwrap_or(true);
        if !viewport_focused {
            self.ime_composing = false;
            self.clipboard_paste_failed = false;
        }
        for event in raw_input.events.iter().filter(|_| viewport_focused) {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    self.ime_event_this_frame = true;
                    self.ime_composing = !text.is_empty();
                }
                egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                    self.ime_event_this_frame = true;
                    self.ime_composing = false;
                }
                _ => {}
            }
        }

        // 检测 Ctrl+V 但没有文字粘贴事件的情况（剪贴板可能是图片）。
        // 即使窗口失焦也读取一次系统按键状态，避免重新聚焦后消费到陈旧的按下事件。
        let system_paste_shortcut_pressed = Self::system_paste_shortcut_pressed();
        self.clipboard_paste_failed =
            clipboard_image_paste_requested(&raw_input, system_paste_shortcut_pressed);
    }

    /// 装配字体：内置中文字体作主字体，系统字体补充覆盖，并显式注册 emoji 与 Unifont。
    fn setup_fonts(ctx: &egui::Context) {
        use egui_system_fonts::{FontPreset, FontStyle};

        // 保留内置中文字体作为主字体；系统字体负责补充覆盖。
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "notosans".into(),
            std::sync::Arc::new(egui::FontData::from_static(
                crate::assets::fonts::FONT_思源黑体,
            )),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "notosans".into());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("notosans".into());
        ctx.set_fonts(fonts);
        egui_system_fonts::add_auto(ctx, FontStyle::Sans);
        // 默认语言预设不包含 emoji，显式补充可用的系统 emoji 字体。
        // 优先使用有轮廓的字体；彩色位图字体的实际显示取决于 egui 支持。
        egui_system_fonts::add_with_presets(
            ctx,
            [FontPreset::Custom(vec![
                "Noto Emoji".into(),
                "Segoe UI Emoji".into(),
                "Apple Color Emoji".into(),
                "Noto Color Emoji".into(),
            ])],
            FontStyle::Sans,
        );
        // 必须最后注册，避免 Unifont 提前命中系统字体能更好显示的符号。
        ctx.add_font(egui::epaint::text::FontInsert {
            name: "unifont".into(),
            data: egui::FontData::from_static(crate::assets::fonts::FONT_UNIFONT),
            families: [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
                .into_iter()
                .map(|family| egui::epaint::text::InsertFontFamily {
                    family,
                    priority: egui::epaint::text::FontPriority::Lowest,
                })
                .collect(),
        });
    }

    /// 统一交互样式：去掉控件的 hover/active/open 外扩，让按钮和面板不随交互膨胀。
    fn setup_interaction_style(ctx: &egui::Context) {
        ctx.all_styles_mut(|style| {
            style.visuals.widgets.hovered.expansion = 0.0;
            style.visuals.widgets.active.expansion = 0.0;
            style.visuals.widgets.open.expansion = 0.0;
        });
    }

    /// 与 Icalingua++ 一致：窄窗口只保留会话页或聊天页其中之一。
    ///
    /// 保留现有的 `disable_adaptive_single_panel_mode` 配置语义：勾选时始终使用
    /// 三栏布局，取消勾选后才会在窄窗口自动切换。
    pub fn uses_compact_chat_layout(&self, ctx: &egui::Context) -> bool {
        ctx.viewport_id() == egui::ViewportId::ROOT
            && !self.custom_chat.disable_adaptive_single_panel_mode
            && ctx.content_rect().width() < 720.0
    }

    /// 构造应用：装配字体/样式、运行时与初始状态，并触发一次收藏表情刷新。
    pub fn new(cc: &CreationContext<'_>, config_store: ConfigStore) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self::setup_interaction_style(&cc.egui_ctx);

        let config = config_store.snapshot();
        let mut runtime = AppRuntime::new(&cc.egui_ctx, &config);
        let bridge_states = runtime.take_sessions();
        let sticker_store = stickers::StickerStore::resolve(&config, config_store.paths())
            .unwrap_or_else(|error| {
                stickers::StickerStore::unavailable(
                    config_store.paths().data_dir().join("stickers"),
                    error,
                )
            });
        let state =
            state::AppState::new(&config, &config_store, bridge_states, sticker_store.clone());
        let mut app = Self {
            runtime,
            config: config_store,
            state,
            chat_windows: Vec::new(),
        };
        app.media_notice = sticker_store.fallback_notice();
        app.spawn_media_task(
            &cc.egui_ctx,
            MediaTask::RefreshStickers {
                store: sticker_store,
                sort_newest_first: config.custom_chat.sort_stickers_by_time,
            },
        );
        app
    }

    /// 把一个媒体任务 spawn 到 Tokio，完成后把结果作为 `AppEvent::Media` 投回主线程。
    fn spawn_media_task(&self, ctx: &egui::Context, task: MediaTask) {
        let event_tx = self.runtime.event_sender();
        let repaint = ctx.clone();
        self.runtime.spawn(async move {
            let event = task.run().await;
            let _ = event_tx.send(event::AppEvent::Media(event));
            repaint.request_repaint();
        });
    }

    /// 应用媒体后台任务回传的结果：更新提示/错误，或把读取到的表情加入待发送图片。
    fn apply_media_event(&mut self, event: MediaEvent) {
        match event {
            MediaEvent::Completed(message) => {
                self.media_error = None;
                self.media_notice = Some(message);
            }
            MediaEvent::Failed { operation, error } => {
                tracing::warn!(operation, error = %error, "媒体后台任务执行失败");
                self.media_error = Some(format!("{operation}失败：{error}"));
            }
            MediaEvent::StickersRefreshed(count) => {
                tracing::debug!(count, "收藏表情已刷新");
            }
            MediaEvent::StickerLoaded {
                bridge_key,
                room_id,
                image,
            } => {
                if let Some(session) = self
                    .bridge_states
                    .iter_mut()
                    .find(|session| session.bridge_key == bridge_key)
                {
                    session.conversation_mut(room_id).pending_images.push(image);
                    self.media_notice = Some("已加入待发送图片".to_string());
                } else {
                    tracing::warn!(bridge = %bridge_key, room_id, "收藏表情所属 bridge 已关闭");
                    self.media_error = Some("收藏表情所属 bridge 已关闭".to_string());
                }
            }
        }
    }

    /// 取当前激活 bridge 的协议状态（无激活或索引越界时为 `None`）。
    pub fn active_bridge_state(&self) -> Option<&BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get(idx))
            .map(|session| session.state())
    }

    /// 取当前激活 bridge 协议状态的可变引用。
    pub fn active_bridge_state_mut(&mut self) -> Option<&mut BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get_mut(idx))
            .map(|session| session.state_mut())
    }

    /// 切换激活的 bridge，并重置一系列与旧 bridge 相关的临时 UI 状态。
    ///
    /// 索引越界或与当前激活相同则不做任何事。
    pub fn switch_active_bridge(&mut self, bridge_idx: usize) {
        if self.active_bridge_idx == Some(bridge_idx) || bridge_idx >= self.bridge_states.len() {
            return;
        }

        self.active_bridge_idx = Some(bridge_idx);
        self.ensure_selected_chat_group_valid();
        self.chat_list_scroll_target = ChatListScrollTarget::Top;
        self.show_face_picker = false;
        self.show_mention_picker = false;
        self.group_member_panel.open = false;
        self.group_member_panel.confirmation = None;
        self.mention_search_query.clear();
        self.mention_search_focus_requested = false;
        self.mention_replace_trigger = false;
        self.mention_selected_index = 0;
    }

    /// 每帧非阻塞地消费跨线程事件（Bridge/媒体/Agent 上下文），并应用到本地状态。
    ///
    /// 单帧最多处理 `MAX_EVENTS_PER_FRAME` 条，若有积压则请求下一帧重绘继续处理，
    /// 避免一次性处理过多导致掉帧；所属 bridge 已不存在的事件被丢弃。
    fn poll_socketio_events(&mut self, ctx: &egui::Context) {
        const MAX_EVENTS_PER_FRAME: usize = 128;
        let mut processed = 0;

        while processed < MAX_EVENTS_PER_FRAME {
            let Ok(event) = self.runtime.event_rx.try_recv() else {
                break;
            };
            processed += 1;
            let event = match event {
                event::AppEvent::AgentContext(request) => {
                    self.handle_agent_context_request(request);
                    continue;
                }
                event::AppEvent::Bridge(event) => event,
                event::AppEvent::Media(event) => {
                    self.apply_media_event(event);
                    continue;
                }
            };
            let bridge_key = event.bridge_key.as_str();
            let event_kind = &event.kind;
            let Some(bridge_idx) = self
                .bridge_states
                .iter()
                .position(|state| state.bridge_key == bridge_key)
            else {
                continue;
            };

            if let BridgeEventKind::VoiceSendResult(payload) = event_kind {
                self.apply_voice_send_result(bridge_key, payload);
                continue;
            }
            if matches!(event_kind, BridgeEventKind::SocketReconnectExhausted(_)) {
                self.audio.fail_pending_send_for_bridge(bridge_key);
            }

            if let BridgeEventKind::SetAllChatGroups(payload) = event_kind {
                if let Some(value) = payload.as_array().and_then(|values| values.first()) {
                    match <Vec<chat_groups::ChatGroup> as serde::Deserialize>::deserialize(value) {
                        Ok(groups) => {
                            let state = self.state.bridge_states[bridge_idx].state_mut();
                            state.chat_groups.groups = groups;
                            state.invalidate_visible_room_indices();
                            if let SelectedChatGroup::Custom(idx) = &state.selected_chat_group
                                && *idx >= state.chat_groups.groups.len()
                            {
                                state.selected_chat_group = SelectedChatGroup::All;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                bridge = %bridge_key,
                                error = %e,
                                "setAllChatGroups 解析失败"
                            );
                            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                                state.last_error =
                                    Some(format!("setAllChatGroups 解析失败: {}", e));
                                state.sync_status_history();
                            }
                        }
                    }
                }
                continue;
            }

            let should_refresh_relation_network = matches!(
                event_kind,
                BridgeEventKind::OnlineData(_)
                    | BridgeEventKind::SetAllRooms(_)
                    | BridgeEventKind::GroupMembersResponse(_)
            ) || matches!(
                event_kind,
                BridgeEventKind::CommandFailed(payload)
                    if payload.get("kind").and_then(serde_json::Value::as_str)
                        == Some("fetchGroupMembers")
            );

            let should_refresh_system_messages = {
                let state = &mut self.bridge_states[bridge_idx];
                let prev_auth_state = state.auth_state;

                state.last_event = Some(event_kind.name().to_string());

                Self::apply_bridge_event(state, event_kind);
                state.sync_status_history();

                // 仅在“本次事件使鉴权从未成功变为成功”时拉取一次系统消息，
                // 避免鉴权成功后每个事件都重复请求。
                prev_auth_state != AuthState::Succeeded && state.auth_state == AuthState::Succeeded
            };

            if should_refresh_relation_network {
                self.refresh_relation_network_after_bridge_update(bridge_idx);
            }

            if should_refresh_system_messages {
                self.request_system_messages(bridge_idx);
            }
        }

        if processed == MAX_EVENTS_PER_FRAME {
            ctx.request_repaint();
        }
    }
}

/// eframe 应用生命周期。
impl eframe::App for IcaApp {
    /// 退出前停止所有 bridge 会话，发送停止信号让后台任务收尾。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for session in &mut self.state.bridge_states {
            session.stop();
        }
    }

    /// 每帧绘制入口：先处理输入与事件，再按布局渲染各面板与独立窗口。
    ///
    /// 顺序重要：事件轮询要早于视图绘制，保证本帧反映最新状态；窄窗口下只显示一个
    /// 主面板，避免各区域互相挤压。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.sync_agent_context_config();
        self.update_chat_input(ui.ctx());
        self.handle_chat_navigation_shortcuts(ui.ctx());
        self.poll_socketio_events(ui.ctx());
        self.process_audio_actions();
        self.tick_auto_sign(ui.ctx());

        self.handle_chat_escape(ui.ctx());

        // 功能视图由 `chat` 和顶层 `shell` 实现。窄窗口只显示一个主面板，避免
        // 左侧导航、会话列表、成员面板和聊天正文互相挤压。
        let compact_layout = self.uses_compact_chat_layout(ui.ctx());
        if compact_layout
            && self.compact_chat_panel == CompactChatPanel::Chat
            && self
                .active_bridge_state()
                .and_then(|state| state.selected_room_id)
                .is_none()
        {
            self.compact_chat_panel = CompactChatPanel::Conversations;
        }
        self.render_top_panel(ui);
        let show_conversations =
            !compact_layout || self.compact_chat_panel == CompactChatPanel::Conversations;
        let show_chat = !compact_layout || self.compact_chat_panel == CompactChatPanel::Chat;
        if show_conversations {
            if !self.custom_chat.disable_chat_group && !self.custom_chat.hide_chat_group_sidebar {
                self.render_left_groups_panel(ui);
            }
            self.render_chat_list_panel(ui);
        }
        if show_chat {
            if self.active_chat_is_detached() {
                self.render_detached_chat_placeholder(ui);
            } else {
                self.render_group_members_panel(ui);
                self.render_central_panel(ui);
            }
        }
        self.render_group_ban_confirmation(ui.ctx());
        self.render_windows(ui);
        self.render_chat_windows(ui.ctx());
        self.render_agent_context(ui.ctx());
    }
}
