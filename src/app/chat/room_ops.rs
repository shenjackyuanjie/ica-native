//! 会话（房间）维度的操作：切换会话、拉取历史与群成员、可见会话缓存及相关设置。
//!
//! 这里集中了“选中一个房间时发生了什么”：本地未读即时清除、漫游历史与缓存
//! 快照的请求顺序、自动已读上报，以及按需失效的可见会话索引缓存。所有请求
//! 都转成 `IcaCommand` 交给 IO 线程，本模块只维护 GUI 侧状态。

#[cfg(test)]
#[path = "tests/room_ops.rs"]
mod tests;

use crate::app::chat_shortcuts::filtered_sorted_room_indices;
use crate::app::{CompactChatPanel, IcaApp, VisibleRoomIndicesCache};
use crate::config::ReEditDraftConflictMode;
use crate::ica::IcaCommand;
use crate::ica::types::{RoomId, message::At, room::Room};

/// 判断切换会话时是否需要请求一份完整的消息快照。
///
/// 实时收到的 `addMessage` 只能证明本地缓存里存在零散消息，不能证明已经请求过该
/// 会话的历史快照。因此这个标记只应在主动发起请求或收到 `setMessages` 时维护。
fn should_request_messages_on_room_select(
    has_requested_snapshot: bool,
    room_changed: bool,
    auto_fetch_history_on_select: bool,
) -> bool {
    !has_requested_snapshot || (room_changed && auto_fetch_history_on_select)
}

/// 将刚被打开会话的本地未读状态立即清除，不依赖 bridge 稍后广播 `syncRead`。
///
/// 远端已读回执仍由调用方发送；这里仅让会话列表、分组红点和当前聊天视图立即保持一致。
fn clear_room_unread(rooms: &mut [Room], room_id: RoomId) -> bool {
    let Some(room) = rooms.iter_mut().find(|room| room.room_id == room_id) else {
        return false;
    };
    let has_unread_state = room.unread_count > 0 || matches!(room.at, At::All | At::Bool(true));
    if !has_unread_state {
        return false;
    }

    room.unread_count = 0;
    room.at = At::Bool(false);
    true
}

impl IcaApp {
    /// 请求指定群（`room_id < 0`）的成员列表。
    ///
    /// 已在加载中或已加载过时跳过（`force` 可强制刷新）；发送失败时复位加载
    /// 标志并置错误提示，避免界面永久停在“加载中”。
    pub fn request_group_members(&mut self, bridge_idx: usize, room_id: RoomId, force: bool) {
        if room_id >= 0 {
            return;
        }
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let conversation = state.conversation_mut(room_id);
        if conversation.loading_group_members || (!force && conversation.group_members_loaded) {
            return;
        }
        conversation.loading_group_members = true;
        if let Err(e) =
            self.bridge_states[bridge_idx].send(IcaCommand::FetchGroupMembers { room_id })
        {
            tracing::warn!(error = %e, room_id, "发送 FetchGroupMembers 命令失败");
            self.bridge_states[bridge_idx]
                .conversation_mut(room_id)
                .loading_group_members = false;
            self.bridge_states[bridge_idx].last_error = Some(format!("群成员列表请求失败: {e}"));
        }
    }

    /// 请求指定会话的完整消息快照；`scroll_to_bottom` 决定快照到达后是否滚到底部。
    ///
    /// 发送失败时撤销 `requested_snapshot` 占位并清滚动意图，让用户能重试。
    pub fn request_room_messages(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        scroll_to_bottom: bool,
    ) {
        let Some(command_tx) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| session.command_sender())
        else {
            return;
        };

        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            let conversation = state.conversation_mut(room_id);
            if scroll_to_bottom {
                conversation.pending_message_scroll_to_bottom = true;
            } else {
                conversation.pending_message_scroll_to_bottom = false;
                conversation.scroll_to_bottom = false;
            }
        }

        if let Err(e) = command_tx.send(IcaCommand::FetchMessages(room_id)) {
            tracing::warn!(error = %e, room_id, "发送 fetchMessages 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                // 命令没有进入后台任务时必须撤销占位，否则关闭自动刷新后，后续点击
                // 会误以为该房间已经请求过，从而失去重试机会。
                let conversation = state.conversation_mut(room_id);
                conversation.requested_snapshot = false;
                conversation.pending_message_scroll_to_bottom = false;
                state.last_error = Some(format!("历史消息请求发送失败: {e}"));
            }
        }
    }

    /// 请求协议端漫游历史（fetchLatestHistory），`current_loaded_messages` 供
    /// Bridge 判断本地缺口、避免重复下发。
    fn request_latest_room_history(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        current_loaded_messages: usize,
    ) {
        let Some(session) = self.bridge_states.get(bridge_idx) else {
            return;
        };
        if let Err(e) = session.send(IcaCommand::FetchLatestHistory {
            room_id,
            current_loaded_messages,
        }) {
            tracing::warn!(error = %e, room_id, "发送 fetchHistory 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("最新历史拉取命令发送失败: {e}"));
            }
        }
    }

    /// 向上分页加载更早的消息（fetchOlderMessages），以当前最旧消息的时间与
    /// msg_id 为游标；已在加载或没有更多历史时跳过。
    pub fn request_older_messages(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(command_tx) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| session.command_sender())
        else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let conversation = state.conversation_mut(room_id);
        if conversation.loading_older_messages || conversation.no_more_history {
            return;
        }
        let Some(oldest_message) = conversation.messages.first() else {
            conversation.no_more_history = true;
            return;
        };
        let before_time = oldest_message.time.timestamp_millis();
        let before_id = oldest_message.msg_id.clone();
        conversation.loading_older_messages = true;

        if let Err(e) = command_tx.send(IcaCommand::FetchOlderMessages {
            room_id,
            before_time,
            before_id,
        }) {
            tracing::warn!(error = %e, room_id, "发送 fetchOlderMessages 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.conversation_mut(room_id).loading_older_messages = false;
            }
        }
    }

    /// 请求当前 bridge 的系统消息（验证消息等）。
    pub fn request_system_messages(&self, bridge_idx: usize) {
        let Some(session) = self.bridge_states.get(bridge_idx) else {
            return;
        };

        if let Err(e) = session.send(IcaCommand::GetSystemMsg) {
            tracing::warn!(error = %e, "发送 getSystemMsg 命令失败");
        }
    }

    /// 返回当前 bridge 经搜索词、选中分组与分组开关筛选排序后的会话索引。
    ///
    /// 结果按 `rooms_revision`、搜索词、分组选择与分组开关缓存，任一变化才重算。
    pub fn visible_room_indices(&mut self, bridge_idx: usize) -> Vec<usize> {
        let disable_chat_group = self.custom_chat.disable_chat_group;
        let Some(current_state) = self.bridge_states.get(bridge_idx) else {
            return Vec::new();
        };
        let selected_chat_group = current_state.selected_chat_group.clone();
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return Vec::new();
        };

        if let Some(cache) = &state.visible_room_indices_cache
            && cache.revision == state.rooms_revision
            && cache.query == state.room_search_query
            && cache.selected_chat_group == selected_chat_group
            && cache.disable_chat_group == disable_chat_group
        {
            return cache.indices.clone();
        }

        let room_indices = filtered_sorted_room_indices(
            &state.rooms,
            &state.chat_groups,
            &selected_chat_group,
            disable_chat_group,
            &state.room_search_query,
        );
        state.visible_room_indices_cache = Some(VisibleRoomIndicesCache {
            revision: state.rooms_revision,
            query: state.room_search_query.clone(),
            selected_chat_group,
            disable_chat_group,
            indices: room_indices,
        });
        state
            .visible_room_indices_cache
            .as_ref()
            .map(|cache| cache.indices.clone())
            .unwrap_or_default()
    }

    /// 设置“选中会话时清空搜索词”，写入配置并使可见会话缓存失效。
    pub fn set_clear_search_on_room_select(&mut self, enabled: bool) {
        self.clear_search_on_room_select = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.clear_search_on_room_select = enabled;
        });
        if let Some(state) = self.active_bridge_state_mut() {
            state.invalidate_visible_room_indices();
        }
    }

    /// 设置“发送后滚动到底部”，写入配置。
    pub fn set_scroll_to_bottom_after_send(&mut self, enabled: bool) {
        self.scroll_to_bottom_after_send = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.scroll_to_bottom_after_send = enabled;
        });
    }

    /// 设置“切换会话时自动拉取漫游历史”，写入配置。
    pub fn set_auto_fetch_history_on_room_select(&mut self, enabled: bool) {
        self.auto_fetch_history_on_room_select = enabled;
        self.update_config(|cfg| {
            cfg.ui_setting.auto_fetch_history_on_room_select = enabled;
        });
    }

    /// 设置“编辑重发时与现有草稿冲突的处理方式”，写入配置。
    pub fn set_reedit_draft_conflict_mode(&mut self, mode: ReEditDraftConflictMode) {
        self.reedit_draft_conflict_mode = mode;
        self.update_config(|cfg| {
            cfg.ui_setting.reedit_draft_conflict_mode = mode;
        });
    }

    /// 选中指定会话，沿用“选中时清空搜索”的当前设置。
    pub fn select_active_room(&mut self, room_id: RoomId) {
        self.select_active_room_with_search_policy(room_id, self.clear_search_on_room_select);
    }

    /// 快捷键沿当前筛选结果导航，不应用鼠标选房时的“清空搜索”选项。
    pub fn select_active_room_preserving_filter(&mut self, room_id: RoomId) {
        self.select_active_room_with_search_policy(room_id, false);
    }

    /// 选中指定会话的核心流程；`clear_search_on_room_select` 区分鼠标选房与快捷键选房。
    ///
    /// 依次处理：切换会话时重置提及/成员面板等瞬态、按需清除本地未读并上报
    /// 远端已读、裁剪消息缓存、按快照请求规则决定是否拉取完整消息（开启自动
    /// 历史时先请求漫游历史再读缓存），最后按需刷新群成员。
    fn select_active_room_with_search_policy(
        &mut self,
        room_id: RoomId,
        clear_search_on_room_select: bool,
    ) {
        self.compact_chat_panel = CompactChatPanel::Chat;
        self.show_face_picker = false;
        let selected_room_changed = self
            .active_bridge_state()
            .is_none_or(|state| state.selected_room_id != Some(room_id));
        if selected_room_changed {
            self.show_mention_picker = false;
            self.mention_search_query.clear();
            self.mention_search_focus_requested = false;
            self.mention_replace_trigger = false;
            self.mention_selected_index = 0;
            self.group_member_panel.confirmation = None;
        }
        if room_id > 0 {
            self.group_member_panel.open = false;
            self.group_member_panel.confirmation = None;
        }
        let mut should_request = false;
        let mut should_clear_remote_unread = false;
        let auto_fetch_history_on_select = self.auto_fetch_history_on_room_select;
        let auto_read = self.custom_chat.auto_read_on_select;
        let mut last_msg_id: Option<String> = None;
        let mut current_loaded_messages = 0;
        if let Some(state) = self.active_bridge_state_mut() {
            let room_changed = state.selected_room_id != Some(room_id);
            state.selected_room_id = Some(room_id);
            // 仅在开启自动已读时清除本地标记并同步到 Bridge/QQ。
            if auto_read && clear_room_unread(&mut state.rooms, room_id) {
                state.bump_rooms_revision();
                should_clear_remote_unread = true;
            }
            state.trim_message_caches(Some(room_id));
            if clear_search_on_room_select {
                state.room_search_query.clear();
                state.invalidate_visible_room_indices();
            }
            // `requested_rooms` 表示已经发起过完整快照请求，而不是“缓存里碰巧有消息”。
            // 首次打开始终请求；开关启用后，切换回来也刷新。
            let conversation = state.conversation_mut(room_id);
            conversation.scroll_to_message_id = None;
            conversation.scroll_to_message_attempts = 0;
            should_request = should_request_messages_on_room_select(
                conversation.requested_snapshot,
                room_changed,
                auto_fetch_history_on_select,
            );
            if should_request {
                // 先占位可以避免响应返回前连续重绘或重复点击产生并发请求。
                conversation.requested_snapshot = true;
                current_loaded_messages = conversation.messages.len();
            }
            if auto_read {
                last_msg_id = conversation.messages.last().map(|m| m.msg_id.clone());
            }
        }

        if should_clear_remote_unread
            && let Some(bridge_idx) = self.active_bridge_idx
            && let Some(session) = self.bridge_states.get(bridge_idx)
            && let Err(e) = session.send(IcaCommand::ClearRoomUnread { room_id })
        {
            tracing::warn!(error = %e, room_id, "发送 clearRoomUnread 命令失败");
        }

        if should_request && let Some(bridge_idx) = self.active_bridge_idx {
            if auto_fetch_history_on_select {
                // 与 Icalingua++ 的 fetchHistoryOnChatOpen 行为保持一致：先请求协议端
                // 漫游历史，再读取 bridge 当前缓存。漫游拉取完成后，bridge 还会广播
                // 新的 setMessages，届时界面会自动替换为更新后的完整列表。
                self.request_latest_room_history(bridge_idx, room_id, current_loaded_messages);
            }
            self.request_room_messages(bridge_idx, room_id, true);
        }

        if auto_read
            && let Some(msg_id) = last_msg_id
            && let Some(bridge_idx) = self.active_bridge_idx
            && let Some(session) = self.bridge_states.get(bridge_idx)
        {
            let _ = session.send(IcaCommand::ReportRead {
                room_id,
                message_id: msg_id,
            });
        }

        if selected_room_changed
            && room_id < 0
            && self.group_member_panel.open
            && let Some(bridge_idx) = self.active_bridge_idx
        {
            self.request_group_members(bridge_idx, room_id, true);
        }
    }
}
