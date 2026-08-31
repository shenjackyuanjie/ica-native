//! 关系网的数据加载：重建图谱、分批拉取群成员、统计进度与自动降级。

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::app::IcaApp;
use crate::app::state::BridgeState;
use crate::ica::IcaCommand;

use super::super::controller::RelationAction;
use super::super::model::*;
use super::super::state::RelationNetworkState;

const RELATION_MEMBER_LOAD_CONCURRENCY: usize = 12;
const RELATION_REBUILD_GROUP_STEP: usize = 12;

impl IcaApp {
    /// 根据当前桥接状态重新构建关系网图谱。
    ///
    /// 会先同步渲染配置，再从房间列表与群成员数据组装图谱，替换旧图后应用自动降级，
    /// 并记录构建耗时用于调试。
    pub fn rebuild_relation_network(&mut self) {
        self.sync_relation_network_render_setting();
        let started_at = Instant::now();
        let include_unloaded_groups = self
            .relation_network
            .lock()
            .unwrap()
            .include_unloaded_groups;
        let Some(state) = self.active_bridge_state() else {
            self.relation_network
                .lock()
                .unwrap()
                .replace_graph(RelationGraph::default());
            return;
        };
        let login_user_id = relation_login_user_id(state);
        let group_members = state.group_members_snapshot();
        let graph = RelationGraphBuilder::build(
            login_user_id,
            &state.rooms,
            &group_members,
            include_unloaded_groups,
        );
        let node_count = graph.nodes.len();
        let link_count = graph.links.len();
        let loaded_groups = graph.loaded_group_count;
        let total_groups = graph.total_group_count;
        self.relation_network.lock().unwrap().replace_graph(graph);
        self.apply_relation_network_auto_degrade();
        tracing::debug!(
            node_count,
            link_count,
            loaded_groups,
            total_groups,
            include_unloaded_groups,
            elapsed_ms = started_at.elapsed().as_millis(),
            "关系网图谱已重建"
        );
    }

    /// 请求加载群成员列表。
    ///
    /// `limit` 为 `None` 时启动“加载全部”，按并发上限分批请求直到所有群完成；
    /// 为 `Some(n)` 时只排队最多 `n` 个尚未加载的群。
    pub fn request_relation_network_members(&mut self, limit: Option<usize>) {
        if limit.is_none() {
            let Some(bridge_idx) = self.active_bridge_idx else {
                return;
            };
            let loaded_groups = self.bridge_states[bridge_idx]
                .conversations
                .iter()
                .filter(|(room_id, conversation)| {
                    **room_id < 0 && conversation.group_members_loaded
                })
                .count();
            let total_groups = self.bridge_states[bridge_idx]
                .rooms
                .iter()
                .filter(|room| room.room_id < 0)
                .count();
            {
                let mut relation_network = self.relation_network.lock().unwrap();
                relation_network.load_all_active = true;
                relation_network.load_started_at = Some(Instant::now());
                relation_network.load_start_loaded_groups = loaded_groups;
                relation_network.load_last_rebuild_loaded_groups = loaded_groups;
            }
            tracing::debug!(
                loaded_groups,
                total_groups,
                concurrency = RELATION_MEMBER_LOAD_CONCURRENCY,
                "关系网群成员加载已开始"
            );
            self.continue_relation_network_member_loading();
            return;
        }

        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let command_tx = self.bridge_states[bridge_idx].command_sender();
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let queued = request_relation_network_members_with_tx(state, &command_tx, limit);
        tracing::debug!(queued, limit = ?limit, "关系网群成员批次已排队");
    }

    pub fn refresh_relation_network_after_bridge_update(&mut self, bridge_idx: usize) {
        if Some(bridge_idx) != self.active_bridge_idx {
            return;
        }
        let load_all_active = self.relation_network.lock().unwrap().load_all_active;
        let loaded_groups = self.bridge_states[bridge_idx]
            .conversations
            .iter()
            .filter(|(room_id, conversation)| **room_id < 0 && conversation.group_members_loaded)
            .count();
        let pending_groups = self.bridge_states[bridge_idx]
            .rooms
            .iter()
            .filter(|room| {
                room.room_id < 0
                    && !self.bridge_states[bridge_idx].group_members_loaded(room.room_id)
                    && !self.bridge_states[bridge_idx].group_members_loading(room.room_id)
            })
            .count();
        let loading_groups = self.bridge_states[bridge_idx]
            .conversations
            .iter()
            .filter(|(room_id, conversation)| **room_id < 0 && conversation.loading_group_members)
            .count();
        let last_rebuild = self
            .relation_network
            .lock()
            .unwrap()
            .load_last_rebuild_loaded_groups;
        let load_finished = pending_groups == 0 && loading_groups == 0;
        if !load_all_active
            || loaded_groups.saturating_sub(last_rebuild) >= RELATION_REBUILD_GROUP_STEP
            || load_finished
        {
            self.rebuild_relation_network();
            self.relation_network
                .lock()
                .unwrap()
                .load_last_rebuild_loaded_groups = loaded_groups;
        }
        if load_all_active {
            self.continue_relation_network_member_loading();
        }
    }

    pub(super) fn continue_relation_network_member_loading(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let command_tx = self.bridge_states[bridge_idx].command_sender();
        let relation_network_state = self.relation_network.clone();
        let state = self.bridge_states[bridge_idx].state_mut();
        let loading_groups = state
            .conversations
            .iter()
            .filter(|(room_id, conversation)| **room_id < 0 && conversation.loading_group_members)
            .count();
        let pending_groups = state
            .rooms
            .iter()
            .filter(|room| {
                room.room_id < 0
                    && !state.group_members_loaded(room.room_id)
                    && !state.group_members_loading(room.room_id)
            })
            .count();

        if pending_groups == 0 && loading_groups == 0 {
            let (started_at, start_loaded_groups) = {
                let mut relation_network = relation_network_state.lock().unwrap();
                relation_network.load_all_active = false;
                (
                    relation_network.load_started_at.take(),
                    relation_network.load_start_loaded_groups,
                )
            };
            let loaded_groups = state
                .conversations
                .iter()
                .filter(|(room_id, conversation)| {
                    **room_id < 0 && conversation.group_members_loaded
                })
                .count();
            tracing::debug!(
                loaded_groups,
                newly_loaded = loaded_groups.saturating_sub(start_loaded_groups),
                elapsed_ms = started_at.map(|started| started.elapsed().as_millis()),
                "关系网群成员加载已完成"
            );
            return;
        }

        let available_slots = RELATION_MEMBER_LOAD_CONCURRENCY.saturating_sub(loading_groups);
        if available_slots == 0 {
            return;
        }
        let queued =
            request_relation_network_members_with_tx(state, &command_tx, Some(available_slots));
        tracing::debug!(
            queued,
            loading_groups,
            pending_groups,
            "关系网群成员加载已补充"
        );
    }

    pub(super) fn apply_relation_network_auto_degrade(&mut self) {
        let mut relation_network = self.relation_network.lock().unwrap();
        let node_count = relation_network.graph.nodes.len();
        let render_setting = relation_network.render_setting.clone();
        if node_count > render_setting.auto_hide_labels_node_threshold {
            if relation_network.show_labels {
                tracing::debug!(node_count, "关系网已自动隐藏标签");
            }
            relation_network.show_labels = false;
        }
        if node_count > render_setting.auto_hide_acquaintance_node_threshold {
            if relation_network.options.show_acquaintances {
                tracing::debug!(node_count, "关系网已自动隐藏共同群好友");
            }
            relation_network.options.show_acquaintances = false;
        }
        if node_count > render_setting.auto_hide_stranger_node_threshold {
            if relation_network.options.show_strangers {
                tracing::debug!(node_count, "关系网已自动隐藏仅同群");
            }
            relation_network.options.show_strangers = false;
        }
    }
}

/// 渲染关系网界面所需的桥接状态快照。
///
/// 由主视口在每帧构建并传给子视口，包含房间数、群加载进度等只读信息，
/// 使子视口无需直接访问 `BridgeState`。
#[derive(Debug, Clone)]
pub(super) struct RelationBridgeSnapshot {
    pub(super) rooms_len: usize,
    /// 当前桥接房间列表中的群总数，不能使用上一次图谱构建时缓存的总数。
    pub(super) total_groups: usize,
    pub(super) loaded_groups: usize,
    pub(super) loading_groups: usize,
    pub(super) pending_groups: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RelationMemberLoadProgress {
    pub(super) loaded: usize,
    pub(super) total: usize,
    pub(super) ratio: f32,
}

/// 根据实时桥接快照计算群成员加载进度。
///
/// 总数为零时仍向进度条提供一个安全分母，但展示文本保留真实的 `0 / 0`；同时将
/// 已加载数量限制在总数以内，避免房间列表刚刷新、旧成员缓存尚未清理时出现超过
/// 100% 的进度值。
pub(super) fn relation_member_load_progress(
    snapshot: &RelationBridgeSnapshot,
) -> RelationMemberLoadProgress {
    let loaded = snapshot.loaded_groups.min(snapshot.total_groups);
    let ratio = if snapshot.total_groups == 0 {
        0.0
    } else {
        loaded as f32 / snapshot.total_groups as f32
    };
    RelationMemberLoadProgress {
        loaded,
        total: snapshot.total_groups,
        ratio,
    }
}

pub(super) fn queue_relation_member_requests(
    relation_network: &mut RelationNetworkState,
    limit: Option<usize>,
) {
    relation_network.pending_action = Some(RelationAction::LoadGroups(limit));
    tracing::debug!(limit = ?limit, "关系网成员加载动作已排队");
}

pub(super) fn request_relation_network_members_with_tx(
    state: &mut BridgeState,
    command_tx: &UnboundedSender<IcaCommand>,
    limit: Option<usize>,
) -> usize {
    let mut queued = 0usize;
    let group_room_ids: Vec<_> = state
        .rooms
        .iter()
        .filter(|room| room.room_id < 0)
        .map(|room| room.room_id)
        .collect();

    for room_id in group_room_ids {
        if state.group_members_loaded(room_id) || state.group_members_loading(room_id) {
            continue;
        }
        state.conversation_mut(room_id).loading_group_members = true;
        if let Err(e) = command_tx.send(IcaCommand::FetchGroupMembers { room_id }) {
            state.conversation_mut(room_id).loading_group_members = false;
            state.last_error = Some(format!("关系网群成员请求失败: {e}"));
            break;
        }

        queued += 1;
        if limit.is_some_and(|limit| queued >= limit) {
            break;
        }
    }

    if queued > 0 {
        state.last_notice = Some(format!("关系网已开始加载 {queued} 个群的成员列表"));
    } else {
        state.last_notice = Some("关系网没有需要加载的群成员列表".to_string());
    }
    queued
}

pub(super) fn relation_login_user_id(state: &BridgeState) -> Option<i64> {
    (state.online_data.qqid > 0).then_some(state.online_data.qqid)
}

#[cfg(test)]
mod tests {
    use super::{RelationBridgeSnapshot, relation_member_load_progress};

    #[test]
    fn member_load_progress_uses_live_group_total_and_clamps_loaded_count() {
        let snapshot = RelationBridgeSnapshot {
            rooms_len: 12,
            total_groups: 5,
            loaded_groups: 8,
            loading_groups: 0,
            pending_groups: 0,
        };

        let progress = relation_member_load_progress(&snapshot);

        assert_eq!(progress.loaded, 5);
        assert_eq!(progress.total, 5);
        assert_eq!(progress.ratio, 1.0);
    }

    #[test]
    fn member_load_progress_keeps_empty_total_visible_without_dividing_by_zero() {
        let snapshot = RelationBridgeSnapshot {
            rooms_len: 0,
            total_groups: 0,
            loaded_groups: 0,
            loading_groups: 0,
            pending_groups: 0,
        };

        let progress = relation_member_load_progress(&snapshot);

        assert_eq!(progress.loaded, 0);
        assert_eq!(progress.total, 0);
        assert_eq!(progress.ratio, 0.0);
    }
}
