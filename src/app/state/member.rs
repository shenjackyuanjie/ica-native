//! 群成员模型与 Bridge 下发字段的容错反序列化。
//!
//! 不同协议端会把同一个字段下发成字符串或数字，也可能整个缺失，
//! 因此这里统一用宽松的反序列化辅助函数兜底，避免整批成员因单个字段而解析失败。

use serde::Deserialize;
use serde_json::Value as JsonValue;

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

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GroupMember {
    pub user_id: i64,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub nickname: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub card: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub remark: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub level: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub role: String,
    #[serde(default, deserialize_with = "deserialize_i64_or_default")]
    pub shutup_time: i64,
}

impl GroupMember {
    pub fn display_name(&self) -> &str {
        if self.card.trim().is_empty() {
            &self.nickname
        } else {
            &self.card
        }
    }

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

    pub fn is_muted_at(&self, timestamp: i64) -> bool {
        self.shutup_time > timestamp
    }

    pub fn remaining_mute_seconds_at(&self, timestamp: i64) -> u64 {
        u64::try_from(self.shutup_time.saturating_sub(timestamp)).unwrap_or(0)
    }

    pub fn role_rank(&self) -> u8 {
        match self.role.trim().to_ascii_lowercase().as_str() {
            "owner" => 2,
            "admin" | "administrator" => 1,
            _ => 0,
        }
    }

    pub fn role_label(&self) -> Option<&'static str> {
        match self.role_rank() {
            2 => Some("群主"),
            1 => Some("管理员"),
            _ => None,
        }
    }

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

#[cfg(test)]
mod group_member_tests {
    use serde_json::json;

    use super::GroupMember;

    fn member(user_id: i64, role: &str) -> GroupMember {
        serde_json::from_value(json!({
            "user_id": user_id,
            "nickname": user_id.to_string(),
            "role": role,
            "shutup_time": 100,
        }))
        .unwrap()
    }

    #[test]
    fn mute_boundary_and_moderation_permissions_match_group_roles() {
        let owner = member(1, "owner");
        let admin = member(2, "admin");
        let regular = member(3, "member");

        assert!(regular.is_muted_at(99));
        assert!(!regular.is_muted_at(100));
        assert_eq!(regular.remaining_mute_seconds_at(98), 2);
        assert!(GroupMember::moderation_denial_reason(Some(&owner), &admin, 1).is_none());
        assert!(GroupMember::moderation_denial_reason(Some(&admin), &regular, 2).is_none());
        assert!(GroupMember::moderation_denial_reason(Some(&admin), &owner, 2).is_some());
        assert!(GroupMember::moderation_denial_reason(Some(&regular), &admin, 3).is_some());
        assert!(GroupMember::moderation_denial_reason(Some(&owner), &owner, 1).is_some());
    }
}
