//! 仅导出 GUI 已加载的聊天上下文，不连接 Bridge，也不暴露写入能力。
use crate::ica::types::message::Message;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub mod server;
pub const MAX_MESSAGES: usize = 200;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const REQUEST_TIMEOUT_SECONDS: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub bridge: String,
    pub room_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentContextConfig {
    pub enabled: bool,
    pub port: u16,
    pub auth_token: String,
    pub allowlist: Vec<Target>,
}
impl Default for AgentContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 10021,
            auth_token: String::new(),
            allowlist: Vec::new(),
        }
    }
}
impl AgentContextConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.port != 0, "Agent 上下文端口不能为 0");
        if self.enabled {
            anyhow::ensure!(
                !self.auth_token.is_empty()
                    && self.auth_token.bytes().all(|b| (33..=126).contains(&b)),
                "启用 Agent 上下文时必须配置不含空白的 ASCII Token"
            );
        }
        anyhow::ensure!(
            self.allowlist
                .iter()
                .all(|t| !t.bridge.trim().is_empty() && t.room_id != 0),
            "Agent 白名单必须指定 Bridge 和非零会话 ID"
        );
        Ok(())
    }
    pub fn allows(&self, target: &Target) -> bool {
        self.allowlist.contains(target)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Context {
    #[serde(flatten)]
    pub target: Target,
    pub room_name: String,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    #[default]
    Recent,
    Selected,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub target: Option<Target>,
    #[serde(default)]
    pub mode: ReadMode,
    pub limit: Option<usize>,
    pub reason: Option<String>,
}
impl ReadRequest {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.bridge.trim().is_empty() || t.room_id == 0)
        {
            return Err(ApiError::invalid("目标必须同时指定 Bridge 和非零会话 ID"));
        }
        if self.mode == ReadMode::Selected && self.limit.is_some() {
            return Err(ApiError::invalid(
                "多选模式不接受 limit，请在界面调整选中集合",
            ));
        }
        if self.limit.is_some_and(|n| !(1..=MAX_MESSAGES).contains(&n)) {
            return Err(ApiError::invalid("近期消息条数必须在 1～200 之间"));
        }
        if self
            .reason
            .as_ref()
            .is_some_and(|s| s.chars().count() > 200 || s.chars().any(char::is_control))
        {
            return Err(ApiError::invalid(
                "用途说明最多 200 个字符且不能包含控制字符",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Serialize)]
pub struct Attachment {
    pub kind: String,
    pub name: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct ContextMessage {
    pub message_id: String,
    pub sender_id: i64,
    pub sender_name: String,
    pub time: DateTime<Utc>,
    pub content: String,
    pub deleted: bool,
    pub hidden: bool,
    pub flash: bool,
    pub system: bool,
    pub reply_to: Option<String>,
    pub attachments: Vec<Attachment>,
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub source: &'static str,
    pub context: Context,
    pub mode: ReadMode,
    pub captured_at: DateTime<Utc>,
    pub loaded_count: usize,
    pub returned_count: usize,
    pub messages: Vec<ContextMessage>,
}
/// 仅投影允许的字段，即使 GUI 已展开撤回内容也不恢复正文。
pub fn snapshot(
    context: Context,
    request: &ReadRequest,
    loaded: &[Message],
    selected_ids: &HashSet<&str>,
) -> Result<Snapshot, ApiError> {
    request.validate()?;
    let selected: Vec<_> = match request.mode {
        ReadMode::Recent => loaded
            .iter()
            .skip(loaded.len().saturating_sub(request.limit.unwrap_or(50)))
            .collect(),
        ReadMode::Selected => loaded
            .iter()
            .filter(|m| selected_ids.contains(m.msg_id.as_str()))
            .collect(),
    };
    if request.mode == ReadMode::Selected && selected.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "no_selection",
            "请先在目标会话多选消息",
        ));
    }
    if request.mode == ReadMode::Selected && selected.len() != selected_ids.len() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "selection_unavailable",
            "部分选中消息已不在已加载范围，请重新选择；不会交付不完整集合",
        ));
    }
    if selected.len() > MAX_MESSAGES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_selected",
            "多选超过 200 条，请缩小范围",
        ));
    }
    let mut messages = Vec::with_capacity(selected.len());
    let mut bytes = 0;
    for m in selected {
        let protected = m.deleted || m.hide || m.flash;
        if !protected && m.content.len() > MAX_RESPONSE_BYTES {
            return Err(ApiError::too_large());
        }
        let item = ContextMessage {
            message_id: m.msg_id.clone(),
            sender_id: m.sender_id,
            sender_name: m.sender_name.clone(),
            time: m.time,
            content: if protected {
                "[受保护消息：不提供正文]".into()
            } else {
                m.content.clone()
            },
            deleted: m.deleted,
            hidden: m.hide,
            flash: m.flash,
            system: m.system,
            // 回复副本可能残留已隐藏内容，只返回引用 ID。
            reply_to: if protected {
                None
            } else {
                m.reply.as_ref().map(|r| r.msg_id.clone())
            },
            attachments: if protected {
                Vec::new()
            } else {
                m.files
                    .iter()
                    .map(|f| Attachment {
                        kind: f.file_type.clone(),
                        name: f.name.clone(),
                    })
                    .collect()
            },
        };
        bytes += serde_json::to_vec(&item)
            .map_err(|_| ApiError::internal())?
            .len();
        if bytes > MAX_RESPONSE_BYTES {
            return Err(ApiError::too_large());
        }
        messages.push(item);
    }
    let result = Snapshot {
        source: "loaded_only",
        context,
        mode: request.mode,
        captured_at: Utc::now(),
        loaded_count: loaded.len(),
        returned_count: messages.len(),
        messages,
    };
    // 为 HTTP 封装和 request_id 留空间，批准前就发现超限。
    if serde_json::to_vec(&result)
        .map_err(|_| ApiError::internal())?
        .len()
        + 128
        > MAX_RESPONSE_BYTES
    {
        return Err(ApiError::too_large());
    }
    Ok(result)
}
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum QueryResult {
    Contexts { contexts: Vec<Context> },
    Snapshot(Box<Snapshot>),
}
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}
impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "上下文处理失败",
        )
    }
    pub fn too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "result_too_large",
            "结果超过 256 KiB，请减少近期条数或多选范围",
        )
    }
    pub fn cancelled() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "request_cancelled",
            "服务关闭、配置更新或请求已失效",
        )
    }
    pub fn target_not_open() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "target_not_open",
            "目标会话未打开或已经关闭；不会自动打开会话",
        )
    }
    pub fn denied() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "user_denied",
            "用户取消或拒绝了本次读取",
        )
    }
}
#[cfg(test)]
mod tests;
