//! 独立聊天窗口：把某个会话拆到单独的 egui viewport 中显示。
//!
//! 这些窗口复用主应用已有的聊天界面与 socket.io 连接，每次绘制仅通过交换
//! `chat_ui` 状态并临时切换 `active_bridge_idx` / `selected_room_id` 来改变 UI
//! 上下文，不新建连接或消息通道。用稳定的 bridge key 定位会话，避免配置重载后
//! 索引错位。

#[cfg(test)]
#[path = "tests/chat_windows.rs"]
mod tests;

use super::{ChatWindowUiState, IcaApp};

/// 一个已拆分的独立聊天窗口记录。
///
/// 用 `bridge_key` 而非索引定位连接，保证配置重载后仍指向同一会话。
pub struct ChatWindow {
    /// 所属 Bridge 连接的稳定标识。
    bridge_key: String,
    /// 窗口展示的会话 ID。
    room_id: i64,
    /// 该窗口私有的聊天 UI 状态（草稿、滚动、面板开合等），绘制时与主状态临时交换。
    ui: ChatWindowUiState,
}

/// 由 (bridge_key, room_id) 稳定派生 viewport ID，保证同一会话始终映射到同一窗口。
fn viewport_id(bridge_key: &str, room_id: i64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("chat_window", bridge_key, room_id))
}

/// 独立聊天窗口的打开、占位展示与逐帧渲染逻辑。
impl IcaApp {
    /// 把指定 bridge 的某个会话拆分到独立窗口；已打开则仅还原并聚焦。
    ///
    /// 首次打开时借道主状态走一遍“选房”，以触发取历史、自动已读等副作用，再恢复
    /// 主窗口原先的选择与 UI 状态；随后把该会话登记为 detached 并聚焦其 viewport。
    pub fn open_chat_window(&mut self, ctx: &egui::Context, bridge_idx: usize, room_id: i64) {
        let bridge_key = self.bridge_states[bridge_idx].bridge_key.clone();
        let id = viewport_id(&bridge_key, room_id);
        if !self
            .chat_windows
            .iter()
            .any(|window| window.bridge_key == bridge_key && window.room_id == room_id)
        {
            self.bridge_states[bridge_idx]
                .detached_room_ids
                .insert(room_id);
            // 复用首次取历史、自动已读等逻辑，随后恢复主窗口的选择。
            let active_bridge = self.active_bridge_idx;
            let selected_room = self.bridge_states[bridge_idx].selected_room_id;
            let compact_panel = self.compact_chat_panel;
            let mut window_ui = ChatWindowUiState::default();
            std::mem::swap(&mut self.state.chat_ui, &mut window_ui);
            self.active_bridge_idx = Some(bridge_idx);
            self.select_active_room(room_id);
            self.active_bridge_idx = active_bridge;
            self.bridge_states[bridge_idx].selected_room_id = selected_room;
            self.compact_chat_panel = compact_panel;
            std::mem::swap(&mut self.state.chat_ui, &mut window_ui);
            self.chat_windows.push(ChatWindow {
                bridge_key,
                room_id,
                ui: window_ui,
            });
        }
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// 当前选中的会话是否已被拆分到独立窗口（此时主区只显示占位提示）。
    pub fn active_chat_is_detached(&self) -> bool {
        self.active_bridge_state().is_some_and(|state| {
            state
                .selected_room_id
                .is_some_and(|room_id| state.detached_room_ids.contains(&room_id))
        })
    }

    /// 在主窗口中央绘制“会话已在独立窗口打开”的占位页，并提供跳转到该窗口的按钮。
    pub fn render_detached_chat_placeholder(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("此会话已在独立窗口打开，关闭该窗口后可在这里继续聊天。");
            if ui.button("切换到聊天窗口").clicked()
                && let Some(bridge_idx) = self.active_bridge_idx
                && let Some(room_id) = self.bridge_states[bridge_idx].selected_room_id
            {
                self.open_chat_window(ui.ctx(), bridge_idx, room_id);
            }
            self.render_chat_notices(ui);
        });
    }

    /// 渲染某个独立窗口期间，把应用上下文临时切到该窗口的 bridge/会话/UI 状态，
    /// 渲染完再原样恢复。
    ///
    /// 这样窗口内的聊天界面无需感知自己跑在独立 viewport 中，可直接复用主界面代码；
    /// 返回泛型 `R` 以便调用方取回渲染结果（如窗口是否仍保持打开）。
    fn with_chat_window<R>(
        &mut self,
        bridge_idx: usize,
        window: &mut ChatWindow,
        render: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let active_bridge = self.active_bridge_idx;
        let selected_room = self.bridge_states[bridge_idx].selected_room_id;
        self.active_bridge_idx = Some(bridge_idx);
        self.bridge_states[bridge_idx].selected_room_id = Some(window.room_id);
        std::mem::swap(&mut self.state.chat_ui, &mut window.ui);
        let result = render(self);
        std::mem::swap(&mut self.state.chat_ui, &mut window.ui);
        self.bridge_states[bridge_idx].selected_room_id = selected_room;
        self.active_bridge_idx = active_bridge;
        result
    }

    /// 绘制所有独立聊天窗口，并清理已关闭或所属 bridge 已消失的窗口。
    ///
    /// 对每个窗口按稳定 bridge key 重新定位连接，定位不到则移除该窗口记录；
    /// 窗口被用户关闭时同步把会话从 detached 集合移除。
    pub fn render_chat_windows(&mut self, ctx: &egui::Context) {
        let mut windows = std::mem::take(&mut self.chat_windows);
        windows.retain_mut(|window| {
            // 使用稳定的 bridge key，避免配置重载后索引指向另一个连接。
            let Some(bridge_idx) = self
                .bridge_states
                .iter()
                .position(|session| session.bridge_key == window.bridge_key)
            else {
                return false;
            };
            self.bridge_states[bridge_idx]
                .detached_room_ids
                .insert(window.room_id);
            let title = self.bridge_states[bridge_idx]
                .rooms
                .iter()
                .find(|room| room.room_id == window.room_id)
                .map(|room| room.room_name.clone())
                .unwrap_or_else(|| "聊天".into());
            let room_id = window.room_id;
            let bridge_key = window.bridge_key.clone();
            let open = self.with_chat_window(bridge_idx, window, |app| {
                let mut open = true;
                ctx.show_viewport_immediate(
                    viewport_id(&bridge_key, room_id),
                    egui::ViewportBuilder::default()
                        .with_title(format!("{title} — ica-native"))
                        .with_inner_size([800.0, 640.0])
                        .with_min_inner_size([420.0, 320.0]),
                    |ui, _class| {
                        if ui.ctx().input(|input| input.viewport().close_requested()) {
                            open = false;
                            return;
                        }
                        app.update_chat_input(ui.ctx());
                        app.handle_chat_escape(ui.ctx());
                        app.render_group_members_panel(ui);
                        app.render_central_panel(ui);
                        app.render_group_ban_confirmation(ui.ctx());
                        app.render_group_files_window(ui.ctx());
                    },
                );
                open
            });
            if !open {
                self.bridge_states[bridge_idx]
                    .detached_room_ids
                    .remove(&window.room_id);
                ctx.request_repaint();
            }
            open
        });
        self.chat_windows = windows;
    }
}
