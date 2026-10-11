//! 仅导出 GUI 已加载的聊天上下文，不连接 Bridge，也不暴露写入能力。

pub mod server;
#[cfg(test)]
mod tests;

use crate::ica::types::message::Message;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 单次快照最多返回的消息条数。
pub const MAX_MESSAGES: usize = 200;
/// 单次响应体的硬上限（256 KiB），序列化前与序列化后各校验一次。
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// 等待用户批准的单请求超时（秒）。
pub const REQUEST_TIMEOUT_SECONDS: u64 = 120;

/// 读取目标：一个 bridge 中的一个会话。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Bridge 标识。
    pub bridge: String,
    /// 会话 ID（私聊为对方 QQ，群聊为负群号）。
    pub room_id: i64,
}

/// Agent 上下文服务配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentContextConfig {
    /// 是否启用服务。
    pub enabled: bool,
    /// 监听端口（仅本机）。
    pub port: u16,
    /// 鉴权 Token；启用时必填，且不得与 Noticer Token 复用。
    pub auth_token: String,
    /// 已打开会话白名单。
    pub allowlist: Vec<Target>,
}
/// 默认关闭，监听 10021，空白名单。
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
    /// 校验配置：端口非零；启用时 Token 必须为非空可见 ASCII；
    /// 白名单条目必须指定 Bridge 与非零会话 ID。
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

    /// 判断目标是否在白名单内。
    pub fn allows(&self, target: &Target) -> bool {
        self.allowlist.contains(target)
    }
}

/// 快照对应的会话上下文（目标 + 房间名）。
#[derive(Debug, Clone, Serialize)]
pub struct Context {
    /// 目标 bridge 与会话。
    #[serde(flatten)]
    pub target: Target,
    /// 房间显示名。
    pub room_name: String,
}

/// 读取模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    /// 最近若干条已加载消息。
    #[default]
    Recent,
    /// 界面中多选的消息。
    Selected,
}

/// 读取请求体。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    /// 目标会话；缺省时由服务端按当前打开会话补全。
    pub target: Option<Target>,
    /// 读取模式。
    #[serde(default)]
    pub mode: ReadMode,
    /// 近期模式的消息条数（1～200）。
    pub limit: Option<usize>,
    /// 用途说明（≤ 200 字符，无控制字符），供批准界面展示。
    pub reason: Option<String>,
}
impl ReadRequest {
    /// 校验请求：目标完整、多选模式不接受 limit、条数与用途说明合法。
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
/// 消息附件的最小投影：仅类型与文件名，不含下载凭据。
#[derive(Debug, Serialize)]
pub struct Attachment {
    /// 附件 MIME/类型。
    pub kind: String,
    /// 文件名（可能缺失）。
    pub name: Option<String>,
}

/// 交付给调用方的单条消息投影。
#[derive(Debug, Serialize)]
pub struct ContextMessage {
    /// 消息 ID。
    pub message_id: String,
    /// 发送者 QQ。
    pub sender_id: i64,
    /// 发送者昵称。
    pub sender_name: String,
    /// 发送时间（UTC）。
    pub time: DateTime<Utc>,
    /// 正文；受保护消息为占位文本。
    pub content: String,
    /// 是否已撤回。
    pub deleted: bool,
    /// 是否对界面隐藏。
    pub hidden: bool,
    /// 是否闪照。
    pub flash: bool,
    /// 是否系统消息。
    pub system: bool,
    /// 被回复消息的 ID；受保护消息不返回引用详情。
    pub reply_to: Option<String>,
    /// 附件投影列表。
    pub attachments: Vec<Attachment>,
}

/// 一次读取的完整快照。
#[derive(Debug, Serialize)]
pub struct Snapshot {
    /// 数据来源标识（固定为 `loaded_only`）。
    pub source: &'static str,
    /// 目标会话上下文。
    pub context: Context,
    /// 使用的读取模式。
    pub mode: ReadMode,
    /// 快照生成时间（UTC）。
    pub captured_at: DateTime<Utc>,
    /// 会话中已加载的消息总数。
    pub loaded_count: usize,
    /// 实际返回的消息数。
    pub returned_count: usize,
    /// 返回的消息列表。
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
/// 查询结果：会话列表或某个会话的快照。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum QueryResult {
    /// 已打开且在白名单内的会话列表。
    Contexts { contexts: Vec<Context> },
    /// 单个会话的快照。
    Snapshot(Box<Snapshot>),
}

/// API 错误：HTTP 状态码 + 稳定错误码 + 可读信息。
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    /// HTTP 状态码（不参与序列化，仅用于响应）。
    #[serde(skip)]
    pub status: StatusCode,
    /// 稳定错误码。
    pub code: &'static str,
    /// 面向调用方的可读信息。
    pub message: String,
}
impl ApiError {
    /// 构造自定义错误。
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
