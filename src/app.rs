use std::sync::Arc;

use eframe::CreationContext;
use rand::RngExt;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use serde_json::Value as JsonValue;

use crate::cfg::{self, ReEditDraftConflictMode};
use crate::ica;
use crate::{assets, ica::IcaClient};

mod account_tools;
mod actions;
pub mod auto_sign;
pub mod chat_group_editor;
pub mod chat_groups;
mod clipboard;
pub mod config_editer;
pub mod custom_chat;
pub mod events;
mod file_tools;
mod group_tools;
mod message_ops;
mod message_tools;
pub mod online_mode;
pub mod open_page;
pub mod renders;
mod room_ops;
mod room_tools;
mod socket_api;
pub mod state;

use account_tools::AccountToolsState;
use auto_sign::AutoSignState;
use chat_groups::ChatGroups;
use config_editer::ConfigEditer;
use custom_chat::CustomChat;
use file_tools::FileToolsState;
use group_tools::GroupToolsState;
use message_tools::MessageToolsState;
use online_mode::OnlineMode;
use open_page::AppOpenPage;
use room_tools::RoomToolsState;
pub use state::*;

use crate::ica::types::{RoomId, room::Room};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedChatGroup {
    All,
    Private,
    Custom(usize),
}

pub struct IcaApp {
    /// 聊天界面定制选项
    pub custom_chat: CustomChat,
    /// 在线模式
    pub online_mode: OnlineMode,
    /// 打开了什么页面
    pub open_page: AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
    /// 当前选中的聊天分组
    pub selected_chat_group: SelectedChatGroup,
    /// 聊天组
    pub chat_groups: ChatGroups,
    /// 聊天分组编辑器
    pub chat_group_editor: chat_group_editor::ChatGroupEditor,
    /// 配置文件修改
    pub config_editer: ConfigEditer,
    /// 聊天列表滚动目标
    pub chat_list_scroll_target: ChatListScrollTarget,
    /// 选中会话后是否自动清空搜索框
    pub clear_search_on_room_select: bool,
    /// 发送消息后是否自动滚动到底部
    pub scroll_to_bottom_after_send: bool,
    /// 已撤回消息重新编辑时，遇到已有草稿如何处理
    pub reedit_draft_conflict_mode: ReEditDraftConflictMode,
    /// 当前选中的 bridge
    pub active_bridge_idx: Option<usize>,
    /// 每个 bridge 的界面状态
    pub bridge_states: Vec<BridgeState>,
    /// tokio rt
    /// 用来开 socketio
    pub runtime: Runtime,
    /// Socketio 列表
    /// 一些 Socketio 连接
    pub ica_clients: Vec<IcaClient>,
    /// GUI 侧接收事件的 channel
    pub ui_rx: UnboundedReceiver<JsonValue>,
    /// 发送事件到 GUI 的 channel
    pub ui_tx: UnboundedSender<JsonValue>,
    /// Socketio 停止信号
    pub socketio_stop_senders: Vec<oneshot::Sender<()>>,
    /// Ctrl+V 但文字粘贴失败（可能剪贴板是图片）
    pub clipboard_paste_failed: bool,
    /// 输入法当前是否存在预编辑文本。
    pub ime_composing: bool,
    /// 当前输入帧是否包含输入法事件，避免提交候选词时误发送。
    pub ime_event_this_frame: bool,
    /// 是否显示表情选择器
    pub show_face_picker: bool,
    /// 是否显示群成员 @ 选择器。
    pub show_mention_picker: bool,
    /// @ 选择器搜索词。
    pub mention_search_query: String,
    /// @ 选择器打开后，下一帧是否自动聚焦搜索框。
    pub mention_search_focus_requested: bool,
    /// 是否由输入字符 @ 触发；选中成员时需替换该字符。
    pub mention_replace_trigger: bool,
    /// 图片查看器状态（与独立窗口共享）
    pub image_viewer: Option<std::sync::Arc<std::sync::Mutex<state::ImageViewerState>>>,
    /// 高级 socket.io API 调用事件名
    pub socket_api_event: String,
    /// 高级 socket.io API 调用参数，JSON 数组
    pub socket_api_args: String,
    /// 高级 socket.io API 调用是否等待 ack
    pub socket_api_expect_ack: bool,
    /// 高级 socket.io API 当前预设索引
    pub socket_api_preset_idx: usize,
    /// 群/成员管理工具状态
    pub group_tools: GroupToolsState,
    /// 账号/登录设备工具状态
    pub account_tools: AccountToolsState,
    /// 文件/资源工具状态
    pub file_tools: FileToolsState,
    /// 消息检索/历史工具状态
    pub message_tools: MessageToolsState,
    /// 会话设置工具状态
    pub room_tools: RoomToolsState,
    /// 全群自动签到状态
    pub auto_sign: AutoSignState,
}

impl IcaApp {
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

    fn setup_async_rt() -> Runtime {
        let config = crate::cfg::get_cfg_snapshot();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("faild to build tokio rt")
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

    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);

        let (ui_tx, ui_rx) = unbounded_channel::<JsonValue>();

        let config = cfg::get_cfg_snapshot();
        let runtime = Self::setup_async_rt();

        let mut socketio_stop_senders = Vec::new();
        let mut ica_clients = Vec::new();
        let mut bridge_states = Vec::new();

        for bridge in config.bridges.clone() {
            if !bridge.enable {
                continue;
            }
            let (stop_tx, stop_rx) = oneshot::channel();
            socketio_stop_senders.push(stop_tx);

            let bridge_key = if bridge.name.is_empty() {
                bridge.url.clone()
            } else {
                bridge.name.clone()
            };
            let (command_tx, command_rx) = unbounded_channel();
            ica_clients.push(IcaClient {
                bridge_key: bridge_key.clone(),
                command_tx,
            });
            bridge_states.push(BridgeState::new(
                bridge_key.clone(),
                config.chat_groups.clone(),
            ));

            let ui_tx_clone = ui_tx.clone();
            runtime.spawn(async move {
                if let Err(e) = ica::main(stop_rx, &bridge, Some(ui_tx_clone), command_rx).await {
                    tracing::error!("socketio bridge {} stopped with error: {}", bridge_key, e);
                }
            });
        }

        Self {
            custom_chat: config.custom_chat.clone(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            selected_chat_group: SelectedChatGroup::All,
            chat_groups: config.chat_groups.clone(),
            chat_group_editor: chat_group_editor::ChatGroupEditor::default(),
            config_editer: ConfigEditer::default(),
            chat_list_scroll_target: ChatListScrollTarget::Top,
            clear_search_on_room_select: config.ui_setting.clear_search_on_room_select,
            scroll_to_bottom_after_send: config.ui_setting.scroll_to_bottom_after_send,
            reedit_draft_conflict_mode: config.ui_setting.reedit_draft_conflict_mode,
            active_bridge_idx: if bridge_states.is_empty() {
                None
            } else {
                Some(0)
            },
            bridge_states,
            runtime,
            ica_clients,
            ui_rx,
            ui_tx,
            socketio_stop_senders,
            clipboard_paste_failed: false,
            ime_composing: false,
            ime_event_this_frame: false,
            show_face_picker: false,
            show_mention_picker: false,
            mention_search_query: String::new(),
            mention_search_focus_requested: false,
            mention_replace_trigger: false,
            image_viewer: None,
            socket_api_event: String::new(),
            socket_api_args: "[]".to_string(),
            socket_api_expect_ack: true,
            socket_api_preset_idx: 0,
            group_tools: GroupToolsState::default(),
            account_tools: AccountToolsState::default(),
            file_tools: FileToolsState::default(),
            message_tools: MessageToolsState::default(),
            room_tools: RoomToolsState::default(),
            auto_sign: AutoSignState::default(),
        }
    }

    pub fn active_bridge_state(&self) -> Option<&BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get(idx))
    }

    pub fn active_bridge_state_mut(&mut self) -> Option<&mut BridgeState> {
        self.active_bridge_idx
            .and_then(|idx| self.bridge_states.get_mut(idx))
    }

    pub fn switch_active_bridge(&mut self, bridge_idx: usize) {
        if self.active_bridge_idx == Some(bridge_idx) || bridge_idx >= self.bridge_states.len() {
            return;
        }

        if let Some(previous_idx) = self.active_bridge_idx
            && let Some(previous) = self.bridge_states.get_mut(previous_idx)
        {
            previous.chat_groups = self.chat_groups.clone();
            previous.selected_chat_group = self.selected_chat_group.clone();
        }

        let next = &self.bridge_states[bridge_idx];
        self.chat_groups = next.chat_groups.clone();
        self.selected_chat_group = next.selected_chat_group.clone();
        self.active_bridge_idx = Some(bridge_idx);
        self.ensure_selected_chat_group_valid();
        self.chat_list_scroll_target = ChatListScrollTarget::Top;
        self.show_face_picker = false;
        self.show_mention_picker = false;
        self.mention_search_query.clear();
        self.mention_search_focus_requested = false;
        self.mention_replace_trigger = false;
    }

    fn poll_socketio_events(&mut self, ctx: &egui::Context) {
        const MAX_EVENTS_PER_FRAME: usize = 128;
        let mut processed = 0;

        while processed < MAX_EVENTS_PER_FRAME {
            let Ok(event) = self.ui_rx.try_recv() else {
                break;
            };
            processed += 1;
            let Some(event_name) = event.get("event").and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(bridge_key) = event.get("bridge").and_then(|value| value.as_str()) else {
                continue;
            };

            let null_payload = JsonValue::Null;
            let payload = event.get("payload").unwrap_or(&null_payload);
            let Some(bridge_idx) = self
                .bridge_states
                .iter()
                .position(|state| state.bridge_key == bridge_key)
            else {
                continue;
            };

            if event_name == "setAllChatGroups" {
                if let Some(value) = payload.as_array().and_then(|values| values.first()) {
                    match <Vec<chat_groups::ChatGroup> as serde::Deserialize>::deserialize(value) {
                        Ok(groups) => {
                            let state = &mut self.bridge_states[bridge_idx];
                            state.chat_groups.groups = groups;
                            if let SelectedChatGroup::Custom(idx) = &state.selected_chat_group
                                && *idx >= state.chat_groups.groups.len()
                            {
                                state.selected_chat_group = SelectedChatGroup::All;
                            }
                            if self.active_bridge_idx == Some(bridge_idx) {
                                self.chat_groups = state.chat_groups.clone();
                                self.selected_chat_group = state.selected_chat_group.clone();
                            }
                        }
                        Err(e) => {
                            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                                state.last_error =
                                    Some(format!("setAllChatGroups 解析失败: {}", e));
                            }
                        }
                    }
                }
                continue;
            }

            let should_refresh_system_messages = {
                let state = &mut self.bridge_states[bridge_idx];
                let prev_auth_state = state.auth_state;

                state.last_event = Some(event_name.to_string());

                Self::apply_socketio_event(state, event_name, payload);

                prev_auth_state != AuthState::Succeeded && state.auth_state == AuthState::Succeeded
            };

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
        let has_paste = raw_input
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::Paste(_)));
        let paste_shortcut = raw_input.events.iter().any(|e| {
            if let egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                modifiers,
                ..
            } = e
            {
                raw_input.modifiers.command || modifiers.command
            } else {
                false
            }
        });
        self.clipboard_paste_failed =
            !has_paste && (paste_shortcut || Self::system_paste_shortcut_pressed());
    }

    fn on_exit(&mut self) {
        for sender in self.socketio_stop_senders.drain(..) {
            let _ = sender.send(());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_socketio_events(ui.ctx());
        self.tick_auto_sign(ui.ctx());

        // 检测 ESC 键取消选择
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
            && let Some(state) = self.active_bridge_state_mut()
        {
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

        // 渲染相关的方法已移到 `renders.rs` 模块
        self.render_top_panel(ui);
        self.render_left_groups_panel(ui);
        self.render_chat_list_panel(ui);
        self.render_central_panel(ui);
        self.render_windows(ui);
    }
}
