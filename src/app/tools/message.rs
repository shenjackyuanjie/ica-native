use serde_json::{Value as JsonValue, json};

use crate::app::IcaApp;

#[derive(Debug, Clone)]
pub struct MessageToolsState {
    pub room_id: String,
    pub keyword: String,
    pub sender_id: String,
    pub offset: String,
    pub message_id: String,
    pub before: String,
    pub after: String,
    pub forward_res_id: String,
    pub forward_file_name: String,
}

impl Default for MessageToolsState {
    fn default() -> Self {
        Self {
            room_id: String::new(),
            keyword: String::new(),
            sender_id: String::new(),
            offset: "0".to_string(),
            message_id: String::new(),
            before: "20".to_string(),
            after: "20".to_string(),
            forward_res_id: String::new(),
            forward_file_name: String::new(),
        }
    }
}

enum MessageToolAction {
    Call {
        event: &'static str,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
    FillSelectedRoom,
}

impl IcaApp {
    fn parse_message_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    fn selected_message_tool_room_id(&self) -> Option<i64> {
        self.active_bridge_state()?.selected_room_id
    }

    fn execute_message_tool_action(&mut self, action: MessageToolAction) {
        match action {
            MessageToolAction::FillSelectedRoom => {
                if let Some(room_id) = self.selected_message_tool_room_id() {
                    self.message_tools.room_id = room_id.to_string();
                } else if let Some(state) = self.active_bridge_state_mut() {
                    state.last_error = Some("当前没有选中会话".to_string());
                }
            }
            MessageToolAction::Call {
                event,
                args,
                expect_ack,
            } => self.send_socket_api_event(event, args, expect_ack),
        }
    }

    pub fn render_message_tools_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.message_tools;
        let selected_room_id = self.selected_message_tool_room_id();
        let last_response = self
            .active_bridge_state()
            .and_then(|state| state.last_socket_api_response.clone());
        let mut pending_action = None;
        let mut pending_error = None;

        egui::Window::new("消息检索/历史")
            .open(&mut open)
            .default_size(egui::vec2(480.0, 540.0))
            .min_size(egui::vec2(320.0, 340.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("会话 ID");
                    ui.add_sized(
                        [150.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.room_id),
                    );
                    if ui.button("使用当前会话").clicked() {
                        pending_action = Some(MessageToolAction::FillSelectedRoom);
                    }
                    if let Some(room_id) = selected_room_id {
                        ui.weak(format!("当前: {}", room_id));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("偏移量");
                    ui.add_sized(
                        [80.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.offset),
                    );
                    if ui.button("取消息").clicked() {
                        match (
                            Self::parse_message_tool_i64(&self.message_tools.room_id, "会话 ID"),
                            Self::parse_message_tool_i64(&self.message_tools.offset, "offset"),
                        ) {
                            (Ok(room_id), Ok(offset)) => {
                                pending_action = Some(MessageToolAction::Call {
                                    event: "fetchMessages",
                                    args: vec![json!(room_id), json!(offset)],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                    if ui.button("取图片消息").clicked() {
                        match (
                            Self::parse_message_tool_i64(&self.message_tools.room_id, "会话 ID"),
                            Self::parse_message_tool_i64(&self.message_tools.offset, "offset"),
                        ) {
                            (Ok(room_id), Ok(offset)) => {
                                pending_action = Some(MessageToolAction::Call {
                                    event: "fetchImageMessages",
                                    args: vec![json!(room_id), json!(offset), JsonValue::Null],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("关键词");
                    ui.add_sized(
                        [220.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.keyword),
                    );
                    if ui.button("搜索").clicked() {
                        match (
                            Self::parse_message_tool_i64(&self.message_tools.room_id, "会话 ID"),
                            Self::parse_message_tool_i64(&self.message_tools.offset, "offset"),
                        ) {
                            (Ok(room_id), Ok(offset)) => {
                                pending_action = Some(MessageToolAction::Call {
                                    event: "searchMessages",
                                    args: vec![
                                        json!(room_id),
                                        json!(self.message_tools.keyword.trim()),
                                        json!(offset),
                                    ],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("发送者 QQ");
                    ui.add_sized(
                        [140.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.sender_id),
                    );
                    if ui.button("按发送者取消息").clicked() {
                        match (
                            Self::parse_message_tool_i64(&self.message_tools.room_id, "会话 ID"),
                            Self::parse_message_tool_i64(
                                &self.message_tools.sender_id,
                                "发送者 QQ",
                            ),
                            Self::parse_message_tool_i64(&self.message_tools.offset, "offset"),
                        ) {
                            (Ok(room_id), Ok(sender_id), Ok(offset)) => {
                                pending_action = Some(MessageToolAction::Call {
                                    event: "fetchMessagesBySender",
                                    args: vec![json!(room_id), json!(sender_id), json!(offset)],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                                pending_error = Some(e);
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("消息 ID");
                    ui.add_sized(
                        [190.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.message_id),
                    );
                    ui.label("前");
                    ui.add_sized(
                        [52.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.before),
                    );
                    ui.label("后");
                    ui.add_sized(
                        [52.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.after),
                    );
                    if ui.button("取上下文").clicked() {
                        match (
                            Self::parse_message_tool_i64(&self.message_tools.room_id, "会话 ID"),
                            Self::parse_message_tool_i64(&self.message_tools.before, "前"),
                            Self::parse_message_tool_i64(&self.message_tools.after, "后"),
                        ) {
                            (Ok(room_id), Ok(before), Ok(after)) => {
                                pending_action = Some(MessageToolAction::Call {
                                    event: "fetchMessagesAround",
                                    args: vec![
                                        json!(room_id),
                                        json!(self.message_tools.message_id.trim()),
                                        json!(before),
                                        json!(after),
                                    ],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                                pending_error = Some(e);
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("获取首次未读").clicked() {
                        pending_action = Some(MessageToolAction::Call {
                            event: "getFirstUnreadRoom",
                            args: vec![json!(self.notify_level)],
                            expect_ack: true,
                        });
                    }
                    if ui.button("获取未读数").clicked() {
                        pending_action = Some(MessageToolAction::Call {
                            event: "getUnreadCount",
                            args: vec![],
                            expect_ack: true,
                        });
                    }
                    if ui.button("拉取 7 天历史").clicked() {
                        pending_action = Some(MessageToolAction::Call {
                            event: "fetch7DaysHistory",
                            args: vec![],
                            expect_ack: false,
                        });
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("转发 resId");
                    ui.add_sized(
                        [160.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.forward_res_id),
                    );
                    ui.label("文件名");
                    ui.add_sized(
                        [120.0, 0.0],
                        egui::TextEdit::singleline(&mut self.message_tools.forward_file_name),
                    );
                    if ui.button("读取转发").clicked() {
                        let res_id = self.message_tools.forward_res_id.trim();
                        if res_id.is_empty() {
                            pending_error = Some("转发 resId 不能为空".to_string());
                        } else {
                            pending_action = Some(MessageToolAction::Call {
                                event: "getForwardMsg",
                                args: vec![
                                    json!(res_id),
                                    json!(self.message_tools.forward_file_name.trim()),
                                ],
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
                        .max_height(150.0)
                        .show(ui, |ui| {
                            ui.monospace(response);
                        });
                }
            });

        self.open_page.message_tools = open;
        if let Some(error) = pending_error
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.last_error = Some(error);
        }
        if let Some(action) = pending_action {
            self.execute_message_tool_action(action);
        }
    }
}
