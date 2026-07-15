use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ica::IcaCommand;
use tokio::sync::mpsc::UnboundedSender;

const RELATION_MEMBER_LOAD_CONCURRENCY: usize = 12;
const RELATION_REBUILD_GROUP_STEP: usize = 12;

/// 返回关系网独立窗口的固定视口 ID。
///
/// 主视口处理完后台事件后，需要用同一个 ID 主动唤醒独立窗口；如果在不同位置
/// 分别计算 ID，后续修改标识字符串时很容易只改到一处，导致子视口再次停留在旧帧。
pub fn relation_network_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("relation_network")
}

use super::super::IcaApp;
use super::super::state::BridgeState;
use super::controller::RelationAction;
use super::layout::*;
use super::model::*;
use super::state::RelationNetworkState;
use super::theme::RelationTheme;

mod canvas;
mod detail;
mod sidebar;
mod toolbar;
use canvas::render_relation_network_canvas;
use detail::{render_relation_node_detail, render_relation_size_legend};
use sidebar::{render_relation_network_sidebar, reset_relation_canvas_view};
use toolbar::render_relation_network_header;

impl IcaApp {
    /// 渲染关系网独立窗口（deferred 子视口）。
    ///
    /// 窗口未打开时直接返回；否则同步渲染配置、消费子视口写入的动作，并构建桥接状态
    /// 快照后交给 deferred 回调绘制。每次调用都会主动唤醒子视口，确保后台状态变化
    /// 能及时反映到界面上。
    pub fn render_relation_network_window(&mut self, ctx: &egui::Context) {
        if !self.open_page.relation_network {
            return;
        }
        self.sync_relation_network_render_setting();

        // deferred 子视口写入的按钮动作会在下一次主视口帧到来前保存在共享状态中。
        // 必须先消费动作、再生成桥接状态快照，否则“开始加载”后的第一份子窗口快照
        // 仍然是加载前的数据，进度条要等到首个网络响应到达后才可能出现。
        self.apply_relation_network_viewport_actions();
        if !self.open_page.relation_network {
            return;
        }

        let relation_network = self.relation_network.clone();
        let parent_viewport_id = ctx.viewport_id();
        let bridge_snapshot = self.active_bridge_idx.and_then(|idx| {
            self.bridge_states
                .get(idx)
                .map(|state| RelationBridgeSnapshot {
                    rooms_len: state.rooms.len(),
                    total_groups: state.rooms.iter().filter(|room| room.room_id < 0).count(),
                    loaded_groups: state
                        .conversations
                        .iter()
                        .filter(|(room_id, conversation)| {
                            **room_id < 0 && conversation.group_members_loaded
                        })
                        .count(),
                    loading_groups: state
                        .conversations
                        .iter()
                        .filter(|(room_id, conversation)| {
                            **room_id < 0 && conversation.loading_group_members
                        })
                        .count(),
                    pending_groups: state
                        .rooms
                        .iter()
                        .filter(|room| room.room_id < 0)
                        .map(|room| room.room_id)
                        .filter(|room_id| {
                            !state.group_members_loaded(*room_id)
                                && !state.group_members_loading(*room_id)
                        })
                        .count(),
                })
        });

        let viewport_id = relation_network_viewport_id();
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title("QQ 关系网")
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([760.0, 480.0]);

        ctx.show_viewport_deferred(
            viewport_id,
            viewport_builder,
            move |viewport_ctx, _class| {
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    relation_network.lock().unwrap().pending_action = Some(RelationAction::Close);
                    return;
                }

                if viewport_ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    relation_network.lock().unwrap().pending_action = Some(RelationAction::Close);
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
                    state.pending_action.is_some()
                };
                if parent_action_pending {
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                }
            },
        );

        // bridge 事件首先唤醒主视口；主视口更新状态并重新注册上面的 deferred 回调后，
        // 还需要明确唤醒子视口，它才会用这一帧的新快照刷新进度、统计和图谱。
        // 这也覆盖“加载中变为完成”“请求失败后加载数回退”等所有后台状态变化，
        // 避免只有鼠标移入窗口或关闭窗口时界面才更新。
        ctx.request_repaint_of(viewport_id);
    }

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

    fn continue_relation_network_member_loading(&mut self) {
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

    fn apply_relation_network_auto_degrade(&mut self) {
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

    fn sync_relation_network_render_setting(&mut self) {
        let render_setting = self.config.snapshot().ui_setting.relation_network;
        self.relation_network.lock().unwrap().render_setting = render_setting;
    }

    fn apply_relation_network_viewport_actions(&mut self) {
        let action = self.relation_network.lock().unwrap().pending_action.take();
        match action {
            Some(RelationAction::Close) => self.open_page.relation_network = false,
            Some(RelationAction::Rebuild) => self.rebuild_relation_network(),
            Some(RelationAction::LoadGroups(limit)) => self.request_relation_network_members(limit),
            None => {}
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
                let force_animation_enabled =
                    !relation_network.load_all_active && bridge_snapshot.loading_groups == 0;
                render_relation_network_canvas(ui, &mut relation_network, force_animation_enabled);
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
        relation_network.canvas_zoom = 1.0 / RELATION_LAYOUT_SCALE;
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
    let zoom = zoom.clamp(0.05, 4.0);
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
                            .rect_filled(avatar_rect, avatar_radius, node.color());
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

/// 渲染关系网界面所需的桥接状态快照。
///
/// 由主视口在每帧构建并传给子视口，包含房间数、群加载进度等只读信息，
/// 使子视口无需直接访问 `BridgeState`。
#[derive(Debug, Clone)]
struct RelationBridgeSnapshot {
    rooms_len: usize,
    /// 当前桥接房间列表中的群总数，不能使用上一次图谱构建时缓存的总数。
    total_groups: usize,
    loaded_groups: usize,
    loading_groups: usize,
    pending_groups: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RelationMemberLoadProgress {
    loaded: usize,
    total: usize,
    ratio: f32,
}

/// 根据实时桥接快照计算群成员加载进度。
///
/// 总数为零时仍向进度条提供一个安全分母，但展示文本保留真实的 `0 / 0`；同时将
/// 已加载数量限制在总数以内，避免房间列表刚刷新、旧成员缓存尚未清理时出现超过
/// 100% 的进度值。
fn relation_member_load_progress(snapshot: &RelationBridgeSnapshot) -> RelationMemberLoadProgress {
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

fn queue_relation_member_requests(
    relation_network: &mut RelationNetworkState,
    limit: Option<usize>,
) {
    relation_network.pending_action = Some(RelationAction::LoadGroups(limit));
    tracing::debug!(limit = ?limit, "关系网成员加载动作已排队");
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

fn relation_login_user_id(state: &BridgeState) -> Option<i64> {
    (state.online_data.qqid > 0).then_some(state.online_data.qqid)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn group_color_becomes_darker_as_member_count_grows() {
        let small = relation_group_color(Some(10));
        let medium = relation_group_color(Some(1_000));
        let large = relation_group_color(Some(50_000));
        let brightness = |color: egui::Color32| {
            u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b())
        };

        assert!(brightness(small) > brightness(medium));
        assert!(brightness(medium) > brightness(large));
        assert_eq!(relation_group_color(None), RelationNodeKind::Group.color());
    }

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
        let positions =
            relation_unit_node_positions(&graph, &visible, &[0], 2.40 * RELATION_LAYOUT_SCALE, 0.0);

        let member = positions["u:1"];
        let group = positions["g:1"];
        assert!((member - group).length() <= 0.131 * RELATION_LAYOUT_SCALE);
        assert_eq!(
            positions,
            relation_unit_node_positions(&graph, &visible, &[0], 2.40 * RELATION_LAYOUT_SCALE, 0.0,)
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
            1.20,
        );
        let center = transform.position(egui::Vec2::ZERO);
        let horizontal = transform.position(egui::vec2(1.0, 0.0));
        let vertical = transform.position(egui::vec2(0.0, 1.0));

        assert!(((horizontal - center).length() - (vertical - center).length()).abs() < 0.001);
    }

    #[test]
    fn dense_overview_initializes_groups_outside_friends() {
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
        let positions =
            relation_unit_node_positions(&graph, &visible, &[], 2.40 * RELATION_LAYOUT_SCALE, 0.0);

        let max_friend_radius = positions
            .iter()
            .filter(|(id, _)| id.starts_with("u:"))
            .map(|(_, position)| position.length())
            .fold(0.0_f32, f32::max);
        let max_group_radius = positions
            .iter()
            .filter(|(id, _)| id.starts_with("g:"))
            .map(|(_, position)| position.length())
            .fold(0.0_f32, f32::max);
        assert!(max_group_radius > max_friend_radius + 10.0);
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
    }

    #[test]
    fn overview_force_layout_runs_continuously_with_a_real_time_gap() {
        let mut state = RelationNetworkState::default();
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
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);

        let now = Instant::now();
        let interval = advance_relation_force_layout(&mut state, now).unwrap();
        let positions_after_first_tick = state.layout_cache.unit_positions.clone();

        // 在计划时间之前发生的额外 UI 帧只能缩短等待时间，不能提前移动节点。
        let early_wait = advance_relation_force_layout(&mut state, now + interval / 2).unwrap();
        assert!(early_wait < interval);
        assert_eq!(
            state.layout_cache.unit_positions,
            positions_after_first_tick
        );

        let next_interval = advance_relation_force_layout(&mut state, now + interval).unwrap();
        assert_ne!(
            state.layout_cache.unit_positions,
            positions_after_first_tick
        );

        // 即使已经运行了远超旧上限的次数，布局仍会继续安排下一次迭代。
        let mut tick_at = now + interval + next_interval;
        for _ in 0..200 {
            let wait = advance_relation_force_layout(&mut state, tick_at).unwrap();
            tick_at += wait;
        }
        assert!(advance_relation_force_layout(&mut state, tick_at).is_some());
    }

    #[test]
    fn configured_group_and_friend_lengths_form_separate_layers() {
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(
            vec![
                test_node("u:self", RelationNodeKind::SelfUser),
                test_node("u:friend", RelationNodeKind::Friend),
                test_node("g:1", RelationNodeKind::Group),
            ],
            vec![
                RelationLink {
                    source: "u:self".to_string(),
                    target: "u:friend".to_string(),
                },
                RelationLink {
                    source: "u:self".to_string(),
                    target: "g:1".to_string(),
                },
            ],
        ));
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);

        let mut tick_at = Instant::now();
        for _ in 0..100 {
            let wait = advance_relation_force_layout(&mut state, tick_at).unwrap();
            tick_at += wait;
        }

        let friend_radius = state.layout_cache.unit_positions["u:friend"].length();
        let group_radius = state.layout_cache.unit_positions["g:1"].length();
        assert!(group_radius > friend_radius + 0.08);
    }

    #[test]
    fn dense_friends_use_a_wide_radial_band_instead_of_one_outer_ring() {
        let mut nodes = vec![test_node("u:self", RelationNodeKind::SelfUser)];
        let mut links = Vec::new();
        for index in 0..300 {
            let id = format!("u:{index}");
            nodes.push(test_node(&id, RelationNodeKind::Friend));
            links.push(RelationLink {
                source: "u:self".to_string(),
                target: id,
            });
        }
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(nodes, links));
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);

        let mut tick_at = Instant::now();
        for _ in 0..180 {
            let wait = advance_relation_force_layout(&mut state, tick_at).unwrap();
            tick_at += wait;
        }

        let mut radii = state
            .layout_cache
            .unit_positions
            .iter()
            .filter(|(id, _)| id.as_str() != "u:self")
            .map(|(_, position)| position.length())
            .collect::<Vec<_>>();
        radii.sort_by(f32::total_cmp);
        let inner_decile = radii[radii.len() / 10];
        let outer_decile = radii[radii.len() * 9 / 10];
        assert!(outer_decile - inner_decile > 0.20);
        assert!(outer_decile < relation_force_layout_max_radius(&state));
    }

    #[test]
    fn dense_groups_use_a_wide_radius_without_hitting_the_layout_boundary() {
        let mut nodes = vec![test_node("u:self", RelationNodeKind::SelfUser)];
        let mut links = Vec::new();
        for index in 0..160 {
            let friend_id = format!("u:{index}");
            nodes.push(test_node(&friend_id, RelationNodeKind::Friend));
            links.push(RelationLink {
                source: "u:self".to_string(),
                target: friend_id,
            });

            let group_id = format!("g:{index}");
            nodes.push(test_node(&group_id, RelationNodeKind::Group));
            links.push(RelationLink {
                source: "u:self".to_string(),
                target: group_id,
            });
        }
        let mut state = RelationNetworkState::default();
        // 与随仓库提供的配置一致：好友长度保持原值，只扩大群边长度。
        state.render_setting.force_friend_link_length = 0.52;
        state.render_setting.force_group_link_length = 2.40;
        state.replace_graph(test_graph(nodes, links));
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);

        let mut tick_at = Instant::now();
        for _ in 0..180 {
            let wait = advance_relation_force_layout(&mut state, tick_at).unwrap();
            tick_at += wait;
        }

        let mut friend_radii = Vec::new();
        let mut group_radii = Vec::new();
        for (id, position) in &state.layout_cache.unit_positions {
            if id.starts_with("g:") {
                group_radii.push(position.length());
            } else if id.as_str() != "u:self" {
                friend_radii.push(position.length());
            }
        }
        friend_radii.sort_by(f32::total_cmp);
        group_radii.sort_by(f32::total_cmp);
        let outer_friend = friend_radii[friend_radii.len() * 9 / 10];
        let inner_group = group_radii[group_radii.len() / 10];
        let outer_group = group_radii[group_radii.len() * 9 / 10];
        assert!(outer_group > outer_friend + 10.0);
        assert!(outer_group - inner_group > 8.0);
        assert!(outer_group < relation_force_layout_max_radius(&state));
    }

    #[test]
    fn force_layout_repels_overlapping_spatial_neighbors() {
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(
            vec![
                test_node("u:1", RelationNodeKind::Friend),
                test_node("u:2", RelationNodeKind::Friend),
            ],
            Vec::new(),
        ));
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);
        state
            .layout_cache
            .unit_positions
            .insert("u:1".to_string(), egui::Vec2::ZERO);
        state
            .layout_cache
            .unit_positions
            .insert("u:2".to_string(), egui::Vec2::ZERO);

        advance_relation_force_layout(&mut state, Instant::now());

        let distance = (state.layout_cache.unit_positions["u:1"]
            - state.layout_cache.unit_positions["u:2"])
            .length();
        assert!(distance > 0.001);
    }

    #[test]
    fn overview_force_layout_keeps_self_user_at_center() {
        let mut state = RelationNetworkState::default();
        state.replace_graph(test_graph(
            vec![
                test_node("u:self", RelationNodeKind::SelfUser),
                test_node("u:friend", RelationNodeKind::Friend),
                test_node("g:1", RelationNodeKind::Group),
            ],
            vec![
                RelationLink {
                    source: "g:1".to_string(),
                    target: "u:self".to_string(),
                },
                RelationLink {
                    source: "g:1".to_string(),
                    target: "u:friend".to_string(),
                },
            ],
        ));
        let visible = visible_relation_node_ids(&state, "");
        state.layout_cache = build_relation_layout_cache(&state, 1, visible);

        advance_relation_force_layout(&mut state, Instant::now());

        assert_eq!(
            state.layout_cache.unit_positions["u:self"],
            egui::Vec2::ZERO
        );
    }
}
