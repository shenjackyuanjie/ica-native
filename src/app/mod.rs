use std::collections::HashMap;

use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, InteractiveElement, KeyBinding, ObjectFit,
    SharedString, Window, div, img, prelude::*, px, relative,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use theme::{ActiveTheme, Appearance, GlobalTheme, SystemAppearance, ThemeRegistry};
use ui::{Avatar, Color, Icon, IconName, IconSize, Tooltip};

use crate::config::chat_groups::ChatGroup;
use crate::config::{ChatGroups, ConfigStore, IcaCfg, ThemeMode};
use crate::ica::types::{
    RoomId,
    contact::{FriendContact, GroupContact},
    message::{At, DeleteMessage, Mention, Message, NewMessage, ReplyMessage, SendMessage},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Chat,
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
            Self::Contacts => "联系人",
            Self::Requests => "验证",
            Self::Relation => "关系",
            Self::Tools => "工具",
            Self::Settings => "设置",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Chat => IconName::Chat,
            Self::Contacts => IconName::Person,
            Self::Requests => IconName::Bell,
            Self::Relation => IconName::GitGraph,
            Self::Tools => IconName::Terminal,
            Self::Settings => IconName::Settings,
        }
    }
}

#[derive(Default)]
struct Conversation {
    messages: Vec<Message>,
    reply_to: Option<ReplyMessage>,
    requested: bool,
    no_more_history: bool,
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
    pending_forward: Option<(RoomId, JsonValue)>,
    group_members: HashMap<RoomId, Vec<GroupMember>>,
    last_notice: Option<String>,
    last_error: Option<String>,
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
            pending_forward: None,
            group_members: HashMap::new(),
            last_notice: None,
            last_error: None,
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
    message_search: Entity<TextInput>,
    show_stickers: bool,
    show_members: bool,
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
        let message_search = cx.new(|cx| {
            TextInput::new("搜索聊天记录", cx).with_presentation(InputPresentation::Search)
        });
        let snapshot = config.snapshot();
        let sticker_store =
            StickerStore::resolve(&snapshot, config.paths()).unwrap_or_else(|error| {
                StickerStore::unavailable(config.paths().data_dir().join("stickers"), error)
            });
        if let Err(error) = sticker_store.refresh(snapshot.custom_chat.sort_stickers_by_time) {
            tracing::warn!(%error, "刷新收藏表情失败");
        }

        cx.subscribe(&composer, |this, input, event, cx| match event {
            InputEvent::Submitted(text) => {
                if this.send_text(text.clone()).is_ok() {
                    input.update(cx, |input, cx| input.clear(cx));
                }
                cx.notify();
            }
            InputEvent::Changed => cx.notify(),
        })
        .detach();
        cx.subscribe(&room_search, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&tool_event, |_, _, _, cx| cx.notify())
            .detach();
        cx.subscribe(&tool_args, |_, _, _, cx| cx.notify()).detach();
        cx.subscribe(&message_search, |this, input, event, cx| {
            if let InputEvent::Submitted(keyword) = event {
                let keyword = keyword.clone();
                let room_id = this.active().and_then(|bridge| bridge.selected_room_id);
                if let Some(room_id) = room_id {
                    if let Some(bridge) = this.active_mut() {
                        bridge.search_keyword = keyword.clone();
                        bridge.search_results.clear();
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

        let mut event_rx = runtime.take_event_receiver();
        let event_task = cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.apply_bridge_event(event);
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
            message_search,
            show_stickers: false,
            show_members: false,
            room_panel_width: snapshot.ui_setting.room_panel_width.clamp(140.0, 720.0),
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
        if let Some(message_id) = last_message_id {
            let _ = self.send_command(IcaCommand::ReportRead {
                room_id,
                message_id,
            });
        }
        cx.notify();
    }

    fn send_text(&mut self, text: String) -> Result<(), String> {
        let bridge = self.active().ok_or("没有可用的 bridge")?;
        let room_id = bridge.selected_room_id.ok_or("请先选择会话")?;
        let reply = bridge
            .conversations
            .get(&room_id)
            .and_then(|conversation| conversation.reply_to.clone());
        let mentions = if text.contains("@全体成员") {
            vec![Mention {
                user_id: 1,
                text: "全体成员".to_string(),
            }]
        } else {
            Vec::new()
        };
        let mut message = SendMessage::new(text, room_id, reply);
        message.set_mentions(&mentions);
        self.send_command(IcaCommand::SendMessage(message))?;
        if let Some(bridge) = self.active_mut() {
            bridge.conversation_mut(room_id).reply_to = None;
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

    fn apply_bridge_event(&mut self, event: BridgeEvent) {
        let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| bridge.key == event.bridge_key)
        else {
            return;
        };
        let bridge = &mut self.bridges[index];
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
                            let conversation = bridge.conversation_mut(room_id);
                            conversation.no_more_history = messages.is_empty();
                            messages.append(&mut conversation.messages);
                            conversation.messages = messages;
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
                            if let Some(room) =
                                bridge.rooms.iter_mut().find(|room| room.room_id == room_id)
                            {
                                room.last_message.content = Some(new_message.msg.content.clone());
                                room.last_message.username =
                                    Some(new_message.msg.sender_name.clone());
                                room.last_message.timestamp =
                                    Some(new_message.msg.time_text.clone());
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
            BridgeEventKind::SearchMessagesResponse(_) => {
                let keyword = payload
                    .get("keyword")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if keyword == bridge.search_keyword {
                    match payload.get("messages").map(Vec::<Message>::deserialize) {
                        Some(Ok(messages)) => bridge.search_results = messages,
                        Some(Err(error)) => {
                            bridge.last_error = Some(format!("搜索结果解析失败: {error}"))
                        }
                        None => bridge.last_error = Some("搜索响应缺少 messages".to_string()),
                    }
                }
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
            BridgeEventKind::NotifyError(_)
            | BridgeEventKind::MessageError(_)
            | BridgeEventKind::CommandFailed(_)
            | BridgeEventKind::Fatal(_) => {
                bridge.last_error =
                    Self::payload_message(payload).or_else(|| Some(payload.to_string()));
            }
            BridgeEventKind::NotifyMessage(_)
            | BridgeEventKind::MessageSuccess(_)
            | BridgeEventKind::SocketApiResponse(_)
            | BridgeEventKind::FileManagerResponse(_) => {
                bridge.last_notice =
                    Self::payload_message(payload).or_else(|| Some(payload.to_string()));
            }
            _ => {}
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

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let pages = [
            Page::Chat,
            Page::Contacts,
            Page::Requests,
            Page::Relation,
            Page::Tools,
            Page::Settings,
        ];
        div()
            .flex()
            .flex_col()
            .w(px(72.))
            .h_full()
            .items_center()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(
                div()
                    .h(px(56.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(17.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("ica"),
            )
            .children(pages.into_iter().enumerate().map(|(index, page)| {
                let selected = self.page == page;
                div()
                    .id(("page", index))
                    .w(px(60.))
                    .h(px(52.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(11.))
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
                    .child(
                        Icon::new(page.icon())
                            .size(IconSize::Small)
                            .color(if selected {
                                Color::Accent
                            } else {
                                Color::Muted
                            }),
                    )
                    .child(page.label())
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
            .children(self.bridges.iter().enumerate().map(|(index, bridge)| {
                let label = bridge.key.chars().next().unwrap_or('?').to_string();
                let selected = self.active_bridge == Some(index);
                let avatar = if bridge.online.qqid > 0 {
                    Avatar::new(format!(
                        "https://q1.qlogo.cn/g?b=qq&nk={}&s=140",
                        bridge.online.qqid
                    ))
                    .size(px(30.))
                    .into_any_element()
                } else {
                    div()
                        .size(px(30.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors.element_background)
                        .text_sm()
                        .child(label)
                        .into_any_element()
                };
                div()
                    .id(("bridge", index))
                    .size(px(40.))
                    .mb_2()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected {
                        colors.border_focused
                    } else {
                        colors.border_transparent
                    })
                    .bg(if selected {
                        colors.element_selected
                    } else {
                        colors.ghost_element_background
                    })
                    .hover(|style| style.bg(colors.ghost_element_hover))
                    .child(avatar)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_bridge = Some(index);
                        cx.notify();
                    }))
            }))
    }

    fn render_room_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let query = self.room_search.read(cx).text().trim().to_lowercase();
        let selected = self.active().and_then(|bridge| bridge.selected_room_id);
        let room_filter = self
            .active()
            .map_or(RoomFilter::All, |bridge| bridge.room_filter);
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
                            room.last_message.timestamp.clone().unwrap_or_default(),
                            room.unread_count,
                            room.at,
                            room.index > 0,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .w(px(self.room_panel_width))
            .min_w(px(140.))
            .max_w(px(720.))
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.surface_background)
            .child(
                div()
                    .h(px(64.))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(self.room_search.clone()),
            )
            .child(
                div()
                    .id("room-filters")
                    .flex()
                    .items_center()
                    .h(px(38.))
                    .gap_1()
                    .px_3()
                    .overflow_x_scroll()
                    .child(
                        self.render_button(
                            "filter-all",
                            "全部",
                            room_filter == RoomFilter::All,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(bridge) = this.active_mut() {
                                bridge.room_filter = RoomFilter::All;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        self.render_button(
                            "filter-private",
                            "私聊",
                            room_filter == RoomFilter::Private,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(bridge) = this.active_mut() {
                                bridge.room_filter = RoomFilter::Private;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        self.render_button(
                            "filter-group",
                            "群聊",
                            room_filter == RoomFilter::Group,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(bridge) = this.active_mut() {
                                bridge.room_filter = RoomFilter::Group;
                            }
                            cx.notify();
                        })),
                    )
                    .children(custom_groups.into_iter().enumerate().map(|(index, group)| {
                        self.render_button(
                            ("filter-custom", index),
                            group.name,
                            room_filter == RoomFilter::Custom(index),
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(bridge) = this.active_mut() {
                                bridge.room_filter = RoomFilter::Custom(index);
                            }
                            cx.notify();
                        }))
                    })),
            )
            .child(
                div()
                    .id("room-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rooms.into_iter().map(
                        |(
                            room_id,
                            name,
                            avatar_url,
                            preview,
                            sender,
                            timestamp,
                            unread,
                            at,
                            pinned,
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
                                .h(px(68.))
                                .px_3()
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
                                .child(div().mr_3().child(Avatar::new(avatar_url).size(px(40.))))
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
                                                        .text_size(px(15.))
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .child(name),
                                                )
                                                .when(pinned, |element| {
                                                    element.child(
                                                        Icon::new(IconName::Pin)
                                                            .size(IconSize::Indicator)
                                                            .color(Color::Muted),
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
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_size(px(12.))
                                                        .text_color(colors.text_muted)
                                                        .child(if preview.is_empty() {
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
                                                            .bg(badge_color)
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
        let (source_messages, search_keyword) = self
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
                    )
                } else {
                    (
                        bridge.search_results.as_slice(),
                        bridge.search_keyword.clone(),
                    )
                }
            })
            .unwrap_or((&[], String::new()));
        let self_id = self.active().map_or(-1, |bridge| bridge.online.qqid);
        let messages = source_messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
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
                    message.content.clone(),
                    message.time_text.clone(),
                    message.deleted,
                    message.files.clone(),
                    message.as_reply(),
                    message.reply.clone(),
                    message.content.clone(),
                    raw,
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
                            .min_w_0()
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
                            .items_center()
                            .gap_1()
                            .child(div().w(px(170.)).child(self.message_search.clone()))
                            .child(self.render_icon_button("search-messages", IconName::MagnifyingGlass, !search_keyword.is_empty(), cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let keyword = this.message_search.read(cx).text().trim().to_string();
                                    if keyword.is_empty() {
                                        if let Some(bridge) = this.active_mut() {
                                            bridge.search_keyword.clear();
                                            bridge.search_results.clear();
                                        }
                                    } else {
                                        if let Some(bridge) = this.active_mut() {
                                            bridge.search_keyword = keyword.clone();
                                            bridge.search_results.clear();
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
                    .p_4()
                    .gap_3()
                    .when(!search_keyword.is_empty(), |element| {
                        element.child(div().text_sm().text_color(colors.text_accent).child(format!("搜索结果：{search_keyword}")))
                    })
                    .children(messages.into_iter().map(|(id, sender, sender_id, content, time, deleted, files, reply, quoted_reply, edit_content, raw, system, show_date, date, is_self)| {
                        let file_count = files.len();
                        let display_content = if deleted {
                            "[消息已撤回]".to_string()
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
                        let copy_content = edit_content.clone();
                        let reedit_content = edit_content;
                        let attachment_message_id = id.clone();
                        let avatar_url = format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", sender_id.abs());
                        let message_id = id.clone();
                        let reply_id = id.clone();
                        let edit_id = id.clone();
                        let copy_id = id.clone();
                        let forward_id = id.clone();
                        let delete_id = id.clone();
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
                                    this.composer.update(cx, |input, cx| input.set_text(reedit_content.clone(), cx));
                                    window.focus(&this.composer.focus_handle(cx), cx);
                                }),
                            ))
                            .child(self.render_message_action(SharedString::from(format!("copy-{copy_id}")), IconName::Copy, cx).on_click(
                                cx.listener(move |_, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_content.clone()))),
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
                                    .border_color(if is_self { colors.border_selected } else { colors.border_variant })
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
                                    .when(file_count > 0, |element| element.child(
                                        div().flex().flex_col().gap_1().children(files.into_iter().enumerate().map(|(file_index, file)| {
                                            let url = file.url.clone();
                                            let image_url = url.clone();
                                            let label = file.name.unwrap_or_else(|| {
                                                if file.file_type.starts_with("image") { "查看图片".to_string() } else { "打开附件".to_string() }
                                            });
                                            if file.file_type.starts_with("image") && !image_url.is_empty() {
                                                img(image_url.clone())
                                                    .id(SharedString::from(format!("attachment-{attachment_message_id}-{file_index}")))
                                                    .w(px(240.))
                                                    .h(px(180.))
                                                    .object_fit(ObjectFit::Contain)
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .on_click(move |_, _, cx| cx.open_url(&image_url))
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
                            .when(!is_self, |element| {
                                element.child(Avatar::new(leading_avatar_url).size(px(36.)))
                            })
                            .child(bubble)
                            .when(is_self, |element| {
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(room_id < 0, |element| element.child(self.render_icon_button("mention-all", IconName::AtSign, false, cx).on_click(
                                cx.listener(|this, _, window, cx| {
                                    let current = this.composer.read(cx).text().to_string();
                                    let prefix = if current.is_empty() { "" } else { " " };
                                    this.composer.update(cx, |input, cx| {
                                        input.set_text(format!("{current}{prefix}@全体成员 "), cx)
                                    });
                                    window.focus(&this.composer.focus_handle(cx), cx);
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
                                    if !text.is_empty() && this.send_text(text).is_ok() {
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

    fn sign_all_groups(&mut self) {
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
        let mut failed = 0;
        for room_id in &room_ids {
            if self
                .send_command(IcaCommand::SendGroupSign { room_id: *room_id })
                .is_err()
            {
                failed += 1;
            }
        }
        if let Some(bridge) = self.active_mut() {
            bridge.last_notice = Some(format!(
                "已请求 {} 个群签到，失败 {} 个",
                room_ids.len() - failed,
                failed
            ));
        }
    }

    fn adjust_panel_width(&mut self, room_panel: bool, delta: f32) {
        if room_panel {
            self.room_panel_width = (self.room_panel_width + delta).clamp(140.0, 720.0);
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

    fn send_sticker(&mut self, entry: &StickerEntry) {
        let Some(room_id) = self.active().and_then(|bridge| bridge.selected_room_id) else {
            self.set_error("请先选择会话");
            return;
        };
        match self.sticker_store.read_entry(entry) {
            Ok(bytes) => {
                let mut message = SendMessage::new(String::new(), room_id, None);
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
                    .child(format!("收藏表情 ({})", entries.len()))
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
                    .when(entries.is_empty(), |element| {
                        element.child(
                            div().p_3().text_color(colors.text_muted).child(format!(
                                "目录为空：{}",
                                self.sticker_store.root().display()
                            )),
                        )
                    })
                    .children(entries.into_iter().enumerate().map(|(index, entry)| {
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
                    })),
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
                    .id("member-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(members.into_iter().enumerate().map(|(index, member)| {
                        let poke_id = member.user_id;
                        let mute_id = member.user_id;
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
                                                "{} · {} · 禁言至 {}",
                                                member.user_id, member.role, member.shutup_time
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
                                                        duration: 600,
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
        let theme_mode = self.config.snapshot().ui_setting.theme_mode;
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
                            .items_end()
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
                            ),
                    )
                    .child(div().border_t_1().border_color(colors.border))
                    .children(contacts.into_iter().map(|(room_id, name, kind)| {
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
                                this.page = Page::Chat;
                                this.select_room(room_id, cx);
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
                root = root.child(div().text_xl().child("验证消息")).children(
                    requests.into_iter().enumerate().map(|(index, request)| {
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
                    }),
                );
            }
            Page::Relation => {
                if let Some(bridge) = self.active() {
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
                        ));
                }
            }
            Page::Tools => {
                root = root
                    .child(div().text_xl().child("Socket.IO 工具"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                self.render_button("sign-all", "全部群签到", true, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sign_all_groups();
                                        cx.notify();
                                    })),
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
                            )),
                    )
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
                        "配置目录：{}",
                        self.config.paths().data_dir().display()
                    ))
                    .child(format!("版本：{}", crate::VERSION));
            }
            Page::Chat => {}
        }
        root
    }

    fn pick_image(&mut self, _: &gpui::ClickEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        else {
            return;
        };
        let Ok(data) = std::fs::read(&path) else {
            self.set_error("读取图片失败");
            return;
        };
        let Some(room_id) = self.active().and_then(|bridge| bridge.selected_room_id) else {
            self.set_error("请先选择会话");
            return;
        };
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        let mime = match extension.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => "image/png",
        };
        let _ = self.send_command(IcaCommand::SendImageMessage {
            room_id,
            content: String::new(),
            reply_to: None,
            mentions: Vec::new(),
            image_type: mime.to_string(),
            image_data: data.into(),
        });
    }

    fn pick_file(&mut self, _: &gpui::ClickEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };
        let Ok(data) = std::fs::read(&path) else {
            self.set_error("读取文件失败");
            return;
        };
        let Some(room_id) = self.active().and_then(|bridge| bridge.selected_room_id) else {
            self.set_error("请先选择会话");
            return;
        };
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file")
            .to_string();
        let file_type = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin")
            .to_string();
        let _ = self.send_command(IcaCommand::SendFileMessage {
            room_id,
            content: String::new(),
            reply_to: None,
            mentions: Vec::new(),
            file_name: name,
            file_type,
            file_data: data.into(),
        });
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
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family("Noto Sans CJK SC")
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(self.page == Page::Chat, |element| {
                        element.child(self.render_room_list(cx))
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
                        .child(message),
                )
            })
    }
}
