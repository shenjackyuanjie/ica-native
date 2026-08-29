use serde_json::json;

use crate::app::IcaApp;
use crate::ica::IcaCommand;

impl IcaApp {
    /// 常用群文件入口。保留 fileMgr 协议能力，同时免去手工填写群号。
    pub(crate) fn render_group_files_window(&mut self, ctx: &egui::Context) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            self.group_file_panel.open = false;
            return;
        };
        let Some(room_id) = self.bridge_states[bridge_idx]
            .selected_room_id
            .filter(|room_id| *room_id < 0)
        else {
            self.group_file_panel.open = false;
            return;
        };
        if !self.group_file_panel.open {
            return;
        }
        let group_id = -room_id;
        let response = self.bridge_states[bridge_idx]
            .last_socket_api_response
            .clone();
        let mut list_directory = false;
        let mut download_file = false;
        let mut create_directory = false;
        let mut upload_path = None;
        let mut open = self.group_file_panel.open;

        egui::Window::new("群文件")
            .open(&mut open)
            .default_size(egui::vec2(520.0, 420.0))
            .min_size(egui::vec2(360.0, 280.0))
            .show(ctx, |ui| {
                ui.weak("当前群聊的文件目录；文件列表和下载链接会显示在下方响应区。");
                ui.horizontal_wrapped(|ui| {
                    ui.label("目录 FID");
                    ui.add_sized(
                        [190.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_file_panel.directory_fid)
                            .hint_text("留空为根目录"),
                    );
                    ui.label("偏移");
                    ui.add_sized(
                        [56.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_file_panel.list_start),
                    );
                    if ui.button("刷新列表").clicked() {
                        list_directory = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("文件 FID");
                    ui.add_sized(
                        [190.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_file_panel.file_fid),
                    );
                    if ui.button("获取下载链接").clicked() {
                        download_file = true;
                    }
                    if ui.button("上传文件").clicked() {
                        upload_path = rfd::FileDialog::new().pick_file();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("新建文件夹");
                    ui.add_sized(
                        [180.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_file_panel.folder_name),
                    );
                    if ui.button("创建").clicked() {
                        create_directory = true;
                    }
                });
                ui.separator();
                if let Some(response) = response {
                    ui.label("最近响应");
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.monospace(response);
                        });
                } else {
                    ui.weak("尚未请求目录。上传完成后会自动提示，可刷新列表确认。");
                }
            });
        self.group_file_panel.open = open;

        if list_directory {
            let start = self
                .group_file_panel
                .list_start
                .trim()
                .parse::<i64>()
                .unwrap_or(0);
            self.send_file_manager_event(
                group_id,
                "ls",
                vec![
                    json!(self.group_file_panel.directory_fid.trim()),
                    json!(start.max(0)),
                ],
                true,
            );
        }
        if download_file {
            let fid = self.group_file_panel.file_fid.trim().to_string();
            if fid.is_empty() {
                self.bridge_states[bridge_idx].last_error = Some("请先填写文件 FID".to_string());
            } else {
                self.send_file_manager_event(group_id, "download", vec![json!(fid)], true);
            }
        }
        if create_directory {
            let name = self.group_file_panel.folder_name.trim().to_string();
            if name.is_empty() {
                self.bridge_states[bridge_idx].last_error = Some("文件夹名称不能为空".to_string());
            } else {
                self.send_file_manager_event(group_id, "mkdir", vec![json!(name)], true);
            }
        }
        if let Some(path) = upload_path {
            match std::fs::read(&path) {
                Ok(file_data) => {
                    let file_name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file")
                        .to_string();
                    if let Err(error) =
                        self.bridge_states[bridge_idx].send(IcaCommand::UploadGroupFile {
                            group_id,
                            parent_id: self.group_file_panel.directory_fid.trim().to_string(),
                            file_name,
                            file_data: std::sync::Arc::from(file_data),
                        })
                    {
                        self.bridge_states[bridge_idx].last_error =
                            Some(format!("群文件上传请求失败: {error}"));
                    } else {
                        self.bridge_states[bridge_idx].last_notice =
                            Some("正在上传群文件…".to_string());
                    }
                }
                Err(error) => {
                    self.bridge_states[bridge_idx].last_error =
                        Some(format!("读取待上传文件失败: {error}"));
                }
            }
        }
    }
}
