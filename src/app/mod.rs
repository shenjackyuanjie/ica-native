use std::{cell::RefCell, collections::HashMap, time::Duration};

use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, InteractiveElement, KeyBinding, ObjectFit,
    ScrollHandle, SharedString, Window, div, img, prelude::*, px, relative,
};
use rand::RngExt;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use theme::{ActiveTheme, Appearance, GlobalTheme, SystemAppearance, ThemeRegistry};
use ui::{Avatar, Color, Icon, IconName, IconSize, Tooltip};

use crate::config::chat_groups::ChatGroup;
use crate::config::{ChatGroups, ConfigStore, IcaCfg, ReEditDraftConflictMode, ThemeMode};
use crate::ica::types::{
    RoomId,
    contact::{FriendContact, GroupContact},
    message::{
        At, DeleteMessage, LastMessage, Mention, Message, NewMessage, ReplyMessage, SendMessage,
    },
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};
use crate::ica::{BridgeEvent, BridgeEventKind, IcaCommand};

mod input;
pub mod runtime;
mod stickers;

use input::{InputEvent, InputPresentation, TextInput};
use runtime::AppRuntime;
use stickers::{StickerEntry, StickerStore};

fn forward_resource(code: &JsonValue) -> Option<(String, Option<String>)> {
    let parsed;
    let value = if let Some(raw) = code.as_str() {
        if let Ok(json) = serde_json::from_str::<JsonValue>(raw) {
            parsed = json;
            &parsed
        } else if let Ok(document) = roxmltree::Document::parse(raw) {
            let node = document.descendants().find(|node| {
                node.attribute("m_resid").is_some() || node.attribute("m_fileName").is_some()
            })?;
            let res_id = node.attribute("m_resid")?.trim().to_string();
            let file_name = node.attribute("m_fileName").map(ToString::to_string);
            return (!res_id.is_empty()).then_some((res_id, file_name));
        } else {
            return None;
        }
    } else {
        code
    };
    let detail = value.pointer("/meta/detail")?;
    let res_id = detail.get("resid")?.as_str()?.trim().to_string();
    let file_name = detail
        .get("uniseq")
        .or_else(|| detail.get("fileName"))
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    (!res_id.is_empty()).then_some((res_id, file_name))
}

fn collect_forward_lines(value: &JsonValue, lines: &mut Vec<String>) {
    if lines.len() >= 80 {
        return;
    }
    match value {
        JsonValue::Array(values) => {
            for value in values {
                collect_forward_lines(value, lines);
            }
        }
        JsonValue::Object(object) => {
            if let Some(content) = object.get("content").and_then(JsonValue::as_str) {
                let sender = object
                    .get("username")
                    .or_else(|| object.get("senderName"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or("消息");
                lines.push(format!("{sender}：{content}"));
            }
            for value in object.values() {
                collect_forward_lines(value, lines);
            }
        }
        _ => {}
    }
}

async fn download_image_bytes(url: &str) -> Result<(gpui::ImageFormat, Vec<u8>), String> {
    let response = reqwest::get(url).await.map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let format = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|mime| gpui::ImageFormat::from_mime_type(mime.split(';').next().unwrap_or(mime)))
        .or_else(|| {
            let path = url
                .split(['?', '#'])
                .next()
                .unwrap_or(url)
                .to_ascii_lowercase();
            if path.ends_with(".jpg") || path.ends_with(".jpeg") {
                Some(gpui::ImageFormat::Jpeg)
            } else if path.ends_with(".gif") {
                Some(gpui::ImageFormat::Gif)
            } else if path.ends_with(".webp") {
                Some(gpui::ImageFormat::Webp)
            } else if path.ends_with(".bmp") {
                Some(gpui::ImageFormat::Bmp)
            } else {
                Some(gpui::ImageFormat::Png)
            }
        })
        .unwrap_or(gpui::ImageFormat::Png);
    let bytes = response
        .bytes()
        .await
        .map_err(|error| error.to_string())?
        .to_vec();
    Ok((format, bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Chat,
    Groups,
    Contacts,
    Requests,
    Relation,
    Tools,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RoomFilter {
    #[default]
    All,
    Private,
    Group,
    Custom(usize),
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Self::Chat => "消息",
            Self::Groups => "分组",
            Self::Contacts => "联系人",
            Self::Requests => "验证",
            Self::Relation => "关系",
            Self::Tools => "工具",
            Self::Settings => "设置",
        }
    }
}

#[derive(Default)]
struct Conversation {
    messages: Vec<Message>,
    reply_to: Option<ReplyMessage>,
    requested: bool,
    no_more_history: bool,
    draft: String,
    mentions: Vec<Mention>,
    pending_images: Vec<PendingImage>,
    pending_file: Option<PendingFile>,
}

#[derive(Clone)]
struct PendingImage {
    name: String,
    mime: String,
    data: std::sync::Arc<[u8]>,
}

#[derive(Clone)]
struct PendingFile {
    name: String,
    file_type: String,
    data: std::sync::Arc<[u8]>,
}

#[derive(Clone, Debug, Deserialize)]
struct GroupMember {
    #[serde(default, alias = "userId", alias = "uin")]
    user_id: i64,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    card: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    shutup_time: i64,
}

impl GroupMember {
    fn display_name(&self) -> &str {
        if self.card.trim().is_empty() {
            &self.nickname
        } else {
            &self.card
        }
    }
}

struct BridgeViewState {
    key: String,
    socket_status: String,
    auth_status: String,
    online: OnlineData,
    rooms: Vec<Room>,
    chat_groups: ChatGroups,
    room_filter: RoomFilter,
    selected_room_id: Option<RoomId>,
    conversations: HashMap<RoomId, Conversation>,
    requests: Vec<JoinRequestRoom>,
    friends: Vec<FriendContact>,
    groups: Vec<GroupContact>,
    contacts_request_id: u64,
    contacts_loading: bool,
    search_results: Vec<Message>,
    search_keyword: String,
    search_offset: usize,
    search_has_more: bool,
    pending_forward: Option<(RoomId, JsonValue)>,
    group_members: HashMap<RoomId, Vec<GroupMember>>,
    forward_previews: HashMap<String, Vec<String>>,
    pending_forward_fetches: HashMap<u64, String>,
    next_forward_request_id: u64,
    last_notice: Option<String>,
    last_error: Option<String>,
    last_event: Option<String>,
    last_response: Option<String>,
    is_shut_up: bool,
}

impl BridgeViewState {
    fn new(key: String) -> Self {
        Self {
            key,
            socket_status: "连接中".to_string(),
            auth_status: "等待认证".to_string(),
            online: OnlineData::default(),
            rooms: Vec::new(),
            chat_groups: ChatGroups::default(),
            room_filter: RoomFilter::All,
            selected_room_id: None,
            conversations: HashMap::new(),
            requests: Vec::new(),
            friends: Vec::new(),
            groups: Vec::new(),
            contacts_request_id: 0,
            contacts_loading: false,
            search_results: Vec::new(),
            search_keyword: String::new(),
            search_offset: 0,
            search_has_more: false,
            pending_forward: None,
            group_members: HashMap::new(),
            forward_previews: HashMap::new(),
            pending_forward_fetches: HashMap::new(),
            next_forward_request_id: 0,
            last_notice: None,
            last_error: None,
            last_event: None,
            last_response: None,
            is_shut_up: false,
        }
    }

    fn conversation_mut(&mut self, room_id: RoomId) -> &mut Conversation {
        self.conversations.entry(room_id).or_default()
    }

    fn sort_rooms(&mut self) {
        self.rooms.sort_by(|left, right| {
            right
                .index
                .cmp(&left.index)
                .then(right.priority.cmp(&left.priority))
                .then(right.utime.cmp(&left.utime))
        });
    }
}

pub struct IcaApp {
    runtime: AppRuntime,
    config: ConfigStore,
    bridges: Vec<BridgeViewState>,
    active_bridge: Option<usize>,
    page: Page,
    composer: Entity<TextInput>,
    room_search: Entity<TextInput>,
    tool_event: Entity<TextInput>,
    tool_args: Entity<TextInput>,
    tool_gin: Entity<TextInput>,
    message_search: Entity<TextInput>,
    group_name: Entity<TextInput>,
    member_search: Entity<TextInput>,
    mute_duration: Entity<TextInput>,
    relation_search: Entity<TextInput>,
    message_scroll: ScrollHandle,
    room_scroll: ScrollHandle,
    auto_sign_running: bool,
    auto_sign_progress: (usize, usize),
    auto_sign_task: Option<gpui::Task<()>>,
    image_viewer_url: Option<String>,
    image_viewer_zoom: f32,
    show_stickers: bool,
    show_qq_faces: bool,
    face_page: usize,
    face_images: RefCell<HashMap<u16, std::sync::Arc<gpui::Image>>>,
    show_members: bool,
    show_mentions: bool,
    room_panel_width: f32,
    sticker_panel_width: f32,
    sticker_store: StickerStore,
    _event_task: gpui::Task<()>,
}

pub fn bind_keys(cx: &mut App) {
    use input::*;
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("ChatInput")),
        KeyBinding::new("delete", Delete, Some("ChatInput")),
        KeyBinding::new("left", Left, Some("ChatInput")),
        KeyBinding::new("right", Right, Some("ChatInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("ChatInput")),
        KeyBinding::new("shift-right", SelectRight, Some("ChatInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("ChatInput")),
        KeyBinding::new("cmd-v", Paste, Some("ChatInput")),
        KeyBinding::new("cmd-c", Copy, Some("ChatInput")),
        KeyBinding::new("cmd-x", Cut, Some("ChatInput")),
        KeyBinding::new("home", Home, Some("ChatInput")),
        KeyBinding::new("end", End, Some("ChatInput")),
        KeyBinding::new("enter", Submit, Some("ChatInput")),
        KeyBinding::new("shift-enter", Newline, Some("ChatInput")),
        KeyBinding::new("up", Up, Some("ChatInput")),
        KeyBinding::new("down", Down, Some("ChatInput")),
        KeyBinding::new("cmd-z", Undo, Some("ChatInput")),
        KeyBinding::new("cmd-shift-z", Redo, Some("ChatInput")),
        KeyBinding::new("cmd-y", Redo, Some("ChatInput")),
    ]);
}

pub fn apply_configured_theme(config: &IcaCfg, cx: &mut App) {
    let light = match config.ui_setting.theme_mode {
        ThemeMode::System => SystemAppearance::global(cx).0 == Appearance::Light,
        ThemeMode::Light => true,
        ThemeMode::Dark => false,
    };
    let name = if light { "One Light" } else { "One Dark" };
    if let Ok(theme) = ThemeRegistry::global(cx).get(name) {
        GlobalTheme::update_theme(cx, theme);
    }
}

impl IcaApp {
    pub fn new(
        mut runtime: AppRuntime,
        config: ConfigStore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let bridges = runtime
            .connections()
            .iter()
            .map(|connection| BridgeViewState::new(connection.key.clone()))
            .collect::<Vec<_>>();
        let active_bridge = (!bridges.is_empty()).then_some(0);
        let composer = cx.new(|cx| {
            TextInput::new("输入消息，Enter 发送", cx)
                .with_presentation(InputPresentation::Composer)
        });
        let room_search = cx
            .new(|cx| TextInput::new("搜索会话", cx).with_presentation(InputPresentation::Search));
        let tool_event = cx.new(|cx| TextInput::new("Socket.IO 事件名", cx));
        let tool_args = cx.new(|cx| TextInput::new("参数 JSON 数组，例如 []", cx));
        let tool_gin = cx.new(|cx| TextInput::new("文件管理 gin / 群号", cx));
        let message_search = cx.new(|cx| {
            TextInput::new("搜索聊天记录", cx).with_presentation(InputPresentation::Search)
        });
        let group_name = cx.new(|cx| TextInput::new("新分组名称", cx));
        let member_search = cx.new(|cx| {
            TextInput::new("搜索群成员", cx).with_presentation(InputPresentation::Search)
        });
        let mute_duration = cx.new(|cx| TextInput::new("禁言秒数，默认 600", cx));
        let relation_search = cx.new(|cx| {
            TextInput::new("搜索群名、群号或已加载成员", cx)
                .with_presentation(InputPresentation::Search)
        });
        let snapshot = config.snapshot();
        let sticker_store =
            StickerStore::resolve(&snapshot, config.paths()).unwrap_or_else(|error| {
                StickerStore::unavailable(config.paths().data_dir().join("stickers"), error)
            });
        if let Err(error) = sticker_store.refresh(snapshot.custom_chat.sort_stickers_by_time) {
            tracing::warn!(%error, "刷新收藏表情失败");
        }

        cx.subscribe(&composer, |this, input, event, cx| {
            match event {
                InputEvent::Submitted(text) => {
                    if this.send_text(text.clone()).is_ok() {
                        input.update(cx, |input, cx| input.clear(cx));
                    }
                }
                InputEvent::Changed => {
                    let text = input.read(cx).text().to_string();
                    let room_id = this.active().and_then(|bridge| bridge.selected_room_id);
                    if let Some(room_id) = room_id
                        && let Some(bridge) = this.active_mut()
                    {
                        bridge.conversation_mut(room_id).draft = text;
                    }
                }
                InputEvent::PastedImage { mime, data } => {
                    let room_id = this.active().and_then(|bridge| bridge.selected_room_id);
                    if let Some(room_id) = room_id
                        && let Some(bridge) = this.active_mut()
                    {
                        let conversation = bridge.conversation_mut(room_id);
                        conversation.pending_file = None;
                        conversation.pending_images.push(PendingImage {
                            name: format!(
                                "clipboard-{}.{}",
                                chrono::Local::now().format("%Y%m%d-%H%M%S"),
                                mime.rsplit('/').next().unwrap_or("png")
                            ),
                            mime: mime.clone(),
                            data: data.clone(),
                        });
                    }
                }
                InputEvent::PastedPaths(paths) => this.queue_attachments(paths.clone()),
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&room_search, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&tool_event, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&tool_args, |_, _, _, cx| cx.notify()).detach();
        cx.subscribe(&tool_gin, |_, _, _, cx| cx.notify()).detach();
        cx.subscribe(&message_search, |this, input, event, cx| {
            if let InputEvent::Submitted(keyword) = event {
                let keyword = keyword.clone();
                let room_id = this.active().and_then(|bridge| bridge.selected_room_id);
                if let Some(room_id) = room_id {
                    if let Some(bridge) = this.active_mut() {
                        bridge.search_keyword = keyword.clone();
                        bridge.search_results.clear();
                        bridge.search_offset = 0;
                        bridge.search_has_more = false;
                    }
                    let _ = this.send_command(IcaCommand::SearchMessages {
                        room_id,
                        keyword,
                        offset: 0,
                    });
                }
                input.update(cx, |_, _| {});
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&group_name, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&member_search, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&mute_duration, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&relation_search, |_, _, _, cx| cx.notify())
            .detach();

        let mut event_rx = runtime.take_event_receiver();
        let event_task = cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.apply_bridge_event(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        window.focus(&composer.focus_handle(cx), cx);
        Self {
            runtime,
            config,
            bridges,
            active_bridge,
            page: Page::Chat,
            composer,
            room_search,
            tool_event,
            tool_args,
            tool_gin,
            message_search,
            group_name,
            member_search,
            mute_duration,
            relation_search,
            message_scroll: ScrollHandle::new(),
            room_scroll: ScrollHandle::new(),
            auto_sign_running: false,
            auto_sign_progress: (0, 0),
            auto_sign_task: None,
            image_viewer_url: None,
            image_viewer_zoom: 1.0,
            show_stickers: false,
            show_qq_faces: true,
            face_page: 0,
            face_images: RefCell::new(HashMap::new()),
            show_members: false,
            show_mentions: false,
            room_panel_width: snapshot.ui_setting.room_panel_width.clamp(300.0, 700.0),
            sticker_panel_width: snapshot.ui_setting.sticker_panel_width.clamp(300.0, 500.0),
            sticker_store,
            _event_task: event_task,
        }
    }

    fn active(&self) -> Option<&BridgeViewState> {
        self.active_bridge.and_then(|index| self.bridges.get(index))
    }

    fn active_mut(&mut self) -> Option<&mut BridgeViewState> {
        self.active_bridge
            .and_then(|index| self.bridges.get_mut(index))
    }

    fn send_command(&mut self, command: IcaCommand) -> Result<(), String> {
        let index = self.active_bridge.ok_or("没有可用的 bridge")?;
        self.runtime.connections()[index].handle.send(command)
    }

    fn fetch_forward_preview(&mut self, res_id: String, file_name: Option<String>) {
        let request_id = if let Some(bridge) = self.active_mut() {
            bridge.next_forward_request_id = bridge.next_forward_request_id.wrapping_add(1).max(1);
            let request_id = bridge.next_forward_request_id;
            bridge
                .pending_forward_fetches
                .insert(request_id, res_id.clone());
            request_id
        } else {
            return;
        };
        if let Err(error) = self.send_command(IcaCommand::FetchForwardMessages {
            request_id,
            res_id,
            file_name,
            fallback_res_id: None,
        }) {
            self.set_error(format!("读取合并转发失败: {error}"));
        }
    }

    fn select_room(&mut self, room_id: RoomId, cx: &mut Context<Self>) {
        let pending_forward = self
            .active_mut()
            .and_then(|bridge| bridge.pending_forward.take());
        if let Some((origin, node)) = pending_forward {
            if let Err(error) = self.send_command(IcaCommand::SendMergedForward {
                nodes: vec![node],
                direct_message: room_id > 0,
                origin: Some(origin),
                target_room_id: room_id,
            }) {
                self.set_error(error);
            } else if let Some(bridge) = self.active_mut() {
                bridge.last_notice = Some("已请求转发消息".to_string());
            }
        }
        let should_fetch = self
            .active()
            .and_then(|bridge| bridge.conversations.get(&room_id))
            .is_none_or(|conversation| !conversation.requested);
        let settings = self.config.snapshot();
        let should_fetch_latest =
            !should_fetch && settings.ui_setting.auto_fetch_history_on_room_select;
        let last_message_id = self
            .active()
            .and_then(|bridge| bridge.conversations.get(&room_id))
            .and_then(|conversation| conversation.messages.last())
            .map(|message| message.msg_id.clone());
        if let Some(bridge) = self.active_mut() {
            bridge.selected_room_id = Some(room_id);
            if should_fetch {
                bridge.conversation_mut(room_id).requested = true;
            }
            if let Some(room) = bridge.rooms.iter_mut().find(|room| room.room_id == room_id) {
                room.unread_count = 0;
            }
        }
        if should_fetch && let Err(error) = self.send_command(IcaCommand::FetchMessages(room_id)) {
            self.set_error(error);
        }
        if should_fetch_latest {
            let current_loaded_messages = self
                .active()
                .and_then(|bridge| bridge.conversations.get(&room_id))
                .map_or(0, |conversation| conversation.messages.len());
            let _ = self.send_command(IcaCommand::FetchLatestHistory {
                room_id,
                current_loaded_messages,
            });
        }
        if settings.ui_setting.clear_search_on_room_select {
            self.room_search.update(cx, |input, cx| input.clear(cx));
        }
        if settings.custom_chat.auto_read_on_select
            && let Some(message_id) = last_message_id
        {
            let _ = self.send_command(IcaCommand::ReportRead {
                room_id,
                message_id,
            });
        }
        let draft = self
            .active()
            .and_then(|bridge| bridge.conversations.get(&room_id))
            .map(|conversation| conversation.draft.clone())
            .unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_text(draft, cx));
        self.message_scroll.scroll_to_bottom();
        if let Some(position) = self
            .active()
            .and_then(|bridge| bridge.rooms.iter().position(|room| room.room_id == room_id))
        {
            self.room_scroll.scroll_to_item(position);
        }
        cx.notify();
    }

    fn send_text(&mut self, text: String) -> Result<(), String> {
        let bridge = self.active().ok_or("没有可用的 bridge")?;
        let room_id = bridge.selected_room_id.ok_or("请先选择会话")?;
        let conversation = bridge.conversations.get(&room_id);
        let reply = conversation.and_then(|conversation| conversation.reply_to.clone());
        let pending_images = conversation
            .map(|conversation| conversation.pending_images.clone())
            .unwrap_or_default();
        let pending_file = conversation.and_then(|conversation| conversation.pending_file.clone());
        if text.trim().is_empty() && pending_images.is_empty() && pending_file.is_none() {
            return Err("消息内容为空".to_string());
        }
        let mut mentions = conversation
            .map(|conversation| conversation.mentions.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|mention| text.contains(&format!("@{}", mention.text)))
            .collect::<Vec<_>>();
        if text.contains("@全体成员") {
            mentions.push(Mention {
                user_id: 1,
                text: "全体成员".to_string(),
            });
        }
        let command = if let Some(file) = pending_file {
            IcaCommand::SendFileMessage {
                room_id,
                content: text,
                reply_to: reply,
                mentions,
                file_name: file.name,
                file_type: file.file_type,
                file_data: file.data,
            }
        } else if pending_images.len() == 1 {
            let image = pending_images.into_iter().next().unwrap();
            IcaCommand::SendImageMessage {
                room_id,
                content: text,
                reply_to: reply,
                mentions,
                image_type: image.mime,
                image_data: image.data,
            }
        } else if pending_images.len() > 1 {
            IcaCommand::SendMultiImageMessage {
                room_id,
                content: text,
                reply_to: reply,
                mentions,
                images: pending_images
                    .into_iter()
                    .map(|image| (image.mime, image.data))
                    .collect(),
            }
        } else {
            let mut message = SendMessage::new(text, room_id, reply);
            message.set_mentions(&mentions);
            IcaCommand::SendMessage(message)
        };
        self.send_command(command)?;
        if self
            .config
            .snapshot()
            .ui_setting
            .scroll_to_bottom_after_send
        {
            self.message_scroll.scroll_to_bottom();
        }
        if let Some(bridge) = self.active_mut() {
            let conversation = bridge.conversation_mut(room_id);
            conversation.reply_to = None;
            conversation.mentions.clear();
            conversation.pending_images.clear();
            conversation.pending_file = None;
            bridge.last_notice = Some("消息已提交".to_string());
        }
        Ok(())
    }

    fn set_error(&mut self, error: impl Into<String>) {
        if let Some(bridge) = self.active_mut() {
            bridge.last_error = Some(error.into());
        }
    }

    fn first(payload: &JsonValue) -> Option<&JsonValue> {
        payload.as_array().and_then(|values| values.first())
    }

    fn payload_message(payload: &JsonValue) -> Option<String> {
        payload
            .get("message")
            .and_then(JsonValue::as_str)
            .map(ToString::to_string)
            .or_else(|| Self::first(payload)?.as_str().map(ToString::to_string))
    }

    fn apply_bridge_event(&mut self, event: BridgeEvent, cx: &mut Context<Self>) {
        let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| bridge.key == event.bridge_key)
        else {
            return;
        };
        let mut composer_update = None;
        let mut scroll_to_bottom = false;
        let mut keep_visible_item = None;
        let mut keep_selected_room_visible = false;
        let bridge = &mut self.bridges[index];
        bridge.last_event = Some(event.kind.name().to_string());
        let payload = event.kind.payload();
        match &event.kind {
            BridgeEventKind::SocketConnecting(_) | BridgeEventKind::SocketReconnecting(_) => {
                bridge.socket_status = "连接中".to_string();
            }
            BridgeEventKind::SocketConnected(_) => bridge.socket_status = "已连接".to_string(),
            BridgeEventKind::SocketDisconnected(_) => bridge.socket_status = "已断开".to_string(),
            BridgeEventKind::SocketConnectFailed(_)
            | BridgeEventKind::SocketReconnectExhausted(_) => {
                bridge.socket_status = "连接失败".to_string();
                bridge.last_error = Self::payload_message(payload);
            }
            BridgeEventKind::RequireAuth(_) => bridge.auth_status = "认证中".to_string(),
            BridgeEventKind::AuthSucceed(_) => bridge.auth_status = "已认证".to_string(),
            BridgeEventKind::AuthFailed(_) => {
                bridge.auth_status = "认证失败".to_string();
                bridge.last_error = Some("bridge 认证失败".to_string());
            }
            BridgeEventKind::OnlineData(_) => {
                if let Some(value) = Self::first(payload) {
                    bridge.online = OnlineData::new_from_json(value);
                }
            }
            BridgeEventKind::SetAllRooms(_) => {
                if let Some(value) = Self::first(payload) {
                    match Vec::<Room>::deserialize(value) {
                        Ok(rooms) => {
                            bridge.rooms = rooms;
                            bridge.sort_rooms();
                            keep_selected_room_visible = true;
                        }
                        Err(error) => {
                            bridge.last_error = Some(format!("会话列表解析失败: {error}"))
                        }
                    }
                }
            }
            BridgeEventKind::SetAllChatGroups(_) => {
                if let Some(value) = Self::first(payload) {
                    match Vec::<ChatGroup>::deserialize(value) {
                        Ok(groups) => bridge.chat_groups = ChatGroups { groups },
                        Err(error) => {
                            bridge.last_error = Some(format!("聊天分组解析失败: {error}"))
                        }
                    }
                }
            }
            BridgeEventKind::UpdateRoom(_) => {
                if let Some(value) = Self::first(payload)
                    && let Ok(room) = Room::deserialize(value)
                {
                    if let Some(old) = bridge
                        .rooms
                        .iter_mut()
                        .find(|old| old.room_id == room.room_id)
                    {
                        *old = room;
                    } else {
                        bridge.rooms.push(room);
                    }
                    bridge.sort_rooms();
                    keep_selected_room_visible = true;
                }
            }
            BridgeEventKind::SetMessages(_) => {
                if let Some(value) = Self::first(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    match Vec::<Message>::deserialize(&value["messages"]) {
                        Ok(messages) => bridge.conversation_mut(room_id).messages = messages,
                        Err(error) => bridge.last_error = Some(format!("消息解析失败: {error}")),
                    }
                }
            }
            BridgeEventKind::AppendOlderMessages(_) => {
                if let Some(value) = Self::first(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    match Vec::<Message>::deserialize(&value["messages"]) {
                        Ok(mut messages) => {
                            let prepended = messages.len();
                            let conversation = bridge.conversation_mut(room_id);
                            conversation.no_more_history = messages.is_empty();
                            messages.append(&mut conversation.messages);
                            conversation.messages = messages;
                            if self.active_bridge == Some(index)
                                && bridge.selected_room_id == Some(room_id)
                                && prepended > 0
                            {
                                keep_visible_item = Some(prepended);
                            }
                        }
                        Err(error) => {
                            bridge.last_error = Some(format!("历史消息解析失败: {error}"))
                        }
                    }
                }
            }
            BridgeEventKind::AddMessage(_) => {
                if let Some(value) = Self::first(payload) {
                    match NewMessage::deserialize(value) {
                        Ok(new_message) => {
                            let room_id = new_message.room_id;
                            let is_selected = bridge.selected_room_id == Some(room_id);
                            let from_self = bridge.online.qqid > 0
                                && new_message.msg.sender_id == bridge.online.qqid;
                            let distance_from_bottom =
                                self.message_scroll.offset().y + self.message_scroll.max_offset().y;
                            if self.active_bridge == Some(index)
                                && bridge.selected_room_id == Some(room_id)
                                && distance_from_bottom.abs() <= px(80.)
                            {
                                scroll_to_bottom = true;
                            }
                            if let Some(room) =
                                bridge.rooms.iter_mut().find(|room| room.room_id == room_id)
                            {
                                room.last_message.content = Some(new_message.msg.content.clone());
                                room.last_message.username =
                                    Some(new_message.msg.sender_name.clone());
                                room.last_message.timestamp =
                                    Some(new_message.msg.time_text.clone());
                                room.utime = new_message.msg.time.timestamp_millis();
                                if !is_selected && !from_self {
                                    room.unread_count = room.unread_count.saturating_add(1);
                                }
                            }
                            let messages = &mut bridge.conversation_mut(room_id).messages;
                            if let Some(old) = messages
                                .iter_mut()
                                .find(|message| message.msg_id == new_message.msg.msg_id)
                            {
                                *old = new_message.msg;
                            } else {
                                messages.push(new_message.msg);
                            }
                            bridge.sort_rooms();
                            keep_selected_room_visible = true;
                        }
                        Err(error) => bridge.last_error = Some(format!("新消息解析失败: {error}")),
                    }
                }
            }
            BridgeEventKind::DeleteMessage(_) => {
                if let Some(id) = Self::first(payload).and_then(JsonValue::as_str) {
                    for conversation in bridge.conversations.values_mut() {
                        if let Some(message) = conversation
                            .messages
                            .iter_mut()
                            .find(|message| message.msg_id == id)
                        {
                            message.deleted = true;
                        }
                    }
                }
            }
            BridgeEventKind::HideMessage(_) | BridgeEventKind::RevealMessage(_) => {
                if let Some(id) = Self::first(payload).and_then(JsonValue::as_str) {
                    let hidden = matches!(&event.kind, BridgeEventKind::HideMessage(_));
                    for conversation in bridge.conversations.values_mut() {
                        if let Some(message) = conversation
                            .messages
                            .iter_mut()
                            .find(|message| message.msg_id == id)
                        {
                            message.hide = hidden;
                            message.reveal = !hidden;
                        }
                    }
                }
            }
            BridgeEventKind::RenewMessage(_) => {
                if let Some(value) = Self::first(payload) {
                    let room_id = value["roomId"].as_i64().unwrap_or_default();
                    let message_id = value["messageId"]
                        .as_str()
                        .map(ToString::to_string)
                        .or_else(|| value["messageId"].as_i64().map(|id| id.to_string()));
                    if let (Some(message_id), Some(update)) = (message_id, value.get("message"))
                        && let Some(message) = bridge
                            .conversation_mut(room_id)
                            .messages
                            .iter_mut()
                            .find(|message| message.msg_id == message_id)
                    {
                        if let Some(content) = update.get("content").and_then(JsonValue::as_str) {
                            message.content = content.to_string();
                        }
                        if let Some(files) = update.get("files")
                            && let Ok(files) = serde_json::from_value(files.clone())
                        {
                            message.files = files;
                        }
                    }
                }
            }
            BridgeEventKind::RenewMessageUrl(_) => {
                if let Some(value) = Self::first(payload) {
                    let message_id = value
                        .get("messageId")
                        .and_then(|value| {
                            value
                                .as_str()
                                .map(ToString::to_string)
                                .or_else(|| value.as_i64().map(|id| id.to_string()))
                        })
                        .unwrap_or_default();
                    if let Some(url) = value.get("URL").and_then(JsonValue::as_str) {
                        for conversation in bridge.conversations.values_mut() {
                            if let Some(message) = conversation
                                .messages
                                .iter_mut()
                                .find(|message| message.msg_id == message_id)
                            {
                                for file in &mut message.files {
                                    if file.file_type.eq_ignore_ascii_case("image")
                                        || file.file_type.starts_with("image/")
                                    {
                                        file.url = url.to_string();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
            BridgeEventKind::SyncRead(_) => {
                if let Some(room_id) = Self::first(payload).and_then(JsonValue::as_i64)
                    && let Some(room) = bridge.rooms.iter_mut().find(|room| room.room_id == room_id)
                {
                    room.unread_count = 0;
                }
            }
            BridgeEventKind::SetSystemMessages(_) => {
                if let Some(value) = Self::first(payload)
                    && let Ok(mut requests) = Vec::<JoinRequestRoom>::deserialize(value)
                {
                    requests.sort_by_key(|request| std::cmp::Reverse(request.time));
                    bridge.requests = requests;
                }
            }
            BridgeEventKind::HandleRequest(_) | BridgeEventKind::SendAddRequest(_) => {
                if let Some(value) = Self::first(payload)
                    && let Ok(request) = JoinRequestRoom::deserialize(value)
                {
                    bridge.requests.retain(|old| old.flag != request.flag);
                    bridge.requests.insert(0, request);
                }
            }
            BridgeEventKind::ContactsPartResponse(_) => {
                let part = payload
                    .get("part")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if let Some(items) = payload.get("items") {
                    match part {
                        "friends" => {
                            if let Ok(items) = Vec::<FriendContact>::deserialize(items) {
                                bridge.friends = items;
                            }
                        }
                        "groups" => {
                            if let Ok(items) = Vec::<GroupContact>::deserialize(items) {
                                bridge.groups = items;
                            }
                        }
                        _ => {}
                    }
                    bridge.contacts_loading = false;
                }
            }
            BridgeEventKind::ContactsPartFailed(_) => {
                bridge.contacts_loading = false;
                bridge.last_error =
                    Self::payload_message(payload).or_else(|| Some("联系人请求失败".to_string()));
            }
            BridgeEventKind::SearchMessagesResponse(_) => {
                let keyword = payload
                    .get("keyword")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if keyword == bridge.search_keyword {
                    match payload.get("messages").map(Vec::<Message>::deserialize) {
                        Some(Ok(messages)) => {
                            let offset = payload
                                .get("offset")
                                .and_then(JsonValue::as_u64)
                                .unwrap_or_default()
                                as usize;
                            let received = messages.len();
                            if offset == 0 {
                                bridge.search_results = messages;
                            } else {
                                for message in messages {
                                    if !bridge
                                        .search_results
                                        .iter()
                                        .any(|old| old.msg_id == message.msg_id)
                                    {
                                        bridge.search_results.push(message);
                                    }
                                }
                            }
                            bridge.search_offset = bridge.search_results.len();
                            bridge.search_has_more = payload
                                .get("hasMore")
                                .and_then(JsonValue::as_bool)
                                .unwrap_or(received > 0);
                        }
                        Some(Err(error)) => {
                            bridge.last_error = Some(format!("搜索结果解析失败: {error}"))
                        }
                        None => bridge.last_error = Some("搜索响应缺少 messages".to_string()),
                    }
                }
            }
            BridgeEventKind::ForwardMessagesResponse(_) => {
                let request_id = payload
                    .get("requestId")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or_default();
                let res_id = bridge
                    .pending_forward_fetches
                    .remove(&request_id)
                    .or_else(|| {
                        payload
                            .get("resId")
                            .and_then(JsonValue::as_str)
                            .map(ToString::to_string)
                    });
                if let Some(res_id) = res_id {
                    let mut lines = Vec::new();
                    collect_forward_lines(payload.get("messages").unwrap_or(payload), &mut lines);
                    if lines.is_empty() {
                        lines.push("合并转发内容为空或格式暂不支持".to_string());
                    }
                    bridge.forward_previews.insert(res_id, lines);
                }
            }
            BridgeEventKind::ForwardMessagesFailed(_) => {
                let request_id = payload
                    .get("requestId")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or_default();
                bridge.pending_forward_fetches.remove(&request_id);
                bridge.last_error =
                    Self::payload_message(payload).or_else(|| Some("读取合并转发失败".to_string()));
            }
            BridgeEventKind::GroupMembersResponse(_) => {
                let room_id = payload
                    .get("roomId")
                    .and_then(JsonValue::as_i64)
                    .unwrap_or_default();
                match payload.get("members").map(Vec::<GroupMember>::deserialize) {
                    Some(Ok(mut members)) => {
                        members
                            .sort_by(|left, right| left.display_name().cmp(right.display_name()));
                        bridge.group_members.insert(room_id, members);
                    }
                    Some(Err(error)) => {
                        bridge.last_error = Some(format!("群成员解析失败: {error}"))
                    }
                    None => bridge.last_error = Some("群成员响应缺少 members".to_string()),
                }
            }
            BridgeEventKind::SetOnline(_) => bridge.socket_status = "已连接".to_string(),
            BridgeEventKind::SetOffline(_) => {
                bridge.socket_status = "已断开".to_string();
                bridge.last_error = Self::payload_message(payload);
            }
            BridgeEventKind::SetShutUp(_) => {
                bridge.is_shut_up = Self::first(payload)
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
            }
            BridgeEventKind::AddMessageText(_) => {
                if let Some(text) = Self::first(payload).and_then(JsonValue::as_str)
                    && let Some(room_id) = bridge.selected_room_id
                {
                    let draft = &mut bridge.conversation_mut(room_id).draft;
                    draft.push_str(text);
                    if self.active_bridge == Some(index) {
                        composer_update = Some(draft.clone());
                    }
                }
            }
            BridgeEventKind::RequestSetup(_) => {
                bridge.last_error =
                    Some("bridge 尚未登录，需要先在 Icalingua++/bridge 完成登录".to_string());
            }
            BridgeEventKind::LoginVerify(_) => {
                bridge.last_error = Some("bridge 请求网页登录验证".to_string());
            }
            BridgeEventKind::LoginQrcode(_) => {
                bridge.last_error = Some("bridge 请求扫码登录，请查看 bridge 输出".to_string());
            }
            BridgeEventKind::LoginSmsCode(_) => {
                bridge.last_error = Some("bridge 请求短信验证码".to_string());
            }
            BridgeEventKind::LoginSlider(_) => {
                bridge.last_error = Some("bridge 请求滑块验证 ticket".to_string());
            }
            BridgeEventKind::LoginError(_) => {
                bridge.last_error =
                    Self::payload_message(payload).or_else(|| Some("bridge 登录失败".to_string()));
            }
            BridgeEventKind::NotifyError(_)
            | BridgeEventKind::MessageError(_)
            | BridgeEventKind::CommandFailed(_)
            | BridgeEventKind::Fatal(_) => {
                bridge.last_error =
                    Self::payload_message(payload).or_else(|| Some(payload.to_string()));
            }
            BridgeEventKind::SocketApiResponse(_) | BridgeEventKind::FileManagerResponse(_) => {
                let response = payload.to_string();
                bridge.last_response = Some(response.clone());
                bridge.last_notice = Some(response);
            }
            BridgeEventKind::NotifyMessage(_) | BridgeEventKind::MessageSuccess(_) => {
                bridge.last_notice =
                    Self::payload_message(payload).or_else(|| Some(payload.to_string()));
            }
            _ => {}
        }
        if let Some(draft) = composer_update {
            self.composer
                .update(cx, |input, cx| input.set_text(draft, cx));
        }
        if scroll_to_bottom {
            self.message_scroll.scroll_to_bottom();
        } else if let Some(item) = keep_visible_item {
            self.message_scroll.scroll_to_top_of_item(item);
        }
        if keep_selected_room_visible
            && self.active_bridge == Some(index)
            && let Some(room_id) = self.bridges[index].selected_room_id
            && let Some(position) = self.bridges[index]
                .rooms
                .iter()
                .position(|room| room.room_id == room_id)
        {
            self.room_scroll.scroll_to_item(position);
        }
    }

    fn switch_theme(&mut self, light: bool, cx: &mut Context<Self>) {
        let name = if light { "One Light" } else { "One Dark" };
        if let Ok(theme) = ThemeRegistry::global(cx).get(name) {
            GlobalTheme::update_theme(cx, theme);
            self.config.update(|config| {
                config.ui_setting.theme_mode = if light {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
            });
            if let Err(error) = self.config.save() {
                tracing::warn!(%error, "保存主题设置失败");
            }
            cx.refresh_windows();
        }
    }

    fn follow_system_theme(&mut self, cx: &mut Context<Self>) {
        let light = SystemAppearance::global(cx).0 == Appearance::Light;
        let name = if light { "One Light" } else { "One Dark" };
        if let Ok(theme) = ThemeRegistry::global(cx).get(name) {
            GlobalTheme::update_theme(cx, theme);
            self.config
                .update(|config| config.ui_setting.theme_mode = ThemeMode::System);
            if let Err(error) = self.config.save() {
                tracing::warn!(%error, "保存主题设置失败");
            }
            cx.refresh_windows();
        }
    }

    fn render_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = cx.theme().colors().clone();
        div()
            .id(id)
            .px_2()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if selected {
                colors.text_accent
            } else {
                colors.text_muted
            })
            .bg(if selected {
                colors.element_selected
            } else {
                colors.ghost_element_background
            })
            .hover(|style| style.bg(colors.ghost_element_hover))
            .child(label.into())
    }

    fn render_icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: IconName,
        selected: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = cx.theme().colors().clone();
        let tooltip = match icon {
            IconName::MagnifyingGlass => "搜索聊天记录",
            IconName::HistoryRerun => "加载更早记录",
            IconName::CloudDownload => "重新拉取最新漫游记录",
            IconName::Check => "群签到",
            IconName::Person => "群成员",
            IconName::Pin => "置顶或取消置顶",
            IconName::Trash => "删除会话",
            IconName::AtSign => "@全体成员",
            IconName::Image => "发送图片",
            IconName::Paperclip => "发送文件",
            IconName::Sparkle => "收藏表情",
            IconName::Send => "发送",
            IconName::Plus => "导入",
            IconName::Close => "关闭",
            _ => "操作",
        };
        div()
            .id(id)
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .bg(if selected {
                colors.element_selected
            } else {
                colors.ghost_element_background
            })
            .hover(|style| style.bg(colors.ghost_element_hover))
            .tooltip(Tooltip::text(tooltip))
            .child(Icon::new(icon).size(IconSize::Small).color(if selected {
                Color::Accent
            } else {
                Color::Muted
            }))
    }

    fn render_message_action(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: IconName,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = cx.theme().colors().clone();
        let tooltip = match icon {
            IconName::ReplyArrowRight => "回复",
            IconName::Pencil => "重新编辑",
            IconName::Copy => "复制",
            IconName::ForwardArrow => "转发",
            IconName::Trash => "撤回",
            IconName::Bell => "戳一戳",
            IconName::MicMute => "禁言 10 分钟",
            IconName::Close => "关闭",
            IconName::Eye => "显示消息",
            IconName::EyeOff => "隐藏消息",
            IconName::HistoryRerun => "重新获取消息内容",
            IconName::Hash => "复制消息 ID",
            IconName::Plus => "+1 原样发送",
            _ => "操作",
        };
        div()
            .id(id)
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .text_color(colors.text_muted)
            .hover(|style| style.bg(colors.ghost_element_hover))
            .tooltip(Tooltip::text(tooltip))
            .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
    }

    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let pages = [
            Page::Chat,
            Page::Groups,
            Page::Contacts,
            Page::Requests,
            Page::Relation,
            Page::Tools,
            Page::Settings,
        ];
        div()
            .flex_none()
            .flex()
            .items_center()
            .h(px(36.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.toolbar_background)
            .child(
                div()
                    .px_2()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Icalingua++ native"),
            )
            .children(pages.into_iter().enumerate().map(|(index, page)| {
                self.render_button(("top-page", index), page.label(), self.page == page, cx)
                    .h(px(28.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        if page == Page::Contacts {
                            this.refresh_contacts();
                        } else if page == Page::Requests {
                            let _ = this.send_command(IcaCommand::GetSystemMsg);
                        }
                        cx.notify();
                    }))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .px_2()
                    .text_size(px(11.))
                    .text_color(colors.text_muted)
                    .child(format!("v{}", crate::VERSION)),
            )
    }

    fn render_group_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let settings = self.config.snapshot().custom_chat;
        let current_filter = self
            .active()
            .map_or(RoomFilter::All, |bridge| bridge.room_filter);
        let current_filter = if current_filter == RoomFilter::Group {
            RoomFilter::All
        } else {
            current_filter
        };
        let (custom_groups, rooms) = self
            .active()
            .map(|bridge| (bridge.chat_groups.groups.clone(), bridge.rooms.clone()))
            .unwrap_or_default();
        let private_has_unread = rooms
            .iter()
            .any(|room| room.room_id > 0 && room.unread_count > 0);
        let mut entries = vec![
            (RoomFilter::All, "所有聊天".to_string(), false),
            (RoomFilter::Private, "私聊".to_string(), private_has_unread),
        ];
        entries.extend(custom_groups.iter().enumerate().map(|(index, group)| {
            let has_unread = rooms.iter().any(|room| {
                room.unread_count > 0
                    && (group.rooms.contains(&room.room_id)
                        || (group.include_all_personal && room.room_id > 0))
            });
            (RoomFilter::Custom(index), group.name.clone(), has_unread)
        }));

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(70.))
            .h_full()
            .items_center()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                div()
                    .id("chat-group-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .overflow_y_scroll()
                    .pt_2()
                    .children(entries.into_iter().enumerate().map(
                        |(index, (filter, label, has_unread))| {
                            let selected = current_filter == filter;
                            div()
                                .id(("chat-group-filter", index))
                                .relative()
                                .flex()
                                .flex_none()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .w(px(62.))
                                .h(px(58.))
                                .gap(px(2.))
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(if selected {
                                    colors.text
                                } else {
                                    colors.text_muted
                                })
                                .bg(if selected {
                                    colors.element_selected
                                } else {
                                    colors.ghost_element_background
                                })
                                .hover(|style| style.bg(colors.ghost_element_hover))
                                .child(Icon::new(IconName::Chat).size(IconSize::Small).color(
                                    if selected {
                                        Color::Accent
                                    } else {
                                        Color::Muted
                                    },
                                ))
                                .child(div().max_w(px(58.)).truncate().child(label))
                                .when(
                                    has_unread
                                        && !selected
                                        && !settings.disable_chat_group
                                        && !settings.disable_chat_group_dot,
                                    |element| {
                                        element.child(
                                            div()
                                                .absolute()
                                                .top(px(5.))
                                                .right(px(9.))
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(gpui::rgb(0xdc2626)),
                                        )
                                    },
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(bridge) = this.active_mut() {
                                        bridge.room_filter = filter;
                                    }
                                    cx.notify();
                                }))
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .py_2()
                    .w_full()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(
                        self.render_button("new-chat-group", "+", false, cx)
                            .w(px(28.))
                            .px_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = Page::Groups;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_icon_button(
                            "manage-chat-groups",
                            IconName::Settings,
                            false,
                            cx,
                        )
                        .size(px(28.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page = Page::Groups;
                            cx.notify();
                        })),
                    ),
            )
    }

    fn render_room_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let query = self.room_search.read(cx).text().trim().to_lowercase();
        let selected = self.active().and_then(|bridge| bridge.selected_room_id);
        let selected_bridge_name = self
            .active()
            .map(|bridge| bridge.key.clone())
            .unwrap_or_else(|| "没有启用的 bridge".to_string());
        let room_filter = self
            .active()
            .map_or(RoomFilter::All, |bridge| bridge.room_filter);
        let room_filter = if room_filter == RoomFilter::Group {
            RoomFilter::All
        } else {
            room_filter
        };
        let custom_groups = self
            .active()
            .map(|bridge| bridge.chat_groups.groups.clone())
            .unwrap_or_default();
        let rooms = self
            .active()
            .map(|bridge| {
                bridge
                    .rooms
                    .iter()
                    .filter(|room| {
                        let in_filter = match room_filter {
                            RoomFilter::All => true,
                            RoomFilter::Private => room.room_id > 0,
                            RoomFilter::Group => room.room_id < 0,
                            RoomFilter::Custom(index) => {
                                custom_groups.get(index).is_some_and(|group| {
                                    group.rooms.contains(&room.room_id)
                                        || (group.include_all_personal && room.room_id > 0)
                                })
                            }
                        };
                        in_filter
                            && (query.is_empty()
                                || room.room_name.to_lowercase().contains(&query)
                                || room.room_id.to_string().contains(&query))
                    })
                    .map(|room| {
                        (
                            room.room_id,
                            room.room_name.clone(),
                            room.avatar_url(),
                            room.last_message.content.clone().unwrap_or_default(),
                            room.last_message.username.clone().unwrap_or_default(),
                            room.last_message.user_id,
                            room.last_message.timestamp.clone().unwrap_or_default(),
                            room.unread_count,
                            room.at,
                            room.index > 0,
                            bridge
                                .conversations
                                .get(&room.room_id)
                                .map(|conversation| conversation.draft.trim().to_string())
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .w(px(self.room_panel_width))
            .min_w(px(300.))
            .max_w(px(700.))
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.surface_background)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child("Bridge"),
                    )
                    .child(
                        self.render_button(
                            "bridge-selector",
                            format!("{selected_bridge_name}  ▾"),
                            true,
                            cx,
                        )
                        .h(px(26.))
                        .max_w(px(210.))
                        .tooltip(Tooltip::text("点击切换到下一个 Bridge"))
                        .on_click(cx.listener(|this, _, _, cx| {
                                    if this.bridges.is_empty() {
                                        return;
                                    }
                                    let index = this
                                        .active_bridge
                                        .map_or(0, |index| (index + 1) % this.bridges.len());
                                    this.active_bridge = Some(index);
                                    this.room_search.update(cx, |input, cx| input.clear(cx));
                                    let draft = this
                                        .active()
                                        .and_then(|bridge| bridge.selected_room_id)
                                        .and_then(|room_id| {
                                            this.active()
                                                .and_then(|bridge| bridge.conversations.get(&room_id))
                                        })
                                        .map(|conversation| conversation.draft.clone())
                                        .unwrap_or_default();
                                    this.composer.update(cx, |input, cx| input.set_text(draft, cx));
                                    cx.notify();
                                })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(36.))
                    .px_2()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .gap_1()
                    .child(
                        div()
                            .mr_1()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("聊天列表"),
                    )
                    .child(
                        self.render_button("open-contacts-from-list", "联系人", false, cx)
                            .h(px(26.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = Page::Contacts;
                                this.refresh_contacts();
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("refresh-current-room", "刷新", false, cx)
                            .h(px(26.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(room_id) =
                                    this.active().and_then(|bridge| bridge.selected_room_id)
                                {
                                    let _ = this.send_command(IcaCommand::FetchMessages(room_id));
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("room-list-top", "顶部", false, cx)
                            .h(px(26.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.room_scroll.scroll_to_item(0);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("room-list-bottom", "底部", false, cx)
                            .h(px(26.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let last = this
                                    .active()
                                    .map_or(0, |bridge| bridge.rooms.len().saturating_sub(1));
                                this.room_scroll.scroll_to_item(last);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(44.))
                    .px_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(self.room_search.clone()),
            )
            .child(
                div()
                    .id("room-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.room_scroll)
                    .children(rooms.into_iter().map(
                        |(
                            room_id,
                            name,
                            avatar_url,
                            preview,
                            sender,
                            sender_id,
                            timestamp,
                            unread,
                            at,
                            pinned,
                            draft,
                        )| {
                            let is_selected = selected == Some(room_id);
                            let preview =
                                if room_id < 0 && !sender.is_empty() && !preview.is_empty() {
                                    format!("{sender}: {preview}")
                                } else {
                                    preview
                                };
                            let badge_color: gpui::Hsla = match at {
                                At::All => gpui::rgb(0xd97706).into(),
                                At::Bool(true) => gpui::rgb(0xdc2626).into(),
                                _ => colors.text_accent,
                            };
                            div()
                                .id(SharedString::from(format!("room-{room_id}")))
                                .flex()
                                .flex_none()
                                .items_center()
                                .h(px(62.))
                                .mx_1()
                                .px_1()
                                .py_1()
                                .rounded_sm()
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .cursor_pointer()
                                .bg(if is_selected {
                                    colors.element_selected
                                } else {
                                    colors.ghost_element_background
                                })
                                .hover(|style| style.bg(colors.ghost_element_hover))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_room(room_id, cx)
                                    }),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .flex_none()
                                        .size(px(40.))
                                        .mr_2()
                                        .child(Avatar::new(avatar_url).size(px(40.)))
                                        .when(room_id < 0 && sender_id.is_some(), |element| {
                                            let sender_id = sender_id.unwrap_or_default();
                                            element.child(
                                                div()
                                                    .absolute()
                                                    .right(px(-1.))
                                                    .bottom(px(-1.))
                                                    .size(px(20.))
                                                    .p(px(1.))
                                                    .rounded_md()
                                                    .bg(colors.surface_background)
                                                    .child(
                                                        Avatar::new(format!(
                                                            "https://q1.qlogo.cn/g?b=qq&nk={sender_id}&s=140"
                                                        ))
                                                        .size(px(18.)),
                                                    ),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .h(px(20.))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_size(px(16.))
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .child(name),
                                                )
                                                .when(pinned, |element| {
                                                    element.child(
                                                        div()
                                                            .ml_1()
                                                            .text_size(px(11.))
                                                            .text_color(colors.text_muted)
                                                            .child("↑"),
                                                    )
                                                })
                                                .when(!timestamp.is_empty(), |element| {
                                                    element.child(
                                                        div()
                                                            .ml_1()
                                                            .text_size(px(11.))
                                                            .text_color(colors.text_muted)
                                                            .child(timestamp),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .h(px(18.))
                                                .when(!draft.is_empty(), |element| {
                                                    element.child(
                                                        div()
                                                            .mr_1()
                                                            .px_1()
                                                            .h(px(16.))
                                                            .flex()
                                                            .items_center()
                                                            .rounded_full()
                                                            .bg(gpui::rgb(0x006cff))
                                                            .text_size(px(10.))
                                                            .text_color(gpui::rgb(0xffffff))
                                                            .child("草稿"),
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_size(px(12.))
                                                        .text_color(colors.text_muted)
                                                        .child(if !draft.is_empty() {
                                                            draft
                                                        } else if preview.is_empty() {
                                                            "暂无消息".to_string()
                                                        } else {
                                                            preview
                                                        }),
                                                )
                                                .when(unread > 0, |element| {
                                                    element.child(
                                                        div()
                                                            .ml_2()
                                                            .min_w(px(20.))
                                                            .h(px(18.))
                                                            .px_1()
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .rounded_full()
                                                            .bg(if matches!(at, At::None | At::Bool(false)) {
                                                                gpui::rgb(0x808080).into()
                                                            } else {
                                                                badge_color
                                                            })
                                                            .text_size(px(11.))
                                                            .text_color(gpui::rgb(0xffffff))
                                                            .child(unread.min(99).to_string()),
                                                    )
                                                }),
                                        ),
                                )
                        },
                    )),
            )
    }

    fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let custom_chat = self.config.snapshot().custom_chat;
        let hide_chat_images = custom_chat.hide_chat_img;
        let hide_avatars = custom_chat.hide_group_member_avatar;
        let high_contrast = custom_chat.high_contrast_chat;
        let selected_room = self.active().and_then(|bridge| {
            let id = bridge.selected_room_id?;
            bridge
                .rooms
                .iter()
                .find(|room| room.room_id == id)
                .map(|room| (id, room.room_name.clone(), room.index > 0))
        });
        let Some((room_id, room_name, room_is_pinned)) = selected_room else {
            return div()
                .flex()
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .text_color(colors.text_muted)
                .child("选择一个会话开始聊天");
        };
        let (source_messages, search_keyword, search_has_more) = self
            .active()
            .map(|bridge| {
                if bridge.search_keyword.is_empty() {
                    (
                        bridge
                            .conversations
                            .get(&room_id)
                            .map(|conversation| conversation.messages.as_slice())
                            .unwrap_or(&[]),
                        String::new(),
                        false,
                    )
                } else {
                    (
                        bridge.search_results.as_slice(),
                        bridge.search_keyword.clone(),
                        bridge.search_has_more,
                    )
                }
            })
            .unwrap_or((&[], String::new(), false));
        let self_id = self.active().map_or(-1, |bridge| bridge.online.qqid);
        let messages = source_messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let forward = forward_resource(&message.code);
                let forward_preview = forward.as_ref().and_then(|(res_id, _)| {
                    self.active()
                        .and_then(|bridge| bridge.forward_previews.get(res_id))
                        .cloned()
                });
                let raw = message.raw_msg.as_deref().cloned().unwrap_or_else(|| {
                    serde_json::json!({
                        "_id": message.msg_id,
                        "senderId": message.sender_id,
                        "username": message.sender_name,
                        "content": message.content,
                    })
                });
                (
                    message.msg_id.clone(),
                    message.sender_name.clone(),
                    message.sender_id,
                    message.role.clone(),
                    message.title.clone(),
                    message.content.clone(),
                    message.time_text.clone(),
                    message.deleted,
                    message.hide && !message.reveal,
                    message.files.clone(),
                    message.as_reply(),
                    message.reply.clone(),
                    message.content.clone(),
                    raw,
                    forward,
                    forward_preview,
                    message.system,
                    index == 0 || source_messages[index - 1].date_text != message.date_text,
                    message.date_text.clone(),
                    self_id > 0 && message.sender_id == self_id,
                )
            })
            .collect::<Vec<_>>();
        let reply = self
            .active()
            .and_then(|bridge| bridge.conversations.get(&room_id))
            .and_then(|conversation| conversation.reply_to.as_ref())
            .map(|reply| format!("回复 {}：{}", reply.sender_name, reply.content));
        let pending_attachments = self
            .active()
            .and_then(|bridge| bridge.conversations.get(&room_id))
            .map(|conversation| {
                if let Some(file) = &conversation.pending_file {
                    format!("待发送文件：{}", file.name)
                } else if !conversation.pending_images.is_empty() {
                    let names = conversation
                        .pending_images
                        .iter()
                        .take(3)
                        .map(|image| image.name.as_str())
                        .collect::<Vec<_>>()
                        .join("、");
                    format!(
                        "待发送图片 {} 张：{}{}",
                        conversation.pending_images.len(),
                        names,
                        if conversation.pending_images.len() > 3 {
                            "……"
                        } else {
                            ""
                        }
                    )
                } else {
                    String::new()
                }
            })
            .filter(|summary| !summary.is_empty());
        let mention_members = self
            .active()
            .and_then(|bridge| bridge.group_members.get(&room_id))
            .cloned()
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(64.))
                    .px_4()
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.toolbar_background)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(px(180.))
                            .min_w(px(120.))
                            .child(
                                div()
                                    .max_w(px(220.))
                                    .truncate()
                                    .text_size(px(17.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(room_name),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.text_muted)
                                    .child(if room_id < 0 {
                                        format!("群聊 · {}", room_id.abs())
                                    } else {
                                        format!("QQ {}", room_id)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .overflow_hidden()
                            .when(self.show_stickers || self.show_members, |element| {
                                element.hidden()
                            })
                            .child(div().flex_none().w(px(150.)).child(self.message_search.clone()))
                            .child(self.render_icon_button("search-messages", IconName::MagnifyingGlass, !search_keyword.is_empty(), cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let keyword = this.message_search.read(cx).text().trim().to_string();
                                    if keyword.is_empty() {
                                        if let Some(bridge) = this.active_mut() {
                                            bridge.search_keyword.clear();
                                            bridge.search_results.clear();
                                            bridge.search_offset = 0;
                                            bridge.search_has_more = false;
                                        }
                                    } else {
                                        if let Some(bridge) = this.active_mut() {
                                            bridge.search_keyword = keyword.clone();
                                            bridge.search_results.clear();
                                            bridge.search_offset = 0;
                                            bridge.search_has_more = false;
                                        }
                                        let _ = this.send_command(IcaCommand::SearchMessages { room_id, keyword, offset: 0 });
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_icon_button("older", IconName::HistoryRerun, false, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let offset = this
                                        .active()
                                        .and_then(|bridge| bridge.conversations.get(&room_id))
                                        .map_or(0, |conversation| conversation.messages.len());
                                    let _ = this.send_command(IcaCommand::FetchOlderMessages { room_id, offset });
                                }),
                            ))
                            .child(self.render_icon_button("latest", IconName::CloudDownload, false, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let current_loaded_messages = this
                                        .active()
                                        .and_then(|bridge| bridge.conversations.get(&room_id))
                                        .map_or(0, |conversation| conversation.messages.len());
                                    let _ = this.send_command(IcaCommand::FetchLatestHistory {
                                        room_id,
                                        current_loaded_messages,
                                    });
                                }),
                            ))
                            .child(self.render_icon_button("stop-history", IconName::Close, false, cx).on_click(
                                cx.listener(|this, _, _, _| {
                                    let _ = this.send_command(IcaCommand::StopFetchingHistory);
                                }),
                            ))
                            .when(room_id < 0, |element| element.child(self.render_icon_button("sign-group", IconName::Check, false, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let _ = this.send_command(IcaCommand::SendGroupSign { room_id });
                                }),
                            )))
                            .when(room_id < 0, |element| element.child(self.render_icon_button("members", IconName::Person, self.show_members, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.show_members = true;
                                    this.show_stickers = false;
                                    let _ = this.send_command(IcaCommand::FetchGroupMembers { room_id });
                                    cx.notify();
                                }),
                            )))
                            .child(self.render_icon_button("pin-room", IconName::Pin, room_is_pinned, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let pin = !room_is_pinned;
                                    if this.send_command(IcaCommand::PinRoom { room_id, pin }).is_ok() {
                                        if let Some(bridge) = this.active_mut() {
                                            if let Some(room) = bridge.rooms.iter_mut().find(|room| room.room_id == room_id) {
                                                room.index = if pin { 1 } else { 0 };
                                            }
                                            bridge.sort_rooms();
                                        }
                                        cx.notify();
                                    }
                                }),
                            ))
                            .child(self.render_icon_button("remove-room", IconName::Trash, false, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let _ = this.send_command(IcaCommand::RemoveChat(room_id));
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .id("message-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.message_scroll)
                    .p_4()
                    .gap_3()
                    .when(!search_keyword.is_empty(), |element| {
                        let keyword = search_keyword.clone();
                        element.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .text_color(colors.text_accent)
                                .child(format!("搜索结果：{search_keyword}"))
                                .when(search_has_more, |banner| {
                                    banner.child(
                                        self.render_button("more-search-results", "加载更多", false, cx)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let offset = this.active().map_or(0, |bridge| bridge.search_results.len());
                                                let _ = this.send_command(IcaCommand::SearchMessages {
                                                    room_id,
                                                    keyword: keyword.clone(),
                                                    offset,
                                                });
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                    })
                    .children(messages.into_iter().map(|(id, sender, sender_id, role, title, content, time, deleted, hidden, files, reply, quoted_reply, edit_content, raw, forward, forward_preview, system, show_date, date, is_self)| {
                        let files = if hidden { Vec::new() } else { files };
                        let file_count = files.len();
                        let display_content = if deleted {
                            "[消息已撤回]".to_string()
                        } else if hidden {
                            "[消息已隐藏]".to_string()
                        } else if content.is_empty() && file_count > 0 {
                            String::new()
                        } else if content.is_empty() {
                            "[空消息]".to_string()
                        } else {
                            content
                        };
                        if system {
                            return div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(show_date, |element| {
                                    element.child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .justify_center()
                                            .py_1()
                                            .text_size(px(11.))
                                            .text_color(colors.text_muted)
                                            .child(date),
                                    )
                                })
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .justify_center()
                                        .child(
                                            div()
                                                .max_w(relative(0.72))
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .bg(colors.element_background)
                                                .text_size(px(12.))
                                                .text_color(colors.text_muted)
                                                .child(display_content),
                                        ),
                                )
                                .into_any_element();
                        }
                        let has_display_content = !display_content.is_empty();
                        let role_label = match role.trim().to_ascii_lowercase().as_str() {
                            "owner" => "群主".to_string(),
                            "admin" | "administrator" => "管理员".to_string(),
                            "member" | "未知" | "unknown" => String::new(),
                            _ => role,
                        };
                        let copy_content = edit_content.clone();
                        let reedit_content = edit_content.clone();
                        let attachment_message_id = id.clone();
                        let avatar_url = format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", sender_id.abs());
                        let message_id = id.clone();
                        let reply_id = id.clone();
                        let edit_id = id.clone();
                        let copy_id = id.clone();
                        let forward_id = id.clone();
                        let delete_id = id.clone();
                        let plus_id = id.clone();
                        let plus_raw = raw.clone();
                        let plus_content = edit_content.clone();
                        let plus_reply = quoted_reply.clone();
                        let renew_id = id.clone();
                        let visibility_id = id.clone();
                        let hash_id = id.clone();
                        let actions = div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(self.render_message_action(SharedString::from(format!("reply-{reply_id}")), IconName::ReplyArrowRight, cx).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    if let Some(bridge) = this.active_mut() {
                                        bridge.conversation_mut(room_id).reply_to = Some(reply.clone());
                                    }
                                    window.focus(&this.composer.focus_handle(cx), cx);
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("edit-{edit_id}")), IconName::Pencil, cx).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.restore_message_to_draft(
                                        room_id,
                                        reedit_content.clone(),
                                        window,
                                        cx,
                                    );
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("copy-{copy_id}")), IconName::Copy, cx).on_click(
                                cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_content.clone()))),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("plus-{plus_id}")), IconName::Plus, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let command = if plus_raw.is_array() || plus_raw.get("type").is_some() {
                                        IcaCommand::SendRawMessage { room_id, content: plus_raw.clone() }
                                    } else {
                                        IcaCommand::SendMessage(SendMessage::new(
                                            plus_content.clone(),
                                            room_id,
                                            plus_reply.clone(),
                                        ))
                                    };
                                    let _ = this.send_command(command);
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("forward-{forward_id}")), IconName::ForwardArrow, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(bridge) = this.active_mut() {
                                        bridge.pending_forward = Some((room_id, raw.clone()));
                                        bridge.last_notice = Some("请选择目标会话完成转发".to_string());
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("renew-{renew_id}")), IconName::HistoryRerun, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let _ = this.send_command(IcaCommand::RenewMessage {
                                        room_id,
                                        message_id: renew_id.clone(),
                                    });
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("visibility-{visibility_id}")), if hidden { IconName::Eye } else { IconName::EyeOff }, cx).on_click(
                                cx.listener(move |this, _, _, _| {
                                    let command = if hidden {
                                        IcaCommand::RevealMessage {
                                            room_id,
                                            message_id: visibility_id.clone(),
                                        }
                                    } else {
                                        IcaCommand::HideMessage {
                                            room_id,
                                            message_id: visibility_id.clone(),
                                        }
                                    };
                                    let _ = this.send_command(command);
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("copy-id-{hash_id}")), IconName::Hash, cx).on_click(
                                cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(hash_id.clone()))),
                            ))
                            .when(!is_self && room_id < 0 && sender_id > 0, |element| {
                                element.child(self.render_message_action(SharedString::from(format!("poke-{id}")), IconName::Bell, cx).on_click(
                                    cx.listener(move |this, _, _, _| {
                                        let _ = this.send_command(IcaCommand::SendGroupPoke {
                                            room_id,
                                            target_id: sender_id,
                                        });
                                    }),
                                ))
                            })
                            .when(is_self && !deleted, |element| {
                                element.child(self.render_message_action(SharedString::from(format!("delete-{delete_id}")), IconName::Trash, cx).on_click(
                                    cx.listener(move |this, _, _, _| {
                                        let _ = this.send_command(IcaCommand::DeleteMessage(DeleteMessage {
                                            room_id,
                                            message_id: delete_id.clone(),
                                        }));
                                    }),
                                ))
                            });
                        let bubble = div()
                            .flex()
                            .flex_col()
                            .min_w(px(120.))
                            .max_w(relative(0.78))
                            .gap(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .when(is_self, |element| element.justify_end())
                                    .when(has_display_content, |element| element.child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(if deleted { colors.text_muted } else { colors.text_accent })
                                            .child(if sender_id > 0 { format!("{sender} · {sender_id}") } else { sender }),
                                    ))
                                    .when(!role_label.is_empty(), |element| {
                                        element.child(
                                            div()
                                                .px_1()
                                                .rounded_sm()
                                                .bg(colors.element_background)
                                                .text_size(px(10.))
                                                .text_color(colors.text_muted)
                                                .child(role_label),
                                        )
                                    })
                                    .when(!title.trim().is_empty(), |element| {
                                        element.child(
                                            div()
                                                .max_w(px(120.))
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(colors.text_accent)
                                                .child(title),
                                        )
                                    })
                                    .child(div().text_size(px(10.)).text_color(colors.text_muted).child(time))
                                    .child(actions),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(if high_contrast {
                                        colors.border_focused
                                    } else if is_self {
                                        colors.border_selected
                                    } else {
                                        colors.border_variant
                                    })
                                    .bg(if is_self { colors.element_selected } else { colors.elevated_surface_background })
                                    .when_some(quoted_reply, |element, quoted| {
                                        element.child(
                                            div()
                                                .mb_1()
                                                .pl_2()
                                                .border_l_2()
                                                .border_color(colors.border_selected)
                                                .text_size(px(12.))
                                                .text_color(colors.text_muted)
                                                .child(format!("{}：{}", quoted.sender_name, quoted.content)),
                                        )
                                    })
                                    .child(
                                        div()
                                            .whitespace_normal()
                                            .line_height(px(21.))
                                            .text_size(px(14.))
                                            .text_color(if deleted { colors.text_muted } else { colors.text })
                                            .child(display_content),
                                    )
                                    .when_some(forward, |element, (res_id, file_name)| {
                                        let request_res_id = res_id.clone();
                                        element.child(
                                            div()
                                                .mt_1()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .p_2()
                                                .rounded_md()
                                                .bg(colors.element_background)
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .child(file_name.clone().unwrap_or_else(|| "合并转发".to_string())),
                                                )
                                                .when_some(forward_preview.clone(), |preview, lines| {
                                                    preview.children(lines.into_iter().take(8).map(|line| {
                                                        div().truncate().text_size(px(12.)).text_color(colors.text_muted).child(line)
                                                    }))
                                                })
                                                .child(
                                                    self.render_button(
                                                        SharedString::from(format!("forward-preview-{res_id}")),
                                                        if forward_preview.is_some() { "刷新内容" } else { "查看合并转发" },
                                                        false,
                                                        cx,
                                                    )
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.fetch_forward_preview(request_res_id.clone(), file_name.clone());
                                                        cx.notify();
                                                    })),
                                                ),
                                        )
                                    })
                                    .when(file_count > 0, |element| element.child(
                                        div().flex().flex_col().gap_1().children(files.into_iter().enumerate().map(|(file_index, file)| {
                                            let url = file.url.clone();
                                            let image_url = url.clone();
                                            let label = file.name.unwrap_or_else(|| {
                                                if file.file_type.starts_with("image") { "查看图片".to_string() } else { "打开附件".to_string() }
                                            });
                                             if file.file_type.starts_with("image") && !image_url.is_empty() && !hide_chat_images {
                                                 let viewer_url = image_url.clone();
                                                 img(image_url.clone())
                                                    .id(SharedString::from(format!("attachment-{attachment_message_id}-{file_index}")))
                                                    .w(px(240.))
                                                    .h(px(180.))
                                                    .object_fit(ObjectFit::Contain)
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.image_viewer_url = Some(viewer_url.clone());
                                                        cx.notify();
                                                    }))
                                                    .into_any_element()
                                            } else {
                                                self.render_button(SharedString::from(format!("attachment-{attachment_message_id}-{file_index}")), label, false, cx).on_click(
                                                    cx.listener(move |_, _, _, cx| {
                                                        if !url.is_empty() { cx.open_url(&url); }
                                                    }),
                                                ).into_any_element()
                                            }
                                        }))
                                    )));
                        let leading_avatar_url = avatar_url.clone();
                        let row = div()
                            .id(SharedString::from(format!("message-{message_id}")))
                            .w_full()
                            .flex()
                            .items_end()
                            .gap_2()
                            .when(is_self, |element| element.justify_end())
                            .when(!is_self && !hide_avatars, |element| {
                                element.child(Avatar::new(leading_avatar_url).size(px(36.)))
                            })
                            .child(bubble)
                            .when(is_self && !hide_avatars, |element| {
                                element.child(Avatar::new(avatar_url).size(px(36.)))
                            });
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(show_date, |element| {
                                element.child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .justify_center()
                                        .py_1()
                                        .text_size(px(11.))
                                        .text_color(colors.text_muted)
                                        .child(date),
                                )
                            })
                            .child(row)
                            .into_any_element()
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(colors.border)
                    .bg(colors.panel_background)
                    .when_some(reply, |element, reply| {
                        element.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .h(px(28.))
                                .px_2()
                                .border_l_2()
                                .border_color(colors.border_selected)
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child(reply)
                                .child(self.render_message_action("cancel-reply", IconName::Close, cx).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(bridge) = this.active_mut() {
                                            bridge.conversation_mut(room_id).reply_to = None;
                                        }
                                        cx.notify();
                                    }),
                                )),
                        )
                    })
                    .when_some(pending_attachments, |element, summary| {
                        element.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .h(px(30.))
                                .px_2()
                                .rounded_md()
                                .bg(colors.element_background)
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child(summary)
                                .child(
                                    self.render_message_action("clear-attachments", IconName::Close, cx)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(bridge) = this.active_mut() {
                                                let conversation = bridge.conversation_mut(room_id);
                                                conversation.pending_images.clear();
                                                conversation.pending_file = None;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .when(self.show_mentions && room_id < 0, |element| {
                        element.child(
                            div()
                                .id("mention-picker")
                                .flex()
                                .items_center()
                                .gap_1()
                                .h(px(38.))
                                .overflow_x_scroll()
                                .child(
                                    self.render_button("mention-everyone", "@全体成员", true, cx)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.composer.update(cx, |input, cx| {
                                                input.insert_text("@全体成员 ", window, cx)
                                            });
                                            this.show_mentions = false;
                                            window.focus(&this.composer.focus_handle(cx), cx);
                                            cx.notify();
                                        })),
                                )
                                .children(mention_members.into_iter().take(80).enumerate().map(|(index, member)| {
                                    let name = member.display_name().to_string();
                                    let mention_name = name.clone();
                                    let user_id = member.user_id;
                                    self.render_button(("mention-member", index), name, false, cx)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if let Some(bridge) = this.active_mut() {
                                                let conversation = bridge.conversation_mut(room_id);
                                                if !conversation.mentions.iter().any(|mention| mention.user_id == user_id) {
                                                    conversation.mentions.push(Mention {
                                                        user_id,
                                                        text: mention_name.clone(),
                                                    });
                                                }
                                            }
                                            this.composer.update(cx, |input, cx| {
                                                input.insert_text(&format!("@{} ", mention_name), window, cx)
                                            });
                                            this.show_mentions = false;
                                            window.focus(&this.composer.focus_handle(cx), cx);
                                            cx.notify();
                                        }))
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(room_id < 0, |element| element.child(self.render_icon_button("mention-all", IconName::AtSign, false, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.show_mentions = !this.show_mentions;
                                    if this.show_mentions {
                                        let loaded = this.active().and_then(|bridge| bridge.group_members.get(&room_id)).is_some();
                                        if !loaded {
                                            let _ = this.send_command(IcaCommand::FetchGroupMembers { room_id });
                                        }
                                    }
                                    cx.notify();
                                }),
                            )))
                            .child(self.render_icon_button("image", IconName::Image, false, cx).on_click(cx.listener(Self::pick_image)))
                            .child(self.render_icon_button("file", IconName::Paperclip, false, cx).on_click(cx.listener(Self::pick_file)))
                            .child(self.render_icon_button("sticker", IconName::Sparkle, self.show_stickers, cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.show_stickers = !this.show_stickers;
                                    if this.show_stickers { this.show_members = false; }
                                    cx.notify();
                                }),
                            ))
                            .child(div().flex_1().child(self.composer.clone()))
                            .child(self.render_icon_button("send", IconName::Send, true, cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let text = this.composer.read(cx).text().trim().to_string();
                                    if this.send_text(text).is_ok() {
                                        this.composer.update(cx, |input, cx| input.clear(cx));
                                    }
                                    cx.notify();
                                }),
                            )),
                    ),
            )
    }

    fn refresh_contacts(&mut self) {
        if let Some(bridge) = self.active_mut() {
            bridge.contacts_request_id = bridge.contacts_request_id.wrapping_add(1).max(1);
            bridge.contacts_loading = true;
            let request_id = bridge.contacts_request_id;
            let _ = self.send_command(IcaCommand::FetchContacts { request_id });
        }
    }

    fn open_contact(&mut self, room_id: RoomId, room_name: String, cx: &mut Context<Self>) {
        let missing = self
            .active()
            .is_some_and(|bridge| !bridge.rooms.iter().any(|room| room.room_id == room_id));
        if missing {
            let room = Room {
                room_id,
                room_name,
                index: 0,
                unread_count: 0,
                priority: 3,
                utime: chrono::Utc::now().timestamp_millis(),
                users: JsonValue::Null,
                at: At::None,
                last_message: LastMessage {
                    content: None,
                    timestamp: None,
                    username: None,
                    user_id: None,
                },
            };
            if self.send_command(IcaCommand::AddRoom(room.clone())).is_ok()
                && let Some(bridge) = self.active_mut()
            {
                bridge.rooms.push(room);
                bridge.sort_rooms();
            }
        }
        self.page = Page::Chat;
        self.select_room(room_id, cx);
    }

    fn create_chat_group(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.set_error("分组名称不能为空");
            return;
        }
        if self.active().is_some_and(|bridge| {
            bridge
                .chat_groups
                .groups
                .iter()
                .any(|group| group.name == name)
        }) {
            self.set_error("已经存在同名分组");
            return;
        }
        if self
            .send_command(IcaCommand::AddChatGroup {
                name: name.clone(),
                rooms: Vec::new(),
                include_all_personal: false,
            })
            .is_ok()
            && let Some(bridge) = self.active_mut()
        {
            bridge.chat_groups.groups.push(ChatGroup::new_empty(name));
        }
    }

    fn remove_chat_group(&mut self, index: usize) {
        let Some(name) = self
            .active()
            .and_then(|bridge| bridge.chat_groups.groups.get(index))
            .map(|group| group.name.clone())
        else {
            return;
        };
        if self
            .send_command(IcaCommand::RemoveChatGroup { name })
            .is_ok()
            && let Some(bridge) = self.active_mut()
        {
            bridge.chat_groups.remove_group(index);
            if matches!(bridge.room_filter, RoomFilter::Custom(selected) if selected == index) {
                bridge.room_filter = RoomFilter::All;
            }
        }
    }

    fn rename_chat_group(&mut self, index: usize, new_name: String) {
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() {
            self.set_error("请先在上方输入新的分组名称");
            return;
        }
        let Some(old) = self
            .active()
            .and_then(|bridge| bridge.chat_groups.groups.get(index))
            .cloned()
        else {
            return;
        };
        if old.name == new_name {
            return;
        }
        if self.active().is_some_and(|bridge| {
            bridge
                .chat_groups
                .groups
                .iter()
                .any(|group| group.name == new_name)
        }) {
            self.set_error("已经存在同名分组");
            return;
        }
        let _ = self.send_command(IcaCommand::RemoveChatGroup { name: old.name });
        if self
            .send_command(IcaCommand::AddChatGroup {
                name: new_name.clone(),
                rooms: old.rooms,
                include_all_personal: old.include_all_personal,
            })
            .is_ok()
            && let Some(bridge) = self.active_mut()
            && let Some(group) = bridge.chat_groups.groups.get_mut(index)
        {
            group.name = new_name;
        }
    }

    fn move_chat_group(&mut self, index: usize, up: bool) {
        let Some(bridge) = self.active_mut() else {
            return;
        };
        let target = if up {
            index.checked_sub(1)
        } else if index + 1 < bridge.chat_groups.groups.len() {
            Some(index + 1)
        } else {
            None
        };
        if let Some(target) = target {
            bridge.chat_groups.move_group(index, target);
            bridge.room_filter = RoomFilter::Custom(target);
        }
    }

    fn update_chat_group(&mut self, index: usize, toggle_room: bool, toggle_personal: bool) {
        let selected_room = self.active().and_then(|bridge| bridge.selected_room_id);
        let Some(bridge) = self.active_mut() else {
            return;
        };
        let Some(group) = bridge.chat_groups.groups.get_mut(index) else {
            return;
        };
        if toggle_room {
            let Some(room_id) = selected_room else {
                bridge.last_error = Some("当前没有选中会话".to_string());
                return;
            };
            if let Some(position) = group.rooms.iter().position(|id| *id == room_id) {
                group.rooms.remove(position);
            } else {
                group.rooms.push(room_id);
            }
        }
        if toggle_personal {
            group.include_all_personal = !group.include_all_personal;
        }
        let command = IcaCommand::UpdateChatGroup {
            name: group.name.clone(),
            rooms: group.rooms.clone(),
            include_all_personal: group.include_all_personal,
        };
        let _ = self.send_command(command);
    }

    fn sign_all_groups(&mut self, cx: &mut Context<Self>) {
        if self.auto_sign_running {
            self.auto_sign_running = false;
            self.auto_sign_task.take();
            if let Some(bridge) = self.active_mut() {
                bridge.last_notice = Some("已停止全群签到".to_string());
            }
            return;
        }
        let room_ids = self
            .active()
            .map(|bridge| {
                bridge
                    .rooms
                    .iter()
                    .filter(|room| room.room_id < 0)
                    .map(|room| room.room_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if room_ids.is_empty() {
            self.set_error("当前没有可签到的群聊");
            return;
        }
        let Some(index) = self.active_bridge else {
            return;
        };
        let handle = self.runtime.connections()[index].handle.clone();
        self.auto_sign_running = true;
        self.auto_sign_progress = (0, room_ids.len());
        self.auto_sign_task = Some(cx.spawn(async move |this, cx| {
            let mut failed = 0;
            let total = room_ids.len();
            for (position, room_id) in room_ids.into_iter().enumerate() {
                let should_continue = this
                    .update(cx, |this, _| this.auto_sign_running)
                    .unwrap_or(false);
                if !should_continue {
                    return;
                }
                if handle.send(IcaCommand::SendGroupSign { room_id }).is_err() {
                    failed += 1;
                }
                if this
                    .update(cx, |this, cx| {
                        this.auto_sign_progress = (position + 1, total);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                if position + 1 < total {
                    let delay = rand::rng().random_range(2_000_u64..=3_000);
                    cx.background_executor()
                        .timer(Duration::from_millis(delay))
                        .await;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.auto_sign_running = false;
                if let Some(bridge) = this.active_mut() {
                    bridge.last_notice = Some(format!(
                        "全群签到完成：成功 {}，失败 {}",
                        total - failed,
                        failed
                    ));
                }
                cx.notify();
            });
        }));
    }

    fn adjust_panel_width(&mut self, room_panel: bool, delta: f32) {
        if room_panel {
            self.room_panel_width = (self.room_panel_width + delta).clamp(300.0, 700.0);
        } else {
            self.sticker_panel_width = (self.sticker_panel_width + delta).clamp(300.0, 500.0);
        }
        let room_width = self.room_panel_width;
        let sticker_width = self.sticker_panel_width;
        self.config.update(|config| {
            config.ui_setting.room_panel_width = room_width;
            config.ui_setting.sticker_panel_width = sticker_width;
        });
        if let Err(error) = self.config.save() {
            tracing::warn!(%error, "保存面板宽度失败");
        }
    }

    fn toggle_setting(&mut self, key: &'static str) {
        self.config.update(|config| match key {
            "clear_search" => {
                config.ui_setting.clear_search_on_room_select =
                    !config.ui_setting.clear_search_on_room_select
            }
            "auto_history" => {
                config.ui_setting.auto_fetch_history_on_room_select =
                    !config.ui_setting.auto_fetch_history_on_room_select
            }
            "scroll_send" => {
                config.ui_setting.scroll_to_bottom_after_send =
                    !config.ui_setting.scroll_to_bottom_after_send
            }
            "auto_read" => {
                config.custom_chat.auto_read_on_select = !config.custom_chat.auto_read_on_select
            }
            "hide_images" => config.custom_chat.hide_chat_img = !config.custom_chat.hide_chat_img,
            "hide_avatars" => {
                config.custom_chat.hide_group_member_avatar =
                    !config.custom_chat.hide_group_member_avatar
            }
            "high_contrast" => {
                config.custom_chat.high_contrast_chat = !config.custom_chat.high_contrast_chat
            }
            "disable_groups" => {
                config.custom_chat.disable_chat_group = !config.custom_chat.disable_chat_group
            }
            "disable_group_dots" => {
                config.custom_chat.disable_chat_group_dot =
                    !config.custom_chat.disable_chat_group_dot
            }
            "highlight_urls" => {
                config.custom_chat.disable_highlight_url = !config.custom_chat.disable_highlight_url
            }
            "sort_stickers" => {
                config.custom_chat.sort_stickers_by_time = !config.custom_chat.sort_stickers_by_time
            }
            _ => {}
        });
        if let Err(error) = self.config.save() {
            self.set_error(format!("保存设置失败: {error}"));
        }
    }

    fn set_reedit_mode(&mut self, mode: ReEditDraftConflictMode) {
        self.config
            .update(|config| config.ui_setting.reedit_draft_conflict_mode = mode);
        if let Err(error) = self.config.save() {
            self.set_error(format!("保存设置失败: {error}"));
        }
    }

    fn restore_message_to_draft(
        &mut self,
        room_id: RoomId,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.composer.read(cx).text().to_string();
        let restored = match self.config.snapshot().ui_setting.reedit_draft_conflict_mode {
            ReEditDraftConflictMode::Overwrite => content,
            ReEditDraftConflictMode::Append => {
                if current.trim().is_empty() {
                    content
                } else if content.trim().is_empty() {
                    current
                } else {
                    format!("{current}\n{content}")
                }
            }
            ReEditDraftConflictMode::SkipIfNonEmpty => {
                if current.trim().is_empty() {
                    content
                } else {
                    current
                }
            }
        };
        self.composer
            .update(cx, |input, cx| input.set_text(restored.clone(), cx));
        if let Some(bridge) = self.active_mut() {
            bridge.conversation_mut(room_id).draft = restored;
        }
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    fn send_sticker(&mut self, entry: &StickerEntry) {
        let Some(room_id) = self.active().and_then(|bridge| bridge.selected_room_id) else {
            self.set_error("请先选择会话");
            return;
        };
        match self.sticker_store.read_entry(entry) {
            Ok(bytes) => {
                let reply_to = self
                    .active()
                    .and_then(|bridge| bridge.conversations.get(&room_id))
                    .and_then(|conversation| conversation.reply_to.clone());
                let mut message = SendMessage::new(String::new(), room_id, reply_to);
                message.set_img(&bytes, &entry.mime_type, true);
                if let Err(error) = self.send_command(IcaCommand::SendMessage(message)) {
                    self.set_error(error);
                } else {
                    self.show_stickers = false;
                }
            }
            Err(error) => self.set_error(format!("读取收藏表情失败: {error}")),
        }
    }

    fn import_sticker(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| self.sticker_store.add_bytes(&bytes).map(|_| ()))
            .and_then(|()| self.sticker_store.refresh(true).map(|_| ()))
        {
            Ok(()) => {
                if let Some(bridge) = self.active_mut() {
                    bridge.last_notice = Some("收藏表情已导入".to_string());
                }
            }
            Err(error) => self.set_error(format!("导入收藏表情失败: {error}")),
        }
    }

    fn render_sticker_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let entries = self.sticker_store.entries();
        let fallback_notice = self.sticker_store.fallback_notice();
        const FACES_PER_PAGE: usize = 80;
        let face_count = crate::face_data::FACE_COUNT;
        let face_pages = face_count.div_ceil(FACES_PER_PAGE);
        let face_entries = if self.show_qq_faces {
            let mut face_images = self.face_images.borrow_mut();
            crate::face_data::all_face_ids()
                .skip(self.face_page * FACES_PER_PAGE)
                .take(FACES_PER_PAGE)
                .filter_map(|face_id| {
                    let image = if let Some(image) = face_images.get(&face_id) {
                        image.clone()
                    } else {
                        let bytes = crate::face_data::get_face(face_id)?;
                        // GPUI 会持续驱动 APNG 动画重绘；同屏几十个 QQ 动态表情会让
                        // Windows 图像后端不稳定。选择器只需要缩略图，因此固定取首帧。
                        let decoded = image::load_from_memory(&bytes).ok()?;
                        let mut preview = std::io::Cursor::new(Vec::new());
                        decoded
                            .write_to(&mut preview, image::ImageFormat::Png)
                            .ok()?;
                        let image = std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            preview.into_inner(),
                        ));
                        face_images.insert(face_id, image.clone());
                        image
                    };
                    Some((face_id, image, crate::face_data::get_face_name(face_id)))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        div()
            .flex()
            .flex_col()
            .w(px(self.sticker_panel_width))
            .min_w(px(300.))
            .max_w(px(500.))
            .h_full()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(64.))
                    .px_4()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(if self.show_qq_faces {
                        format!("QQ 表情 ({face_count})")
                    } else {
                        format!("收藏表情 ({})", entries.len())
                    })
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                self.render_icon_button(
                                    "import-sticker",
                                    IconName::Plus,
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.import_sticker();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_icon_button(
                                    "close-stickers",
                                    IconName::Close,
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_stickers = false;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        self.render_button("qq-face-tab", "QQ 表情", self.show_qq_faces, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_qq_faces = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("favorite-tab", "收藏", !self.show_qq_faces, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_qq_faces = false;
                                cx.notify();
                            })),
                    )
                    .when(self.show_qq_faces, |element| {
                        element
                            .child(div().flex_1())
                            .child(
                                self.render_button("face-prev", "上一页", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.face_page = this.face_page.saturating_sub(1);
                                        cx.notify();
                                    })),
                            )
                            .child(format!("{}/{}", self.face_page + 1, face_pages))
                            .child(
                                self.render_button("face-next", "下一页", false, cx)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.face_page =
                                            (this.face_page + 1).min(face_pages.saturating_sub(1));
                                        cx.notify();
                                    })),
                            )
                    }),
            )
            .when_some(fallback_notice, |element, notice| {
                element.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(colors.text_accent)
                        .child(notice),
                )
            })
            .child(
                div()
                    .id("sticker-scroll")
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .flex_1()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_2()
                    .when(!self.show_qq_faces && entries.is_empty(), |element| {
                        element.child(
                            div().p_3().text_color(colors.text_muted).child(format!(
                                "目录为空：{}",
                                self.sticker_store.root().display()
                            )),
                        )
                    })
                    .when(self.show_qq_faces, |element| {
                        element.children(face_entries.into_iter().map(|(face_id, image, name)| {
                            div()
                                .id(("qq-face", face_id as usize))
                                .size(px(52.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(colors.ghost_element_hover))
                                .tooltip(Tooltip::text(name.unwrap_or("QQ 表情")))
                                .child(img(image).size(px(38.)).object_fit(ObjectFit::Contain))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.composer.update(cx, |input, cx| {
                                        input.insert_text(&format!("[Face: {face_id}]"), window, cx)
                                    });
                                    window.focus(&this.composer.focus_handle(cx), cx);
                                    this.show_stickers = false;
                                    cx.notify();
                                }))
                        }))
                    })
                    .when(!self.show_qq_faces, |element| {
                        element.children(entries.into_iter().enumerate().map(|(index, entry)| {
                            let selected_entry = entry.clone();
                            div()
                                .id(("sticker-entry", index))
                                .size(px(88.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(colors.ghost_element_hover))
                                .child(
                                    img(entry.path)
                                        .size(px(68.))
                                        .object_fit(ObjectFit::Contain)
                                        .rounded_sm(),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_center()
                                        .text_size(px(10.))
                                        .text_color(colors.text_muted)
                                        .child(entry.name),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send_sticker(&selected_entry);
                                    cx.notify();
                                }))
                        }))
                    }),
            )
    }

    fn render_member_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let room_id = self
            .active()
            .and_then(|bridge| bridge.selected_room_id)
            .unwrap_or_default();
        let members = self
            .active()
            .and_then(|bridge| bridge.group_members.get(&room_id))
            .cloned()
            .unwrap_or_default();
        let member_query = self.member_search.read(cx).text().trim().to_lowercase();
        let mute_duration = self
            .mute_duration
            .read(cx)
            .text()
            .trim()
            .parse::<u64>()
            .unwrap_or(600)
            .min(crate::ica::GROUP_BAN_MAX_DURATION);
        let now = chrono::Utc::now().timestamp();
        let members = members
            .into_iter()
            .filter(|member| {
                member_query.is_empty()
                    || member.display_name().to_lowercase().contains(&member_query)
                    || member.user_id.to_string().contains(&member_query)
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .w(px(320.))
            .h_full()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(64.))
                    .px_4()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(format!("群成员 ({})", members.len()))
                    .child(
                        self.render_icon_button("close-members", IconName::Close, false, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_members = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(div().flex_1().child(self.member_search.clone()))
                    .child(div().w(px(120.)).child(self.mute_duration.clone()))
                    .child(
                        self.render_icon_button(
                            "refresh-members",
                            IconName::HistoryRerun,
                            false,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = this.send_command(IcaCommand::FetchGroupMembers { room_id });
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("member-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(members.into_iter().enumerate().map(|(index, member)| {
                        let poke_id = member.user_id;
                        let mute_id = member.user_id;
                        let is_muted = member.shutup_time > now;
                        let display_name = member.display_name().to_string();
                        div()
                            .id(("member", index))
                            .flex()
                            .items_center()
                            .h(px(64.))
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(
                                Avatar::new(format!(
                                    "https://q1.qlogo.cn/g?b=qq&nk={}&s=140",
                                    member.user_id.abs()
                                ))
                                .size(px(36.)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(div().truncate().text_size(px(14.)).child(display_name))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(colors.text_muted)
                                            .child(format!(
                                                "{} · {}{}",
                                                member.user_id,
                                                member.role,
                                                if is_muted { " · 禁言中" } else { "" }
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        self.render_message_action(
                                            ("poke", index),
                                            IconName::Bell,
                                            cx,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, _| {
                                                let _ =
                                                    this.send_command(IcaCommand::SendGroupPoke {
                                                        room_id,
                                                        target_id: poke_id,
                                                    });
                                            }),
                                        ),
                                    )
                                    .child(
                                        self.render_message_action(
                                            ("mute", index),
                                            IconName::MicMute,
                                            cx,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, _| {
                                                let _ =
                                                    this.send_command(IcaCommand::SetGroupBan {
                                                        room_id,
                                                        target_id: mute_id,
                                                        duration: if is_muted {
                                                            0
                                                        } else {
                                                            mute_duration
                                                        },
                                                    });
                                            }),
                                        ),
                                    ),
                            )
                    })),
            )
    }

    fn render_simple_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let config_snapshot = self.config.snapshot();
        let theme_mode = config_snapshot.ui_setting.theme_mode;
        let mut root = div()
            .id("page-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .w_full()
            .max_w(px(900.))
            .mx_auto()
            .p_6()
            .gap_4()
            .overflow_y_scroll();
        match self.page {
            Page::Groups => {
                let selected_room_id = self.active().and_then(|bridge| bridge.selected_room_id);
                let selected_room_name = self.active().and_then(|bridge| {
                    bridge
                        .rooms
                        .iter()
                        .find(|room| Some(room.room_id) == selected_room_id)
                        .map(|room| room.room_name.clone())
                });
                let groups = self
                    .active()
                    .map(|bridge| bridge.chat_groups.groups.clone())
                    .unwrap_or_default();
                let group_count = groups.len();
                root = root
                    .child(div().text_xl().child("聊天分组管理"))
                    .child(div().text_sm().text_color(colors.text_muted).child(
                        match (selected_room_name, selected_room_id) {
                            (Some(name), Some(id)) => {
                                format!("当前会话：{name} ({id})，可直接加入或移出分组")
                            }
                            _ => "先在消息页选择会话，再管理其所属分组".to_string(),
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(280.)).child(self.group_name.clone()))
                            .child(
                                self.render_button("create-group", "创建分组", true, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let name = this.group_name.read(cx).text().to_string();
                                        this.create_chat_group(name);
                                        this.group_name.update(cx, |input, cx| input.clear(cx));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(groups.is_empty(), |element| {
                        element.child(
                            div()
                                .p_4()
                                .rounded_md()
                                .bg(colors.surface_background)
                                .text_color(colors.text_muted)
                                .child("尚未创建聊天分组"),
                        )
                    })
                    .children(groups.into_iter().enumerate().map(|(index, group)| {
                        let contains_selected =
                            selected_room_id.is_some_and(|room_id| group.rooms.contains(&room_id));
                        div()
                            .id(("chat-group", index))
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.border_variant)
                            .bg(colors.surface_background)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(15.)).child(group.name))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.text_muted)
                                            .child(format!(
                                                "{} 个指定会话{}",
                                                group.rooms.len(),
                                                if group.include_all_personal {
                                                    " · 包含全部私聊"
                                                } else {
                                                    ""
                                                }
                                            )),
                                    ),
                            )
                            .child(
                                self.render_button(("rename-group", index), "重命名", false, cx)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let name = this.group_name.read(cx).text().to_string();
                                        this.rename_chat_group(index, name);
                                        cx.notify();
                                    })),
                            )
                            .when(index > 0, |element| {
                                element.child(
                                    self.render_button(("move-group-up", index), "↑", false, cx)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.move_chat_group(index, true);
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(index + 1 < group_count, |element| {
                                element.child(
                                    self.render_button(("move-group-down", index), "↓", false, cx)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.move_chat_group(index, false);
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                self.render_button(
                                    ("group-room", index),
                                    if contains_selected {
                                        "移出当前会话"
                                    } else {
                                        "加入当前会话"
                                    },
                                    contains_selected,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.update_chat_group(index, true, false);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_button(
                                    ("group-private", index),
                                    "全部私聊",
                                    group.include_all_personal,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.update_chat_group(index, false, true);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_button(("delete-group", index), "删除", false, cx)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_chat_group(index);
                                        cx.notify();
                                    })),
                            )
                    }));
            }
            Page::Contacts => {
                let contacts =
                    self.active()
                        .map(|bridge| {
                            bridge
                                .friends
                                .iter()
                                .map(|friend| (friend.room_id(), friend.display_name(), "好友"))
                                .chain(
                                    bridge.groups.iter().map(|group| {
                                        (group.room_id(), group.display_name(), "群聊")
                                    }),
                                )
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                let contact_count = contacts.len();
                root = root
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("联系人"),
                            )
                            .child(
                                div()
                                    .pb(px(2.))
                                    .text_size(px(12.))
                                    .text_color(colors.text_muted)
                                    .child(format!("{contact_count} 项")),
                            )
                            .child(div().flex_1())
                            .child(
                                self.render_button("refresh-contact-page", "刷新", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_contacts();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().border_t_1().border_color(colors.border))
                    .children(contacts.into_iter().map(|(room_id, name, kind)| {
                        let open_name = name.clone();
                        let avatar_url = if room_id < 0 {
                            let group_id = room_id.abs();
                            format!("https://p.qlogo.cn/gh/{group_id}/{group_id}/0")
                        } else {
                            format!("https://q1.qlogo.cn/g?b=qq&nk={room_id}&s=140")
                        };
                        div()
                            .id(SharedString::from(format!("contact-{room_id}")))
                            .flex()
                            .items_center()
                            .h(px(58.))
                            .px_3()
                            .gap_3()
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .cursor_pointer()
                            .hover(|style| style.bg(colors.ghost_element_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_contact(room_id, open_name.clone(), cx);
                            }))
                            .child(Avatar::new(avatar_url).size(px(38.)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(div().truncate().text_size(px(14.)).child(name))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.text_muted)
                                            .child(room_id.abs().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.text_muted)
                                    .child(kind),
                            )
                    }));
            }
            Page::Requests => {
                let requests = self
                    .active()
                    .map(|bridge| bridge.requests.clone())
                    .unwrap_or_default();
                root = root
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().text_xl().child("验证消息"))
                            .child(div().flex_1())
                            .child(
                                self.render_button("refresh-requests", "刷新", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let _ = this.send_command(IcaCommand::GetSystemMsg);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(requests.is_empty(), |element| {
                        element.child(
                            div()
                                .p_4()
                                .rounded_md()
                                .bg(colors.surface_background)
                                .text_color(colors.text_muted)
                                .child("当前没有新的验证消息"),
                        )
                    })
                    .children(requests.into_iter().enumerate().map(|(index, request)| {
                        let accept = request.clone();
                        let reject = request.clone();
                        div()
                            .id(("request", index))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_md()
                            .bg(colors.surface_background)
                            .child(format!("{} {}", request.nickname, request.comment))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.text_muted)
                                    .child(request.tips),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        self.render_button(("accept", index), "同意", true, cx)
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                let _ =
                                                    this.send_command(IcaCommand::HandleRequest {
                                                        request_type: accept.request_type.clone(),
                                                        flag: accept.flag.clone(),
                                                        accept: true,
                                                    });
                                            })),
                                    )
                                    .child(
                                        self.render_button(("reject", index), "拒绝", false, cx)
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                let _ =
                                                    this.send_command(IcaCommand::HandleRequest {
                                                        request_type: reject.request_type.clone(),
                                                        flag: reject.flag.clone(),
                                                        accept: false,
                                                    });
                                            })),
                                    ),
                            )
                    }));
            }
            Page::Relation => {
                if let Some(bridge) = self.active() {
                    let query = self.relation_search.read(cx).text().trim().to_lowercase();
                    let groups = bridge
                        .rooms
                        .iter()
                        .filter(|room| room.room_id < 0)
                        .filter_map(|room| {
                            let members = bridge.group_members.get(&room.room_id);
                            let matches = query.is_empty()
                                || room.room_name.to_lowercase().contains(&query)
                                || room.room_id.abs().to_string().contains(&query)
                                || members.is_some_and(|members| {
                                    members.iter().any(|member| {
                                        member.display_name().to_lowercase().contains(&query)
                                            || member.user_id.to_string().contains(&query)
                                    })
                                });
                            matches.then(|| {
                                (
                                    room.room_id,
                                    room.room_name.clone(),
                                    members.map_or(0, Vec::len),
                                )
                            })
                        })
                        .collect::<Vec<_>>();
                    let loaded_groups = bridge.group_members.len();
                    let loaded_members = bridge.group_members.values().map(Vec::len).sum::<usize>();
                    root = root
                        .child(div().text_xl().child("关系概览（列表模式）"))
                        .child(format!(
                            "账号：{} ({})",
                            bridge.online.nick, bridge.online.qqid
                        ))
                        .child(format!(
                            "会话：{}，好友：{}，群聊：{}",
                            bridge.rooms.len(),
                            bridge.friends.len(),
                            bridge.groups.len()
                        ))
                        .child(format!(
                            "Bridge：{} / {} 个客户端",
                            bridge.online.icalingua_info.ica_version,
                            bridge.online.icalingua_info.client_count
                        ))
                        .child(format!(
                            "已加载 {loaded_groups} 个群、{loaded_members} 条成员关系"
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().child(self.relation_search.clone()))
                                .child(
                                    self.render_button(
                                        "load-all-relations",
                                        "加载全部群成员",
                                        false,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let room_ids = this
                                                .active()
                                                .map(|bridge| {
                                                    bridge
                                                        .rooms
                                                        .iter()
                                                        .filter(|room| room.room_id < 0)
                                                        .map(|room| room.room_id)
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_default();
                                            for room_id in room_ids {
                                                let _ = this.send_command(
                                                    IcaCommand::FetchGroupMembers { room_id },
                                                );
                                            }
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .children(groups.into_iter().map(|(room_id, name, member_count)| {
                            div()
                                .id(SharedString::from(format!("relation-group-{room_id}")))
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .h(px(52.))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .child(
                                    Avatar::new(format!(
                                        "https://p.qlogo.cn/gh/{0}/{0}/0",
                                        room_id.abs()
                                    ))
                                    .size(px(34.)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().truncate().child(name))
                                        .child(
                                            div().text_sm().text_color(colors.text_muted).child(
                                                format!(
                                                    "{} · {} 个已加载成员",
                                                    room_id.abs(),
                                                    member_count
                                                ),
                                            ),
                                        ),
                                )
                                .child(
                                    self.render_button(
                                        SharedString::from(format!("load-relation-{room_id}")),
                                        "加载/刷新",
                                        false,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let _ =
                                                this.send_command(IcaCommand::FetchGroupMembers {
                                                    room_id,
                                                });
                                            cx.notify();
                                        },
                                    )),
                                )
                        }));
                }
            }
            Page::Tools => {
                let auto_sign_label = if self.auto_sign_running {
                    format!(
                        "停止签到 ({}/{})",
                        self.auto_sign_progress.0, self.auto_sign_progress.1
                    )
                } else {
                    "全部群签到".to_string()
                };
                let active_status = self.active().map(|bridge| {
                    (
                        bridge.key.clone(),
                        bridge.socket_status.clone(),
                        bridge.auth_status.clone(),
                        bridge.rooms.len(),
                        bridge.conversations.len(),
                        bridge.last_event.clone(),
                        bridge.last_response.clone(),
                        bridge.is_shut_up,
                    )
                });
                let selected_room = self.active().and_then(|bridge| {
                    let room_id = bridge.selected_room_id?;
                    bridge
                        .rooms
                        .iter()
                        .find(|room| room.room_id == room_id)
                        .map(|room| (room.room_id, room.room_name.clone(), room.priority))
                });
                root = root
                    .child(div().text_xl().child("状态与管理工具"))
                    .when_some(active_status, |element, status| {
                        element.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .p_3()
                                .rounded_md()
                                .bg(colors.surface_background)
                                .child(format!(
                                    "{} · {} · {}{}",
                                    status.0,
                                    status.1,
                                    status.2,
                                    if status.7 { " · 禁言中" } else { "" }
                                ))
                                .child(div().text_sm().text_color(colors.text_muted).child(
                                    format!(
                                        "{} 个会话 · {} 个已缓存会话 · 最近事件 {}",
                                        status.3,
                                        status.4,
                                        status.5.as_deref().unwrap_or("无")
                                    ),
                                ))
                                .when_some(status.6, |card, response| {
                                    card.child(
                                        div()
                                            .text_sm()
                                            .text_color(colors.text_muted)
                                            .child(format!("最近响应：{response}")),
                                    )
                                }),
                        )
                    })
                    .child(div().text_lg().child("在线状态与签到"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                self.render_button(
                                    "sign-all",
                                    auto_sign_label,
                                    self.auto_sign_running,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.sign_all_groups(cx);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_button("online", "设为在线", false, cx)
                                    .on_click(cx.listener(|this, _, _, _| {
                                        let _ = this.send_command(IcaCommand::SetOnlineStatus(11));
                                    })),
                            )
                            .child(self.render_button("away", "设为离开", false, cx).on_click(
                                cx.listener(|this, _, _, _| {
                                    let _ = this.send_command(IcaCommand::SetOnlineStatus(31));
                                }),
                            ))
                            .children(
                                [
                                    ("hidden", "隐身", 41_u8),
                                    ("busy", "忙碌", 50_u8),
                                    ("ping-me", "Q我吧", 60_u8),
                                    ("dnd", "请勿打扰", 70_u8),
                                ]
                                .into_iter()
                                .map(|(id, label, status)| {
                                    self.render_button(id, label, false, cx)
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            let _ = this
                                                .send_command(IcaCommand::SetOnlineStatus(status));
                                        }))
                                }),
                            ),
                    )
                    .when_some(selected_room, |element, (room_id, room_name, priority)| {
                        element
                            .child(div().text_lg().child("当前会话"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.text_muted)
                                    .child(format!("{room_name} ({room_id}) · 优先级 {priority}")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .children((1_u8..=5).map(|value| {
                                        self.render_button(
                                            ("priority", value as usize),
                                            format!("优先级 {value}"),
                                            priority == value,
                                            cx,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if this
                                                    .send_command(IcaCommand::SetRoomPriority {
                                                        room_id,
                                                        priority: value,
                                                    })
                                                    .is_ok()
                                                    && let Some(bridge) = this.active_mut()
                                                    && let Some(room) = bridge
                                                        .rooms
                                                        .iter_mut()
                                                        .find(|room| room.room_id == room_id)
                                                {
                                                    room.priority = value;
                                                    bridge.sort_rooms();
                                                }
                                                cx.notify();
                                            }),
                                        )
                                    }))
                                    .child(
                                        self.render_button("ignore-room", "忽略会话", false, cx)
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                let _ = this.send_command(IcaCommand::IgnoreChat {
                                                    room_id,
                                                    room_name: room_name.clone(),
                                                });
                                            })),
                                    )
                                    .child(
                                        self.render_button("unignore-room", "移除忽略", false, cx)
                                            .on_click(cx.listener(move |this, _, _, _| {
                                                let _ = this.send_command(
                                                    IcaCommand::RemoveIgnoredChat(room_id),
                                                );
                                            })),
                                    ),
                            )
                    })
                    .child(div().text_lg().child("常用 Bridge 操作"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(
                                [
                                    ("login-devices", "登录设备", "getLoginDevices"),
                                    ("disabled-features", "禁用功能", "getDisabledFeatures"),
                                    ("ignored-chats", "忽略会话列表", "getIgnoredChats"),
                                    ("group-list", "群列表", "getGroupList"),
                                    ("friend-list", "好友列表", "getFriendList"),
                                ]
                                .into_iter()
                                .map(|(id, label, event)| {
                                    self.render_button(id, label, false, cx)
                                        .on_click(cx.listener(move |this, _, _, _| {
                                            let _ = this.send_command(IcaCommand::SocketApiCall {
                                                event: event.to_string(),
                                                args: Vec::new(),
                                                expect_ack: true,
                                            });
                                        }))
                                }),
                            )
                            .child(
                                self.render_button("refresh-contacts", "刷新联系人", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_contacts();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().text_lg().child("高级 Socket.IO API"))
                    .child(self.tool_event.clone())
                    .child(self.tool_args.clone())
                    .child(
                        self.render_button("call-socket", "调用", true, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let event = this.tool_event.read(cx).text().trim().to_string();
                                let args_text = this.tool_args.read(cx).text().trim().to_string();
                                match serde_json::from_str::<Vec<JsonValue>>(&args_text) {
                                    Ok(args) if !event.is_empty() => {
                                        let _ = this.send_command(IcaCommand::SocketApiCall {
                                            event,
                                            args,
                                            expect_ack: true,
                                        });
                                    }
                                    Ok(_) => this.set_error("事件名不能为空"),
                                    Err(error) => {
                                        this.set_error(format!("参数 JSON 无效: {error}"))
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_lg().child("文件管理 API"))
                    .child(self.tool_gin.clone())
                    .child(
                        self.render_button("call-file-manager", "调用文件管理", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let gin = this.tool_gin.read(cx).text().trim().parse::<i64>();
                                let event = this.tool_event.read(cx).text().trim().to_string();
                                let args = serde_json::from_str::<Vec<JsonValue>>(
                                    this.tool_args.read(cx).text().trim(),
                                );
                                match (gin, args) {
                                    (Ok(gin), Ok(args)) if !event.is_empty() => {
                                        let _ = this.send_command(IcaCommand::FileManagerCall {
                                            gin,
                                            event,
                                            args,
                                            expect_ack: true,
                                        });
                                    }
                                    (Err(_), _) => this.set_error("gin 不是有效数字"),
                                    (_, Err(error)) => {
                                        this.set_error(format!("参数 JSON 无效: {error}"))
                                    }
                                    _ => this.set_error("事件名不能为空"),
                                }
                                cx.notify();
                            })),
                    );
            }
            Page::Settings => {
                root = root
                    .child(div().text_xl().child("外观与运行状态"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.render_button(
                                    "system-theme",
                                    "跟随系统",
                                    theme_mode == ThemeMode::System,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.follow_system_theme(cx)),
                                ),
                            )
                            .child(
                                self.render_button(
                                    "dark-theme",
                                    "One Dark",
                                    theme_mode == ThemeMode::Dark,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.switch_theme(false, cx)),
                                ),
                            )
                            .child(
                                self.render_button(
                                    "light-theme",
                                    "One Light",
                                    theme_mode == ThemeMode::Light,
                                    cx,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.switch_theme(true, cx)),
                                ),
                            ),
                    )
                    .child(div().text_lg().child("聊天行为"))
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            [
                                (
                                    "clear_search",
                                    "切换会话清空搜索",
                                    config_snapshot.ui_setting.clear_search_on_room_select,
                                ),
                                (
                                    "auto_history",
                                    "切换会话拉取漫游",
                                    config_snapshot.ui_setting.auto_fetch_history_on_room_select,
                                ),
                                (
                                    "scroll_send",
                                    "发送后滚动到底部",
                                    config_snapshot.ui_setting.scroll_to_bottom_after_send,
                                ),
                                (
                                    "auto_read",
                                    "选中会话自动已读",
                                    config_snapshot.custom_chat.auto_read_on_select,
                                ),
                            ]
                            .into_iter()
                            .map(|(key, label, enabled)| {
                                self.render_button(
                                    SharedString::from(format!("setting-{key}")),
                                    label,
                                    enabled,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.toggle_setting(key);
                                        cx.notify();
                                    },
                                ))
                            }),
                        ),
                    )
                    .child("撤回消息重新编辑时的草稿处理")
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                self.render_button(
                                    "reedit-overwrite",
                                    "覆盖原草稿",
                                    config_snapshot.ui_setting.reedit_draft_conflict_mode
                                        == ReEditDraftConflictMode::Overwrite,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_reedit_mode(ReEditDraftConflictMode::Overwrite);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_button(
                                    "reedit-append",
                                    "追加到原草稿",
                                    config_snapshot.ui_setting.reedit_draft_conflict_mode
                                        == ReEditDraftConflictMode::Append,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_reedit_mode(ReEditDraftConflictMode::Append);
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                self.render_button(
                                    "reedit-skip",
                                    "有草稿时跳过",
                                    config_snapshot.ui_setting.reedit_draft_conflict_mode
                                        == ReEditDraftConflictMode::SkipIfNonEmpty,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_reedit_mode(
                                            ReEditDraftConflictMode::SkipIfNonEmpty,
                                        );
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(div().text_lg().child("聊天显示"))
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            [
                                (
                                    "hide_images",
                                    "隐藏聊天图片",
                                    config_snapshot.custom_chat.hide_chat_img,
                                ),
                                (
                                    "hide_avatars",
                                    "纯文字模式",
                                    config_snapshot.custom_chat.hide_group_member_avatar,
                                ),
                                (
                                    "high_contrast",
                                    "高对比度气泡",
                                    config_snapshot.custom_chat.high_contrast_chat,
                                ),
                                (
                                    "disable_groups",
                                    "禁用聊天分组",
                                    config_snapshot.custom_chat.disable_chat_group,
                                ),
                                (
                                    "disable_group_dots",
                                    "禁用分组红点",
                                    config_snapshot.custom_chat.disable_chat_group_dot,
                                ),
                                (
                                    "highlight_urls",
                                    "禁用 URL 高亮",
                                    config_snapshot.custom_chat.disable_highlight_url,
                                ),
                                (
                                    "sort_stickers",
                                    "表情按时间倒序",
                                    config_snapshot.custom_chat.sort_stickers_by_time,
                                ),
                            ]
                            .into_iter()
                            .map(|(key, label, enabled)| {
                                self.render_button(
                                    SharedString::from(format!("setting-{key}")),
                                    label,
                                    enabled,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.toggle_setting(key);
                                        cx.notify();
                                    },
                                ))
                            }),
                        ),
                    )
                    .child(format!("会话栏宽度：{:.0}px", self.room_panel_width))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.render_button("room-narrower", "会话栏 -20", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.adjust_panel_width(true, -20.);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.render_button("room-wider", "会话栏 +20", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.adjust_panel_width(true, 20.);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(format!("表情栏宽度：{:.0}px", self.sticker_panel_width))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.render_button("sticker-narrower", "表情栏 -20", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.adjust_panel_width(false, -20.);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                self.render_button("sticker-wider", "表情栏 +20", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.adjust_panel_width(false, 20.);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(format!(
                        "配置文件：{}",
                        self.config.paths().config_file().display()
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.render_button("open-config", "用记事本打开配置", false, cx)
                                    .on_click(cx.listener(|this, _, _, _| {
                                        if let Err(error) =
                                            std::process::Command::new("notepad.exe")
                                                .arg(this.config.paths().config_file())
                                                .spawn()
                                        {
                                            this.set_error(format!("打开配置失败: {error}"));
                                        }
                                    })),
                            )
                            .child(
                                self.render_button("reload-config", "重新载入配置", false, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        match this.config.reload() {
                                            Ok(()) => {
                                                if let Some(bridge) = this.active_mut() {
                                                    bridge.last_notice =
                                                        Some("配置已重新载入".to_string());
                                                }
                                            }
                                            Err(error) => {
                                                this.set_error(format!("载入配置失败: {error}"))
                                            }
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(format!(
                        "版本：{} · Bridge 协议 {}",
                        crate::VERSION,
                        crate::ica::ICA_PROTOCOL_VERSION
                    ))
                    .child(
                        self.render_button("open-github", "打开 GitHub / 文档", false, cx)
                            .on_click(cx.listener(|_, _, _, cx| cx.open_url(crate::GITHUB_LINK))),
                    );
            }
            Page::Chat => {}
        }
        root
    }

    fn pick_image(&mut self, _: &gpui::ClickEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_files()
        else {
            return;
        };
        self.queue_attachments(paths);
    }

    fn copy_viewer_image(&mut self, url: String, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this, cx| match download_image_bytes(&url).await {
                Ok((format, bytes)) => {
                    let image = gpui::Image::from_bytes(format, bytes);
                    let _ = this.update(cx, |this, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_image(&image));
                        if let Some(bridge) = this.active_mut() {
                            bridge.last_notice = Some("图片已复制到剪贴板".to_string());
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("复制图片失败: {error}"));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn save_viewer_image(&mut self, url: String, cx: &mut Context<Self>) {
        let file_name = url
            .split(['?', '#'])
            .next()
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .unwrap_or("image.png");
        let Some(path) = rfd::FileDialog::new().set_file_name(file_name).save_file() else {
            return;
        };
        cx.spawn(
            async move |this, cx| match download_image_bytes(&url).await {
                Ok((_, bytes)) => {
                    let result = std::fs::write(&path, bytes).map_err(|error| error.to_string());
                    let _ = this.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                if let Some(bridge) = this.active_mut() {
                                    bridge.last_notice =
                                        Some(format!("图片已保存到 {}", path.display()));
                                }
                            }
                            Err(error) => this.set_error(format!("保存图片失败: {error}")),
                        }
                        cx.notify();
                    });
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("下载图片失败: {error}"));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn render_image_viewer(&self, url: String, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let copy_image_url = url.clone();
        let copy_url = url.clone();
        let external_url = url.clone();
        let save_url = url.clone();
        let zoom = self.image_viewer_zoom;
        div()
            .id("image-viewer-overlay")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .bg(gpui::rgba(0x000000dd))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(format!("{:.0}%", zoom * 100.0))
                    .child(self.render_button("fit-image", "适应", false, cx).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.image_viewer_zoom = 1.0;
                            cx.notify();
                        }),
                    ))
                    .child(
                        self.render_button("zoom-image-out", "−", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.image_viewer_zoom = (this.image_viewer_zoom - 0.25).max(0.25);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("zoom-image-in", "+", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.image_viewer_zoom = (this.image_viewer_zoom + 0.25).min(4.0);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_button("copy-image", "复制图片", false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_viewer_image(copy_image_url.clone(), cx);
                            })),
                    )
                    .child(
                        self.render_button("copy-image-url", "复制 URL", false, cx)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                            })),
                    )
                    .child(
                        self.render_button("save-image", "另存为", false, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_viewer_image(save_url.clone(), cx);
                            })),
                    )
                    .child(
                        self.render_button("open-image-external", "浏览器打开", false, cx)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.open_url(&external_url);
                            })),
                    )
                    .child(
                        self.render_button("close-image-viewer", "关闭", true, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.image_viewer_url = None;
                                this.image_viewer_zoom = 1.0;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("image-viewer-scroll")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(colors.background)
                    .child(
                        div().flex_none().w(relative(zoom)).h(relative(zoom)).child(
                            img(url)
                                .w_full()
                                .h_full()
                                .object_fit(ObjectFit::Contain)
                                .rounded_lg(),
                        ),
                    ),
            )
    }

    fn pick_file(&mut self, _: &gpui::ClickEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };
        self.queue_attachments(vec![path]);
    }

    fn queue_attachments(&mut self, paths: Vec<std::path::PathBuf>) {
        let Some(room_id) = self.active().and_then(|bridge| bridge.selected_room_id) else {
            self.set_error("请先选择会话");
            return;
        };
        let mut images = Vec::new();
        let mut file = None;
        for path in paths {
            let Ok(data) = std::fs::read(&path) else {
                self.set_error(format!("读取附件失败: {}", path.display()));
                continue;
            };
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("file")
                .to_string();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("bin")
                .to_ascii_lowercase();
            let mime = match extension.as_str() {
                "png" => Some("image/png"),
                "jpg" | "jpeg" => Some("image/jpeg"),
                "gif" => Some("image/gif"),
                "webp" => Some("image/webp"),
                "bmp" => Some("image/bmp"),
                _ => None,
            };
            if let Some(mime) = mime {
                images.push(PendingImage {
                    name,
                    mime: mime.to_string(),
                    data: data.into(),
                });
            } else if file.is_none() {
                file = Some(PendingFile {
                    name,
                    file_type: extension,
                    data: data.into(),
                });
            }
        }
        if let Some(bridge) = self.active_mut() {
            let conversation = bridge.conversation_mut(room_id);
            if !images.is_empty() {
                conversation.pending_file = None;
                conversation.pending_images.extend(images);
            } else if let Some(file) = file {
                conversation.pending_images.clear();
                conversation.pending_file = Some(file);
            }
        }
    }

    fn handle_file_drop(
        &mut self,
        paths: &gpui::ExternalPaths,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_attachments(
            paths
                .paths()
                .iter()
                .map(|path| path.to_path_buf())
                .collect(),
        );
        cx.notify();
    }
}

impl Render for IcaApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let status = self.active().and_then(|bridge| {
            bridge
                .last_error
                .clone()
                .map(|message| (message, true))
                .or_else(|| bridge.last_notice.clone().map(|message| (message, false)))
        });
        div()
            .id("app-root")
            .relative()
            .on_drop(cx.listener(Self::handle_file_drop))
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family("Noto Sans CJK SC")
            .child(self.render_top_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.page == Page::Chat, |element| {
                        element
                            .child(self.render_group_rail(cx))
                            .child(self.render_room_list(cx))
                    })
                    .child(if self.page == Page::Chat {
                        self.render_chat(cx).into_any_element()
                    } else {
                        self.render_simple_page(cx).into_any_element()
                    })
                    .when(self.page == Page::Chat && self.show_stickers, |element| {
                        element.child(self.render_sticker_panel(cx))
                    })
                    .when(self.page == Page::Chat && self.show_members, |element| {
                        element.child(self.render_member_panel(cx))
                    }),
            )
            .when_some(status, |element, (message, error)| {
                element.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(colors.border)
                        .bg(colors.status_bar_background)
                        .text_sm()
                        .text_color(if error {
                            colors.text_accent
                        } else {
                            colors.text_muted
                        })
                        .child(message)
                        .child(div().flex_1())
                        .child(
                            self.render_message_action("clear-status", IconName::Close, cx)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(bridge) = this.active_mut() {
                                        if error {
                                            bridge.last_error = None;
                                        } else {
                                            bridge.last_notice = None;
                                        }
                                    }
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.image_viewer_url.clone(), |element, url| {
                element.child(self.render_image_viewer(url, cx))
            })
    }
}
