use super::loading::queue_relation_member_requests;
use super::*;

pub fn render_relation_network_header(
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
                relation_network.pending_action = Some(RelationAction::Rebuild);
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
