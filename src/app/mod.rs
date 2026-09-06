use std::ops::{Deref, DerefMut};

use crate::config::ConfigStore;
use eframe::CreationContext;
use rand::RngExt;

pub mod auto_sign;
mod chat;
pub mod chat_groups;
mod chat_windows;
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
    chat_windows: Vec<chat_windows::ChatWindow>,
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
    pub fn update_config(&self, updater: impl FnOnce(&mut crate::config::IcaCfg)) {
        self.config.update(updater);
        if let Err(error) = self.config.save() {
            tracing::error!("保存配置失败: {error}");
        }
    }

    pub fn handle_chat_escape(&mut self, ctx: &egui::Context) {
        // ESC 优先关闭输入区弹层，再处理会话内的选择状态。
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
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
                    ctx.memory_mut(|memory| memory.request_focus(composer_id));
                }
            } else if let Some(state) = self.active_bridge_state_mut() {
                if state.forward_target_picker_open {
                    state.forward_target_picker_open = false;
                } else if let Some(room_id) = state.selected_room_id {
                    if state.is_forward_selection_active(room_id) {
                        state.clear_forward_selection();
                    } else if ctx.viewport_id() == egui::ViewportId::ROOT {
                        state.selected_room_id = None;
                    }
                }
            }
        }
    }

    pub fn update_chat_input(&mut self, ctx: &egui::Context) {
        let raw_input = ctx.input(|input| input.raw.clone());
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
            clipboard_image_paste_requested(&raw_input, system_paste_shortcut_pressed);
    }

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
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for session in &mut self.state.bridge_states {
            session.stop();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_chat_input(ui.ctx());
        self.poll_socketio_events(ui.ctx());
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
    }
}

#[cfg(test)]
mod system_font_tests {
    use super::IcaApp;

    #[test]
    fn bundled_fonts_surround_system_fallbacks() {
        let ctx = egui::Context::default();
        IcaApp::setup_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts(|fonts| {
                let definitions = fonts.definitions();
                let proportional = &definitions.families[&egui::FontFamily::Proportional];
                assert_eq!(proportional.first().map(String::as_str), Some("notosans"));
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    assert_eq!(
                        definitions.families[&family].last().map(String::as_str),
                        Some("unifont")
                    );
                }
            });
        });
        output.textures_delta.clear();
    }

    #[test]
    #[ignore = "需要系统安装中文字体和含思考表情轮廓的 emoji 字体；字体方案变更时在桌面环境显式运行"]
    fn system_fonts_render_chinese_and_thinking_face() {
        let ctx = egui::Context::default();
        IcaApp::setup_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    let font_id = egui::FontId::new(18.0, family);
                    // egui 0.36 的 has_glyph 按字体身份判断是否命中替代字形，
                    // 会误判与替代字形同属一套字体的正常字符。直接比较实际网格 UV。
                    let missing = fonts.layout_no_wrap(
                        "\u{10ffff}".into(),
                        font_id.clone(),
                        egui::Color32::WHITE,
                    );
                    let missing_uv: Vec<_> = missing
                        .rows
                        .iter()
                        .flat_map(|row| row.visuals.mesh.vertices.iter().map(|vertex| vertex.uv))
                        .collect();
                    for character in ['中', 'A', '1', '🤔'] {
                        let galley = fonts.layout_no_wrap(
                            character.to_string(),
                            font_id.clone(),
                            egui::Color32::WHITE,
                        );
                        let glyph_uv: Vec<_> = galley
                            .rows
                            .iter()
                            .flat_map(|row| {
                                row.visuals.mesh.vertices.iter().map(|vertex| vertex.uv)
                            })
                            .collect();
                        assert_ne!(glyph_uv, missing_uv, "字符落到了缺字替代图形: {character}");
                        assert!(
                            galley
                                .rows
                                .iter()
                                .any(|row| !row.visuals.mesh.vertices.is_empty()),
                            "字形没有生成可绘制轮廓: {character}"
                        );
                    }
                }
            });
        });
        output.textures_delta.clear();
    }
}
