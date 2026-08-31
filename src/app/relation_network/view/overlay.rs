//! 画布网格、缩放控件、性能档位与节点悬浮信息的绘制。

use super::super::layout::*;
use super::super::model::*;
use super::super::state::RelationNetworkState;
use super::super::theme::RelationTheme;
use super::detail::render_relation_node_detail;

pub fn render_relation_canvas_grid(
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

pub fn render_relation_canvas_controls(
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

pub fn set_relation_canvas_zoom(
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

pub fn relation_node_draw_radius(
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

pub fn render_relation_network_overlay(
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

pub fn render_relation_indicator(ui: &mut egui::Ui, rect: egui::Rect, text: &str) {
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

pub fn render_relation_node_popup(
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

#[derive(Debug, Clone, Copy)]
pub struct RelationPerformanceLevel {
    pub index: usize,
    pub label: &'static str,
}

pub fn relation_performance_level(node_count: usize) -> RelationPerformanceLevel {
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
