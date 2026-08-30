use serde::{Deserialize, Deserializer};
use serde_json::Value as JsonValue;

use crate::ica::types::RoomId;

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

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    parse_i64(JsonValue::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

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

#[derive(Deserialize)]
struct FriendContactWire {
    #[serde(default, deserialize_with = "deserialize_optional_i64")]
    uin: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_i64")]
    user_id: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    nick: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    nickname: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    remark: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendContact {
    pub uin: i64,
    pub nick: String,
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
    pub fn room_id(&self) -> RoomId {
        self.uin.abs()
    }

    pub fn display_name(&self) -> String {
        [self.remark.trim(), self.nick.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.uin.to_string())
    }

    pub fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.uin.to_string().contains(query)
            || self.nick.to_uppercase().contains(query)
            || self.remark.to_uppercase().contains(query)
    }

    pub fn avatar_url(&self) -> String {
        format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", self.uin.abs())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GroupContact {
    #[serde(deserialize_with = "deserialize_i64")]
    pub group_id: i64,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub group_name: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub group_remark: String,
}

impl GroupContact {
    pub fn room_id(&self) -> RoomId {
        -self.group_id.abs()
    }

    pub fn display_name(&self) -> String {
        [self.group_remark.trim(), self.group_name.trim()]
            .into_iter()
            .find(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.group_id.to_string())
    }

    pub fn room_name(&self) -> String {
        if self.group_name.trim().is_empty() {
            self.display_name()
        } else {
            self.group_name.trim().to_string()
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.group_id.to_string().contains(query)
            || self.group_name.to_uppercase().contains(query)
            || self.group_remark.to_uppercase().contains(query)
    }

    pub fn avatar_url(&self) -> String {
        let group_id = self.group_id.abs();
        format!("https://p.qlogo.cn/gh/{group_id}/{group_id}/0")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FriendContact, GroupContact};

    #[test]
    fn friend_contract_accepts_oicq_fallback_and_duplicate_field_names() {
        let fallback: FriendContact = serde_json::from_value(json!({
            "uin": 10001,
            "nick": "Alice",
            "remark": "同事"
        }))
        .unwrap();
        let oicq: FriendContact = serde_json::from_value(json!({
            "user_id": "10002",
            "nickname": "Bob",
            "remark": null
        }))
        .unwrap();
        let bridge: FriendContact = serde_json::from_value(json!({
            "uin": 10003,
            "user_id": "99999",
            "nick": "Carol",
            "nickname": "旧昵称",
            "remark": ""
        }))
        .unwrap();

        assert_eq!(fallback.display_name(), "同事");
        assert_eq!(oicq.uin, 10002);
        assert_eq!(oicq.display_name(), "Bob");
        assert_eq!(bridge.uin, 10003);
        assert_eq!(bridge.display_name(), "Carol");
        assert!(fallback.matches_query("ALICE"));
    }

    #[test]
    fn group_contract_uses_negative_room_ids_and_remark_for_display() {
        let group: GroupContact = serde_json::from_value(json!({
            "group_id": "123456",
            "group_name": "开发群",
            "group_remark": "工作"
        }))
        .unwrap();

        assert_eq!(group.room_id(), -123456);
        assert_eq!(group.display_name(), "工作");
        assert_eq!(group.room_name(), "开发群");
        assert!(group.matches_query("1234"));
    }
}
