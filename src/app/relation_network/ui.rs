use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cfg::{self, RelationNetworkSetting};
use crate::ica::IcaCommand;
use tokio::sync::mpsc::UnboundedSender;

const RELATION_MEMBER_LOAD_CONCURRENCY: usize = 12;
const RELATION_REBUILD_GROUP_STEP: usize = 12;

use super::super::IcaApp;
use super::super::state::BridgeState;
use super::layout::*;
use super::model::*;
use super::theme::RelationTheme;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RelationViewMode {
    #[default]
    Default,
    Focused(String),
    MultiSelect,
    MultiSelectRelationship,
}

#[derive(Debug, Clone, Default)]
pub(super) struct RelationLayoutCache {
    pub(super) view_key: u64,
    pub(super) visible_ids: Vec<String>,
    pub(super) visible_node_indices: Vec<usize>,
    pub(super) visible_link_indices: Vec<usize>,
    pub(super) unit_positions: HashMap<String, egui::Vec2>,
    /// 力导向计算使用的节点速度；键与 `unit_positions` 中的节点 ID 一致。
    ///
    /// 速度需要跨帧保存，否则每帧都会从静止状态重新开始，布局会显得僵硬且难以收敛。
    pub(super) velocities: HashMap<String, egui::Vec2>,
    /// 当前聚焦视图还需要执行的力导向步数；降为零后停止持续重绘。
    pub(super) force_ticks_remaining: u16,
}

#[derive(Debug, Clone)]
pub struct RelationNetworkState {
    include_unloaded_groups: bool,
    show_labels: bool,
    pub(super) options: RelationGraphOptions,
    search_query: String,
    group_search_query: String,
    pub(super) focused_node_id: Option<String>,
    selected_node_id: Option<String>,
    hovered_node_id: Option<String>,
    pub(super) selected_node_ids: HashSet<String>,
    pub(super) view_mode: RelationViewMode,
    canvas_zoom: f32,
    canvas_pan: egui::Vec2,
    pub(super) graph_revision: u64,
    pub(super) layout_cache: RelationLayoutCache,
    pub(super) graph: RelationGraph,
    closed: bool,
    pending_load_limit: Option<Option<usize>>,
    pending_rebuild: bool,
    load_all_active: bool,
    load_started_at: Option<Instant>,
    load_start_loaded_groups: usize,
    load_last_rebuild_loaded_groups: usize,
    pub(super) render_setting: RelationNetworkSetting,
}

impl Default for RelationNetworkState {
    fn default() -> Self {
        Self {
            include_unloaded_groups: true,
            show_labels: true,
            options: RelationGraphOptions::default(),
            search_query: String::new(),
            group_search_query: String::new(),
            focused_node_id: None,
            selected_node_id: None,
            hovered_node_id: None,
            selected_node_ids: HashSet::new(),
            view_mode: RelationViewMode::Default,
            canvas_zoom: 1.0,
            canvas_pan: egui::Vec2::ZERO,
            graph_revision: 0,
            layout_cache: RelationLayoutCache::default(),
            graph: RelationGraph::default(),
            closed: false,
            pending_load_limit: None,
            pending_rebuild: false,
            load_all_active: false,
            load_started_at: None,
            load_start_loaded_groups: 0,
            load_last_rebuild_loaded_groups: 0,
            render_setting: RelationNetworkSetting::default(),
        }
    }
}

impl RelationNetworkState {
    pub fn with_render_setting(mut self, render_setting: RelationNetworkSetting) -> Self {
        self.render_setting = render_setting;
        self
    }

    fn replace_graph(&mut self, graph: RelationGraph) {
        let should_reset_view = self.graph.nodes.is_empty();
        self.graph = graph;
        self.graph_revision = self.graph_revision.wrapping_add(1);
        self.layout_cache = RelationLayoutCache::default();
        if should_reset_view {
            reset_relation_canvas_view(self);
        }
    }
}

impl IcaApp {
    pub fn render_relation_network_window(&mut self, ctx: &egui::Context) {
        if !self.open_page.relation_network {
            return;
        }
        self.sync_relation_network_render_setting();

        let relation_network = self.relation_network.clone();
        let parent_viewport_id = ctx.viewport_id();
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
                    pending_groups: state
                        .rooms
                        .iter()
                        .filter(|room| room.room_id < 0)
                        .map(|room| room.room_id)
                        .filter(|room_id| {
                            !state.group_members_by_room.contains_key(room_id)
                                && !state.loading_group_members.contains(room_id)
                        })
                        .count(),
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

                let theme = RelationTheme::from_visuals(&viewport_ctx.style().visuals);
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme.page_bg))
                    .show(viewport_ctx, |ui| {
                        render_relation_network_ui(ui, &relation_network, bridge_snapshot.as_ref());
                    });

                // 独立关系网使用 deferred 子视口，按钮点击发生在主视口完成本帧动作消费之后。
                // 因此这里检测子视口刚刚写入的加载/刷新动作，并主动唤醒父视口。
                // 如果只请求子视口重绘，主应用不会消费动作，表象就是必须关闭独立窗口才开始加载。
                let parent_action_pending = {
                    let state = relation_network.lock().unwrap();
                    state.pending_rebuild || state.pending_load_limit.is_some()
                };
                if parent_action_pending {
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                }
            },
        );

        self.apply_relation_network_viewport_actions();
    }

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
        let graph = RelationGraphBuilder::build(
            login_user_id,
            &state.rooms,
            &state.group_members_by_room,
            include_unloaded_groups,
        );
        let node_count = graph.nodes.len();
        let link_count = graph.links.len();
        let loaded_groups = graph.loaded_group_count;
        let total_groups = graph.total_group_count;
        self.relation_network.lock().unwrap().replace_graph(graph);
        self.apply_relation_network_auto_degrade();
        tracing::info!(
            node_count,
            link_count,
            loaded_groups,
            total_groups,
            include_unloaded_groups,
            elapsed_ms = started_at.elapsed().as_millis(),
            "relation network graph rebuilt"
        );
    }

    pub fn request_relation_network_members(&mut self, limit: Option<usize>) {
        if limit.is_none() {
            let Some(bridge_idx) = self.active_bridge_idx else {
                return;
            };
            let loaded_groups = self.bridge_states[bridge_idx]
                .group_members_by_room
                .keys()
                .filter(|room_id| **room_id < 0)
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
            tracing::info!(
                loaded_groups,
                total_groups,
                concurrency = RELATION_MEMBER_LOAD_CONCURRENCY,
                "relation network group-member load started"
            );
            self.continue_relation_network_member_loading();
            return;
        }

        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let queued = request_relation_network_members_with_tx(
            state,
            &self.ica_clients[bridge_idx].command_tx,
            limit,
        );
        tracing::info!(queued, limit = ?limit, "relation network group-member batch queued");
    }

    pub fn refresh_relation_network_after_bridge_update(&mut self, bridge_idx: usize) {
        if Some(bridge_idx) != self.active_bridge_idx {
            return;
        }
        let load_all_active = self.relation_network.lock().unwrap().load_all_active;
        let loaded_groups = self.bridge_states[bridge_idx]
            .group_members_by_room
            .keys()
            .filter(|room_id| **room_id < 0)
            .count();
        let pending_groups = self.bridge_states[bridge_idx]
            .rooms
            .iter()
            .filter(|room| {
                room.room_id < 0
                    && !self.bridge_states[bridge_idx]
                        .group_members_by_room
                        .contains_key(&room.room_id)
                    && !self.bridge_states[bridge_idx]
                        .loading_group_members
                        .contains(&room.room_id)
            })
            .count();
        let loading_groups = self.bridge_states[bridge_idx]
            .loading_group_members
            .iter()
            .filter(|room_id| **room_id < 0)
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

    fn continue_relation_network_member_loading(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let state = &mut self.bridge_states[bridge_idx];
        let loading_groups = state
            .loading_group_members
            .iter()
            .filter(|room_id| **room_id < 0)
            .count();
        let pending_groups = state
            .rooms
            .iter()
            .filter(|room| {
                room.room_id < 0
                    && !state.group_members_by_room.contains_key(&room.room_id)
                    && !state.loading_group_members.contains(&room.room_id)
            })
            .count();

        if pending_groups == 0 && loading_groups == 0 {
            let (started_at, start_loaded_groups) = {
                let mut relation_network = self.relation_network.lock().unwrap();
                relation_network.load_all_active = false;
                (
                    relation_network.load_started_at.take(),
                    relation_network.load_start_loaded_groups,
                )
            };
            let loaded_groups = state
                .group_members_by_room
                .keys()
                .filter(|room_id| **room_id < 0)
                .count();
            tracing::info!(
                loaded_groups,
                newly_loaded = loaded_groups.saturating_sub(start_loaded_groups),
                elapsed_ms = started_at.map(|started| started.elapsed().as_millis()),
                "relation network group-member load finished"
            );
            return;
        }

        let available_slots = RELATION_MEMBER_LOAD_CONCURRENCY.saturating_sub(loading_groups);
        if available_slots == 0 {
            return;
        }
        let queued = request_relation_network_members_with_tx(
            state,
            &self.ica_clients[bridge_idx].command_tx,
            Some(available_slots),
        );
        tracing::debug!(
            queued,
            loading_groups,
            pending_groups,
            "relation network group-member load refilled"
        );
    }

    fn apply_relation_network_auto_degrade(&mut self) {
        let mut relation_network = self.relation_network.lock().unwrap();
        let node_count = relation_network.graph.nodes.len();
        let render_setting = relation_network.render_setting.clone();
        if node_count > render_setting.auto_hide_labels_node_threshold {
            if relation_network.show_labels {
                tracing::info!(node_count, "relation network auto-disabled labels");
            }
            relation_network.show_labels = false;
        }
        if node_count > render_setting.auto_hide_acquaintance_node_threshold {
            if relation_network.options.show_acquaintances {
                tracing::info!(node_count, "relation network auto-hidden acquaintances");
            }
            relation_network.options.show_acquaintances = false;
        }
        if node_count > render_setting.auto_hide_stranger_node_threshold {
            if relation_network.options.show_strangers {
                tracing::info!(node_count, "relation network auto-hidden strangers");
            }
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
) {
    ui.scope(|ui| {
        ui.style_mut().visuals.widgets.hovered.expansion = 0.0;
        ui.style_mut().visuals.widgets.active.expansion = 0.0;
        ui.style_mut().visuals.widgets.open.expansion = 0.0;
        ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 8.0);
        let theme = RelationTheme::from_ui(ui);

        let Some(bridge_snapshot) = bridge_snapshot else {
            render_relation_network_empty_state(ui);
            return;
        };

        let mut relation_network = relation_network.lock().unwrap();

        egui::Panel::top("relation_network_header")
            .exact_size(66.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme.surface)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .inner_margin(egui::Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| {
                render_relation_network_header(ui, &mut relation_network, bridge_snapshot);
            });

        if relation_network.load_all_active || bridge_snapshot.loading_groups > 0 {
            egui::Panel::top("relation_network_progress")
                .exact_size(38.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme.surface)
                        .inner_margin(egui::Margin::symmetric(18, 7)),
                )
                .show(ui, |ui| {
                    let total = relation_network.graph.total_group_count.max(1);
                    let loaded = bridge_snapshot.loaded_groups.min(total);
                    ui.add(
                        egui::ProgressBar::new(loaded as f32 / total as f32)
                            .desired_width(ui.available_width())
                            .text(format!(
                                "群成员加载 {loaded} / {total}  ·  {} 加载中  ·  {} 等待中",
                                bridge_snapshot.loading_groups, bridge_snapshot.pending_groups
                            )),
                    );
                });
        }

        egui::Panel::left("relation_network_sidebar_panel")
            .exact_size(284.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme.surface)
                    .stroke(egui::Stroke::new(1.0, theme.border)),
            )
            .show(ui, |ui| {
                render_relation_network_sidebar(ui, &mut relation_network, bridge_snapshot);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.page_bg)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ui, |ui| {
                render_relation_network_canvas(ui, &mut relation_network);
            });
    });
}

fn render_relation_network_empty_state(ui: &mut egui::Ui) {
    let theme = RelationTheme::from_ui(ui);
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme.page_bg)
                .inner_margin(egui::Margin::same(24)),
        )
        .show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                egui::Frame::new()
                    .fill(theme.surface)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(32, 24))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("QQ 关系网暂不可用");
                            ui.add_space(4.0);
                            ui.weak("请先连接并启用一个 bridge，然后重新打开关系网。");
                        });
                    });
            });
        });
}

fn render_relation_network_header(
    ui: &mut egui::Ui,
    relation_network: &mut RelationNetworkState,
    bridge_snapshot: &RelationBridgeSnapshot,
) {
    let theme = RelationTheme::from_ui(ui);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("QQ 关系网可视化")
                    .size(18.0)
                    .strong()
                    .color(theme.text),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} 个会话  ·  {} 个节点  ·  {} 条连线",
                    bridge_snapshot.rooms_len,
                    relation_network.graph.nodes.len(),
                    relation_network.graph.links.len()
                ))
                .size(11.0)
                .color(theme.muted),
            );
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if relation_toolbar_button(ui, "加载全部群成员", true).clicked() {
                queue_relation_member_requests(relation_network, None);
            }
            if relation_toolbar_button(ui, "刷新", false).clicked() {
                relation_network.pending_rebuild = true;
            }

            let multi_label = match relation_network.view_mode {
                RelationViewMode::MultiSelectRelationship => "退出关系视图",
                RelationViewMode::MultiSelect => "查看关系",
                _ => "多选模式",
            };
            if relation_toolbar_button(ui, multi_label, false).clicked() {
                toggle_relation_multi_select(relation_network);
            }

            let performance = relation_performance_level(relation_network.graph.nodes.len());
            relation_status_badge(
                ui,
                &format!("性能: {}", performance.label),
                theme.success_fill,
                theme.success_text,
            );
            relation_status_badge(ui, "已连接", theme.success_fill, theme.success_text);
        });
    });
}

fn relation_toolbar_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let theme = RelationTheme::from_ui(ui);
    let (fill, stroke, text_color) = if primary {
        (
            ui.visuals().selection.bg_fill,
            ui.visuals().selection.bg_fill,
            ui.visuals().selection.stroke.color,
        )
    } else {
        (theme.button_fill, theme.button_border, theme.text)
    };

    ui.add(
        egui::Button::new(egui::RichText::new(label).color(text_color).size(13.0))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(egui::vec2(0.0, 32.0)),
    )
}

fn relation_status_badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32, color: egui::Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(16))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 4.0, color);
                ui.label(egui::RichText::new(text).size(12.0).color(color));
            });
        });
}

fn render_relation_network_sidebar(
    ui: &mut egui::Ui,
    relation_network: &mut RelationNetworkState,
    bridge_snapshot: &RelationBridgeSnapshot,
) {
    let theme = RelationTheme::from_ui(ui);
    egui::ScrollArea::vertical()
        .id_salt("relation_network_sidebar")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add_space(12.0);

            relation_sidebar_card(ui, "数据统计", |ui| {
                render_relation_stats(ui, relation_network);
                ui.add_space(4.0);
                let total_groups = relation_network.graph.total_group_count.max(1);
                let loaded_groups = bridge_snapshot.loaded_groups.min(total_groups);
                ui.add(
                    egui::ProgressBar::new(loaded_groups as f32 / total_groups as f32)
                        .desired_width(ui.available_width())
                        .text(format!("群成员 {loaded_groups} / {total_groups}")),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} 加载中  ·  {} 等待中",
                        bridge_snapshot.loading_groups, bridge_snapshot.pending_groups
                    ))
                    .size(11.0)
                    .color(theme.muted),
                );
                if relation_network.graph.loaded_group_count != bridge_snapshot.loaded_groups {
                    ui.colored_label(theme.warning, "图谱将在下一批完成后刷新");
                }
            });

            relation_sidebar_card(ui, "显示选项", |ui| {
                render_relation_filter_options(ui, relation_network);
                ui.separator();
                ui.checkbox(&mut relation_network.show_labels, "显示节点标签");
            });

            relation_sidebar_card(ui, "查找节点", |ui| {
                ui.add_sized(
                    [ui.available_width(), 30.0],
                    egui::TextEdit::singleline(&mut relation_network.search_query)
                        .hint_text("昵称 / QQ / 群号"),
                );
                if !relation_network.search_query.is_empty()
                    && ui.small_button("清除搜索").clicked()
                {
                    relation_network.search_query.clear();
                }
            });

            relation_sidebar_card(ui, "群列表", |ui| {
                render_relation_group_list(ui, relation_network);
                ui.separator();
                ui.checkbox(
                    &mut relation_network.include_unloaded_groups,
                    "显示未加载成员的群",
                );
                ui.horizontal(|ui| {
                    if ui.small_button("应用范围").clicked() {
                        relation_network.pending_rebuild = true;
                    }
                    if ui.small_button("加载 10 个群").clicked() {
                        queue_relation_member_requests(relation_network, Some(10));
                    }
                });
            });

            if relation_network.view_mode != RelationViewMode::Default
                || relation_network.selected_node_id.is_some()
            {
                relation_sidebar_card(ui, "当前视图", |ui| {
                    render_relation_view_hint(ui, relation_network);
                    if ui.small_button("返回完整关系网").clicked() {
                        clear_relation_selection(relation_network);
                    }
                    if let Some(node_id) = &relation_network.selected_node_id
                        && let Some(node) = relation_node_by_id(&relation_network.graph, node_id)
                    {
                        ui.separator();
                        render_relation_node_detail(ui, node);
                    }
                });
            }

            relation_sidebar_card(ui, "节点大小说明", render_relation_size_legend);
            ui.add_space(12.0);
        });
}

fn relation_sidebar_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let theme = RelationTheme::from_ui(ui);
    egui::Frame::new()
        .fill(theme.surface)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(title)
                    .size(14.0)
                    .strong()
                    .color(theme.text),
            );
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(2.0);
            add_contents(ui);
        });
}

fn render_relation_group_list(ui: &mut egui::Ui, relation_network: &mut RelationNetworkState) {
    ui.add_sized(
        [ui.available_width(), 28.0],
        egui::TextEdit::singleline(&mut relation_network.group_search_query).hint_text("搜索群"),
    );

    let query = relation_network.group_search_query.trim().to_lowercase();
    let selected_id = relation_network.selected_node_id.as_deref();
    let mut clicked_node_id = None;

    egui::ScrollArea::vertical()
        .id_salt("relation_network_group_list")
        .max_height(220.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let mut displayed = 0usize;
            for &node_index in &relation_network.graph.group_node_indices {
                let Some(node) = relation_network.graph.nodes.get(node_index) else {
                    continue;
                };
                if !node.matches_query(&query) {
                    continue;
                }
                displayed += 1;
                if displayed > 200 {
                    break;
                }

                let selected = selected_id == Some(node.id.as_str());
                let response = ui
                    .horizontal(|ui| {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            5.0,
                            RelationNodeKind::Group.color(),
                        );
                        let label = if let Some(member_count) = node.member_count {
                            format!("{}  ·  {member_count}", node.name)
                        } else if node.value > 0 {
                            format!("{}  ·  {}", node.name, node.value)
                        } else {
                            node.name.clone()
                        };
                        ui.selectable_label(selected, label)
                    })
                    .inner;
                if response.clicked() {
                    clicked_node_id = Some(node.id.clone());
                }
            }

            if displayed == 0 {
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.weak("没有匹配的群");
                    ui.add_space(12.0);
                });
            }
        });

    if let Some(node_id) = clicked_node_id {
        handle_relation_node_click(relation_network, node_id);
    }
}

fn render_relation_stats(ui: &mut egui::Ui, relation_network: &RelationNetworkState) {
    let counts = relation_network.graph.node_counts();
    egui::Grid::new("relation_stats_grid")
        .num_columns(2)
        .spacing([8.0, 8.0])
        .show(ui, |ui| {
            relation_stat_card(ui, "好友数", counts.friend);
            relation_stat_card(ui, "群数", counts.group);
            ui.end_row();
            relation_stat_card(ui, "总节点", relation_network.graph.nodes.len());
            relation_stat_card(ui, "总边数", relation_network.graph.links.len());
            ui.end_row();
        });
}

fn relation_stat_card(ui: &mut egui::Ui, label: &str, value: usize) {
    let theme = RelationTheme::from_ui(ui);
    egui::Frame::NONE
        .fill(theme.surface_alt)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_min_width(82.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(label).size(11.0).color(theme.muted));
                ui.label(
                    egui::RichText::new(value.to_string())
                        .size(18.0)
                        .strong()
                        .color(theme.text),
                );
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
    let theme = RelationTheme::from_ui(ui);
    ui.horizontal(|ui| {
        ui.checkbox(enabled, "");
        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(dot_rect.center(), 6.0, kind.color());
        ui.label(kind.label());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(count.to_string())
                    .size(11.0)
                    .color(theme.subtle),
            );
        });
    });
}

fn render_relation_view_hint(ui: &mut egui::Ui, relation_network: &RelationNetworkState) {
    let accent = ui.visuals().hyperlink_color;
    match &relation_network.view_mode {
        RelationViewMode::Focused(node_id) => {
            if let Some(node) = relation_node_by_id(&relation_network.graph, node_id) {
                ui.colored_label(accent, format!("一级关系网: {}", node.name));
            }
        }
        RelationViewMode::MultiSelect => {
            ui.colored_label(
                accent,
                format!("已选择 {} 个节点", relation_network.selected_node_ids.len()),
            );
        }
        RelationViewMode::MultiSelectRelationship => {
            ui.colored_label(
                accent,
                format!(
                    "关系视图: {} 个节点",
                    relation_network.selected_node_ids.len()
                ),
            );
        }
        RelationViewMode::Default => {}
    }
}

fn clear_relation_selection(relation_network: &mut RelationNetworkState) {
    relation_network.focused_node_id = None;
    relation_network.selected_node_id = None;
    relation_network.hovered_node_id = None;
    relation_network.selected_node_ids.clear();
    relation_network.view_mode = RelationViewMode::Default;
    reset_relation_canvas_view(relation_network);
}

fn reset_relation_canvas_view(relation_network: &mut RelationNetworkState) {
    relation_network.canvas_zoom = 1.0;
    relation_network.canvas_pan = egui::Vec2::ZERO;
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
    let theme = RelationTheme::from_ui(ui);
    for (label, radius, border) in [
        ("少量关联 (<10)", 5.0, 0.0),
        ("数十 / 百人", 8.0, 2.0),
        ("千人群", 11.0, 3.0),
        ("万人群", 14.0, 4.0),
    ] {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 24.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), radius, RelationNodeKind::Group.color());
            if border > 0.0 {
                ui.painter().circle_stroke(
                    rect.center(),
                    radius,
                    egui::Stroke::new(border, theme.node_outline),
                );
            }
            ui.label(egui::RichText::new(label).size(11.0).color(theme.muted));
        });
    }
}

fn render_relation_network_canvas(ui: &mut egui::Ui, relation_network: &mut RelationNetworkState) {
    let theme = RelationTheme::from_ui(ui);
    let available = ui.available_size();
    let size = egui::vec2(available.x.max(320.0), available.y.max(280.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, theme.canvas);
    painter.rect_stroke(
        rect,
        10.0,
        egui::Stroke::new(1.0, theme.border),
        egui::StrokeKind::Inside,
    );

    if response.dragged_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        relation_network.canvas_pan += response.drag_delta();
    }
    if response.hovered() {
        response.clone().on_hover_cursor(egui::CursorIcon::Grab);
        let (gesture_zoom, scroll_delta, pointer) = ui.input(|input| {
            (
                input.zoom_delta(),
                input.smooth_scroll_delta().y,
                input.pointer.latest_pos(),
            )
        });
        let wheel_zoom = (scroll_delta * 0.0025).exp();
        let zoom_factor = gesture_zoom * wheel_zoom;
        if (zoom_factor - 1.0).abs() > f32::EPSILON {
            let anchor = pointer.unwrap_or(rect.center());
            set_relation_canvas_zoom(
                relation_network,
                relation_network.canvas_zoom * zoom_factor,
                anchor,
                rect,
            );
        }
    }

    render_relation_canvas_grid(&painter, rect, relation_network, theme.grid);

    let query = relation_network.search_query.trim().to_lowercase();
    let view_key = relation_view_cache_key(relation_network, &query);
    if relation_network.layout_cache.view_key != view_key {
        let visible = visible_relation_node_ids(relation_network, &query);
        relation_network.layout_cache =
            build_relation_layout_cache(relation_network, view_key, visible);
    }
    if advance_relation_force_layout(relation_network) {
        // 力导向布局尚未收敛时按约 60 FPS 请求下一帧；步数耗尽后不再产生额外重绘。
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    if relation_network.layout_cache.visible_ids.is_empty() {
        let toolbar_clicked = render_relation_canvas_controls(ui, rect, relation_network);
        painter.text(
            rect.center() - egui::vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            if query.is_empty() {
                "暂无关系数据\n先加载群成员，或等待会话数据同步"
            } else {
                "没有匹配的节点\n请更换搜索关键词或显示选项"
            },
            egui::FontId::proportional(14.0),
            theme.muted,
        );
        if response.clicked() && !toolbar_clicked {
            exit_relation_focus_or_multiselect(relation_network);
        }
        return;
    }

    let canvas_transform = RelationCanvasTransform::new(
        rect,
        relation_network.canvas_zoom,
        relation_network.canvas_pan,
    );
    let performance = relation_performance_level(relation_network.layout_cache.visible_ids.len());
    let line_opacity = match performance.index {
        0 => 0.30,
        1 => 0.20,
        2 => 0.055,
        3 => 0.032,
        _ => 0.018,
    };

    for &link_index in &relation_network.layout_cache.visible_link_indices {
        let Some(link) = relation_network.graph.links.get(link_index) else {
            continue;
        };
        let Some(source) = relation_network
            .layout_cache
            .unit_positions
            .get(&link.source)
            .copied()
            .map(|position| canvas_transform.position(position))
        else {
            continue;
        };
        let Some(target) = relation_network
            .layout_cache
            .unit_positions
            .get(&link.target)
            .copied()
            .map(|position| canvas_transform.position(position))
        else {
            continue;
        };
        let is_hovered_link = relation_network
            .hovered_node_id
            .as_deref()
            .is_some_and(|hovered| link.source == hovered || link.target == hovered);
        painter.line_segment(
            [source, target],
            egui::Stroke::new(
                if is_hovered_link {
                    2.2
                } else if performance.index <= 1 {
                    1.0
                } else {
                    0.8
                },
                if is_hovered_link {
                    theme.edge_hover.linear_multiply(0.75)
                } else {
                    theme.edge.linear_multiply(line_opacity)
                },
            ),
        );
    }
    let drawn_links = relation_network.layout_cache.visible_link_indices.len();

    let pointer_pos = response.hover_pos();
    let mut hovered_node_id = None;
    for &node_index in &relation_network.layout_cache.visible_node_indices {
        let Some(node) = relation_network.graph.nodes.get(node_index) else {
            continue;
        };
        let Some(pos) = relation_network
            .layout_cache
            .unit_positions
            .get(&node.id)
            .copied()
            .map(|position| canvas_transform.position(position))
        else {
            continue;
        };
        let is_selected = relation_network.selected_node_id.as_deref() == Some(node.id.as_str());
        let is_multi_selected = relation_network.selected_node_ids.contains(&node.id);
        let base_radius =
            relation_node_draw_radius(node, performance, relation_network.canvas_zoom);
        let radius =
            if is_multi_selected && relation_network.view_mode == RelationViewMode::MultiSelect {
                base_radius * 1.18
            } else {
                base_radius
            };
        let color = node.kind.color();
        let has_multi_selection = !relation_network.selected_node_ids.is_empty();
        let fill = if relation_network.view_mode == RelationViewMode::MultiSelect
            && has_multi_selection
            && !is_multi_selected
        {
            color.linear_multiply(0.48)
        } else {
            color
        };
        let stroke = if is_selected || is_multi_selected {
            egui::Stroke::new(2.5, theme.node_outline)
        } else if node.kind == RelationNodeKind::Group {
            egui::Stroke::new((0.7 + node.size_level as f32).min(2.5), theme.node_outline)
        } else if node.kind == RelationNodeKind::Friend {
            egui::Stroke::new(0.7, theme.node_outline)
        } else {
            egui::Stroke::new(1.0, theme.canvas)
        };
        if performance.index <= 1 {
            painter.circle_filled(pos + egui::vec2(1.5, 2.0), radius + 1.5, theme.shadow);
        }
        painter.circle_filled(pos, radius, fill);
        painter.circle_stroke(pos, radius, stroke);

        if relation_network.show_labels
            && relation_network.layout_cache.visible_ids.len()
                <= relation_network.render_setting.max_labels
        {
            let label = if relation_network.view_mode == RelationViewMode::MultiSelect
                && !is_multi_selected
            {
                ""
            } else {
                &node.name
            };
            painter.text(
                pos + egui::vec2(radius + 3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(if performance.index == 0 { 11.0 } else { 10.0 }),
                theme.canvas_text,
            );
        }

        if pointer_pos.is_some_and(|pointer| pointer.distance(pos) <= radius + 3.0) {
            hovered_node_id = Some(node.id.clone());
        }
    }
    relation_network.hovered_node_id = hovered_node_id.clone();

    render_relation_network_overlay(
        ui,
        relation_network,
        rect,
        &relation_network.layout_cache.visible_ids,
        drawn_links,
        performance,
    );

    let toolbar_clicked = render_relation_canvas_controls(ui, rect, relation_network);
    if response.clicked() && !toolbar_clicked {
        if let Some(node_id) = hovered_node_id {
            handle_relation_node_click(relation_network, node_id);
        } else {
            exit_relation_focus_or_multiselect(relation_network);
        }
    }

    if let Some(node_id) = relation_network.hovered_node_id.clone()
        && let Some(node) = relation_node_by_id(&relation_network.graph, &node_id)
    {
        render_relation_node_popup(ui, rect, pointer_pos.unwrap_or(rect.center()), node);
    }
}

fn render_relation_canvas_grid(
    painter: &egui::Painter,
    rect: egui::Rect,
    relation_network: &RelationNetworkState,
    color: egui::Color32,
) {
    let spacing = (36.0 * relation_network.canvas_zoom).clamp(18.0, 72.0);
    let offset_x = relation_network.canvas_pan.x.rem_euclid(spacing);
    let offset_y = relation_network.canvas_pan.y.rem_euclid(spacing);
    let mut x = rect.left() + offset_x;
    while x < rect.right() {
        let mut y = rect.top() + offset_y;
        while y < rect.bottom() {
            painter.circle_filled(egui::pos2(x, y), 0.8, color);
            y += spacing;
        }
        x += spacing;
    }
}

fn render_relation_canvas_controls(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    relation_network: &mut RelationNetworkState,
) -> bool {
    let theme = RelationTheme::from_ui(ui);
    let button_size = egui::vec2(34.0, 30.0);
    let right = rect.right() - 12.0;
    let bottom = rect.bottom() - 12.0;
    let button = |label: &str| {
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.0)
                .color(theme.control_text),
        )
        .fill(theme.surface)
        .stroke(egui::Stroke::new(1.0, theme.button_border))
        .corner_radius(egui::CornerRadius::same(6))
    };

    let fit_rect = egui::Rect::from_min_size(
        egui::pos2(right - 82.0, bottom - button_size.y),
        egui::vec2(82.0, button_size.y),
    );
    let zoom_out_rect = egui::Rect::from_min_size(
        egui::pos2(right - 82.0, bottom - button_size.y * 2.0 - 6.0),
        button_size,
    );
    let zoom_in_rect = egui::Rect::from_min_size(
        egui::pos2(right - 34.0, bottom - button_size.y * 2.0 - 6.0),
        button_size,
    );

    let zoom_out = ui.put(zoom_out_rect, button("−")).clicked();
    let zoom_in = ui.put(zoom_in_rect, button("+")).clicked();
    let fit = ui.put(fit_rect, button("适应画布")).clicked();

    if zoom_out {
        set_relation_canvas_zoom(
            relation_network,
            relation_network.canvas_zoom / 1.25,
            rect.center(),
            rect,
        );
    }
    if zoom_in {
        set_relation_canvas_zoom(
            relation_network,
            relation_network.canvas_zoom * 1.25,
            rect.center(),
            rect,
        );
    }
    if fit {
        relation_network.canvas_zoom = 1.0;
        relation_network.canvas_pan = egui::Vec2::ZERO;
    }

    zoom_out || zoom_in || fit
}

fn set_relation_canvas_zoom(
    relation_network: &mut RelationNetworkState,
    zoom: f32,
    anchor: egui::Pos2,
    rect: egui::Rect,
) {
    let previous = relation_network.canvas_zoom.max(0.01);
    let zoom = zoom.clamp(0.35, 4.0);
    let scale = zoom / previous;
    let anchor_from_center = anchor - rect.center();
    relation_network.canvas_pan =
        anchor_from_center - (anchor_from_center - relation_network.canvas_pan) * scale;
    relation_network.canvas_zoom = zoom;
}

fn relation_node_draw_radius(
    node: &RelationNode,
    performance: RelationPerformanceLevel,
    zoom: f32,
) -> f32 {
    let normalized = ((node.radius - 8.0) / 36.0).clamp(0.0, 1.0);
    let (min_radius, max_radius) = match performance.index {
        0 => (6.0, 28.0),
        1 => (5.0, 20.0),
        2 => (3.8, 13.0),
        3 => (3.0, 8.0),
        _ => (2.4, 5.5),
    };
    let kind_boost = if node.kind == RelationNodeKind::SelfUser {
        2.5
    } else {
        0.0
    };
    ((min_radius + (max_radius - min_radius) * normalized + kind_boost)
        * zoom.sqrt().clamp(0.75, 1.8))
    .max(2.0)
}

fn render_relation_network_overlay(
    ui: &mut egui::Ui,
    relation_network: &RelationNetworkState,
    rect: egui::Rect,
    visible: &[String],
    drawn_links: usize,
    performance: RelationPerformanceLevel,
) {
    let theme = RelationTheme::from_ui(ui);
    let painter = ui.painter_at(rect);
    let badge = format!(
        "显示 {} / {} 节点  ·  {} / {} 连线  ·  {}",
        visible.len(),
        relation_network.graph.nodes.len(),
        drawn_links,
        relation_network.graph.links.len(),
        performance.label
    );
    let galley = painter.layout_no_wrap(badge, egui::FontId::proportional(11.0), theme.muted);
    let badge_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(12.0, 12.0),
        galley.size() + egui::vec2(20.0, 10.0),
    );
    painter.rect_filled(badge_rect, 6.0, theme.overlay_fill);
    painter.rect_stroke(
        badge_rect,
        6.0,
        egui::Stroke::new(1.0, theme.overlay_border),
        egui::StrokeKind::Inside,
    );
    painter.galley(badge_rect.min + egui::vec2(10.0, 5.0), galley, theme.muted);

    painter.text(
        rect.left_bottom() + egui::vec2(14.0, -14.0),
        egui::Align2::LEFT_BOTTOM,
        format!(
            "拖动画布  ·  滚轮缩放  ·  当前 {}%",
            (relation_network.canvas_zoom * 100.0).round() as i32
        ),
        egui::FontId::proportional(11.0),
        theme.canvas_hint,
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
            egui::FontId::proportional(11.0),
            theme.canvas_hint,
        );
    }
}

fn render_relation_indicator(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
    let fill = ui.visuals().selection.bg_fill;
    let text_color = ui.visuals().selection.stroke.color;
    let painter = ui.painter_at(rect);
    let galley = painter.layout_no_wrap(
        text.to_string(),
        egui::TextStyle::Small.resolve(ui.style()),
        text_color,
    );
    let indicator_rect = egui::Rect::from_center_size(
        rect.center_top() + egui::vec2(0.0, 28.0),
        galley.size() + egui::vec2(28.0, 10.0),
    );
    painter.rect_filled(indicator_rect, 16.0, fill);
    painter.galley(
        indicator_rect.min + egui::vec2(14.0, 5.0),
        galley,
        text_color,
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
            reset_relation_canvas_view(relation_network);
        }
    }
}

fn exit_relation_focus_or_multiselect(relation_network: &mut RelationNetworkState) {
    match relation_network.view_mode {
        RelationViewMode::MultiSelectRelationship => {
            relation_network.view_mode = RelationViewMode::MultiSelect;
            reset_relation_canvas_view(relation_network);
        }
        RelationViewMode::MultiSelect => {}
        RelationViewMode::Focused(_) => {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
            relation_network.view_mode = RelationViewMode::Default;
            reset_relation_canvas_view(relation_network);
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
            reset_relation_canvas_view(relation_network);
        }
        RelationViewMode::MultiSelect => {
            if relation_network.selected_node_ids.len() >= 2 {
                relation_network.view_mode = RelationViewMode::MultiSelectRelationship;
                reset_relation_canvas_view(relation_network);
            }
        }
        RelationViewMode::Default | RelationViewMode::Focused(_) => {
            relation_network.focused_node_id = None;
            relation_network.selected_node_id = None;
            relation_network.selected_node_ids.clear();
            relation_network.view_mode = RelationViewMode::MultiSelect;
            reset_relation_canvas_view(relation_network);
        }
    }
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

#[derive(Debug, Clone)]
struct RelationBridgeSnapshot {
    rooms_len: usize,
    loaded_groups: usize,
    loading_groups: usize,
    pending_groups: usize,
}

fn queue_relation_member_requests(
    relation_network: &mut RelationNetworkState,
    limit: Option<usize>,
) {
    relation_network.pending_load_limit = Some(limit);
    tracing::debug!(limit = ?limit, "relation network member-load action queued");
}

fn request_relation_network_members_with_tx(
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
    queued
}

fn relation_login_user_id(state: &BridgeState) -> Option<i64> {
    (state.online_data.qqid > 0).then_some(state.online_data.qqid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_node(id: &str, kind: RelationNodeKind) -> RelationNode {
        RelationNode {
            id: id.to_string(),
            name: id.to_string(),
            kind,
            value: 1,
            radius: 8.0,
            size_level: 0,
            qq: None,
            group_id: None,
            member_count: None,
            common_group_count: 0,
            role: String::new(),
        }
    }

    fn test_graph(nodes: Vec<RelationNode>, links: Vec<RelationLink>) -> RelationGraph {
        let node_index = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let group_node_indices = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| (node.kind == RelationNodeKind::Group).then_some(index))
            .collect();
        RelationGraph {
            nodes,
            links,
            node_index,
            group_node_indices,
            node_counts: RelationNodeCounts::default(),
            loaded_group_count: 0,
            total_group_count: 0,
        }
    }

    #[test]
    fn topology_layout_clusters_member_around_its_group() {
        let graph = test_graph(
            vec![
                test_node("u:1", RelationNodeKind::Friend),
                test_node("g:1", RelationNodeKind::Group),
            ],
            vec![RelationLink {
                source: "g:1".to_string(),
                target: "u:1".to_string(),
            }],
        );
        let visible = vec!["u:1".to_string(), "g:1".to_string()];
        let positions = relation_unit_node_positions(&graph, &visible, &[0]);

        let member = positions["u:1"];
        let group = positions["g:1"];
        assert!((member - group).length() <= 0.131);
        assert_eq!(
            positions,
            relation_unit_node_positions(&graph, &visible, &[0])
        );
    }

    #[test]
    fn layout_cache_key_changes_with_filters_and_graph_revision() {
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(
            vec![test_node("g:1", RelationNodeKind::Group)],
            Vec::new(),
        ));
        let initial = relation_view_cache_key(&state, "");
        state.options.show_groups = false;
        let filtered = relation_view_cache_key(&state, "");
        assert_ne!(initial, filtered);

        state.options.show_groups = true;
        state.graph_revision = state.graph_revision.wrapping_add(1);
        assert_ne!(initial, relation_view_cache_key(&state, ""));
    }

    #[test]
    fn layout_cache_honors_zero_link_limit() {
        let mut state = RelationNetworkState::default();
        state.render_setting.max_drawn_links = 0;
        state.replace_graph(test_graph(
            vec![
                test_node("u:1", RelationNodeKind::Friend),
                test_node("g:1", RelationNodeKind::Group),
            ],
            vec![RelationLink {
                source: "g:1".to_string(),
                target: "u:1".to_string(),
            }],
        ));
        let visible = vec!["u:1".to_string(), "g:1".to_string()];
        let cache = build_relation_layout_cache(&state, 1, visible);
        assert!(cache.visible_link_indices.is_empty());
    }

    #[test]
    fn canvas_transform_keeps_equal_scale_on_wide_windows() {
        let transform = RelationCanvasTransform::new(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1_200.0, 500.0)),
            1.0,
            egui::Vec2::ZERO,
        );
        let center = transform.position(egui::Vec2::ZERO);
        let horizontal = transform.position(egui::vec2(1.0, 0.0));
        let vertical = transform.position(egui::vec2(0.0, 1.0));

        assert!(((horizontal - center).length() - (vertical - center).length()).abs() < 0.001);
    }

    #[test]
    fn dense_overview_separates_friends_and_groups() {
        let mut nodes = Vec::new();
        let mut visible = Vec::new();
        for index in 0..32 {
            let id = format!("u:{index}");
            nodes.push(test_node(&id, RelationNodeKind::Friend));
            visible.push(id);
        }
        for index in 0..32 {
            let id = format!("g:{index}");
            nodes.push(test_node(&id, RelationNodeKind::Group));
            visible.push(id);
        }
        let graph = test_graph(nodes, Vec::new());
        let positions = relation_unit_node_positions(&graph, &visible, &[]);

        let max_friend_radius = positions
            .iter()
            .filter(|(id, _)| id.starts_with("u:"))
            .map(|(_, position)| position.length())
            .fold(0.0_f32, f32::max);
        let min_group_radius = positions
            .iter()
            .filter(|(id, _)| id.starts_with("g:"))
            .map(|(_, position)| position.length())
            .fold(f32::INFINITY, f32::min);
        assert!(max_friend_radius < min_group_radius);
    }

    #[test]
    fn focused_view_keeps_focus_when_node_limit_truncates_neighbors() {
        let mut nodes = (0..8)
            .map(|index| test_node(&format!("u:{index}"), RelationNodeKind::Friend))
            .collect::<Vec<_>>();
        nodes.push(test_node("g:focus", RelationNodeKind::Group));
        let links = (0..8)
            .map(|index| RelationLink {
                source: "g:focus".to_string(),
                target: format!("u:{index}"),
            })
            .collect();
        let mut state = RelationNetworkState::default();
        state.render_setting.max_visible_nodes_focused = 4;
        state.replace_graph(test_graph(nodes, links));
        state.view_mode = RelationViewMode::Focused("g:focus".to_string());

        let visible = visible_relation_node_ids(&state, "");
        assert_eq!(visible.len(), 4);
        assert_eq!(visible[0], "g:focus");
    }

    #[test]
    fn focused_force_layout_anchors_focus_and_spreads_neighbors() {
        let mut nodes = vec![test_node("g:focus", RelationNodeKind::Group)];
        let mut links = Vec::new();
        for index in 0..120 {
            let id = format!("u:{index}");
            nodes.push(test_node(&id, RelationNodeKind::Friend));
            links.push(RelationLink {
                source: "g:focus".to_string(),
                target: id,
            });
        }
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(nodes, links));
        state.view_mode = RelationViewMode::Focused("g:focus".to_string());
        let visible = visible_relation_node_ids(&state, "");
        let cache = build_relation_layout_cache(&state, 1, visible);

        assert!(cache.unit_positions["g:focus"].length() < 0.001);
        let average_neighbor_radius = cache
            .unit_positions
            .iter()
            .filter(|(id, _)| id.as_str() != "g:focus")
            .map(|(_, position)| position.length())
            .sum::<f32>()
            / 120.0;
        assert!(average_neighbor_radius > 0.35);
        assert!(cache.force_ticks_remaining > 0);
    }
}
