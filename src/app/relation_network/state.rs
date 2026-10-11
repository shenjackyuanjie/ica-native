//! 关系网窗口的界面状态：图数据、视图变换、搜索与加载进度。
//!
//! `RelationNetworkState` 通过 `Deref` 直接暴露底层 `RelationLayoutModel`
//! 的图数据与布局缓存，自身只保存视图相关的状态；`replace_graph` 在数据
//! 整体替换时递增修订号，并在首次有数据时复位视图。

use super::controller::RelationAction;
use super::layout::{RELATION_LAYOUT_SCALE, RelationLayoutCache, RelationLayoutModel};
use super::model::RelationGraph;
use crate::config::RelationNetworkSetting;
use std::ops::{Deref, DerefMut};
use std::time::Instant;

/// 关系网窗口持有的全部界面状态。
#[derive(Debug, Clone)]
pub struct RelationNetworkState {
    /// 图数据与布局缓存（经 `Deref` 直接暴露其字段与方法）。
    pub layout: RelationLayoutModel,
    /// 是否把未加载群聊的成员并入图中。
    pub include_unloaded_groups: bool,
    /// 是否在节点旁显示昵称标签。
    pub show_labels: bool,
    /// 成员搜索词。
    pub search_query: String,
    /// 群聊搜索词。
    pub group_search_query: String,
    /// 当前选中节点。
    pub selected_node_id: Option<String>,
    /// 当前悬停节点。
    pub hovered_node_id: Option<String>,
    /// 画布缩放；首帧按 `RELATION_LAYOUT_SCALE` 的倒数初始化，把图缩回画布内。
    pub canvas_zoom: f32,
    /// 画布平移偏移。
    pub canvas_pan: egui::Vec2,
    /// 待执行的数据操作，由 `controller` 在帧末消费。
    pub pending_action: Option<RelationAction>,
    /// 是否正在全量加载群成员。
    pub load_all_active: bool,
    /// 全量加载开始时间，用于估算进度。
    pub load_started_at: Option<Instant>,
    /// 全量加载开始时的已加载群数，作为进度基准。
    pub load_start_loaded_groups: usize,
    /// 上次重建图时的已加载群数；仅当它变化才重建，避免每帧重建。
    pub load_last_rebuild_loaded_groups: usize,
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
            // 单位坐标乘了 RELATION_LAYOUT_SCALE，首帧用其倒数把图缩回画布内。
            canvas_zoom: 1.0 / RELATION_LAYOUT_SCALE,
            canvas_pan: egui::Vec2::ZERO,
            pending_action: None,
            load_all_active: false,
            load_started_at: None,
            load_start_loaded_groups: 0,
            load_last_rebuild_loaded_groups: 0,
        }
    }
}

/// 让界面代码直接访问布局模型里的图数据与缓存字段。
impl Deref for RelationNetworkState {
    type Target = RelationLayoutModel;

    fn deref(&self) -> &Self::Target {
        &self.layout
    }
}

/// 对应的可变访问，供界面代码直接修改图数据。
impl DerefMut for RelationNetworkState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.layout
    }
}

impl RelationNetworkState {
    /// 用渲染设置初始化状态（建造者模式）。
    pub fn with_render_setting(mut self, render_setting: RelationNetworkSetting) -> Self {
        self.render_setting = render_setting;
        self
    }

    /// 整体替换图数据。
    ///
    /// 递增修订号并清空布局缓存；原图为空时视为首次加载，
    /// 顺带把缩放与平移复位，让整张图适配画布。
    pub fn replace_graph(&mut self, graph: RelationGraph) {
        let should_reset_view = self.graph.nodes.is_empty();
        self.graph = graph;
        self.graph_revision = self.graph_revision.wrapping_add(1);
        self.layout_cache = RelationLayoutCache::default();
        if should_reset_view {
            self.canvas_zoom = 1.0 / RELATION_LAYOUT_SCALE;
            self.canvas_pan = egui::Vec2::ZERO;
        }
    }
}
