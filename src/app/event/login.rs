//! 远端登录流程（二维码、短信、滑块）相关的事件。
//!
//! Bridge 需要人工介入登录时通过这些事件通知客户端。本模块只把状态汇总成一条
//! 面向用户的提示写入 `last_error`，实际的二维码/验证码输入在 bridge 端或“账号/
//! 登录设备”窗口完成，客户端本身不直接处理登录交互。

use super::payload;
use crate::app::state::BridgeState;
use serde_json::Value as JsonValue;

/// 处理本模块负责的事件；返回 false 表示事件不属于这里，交给下一个模块。
pub fn apply(state: &mut BridgeState, event_name: &str, payload: &JsonValue) -> bool {
    match event_name {
        "login-verify" => {
            state.last_error =
                Some("bridge 请求网页登录验证；可在“账号/登录设备”窗口重试或完成验证".to_string());
        }
        "login-qrcodeLogin" => {
            state.last_error = Some(
                "bridge 请求扫码登录；请查看 bridge 日志/二维码输出后在“账号/登录设备”继续"
                    .to_string(),
            );
        }
        "login-smsCodeVerify" => {
            state.last_error =
                Some("bridge 请求短信验证码；可在“账号/登录设备”填写验证码".to_string());
        }
        "login-error" => {
            state.last_error = payload::first_payload_display_message(payload)
                .or_else(|| Some("bridge 登录失败".to_string()));
        }
        "login-slider" => {
            state.last_error =
                Some("bridge 请求滑块验证；可在“账号/登录设备”填写滑块 ticket".to_string());
        }
        _ => return false,
    }
    true
}
