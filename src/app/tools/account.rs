//! 账号 / 登录设备工具面板。
//!
//! 位于 `crate::app::tools` 之下，把“账号登录、登录设备管理、验证码/滑块提交”
//! 一类 bridge Socket API 的输入状态、校验与 egui 窗口收拢到一起。
//! 与其它工具面板一致：UI 线程先解析/校验输入，真正要下发的事件在窗口闭包结束、
//! 借用释放后再统一 flush，避免在渲染期间改动应用状态。

use crate::app::IcaApp;
use serde_json::{Value as JsonValue, json};

/// 账号 / 登录设备面板的输入状态。
///
/// 字段都是用户在窗口里直接编辑的原始文本；解析成 `i64` 等类型发生在点击按钮时，
/// 失败只写入错误提示，不回写这些原始字符串。
#[derive(Debug, Clone)]
pub struct AccountToolsState {
    /// 账号 QQ（登录 / 随机设备用）。
    pub username: String,
    /// 登录密码（仅远端登录用，UI 中以密码框遮挡）。
    pub password: String,
    /// 登录平台标识，默认 `"5"`，对应常用手机平台。
    pub platform: String,
    /// 短信验证码，用于 `submitSmsCode`。
    pub sms_code: String,
    /// 滑块验证 ticket，用于 `login-slider-ticket`。
    pub slider_ticket: String,
    /// 待删除登录设备的验证标记（flag），对应 `deleteLoginDevice`。
    pub delete_device_flag: String,
}

/// 默认把平台预填为 `"5"`，其余输入留空，减少每次登录的重复填写。
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

/// 账号面板要延迟执行的动作。
///
/// 窗口闭包运行在渲染 borrow 期间，只能“记录意图”；这里用枚举承载待下发动作，
/// 闭包结束后由 `execute_account_tool_action` 统一执行。
enum AccountToolAction {
    /// 通过 Socket API 通道向 bridge 发送一个事件。
    Call {
        /// bridge 端事件名，如 `getLoginDevices`、`login`。
        event: &'static str,
        /// 事件的 JSON 参数数组。
        args: Vec<JsonValue>,
        /// 是否期待 bridge 回报 ack（影响上层如何展示响应）。
        expect_ack: bool,
    },
}

/// `IcaApp` 的账号/登录设备工具固有方法：输入解析、动作执行与窗口渲染。
impl IcaApp {
    /// 把输入框文本去除首尾空白后解析为 `i64`。
    ///
    /// - 参数 `value`：原始输入文本；
    /// - 参数 `label`：出错时用于提示的字段中文名；
    /// - 返回：解析成功得到数值，失败返回 `<label> 不是有效数字` 供 UI 展示。
    fn parse_account_i64(value: &str, label: &str) -> Result<i64, String> {
        value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} 不是有效数字", label))
    }

    /// 执行一个延迟的账号面板动作：目前仅 `Call` 一种，转交 Socket API 通道下发。
    fn execute_account_tool_action(&mut self, action: AccountToolAction) {
        match action {
            AccountToolAction::Call {
                event,
                args,
                expect_ack,
            } => self.send_socket_api_event(event, args, expect_ack),
        }
    }

    /// 绘制“账号/登录设备”窗口。
    ///
    /// 窗口内的按钮只做输入校验并把意图记入 `pending_action` / `pending_error`；
    /// 待 egui 闭包结束、对 `self` 的借用释放后，再把错误写入当前 bridge 状态、
    /// 并真正下发动作，避免在渲染期间产生借用冲突或状态读写竞争。
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
                    ui.label("验证标记");
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

        // 闭包已结束，这里才把渲染期间收集到的错误与动作统一落地。
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
