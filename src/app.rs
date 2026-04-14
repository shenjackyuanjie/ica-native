use std::{
    cmp::Reverse,
    collections::HashSet,
    path::Path,
    sync::Arc,
};

use eframe::CreationContext;
use rand::RngExt;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use serde_json::Value as JsonValue;

use crate::cfg::{self, ReEditDraftConflictMode};
use crate::ica;
use crate::{
    assets,
    ica::{IcaClient, IcaCommand},
};

pub mod chat_groups;
pub mod config_editer;
pub mod custom_chat;
pub mod events;
pub mod online_mode;
pub mod open_page;
pub mod renders;
pub mod state;

use chat_groups::ChatGroups;
use config_editer::ConfigEditer;
use custom_chat::CustomChat;
use online_mode::OnlineMode;
use open_page::AppOpenPage;
pub use state::*;

use crate::ica::types::{
    RoomId,
    message::{DeleteMessage, Message, ReplyMessage, SendMessage},
    room::Room,
};

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
    /// 是否选中某个聊天组
    pub chat_group_selected: bool,
    /// 选中了哪个聊天组
    pub chat_group_idx: usize,
    /// 聊天组
    pub chat_groups: ChatGroups,
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
            bridge_states.push(BridgeState::new(bridge_key.clone()));

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
            chat_group_selected: false,
            chat_group_idx: 0,
            chat_groups: ChatGroups::new(),
            config_editer: ConfigEditer::default(),
            chat_list_scroll_target: ChatListScrollTarget::Top,
            clear_search_on_room_select: config.ui_setting.clear_search_on_room_select,
            scroll_to_bottom_after_send: config.ui_setting.scroll_to_bottom_after_send,
            reedit_draft_conflict_mode: config.ui_setting.reedit_draft_conflict_mode,
            active_bridge_idx: if bridge_states.is_empty() { None } else { Some(0) },
            bridge_states,
            runtime,
            ica_clients,
            ui_rx,
            ui_tx,
            socketio_stop_senders,
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

    pub fn request_room_messages(&mut self, bridge_idx: usize, room_id: RoomId, scroll_to_bottom: bool) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            if scroll_to_bottom {
                state.pending_message_scroll_to_bottom.insert(room_id);
            } else {
                state.pending_message_scroll_to_bottom.remove(&room_id);
                state.message_scroll_to_bottom.remove(&room_id);
            }
        }

        // 房间历史消息通过 socket 任务去拉，GUI 本身不直接碰底层 client。
        if let Err(e) = client.command_tx.send(IcaCommand::FetchMessages(room_id)) {
            tracing::warn!("send fetchMessages command failed: {}", e);
        }
    }

    pub fn request_system_messages(&self, bridge_idx: usize) {
        let Some(client) = self.ica_clients.get(bridge_idx) else {
            return;
        };

        if let Err(e) = client.command_tx.send(IcaCommand::GetSystemMsg) {
            tracing::warn!("send getSystemMsg command failed: {}", e);
        }
    }

    pub fn visible_rooms(&self, bridge_idx: usize) -> Vec<Room> {
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            return Vec::new();
        };

        let query = state.room_search_query.trim().to_uppercase();
        let mut rooms: Vec<_> = state
            .rooms
            .iter()
            .filter(|room| {
                query.is_empty()
                    || room.room_name.to_uppercase().contains(&query)
                    || room.room_id.to_string().contains(query.as_str())
            })
            .cloned()
            .collect();

        rooms.sort_by_key(|room| Reverse(room.index > 0));
        rooms
    }

    fn extract_raw_chain(message: &Message) -> Option<JsonValue> {
        match &message.raw_msg {
            JsonValue::Array(values) if !values.is_empty() => Some(JsonValue::Array(values.clone())),
            JsonValue::Object(map) if map.contains_key("type") => {
                Some(JsonValue::Array(vec![message.raw_msg.clone()]))
            }
            _ => None,
        }
    }

    fn send_raw_chain(&mut self, room_id: RoomId, chain: JsonValue) -> bool {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendRawMessage { room_id, content: chain })
        {
            tracing::warn!("send raw sendMessage command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("原样发送命令发送失败: {}", room_id));
            }
            return false;
        }

        if self.scroll_to_bottom_after_send && let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.pending_send_scroll_to_bottom.insert(room_id);
        }
        true
    }

    fn clone_message_from_active_bridge(&self, room_id: RoomId, message_id: &str) -> Option<Message> {
        let bridge_idx = self.active_bridge_idx?;
        self.bridge_states
            .get(bridge_idx)?
            .find_message(room_id, message_id)
            .cloned()
    }

    fn selected_forward_messages(&self, bridge_idx: usize, room_id: RoomId) -> Vec<Message> {
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            return Vec::new();
        };
        if state.forward_room_id != Some(room_id) {
            return Vec::new();
        }

        let selected_ids: HashSet<&str> = state
            .forward_selected_message_ids
            .iter()
            .map(String::as_str)
            .collect();

        state
            .messages_by_room
            .get(&room_id)
            .into_iter()
            .flatten()
            .filter(|message| selected_ids.contains(message.msg_id.as_str()))
            .cloned()
            .collect()
    }

    fn send_message_clone_to_room(&mut self, target_room_id: RoomId, message: &Message) -> bool {
        if let Some(chain) = Self::extract_raw_chain(message) {
            return self.send_raw_chain(target_room_id, chain);
        }

        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };

        if message.content.trim().is_empty() {
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("该消息缺少可复用的原始节点，无法原样发送".to_string());
            }
            return false;
        }

        let outgoing = SendMessage::new(
            message.content.clone(),
            target_room_id,
            message.reply.clone(),
        );

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendMessage(outgoing))
        {
            tracing::warn!("send cloned sendMessage command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("消息发送失败: {}", target_room_id));
            }
            return false;
        }

        if !message.files.is_empty() && let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.last_error = Some("部分附件消息缺少原始节点，已退化为纯文本发送".to_string());
        }

        if self.scroll_to_bottom_after_send && let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.pending_send_scroll_to_bottom.insert(target_room_id);
        }
        true
    }

    pub fn copy_message_to_draft(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };

        if let Some(state) = self.active_bridge_state_mut() {
            state.draft_by_room.insert(room_id, message.content.clone());
            if let Some(reply) = message.reply.clone() {
                state.reply_to_by_room.insert(room_id, reply);
            } else {
                state.reply_to_by_room.remove(&room_id);
            }
            if !message.files.is_empty() {
                state.last_error = Some("复制到编辑区暂不恢复附件，如需原样发送请使用 +1 或 转发".to_string());
            }
        }
    }

    pub fn plus_one_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };
        let _ = self.send_message_clone_to_room(room_id, &message);
    }

    pub fn begin_forward_selection(&mut self, room_id: RoomId, message_id: String, open_picker: bool) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.replace_forward_selection(room_id, message_id);
            state.forward_target_picker_open = open_picker;
            if open_picker {
                state.forward_target_search_query.clear();
            }
        }
    }

    pub fn toggle_forward_message_selection(&mut self, room_id: RoomId, message_id: String) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.toggle_forward_selection(room_id, message_id);
        }
    }

    pub fn plus_one_forward_selection(&mut self, room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let messages = self.selected_forward_messages(bridge_idx, room_id);
        if messages.is_empty() {
            return;
        }

        let mut failed = 0_usize;
        for message in &messages {
            if !self.send_message_clone_to_room(room_id, message) {
                failed += 1;
            }
        }

        if failed > 0 && let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.last_error = Some(format!("有 {} 条消息无法完整 +1", failed));
        }
    }

    pub fn open_forward_target_picker(&mut self, room_id: RoomId) {
        if let Some(state) = self.active_bridge_state_mut() && state.is_forward_selection_active(room_id) {
            state.forward_target_picker_open = true;
            state.forward_target_search_query.clear();
        }
    }

    pub fn clear_forward_selection(&mut self) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.clear_forward_selection();
        }
    }

    pub fn forward_selected_messages_to_room(&mut self, target_room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(source_room_id) = self.bridge_states[bridge_idx].forward_room_id else {
            return;
        };
        let messages = self.selected_forward_messages(bridge_idx, source_room_id);
        if messages.is_empty() {
            self.bridge_states[bridge_idx].clear_forward_selection();
            return;
        }

        let mut failed = 0_usize;
        for message in &messages {
            if !self.send_message_clone_to_room(target_room_id, message) {
                failed += 1;
            }
        }

        if failed > 0 {
            self.bridge_states[bridge_idx].last_error = Some(format!("有 {} 条消息无法完整转发", failed));
        }
        self.bridge_states[bridge_idx].clear_forward_selection();
    }

    pub fn set_room_pinned(&mut self, bridge_idx: usize, room_id: RoomId, pin: bool) {
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };

        let previous_index = state
            .rooms
            .iter()
            .find(|room| room.room_id == room_id)
            .map(|room| room.index)
            .unwrap_or_default();

        if let Some(room) = state.rooms.iter_mut().find(|room| room.room_id == room_id) {
            room.index = if pin { 1 } else { 0 };
        }

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::PinRoom { room_id, pin })
        {
            tracing::warn!("send pinRoom command failed: {}", e);
            if let Some(room) = self.bridge_states[bridge_idx]
                .rooms
                .iter_mut()
                .find(|room| room.room_id == room_id)
            {
                room.index = previous_index;
            }
            self.bridge_states[bridge_idx].last_error = Some(format!("置顶命令发送失败: {}", room_id));
        }
    }

    pub fn remove_chat(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        state.rooms.retain(|room| room.room_id != room_id);
        if state.selected_room_id == Some(room_id) {
            state.selected_room_id = None;
        }
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::RemoveChat(room_id))
        {
            tracing::warn!("send removeChat command failed: {}", e);
        }
    }

    pub fn ignore_chat(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::IgnoreChat { room_id, room_name })
        {
            tracing::warn!("send ignoreChat command failed: {}", e);
        }
    }

    pub fn set_clear_search_on_room_select(&mut self, enabled: bool) {
        self.clear_search_on_room_select = enabled;
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.clear_search_on_room_select = enabled;
        });
    }

    pub fn set_scroll_to_bottom_after_send(&mut self, enabled: bool) {
        self.scroll_to_bottom_after_send = enabled;
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.scroll_to_bottom_after_send = enabled;
        });
    }

    pub fn set_reedit_draft_conflict_mode(&mut self, mode: ReEditDraftConflictMode) {
        self.reedit_draft_conflict_mode = mode;
        cfg::update_and_save_cfg(|cfg| {
            cfg.ui_setting.reedit_draft_conflict_mode = mode;
        });
    }

    fn image_mime_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            _ => None,
        }
    }

    fn load_pending_image(path: &Path) -> anyhow::Result<PendingImage> {
        let mime_type = Self::image_mime_type(path)
            .ok_or_else(|| anyhow::anyhow!("不支持的图片格式: {}", path.display()))?;
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());

        Ok(PendingImage {
            name,
            mime_type: mime_type.to_string(),
            data,
        })
    }

    fn guess_mime_type(path: &Path) -> String {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "flac" => "audio/flac",
            "mp4" => "video/mp4",
            "pdf" => "application/pdf",
            "zip" => "application/zip",
            "txt" => "text/plain",
            _ => "application/octet-stream",
        }
        .to_string()
    }

    fn load_pending_file(path: &Path) -> anyhow::Result<PendingFile> {
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let file_type = Self::guess_mime_type(path);

        Ok(PendingFile {
            name,
            file_type,
            data,
        })
    }

    pub fn select_active_room(&mut self, room_id: RoomId) {
        let mut should_request = false;
        let clear_search_on_room_select = self.clear_search_on_room_select;
        if let Some(state) = self.active_bridge_state_mut() {
            state.selected_room_id = Some(room_id);
            if clear_search_on_room_select {
                state.room_search_query.clear();
            }
            should_request = state.requested_rooms.insert(room_id);
        }

        if should_request && let Some(bridge_idx) = self.active_bridge_idx {
            self.request_room_messages(bridge_idx, room_id, true);
        }
    }

    pub fn send_current_message(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let scroll_to_bottom_after_send = self.scroll_to_bottom_after_send;
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let Some(room_id) = state.selected_room_id else {
            return;
        };

        let draft = state.draft_by_room.entry(room_id).or_default();
        let content = draft.trim().to_string();
        let reply_to = state.reply_to_by_room.remove(&room_id);
        let pending_image = state.pending_image_by_room.remove(&room_id);
        let pending_file = state.pending_file_by_room.remove(&room_id);
        if content.is_empty() && pending_image.is_none() && pending_file.is_none() {
            if let Some(reply_to) = reply_to {
                state.reply_to_by_room.insert(room_id, reply_to);
            }
            return;
        }
        draft.clear();

        // 文件走分块上传协议
        if let Some(file) = pending_file {
            let command = IcaCommand::SendFileMessage {
                room_id,
                content: content.clone(),
                reply_to: reply_to.clone(),
                file_name: file.name,
                file_type: file.file_type,
                file_data: file.data,
            };
            if let Err(e) = self.ica_clients[bridge_idx].command_tx.send(command) {
                tracing::warn!("send sendFileMessage command failed: {}", e);
                state.draft_by_room.insert(room_id, content);
                if let Some(reply_to) = reply_to {
                    state.reply_to_by_room.insert(room_id, reply_to);
                }
            } else if scroll_to_bottom_after_send {
                state.pending_send_scroll_to_bottom.insert(room_id);
            }
            return;
        }

        // 图片走 base64 内联
        let mut message = SendMessage::new(content.clone(), room_id, reply_to.clone());
        if let Some(image) = pending_image.as_ref() {
            message.set_img(&image.data, &image.mime_type, false);
        }
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::SendMessage(message))
        {
            tracing::warn!("send sendMessage command failed: {}", e);
            state.draft_by_room.insert(room_id, content);
            if let Some(reply_to) = reply_to {
                state.reply_to_by_room.insert(room_id, reply_to);
            }
            if let Some(image) = pending_image {
                state.pending_image_by_room.insert(room_id, image);
            }
        } else if scroll_to_bottom_after_send {
            state.pending_send_scroll_to_bottom.insert(room_id);
        }
    }

    pub fn queue_reply(&mut self, room_id: RoomId, reply: ReplyMessage) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.reply_to_by_room.insert(room_id, reply);
        }
    }

    pub fn send_delete_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let message = DeleteMessage::new(room_id, message_id.clone());
        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::DeleteMessage(message))
        {
            tracing::warn!("send deleteMessage command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("撤回消息命令发送失败: {}", message_id));
            }
        }
    }

    pub fn set_message_reveal(&mut self, room_id: RoomId, message_id: String, reveal: bool) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let command = if reveal {
            IcaCommand::RevealMessage {
                room_id,
                message_id: message_id.clone(),
            }
        } else {
            IcaCommand::HideMessage {
                room_id,
                message_id: message_id.clone(),
            }
        };

        if let Err(e) = self.ica_clients[bridge_idx].command_tx.send(command) {
            tracing::warn!("send reveal/hide message command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("显示/隐藏消息命令发送失败: {}", message_id));
            }
            return;
        }

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            if reveal {
                state.mark_message_revealed(&message_id);
            } else {
                state.mark_message_hidden(&message_id);
            }
        }
    }

    pub fn restore_deleted_message_to_draft(&mut self, room_id: RoomId, content: String) {
        let mode = self.reedit_draft_conflict_mode;
        if let Some(state) = self.active_bridge_state_mut() {
            let draft = state.draft_by_room.entry(room_id).or_default();
            match mode {
                ReEditDraftConflictMode::Overwrite => {
                    *draft = content;
                }
                ReEditDraftConflictMode::Append => {
                    if draft.trim().is_empty() {
                        *draft = content;
                    } else if !content.trim().is_empty() {
                        if !draft.ends_with('\n') {
                            draft.push('\n');
                        }
                        draft.push_str(&content);
                    }
                }
                ReEditDraftConflictMode::SkipIfNonEmpty => {
                    if draft.trim().is_empty() {
                        *draft = content;
                    }
                }
            }
        }
    }

    pub fn handle_join_request(&mut self, request_type: String, flag: String, accept: bool) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        if let Err(e) = self.ica_clients[bridge_idx]
            .command_tx
            .send(IcaCommand::HandleRequest {
                request_type: request_type.clone(),
                flag: flag.clone(),
                accept,
            })
        {
            tracing::warn!("send handleRequest command failed: {}", e);
            if let Some(state) = self.active_bridge_state_mut() {
                state.last_error = Some(format!("验证消息操作发送失败: {}", flag));
            }
            return;
        }

        if let Some(state) = self.active_bridge_state_mut() {
            state.join_requests.retain(|request| request.flag != flag);
        }
    }

    pub fn pick_image_for_current_room(&mut self) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self.bridge_states[active_bridge_idx].selected_room_id else {
            return;
        };

        let Some(path) = rfd::FileDialog::new()
            .add_filter("image", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .pick_file()
        else {
            return;
        };

        match Self::load_pending_image(&path) {
            Ok(image) => {
                self.bridge_states[active_bridge_idx]
                    .pending_image_by_room
                    .insert(room_id, image);
            }
            Err(e) => {
                self.bridge_states[active_bridge_idx].last_error = Some(e.to_string());
            }
        }
    }

    pub fn pick_file_for_current_room(&mut self) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(room_id) = self.bridge_states[active_bridge_idx].selected_room_id else {
            return;
        };

        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };

        match Self::load_pending_file(&path) {
            Ok(file) => {
                self.bridge_states[active_bridge_idx]
                    .pending_file_by_room
                    .insert(room_id, file);
            }
            Err(e) => {
                self.bridge_states[active_bridge_idx].last_error = Some(e.to_string());
            }
        }
    }

    fn poll_socketio_events(&mut self) {
        while let Ok(event) = self.ui_rx.try_recv() {
            let Some(event_name) = event.get("event").and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(bridge_key) = event.get("bridge").and_then(|value| value.as_str()) else {
                continue;
            };

            let payload = event.get("payload").cloned().unwrap_or(JsonValue::Null);
            let Some(bridge_idx) = self
                .bridge_states
                .iter()
                .position(|state| state.bridge_key == bridge_key)
            else {
                continue;
            };

            let should_refresh_system_messages = {
                let state = &mut self.bridge_states[bridge_idx];
                let prev_auth_state = state.auth_state;

                state.last_event = Some(event_name.to_string());

                Self::apply_socketio_event(state, event_name, &payload);

                prev_auth_state != AuthState::Succeeded && state.auth_state == AuthState::Succeeded
            };

            if should_refresh_system_messages {
                self.request_system_messages(bridge_idx);
            }
        }
    }
}

impl eframe::App for IcaApp {
    fn on_exit(&mut self) {
        for sender in self.socketio_stop_senders.drain(..) {
            let _ = sender.send(());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_socketio_events();

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
