//! 即时 viewport 复用现有聊天界面和连接，每次绘制仅切换窗口的 UI 上下文。

#[cfg(test)]
#[path = "tests/chat_windows.rs"]
mod tests;

use super::{ChatWindowUiState, IcaApp};

pub struct ChatWindow {
    bridge_key: String,
    room_id: i64,
    ui: ChatWindowUiState,
}

fn viewport_id(bridge_key: &str, room_id: i64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("chat_window", bridge_key, room_id))
}

impl IcaApp {
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

    pub fn active_chat_is_detached(&self) -> bool {
        self.active_bridge_state().is_some_and(|state| {
            state
                .selected_room_id
                .is_some_and(|room_id| state.detached_room_ids.contains(&room_id))
        })
    }

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
