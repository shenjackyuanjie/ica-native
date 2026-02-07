use serde_json::Value as JsonValue;

/// EventFromServer 是 GUI 侧接收的原始服务器事件类型（以 JSON 表示）
/// 在 GUI 中你可以根据 `event` 字段解析 `payload` 的具体结构
pub type EventFromServer = JsonValue;
