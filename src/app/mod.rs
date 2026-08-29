use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use crate::assets;
use crate::config::ConfigStore;
use eframe::CreationContext;
use rand::RngExt;

pub mod auto_sign;
mod chat;
pub mod chat_groups;
mod contacts;
mod event;
mod media;
pub mod online_mode;
pub mod open_page;
mod relation_network;
mod runtime;
pub mod settings;
mod shell;
pub mod state;
pub mod stickers;
mod tools;

use media::{MediaEvent, MediaTask};
use runtime::AppRuntime;
pub use state::*;

use crate::ica::BridgeEventKind;
use crate::ica::types::room::Room;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedChatGroup {
    All,
    Group,
    Private,
    Custom(usize),
}

pub struct IcaApp {
    runtime: AppRuntime,
    config: ConfigStore,
    state: state::AppState,
}

impl Deref for IcaApp {
    type Target = state::AppState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for IcaApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl IcaApp {
    pub(super) fn update_config(&self, updater: impl FnOnce(&mut crate::config::IcaCfg)) {
        self.config.update(updater);
        if let Err(error) = self.config.save() {
            tracing::error!("保存配置失败: {error}");
        }
    }

    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        let font_sy_data = egui::FontData::from_static(assets::fonts::FONT_思源黑体);
        let font_unifont_data = egui::FontData::from_static(assets::fonts::FONT_UNIFONT);

        let sy_font_name = "notosans".to_string();
        let unifont_name = "unifont".to_string();

        fonts
            .font_data
            .insert(sy_font_name.clone(), Arc::new(font_sy_data));

        fonts
            .font_data
            .insert(unifont_name.clone(), Arc::new(font_unifont_data));

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, unifont_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(unifont_name.clone());

        ctx.set_fonts(fonts);
    }

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
        !self.custom_chat.disable_adaptive_single_panel_mode && ctx.content_rect().width() < 720.0
    }

    /// 生成测试用的聊天室数据
    #[allow(unused)]
    fn test_chat_rooms() -> Vec<Room> {
        // 生成随机房间数据
        use rand::Rng;
        use rand::rng;
        use rand::seq::SliceRandom;
        let mut rooms = Vec::with_capacity(50);
        let room_names = vec![
            "测试群聊",
            "开发讨论组",
            "项目协作",
            "闲聊灌水",
            "技术交流",
            "学习小组",
            "游戏开黑",
            "音乐分享",
            "读书会",
            "运动健身",
        ];

        let user_names = vec![
            "张三",
            "李四",
            "王五",
            "赵六",
            "钱七",
            "孙八",
            "周九",
            "吴十",
            "郑十一",
            "王十二",
        ];

        let message_templates = vec![
            "大家好！今天天气不错",
            "有人在线吗？",
            "这个功能什么时候能做完？",
            "晚上一起吃饭吗？",
            "[图片]",
            "我刚刚上传了文件",
            "明天会议几点开始？",
            "这个问题怎么解决？",
            "有人玩{}吗？",
            "推荐一个好看的{}",
        ];

        let mut rng = rng();

        for i in 0..500 {
            let room_name_idx = rng.random_range(0..room_names.len());
            let user_idx = rng.random_range(0..user_names.len());
            let message_idx = rng.random_range(0..message_templates.len());

            // 随机生成消息内容
            let mut message = message_templates[message_idx].to_string();
            if message.contains("{}") {
                let replacements = ["游戏", "电影", "书", "餐厅", "音乐", "软件"];
                let replacement = replacements[rng.random_range(0..replacements.len())];
                message = message.replace("{}", replacement);
            }

            // 随机添加表情或标签
            if rng.random_bool(0.3) {
                message += if rng.random_bool(0.5) {
                    " 😊"
                } else {
                    " #标签"
                };
            }

            rooms.push(Room {
                room_id: if rng.random_bool(0.7) {
                    -rng.random_range(100_000_000..1_000_000_000)
                } else {
                    rng.random_range(100_000_000..1_000_000_000)
                },
                room_name: format!("{} {}", room_names[room_name_idx], rng.random_range(1..100)),
                index: i as i64 + 1,
                unread_count: rng.random_range(0..100),
                priority: rng.random_range(1..4),
                utime: 1700000000 + rng.random_range(0..100000),
                users: serde_json::json!([
                    { "_id": 1, "username": "1" },
                    { "_id": 2, "username": "2" }
                ]),
                at: match rng.random_range(0..5) {
                    0 => crate::ica::types::message::At::All,
                    1 => crate::ica::types::message::At::Bool(rng.random_bool(0.2)),
                    _ => crate::ica::types::message::At::None,
                },
                last_message: crate::ica::types::message::LastMessage {
                    content: Some(message),
                    timestamp: Some(match rng.random_range(0..4) {
                        0 => "刚刚".to_string(),
                        1 => format!("{}:{}", rng.random_range(0..24), rng.random_range(0..60)),
                        2 => "昨天".to_string(),
                        _ => "前天".to_string(),
                    }),
                    username: Some(user_names[user_idx].to_string()),
                    user_id: Some(rng.random_range(100_000_000..1_000_000_000)),
                },
            });
        }

        // 打乱房间顺序
        rooms.shuffle(&mut rng);
        rooms
    }

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

    fn spawn_media_task(&self, ctx: &egui::Context, task: MediaTask) {
        let event_tx = self.runtime.event_sender();
        let repaint = ctx.clone();
        self.runtime.spawn(async move {
            let event = task.run().await;
            let _ = event_tx.send(event::AppEvent::Media(event));
            repaint.request_repaint();
        });
    }

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

    pub fn active_bridge_state(&self) -> Option<&BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get(idx))
            .map(|session| session.state())
    }

    pub fn active_bridge_state_mut(&mut self) -> Option<&mut BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get_mut(idx))
            .map(|session| session.state_mut())
    }

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

    fn poll_socketio_events(&mut self, ctx: &egui::Context) {
        const MAX_EVENTS_PER_FRAME: usize = 128;
        let mut processed = 0;

        while processed < MAX_EVENTS_PER_FRAME {
            let Ok(event) = self.runtime.event_rx.try_recv() else {
                break;
            };
            let event = match event {
                event::AppEvent::Bridge(event) => event,
                event::AppEvent::Media(event) => {
                    self.apply_media_event(event);
                    continue;
                }
            };
            processed += 1;
            let bridge_key = event.bridge_key.as_str();
            let event_kind = &event.kind;
            let Some(bridge_idx) = self
                .bridge_states
                .iter()
                .position(|state| state.bridge_key == bridge_key)
            else {
                continue;
            };

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

impl eframe::App for IcaApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.ime_event_this_frame = false;
        for event in &raw_input.events {
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
            clipboard_image_paste_requested(raw_input, system_paste_shortcut_pressed);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for session in &mut self.state.bridge_states {
            session.stop();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_socketio_events(ui.ctx());
        self.tick_auto_sign(ui.ctx());

        // ESC 优先关闭输入区弹层，再处理会话内的选择状态。
        if ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)) {
            if self.show_mention_picker {
                self.show_mention_picker = false;
                self.mention_search_query.clear();
                self.mention_search_focus_requested = false;
                self.mention_replace_trigger = false;
                self.mention_selected_index = 0;
                if let Some(bridge_idx) = self.active_bridge_idx
                    && let Some(room_id) = self.bridge_states[bridge_idx].selected_room_id
                {
                    let composer_id = egui::Id::new(("message_composer", bridge_idx, room_id));
                    ui.ctx()
                        .memory_mut(|memory| memory.request_focus(composer_id));
                }
            } else if let Some(state) = self.active_bridge_state_mut() {
                if state.forward_target_picker_open {
                    state.forward_target_picker_open = false;
                } else if let Some(room_id) = state.selected_room_id {
                    if state.is_forward_selection_active(room_id) {
                        state.clear_forward_selection();
                    } else {
                        state.selected_room_id = None;
                    }
                }
            }
        }

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
            self.render_group_members_panel(ui);
            self.render_central_panel(ui);
        }
        self.render_group_ban_confirmation(ui.ctx());
        self.render_windows(ui);
    }
}
