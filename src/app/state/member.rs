//! 群成员模型与 Bridge 下发字段的容错反序列化。
//!
//! 不同协议端会把同一个字段下发成字符串或数字，也可能整个缺失，
//! 因此这里统一用宽松的反序列化辅助函数兜底，避免整批成员因单个字段而解析失败。

#[cfg(test)]
#[path = "tests/member.rs"]
mod tests;

use serde::Deserialize;
use serde_json::Value as JsonValue;

/// 宽松读取字符串字段：缺失/null 得空串，数字与布尔转成其文本形式，
/// 数组/对象则序列化为 JSON 文本，保证单个脏字段不会让整批成员解析失败。
fn deserialize_string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(String::new());
    };

    Ok(match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value,
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            serde_json::to_string(&value).unwrap_or_else(|_| String::new())
        }
    })
}

/// 宽松读取整数字段：缺失/null 得 0，数字超出 i64 范围才报错，
/// 字符串在 trim 后解析，空串按 0 处理。
fn deserialize_i64_or_default<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(0);
    };
    match value {
        JsonValue::Null => Ok(0),
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .ok_or_else(|| serde::de::Error::custom("integer is outside i64 range")),
        JsonValue::String(value) if value.trim().is_empty() => Ok(0),
        JsonValue::String(value) => value.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("应为整数、整数字符串或 null")),
    }
}

/// 群成员条目；字符串字段统一走宽松反序列化，兼容各协议端的下发差异。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GroupMember {
    /// 成员 QQ 号。
    pub user_id: i64,
    /// 昵称。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub nickname: String,
    /// 群名片。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub card: String,
    /// 备注。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub remark: String,
    /// 群头衔。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub title: String,
    /// 等级（活跃/等级头衔文本）。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub level: String,
    /// 角色：`owner` / `admin` 等，决定管理权限。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub role: String,
    /// 禁言截止时间戳（秒）；0 或早于当前时间表示未被禁言。
    #[serde(default, deserialize_with = "deserialize_i64_or_default")]
    pub shutup_time: i64,
}

impl GroupMember {
    /// 展示名：有群名片时用名片，否则用昵称。
    pub fn display_name(&self) -> &str {
        if self.card.trim().is_empty() {
            &self.nickname
        } else {
            &self.card
        }
    }

    /// 成员搜索匹配：空 query 匹配全部，否则按 QQ 号或任一文本字段包含匹配。
    pub fn matches_search(&self, query: &str) -> bool {
        query.is_empty()
            || self.user_id.to_string().contains(query)
            || [
                self.display_name(),
                self.nickname.as_str(),
                self.card.as_str(),
                self.remark.as_str(),
                self.title.as_str(),
                self.level.as_str(),
                self.role.as_str(),
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(query))
    }

    /// 给定时间戳（秒）时是否仍处于禁言中。
    pub fn is_muted_at(&self, timestamp: i64) -> bool {
        self.shutup_time > timestamp
    }

    /// 给定时间戳（秒）时剩余的禁言秒数；未禁言为 0。
    pub fn remaining_mute_seconds_at(&self, timestamp: i64) -> u64 {
        u64::try_from(self.shutup_time.saturating_sub(timestamp)).unwrap_or(0)
    }

    /// 权限等级：2 群主、1 管理员、0 普通成员，供排序与权限判定复用。
    pub fn role_rank(&self) -> u8 {
        match self.role.trim().to_ascii_lowercase().as_str() {
            "owner" => 2,
            "admin" | "administrator" => 1,
            _ => 0,
        }
    }

    /// 权限中文标签；普通成员返回 `None`（不展示角标）。
    pub fn role_label(&self) -> Option<&'static str> {
        match self.role_rank() {
            2 => Some("群主"),
            1 => Some("管理员"),
            _ => None,
        }
    }

    /// 判断一次群管理操作是否被拒绝，返回拒绝原因；允许时返回 `None`。
    ///
    /// 规则依次为：不能管理自己；拿不到当前账号的成员信息时无法判断权限；
    /// 普通成员只能查看；不能管理同级或更高权限的成员。
    pub fn moderation_denial_reason(
        actor: Option<&GroupMember>,
        target: &GroupMember,
        self_id: i64,
    ) -> Option<&'static str> {
        if target.user_id == self_id {
            return Some("不能管理自己");
        }
        let Some(actor) = actor else {
            return Some("成员列表中没有当前账号的权限信息");
        };
        if actor.role_rank() == 0 {
            return Some("普通成员只能查看群成员");
        }
        if target.role_rank() >= actor.role_rank() {
            return Some("不能管理同级或更高权限成员");
        }
        None
    }
}
