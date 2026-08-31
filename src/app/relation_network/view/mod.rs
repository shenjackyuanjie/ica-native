use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 返回关系网独立窗口的固定视口 ID。
///
/// 主视口处理完后台事件后，需要用同一个 ID 主动唤醒独立窗口；如果在不同位置
/// 分别计算 ID，后续修改标识字符串时很容易只改到一处，导致子视口再次停留在旧帧。
pub fn relation_network_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("relation_network")
}

use super::super::IcaApp;
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
use sidebar::render_relation_network_sidebar;
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

mod interaction;
mod loading;
mod overlay;

#[cfg(test)]
mod tests;

use interaction::*;
use loading::RelationBridgeSnapshot;
use overlay::*;

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
