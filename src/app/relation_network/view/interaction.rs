//! 关系网节点的点击、聚焦与多选交互。

use super::super::layout::*;
use super::super::state::RelationNetworkState;
use super::sidebar::reset_relation_canvas_view;

/// 处理画布上节点的点击。
///
/// 多选模式下点击在勾选/取消勾选间切换；其余模式点击进入以该节点为中心
/// 的聚焦视图，并复位画布变换。
pub fn handle_relation_node_click(relation_network: &mut RelationNetworkState, node_id: String) {
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

/// 退出当前聚焦或多选关系视图（点击画布空白处触发）。
///
/// 多选关系退回多选、聚焦退回总览并清空选中；纯多选与总览的空白点击
/// 不改变视图，只清理选中态。
pub fn exit_relation_focus_or_multiselect(relation_network: &mut RelationNetworkState) {
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

/// 切换多选模式。
///
/// 多选已勾选至少两个节点时再切一次进入“多选关系”视图；
/// 已在多选关系时退出到总览并清空勾选。
pub fn toggle_relation_multi_select(relation_network: &mut RelationNetworkState) {
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
