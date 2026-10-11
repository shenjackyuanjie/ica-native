//! 文件 / 资源工具面板。
//!
//! 位于 `crate::app::tools` 之下，聚合“群文件元信息、群文件管理器、私聊文件 URL、
//! 图片 URL 刷新、cookies 获取”等一类文件/资源相关的 bridge Socket API。
//! 除普通 Socket API 外，群文件管理器走独立的 `IcaCommand::FileManagerCall` 通道，
//! 由 `send_file_manager_event` 负责下发。

use crate::app::IcaApp;
use crate::ica::IcaCommand;
use serde_json::{Value as JsonValue, json};

/// 文件 / 资源面板的输入状态。
///
/// 字段同样是用户编辑的原始文本；数字字段（群号、起始位置等）在点击按钮时解析，
/// 失败只提示，不回写。
#[derive(Debug, Clone)]
pub struct FileToolsState {
    /// 群号（gin），群文件相关操作的目标群。
    pub group_id: String,
    /// 群文件 ID（fid），用于元信息/下载/详情/删除/移动/重命名。
    pub fid: String,
    /// 私聊文件 ID，用于 `getPrivateFileUrl`。
    pub private_file_id: String,
    /// 获取 cookies 的目标域名，默认 `"qq.com"`。
    pub cookie_domain: String,
    /// 消息 ID，用于 `getMsgNewURL` 刷新图片地址。
    pub message_id: String,
    /// 群文件管理器中的目录 fid；列出目录/创建/移动时作为目标或父目录。
    pub dir_fid: String,
    /// 列出目录时的起始位置（分页偏移），默认 `"0"`。
    pub list_start: String,
    /// 移动文件时的目标目录 fid。
    pub target_dir_fid: String,
    /// 新建文件夹名称。
    pub folder_name: String,
    /// 重命名 fid 时的新名称。
    pub new_name: String,
}

/// 默认把 Cookie 域名预填为 `"qq.com"`、起始位置为 `"0"`，其余留空。
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

/// 文件面板要延迟执行的动作，闭包结束后统一执行。
enum FileToolAction {
    /// 走普通 Socket API 通道下发一个事件。
    Call {
        /// bridge 端事件名。
        event: &'static str,
        /// 事件的 JSON 参数数组。
        args: Vec<JsonValue>,
        /// 是否期待 bridge 回报 ack。
        expect_ack: bool,
    },
    /// 走群文件管理器通道（`IcaCommand::FileManagerCall`）下发。
    FileManagerCall {
        /// 目标群号（gin）。
        gin: i64,
        /// 文件管理器子命令，如 `ls`、`download`、`mkdir`、`mv`、`rename`。
        event: &'static str,
        /// 子命令的 JSON 参数数组。
        args: Vec<JsonValue>,
        /// 是否期待 bridge 回报 ack。
        expect_ack: bool,
    },
    /// 把“当前选中的群聊”群号回填到输入框。
    FillSelectedGroup,
}

/// `IcaApp` 的文件/资源工具固有方法：群号回填、参数解析、命令下发与窗口渲染。
impl IcaApp {
    /// 返回当前选中会话对应的群号（gin）。
    ///
    /// 约定：群聊的 `room_id` 为负数，取其相反数即群号；
    /// 当选中的不是群聊（或没有选中 / 没有活动 bridge）时返回 `None`。
    fn selected_file_tool_gin(&self) -> Option<i64> {
        let room_id = self.active_bridge_state()?.selected_room_id?;
        (room_id < 0).then_some(-room_id)
    }

    /// 去除首尾空白后把输入解析为 `i64`；失败返回 `<label> 不是有效数字`。
    fn parse_file_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    /// 执行一个延迟的文件面板动作：回填群号、走 Socket API 或走文件管理器通道。
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

    /// 通过活动 bridge 的文件管理器通道下发一个子命令。
    ///
    /// - 参数 `gin`：目标群号；
    /// - 参数 `event`：文件管理器子命令名（如 `ls`、`rm`）；
    /// - 参数 `args`：子命令的 JSON 参数数组；
    /// - 参数 `expect_ack`：是否期待回报；
    /// - 副作用：向 bridge 发送 `IcaCommand::FileManagerCall`，发送失败时记录 `last_error`；
    ///   没有活动 bridge 时静默返回。
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

    /// 绘制“文件/资源工具”窗口。
    ///
    /// 与其它面板一致：按钮只做校验并记录意图，闭包结束后统一 flush 错误与动作。
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

        // 闭包已结束，统一落地渲染期间收集到的错误与动作。
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
