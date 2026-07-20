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

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    match value {
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .ok_or_else(|| serde::de::Error::custom("integer is outside i64 range")),
        JsonValue::String(value) => value.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom(
            "expected integer or integer string",
        )),
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FriendContact {
    #[serde(alias = "user_id", deserialize_with = "deserialize_i64")]
    pub uin: i64,
    #[serde(
        default,
        alias = "nickname",
        deserialize_with = "deserialize_string_or_default"
    )]
    pub nick: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub remark: String,
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
    fn friend_contract_accepts_oicq_and_fallback_field_names() {
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

        assert_eq!(fallback.display_name(), "同事");
        assert_eq!(oicq.uin, 10002);
        assert_eq!(oicq.display_name(), "Bob");
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
