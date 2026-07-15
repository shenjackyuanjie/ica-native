use std::ops::{Deref, DerefMut};
use std::time::Instant;

use crate::config::RelationNetworkSetting;

use super::controller::RelationAction;
use super::layout::{RelationLayoutCache, RelationLayoutModel};
use super::model::RelationGraph;

#[derive(Debug, Clone)]
pub struct RelationNetworkState {
    pub(super) layout: RelationLayoutModel,
    pub(super) include_unloaded_groups: bool,
    pub(super) show_labels: bool,
    pub(super) search_query: String,
    pub(super) group_search_query: String,
    pub(super) selected_node_id: Option<String>,
    pub(super) hovered_node_id: Option<String>,
    pub(super) canvas_zoom: f32,
    pub(super) canvas_pan: egui::Vec2,
    pub(super) pending_action: Option<RelationAction>,
    pub(super) load_all_active: bool,
    pub(super) load_started_at: Option<Instant>,
    pub(super) load_start_loaded_groups: usize,
    pub(super) load_last_rebuild_loaded_groups: usize,
}

impl Default for RelationNetworkState {
    fn default() -> Self {
        Self {
            layout: RelationLayoutModel::default(),
            include_unloaded_groups: true,
            show_labels: true,
            search_query: String::new(),
            group_search_query: String::new(),
            selected_node_id: None,
            hovered_node_id: None,
            canvas_zoom: 1.0,
            canvas_pan: egui::Vec2::ZERO,
            pending_action: None,
            load_all_active: false,
            load_started_at: None,
            load_start_loaded_groups: 0,
            load_last_rebuild_loaded_groups: 0,
        }
    }
}

impl Deref for RelationNetworkState {
    type Target = RelationLayoutModel;

    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

impl DerefMut for RelationNetworkState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.layout
    }
}

impl RelationNetworkState {
    pub fn with_render_setting(mut self, render_setting: RelationNetworkSetting) -> Self {
        self.render_setting = render_setting;
        self
    }

    pub(super) fn replace_graph(&mut self, graph: RelationGraph) {
        let should_reset_view = self.graph.nodes.is_empty();
        self.graph = graph;
        self.graph_revision = self.graph_revision.wrapping_add(1);
        self.layout_cache = RelationLayoutCache::default();
        if should_reset_view {
            self.canvas_zoom = 1.0;
            self.canvas_pan = egui::Vec2::ZERO;
        }
    }
}
