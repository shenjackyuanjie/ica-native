use serde_json::{Value as JsonValue, json};

use crate::ica::IcaCommand;

use crate::app::IcaApp;

#[derive(Debug, Clone)]
pub struct FileToolsState {
    pub group_id: String,
    pub fid: String,
    pub private_file_id: String,
    pub cookie_domain: String,
    pub message_id: String,
    pub dir_fid: String,
    pub list_start: String,
    pub target_dir_fid: String,
    pub folder_name: String,
    pub new_name: String,
}

impl Default for FileToolsState {
    fn default() -> Self {
        Self {
            group_id: String::new(),
            fid: String::new(),
            private_file_id: String::new(),
            cookie_domain: "qq.com".to_string(),
            message_id: String::new(),
            dir_fid: String::new(),
            list_start: "0".to_string(),
            target_dir_fid: String::new(),
            folder_name: String::new(),
            new_name: String::new(),
        }
    }
}

enum FileToolAction {
    Call {
        event: &'static str,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    FileManagerCall {
        gin: i64,
        event: &'static str,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    FillSelectedGroup,
}

impl IcaApp {
    fn selected_file_tool_gin(&self) -> Option<i64> {
        let room_id = self.active_bridge_state()?.selected_room_id?;
        (room_id < 0).then_some(-room_id)
    }

    fn parse_file_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    fn execute_file_tool_action(&mut self, action: FileToolAction) {
        match action {
            FileToolAction::FillSelectedGroup => {
                if let Some(gin) = self.selected_file_tool_gin() {
                    self.file_tools.group_id = gin.to_string();
                } else if let Some(state) = self.active_bridge_state_mut() {
                    state.last_error = Some("当前选中的不是群聊".to_string());
                }
            }
            FileToolAction::Call {
                event,
                args,
                expect_ack,
            } => self.send_socket_api_event(event, args, expect_ack),
            FileToolAction::FileManagerCall {
                gin,
                event,
                args,
                expect_ack,
            } => self.send_file_manager_event(gin, event, args, expect_ack),
        }
    }

    pub fn send_file_manager_event(
        &mut self,
        gin: i64,
        event: impl Into<String>,
        args: Vec<JsonValue>,
        expect_ack: bool,
    ) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let command = IcaCommand::FileManagerCall {
            gin,
            event: event.into(),
            args,
            expect_ack,
        };
        if let Err(e) = self.bridge_states[bridge_idx].send(command) {
            tracing::warn!(error = %e, "发送文件管理命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("文件管理命令发送失败".to_string());
            }
        }
    }

    pub fn render_file_tools_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.file_tools;
        let selected_group_gin = self.selected_file_tool_gin();
        let last_response = self
            .active_bridge_state()
            .and_then(|state| state.last_socket_api_response.clone());
        let mut pending_action = None;
        let mut pending_error = None;

        egui::Window::new("文件/资源工具")
            .open(&mut open)
            .default_size(egui::vec2(460.0, 460.0))
            .min_size(egui::vec2(320.0, 300.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("群号");
                    ui.add_sized(
                        [150.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.group_id),
                    );
                    if ui.button("使用当前群聊").clicked() {
                        pending_action = Some(FileToolAction::FillSelectedGroup);
                    }
                    if let Some(gin) = selected_group_gin {
                        ui.weak(format!("当前: {}", gin));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("文件 ID");
                    ui.add_sized(
                        [260.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.fid),
                    );
                    if ui.button("群文件元信息").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::Call {
                                    event: "getGroupFileMeta",
                                    args: vec![json!(gin), json!(self.file_tools.fid.trim())],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("请求群文件 token").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::Call {
                                    event: "requestGfsToken",
                                    args: vec![json!(gin)],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.label("群文件管理器");
                ui.horizontal_wrapped(|ui| {
                    ui.label("目录 fid");
                    ui.add_sized(
                        [180.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.dir_fid),
                    );
                    ui.label("起始位置");
                    ui.add_sized(
                        [64.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.list_start),
                    );
                    if ui.button("列出目录").clicked() {
                        match (
                            Self::parse_file_tool_i64(&self.file_tools.group_id, "群号"),
                            Self::parse_file_tool_i64(&self.file_tools.list_start, "start"),
                        ) {
                            (Ok(gin), Ok(start)) => {
                                pending_action = Some(FileToolAction::FileManagerCall {
                                    gin,
                                    event: "ls",
                                    args: vec![json!(self.file_tools.dir_fid.trim()), json!(start)],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("下载链接").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::FileManagerCall {
                                    gin,
                                    event: "download",
                                    args: vec![json!(self.file_tools.fid.trim())],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("文件详情").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::FileManagerCall {
                                    gin,
                                    event: "stat",
                                    args: vec![json!(self.file_tools.fid.trim())],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("删除文件").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::FileManagerCall {
                                    gin,
                                    event: "rm",
                                    args: vec![json!(self.file_tools.fid.trim())],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("新文件夹");
                    ui.add_sized(
                        [160.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.folder_name),
                    );
                    if ui.button("创建").clicked() {
                        let name = self.file_tools.folder_name.trim();
                        if name.is_empty() {
                            pending_error = Some("新文件夹名称不能为空".to_string());
                        } else {
                            match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                                Ok(gin) => {
                                    pending_action = Some(FileToolAction::FileManagerCall {
                                        gin,
                                        event: "mkdir",
                                        args: vec![json!(name)],
                                        expect_ack: true,
                                    });
                                }
                                Err(e) => pending_error = Some(e),
                            }
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("目标目录");
                    ui.add_sized(
                        [160.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.target_dir_fid),
                    );
                    if ui.button("移动 fid").clicked() {
                        match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(FileToolAction::FileManagerCall {
                                    gin,
                                    event: "mv",
                                    args: vec![
                                        json!(self.file_tools.fid.trim()),
                                        json!(self.file_tools.target_dir_fid.trim()),
                                    ],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("新名称");
                    ui.add_sized(
                        [180.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.new_name),
                    );
                    if ui.button("重命名 fid").clicked() {
                        let name = self.file_tools.new_name.trim();
                        if name.is_empty() {
                            pending_error = Some("新名称不能为空".to_string());
                        } else {
                            match Self::parse_file_tool_i64(&self.file_tools.group_id, "群号") {
                                Ok(gin) => {
                                    pending_action = Some(FileToolAction::FileManagerCall {
                                        gin,
                                        event: "rename",
                                        args: vec![json!(self.file_tools.fid.trim()), json!(name)],
                                        expect_ack: true,
                                    });
                                }
                                Err(e) => pending_error = Some(e),
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("私聊文件 ID");
                    ui.add_sized(
                        [220.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.private_file_id),
                    );
                    if ui.button("获取 URL").clicked() {
                        let file_id = self.file_tools.private_file_id.trim();
                        if file_id.is_empty() {
                            pending_error = Some("私聊文件 ID 不能为空".to_string());
                        } else {
                            pending_action = Some(FileToolAction::Call {
                                event: "getPrivateFileUrl",
                                args: vec![json!(file_id)],
                                expect_ack: true,
                            });
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("消息 ID");
                    ui.add_sized(
                        [240.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.message_id),
                    );
                    if ui.button("刷新图片 URL").clicked() {
                        let message_id = self.file_tools.message_id.trim();
                        if message_id.is_empty() {
                            pending_error = Some("消息 ID 不能为空".to_string());
                        } else {
                            pending_action = Some(FileToolAction::Call {
                                event: "getMsgNewURL",
                                args: vec![json!(message_id)],
                                expect_ack: true,
                            });
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Cookie 域名");
                    ui.add_sized(
                        [220.0, 0.0],
                        egui::TextEdit::singleline(&mut self.file_tools.cookie_domain),
                    );
                    if ui.button("获取 cookies").clicked() {
                        let domain = self.file_tools.cookie_domain.trim();
                        if domain.is_empty() {
                            pending_error = Some("Cookie 域名不能为空".to_string());
                        } else {
                            pending_action = Some(FileToolAction::Call {
                                event: "getCookies",
                                args: vec![json!(domain)],
                                expect_ack: true,
                            });
                        }
                    }
                });

                if let Some(error) = &pending_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if let Some(response) = &last_response {
                    ui.separator();
                    ui.label("最近响应");
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.monospace(response);
                        });
                }
            });

        self.open_page.file_tools = open;
        if let Some(error) = pending_error
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.last_error = Some(error);
        }
        if let Some(action) = pending_action {
            self.execute_file_tool_action(action);
        }
    }
}
