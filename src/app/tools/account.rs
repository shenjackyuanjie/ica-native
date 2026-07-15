use serde_json::{Value as JsonValue, json};

use crate::app::IcaApp;

#[derive(Debug, Clone)]
pub struct AccountToolsState {
    pub username: String,
    pub password: String,
    pub platform: String,
    pub sms_code: String,
    pub slider_ticket: String,
    pub delete_device_flag: String,
}

impl Default for AccountToolsState {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            platform: "5".to_string(),
            sms_code: String::new(),
            slider_ticket: String::new(),
            delete_device_flag: String::new(),
        }
    }
}

enum AccountToolAction {
    Call {
        event: &'static str,
        args: Vec<JsonValue>,
        expect_ack: bool,
    },
}

impl IcaApp {
    fn parse_account_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    fn execute_account_tool_action(&mut self, action: AccountToolAction) {
        match action {
            AccountToolAction::Call {
                event,
                args,
                expect_ack,
            } => self.send_socket_api_event(event, args, expect_ack),
        }
    }

    pub fn render_account_tools_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.account_tools;
        let last_response = self
            .active_bridge_state()
            .and_then(|state| state.last_socket_api_response.clone());
        let mut pending_action = None;
        let mut pending_error = None;

        egui::Window::new("账号/登录设备")
            .open(&mut open)
            .default_size(egui::vec2(460.0, 520.0))
            .min_size(egui::vec2(320.0, 340.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("获取登录设备").clicked() {
                        pending_action = Some(AccountToolAction::Call {
                            event: "getLoginDevices",
                            args: vec![],
                            expect_ack: true,
                        });
                    }
                    if ui.button("获取禁用功能").clicked() {
                        pending_action = Some(AccountToolAction::Call {
                            event: "getDisabledFeatures",
                            args: vec![],
                            expect_ack: true,
                        });
                    }
                    if ui.button("重新登录").clicked() {
                        pending_action = Some(AccountToolAction::Call {
                            event: "reLogin",
                            args: vec![],
                            expect_ack: false,
                        });
                    }
                });

                ui.separator();
                ui.label("删除登录设备");
                ui.horizontal_wrapped(|ui| {
                    ui.label("flag");
                    ui.add_sized(
                        [240.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.delete_device_flag),
                    );
                    if ui.button("删除").clicked() {
                        let flag = self.account_tools.delete_device_flag.trim();
                        if flag.is_empty() {
                            pending_error = Some("设备 flag 不能为空".to_string());
                        } else {
                            pending_action = Some(AccountToolAction::Call {
                                event: "deleteLoginDevice",
                                args: vec![json!(flag)],
                                expect_ack: false,
                            });
                        }
                    }
                });

                ui.separator();
                ui.label("登录/验证");
                ui.horizontal_wrapped(|ui| {
                    ui.label("QQ");
                    ui.add_sized(
                        [120.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.username),
                    );
                    ui.label("平台");
                    ui.add_sized(
                        [58.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.platform),
                    );
                    if ui.button("随机设备").clicked() {
                        match Self::parse_account_i64(&self.account_tools.username, "QQ") {
                            Ok(username) => {
                                pending_action = Some(AccountToolAction::Call {
                                    event: "randomDevice",
                                    args: vec![json!(username)],
                                    expect_ack: false,
                                });
                            }
                            Err(e) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("密码");
                    ui.add_sized(
                        [280.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.password).password(true),
                    );
                    if ui.button("远端登录").clicked() {
                        match (
                            Self::parse_account_i64(&self.account_tools.username, "QQ"),
                            Self::parse_account_i64(&self.account_tools.platform, "平台"),
                        ) {
                            (Ok(username), Ok(platform)) => {
                                pending_action = Some(AccountToolAction::Call {
                                    event: "login",
                                    args: vec![json!({
                                        "username": username,
                                        "password": self.account_tools.password,
                                        "platform": platform,
                                    })],
                                    expect_ack: false,
                                });
                            }
                            (Err(e), _) | (_, Err(e)) => pending_error = Some(e),
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("短信验证码");
                    ui.add_sized(
                        [160.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.sms_code),
                    );
                    if ui.button("提交").clicked() {
                        let code = self.account_tools.sms_code.trim();
                        if code.is_empty() {
                            pending_error = Some("短信验证码不能为空".to_string());
                        } else {
                            pending_action = Some(AccountToolAction::Call {
                                event: "submitSmsCode",
                                args: vec![json!(code)],
                                expect_ack: false,
                            });
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("滑块 ticket");
                    ui.add_sized(
                        [240.0, 0.0],
                        egui::TextEdit::singleline(&mut self.account_tools.slider_ticket),
                    );
                    if ui.button("提交").clicked() {
                        let ticket = self.account_tools.slider_ticket.trim();
                        if ticket.is_empty() {
                            pending_error = Some("滑块 ticket 不能为空".to_string());
                        } else {
                            pending_action = Some(AccountToolAction::Call {
                                event: "login-slider-ticket",
                                args: vec![json!(ticket)],
                                expect_ack: false,
                            });
                        }
                    }
                    if ui.button("验证窗口关闭后重登").clicked() {
                        pending_action = Some(AccountToolAction::Call {
                            event: "login-verify-reLogin",
                            args: vec![],
                            expect_ack: false,
                        });
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

        self.open_page.account_tools = open;
        if let Some(error) = pending_error
            && let Some(state) = self.active_bridge_state_mut()
        {
            state.last_error = Some(error);
        }
        if let Some(action) = pending_action {
            self.execute_account_tool_action(action);
        }
    }
}
