use crate::app::{IcaApp, PendingFile, PendingImage};

impl IcaApp {
    pub(super) fn handle_composer_drop(
        &mut self,
        ui: &mut egui::Ui,
        active_bridge_idx: usize,
        room_id: i64,
    ) {
        // 拖放文件上传
        let hovered = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());

        if hovered {
            let screen_rect = ui.ctx().input(|i| i.viewport_rect());
            egui::Area::new(egui::Id::new("drop_overlay"))
                .fixed_pos(screen_rect.min)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    let painter = ui.painter();
                    painter.rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(100));
                    painter.text(
                        screen_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "拖放文件到此处",
                        egui::FontId::proportional(24.0),
                        egui::Color32::WHITE,
                    );
                });
        }

        if !dropped.is_empty() {
            let mut dropped_images = Vec::new();
            let mut dropped_file = None;
            let mut dropped_errors = Vec::new();

            for file in dropped {
                let file_name = if !file.name.is_empty() {
                    file.name.clone()
                } else if let Some(p) = &file.path {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                } else {
                    "unknown".to_string()
                };

                let data = if let Some(bytes) = file.bytes {
                    bytes.to_vec()
                } else if let Some(path) = &file.path {
                    std::fs::read(path).unwrap_or_default()
                } else {
                    Vec::new()
                };

                if data.is_empty() {
                    continue;
                }

                let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
                let image_exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
                if image_exts.contains(&ext.as_str()) {
                    let mime = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                    dropped_images.push(PendingImage::new(file_name, mime, data));
                } else if dropped_file.is_none() {
                    let ft = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                    dropped_file = Some(PendingFile::new(file_name, ft, data));
                } else {
                    dropped_errors.push(format!("暂不支持同时拖放多份非图片文件: {}", file_name));
                }
            }

            if !dropped_images.is_empty() {
                self.append_pending_images(active_bridge_idx, room_id, dropped_images);
            }
            if let Some(file) = dropped_file {
                self.bridge_states[active_bridge_idx]
                    .pending_file_by_room
                    .insert(room_id, file);
            }
            if !dropped_errors.is_empty() {
                self.bridge_states[active_bridge_idx].last_error = Some(dropped_errors.join("；"));
            }
        }
    }
}
