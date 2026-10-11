//! 会话设置工具面板。
//!
//! 位于 `crate::app::tools` 之下，聚合“获取/移除/忽略会话、添加会话、优先级、
//! 置顶、清未读、自动下载及路径”等一类会话（room）相关的 bridge Socket API。

use crate::app::IcaApp;
use serde_json::{Value as JsonValue, json};

/// 会话设置面板的输入状态。
#[derive(Debug, Clone)]
pub struct RoomToolsState {
    /// 会话 ID（roomId）；群聊为负、私聊为正。
    pub room_id: String,
    /// 会话名称，添加/忽略会话时作为展示名。
    pub room_name: String,
    /// 会话优先级字符串，默认 `"3"`；解析为 `u8` 后随设置/添加一起下发。
    pub priority: String,
    /// 是否开启该会话的自动下载（`setRoomAutoDownload`）。
    pub auto_download: bool,
    /// 自动下载的目标路径（`setRoomAutoDownloadPath`）。
    pub auto_download_path: String,
}

/// 默认优先级为 `"3"`、关闭自动下载，其余留空。
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

/// 会画面板要延迟执行的动作，闭包结束后统一执行。
enum RoomToolAction {
    /// 走普通 Socket API 通道下发一个事件。
    Call {
        /// bridge 端事件名。
        event: &'static str,
        /// 事件的 JSON 参数数组。
        args: Vec<JsonValue>,
        /// 是否期待 bridge 回报 ack。
        expect_ack: bool,
    },
    /// 把“当前选中的会话”信息回填到输入框。
    FillSelectedRoom,
}

/// `IcaApp` 的会话设置工具固有方法：会话回填、参数解析、动作执行与窗口渲染。
impl IcaApp {
    /// 返回当前选中会话的 ID；没有选中 / 没有活动 bridge 时返回 `None`。
    fn selected_room_tool_room_id(&self) -> Option<i64> {
        self.active_bridge_state()?.selected_room_id
    }

    /// 去除首尾空白后把输入解析为 `i64`；失败返回 `<label> 不是有效数字`。
    fn parse_room_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    /// 执行一个延迟的会画面板动作：回填选中会话或走 Socket API 下发。
    fn execute_room_tool_action(&mut self, action: RoomToolAction) {
        match action {
            RoomToolAction::FillSelectedRoom => {
                if let Some(room_id) = self.selected_room_tool_room_id() {
                    self.room_tools.room_id = room_id.to_string();
                    // 顺带从已加载会话列表带回名称与优先级，省去手动填写。
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

    /// 绘制“会话设置”窗口。
    ///
    /// 按钮只做校验并记录意图，闭包结束后统一 flush 错误与动作。
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

        // 闭包已结束，统一落地渲染期间收集到的错误与动作。
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
