//! 聊天顶部统一绘制应用和 Bridge 提示，提示来源仍各自保存、清除。

use crate::app::IcaApp;

fn render_notice_row(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    message: &str,
    is_error: bool,
) -> bool {
    ui.push_id(id_salt, |ui| {
        ui.horizontal_wrapped(|ui| {
            if is_error {
                ui.colored_label(egui::Color32::LIGHT_RED, message);
            } else {
                ui.weak(message);
            }
            ui.small_button("清除").clicked()
        })
        .inner
    })
    .inner
}

impl IcaApp {
    pub fn render_chat_notices(&mut self, ui: &mut egui::Ui) {
        // 保留应用错误优先于普通提示的语义，关闭错误后再显示待处理提示。
        if let Some(error) = self.media_error.as_deref() {
            if render_notice_row(ui, "media_error", error, true) {
                self.media_error = None;
            }
        } else if let Some(notice) = self.media_notice.as_deref()
            && render_notice_row(ui, "media_notice", notice, false)
        {
            self.media_notice = None;
        }

        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let bridge = &mut self.bridge_states[bridge_idx];
        bridge.sync_status_history();
        if let Some(error) = bridge.last_error.as_deref()
            && render_notice_row(ui, ("bridge_error", &bridge.bridge_key), error, true)
        {
            bridge.clear_error();
        }
        if let Some(notice) = bridge.last_notice.as_deref()
            && render_notice_row(ui, ("bridge_notice", &bridge.bridge_key), notice, false)
        {
            bridge.clear_notice();
        }
    }
}
