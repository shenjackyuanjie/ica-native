//! 聊天区的用户操作动作。
//!
//! 全部方法挂在 `impl IcaApp` 上，供界面层调用：消息 +1、转发（逐条/合并）、
//! 复制到编辑区，转发目标会话的选择与提交，群分组（chat group）的增删改与
//! 向 Bridge 同步，会话置顶/移除/忽略，在线状态切换，群签到与戳一戳，以及
//! 头像 @ 提及与成员管理入口。
//!
//! 约定：所有命令统一封装为 `IcaCommand` 经 session channel 发给 IO 线程，
//! GUI 线程不直接接触 socket；发送失败时记录 `last_error` 提示，部分命令
//! 先本地乐观更新再按结果回滚（如置顶）。

#[cfg(test)]
#[path = "tests/actions.rs"]
mod tests;

use crate::app::online_mode::OnlineMode;
use crate::app::{GroupMemberFilter, IcaApp, SelectedChatGroup};
use crate::config::ChatGroups;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    message::{Message, SendMessage},
};
use serde_json::Value as JsonValue;
use std::collections::HashSet;

impl IcaApp {
    /// 从消息中提取可原样发送的原始消息链（raw chain）。
    ///
    /// 兼容 Bridge 的两种 `raw_msg` 形态：非空数组直接取用；带 `type` 的单个
    /// 对象包装为一员数组。都取不到时返回 `None`，调用方退化为文本重建发送。
    fn extract_raw_chain(message: &Message) -> Option<JsonValue> {
        let raw_msg = message.raw_msg.as_deref()?;
        match raw_msg {
            JsonValue::Array(values) if !values.is_empty() => {
                Some(JsonValue::Array(values.clone()))
            }
            JsonValue::Object(map) if map.contains_key("type") => {
                Some(JsonValue::Array(vec![raw_msg.clone()]))
            }
            _ => None,
        }
    }

    /// 把原始消息链作为 `SendRawMessage` 命令发给指定房间，返回是否发送成功。
    ///
    /// 没有活跃 bridge、channel 发送失败时返回 `false` 并记录错误；发送成功后
    /// 按设置标记该会话"发送后滚动到底部"。
    fn send_raw_chain(&mut self, room_id: RoomId, chain: JsonValue) -> bool {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };

        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::SendRawMessage {
            room_id,
            content: chain,
        }) {
            tracing::warn!(error = %e, room_id, "发送原始 sendMessage 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("原样发送命令发送失败: {}", room_id));
            }
            return false;
        }

        if self.scroll_to_bottom_after_send
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state
                .conversation_mut(room_id)
                .pending_send_scroll_to_bottom = true;
        }
        true
    }

    /// 从当前活跃 bridge 的会话缓存中按消息 id 克隆一条消息。
    ///
    /// 找不到 bridge 或消息时返回 `None`；克隆避免调用方持有会话缓存的引用。
    fn clone_message_from_active_bridge(
        &self,
        room_id: RoomId,
        message_id: &str,
    ) -> Option<Message> {
        let bridge_idx = self.active_bridge_idx?;
        self.bridge_states
            .get(bridge_idx)?
            .find_message(room_id, message_id)
            .cloned()
    }

    /// 取出指定 bridge/房间下被选为转发的消息，按缓存中的时间顺序返回。
    ///
    /// 房间与当前转发来源不一致时返回空列表；选中 id 收集为 `HashSet` 判定。
    pub fn selected_forward_messages(&self, bridge_idx: usize, room_id: RoomId) -> Vec<Message> {
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
            .conversation(room_id)
            .map(|conversation| &conversation.messages)
            .into_iter()
            .flatten()
            .filter(|message| selected_ids.contains(message.msg_id.as_str()))
            .cloned()
            .collect()
    }

    /// 把一条已有消息克隆发送到目标房间，返回是否发送成功。
    ///
    /// 优先原样发送原始消息链；取不到 raw chain 时退化为按文本重建（保留
    /// 引用），附件会被丢弃并提示"已退化为纯文本发送"。空文本且无原始节点
    /// 时直接失败。
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

        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::SendMessage(outgoing)) {
            tracing::warn!(error = %e, room_id = target_room_id, "发送克隆消息的 sendMessage 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some(format!("消息发送失败: {}", target_room_id));
            }
            return false;
        }

        if !message.files.is_empty()
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.last_error = Some("部分附件消息缺少原始节点，已退化为纯文本发送".to_string());
        }

        if self.scroll_to_bottom_after_send
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state
                .conversation_mut(target_room_id)
                .pending_send_scroll_to_bottom = true;
        }
        true
    }

    /// 把一条消息的内容与引用复制到当前会话的编辑区（草稿）。
    ///
    /// 附件不恢复并给出提示；正在"编辑重发提交中"时忽略，避免覆盖待撤回的草稿。
    pub fn copy_message_to_draft(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };

        if let Some(state) = self.active_bridge_state_mut() {
            let conversation = state.conversation_mut(room_id);
            if conversation.editing_send_pending {
                return;
            }
            conversation.draft = message.content.clone();
            if let Some(reply) = message.reply.clone() {
                conversation.reply_to = Some(reply);
            } else {
                conversation.reply_to = None;
            }
            if !message.files.is_empty() {
                state.last_error =
                    Some("复制到编辑区暂不恢复附件，如需原样发送请使用 +1 或 转发".to_string());
            }
        }
    }

    /// 对一条消息执行 +1（原样再发一遍到所在会话）。
    ///
    /// 取不到消息时静默返回；发送失败的错误由 `send_message_clone_to_room` 记录。
    pub fn plus_one_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };
        let _ = self.send_message_clone_to_room(room_id, &message);
    }

    /// 开始一条消息的转发选择，并可选直接打开目标会话选择器。
    ///
    /// 替换（不是追加）当前转发选择；打开选择器时清空搜索词与已选目标。
    pub fn begin_forward_selection(
        &mut self,
        room_id: RoomId,
        message_id: String,
        open_picker: bool,
    ) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.replace_forward_selection(room_id, message_id);
            state.forward_target_as_merged = true;
            state.forward_target_picker_open = open_picker;
            if open_picker {
                state.forward_target_search_query.clear();
                state.forward_target_room_ids.clear();
            }
        }
    }

    /// 切换一条消息在转发选择中的选中状态。
    pub fn toggle_forward_message_selection(&mut self, room_id: RoomId, message_id: String) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.toggle_forward_selection(room_id, message_id);
        }
    }

    /// 把转发选择中的消息逐条 +1 到当前会话；部分失败时提示条数。
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

        if failed > 0
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.last_error = Some(format!("有 {} 条消息无法完整 +1", failed));
        }
    }

    /// 打开转发目标选择器（逐条转发模式）。
    pub fn open_forward_target_picker(&mut self, room_id: RoomId) {
        self.open_forward_target_picker_with_mode(room_id, false);
    }

    /// 打开转发目标选择器并指定合并/逐条模式；仅在转发选择进行中有效。
    ///
    /// 打开时清空搜索词与已选目标，避免沿用上次会话的选择。
    pub fn open_forward_target_picker_with_mode(&mut self, room_id: RoomId, merged: bool) {
        if let Some(state) = self.active_bridge_state_mut()
            && state.is_forward_selection_active(room_id)
        {
            state.forward_target_as_merged = merged;
            state.forward_target_picker_open = true;
            state.forward_target_search_query.clear();
            state.forward_target_room_ids.clear();
        }
    }

    /// 清空当前转发选择与目标选择状态。
    pub fn clear_forward_selection(&mut self) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.clear_forward_selection();
        }
    }

    /// 把当前选中的消息转发到一批目标会话。
    ///
    /// 目标列表先去重；合并转发模式下逐目标提交一条合并消息，全部成功才清空
    /// 选择；逐条模式下按"目标 × 消息"两层循环发送，任一目标有成功即清空
    /// 选择并提示汇总，失败数记入 `last_error`。
    pub fn forward_selected_messages_to_rooms(&mut self, target_room_ids: Vec<RoomId>) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(source_room_id) = self.bridge_states[bridge_idx].forward_room_id else {
            return;
        };
        let mut targets = Vec::new();
        for room_id in target_room_ids {
            if !targets.contains(&room_id) {
                targets.push(room_id);
            }
        }
        if targets.is_empty() {
            return;
        }

        if self.bridge_states[bridge_idx].forward_target_as_merged {
            let mut sent_targets = 0_usize;
            for target_room_id in &targets {
                if self.send_selected_messages_as_merged_forward(*target_room_id) {
                    sent_targets += 1;
                }
            }
            let failed_targets = targets.len() - sent_targets;
            if failed_targets > 0 {
                self.bridge_states[bridge_idx].last_error =
                    Some(format!("有 {failed_targets} 个目标未能提交合并转发"));
            }
            if sent_targets > 0 {
                self.bridge_states[bridge_idx].clear_forward_selection();
            }
            return;
        }
        let messages = self.selected_forward_messages(bridge_idx, source_room_id);
        if messages.is_empty() {
            self.bridge_states[bridge_idx].clear_forward_selection();
            return;
        }

        let mut sent_targets = 0_usize;
        let mut failed_messages = 0_usize;
        for target_room_id in &targets {
            let mut target_sent = false;
            for message in &messages {
                if self.send_message_clone_to_room(*target_room_id, message) {
                    target_sent = true;
                } else {
                    failed_messages += 1;
                }
            }
            if target_sent {
                sent_targets += 1;
            }
        }

        if failed_messages > 0 {
            self.bridge_states[bridge_idx].last_error =
                Some(format!("有 {failed_messages} 条目标消息无法完整转发"));
        }
        if sent_targets > 0 {
            self.bridge_states[bridge_idx].last_notice = Some(format!(
                "已向 {sent_targets} 个会话提交逐条转发，共 {} 条消息",
                messages.len()
            ));
            self.bridge_states[bridge_idx].clear_forward_selection();
        }
    }

    /// 通知 Bridge 新增一个群分组；`include_all_personal` 表示自动包含全部私聊。
    pub fn send_add_chat_group(
        &self,
        bridge_idx: usize,
        name: &str,
        rooms: &[RoomId],
        include_all_personal: bool,
    ) {
        if let Some(session) = self.bridge_states.get(bridge_idx) {
            let _ = session.send(IcaCommand::AddChatGroup {
                name: name.to_string(),
                rooms: rooms.to_vec(),
                include_all_personal,
            });
        }
    }

    /// 通知 Bridge 删除指定名称的群分组。
    pub fn send_remove_chat_group(&self, bridge_idx: usize, name: &str) {
        if let Some(session) = self.bridge_states.get(bridge_idx) {
            let _ = session.send(IcaCommand::RemoveChatGroup {
                name: name.to_string(),
            });
        }
    }

    /// 通知 Bridge 更新指定群分组的房间与私聊包含范围。
    pub fn send_update_chat_group(
        &self,
        bridge_idx: usize,
        name: &str,
        rooms: &[RoomId],
        include_all_personal: bool,
    ) {
        if let Some(session) = self.bridge_states.get(bridge_idx) {
            let _ = session.send(IcaCommand::UpdateChatGroup {
                name: name.to_string(),
                rooms: rooms.to_vec(),
                include_all_personal,
            });
        }
    }

    /// 把 bridge 侧的群分组与旧配置对比，向 IO 线程发送增/删/改命令。
    ///
    /// 按名称匹配：同名但房间或"包含全部私聊"变化的发更新，旧有新无的删除，
    /// 新有旧无的新增；只发命令，不等待回执。
    pub fn sync_chat_groups_to_bridge(&self, bridge_idx: usize, old: &ChatGroups) {
        let Some(new) = self
            .bridge_states
            .get(bridge_idx)
            .map(|session| &session.chat_groups)
        else {
            return;
        };

        for old_group in &old.groups {
            if let Some(new_group) = new.groups.iter().find(|g| g.name == old_group.name) {
                if new_group.rooms != old_group.rooms
                    || new_group.include_all_personal != old_group.include_all_personal
                {
                    self.send_update_chat_group(
                        bridge_idx,
                        &new_group.name,
                        &new_group.rooms,
                        new_group.include_all_personal,
                    );
                }
            } else {
                self.send_remove_chat_group(bridge_idx, &old_group.name);
            }
        }

        for new_group in &new.groups {
            if !old.groups.iter().any(|g| g.name == new_group.name) {
                self.send_add_chat_group(
                    bridge_idx,
                    &new_group.name,
                    &new_group.rooms,
                    new_group.include_all_personal,
                );
            }
        }
    }

    /// 设置/取消会话置顶；先本地乐观更新房间排序，命令失败时回滚。
    ///
    /// 回滚按发送前记下的 `index` 恢复，并刷新房间 revision 让列表重排。
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
        state.bump_rooms_revision();

        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::PinRoom { room_id, pin }) {
            tracing::warn!(error = %e, room_id, "发送 pinRoom 命令失败");
            if let Some(room) = self.bridge_states[bridge_idx]
                .rooms
                .iter_mut()
                .find(|room| room.room_id == room_id)
            {
                room.index = previous_index;
            }
            self.bridge_states[bridge_idx].bump_rooms_revision();
            self.bridge_states[bridge_idx].last_error =
                Some(format!("置顶命令发送失败: {}", room_id));
        }
    }

    /// 从本地房间列表移除会话并通知 Bridge；被移除的恰好是当前会话时清空选中。
    pub fn remove_chat(&mut self, bridge_idx: usize, room_id: RoomId) {
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        state.rooms.retain(|room| room.room_id != room_id);
        if state.selected_room_id == Some(room_id) {
            state.selected_room_id = None;
        }
        state.bump_rooms_revision();
        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::RemoveChat(room_id)) {
            tracing::warn!(error = %e, room_id, "发送 removeChat 命令失败");
        }
    }

    /// 把会话加入忽略列表（Bridge 侧不再推送该会话的新消息）。
    pub fn ignore_chat(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Err(e) =
            self.bridge_states[bridge_idx].send(IcaCommand::IgnoreChat { room_id, room_name })
        {
            tracing::warn!(error = %e, room_id, "发送 ignoreChat 命令失败");
        }
    }

    /// 把当前在线状态模式换算为 Bridge 的状态码并发送。
    ///
    /// 在线/离开/隐身/忙碌/求戳/请勿打扰分别映射到固定数值；成功发 Notice，
    /// 失败记 `last_error`。
    pub fn apply_online_status(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let status = match self.online_mode {
            OnlineMode::Online => 11,
            OnlineMode::Left => 31,
            OnlineMode::Hidden => 41,
            OnlineMode::Busy => 50,
            OnlineMode::PingMe => 60,
            OnlineMode::DoNotDisturb => 70,
        };

        let online_mode = self.online_mode;
        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::SetOnlineStatus(status)) {
            tracing::warn!(error = %e, status, "发送 setOnlineStatus 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("在线状态命令发送失败".to_string());
            }
        } else if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.last_notice = Some(format!("已请求切换在线状态为 {}", online_mode));
        }
    }

    /// 对当前群发起签到命令。
    pub fn send_group_sign(&mut self, room_id: RoomId) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Err(e) = self.bridge_states[bridge_idx].send(IcaCommand::SendGroupSign { room_id }) {
            tracing::warn!(error = %e, room_id, "发送 sendGroupSign 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("群签到命令发送失败".to_string());
            }
        }
    }

    /// 对群成员发起戳一戳；目标 id 非法时直接提示，不发命令。
    pub fn send_group_poke(&mut self, room_id: RoomId, target_id: i64) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if crate::ica::client::poke_target(room_id, target_id).is_none() {
            self.bridge_states[bridge_idx].last_error = Some("戳一戳目标无效".to_string());
            return;
        }
        if let Err(e) =
            self.bridge_states[bridge_idx].send(IcaCommand::SendGroupPoke { room_id, target_id })
        {
            tracing::warn!(error = %e, room_id, target_id, "发送 sendGroupPoke 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("戳一戳命令发送失败".to_string());
            }
        }
    }

    /// 在当前会话的保存光标处插入真实提及，保留草稿、附件与回复。
    pub fn mention_avatar_sender(
        &mut self,
        ctx: &egui::Context,
        bridge_idx: usize,
        room_id: RoomId,
        target_id: i64,
        name: String,
    ) {
        if bridge_idx >= self.bridge_states.len() || room_id >= 0 || target_id <= 0 {
            return;
        }
        if self.active_bridge_idx != Some(bridge_idx) {
            self.switch_active_bridge(bridge_idx);
        }
        self.select_active_room(room_id);
        let name: String = name
            .chars()
            .filter(|ch| !matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .take(48)
            .collect();
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = if name.is_empty() {
            target_id.to_string()
        } else {
            name
        };
        let visible_text = format!("@{name}");
        let composer_id = egui::Id::new(("message_composer", bridge_idx, room_id));
        let conversation = self.bridge_states[bridge_idx].conversation_mut(room_id);
        if conversation.editing_send_pending {
            return;
        }
        super::insert_mention_at_saved_cursor(
            ctx,
            composer_id,
            &mut conversation.draft,
            &format!("{visible_text} "),
            false,
        );
        if !conversation
            .mentions
            .iter()
            .any(|mention| mention.user_id == target_id && mention.text == visible_text)
        {
            conversation
                .mentions
                .push(crate::ica::types::message::Mention {
                    user_id: target_id,
                    text: visible_text,
                });
        }
        self.show_mention_picker = false;
        self.mention_search_query.clear();
        self.mention_search_focus_requested = false;
        self.mention_replace_trigger = false;
        self.mention_selected_index = 0;
        ctx.memory_mut(|memory| memory.request_focus(composer_id));
    }

    /// 复用有权限检查和二次确认的成员面板，不直接绕过检查发送管理命令。
    pub fn open_avatar_member_management(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        target_id: i64,
    ) {
        if bridge_idx >= self.bridge_states.len() || room_id >= 0 || target_id <= 0 {
            return;
        }
        if self.active_bridge_idx != Some(bridge_idx) {
            self.switch_active_bridge(bridge_idx);
        }
        self.select_active_room(room_id);
        self.group_member_panel.open = true;
        self.group_member_panel.search_query = target_id.to_string();
        self.group_member_panel.filter = GroupMemberFilter::All;
        self.group_member_panel.confirmation = None;
        self.group_member_panel.error = None;
        self.request_group_members(bridge_idx, room_id, true);
    }

    /// 校验当前选中的自定义分组下标仍然有效，越界时回退为"所有聊天"。
    ///
    /// 分组被删除后、界面刷新前调用，防止筛选条件指向不存在的分组。
    pub fn ensure_selected_chat_group_valid(&mut self) {
        if let Some(state) = self.active_bridge_state_mut()
            && let SelectedChatGroup::Custom(idx) = &state.selected_chat_group
            && *idx >= state.chat_groups.groups.len()
        {
            state.selected_chat_group = SelectedChatGroup::All;
        }
    }
}
