use super::loading::{queue_relation_member_requests, relation_member_load_progress};
use super::*;

pub(super) fn render_relation_network_sidebar(
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
                let progress = relation_member_load_progress(bridge_snapshot);
                ui.add(
                    egui::ProgressBar::new(progress.ratio)
                        .desired_width(ui.available_width())
                        .text(format!("群成员 {} / {}", progress.loaded, progress.total)),
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
                        relation_network.pending_action = Some(RelationAction::Rebuild);
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
                        ui.painter()
                            .circle_filled(dot_rect.center(), 5.0, node.color());
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

pub(super) fn clear_relation_selection(relation_network: &mut RelationNetworkState) {
    relation_network.focused_node_id = None;
    relation_network.selected_node_id = None;
    relation_network.hovered_node_id = None;
    relation_network.selected_node_ids.clear();
    relation_network.view_mode = RelationViewMode::Default;
    reset_relation_canvas_view(relation_network);
}

pub(super) fn reset_relation_canvas_view(relation_network: &mut RelationNetworkState) {
    relation_network.canvas_zoom = 1.0;
    relation_network.canvas_pan = egui::Vec2::ZERO;
}
