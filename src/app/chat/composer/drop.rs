//! 输入区的文件拖放处理。
//!
//! 悬停时铺一层全屏蒙层提示“拖放文件到此处”；松手后按扩展名把文件分流为
//! 待发送图片与至多一份普通文件：多份非图片文件被拒绝，读取失败与空文件
//! 汇总成一条错误提示，不阻断已成功入队的图片。

use crate::app::{IcaApp, PendingFile, PendingImage};

impl IcaApp {
    /// 处理拖到输入区的文件：图片追加到当前会话的待发送队列，首份非图片文件
    /// 设为待发送文件；读取失败、空文件与多份非图片文件汇总为一条错误提示。
    pub fn handle_composer_drop(
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
                let path = file.path();
                let file_name = path
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "未知文件".to_string());

                let data = match file.bytes() {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::warn!(file_name, error, "读取拖放文件失败");
                        dropped_errors.push(format!("读取拖放文件“{file_name}”失败: {error}"));
                        continue;
                    }
                };

                if data.is_empty() {
                    dropped_errors.push(format!("拖放文件“{file_name}”内容为空"));
                    continue;
                }

                let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
                let image_exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
                if image_exts.contains(&ext.as_str()) {
                    // 仅按文件名（扩展名）猜测 MIME，不读取文件内容。
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
                    .conversation_mut(room_id)
                    .pending_file = Some(file);
            }
            if !dropped_errors.is_empty() {
                self.bridge_states[active_bridge_idx].last_error = Some(dropped_errors.join("；"));
            }
        }
    }
}
