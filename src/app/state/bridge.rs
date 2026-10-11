//! 单个 bridge 在 GUI 侧维护的完整状态。
//!
//! 一个 bridge 对应一条到 QQ 的 Socket.IO 连接。这里把连接状态、房间列表、
//! 各会话的消息缓存、转发/公告/搜索等附属窗口状态收拢到同一个 `BridgeState`，
//! 这样在多个 bridge 之间切换时不需要把全局状态来回覆写，独立窗口也能按
//! bridge key 稳定找到自己要显示的数据。
//!
//! 内存控制是本模块的关键设计：消息缓存按“活跃会话/独立窗口会话/后台会话”
//! 三档限流，布局缓存只保留活跃会话，超限时从旧到新裁剪并让对应会话重新
//! 拉取历史，避免长时间运行后内存无限增长。

#[cfg(test)]
#[path = "tests/bridge.rs"]
mod tests;

use super::{
    ConversationState, ForwardViewerState, GroupAnnouncementViewerState, GroupMember,
    MemberHistoryState, MessageSearchState, RoomDirectory,
};
use crate::app::SelectedChatGroup;
use crate::app::contacts::ContactDirectory;
use crate::config::ChatGroups;
use crate::ica::types::{RoomId, message::Message, room::JoinRequestRoom};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

/// 左侧会话列表“当前过滤条件下可见房间”的缓存。
///
/// 列表过滤涉及搜索词、聊天分组与置顶排序，逐帧重算代价高，因此把结果按
/// 输入条件缓存；`revision` 或查询条件变化时由 `BridgeState` 主动失效。
#[derive(Debug, Clone)]
pub struct VisibleRoomIndicesCache {
    /// 生成缓存时的房间列表修订号，见 `RoomDirectory::rooms_revision`。
    pub revision: u64,
    /// 生成缓存时的会话搜索词。
    pub query: String,
    /// 生成缓存时选中的聊天分组。
    pub selected_chat_group: SelectedChatGroup,
    /// 生成缓存时“禁用聊天分组”的外观设置。
    pub disable_chat_group: bool,
    /// 可见房间在 `RoomDirectory::rooms` 中的下标，已按展示顺序排好。
    pub indices: Vec<usize>,
}

/// Bridge 数据库升级（例如重建消息搜索索引）的当前进度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseUpgradeProgress {
    /// 是否有升级任务正在进行。
    pub active: bool,
    /// 已完成的步骤数。
    pub step: u64,
    /// 总步骤数；为 0 表示总量未知。
    pub total: u64,
    /// 当前步骤的可读描述，直接展示给用户。
    pub message: String,
}

impl DatabaseUpgradeProgress {
    /// 返回 0 到 1 之间的进度；未知总量时保持在 0，交给界面以不定进度展示。
    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.step as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone)]
/// 单个 bridge 在 GUI 侧维护的完整状态。
///
/// 这里既存连接状态，也存房间列表、消息缓存、验证消息和草稿，
/// 这样切换 bridge 时不需要把全局状态来回覆写。
pub struct BridgeState {
    /// 该 bridge 的房间目录、连接状态与验证消息。
    pub directory: RoomDirectory,
    /// 每个会话一份的可变状态（消息、草稿、滚动与布局缓存），按需惰性创建。
    pub conversations: HashMap<RoomId, ConversationState>,
    /// 当前选中并展示在主区域的会话；`None` 表示尚未选择。
    pub selected_room_id: Option<RoomId>,
    /// 已被拆到独立窗口的会话；这些会话即使不在前台也保留完整消息缓存。
    pub detached_room_ids: std::collections::HashSet<RoomId>,
    /// 合并转发的来源会话。
    pub forward_room_id: Option<RoomId>,
    /// 已选中待转发的消息 id 列表。
    pub forward_selected_message_ids: Vec<String>,
    /// 转发目标选择窗口是否打开。
    pub forward_target_picker_open: bool,
    /// 转发目标选择窗口内的搜索词。
    pub forward_target_search_query: String,
    /// 已勾选的转发目标会话。
    pub forward_target_room_ids: Vec<RoomId>,
    /// 多条消息的发送方式：`true` 合并转发，`false` 逐条转发。
    pub forward_target_as_merged: bool,
    /// 合并转发查看器状态；与转发查看窗口共享。
    pub forward_viewer: Arc<Mutex<ForwardViewerState>>,
    /// 当前 bridge 的群公告窗口状态。
    pub group_announcement_viewer: Arc<Mutex<GroupAnnouncementViewerState>>,
    /// 会话列表搜索词。
    pub room_search_query: String,
    /// 从当前 bridge 获取、用于发起新会话的好友和群列表。
    pub contacts: Arc<Mutex<ContactDirectory>>,
    /// 当前 bridge 的聊天记录搜索窗口状态。
    pub message_search: MessageSearchState,
    /// 当前 bridge 的成员发言记录窗口状态。
    pub member_history: MemberHistoryState,
    /// Bridge 数据库升级进度；非活跃时保留最后一次完成状态，界面不显示横幅。
    pub db_upgrade_progress: DatabaseUpgradeProgress,
}

/// 让 `BridgeState` 可以直接访问房间目录字段（`rooms`、`connection` 等），
/// 因为房间目录才是界面上“这个 bridge”的主体视图。
impl Deref for BridgeState {
    type Target = RoomDirectory;

    fn deref(&self) -> &Self::Target {
        &self.directory
    }
}

impl DerefMut for BridgeState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.directory
    }
}

impl BridgeState {
    /// 活跃会话（当前选中或独立窗口中）保留的消息条数上限。
    const ACTIVE_ROOM_MESSAGE_LIMIT: usize = 5_000;
    /// 后台会话保留的消息条数上限；只够展示预览与少量滚动。
    const BACKGROUND_ROOM_MESSAGE_LIMIT: usize = 300;
    /// 聊天记录搜索结果保留的条数上限。
    const SEARCH_MESSAGE_LIMIT: usize = 200;

    /// 用 bridge key 与聊天分组配置创建一份空状态；连接从“连接中”开始。
    pub fn new(bridge_key: String, chat_groups: ChatGroups) -> Self {
        Self {
            directory: RoomDirectory::new(bridge_key, chat_groups),
            conversations: HashMap::new(),
            selected_room_id: None,
            detached_room_ids: Default::default(),
            forward_room_id: None,
            forward_selected_message_ids: Vec::new(),
            forward_target_picker_open: false,
            forward_target_search_query: String::new(),
            forward_target_room_ids: Vec::new(),
            forward_target_as_merged: true,
            forward_viewer: Arc::new(Mutex::new(ForwardViewerState::default())),
            group_announcement_viewer: Arc::new(
                Mutex::new(GroupAnnouncementViewerState::default()),
            ),
            room_search_query: String::new(),
            contacts: Arc::new(Mutex::new(ContactDirectory::default())),
            message_search: MessageSearchState::default(),
            member_history: MemberHistoryState::default(),
            db_upgrade_progress: DatabaseUpgradeProgress::default(),
        }
    }

    /// 按房间号取会话状态的只读引用；从未打开过的房间返回 `None`。
    pub fn conversation(&self, room_id: RoomId) -> Option<&ConversationState> {
        self.conversations.get(&room_id)
    }

    /// 按房间号取会话状态的可变引用；不存在则插入默认值。
    pub fn conversation_mut(&mut self, room_id: RoomId) -> &mut ConversationState {
        self.conversations.entry(room_id).or_default()
    }

    /// 导出所有“已加载群成员”的群成员快照，供关系网等独立窗口使用。
    pub fn group_members_snapshot(&self) -> HashMap<RoomId, Vec<GroupMember>> {
        self.conversations
            .iter()
            .filter(|(room_id, conversation)| **room_id < 0 && conversation.group_members_loaded)
            .map(|(room_id, conversation)| (*room_id, conversation.group_members.clone()))
            .collect()
    }

    /// 该群的成员列表是否已加载完成。
    pub fn group_members_loaded(&self, room_id: RoomId) -> bool {
        self.conversation(room_id)
            .is_some_and(|conversation| conversation.group_members_loaded)
    }

    /// 该群的成员列表是否正在加载（避免重复发起请求）。
    pub fn group_members_loading(&self, room_id: RoomId) -> bool {
        self.conversation(room_id)
            .is_some_and(|conversation| conversation.loading_group_members)
    }

    /// 房间列表发生变化：递增修订号并使可见房间缓存失效。
    pub fn bump_rooms_revision(&mut self) {
        self.rooms_revision = self.rooms_revision.wrapping_add(1);
        if self.rooms_revision == 0 {
            self.rooms_revision = 1;
        }
        self.visible_room_indices_cache = None;
    }

    /// 仅使可见房间缓存失效（搜索词、分组等过滤条件变化时使用）。
    pub fn invalidate_visible_room_indices(&mut self) {
        self.visible_room_indices_cache = None;
    }

    /// 丢弃某个会话的消息行布局缓存（宽度、外观等布局键变化时使用）。
    pub fn invalidate_message_layout(&mut self, room_id: RoomId) {
        if let Some(conversation) = self.conversations.get_mut(&room_id) {
            conversation.message_row_heights.clear();
            conversation.message_row_layouts.clear();
            conversation.message_layout_cache_key = None;
            conversation.last_content_height = None;
        }
    }

    /// 把某会话的消息裁剪到 `limit` 条，从最旧的一端删除。
    ///
    /// 返回是否真的发生了裁剪。删除后布局缓存失效，并清除“没有更多历史”
    /// 标记，让用户继续上滑时可以重新拉取更早的消息。
    fn trim_room_messages_to_limit(&mut self, room_id: RoomId, limit: usize) -> bool {
        let Some(conversation) = self.conversations.get_mut(&room_id) else {
            return false;
        };
        let messages = &mut conversation.messages;
        if messages.len() <= limit {
            return false;
        }

        let remove_count = messages.len() - limit;
        messages.drain(..remove_count);
        self.invalidate_message_layout(room_id);
        self.conversation_mut(room_id).no_more_history = false;
        true
    }

    /// 与 `trim_room_messages_to_limit` 相同，但保留最旧的消息、删除最新的一端。
    ///
    /// 用于“向前插入历史”之后的裁剪：用户正看着旧消息，必须保证视口内的
    /// 内容不被抽走，否则会出现滚动位置跳动。
    fn trim_room_messages_to_limit_keep_oldest(&mut self, room_id: RoomId, limit: usize) -> bool {
        let Some(conversation) = self.conversations.get_mut(&room_id) else {
            return false;
        };
        let messages = &mut conversation.messages;
        if messages.len() <= limit {
            return false;
        }

        messages.truncate(limit);
        self.invalidate_message_layout(room_id);
        self.conversation_mut(room_id).no_more_history = false;
        true
    }

    /// 向前插入历史消息后的统一裁剪入口。
    pub fn trim_after_history_prepend(&mut self, room_id: RoomId) {
        self.trim_room_messages_to_limit_keep_oldest(room_id, Self::ACTIVE_ROOM_MESSAGE_LIMIT);
        self.trim_layout_caches_to_active_room(self.selected_room_id);
        self.trim_message_search_results();
    }

    /// 全量裁剪所有会话的消息缓存与布局缓存。
    ///
    /// `active_room_id` 为当前前台会话；它和独立窗口中的会话按活跃上限保留，
    /// 其余会话按后台上限保留。定时调用以控制内存占用。
    pub fn trim_message_caches(&mut self, active_room_id: Option<RoomId>) {
        let room_ids = self.conversations.keys().copied().collect::<Vec<_>>();
        for room_id in room_ids {
            let limit =
                if Some(room_id) == active_room_id || self.detached_room_ids.contains(&room_id) {
                    Self::ACTIVE_ROOM_MESSAGE_LIMIT
                } else {
                    Self::BACKGROUND_ROOM_MESSAGE_LIMIT
                };
            self.trim_room_messages_to_limit(room_id, limit);
        }
        self.trim_layout_caches_to_active_room(active_room_id);
        self.trim_message_search_results();
    }

    /// 丢弃非活跃会话的消息行布局缓存，只保留活跃与独立窗口会话。
    pub fn trim_layout_caches_to_active_room(&mut self, active_room_id: Option<RoomId>) {
        for (room_id, conversation) in &mut self.conversations {
            if Some(*room_id) != active_room_id && !self.detached_room_ids.contains(room_id) {
                conversation.message_row_heights.clear();
                conversation.message_row_layouts.clear();
                conversation.message_layout_cache_key = None;
                conversation.last_content_height = None;
            }
        }
    }

    /// 把聊天记录搜索结果裁剪到 `SEARCH_MESSAGE_LIMIT` 条，从最旧的一端删除。
    pub fn trim_message_search_results(&mut self) {
        if self.message_search.messages.len() <= Self::SEARCH_MESSAGE_LIMIT {
            return;
        }
        let remove_count = self.message_search.messages.len() - Self::SEARCH_MESSAGE_LIMIT;
        self.message_search.messages.drain(..remove_count);
    }

    /// 消息列表顺序变化（新消息、撤回等）后丢弃整段行布局，下次重排。
    pub fn invalidate_message_rows(&mut self, room_id: RoomId) {
        if let Some(conversation) = self.conversations.get_mut(&room_id) {
            conversation.message_row_layouts.clear();
        }
    }

    /// 单条消息内容变化：删除其高度缓存，并让所有会话重排行布局。
    pub fn invalidate_message_height(&mut self, msg_id: &str) {
        for conversation in self.conversations.values_mut() {
            conversation.message_row_heights.remove(msg_id);
            conversation.message_row_layouts.clear();
        }
    }

    /// 生成会话列表预览文本：撤回、纯附件、系统消息都要有可读占位。
    fn preview_content(message: &Message) -> String {
        if message.deleted {
            "[已撤回]".to_string()
        } else if !message.content.is_empty() {
            message.content.clone()
        } else if !message.files.is_empty() {
            format!("[{} 个文件]", message.files.len())
        } else if message.system {
            "[系统消息]".to_string()
        } else {
            "[空消息]".to_string()
        }
    }

    /// 用最新一条消息同步会话列表上的预览、时间与排序字段。
    pub fn sync_room_preview(&mut self, room_id: RoomId, message: &Message) {
        let Some(room) = self.rooms.iter_mut().find(|room| room.room_id == room_id) else {
            return;
        };

        room.last_message.content = Some(Self::preview_content(message));
        room.last_message.username = Some(message.sender_name.clone());
        room.last_message.user_id = Some(message.sender_id);
        room.last_message.timestamp = Some(message.time_text.clone());
        room.utime = message.time.timestamp_millis();
        self.bump_rooms_revision();
    }

    /// 插入或更新一条消息。
    ///
    /// 已存在相同 `msg_id` 时原地替换（返回 `false`），否则追加（返回 `true`）；
    /// 两种情况都会让该消息的高度缓存与行布局失效，并按活跃上限裁剪。
    pub fn upsert_message(&mut self, room_id: RoomId, message: Message) -> bool {
        let msg_id = message.msg_id.clone();
        let conversation = self.conversations.entry(room_id).or_default();
        let messages = &mut conversation.messages;
        let inserted = if let Some(existing) = messages
            .iter_mut()
            .find(|item| item.msg_id == message.msg_id)
        {
            *existing = message;
            false
        } else {
            messages.push(message);
            true
        };
        conversation.message_row_heights.remove(&msg_id);
        self.invalidate_message_rows(room_id);
        self.trim_room_messages_to_limit(room_id, Self::ACTIVE_ROOM_MESSAGE_LIMIT);
        inserted
    }

    /// 按消息 id 标记撤回（任意会话中命中即生效），并刷新其高度缓存。
    pub fn mark_message_deleted(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.deleted = true;
                message.reveal = false;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    /// 按消息 id 标记隐藏（“不感兴趣”），并刷新其高度缓存。
    pub fn mark_message_hidden(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = true;
                message.reveal = false;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    /// 按消息 id 取消隐藏并标记为主动展开，刷新其高度缓存。
    pub fn mark_message_revealed(&mut self, msg_id: &str) {
        let mut changed = false;
        for conversation in self.conversations.values_mut() {
            let messages = &mut conversation.messages;
            if let Some(message) = messages.iter_mut().find(|item| item.msg_id == msg_id) {
                message.hide = false;
                message.reveal = true;
                changed = true;
                break;
            }
        }
        if changed {
            self.invalidate_message_height(msg_id);
        }
    }

    /// 插入或更新一条加群/好友验证消息，相同 `flag` 视为同一条。
    ///
    /// 列表按时间倒序保持，最新请求排在最前。
    pub fn upsert_join_request(&mut self, request: JoinRequestRoom) {
        if let Some(existing) = self
            .join_requests
            .iter_mut()
            .find(|item| item.flag == request.flag)
        {
            *existing = request;
        } else {
            self.join_requests.insert(0, request);
        }
        self.join_requests
            .sort_by_key(|request| std::cmp::Reverse(request.time));
    }

    /// 用一整批验证消息替换现有列表（全量刷新），同样按时间倒序。
    pub fn replace_join_requests(&mut self, mut requests: Vec<JoinRequestRoom>) {
        requests.sort_by_key(|request| std::cmp::Reverse(request.time));
        self.join_requests = requests;
    }

    /// 在指定会话中按消息 id 查找消息（右键菜单、定位等场景）。
    pub fn find_message(&self, room_id: RoomId, message_id: &str) -> Option<&Message> {
        self.conversations
            .get(&room_id)?
            .messages
            .iter()
            .find(|message| message.msg_id == message_id)
    }

    /// 该会话当前是否处于“多选转发”模式。
    pub fn is_forward_selection_active(&self, room_id: RoomId) -> bool {
        self.forward_room_id == Some(room_id) && !self.forward_selected_message_ids.is_empty()
    }

    /// 某条消息是否被勾选为待转发。
    pub fn is_forward_selected(&self, room_id: RoomId, message_id: &str) -> bool {
        self.is_forward_selection_active(room_id)
            && self
                .forward_selected_message_ids
                .iter()
                .any(|selected_id| selected_id == message_id)
    }

    /// 退出多选转发模式，清空来源、勾选与目标选择器的全部中间状态。
    pub fn clear_forward_selection(&mut self) {
        self.forward_room_id = None;
        self.forward_selected_message_ids.clear();
        self.forward_target_picker_open = false;
        self.forward_target_search_query.clear();
        self.forward_target_room_ids.clear();
        self.forward_target_as_merged = true;
    }

    /// 把多选转发模式重置为“仅选中这一条”（从别的会话发起时使用）。
    pub fn replace_forward_selection(&mut self, room_id: RoomId, message_id: String) {
        self.forward_room_id = Some(room_id);
        self.forward_selected_message_ids.clear();
        self.forward_selected_message_ids.push(message_id);
        self.forward_target_room_ids.clear();
    }

    /// 勾选或取消勾选一个转发目标会话。
    pub fn set_forward_target_selected(&mut self, room_id: RoomId, selected: bool) {
        if selected {
            if !self.forward_target_room_ids.contains(&room_id) {
                self.forward_target_room_ids.push(room_id);
            }
        } else {
            self.forward_target_room_ids
                .retain(|target_room_id| *target_room_id != room_id);
        }
    }

    /// 批量添加转发目标（“全选当前结果”按钮使用）。
    pub fn add_forward_targets(&mut self, room_ids: impl IntoIterator<Item = RoomId>) {
        for room_id in room_ids {
            self.set_forward_target_selected(room_id, true);
        }
    }

    /// 在多选转发模式中切换一条消息的勾选状态。
    ///
    /// 从其他会话发起时先重置为单选；取消最后一条时自动退出多选模式。
    pub fn toggle_forward_selection(&mut self, room_id: RoomId, message_id: String) {
        if self.forward_room_id != Some(room_id) {
            self.replace_forward_selection(room_id, message_id);
            return;
        }

        if let Some(index) = self
            .forward_selected_message_ids
            .iter()
            .position(|selected_id| selected_id == &message_id)
        {
            self.forward_selected_message_ids.remove(index);
            if self.forward_selected_message_ids.is_empty() {
                self.clear_forward_selection();
            }
        } else {
            self.forward_selected_message_ids.push(message_id);
        }
    }
}
