//! 关系网节点的点击、聚焦与多选交互。

use super::super::layout::*;
use super::super::state::RelationNetworkState;
use super::sidebar::reset_relation_canvas_view;

pub(super) fn handle_relation_node_click(
    relation_network: &mut RelationNetworkState,
    node_id: String,
) {
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

pub(super) fn exit_relation_focus_or_multiselect(relation_network: &mut RelationNetworkState) {
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

pub(super) fn toggle_relation_multi_select(relation_network: &mut RelationNetworkState) {
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
