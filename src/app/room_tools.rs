use serde_json::{Value as JsonValue, json};

use super::IcaApp;

#[derive(Debug, Clone)]
pub struct RoomToolsState {
    pub room_id: String,
    pub room_name: String,
    pub priority: String,
    pub auto_download: bool,
    pub auto_download_path: String,
}

impl Default for RoomToolsState {
    fn default() -> Self {
        Self {
            room_id: String::new(),
            room_name: String::new(),
            priority: "3".to_string(),
            auto_download: false,
            auto_download_path: String::new(),
        }
    }
}

enum RoomToolAction {
    Call {
        event: &'static str,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    FillSelectedRoom,
}

impl IcaApp {
    fn selected_room_tool_room_id(&self) -> Option<i64> {
        self.active_bridge_state()?.selected_room_id
    }

    fn parse_room_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    fn execute_room_tool_action(&mut self, action: RoomToolAction) {
        match action {
            RoomToolAction::FillSelectedRoom => {
                if let Some(room_id) = self.selected_room_tool_room_id() {
                    self.room_tools.room_id = room_id.to_string();
                    if let Some((room_name, priority)) = self
                        .active_bridge_state()
                        .and_then(|state| state.rooms.iter().find(|room| room.room_id == room_id))
                        .map(|room| (room.room_name.clone(), room.priority))
                    {
                        self.room_tools.room_name = room_name;
                        self.room_tools.priority = priority.to_string();
                    }
                } else if let Some(state) = self.active_bridge_state_mut() {
                    state.last_error = Some("当前没有选中会话".to_string());
                }
            }
            RoomToolAction::Call {
                event,
                args,
                expect_ack,
            } => self.send_socket_api_event(event, args, expect_ack),
        }
    }

    pub fn render_room_tools_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.room_tools;
        let selected_room_id = self.selected_room_tool_room_id();
        let last_response = self
            .active_bridge_state()
            .and_then(|state| state.last_socket_api_response.clone());
        let mut pending_action = None;
        let mut pending_error = None;

        egui::Window::new("会话设置")
            .open(&mut open)
            .default_size(egui::vec2(460.0, 500.0))
            .min_size(egui::vec2(320.0, 320.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("会话 ID");
                    ui.add_sized(
                        [150.0, 0.0],
                        egui::TextEdit::singleline(&mut self.room_tools.room_id),
                    );
                    if ui.button("使用当前会话").clicked() {
                        pending_action = Some(RoomToolAction::FillSelectedRoom);
                    }
                    if let Some(room_id) = selected_room_id {
                        ui.weak(format!("当前: {}", room_id));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("获取会话").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "getRoom",
                                    args: vec![json!(room_id)],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("移除会话").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "removeChat",
                                    args: vec![json!(room_id)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("忽略会话").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "ignoreChat",
                                    args: vec![json!({
                                        "id": room_id,
                                        "name": self.room_tools.room_name,
                                    })],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("移除忽略").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "removeIgnoredChat",
                                    args: vec![json!(room_id)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("会话名称");
                    ui.add_sized(
                        [240.0, 0.0],
                        egui::TextEdit::singleline(&mut self.room_tools.room_name),
                    );
                    if ui.button("添加会话").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "addRoom",
                                    args: vec![json!({
                                        "roomId": room_id,
                                        "roomName": self.room_tools.room_name,
                                        "index": 0,
                                        "unreadCount": 0,
                                        "priority": self.room_tools.priority.parse::<u8>().unwrap_or(3),
                                        "utime": 0,
                                        "at": false,
                                        "lastMessage": {},
                                    })],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("优先级");
                    ui.add_sized(
                        [48.0, 0.0],
                        egui::TextEdit::singleline(&mut self.room_tools.priority),
                    );
                    if ui.button("设置优先级").clicked() {
                        match (
                            Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID"),
                            self.room_tools.priority.trim().parse::<u8>(),
                        ) {
                            (Ok(room_id), Ok(priority)) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "setRoomPriority",
                                    args: vec![json!(room_id), json!(priority)],
                                    expect_ack: false,
                                });
                            }
                            (Err(e), _) => pending_error = Some(e),
                            (_, Err(_)) => pending_error = Some("优先级不是有效数字".to_string()),
                        }
                    }
                    if ui.button("清未读").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "updateRoom",
                                    args: vec![json!(room_id), json!({"unreadCount": 0, "at": false})],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("置顶").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "pinRoom",
                                    args: vec![json!(room_id), json!(true)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("取消置顶").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "pinRoom",
                                    args: vec![json!(room_id), json!(false)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.room_tools.auto_download, "自动下载");
                    if ui.button("应用").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "setRoomAutoDownload",
                                    args: vec![json!(room_id), json!(self.room_tools.auto_download)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("下载路径");
                    ui.add_sized(
                        [280.0, 0.0],
                        egui::TextEdit::singleline(&mut self.room_tools.auto_download_path),
                    );
                    if ui.button("设置路径").clicked() {
                        match Self::parse_room_tool_i64(&self.room_tools.room_id, "会话 ID") {
                            Ok(room_id) => {
                                pending_action = Some(RoomToolAction::Call {
                                    event: "setRoomAutoDownloadPath",
                                    args: vec![
                                        json!(room_id),
                                        json!(self.room_tools.auto_download_path.trim()),
                                    ],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                if ui.button("获取忽略会话列表").clicked() {
                    pending_action = Some(RoomToolAction::Call {
                        event: "getIgnoredChats",
                        args: vec![],
                        expect_ack: true,
                    });
                }

                if let Some(error) = &pending_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if let Some(response) = &last_response {
                    ui.separator();
                    ui.label("最近响应");
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            ui.monospace(response);
                        });
                }
            });

        self.open_page.room_tools = open;
        if let Some(error) = pending_error
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.last_error = Some(error);
        }
        if let Some(action) = pending_action {
            self.execute_room_tool_action(action);
        }
    }
}
