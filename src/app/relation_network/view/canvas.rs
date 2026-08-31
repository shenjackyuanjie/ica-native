use super::*;

pub fn render_relation_network_canvas(
    ui: &mut egui::Ui,
    relation_network: &mut RelationNetworkState,
    force_animation_enabled: bool,
) {
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
    let repaint_after = if force_animation_enabled {
        advance_relation_force_layout(relation_network, Instant::now())
    } else {
        pause_relation_force_layout(relation_network);
        None
    };
    if let Some(repaint_after) = repaint_after {
        ui.ctx().request_repaint_after(repaint_after);
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
        relation_force_canvas_max_radius(relation_network),
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
        let color = node.color();
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
