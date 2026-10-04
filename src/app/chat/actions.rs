use std::collections::HashSet;

use serde_json::Value as JsonValue;

use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    message::{Message, SendMessage},
};

use crate::config::ChatGroups;

use crate::app::online_mode::OnlineMode;
use crate::app::{GroupMemberFilter, IcaApp, SelectedChatGroup};

impl IcaApp {
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

    pub fn plus_one_message(&mut self, room_id: RoomId, message_id: String) {
        let Some(message) = self.clone_message_from_active_bridge(room_id, &message_id) else {
            return;
        };
        let _ = self.send_message_clone_to_room(room_id, &message);
    }

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

        if failed > 0
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.last_error = Some(format!("有 {} 条消息无法完整 +1", failed));
        }
    }

    pub fn open_forward_target_picker(&mut self, room_id: RoomId) {
        self.open_forward_target_picker_with_mode(room_id, false);
    }

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

    pub fn clear_forward_selection(&mut self) {
        if let Some(state) = self.active_bridge_state_mut() {
            state.clear_forward_selection();
        }
    }

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

    pub fn send_remove_chat_group(&self, bridge_idx: usize, name: &str) {
        if let Some(session) = self.bridge_states.get(bridge_idx) {
            let _ = session.send(IcaCommand::RemoveChatGroup {
                name: name.to_string(),
            });
        }
    }

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

    pub fn ignore_chat(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Err(e) =
            self.bridge_states[bridge_idx].send(IcaCommand::IgnoreChat { room_id, room_name })
        {
            tracing::warn!(error = %e, room_id, "发送 ignoreChat 命令失败");
        }
    }

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

    pub fn ensure_selected_chat_group_valid(&mut self) {
        if let Some(state) = self.active_bridge_state_mut()
            && let SelectedChatGroup::Custom(idx) = &state.selected_chat_group
            && *idx >= state.chat_groups.groups.len()
        {
            state.selected_chat_group = SelectedChatGroup::All;
        }
    }
}

#[cfg(test)]
pub mod avatar_action_tests {
    use super::*;
    use crate::app::{
        AppState, BridgeSession, BridgeState, runtime::AppRuntime, stickers::StickerStore,
    };
    use crate::config::{ConfigStore, IcaCfg};
    use crate::ica::BridgeHandle;
    use tokio::sync::{mpsc, oneshot};

    pub fn test_app() -> (IcaApp, mpsc::UnboundedReceiver<IcaCommand>) {
        let mut config: IcaCfg = toml::from_str("bridges = []").unwrap();
        config.tokio_rt_work_thread = 1;
        let store = ConfigStore::from_config(
            config.clone(),
            std::env::temp_dir().join("ica-avatar-interaction-test.toml"),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let (stop, _) = oneshot::channel();
        let mut bridge = BridgeState::new("avatar-test".into(), ChatGroups::default());
        bridge.selected_room_id = Some(-123);
        bridge.conversation_mut(-123).requested_snapshot = true;
        let state = AppState::new(
            &config,
            &store,
            vec![BridgeSession::new(
                BridgeHandle::new("avatar-test".into(), tx),
                bridge,
                stop,
            )],
            StickerStore::unavailable(
                std::env::temp_dir().join("ica-avatar-test-stickers"),
                "测试",
            ),
        );
        (
            IcaApp {
                runtime: AppRuntime::new(&egui::Context::default(), &config),
                config: store,
                state,
                chat_windows: Vec::new(),
            },
            rx,
        )
    }

    #[test]
    fn avatar_mention_replaces_unicode_selection_and_keeps_one_protocol_mention() {
        let (mut app, _rx) = test_app();
        let ctx = egui::Context::default();
        let id = egui::Id::new(("message_composer", 0_usize, -123_i64));
        app.bridge_states[0].conversation_mut(-123).draft = "甲待替换乙".into();
        let mut edit = egui::widgets::text_edit::TextEditState::default();
        edit.cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(1),
                egui::text::CCursor::new(4),
            )));
        edit.store(&ctx, id);
        app.mention_avatar_sender(&ctx, 0, -123, 456, "测试成员".into());
        assert_eq!(
            app.bridge_states[0].conversation(-123).unwrap().draft,
            "甲@测试成员 乙"
        );
        app.mention_avatar_sender(&ctx, 0, -123, 456, "测试成员".into());
        let conversation = app.bridge_states[0].conversation(-123).unwrap();
        assert_eq!(conversation.draft, "甲@测试成员 @测试成员 乙");
        assert_eq!(
            conversation.mentions,
            vec![crate::ica::types::message::Mention {
                user_id: 456,
                text: "@测试成员".into(),
            }]
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
    }

    #[test]
    fn avatar_mention_sanitizes_controls_and_rejects_private_or_virtual_targets() {
        let (mut app, _rx) = test_app();
        let ctx = egui::Context::default();
        app.mention_avatar_sender(&ctx, 0, -123, 456, "\n\u{202E}".into());
        app.mention_avatar_sender(&ctx, 0, 123, 456, "不可提及".into());
        app.mention_avatar_sender(&ctx, 0, -123, -1, "虚拟用户".into());
        let conversation = app.bridge_states[0].conversation(-123).unwrap();
        assert_eq!(conversation.draft, "@456 ");
        assert_eq!(conversation.mentions.len(), 1);
        assert_eq!(app.bridge_states[0].selected_room_id, Some(-123));
        assert!(app.bridge_states[0].conversation(123).is_none());
    }

    #[test]
    fn avatar_management_loads_the_target_group_without_sending_a_ban() {
        let (mut app, mut rx) = test_app();
        app.group_member_panel.filter = GroupMemberFilter::Muted;
        app.group_member_panel.error = Some("旧错误".into());
        app.open_avatar_member_management(0, -123, 456);
        assert!(app.group_member_panel.open);
        assert_eq!(app.group_member_panel.search_query, "456");
        assert_eq!(app.group_member_panel.filter, GroupMemberFilter::All);
        assert!(app.group_member_panel.error.is_none());
        let mut requested_members = false;
        while let Ok(command) = rx.try_recv() {
            assert!(!matches!(command, IcaCommand::SetGroupBan { .. }));
            if let IcaCommand::FetchGroupMembers { room_id } = command {
                assert_eq!(room_id, -123);
                requested_members = true;
            }
        }
        assert!(requested_members);
    }

    #[test]
    fn invalid_avatar_poke_is_not_queued() {
        let (mut app, mut rx) = test_app();
        app.send_group_poke(i64::MIN, 456);
        assert!(rx.try_recv().is_err());
        assert!(app.bridge_states[0].last_error.is_some());
    }
}
