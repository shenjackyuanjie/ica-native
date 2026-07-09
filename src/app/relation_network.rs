use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::cfg::{self, RelationNetworkSetting};
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RelationViewMode {
    #[default]
    Default,
    Focused(String),
    MultiSelect,
    MultiSelectRelationship,
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
    pub hovered_node_id: Option<String>,
    pub selected_node_ids: HashSet<String>,
    pub view_mode: RelationViewMode,
    pub graph: RelationGraph,
    pub closed: bool,
    pub pending_load_limit: Option<Option<usize>>,
    pub pending_rebuild: bool,
    pub render_setting: RelationNetworkSetting,
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
            hovered_node_id: None,
            selected_node_ids: HashSet::new(),
            view_mode: RelationViewMode::Default,
            graph: RelationGraph::default(),
            closed: false,
            pending_load_limit: None,
            pending_rebuild: false,
            render_setting: RelationNetworkSetting::default(),
        }
    }
}

impl RelationNetworkState {
    pub fn with_render_setting(mut self, render_setting: RelationNetworkSetting) -> Self {
        self.render_setting = render_setting;
        self
    }
}

impl IcaApp {
    pub fn render_relation_network_window(&mut self, ctx: &egui::Context) {
        if !self.open_page.relation_network {
            return;
        }
        self.sync_relation_network_render_setting();

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
        self.sync_relation_network_render_setting();

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
        let render_setting = relation_network.render_setting.clone();
        if node_count > render_setting.auto_hide_labels_node_threshold {
            relation_network.show_labels = false;
        }
        if node_count > render_setting.auto_hide_acquaintance_node_threshold {
            relation_network.options.show_acquaintances = false;
        }
        if node_count > render_setting.auto_hide_stranger_node_threshold {
            relation_network.options.show_strangers = false;
        }
    }

    fn sync_relation_network_render_setting(&mut self) {
        let render_setting = cfg::get_cfg_snapshot().ui_setting.relation_network;
        self.relation_network.lock().unwrap().render_setting = render_setting;
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
        render_relation_network_sidebar(ui, &mut relation_network, bridge_snapshot, command_tx);
        ui.separator();
        render_relation_network_canvas(ui, &mut relation_network);
    });
}

fn render_relation_network_sidebar(
    ui: &mut egui::Ui,
    relation_network: &mut RelationNetworkState,
    bridge_snapshot: &RelationBridgeSnapshot,
    command_tx: Option<&UnboundedSender<IcaCommand>>,
) {
    egui::ScrollArea::vertical()
        .id_salt("relation_network_sidebar")
        .max_width(292.0)
        .show(ui, |ui| {
            ui.set_width(268.0);
            ui.heading("QQ 关系网");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("会话: {}", bridge_snapshot.rooms_len));
                ui.separator();
                ui.label(format!(
                    "群成员: {}/{}",
                    relation_network.graph.loaded_group_count,
                    relation_network.graph.total_group_count
                ));
                if bridge_snapshot.loading_groups > 0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 126, 34),
                        format!("{} 加载中", bridge_snapshot.loading_groups),
                    );
                }
            });
            if relation_network.graph.loaded_group_count != bridge_snapshot.loaded_groups {
                ui.weak("图数据等待下一次刷新");
            }

            ui.add_space(6.0);
            render_relation_stats(ui, relation_network);
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("刷新").clicked() {
                    relation_network.pending_rebuild = true;
                }
                let multi_label = match relation_network.view_mode {
                    RelationViewMode::MultiSelectRelationship => "退出",
                    RelationViewMode::MultiSelect => "查看关系",
                    _ => "多选模式",
                };
                if ui.button(multi_label).clicked() {
                    toggle_relation_multi_select(relation_network);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("加载 10 个群").clicked() {
                    queue_relation_member_requests(
                        relation_network,
                        bridge_snapshot,
                        command_tx,
                        Some(10),
                    );
                }
                if ui.button("加载全部群").clicked() {
                    queue_relation_member_requests(
                        relation_network,
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
            render_relation_filter_options(ui, relation_network);

            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut relation_network.search_query)
                    .hint_text("搜索昵称 / QQ / 群号"),
            );

            render_relation_view_hint(ui, relation_network);
            if ui.button("清除聚焦 / 多选").clicked() {
                relation_network.focused_node_id = None;
                relation_network.selected_node_id = None;
                relation_network.selected_node_ids.clear();
                relation_network.view_mode = RelationViewMode::Default;
            }

            if let Some(node_id) = &relation_network.selected_node_id
                && let Some(node) = relation_network
                    .graph
                    .nodes
                    .iter()
                    .find(|node| &node.id == node_id)
            {
                ui.separator();
                render_relation_node_detail(ui, node);
            }

            ui.separator();
            render_relation_size_legend(ui);
        });
}

fn render_relation_stats(ui: &mut egui::Ui, relation_network: &RelationNetworkState) {
    let counts = relation_network.graph.node_counts();
    egui::Grid::new("relation_stats_grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            relation_stat_card(ui, "节点", relation_network.graph.nodes.len());
            relation_stat_card(ui, "连线", relation_network.graph.links.len());
            ui.end_row();
            relation_stat_card(ui, "好友", counts.friend);
            relation_stat_card(ui, "群", counts.group);
            ui.end_row();
        });
}

fn relation_stat_card(ui: &mut egui::Ui, label: &str, value: usize) {
    egui::Frame::NONE
        .fill(egui::Color32::from_rgb(248, 249, 250))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.weak(label);
                ui.strong(value.to_string());
            });
        });
}

fn render_relation_filter_options(ui: &mut egui::Ui, relation_network: &mut RelationNetworkState) {
    let counts = relation_network.graph.node_counts();
    relation_option_row(
        ui,
        RelationNodeKind::SelfUser,
        &mut relation_network.options.show_self_user,
        counts.self_user,
    );
    relation_option_row(
        ui,
        RelationNodeKind::Friend,
        &mut relation_network.options.show_friends,
        counts.friend,
    );
    relation_option_row(
        ui,
        RelationNodeKind::Acquaintance,
        &mut relation_network.options.show_acquaintances,
        counts.acquaintance,
    );
    relation_option_row(
        ui,
        RelationNodeKind::Stranger,
        &mut relation_network.options.show_strangers,
        counts.stranger,
    );
    relation_option_row(
        ui,
        RelationNodeKind::Group,
        &mut relation_network.options.show_groups,
        counts.group,
    );
}

fn relation_option_row(
    ui: &mut egui::Ui,
    kind: RelationNodeKind,
    enabled: &mut bool,
    count: usize,
) {
    ui.horizontal(|ui| {
        ui.checkbox(enabled, "");
        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(dot_rect.center(), 6.0, kind.color());
        ui.label(kind.label());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(format!("({count})"));
        });
    });
}

fn render_relation_view_hint(ui: &mut egui::Ui, relation_network: &RelationNetworkState) {
    match &relation_network.view_mode {
        RelationViewMode::Focused(node_id) => {
            if let Some(node) = relation_network
                .graph
                .nodes
                .iter()
                .find(|node| &node.id == node_id)
            {
                ui.colored_label(
                    egui::Color32::from_rgb(74, 144, 217),
                    format!("一级关系网: {}", node.name),
                );
            }
        }
        RelationViewMode::MultiSelect => {
            ui.colored_label(
                egui::Color32::from_rgb(74, 144, 217),
                format!("已选择 {} 个节点", relation_network.selected_node_ids.len()),
            );
        }
        RelationViewMode::MultiSelectRelationship => {
            ui.colored_label(
                egui::Color32::from_rgb(74, 144, 217),
                format!(
                    "关系视图: {} 个节点",
                    relation_network.selected_node_ids.len()
                ),
            );
        }
        RelationViewMode::Default => {}
    }
}

fn render_relation_node_detail(ui: &mut egui::Ui, node: &RelationNode) {
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

fn render_relation_size_legend(ui: &mut egui::Ui) {
    ui.label("大小说明");
    for (label, radius, border) in [
        ("少量关联", 5.0, 0.0),
        ("数十人 / 百人群", 8.0, 2.0),
        ("千人群", 11.0, 3.0),
        ("万人群", 14.0, 4.0),
    ] {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 24.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), radius, RelationNodeKind::Group.color());
            if border > 0.0 {
                ui.painter().circle_stroke(
                    rect.center(),
                    radius,
                    egui::Stroke::new(border, egui::Color32::WHITE),
                );
            }
            ui.weak(label);
        });
    }
}

fn render_relation_network_canvas(ui: &mut egui::Ui, relation_network: &mut RelationNetworkState) {
    let available = ui.available_size_before_wrap();
    let size = egui::vec2(available.x.max(360.0), available.y.max(320.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(245, 247, 250));

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
    let focused = relation_focused_node_id(relation_network);
    let focus_neighbors = focused.map(|focused| {
        relation_neighbors(&relation_network.graph, focused)
            .into_iter()
            .collect::<HashSet<_>>()
    });
    let performance = relation_performance_level(visible.len());
    let line_opacity = match performance.index {
        0 => 0.30,
        1 => 0.20,
        2 => 0.10,
        3 => 0.05,
        _ => 0.03,
    };

    let max_links = relation_drawn_link_limit(relation_network);
    let mut drawn_links = 0usize;
    for link in &relation_network.graph.links {
        if drawn_links >= max_links {
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
                relation_link_visible_for_focus(&relation_network.graph, link, focused, neighbors)
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
            egui::Stroke::new(
                if performance.index <= 1 { 1.0 } else { 0.8 },
                egui::Color32::from_gray(150).linear_multiply(line_opacity),
            ),
        );
        drawn_links += 1;
    }

    render_relation_network_overlay(
        ui,
        relation_network,
        rect,
        &visible,
        drawn_links,
        performance,
    );

    if visible.len() < relation_network.graph.nodes.len()
        || drawn_links < relation_network.graph.links.len()
    {
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!(
                "显示 {} / {} 节点，{} / {} 连线",
                visible.len(),
                relation_network.graph.nodes.len(),
                drawn_links,
                relation_network.graph.links.len()
            ),
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
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
        let is_multi_selected = relation_network.selected_node_ids.contains(&node.id);
        let radius =
            if is_multi_selected && relation_network.view_mode == RelationViewMode::MultiSelect {
                node.radius.max(6.0) * 1.18
            } else {
                node.radius.max(6.0)
            };
        let color = node.kind.color();
        let fill =
            if relation_network.view_mode == RelationViewMode::MultiSelect && !is_multi_selected {
                color.linear_multiply(0.30)
            } else {
                color
            };
        let stroke = if is_selected || is_multi_selected {
            egui::Stroke::new(3.0, egui::Color32::WHITE)
        } else if node.kind == RelationNodeKind::Group {
            egui::Stroke::new(1.0 + node.size_level as f32, egui::Color32::WHITE)
        } else if node.kind == RelationNodeKind::Friend {
            egui::Stroke::new(1.0, egui::Color32::WHITE)
        } else {
            egui::Stroke::new(1.0, ui.visuals().panel_fill)
        };
        painter.circle_filled(*pos, radius, fill);
        painter.circle_stroke(*pos, radius, stroke);

        if relation_network.show_labels
            && visible.len() <= relation_network.render_setting.max_labels
        {
            let label = if relation_network.view_mode == RelationViewMode::MultiSelect
                && !is_multi_selected
            {
                ""
            } else {
                &node.name
            };
            painter.text(
                *pos + egui::vec2(radius + 3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::TextStyle::Small.resolve(ui.style()),
                ui.visuals().text_color(),
            );
        }

        if pointer_pos.is_some_and(|pointer| pointer.distance(*pos) <= radius + 3.0) {
            hovered_node_id = Some(node.id.clone());
        }
    }
    relation_network.hovered_node_id = hovered_node_id.clone();

    if response.clicked() {
        if let Some(node_id) = hovered_node_id {
            handle_relation_node_click(relation_network, node_id);
        } else {
            exit_relation_focus_or_multiselect(relation_network);
        }
    }

    if let Some(node_id) = relation_network.hovered_node_id.clone()
        && let Some(node) = relation_network
            .graph
            .nodes
            .iter()
            .find(|node| node.id == node_id)
    {
        render_relation_node_popup(ui, rect, pointer_pos.unwrap_or(rect.center()), node);
    }
}

fn visible_relation_node_ids(relation_network: &RelationNetworkState, query: &str) -> Vec<String> {
    let mut ids = match &relation_network.view_mode {
        RelationViewMode::Focused(focused) => {
            relation_visible_ids_from_focus(relation_network, focused, query)
        }
        RelationViewMode::MultiSelectRelationship => {
            let mut ids: HashSet<_> = relation_multi_select_relationship_ids(relation_network)
                .into_iter()
                .collect();
            ids.retain(|id| {
                relation_node_by_id(&relation_network.graph, id).is_some_and(|node| {
                    relation_network.options.allows(node.kind) && node.matches_query(query)
                })
            });
            relation_node_kind_ordered_ids(&relation_network.graph, ids)
        }
        RelationViewMode::Default | RelationViewMode::MultiSelect => {
            relation_visible_ids_default(relation_network, query)
        }
    };
    ids.truncate(relation_view_limit(relation_network));
    ids
}

fn render_relation_network_overlay(
    ui: &mut egui::Ui,
    relation_network: &RelationNetworkState,
    rect: egui::Rect,
    visible: &[String],
    drawn_links: usize,
    performance: RelationPerformanceLevel,
) {
    let painter = ui.painter_at(rect);
    let badge = format!("性能: {} ({}节点)", performance.label, visible.len());
    let galley = painter.layout_no_wrap(
        badge,
        egui::TextStyle::Small.resolve(ui.style()),
        egui::Color32::from_rgb(46, 125, 50),
    );
    let badge_rect = egui::Rect::from_min_size(
        rect.right_top() - egui::vec2(galley.size().x + 32.0, -12.0),
        galley.size() + egui::vec2(24.0, 8.0),
    );
    painter.rect_filled(badge_rect, 16.0, egui::Color32::from_rgb(232, 245, 233));
    painter.circle_filled(
        egui::pos2(badge_rect.left() + 10.0, badge_rect.center().y),
        4.0,
        egui::Color32::from_rgb(76, 175, 80),
    );
    painter.galley(
        badge_rect.min + egui::vec2(18.0, 4.0),
        galley,
        egui::Color32::from_rgb(46, 125, 50),
    );

    match &relation_network.view_mode {
        RelationViewMode::Focused(node_id) => {
            if let Some(node) = relation_node_by_id(&relation_network.graph, node_id) {
                render_relation_indicator(
                    ui,
                    rect,
                    &format!("一级关系网: {}（点击空白处返回）", node.name),
                );
            }
        }
        RelationViewMode::MultiSelectRelationship => {
            render_relation_indicator(ui, rect, "多选关系网（点击空白处返回选择）");
        }
        RelationViewMode::Default | RelationViewMode::MultiSelect => {}
    }

    if drawn_links == 0 && visible.len() > 1 {
        painter.text(
            rect.center_top() + egui::vec2(0.0, 48.0),
            egui::Align2::CENTER_TOP,
            "当前筛选下没有可见连线",
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
    }
}

fn render_relation_indicator(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    let painter = ui.painter_at(rect);
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::TextStyle::Small.resolve(ui.style()),
        egui::Color32::WHITE,
    );
    let indicator_rect = egui::Rect::from_center_size(
        rect.center_top() + egui::vec2(0.0, 28.0),
        galley.size() + egui::vec2(28.0, 10.0),
    );
    painter.rect_filled(
        indicator_rect,
        16.0,
        egui::Color32::from_rgba_premultiplied(74, 144, 217, 230),
    );
    painter.galley(
        indicator_rect.min + egui::vec2(14.0, 5.0),
        galley,
        egui::Color32::WHITE,
    );
}

fn render_relation_node_popup(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    pointer: egui::Pos2,
    node: &RelationNode,
) {
    let popup_size = egui::vec2(280.0, 188.0);
    let mut pos = pointer + egui::vec2(18.0, -18.0);
    if pos.x + popup_size.x > canvas_rect.right() - 10.0 {
        pos.x = pointer.x - popup_size.x - 18.0;
    }
    if pos.y + popup_size.y > canvas_rect.bottom() - 10.0 {
        pos.y = canvas_rect.bottom() - popup_size.y - 10.0;
    }
    pos.x = pos.x.max(canvas_rect.left() + 10.0);
    pos.y = pos.y.max(canvas_rect.top() + 10.0);

    egui::Area::new(egui::Id::new(("relation_node_popup", &node.id)))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(250.0);
                    ui.horizontal(|ui| {
                        let (avatar_rect, _) =
                            ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                        let avatar_radius = if node.kind == RelationNodeKind::Group {
                            10.0
                        } else {
                            24.0
                        };
                        ui.painter()
                            .rect_filled(avatar_rect, avatar_radius, node.kind.color());
                        ui.painter().text(
                            avatar_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            if node.kind == RelationNodeKind::Group {
                                "群"
                            } else {
                                "QQ"
                            },
                            egui::TextStyle::Button.resolve(ui.style()),
                            egui::Color32::WHITE,
                        );
                        ui.vertical(|ui| {
                            ui.strong(&node.name);
                            ui.weak(node.kind.label());
                        });
                    });
                    ui.separator();
                    render_relation_node_detail(ui, node);
                });
        });
}

fn handle_relation_node_click(relation_network: &mut RelationNetworkState, node_id: String) {
    relation_network.selected_node_id = Some(node_id.clone());
    match relation_network.view_mode {
        RelationViewMode::MultiSelect | RelationViewMode::MultiSelectRelationship => {
            if relation_network.selected_node_ids.contains(&node_id) {
                relation_network.selected_node_ids.remove(&node_id);
            } else {
                relation_network.selected_node_ids.insert(node_id);
            }
            relation_network.view_mode = RelationViewMode::MultiSelect;
        }
        RelationViewMode::Default | RelationViewMode::Focused(_) => {
            relation_network.focused_node_id = Some(node_id.clone());
            relation_network.view_mode = RelationViewMode::Focused(node_id);
        }
    }
}

fn exit_relation_focus_or_multiselect(relation_network: &mut RelationNetworkState) {
    match relation_network.view_mode {
        RelationViewMode::MultiSelectRelationship => {
            relation_network.view_mode = RelationViewMode::MultiSelect;
        }
        RelationViewMode::MultiSelect => {}
        RelationViewMode::Focused(_) => {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
            relation_network.view_mode = RelationViewMode::Default;
        }
        RelationViewMode::Default => {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
        }
    }
}

fn toggle_relation_multi_select(relation_network: &mut RelationNetworkState) {
    match relation_network.view_mode {
        RelationViewMode::MultiSelectRelationship => {
            relation_network.view_mode = RelationViewMode::Default;
            relation_network.selected_node_ids.clear();
        }
        RelationViewMode::MultiSelect => {
            if relation_network.selected_node_ids.len() >= 2 {
                relation_network.view_mode = RelationViewMode::MultiSelectRelationship;
            }
        }
        RelationViewMode::Default | RelationViewMode::Focused(_) => {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
            relation_network.selected_node_ids.clear();
            relation_network.view_mode = RelationViewMode::MultiSelect;
        }
    }
}

fn relation_focused_node_id(relation_network: &RelationNetworkState) -> Option<&str> {
    match &relation_network.view_mode {
        RelationViewMode::Focused(node_id) => Some(node_id.as_str()),
        _ => relation_network.focused_node_id.as_deref(),
    }
}

fn relation_multi_select_relationship_ids(relation_network: &RelationNetworkState) -> Vec<String> {
    if relation_network.selected_node_ids.is_empty() {
        return Vec::new();
    }

    let mut visible_ids = relation_network.selected_node_ids.clone();
    let selected_groups: HashSet<_> = relation_network
        .selected_node_ids
        .iter()
        .filter(|id| {
            relation_node_by_id(&relation_network.graph, id)
                .is_some_and(|node| node.kind == RelationNodeKind::Group)
        })
        .cloned()
        .collect();
    let selected_non_groups: HashSet<_> = relation_network
        .selected_node_ids
        .iter()
        .filter(|id| !selected_groups.contains(*id))
        .cloned()
        .collect();

    if selected_non_groups.len() >= 2 {
        for link in &relation_network.graph.links {
            let source_node = relation_node_by_id(&relation_network.graph, &link.source);
            let target_node = relation_node_by_id(&relation_network.graph, &link.target);
            if source_node.is_some_and(|node| node.kind == RelationNodeKind::Group)
                && selected_non_groups.contains(&link.target)
            {
                visible_ids.insert(link.source.clone());
            } else if target_node.is_some_and(|node| node.kind == RelationNodeKind::Group)
                && selected_non_groups.contains(&link.source)
            {
                visible_ids.insert(link.target.clone());
            }
        }
    }

    if selected_groups.len() >= 2 {
        let mut member_group_counts: HashMap<String, usize> = HashMap::new();
        for link in &relation_network.graph.links {
            let member_id = if selected_groups.contains(&link.source) {
                &link.target
            } else if selected_groups.contains(&link.target) {
                &link.source
            } else {
                continue;
            };
            let Some(member_node) = relation_node_by_id(&relation_network.graph, member_id) else {
                continue;
            };
            if member_node.kind == RelationNodeKind::Group
                || !relation_network.options.allows(member_node.kind)
            {
                continue;
            }
            *member_group_counts.entry(member_id.clone()).or_default() += 1;
        }
        for (member_id, group_count) in member_group_counts {
            if group_count >= 2 {
                visible_ids.insert(member_id);
            }
        }
    }

    if relation_network.selected_node_ids.len() > 1
        && let Some(self_node) = relation_network
            .graph
            .nodes
            .iter()
            .find(|node| node.kind == RelationNodeKind::SelfUser)
    {
        visible_ids.insert(self_node.id.clone());
    }

    relation_node_kind_ordered_ids(&relation_network.graph, visible_ids)
}

#[derive(Debug, Clone, Copy)]
struct RelationPerformanceLevel {
    index: usize,
    label: &'static str,
}

fn relation_performance_level(node_count: usize) -> RelationPerformanceLevel {
    match node_count {
        0..=100 => RelationPerformanceLevel {
            index: 0,
            label: "流畅",
        },
        101..=500 => RelationPerformanceLevel {
            index: 1,
            label: "良好",
        },
        501..=2_000 => RelationPerformanceLevel {
            index: 2,
            label: "标准",
        },
        2_001..=10_000 => RelationPerformanceLevel {
            index: 3,
            label: "性能",
        },
        _ => RelationPerformanceLevel {
            index: 4,
            label: "极速",
        },
    }
}

fn relation_node_by_id<'a>(graph: &'a RelationGraph, id: &str) -> Option<&'a RelationNode> {
    graph.nodes.iter().find(|node| node.id == id)
}

fn relation_link_visible_for_focus(
    graph: &RelationGraph,
    link: &RelationLink,
    focused: &str,
    focus_neighbors: &HashSet<&str>,
) -> bool {
    link.source == focused
        || link.target == focused
        || (focus_neighbors.contains(link.source.as_str())
            && focus_neighbors.contains(link.target.as_str())
            && relation_node_by_id(graph, &link.source)
                .zip(relation_node_by_id(graph, &link.target))
                .is_some_and(|(source, target)| {
                    source.kind == RelationNodeKind::Group || target.kind == RelationNodeKind::Group
                }))
}

fn relation_node_kind_ordered_ids(graph: &RelationGraph, ids: HashSet<String>) -> Vec<String> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    ids.sort_by(|left, right| {
        match (
            relation_node_by_id(graph, left),
            relation_node_by_id(graph, right),
        ) {
            (Some(left_node), Some(right_node)) => node_kind_order(left_node.kind)
                .cmp(&node_kind_order(right_node.kind))
                .then_with(|| right_node.value.cmp(&left_node.value))
                .then_with(|| left_node.name.cmp(&right_node.name)),
            _ => left.cmp(right),
        }
    });
    ids
}

fn relation_node_ids_matching_options(
    relation_network: &RelationNetworkState,
    query: &str,
) -> HashSet<String> {
    relation_network
        .graph
        .nodes
        .iter()
        .filter(|node| relation_network.options.allows(node.kind) && node.matches_query(query))
        .map(|node| node.id.clone())
        .collect()
}

fn relation_visible_ids_default(
    relation_network: &RelationNetworkState,
    query: &str,
) -> Vec<String> {
    relation_node_kind_ordered_ids(
        &relation_network.graph,
        relation_node_ids_matching_options(relation_network, query),
    )
}

fn relation_visible_ids_from_focus(
    relation_network: &RelationNetworkState,
    focused: &str,
    query: &str,
) -> Vec<String> {
    let mut allowed = relation_node_ids_matching_options(relation_network, query);
    allowed.insert(focused.to_string());
    let neighbors: HashSet<_> = relation_neighbors(&relation_network.graph, focused)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    allowed.retain(|id| id == focused || neighbors.contains(id));
    relation_node_kind_ordered_ids(&relation_network.graph, allowed)
}

fn relation_view_limit(relation_network: &RelationNetworkState) -> usize {
    if matches!(relation_network.view_mode, RelationViewMode::Focused(_)) {
        relation_network.render_setting.max_visible_nodes_focused
    } else {
        relation_network.render_setting.max_visible_nodes
    }
}

fn relation_drawn_link_limit(relation_network: &RelationNetworkState) -> usize {
    if matches!(relation_network.view_mode, RelationViewMode::Focused(_)) {
        relation_network.render_setting.max_drawn_links_focused
    } else {
        relation_network.render_setting.max_drawn_links
    }
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

    let usable_rect = rect.shrink2(egui::vec2(24.0, 24.0));
    let center = usable_rect.center();
    let max_radius = usable_rect.width().min(usable_rect.height()) * 0.24;
    let mut rings: [Vec<&RelationNode>; 5] = std::array::from_fn(|_| Vec::new());
    let visible_set: HashSet<&str> = visible_ids.iter().map(String::as_str).collect();
    for node in &graph.nodes {
        if visible_set.contains(node.id.as_str()) {
            rings[node_kind_order(node.kind) as usize].push(node);
        }
    }

    place_radial_ring(&mut positions, &rings[0], center, 0.0, 0.0);
    place_radial_ring(&mut positions, &rings[1], center, max_radius * 0.70, 0.21);
    place_radial_ring(&mut positions, &rings[2], center, max_radius * 1.10, 0.53);
    place_radial_ring(&mut positions, &rings[3], center, max_radius * 1.45, 0.89);

    if !rings[4].is_empty() {
        place_group_grid(&mut positions, &rings[4], usable_rect);
    }

    positions
}

fn place_radial_ring(
    positions: &mut HashMap<String, egui::Pos2>,
    nodes: &[&RelationNode],
    center: egui::Pos2,
    radius: f32,
    phase: f32,
) {
    match nodes.len() {
        0 => {}
        1 => {
            positions.insert(nodes[0].id.clone(), center);
        }
        len => {
            let angle_step = std::f32::consts::TAU / len as f32;
            for (idx, node) in nodes.iter().enumerate() {
                let angle = idx as f32 * angle_step + phase;
                positions.insert(
                    node.id.clone(),
                    egui::pos2(
                        center.x + angle.cos() * radius,
                        center.y + angle.sin() * radius,
                    ),
                );
            }
        }
    }
}

fn place_group_grid(
    positions: &mut HashMap<String, egui::Pos2>,
    nodes: &[&RelationNode],
    rect: egui::Rect,
) {
    let count = nodes.len().max(1);
    let aspect = (rect.width() / rect.height().max(1.0)).max(0.25);
    let cols = ((count as f32 * aspect).sqrt().ceil() as usize).max(1);
    let rows = count.div_ceil(cols).max(1);
    let x_step = rect.width() / cols as f32;
    let y_step = rect.height() / rows as f32;

    for (idx, node) in nodes.iter().enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let stagger = if row % 2 == 0 { 0.0 } else { x_step * 0.35 };
        let x = rect.left() + (col as f32 + 0.5) * x_step + stagger;
        let y = rect.top() + (row as f32 + 0.5) * y_step;
        positions.insert(
            node.id.clone(),
            egui::pos2(
                x.clamp(rect.left() + 6.0, rect.right() - 6.0),
                y.clamp(rect.top() + 6.0, rect.bottom() - 6.0),
            ),
        );
    }
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
