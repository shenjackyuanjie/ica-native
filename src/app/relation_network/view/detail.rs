//! 节点详情卡片与节点尺寸图例。

use super::*;

/// 绘制单个节点的详情：名称、类型、QQ/群号、成员数、共同群与关联数。
pub fn render_relation_node_detail(ui: &mut egui::Ui, node: &RelationNode) {
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

/// 绘制节点尺寸图例：示例圆按成员数分档着色，说明“越大关联越多”。
pub fn render_relation_size_legend(ui: &mut egui::Ui) {
    let theme = RelationTheme::from_ui(ui);
    for (label, radius, border, member_count) in [
        ("少量关联 (<10)", 5.0, 0.0, 5),
        ("数十 / 百人", 8.0, 2.0, 100),
        ("千人群", 11.0, 3.0, 1_000),
        ("万人群", 14.0, 4.0, 10_000),
    ] {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 24.0), egui::Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                radius,
                relation_group_color(Some(member_count)),
            );
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
