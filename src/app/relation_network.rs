use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::ica::IcaCommand;
use crate::ica::types::{RoomId, room::Room};
use tokio::sync::mpsc::UnboundedSender;

use super::IcaApp;
use super::state::{BridgeState, GroupMember};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelationNodeKind {
    SelfUser,
    Friend,
    Acquaintance,
    Stranger,
    Group,
}

impl RelationNodeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::SelfUser => "自己",
            Self::Friend => "好友",
            Self::Acquaintance => "共同群好友",
            Self::Stranger => "仅同群",
            Self::Group => "群",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::SelfUser => egui::Color32::from_rgb(255, 215, 0),
            Self::Friend => egui::Color32::from_rgb(74, 144, 217),
            Self::Acquaintance => egui::Color32::from_rgb(135, 206, 235),
            Self::Stranger => egui::Color32::from_rgb(176, 196, 222),
            Self::Group => egui::Color32::from_rgb(231, 76, 60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RelationNode {
    pub id: String,
    pub name: String,
    pub kind: RelationNodeKind,
    pub value: usize,
    pub radius: f32,
    pub size_level: u8,
    pub qq: Option<i64>,
    pub group_id: Option<i64>,
    pub member_count: Option<usize>,
    pub common_group_count: usize,
    pub role: String,
}

impl RelationNode {
    fn new_user(user_id: i64, name: String, kind: RelationNodeKind) -> Self {
        Self {
            id: format!("u:{user_id}"),
            name,
            kind,
            value: 0,
            radius: 8.0,
            size_level: 0,
            qq: Some(user_id),
            group_id: None,
            member_count: None,
            common_group_count: 0,
            role: String::new(),
        }
    }

    fn new_group(room: &Room, member_count: Option<usize>) -> Self {
        let group_id = -room.room_id;
        Self {
            id: format!("g:{group_id}"),
            name: if room.room_name.trim().is_empty() {
                group_id.to_string()
            } else {
                room.room_name.clone()
            },
            kind: RelationNodeKind::Group,
            value: 0,
            radius: 8.0,
            size_level: 0,
            qq: None,
            group_id: Some(group_id),
            member_count,
            common_group_count: 0,
            role: String::new(),
        }
    }

    fn update_size(&mut self) {
        let min_size = 8.0_f32;
        let max_size = 44.0_f32;
        let max_value = 50_000.0_f32;
        let value = self
            .member_count
            .filter(|_| self.kind == RelationNodeKind::Group)
            .unwrap_or(self.value)
            .max(self.value) as f32;

        self.radius = if value <= 0.0 {
            min_size
        } else {
            let normalized = ((value + 1.0).ln() / (max_value + 1.0).ln()).powf(0.7);
            (min_size + (max_size - min_size) * normalized).clamp(min_size, max_size)
        };

        self.size_level = if value < 10.0 {
            0
        } else if value < 100.0 {
            1
        } else if value < 1_000.0 {
            2
        } else if value < 10_000.0 {
            3
        } else {
            4
        };
    }

    pub fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self.qq.is_some_and(|qq| qq.to_string().contains(query))
            || self
                .group_id
                .is_some_and(|group_id| group_id.to_string().contains(query))
    }
}

#[derive(Debug, Clone)]
pub struct RelationLink {
    pub source: String,
    pub target: String,
    pub group_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct RelationGraph {
    pub nodes: Vec<RelationNode>,
    pub links: Vec<RelationLink>,
    pub loaded_group_count: usize,
    pub total_group_count: usize,
}

impl RelationGraph {
    pub fn node_counts(&self) -> RelationNodeCounts {
        let mut counts = RelationNodeCounts::default();
        for node in &self.nodes {
            match node.kind {
                RelationNodeKind::SelfUser => counts.self_user += 1,
                RelationNodeKind::Friend => counts.friend += 1,
                RelationNodeKind::Acquaintance => counts.acquaintance += 1,
                RelationNodeKind::Stranger => counts.stranger += 1,
                RelationNodeKind::Group => counts.group += 1,
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RelationNodeCounts {
    pub self_user: usize,
    pub friend: usize,
    pub acquaintance: usize,
    pub stranger: usize,
    pub group: usize,
}

#[derive(Debug, Clone)]
pub struct RelationGraphOptions {
    pub show_self_user: bool,
    pub show_friends: bool,
    pub show_acquaintances: bool,
    pub show_strangers: bool,
    pub show_groups: bool,
}

impl Default for RelationGraphOptions {
    fn default() -> Self {
        Self {
            show_self_user: true,
            show_friends: true,
            show_acquaintances: true,
            show_strangers: true,
            show_groups: true,
        }
    }
}

impl RelationGraphOptions {
    pub fn allows(self: &Self, kind: RelationNodeKind) -> bool {
        match kind {
            RelationNodeKind::SelfUser => self.show_self_user,
            RelationNodeKind::Friend => self.show_friends,
            RelationNodeKind::Acquaintance => self.show_acquaintances,
            RelationNodeKind::Stranger => self.show_strangers,
            RelationNodeKind::Group => self.show_groups,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RelationNetworkState {
    pub include_unloaded_groups: bool,
    pub auto_request_members: bool,
    pub show_labels: bool,
    pub options: RelationGraphOptions,
    pub search_query: String,
    pub focused_node_id: Option<String>,
    pub selected_node_id: Option<String>,
    pub graph: RelationGraph,
    pub closed: bool,
    pub pending_load_limit: Option<Option<usize>>,
    pub pending_rebuild: bool,
}

impl Default for RelationNetworkState {
    fn default() -> Self {
        Self {
            include_unloaded_groups: true,
            auto_request_members: false,
            show_labels: true,
            options: RelationGraphOptions::default(),
            search_query: String::new(),
            focused_node_id: None,
            selected_node_id: None,
            graph: RelationGraph::default(),
            closed: false,
            pending_load_limit: None,
            pending_rebuild: false,
        }
    }
}

impl IcaApp {
    pub fn render_relation_network_window(&mut self, ctx: &egui::Context) {
        if !self.open_page.relation_network {
            return;
        }

        let relation_network = self.relation_network.clone();
        let command_tx = self
            .active_bridge_idx
            .and_then(|idx| self.ica_clients.get(idx))
            .map(|client| client.command_tx.clone());
        let bridge_snapshot = self.active_bridge_idx.and_then(|idx| {
            self.bridge_states
                .get(idx)
                .map(|state| RelationBridgeSnapshot {
                    rooms_len: state.rooms.len(),
                    loaded_groups: state
                        .group_members_by_room
                        .keys()
                        .filter(|room_id| **room_id < 0)
                        .count(),
                    loading_groups: state
                        .loading_group_members
                        .iter()
                        .filter(|room_id| **room_id < 0)
                        .count(),
                    pending_group_room_ids: state
                        .rooms
                        .iter()
                        .filter(|room| room.room_id < 0)
                        .map(|room| room.room_id)
                        .filter(|room_id| {
                            !state.group_members_by_room.contains_key(room_id)
                                && !state.loading_group_members.contains(room_id)
                        })
                        .collect(),
                })
        });

        let viewport_id = egui::ViewportId::from_hash_of("relation_network");
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title("QQ 关系网")
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([760.0, 480.0]);

        ctx.show_viewport_deferred(
            viewport_id,
            viewport_builder,
            move |viewport_ctx, _class| {
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    relation_network.lock().unwrap().closed = true;
                    return;
                }

                if viewport_ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    relation_network.lock().unwrap().closed = true;
                    viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }

                egui::CentralPanel::default().show(viewport_ctx, |ui| {
                    render_relation_network_ui(
                        ui,
                        &relation_network,
                        bridge_snapshot.as_ref(),
                        command_tx.as_ref(),
                    );
                });
            },
        );

        self.apply_relation_network_viewport_actions();
    }

    pub fn rebuild_relation_network(&mut self) {
        let Some(state) = self.active_bridge_state() else {
            self.relation_network.lock().unwrap().graph = RelationGraph::default();
            return;
        };

        let login_user_id = relation_login_user_id(state);
        let rooms = state.rooms.clone();
        let group_members_by_room = state.group_members_by_room.clone();
        let include_unloaded_groups = self
            .relation_network
            .lock()
            .unwrap()
            .include_unloaded_groups;
        let graph = RelationGraphBuilder::build(
            login_user_id,
            &rooms,
            &group_members_by_room,
            include_unloaded_groups,
        );
        self.relation_network.lock().unwrap().graph = graph;
        self.apply_relation_network_auto_degrade();
    }

    pub fn request_relation_network_members(&mut self, limit: Option<usize>) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };

        request_relation_network_members_with_tx(
            state,
            &self.ica_clients[bridge_idx].command_tx,
            limit,
        );
    }

    pub fn refresh_relation_network_after_bridge_update(&mut self, bridge_idx: usize) {
        if Some(bridge_idx) != self.active_bridge_idx {
            return;
        }
        self.rebuild_relation_network();
    }

    fn apply_relation_network_auto_degrade(&mut self) {
        let mut relation_network = self.relation_network.lock().unwrap();
        let node_count = relation_network.graph.nodes.len();
        if node_count > 2_000 {
            relation_network.show_labels = false;
        }
        if node_count > 10_000 {
            relation_network.options.show_acquaintances = false;
        }
        if node_count > 50_000 {
            relation_network.options.show_strangers = false;
        }
    }

    fn apply_relation_network_viewport_actions(&mut self) {
        let (closed, pending_rebuild, pending_load_limit) = {
            let mut relation_network = self.relation_network.lock().unwrap();
            let actions = (
                relation_network.closed,
                relation_network.pending_rebuild,
                relation_network.pending_load_limit.take(),
            );
            relation_network.pending_rebuild = false;
            relation_network.closed = false;
            actions
        };

        if closed {
            self.open_page.relation_network = false;
        }
        if pending_rebuild {
            self.rebuild_relation_network();
        }
        if let Some(limit) = pending_load_limit {
            self.request_relation_network_members(limit);
        }
    }
}

fn render_relation_network_ui(
    ui: &mut egui::Ui,
    relation_network: &Arc<Mutex<RelationNetworkState>>,
    bridge_snapshot: Option<&RelationBridgeSnapshot>,
    command_tx: Option<&UnboundedSender<IcaCommand>>,
) {
    let Some(bridge_snapshot) = bridge_snapshot else {
        ui.weak("当前没有启用的 bridge");
        return;
    };

    let mut relation_network = relation_network.lock().unwrap();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(250.0);
            ui.heading("关系网");
            ui.label(format!("会话: {}", bridge_snapshot.rooms_len));
            ui.label(format!(
                "群成员: {}/{} 已加载，{} 加载中",
                relation_network.graph.loaded_group_count,
                relation_network.graph.total_group_count,
                bridge_snapshot.loading_groups
            ));
            if relation_network.graph.loaded_group_count != bridge_snapshot.loaded_groups {
                ui.weak("图数据等待下一次刷新");
            }
            ui.separator();

            if ui.button("重建关系网").clicked() {
                relation_network.pending_rebuild = true;
            }
            ui.horizontal(|ui| {
                if ui.button("加载 10 个群").clicked() {
                    queue_relation_member_requests(
                        &mut relation_network,
                        bridge_snapshot,
                        command_tx,
                        Some(10),
                    );
                }
                if ui.button("加载全部群").clicked() {
                    queue_relation_member_requests(
                        &mut relation_network,
                        bridge_snapshot,
                        command_tx,
                        None,
                    );
                }
            });

            ui.checkbox(
                &mut relation_network.include_unloaded_groups,
                "显示未加载群",
            );
            if ui.button("应用显示范围").clicked() {
                relation_network.pending_rebuild = true;
            }
            ui.checkbox(&mut relation_network.show_labels, "显示标签");
            ui.separator();

            let counts = relation_network.graph.node_counts();
            ui.checkbox(
                &mut relation_network.options.show_self_user,
                format!(
                    "{} ({})",
                    RelationNodeKind::SelfUser.label(),
                    counts.self_user
                ),
            );
            ui.checkbox(
                &mut relation_network.options.show_friends,
                format!("{} ({})", RelationNodeKind::Friend.label(), counts.friend),
            );
            ui.checkbox(
                &mut relation_network.options.show_acquaintances,
                format!(
                    "{} ({})",
                    RelationNodeKind::Acquaintance.label(),
                    counts.acquaintance
                ),
            );
            ui.checkbox(
                &mut relation_network.options.show_strangers,
                format!(
                    "{} ({})",
                    RelationNodeKind::Stranger.label(),
                    counts.stranger
                ),
            );
            ui.checkbox(
                &mut relation_network.options.show_groups,
                format!("{} ({})", RelationNodeKind::Group.label(), counts.group),
            );

            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut relation_network.search_query)
                    .hint_text("搜索昵称 / QQ / 群号"),
            );
            if ui.button("清除聚焦").clicked() {
                relation_network.focused_node_id = None;
                relation_network.selected_node_id = None;
            }

            if let Some(node_id) = &relation_network.selected_node_id
                && let Some(node) = relation_network
                    .graph
                    .nodes
                    .iter()
                    .find(|node| &node.id == node_id)
            {
                ui.separator();
                ui.strong(&node.name);
                ui.label(node.kind.label());
                if let Some(qq) = node.qq {
                    ui.label(format!("QQ: {qq}"));
                }
                if let Some(group_id) = node.group_id {
                    ui.label(format!("群号: {group_id}"));
                }
                if let Some(member_count) = node.member_count {
                    ui.label(format!("成员数: {member_count}"));
                }
                if node.common_group_count > 0 {
                    ui.label(format!("共同群: {}", node.common_group_count));
                }
                ui.label(format!("关联数: {}", node.value));
                if !node.role.trim().is_empty() {
                    ui.label(format!("角色: {}", node.role));
                }
            }
        });

        ui.separator();
        render_relation_network_canvas(ui, &mut relation_network);
    });
}

fn render_relation_network_canvas(ui: &mut egui::Ui, relation_network: &mut RelationNetworkState) {
    let available = ui.available_size_before_wrap();
    let size = egui::vec2(available.x.max(360.0), available.y.max(320.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);

    let query = relation_network.search_query.trim().to_lowercase();
    let visible = visible_relation_node_ids(relation_network, &query);
    if visible.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "暂无关系数据。先加载群成员或等待会话数据同步。",
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        return;
    }

    let positions = relation_node_positions(&relation_network.graph, &visible, rect);
    let visible_set: HashSet<&str> = visible.iter().map(String::as_str).collect();
    let focused = relation_network.focused_node_id.as_deref();
    let focus_neighbors = focused.map(|focused| {
        relation_neighbors(&relation_network.graph, focused)
            .into_iter()
            .collect::<HashSet<_>>()
    });

    let mut drawn_links = 0usize;
    for link in &relation_network.graph.links {
        if drawn_links >= 4_000 {
            break;
        }
        if !visible_set.contains(link.source.as_str())
            || !visible_set.contains(link.target.as_str())
        {
            continue;
        }
        if let Some(focused) = focused
            && link.source != focused
            && link.target != focused
            && !focus_neighbors.as_ref().is_some_and(|neighbors| {
                neighbors.contains(link.source.as_str()) && neighbors.contains(link.target.as_str())
            })
        {
            continue;
        }
        let Some(source) = positions.get(&link.source) else {
            continue;
        };
        let Some(target) = positions.get(&link.target) else {
            continue;
        };
        painter.line_segment(
            [*source, *target],
            egui::Stroke::new(1.0, ui.visuals().weak_text_color().linear_multiply(0.28)),
        );
        drawn_links += 1;
    }

    let pointer_pos = response.hover_pos();
    let mut hovered_node_id = None;
    for node in &relation_network.graph.nodes {
        if !visible_set.contains(node.id.as_str()) {
            continue;
        }
        if let Some(focused) = focused
            && node.id != focused
            && !focus_neighbors
                .as_ref()
                .is_some_and(|neighbors| neighbors.contains(node.id.as_str()))
        {
            continue;
        }
        let Some(pos) = positions.get(&node.id) else {
            continue;
        };
        let is_selected = relation_network.selected_node_id.as_deref() == Some(node.id.as_str());
        let radius = node.radius.max(6.0);
        let color = node.kind.color();
        let stroke = if is_selected {
            egui::Stroke::new(3.0, egui::Color32::WHITE)
        } else if node.kind == RelationNodeKind::Group {
            egui::Stroke::new(1.0 + node.size_level as f32, egui::Color32::WHITE)
        } else {
            egui::Stroke::new(1.0, ui.visuals().panel_fill)
        };
        painter.circle_filled(*pos, radius, color);
        painter.circle_stroke(*pos, radius, stroke);

        if relation_network.show_labels && visible.len() <= 700 {
            painter.text(
                *pos + egui::vec2(radius + 3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &node.name,
                egui::TextStyle::Small.resolve(ui.style()),
                ui.visuals().text_color(),
            );
        }

        if pointer_pos.is_some_and(|pointer| pointer.distance(*pos) <= radius + 3.0) {
            hovered_node_id = Some(node.id.clone());
        }
    }

    if response.clicked() {
        if let Some(node_id) = hovered_node_id {
            relation_network.selected_node_id = Some(node_id.clone());
            relation_network.focused_node_id = Some(node_id);
        } else {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
        }
    }
}

fn visible_relation_node_ids(relation_network: &RelationNetworkState, query: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let focused = relation_network.focused_node_id.as_deref();
    let focus_neighbors = focused.map(|focused| {
        relation_neighbors(&relation_network.graph, focused)
            .into_iter()
            .collect::<HashSet<_>>()
    });

    for node in &relation_network.graph.nodes {
        if !relation_network.options.allows(node.kind) || !node.matches_query(query) {
            continue;
        }
        if let Some(focused) = focused
            && node.id != focused
            && !focus_neighbors
                .as_ref()
                .is_some_and(|neighbors| neighbors.contains(node.id.as_str()))
        {
            continue;
        }
        ids.push(node.id.clone());
        if ids.len() >= 6_000 {
            break;
        }
    }
    ids
}

#[derive(Debug, Clone)]
struct RelationBridgeSnapshot {
    rooms_len: usize,
    loaded_groups: usize,
    loading_groups: usize,
    pending_group_room_ids: Vec<RoomId>,
}

fn queue_relation_member_requests(
    relation_network: &mut RelationNetworkState,
    snapshot: &RelationBridgeSnapshot,
    command_tx: Option<&UnboundedSender<IcaCommand>>,
    limit: Option<usize>,
) {
    let Some(command_tx) = command_tx else {
        relation_network.pending_load_limit = Some(limit);
        return;
    };

    let mut queued = 0usize;
    for &room_id in &snapshot.pending_group_room_ids {
        if let Err(e) = command_tx.send(IcaCommand::FetchGroupMembers { room_id }) {
            tracing::warn!("send relation network fetchGroupMembers failed: {}", e);
            break;
        }
        queued += 1;
        if limit.is_some_and(|limit| queued >= limit) {
            break;
        }
    }

    if queued == 0 {
        relation_network.pending_load_limit = Some(limit);
    }
}

fn request_relation_network_members_with_tx(
    state: &mut BridgeState,
    command_tx: &UnboundedSender<IcaCommand>,
    limit: Option<usize>,
) {
    let mut queued = 0usize;
    let group_room_ids: Vec<_> = state
        .rooms
        .iter()
        .filter(|room| room.room_id < 0)
        .map(|room| room.room_id)
        .collect();

    for room_id in group_room_ids {
        if state.group_members_by_room.contains_key(&room_id)
            || state.loading_group_members.contains(&room_id)
        {
            continue;
        }
        state.loading_group_members.insert(room_id);
        if let Err(e) = command_tx.send(IcaCommand::FetchGroupMembers { room_id }) {
            state.loading_group_members.remove(&room_id);
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
}

fn relation_login_user_id(state: &BridgeState) -> Option<i64> {
    (state.online_data.qqid > 0).then_some(state.online_data.qqid)
}

fn relation_neighbors<'a>(graph: &'a RelationGraph, node_id: &str) -> Vec<&'a str> {
    graph
        .links
        .iter()
        .filter_map(|link| {
            if link.source == node_id {
                Some(link.target.as_str())
            } else if link.target == node_id {
                Some(link.source.as_str())
            } else {
                None
            }
        })
        .collect()
}

fn relation_node_positions(
    graph: &RelationGraph,
    visible_ids: &[String],
    rect: egui::Rect,
) -> HashMap<String, egui::Pos2> {
    let mut positions = HashMap::with_capacity(visible_ids.len());
    let center = rect.center();
    if visible_ids.len() == 1 {
        positions.insert(visible_ids[0].clone(), center);
        return positions;
    }

    let max_radius = rect.width().min(rect.height()) * 0.43;
    let mut rings: [Vec<&RelationNode>; 5] = std::array::from_fn(|_| Vec::new());
    let visible_set: HashSet<&str> = visible_ids.iter().map(String::as_str).collect();
    for node in &graph.nodes {
        if visible_set.contains(node.id.as_str()) {
            rings[node_kind_order(node.kind) as usize].push(node);
        }
    }

    let ring_count = rings.iter().filter(|ring| !ring.is_empty()).count().max(1);
    let mut ring_idx = 0usize;
    for ring in rings {
        if ring.is_empty() {
            continue;
        }
        let radius = if ring_count == 1 {
            max_radius * 0.65
        } else {
            max_radius * ((ring_idx + 1) as f32 / ring_count as f32)
        };
        let angle_step = std::f32::consts::TAU / ring.len().max(1) as f32;
        for (idx, node) in ring.iter().enumerate() {
            let angle = idx as f32 * angle_step + ring_idx as f32 * 0.37;
            positions.insert(
                node.id.clone(),
                egui::pos2(
                    center.x + angle.cos() * radius,
                    center.y + angle.sin() * radius,
                ),
            );
        }
        ring_idx += 1;
    }

    positions
}

#[derive(Debug, Default)]
pub struct RelationGraphBuilder {
    nodes: HashMap<String, RelationNode>,
    links: HashMap<(String, String), RelationLink>,
    user_group_map: HashMap<i64, HashSet<i64>>,
    friend_ids: HashSet<i64>,
    login_user_id: Option<i64>,
}

impl RelationGraphBuilder {
    pub fn build(
        login_user_id: Option<i64>,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
        include_unloaded_groups: bool,
    ) -> RelationGraph {
        let mut builder = Self {
            login_user_id,
            ..Self::default()
        };
        builder.add_self_user();
        builder.add_private_rooms(rooms);
        builder.add_groups(rooms, group_members_by_room, include_unloaded_groups);
        builder.add_group_members(group_members_by_room);
        builder.finalize(rooms, group_members_by_room)
    }

    fn add_self_user(&mut self) {
        let Some(user_id) = self.login_user_id else {
            return;
        };
        let node = RelationNode::new_user(user_id, "我".to_string(), RelationNodeKind::SelfUser);
        self.nodes.insert(node.id.clone(), node);
    }

    fn add_private_rooms(&mut self, rooms: &[Room]) {
        for room in rooms.iter().filter(|room| room.room_id > 0) {
            let user_id = room.room_id;
            self.friend_ids.insert(user_id);
            let node = RelationNode::new_user(
                user_id,
                if room.room_name.trim().is_empty() {
                    user_id.to_string()
                } else {
                    room.room_name.clone()
                },
                RelationNodeKind::Friend,
            );
            self.nodes.entry(node.id.clone()).or_insert(node);
            if let Some(login_user_id) = self.login_user_id {
                self.add_link(format!("u:{login_user_id}"), format!("u:{user_id}"), None);
            }
        }
    }

    fn add_groups(
        &mut self,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
        include_unloaded_groups: bool,
    ) {
        for room in rooms.iter().filter(|room| room.room_id < 0) {
            if !include_unloaded_groups && !group_members_by_room.contains_key(&room.room_id) {
                continue;
            }
            let member_count = group_members_by_room.get(&room.room_id).map(Vec::len);
            let node = RelationNode::new_group(room, member_count);
            let group_id = node.group_id.unwrap_or_default();
            self.nodes.entry(node.id.clone()).or_insert(node);

            if let Some(login_user_id) = self.login_user_id {
                self.user_group_map
                    .entry(login_user_id)
                    .or_default()
                    .insert(group_id);
                self.add_link(
                    format!("u:{login_user_id}"),
                    format!("g:{group_id}"),
                    Some(group_id),
                );
            }
        }
    }

    fn add_group_members(&mut self, group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>) {
        for (&room_id, members) in group_members_by_room {
            if room_id >= 0 {
                continue;
            }
            let group_id = -room_id;
            let group_node_id = format!("g:{group_id}");
            if !self.nodes.contains_key(&group_node_id) {
                continue;
            }

            for member in members {
                let user_id = member.user_id;
                let user_node_id = format!("u:{user_id}");
                self.user_group_map
                    .entry(user_id)
                    .or_default()
                    .insert(group_id);

                if Some(user_id) != self.login_user_id {
                    let kind = if self.friend_ids.contains(&user_id) {
                        RelationNodeKind::Friend
                    } else if self
                        .user_group_map
                        .get(&user_id)
                        .is_some_and(|groups| groups.len() >= 2)
                    {
                        RelationNodeKind::Acquaintance
                    } else {
                        RelationNodeKind::Stranger
                    };

                    let name = member.display_name();
                    let name = if name.trim().is_empty() {
                        user_id.to_string()
                    } else {
                        name.to_string()
                    };
                    let node = self
                        .nodes
                        .entry(user_node_id.clone())
                        .or_insert_with(|| RelationNode::new_user(user_id, name, kind));

                    if node.kind != RelationNodeKind::Friend {
                        node.kind = kind;
                    }
                    if node.name == user_id.to_string() && !member.display_name().trim().is_empty()
                    {
                        node.name = member.display_name().to_string();
                    }
                    if !member.role.trim().is_empty() {
                        node.role = member.role.clone();
                    }
                }

                self.add_link(group_node_id.clone(), user_node_id, Some(group_id));
            }
        }
    }

    fn add_link(&mut self, source: String, target: String, group_id: Option<i64>) {
        if source == target {
            return;
        }
        let key = if source <= target {
            (source.clone(), target.clone())
        } else {
            (target.clone(), source.clone())
        };
        if self.links.contains_key(&key) {
            return;
        }
        if let Some(node) = self.nodes.get_mut(&source) {
            node.value += 1;
        }
        if let Some(node) = self.nodes.get_mut(&target) {
            node.value += 1;
        }
        self.links.insert(
            key,
            RelationLink {
                source,
                target,
                group_id,
            },
        );
    }

    fn finalize(
        mut self,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
    ) -> RelationGraph {
        for node in self.nodes.values_mut() {
            if let Some(qq) = node.qq {
                node.common_group_count = self.user_group_map.get(&qq).map_or(0, HashSet::len);
            }
            node.update_size();
        }

        let mut nodes: Vec<_> = self.nodes.into_values().collect();
        nodes.sort_by(|left, right| {
            node_kind_order(left.kind)
                .cmp(&node_kind_order(right.kind))
                .then_with(|| right.value.cmp(&left.value))
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut links: Vec<_> = self.links.into_values().collect();
        links.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.target.cmp(&right.target))
        });

        RelationGraph {
            nodes,
            links,
            loaded_group_count: group_members_by_room
                .keys()
                .filter(|room_id| **room_id < 0)
                .count(),
            total_group_count: rooms.iter().filter(|room| room.room_id < 0).count(),
        }
    }
}

fn node_kind_order(kind: RelationNodeKind) -> u8 {
    match kind {
        RelationNodeKind::SelfUser => 0,
        RelationNodeKind::Friend => 1,
        RelationNodeKind::Acquaintance => 2,
        RelationNodeKind::Stranger => 3,
        RelationNodeKind::Group => 4,
    }
}
