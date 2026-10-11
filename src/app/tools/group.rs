//! 群 / 成员管理工具面板。
//!
//! 位于 `crate::app::tools` 之下，聚合“群资料、成员列表、禁言/解禁、踢出、
//! 群名片/备注、好友备注”等一类群与成员相关的 bridge Socket API。
//! 禁言是敏感操作，不直接下发，而是转交群成员面板的确认流程
//! （`GroupBanConfirmation`）后再执行。

use crate::app::IcaApp;
use crate::app::state::GroupBanConfirmation;
use crate::ica::GROUP_BAN_MAX_DURATION;
use serde_json::{Value as JsonValue, json};

/// 群 / 成员管理面板的输入状态。
#[derive(Debug, Clone)]
pub struct GroupToolsState {
    /// 群号（gin）。
    pub group_id: String,
    /// 成员 QQ，用于成员资料/禁言/解禁/踢出。
    pub member_id: String,
    /// 禁言秒数，默认 `"600"`；必须在 `1..=GROUP_BAN_MAX_DURATION` 之间。
    pub ban_seconds: String,
    /// 我在该群的群名片，`setGroupNick` 用。
    pub group_nick: String,
    /// 群备注，`setGroupRemark` 用。
    pub group_remark: String,
    /// 好友 QQ，`setFriendRemark` 用。
    pub friend_id: String,
    /// 好友备注名。
    pub friend_remark: String,
}

/// 默认禁言秒数预填 `"600"`（10 分钟），其余留空。
impl Default for GroupToolsState {
    fn default() -> Self {
        Self {
            group_id: String::new(),
            member_id: String::new(),
            ban_seconds: "600".to_string(),
            group_nick: String::new(),
            group_remark: String::new(),
            friend_id: String::new(),
            friend_remark: String::new(),
        }
    }
}

/// 群 / 成员面板要延迟执行的动作，闭包结束后统一执行。
enum GroupToolAction {
    /// 走普通 Socket API 通道下发一个事件。
    Call {
        /// bridge 端事件名。
        event: &'static str,
        /// 事件的 JSON 参数数组。
        args: Vec<JsonValue>,
        /// 是否期待 bridge 回报 ack。
        expect_ack: bool,
    },
    /// 请求禁言/解禁某成员；转交群成员面板确认流程，不直接下发。
    SetGroupBan {
        /// 群号（gin）。
        group_id: i64,
        /// 目标成员 QQ。
        target_id: i64,
        /// 禁言秒数；`0` 表示解除禁言。
        duration: u64,
    },
    /// 把“当前选中的群聊”群号回填到输入框。
    FillSelectedGroup,
}

/// `IcaApp` 的群/成员工具固有方法：群号回填、参数解析、动作执行与窗口渲染。
impl IcaApp {
    /// 返回当前选中会话对应的群号（gin）；非群聊或未选中时返回 `None`。
    ///
    /// 群聊的 `room_id` 约定为负数，取相反数即群号。
    fn selected_group_gin(&self) -> Option<i64> {
        let room_id = self.active_bridge_state()?.selected_room_id?;
        (room_id < 0).then_some(-room_id)
    }

    /// 去除首尾空白后把输入解析为 `i64`；失败返回 `<label> 不是有效数字`。
    fn parse_group_tool_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    /// 去除首尾空白后把输入解析为 `u64`（禁言秒数专用）；失败返回 `<label> 不是有效数字`。
    fn parse_group_tool_u64(value: &str, label: &str) -> Result<u64, String> {
        value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    /// 执行一个延迟的群 / 成员面板动作。
    ///
    /// 除普通 `Call` 外，`SetGroupBan` 不直接下发，而是校验群号后写入
    /// `group_member_panel.confirmation`，交由群成员面板的确认流程处理。
    fn execute_group_tool_action(&mut self, action: GroupToolAction) {
        match action {
            GroupToolAction::FillSelectedGroup => {
                if let Some(gin) = self.selected_group_gin() {
                    self.group_tools.group_id = gin.to_string();
                } else if let Some(state) = self.active_bridge_state_mut() {
                    state.last_error = Some("当前选中的不是群聊".to_string());
                }
            }
            GroupToolAction::Call {
                event,
                args,
                expect_ack,
            } => {
                self.send_socket_api_event(event, args, expect_ack);
            }
            GroupToolAction::SetGroupBan {
                group_id,
                target_id,
                duration,
            } => {
                // 取绝对值得到群号并拒绝 0；确认对象用约定的负数 room_id。
                let Some(group_id) = group_id.checked_abs().filter(|group_id| *group_id > 0) else {
                    if let Some(state) = self.active_bridge_state_mut() {
                        state.last_error = Some("群号无效".to_string());
                    }
                    return;
                };
                self.group_member_panel.confirmation = Some(GroupBanConfirmation {
                    room_id: -group_id,
                    target_id,
                    target_name: target_id.to_string(),
                    duration,
                });
            }
        }
    }

    /// 绘制“群/成员管理”窗口。
    ///
    /// 禁言按钮会先校验秒数范围（`1..=GROUP_BAN_MAX_DURATION`），再转确认流程；
    /// 其它按钮只做校验并记录意图，闭包结束后统一 flush 错误与动作。
    pub fn render_group_tools_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.group_tools;
        let selected_group_gin = self.selected_group_gin();
        let last_response = self
            .active_bridge_state()
            .and_then(|state| state.last_socket_api_response.clone());
        let mut pending_action = None;
        let mut pending_error = None;

        egui::Window::new("群/成员管理")
            .open(&mut open)
            .default_size(egui::vec2(460.0, 560.0))
            .min_size(egui::vec2(320.0, 360.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("群号");
                    ui.add_sized(
                        [150.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.group_id),
                    );
                    if ui.button("使用当前群聊").clicked() {
                        pending_action = Some(GroupToolAction::FillSelectedGroup);
                    }
                    if let Some(gin) = selected_group_gin {
                        ui.weak(format!("当前: {}", gin));
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("获取群资料").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "getGroup",
                                    args: vec![json!(gin)],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("获取成员列表").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "getGroupMembers",
                                    args: vec![json!(gin)],
                                    expect_ack: true,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                    if ui.button("退出群").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "setGroupLeave",
                                    args: vec![json!(gin)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("成员 QQ");
                    ui.add_sized(
                        [150.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.member_id),
                    );
                    ui.label("禁言秒数");
                    ui.add_sized(
                        [88.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.ban_seconds),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("成员资料").clicked() {
                        match (
                            Self::parse_group_tool_i64(&self.group_tools.group_id, "群号"),
                            Self::parse_group_tool_i64(&self.group_tools.member_id, "成员 QQ"),
                        ) {
                            (Ok(gin), Ok(uin)) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "getGroupMemberInfo",
                                    args: vec![json!(gin), json!(uin), json!(true)],
                                    expect_ack: true,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                    if ui.button("禁言").clicked() {
                        match (
                            Self::parse_group_tool_i64(&self.group_tools.group_id, "群号"),
                            Self::parse_group_tool_i64(&self.group_tools.member_id, "成员 QQ"),
                            Self::parse_group_tool_u64(&self.group_tools.ban_seconds, "禁言秒数"),
                        ) {
                            (Ok(gin), Ok(uin), Ok(duration))
                                if (1..=GROUP_BAN_MAX_DURATION).contains(&duration) =>
                            {
                                pending_action = Some(GroupToolAction::SetGroupBan {
                                    group_id: gin,
                                    target_id: uin,
                                    duration,
                                });
                            }
                            (Ok(_), Ok(_), Ok(_)) => {
                                pending_error = Some(format!(
                                    "禁言秒数必须在 1..={} 之间",
                                    GROUP_BAN_MAX_DURATION
                                ));
                            }
                            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                                pending_error = Some(e);
                            }
                        }
                    }
                    if ui.button("解除禁言").clicked() {
                        match (
                            Self::parse_group_tool_i64(&self.group_tools.group_id, "群号"),
                            Self::parse_group_tool_i64(&self.group_tools.member_id, "成员 QQ"),
                        ) {
                            (Ok(gin), Ok(uin)) => {
                                pending_action = Some(GroupToolAction::SetGroupBan {
                                    group_id: gin,
                                    target_id: uin,
                                    duration: 0,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                    if ui.button("踢出").clicked() {
                        match (
                            Self::parse_group_tool_i64(&self.group_tools.group_id, "群号"),
                            Self::parse_group_tool_i64(&self.group_tools.member_id, "成员 QQ"),
                        ) {
                            (Ok(gin), Ok(uin)) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "setGroupKick",
                                    args: vec![json!(gin), json!(uin)],
                                    expect_ack: false,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                });

                ui.separator();
                ui.label("备注/名片");
                ui.horizontal_wrapped(|ui| {
                    ui.label("我的群名片");
                    ui.add_sized(
                        [220.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.group_nick),
                    );
                    if ui.button("设置").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "setGroupNick",
                                    args: vec![json!(gin), json!(self.group_tools.group_nick)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("群备注");
                    ui.add_sized(
                        [220.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.group_remark),
                    );
                    if ui.button("设置").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.group_id, "群号") {
                            Ok(gin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "setGroupRemark",
                                    args: vec![json!(gin), json!(self.group_tools.group_remark)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("好友 QQ");
                    ui.add_sized(
                        [120.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.friend_id),
                    );
                    ui.label("备注");
                    ui.add_sized(
                        [180.0, 0.0],
                        egui::TextEdit::singleline(&mut self.group_tools.friend_remark),
                    );
                    if ui.button("设置").clicked() {
                        match Self::parse_group_tool_i64(&self.group_tools.friend_id, "好友 QQ") {
                            Ok(uin) => {
                                pending_action = Some(GroupToolAction::Call {
                                    event: "setFriendRemark",
                                    args: vec![json!(uin), json!(self.group_tools.friend_remark)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
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
        self.open_page.group_tools = open;
        if let Some(error) = pending_error
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.last_error = Some(error);
        }
        if let Some(action) = pending_action {
            self.execute_group_tool_action(action);
        }
    }
}
