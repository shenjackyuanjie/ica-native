//! 联系人（好友与群）的协议类型。
//!
//! Bridge 各适配器下发的字段名与类型不完全一致（`uin` / `user_id`、数字或
//! 字符串的 ID），因此反序列化容忍这些差异，并在 `FriendContact` 上手动实现
//! `Deserialize` 做归一化。

#[cfg(test)]
#[path = "tests/contact.rs"]
mod tests;

use crate::ica::types::RoomId;
use serde::{Deserialize, Deserializer};
use serde_json::Value as JsonValue;

/// 把任意 JSON 值宽松地转成字符串；缺失、null 或复合类型时取空串。
fn deserialize_string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(String::new());
    };

    Ok(match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value,
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(_) | JsonValue::Object(_) => String::new(),
    })
}

/// 解析可能是数字或字符串的整数字段；超出 i64 范围或类型不符时报错。
fn parse_i64(value: JsonValue) -> Result<i64, String> {
    match value {
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .ok_or_else(|| "integer is outside i64 range".to_string()),
        JsonValue::String(value) => value.parse::<i64>().map_err(|error| error.to_string()),
        _ => Err("应为整数或整数字符串".to_string()),
    }
}

/// serde 适配：要求字段存在且可解析为 i64。
fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    parse_i64(JsonValue::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

/// serde 适配：字段缺失或为 null 时得到 None。
fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<JsonValue>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if value.is_null() {
        Ok(None)
    } else {
        parse_i64(value).map(Some).map_err(serde::de::Error::custom)
    }
}

/// 好友联系人的线上表示，兼容不同 Bridge 的字段名。
#[derive(Deserialize)]
struct FriendContactWire {
    /// QQ 号；部分适配器只下发 `user_id`。
    #[serde(default, deserialize_with = "deserialize_optional_i64")]
    uin: Option<i64>,
    /// 兼容字段：与 `uin` 等价，`uin` 缺失时使用。
    #[serde(default, deserialize_with = "deserialize_optional_i64")]
    user_id: Option<i64>,
    /// 昵称；为空时回退到 `nickname`。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    nick: String,
    /// 兼容字段：与 `nick` 等价。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    nickname: String,
    /// 备注名。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    remark: String,
}

/// 好友联系人（归一化后的形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendContact {
    /// 好友 QQ 号。
    pub uin: i64,
    /// 昵称（已做空值回退）。
    pub nick: String,
    /// 备注名。
    pub remark: String,
}

impl<'de> Deserialize<'de> for FriendContact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = FriendContactWire::deserialize(deserializer)?;
        let uin = wire
            .uin
            .or(wire.user_id)
            .ok_or_else(|| serde::de::Error::missing_field("uin 或 user_id"))?;
        let nick = if wire.nick.trim().is_empty() {
            wire.nickname
        } else {
            wire.nick
        };
        Ok(Self {
            uin,
            nick,
            remark: wire.remark,
        })
    }
}

impl FriendContact {
    /// 换算成会话 ID（私聊为正 QQ 号）。
    pub fn room_id(&self) -> RoomId {
        self.uin.abs()
    }

    /// 展示名：优先备注，再昵称，最后退化为 QQ 号。
    pub fn display_name(&self) -> String {
        [self.remark.trim(), self.nick.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.uin.to_string())
    }

    /// 是否匹配搜索词（空词恒真；大小写不敏感地匹配 QQ 号、昵称与备注）。
    pub fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.uin.to_string().contains(query)
            || self.nick.to_uppercase().contains(query)
            || self.remark.to_uppercase().contains(query)
    }

    /// 好友头像 URL。
    pub fn avatar_url(&self) -> String {
        format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", self.uin.abs())
    }
}

/// 群联系人。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GroupContact {
    /// 群号。
    #[serde(deserialize_with = "deserialize_i64")]
    pub group_id: i64,
    /// 群名称。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub group_name: String,
    /// 群备注。
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub group_remark: String,
}

impl GroupContact {
    /// 换算成会话 ID（群聊为负群号）。
    pub fn room_id(&self) -> RoomId {
        -self.group_id.abs()
    }

    /// 展示名：优先群备注，再群名，最后退化为群号。
    pub fn display_name(&self) -> String {
        [self.group_remark.trim(), self.group_name.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.group_id.to_string())
    }

    /// 房间名：群名为空时回退到展示名。
    pub fn room_name(&self) -> String {
        if self.group_name.trim().is_empty() {
            self.display_name()
        } else {
            self.group_name.trim().to_string()
        }
    }

    /// 是否匹配搜索词（空词恒真；大小写不敏感地匹配群号、群名与群备注）。
    pub fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.group_id.to_string().contains(query)
            || self.group_name.to_uppercase().contains(query)
            || self.group_remark.to_uppercase().contains(query)
    }

    /// 群头像 URL。
    pub fn avatar_url(&self) -> String {
        let group_id = self.group_id.abs();
        format!("https://p.qlogo.cn/gh/{group_id}/{group_id}/0")
    }
}
