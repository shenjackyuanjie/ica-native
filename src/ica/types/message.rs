use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    All,
    Bool(bool),
    /// dummy
    None,
}

impl Serialize for At {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            At::All => serializer.serialize_str("all"),
            At::Bool(b) => serializer.serialize_bool(*b),
            At::None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for At {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AtVisitor;

        impl<'de> Visitor<'de> for AtVisitor {
            type Value = At;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a boolean or string")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(At::Bool(value))
            }

            fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(At::All)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(At::None)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(At::None)
            }
        }

        deserializer.deserialize_any(AtVisitor)
    }
}

/*export default interface LastMessage {
    content?: string
    timestamp?: string
    username?: string
    userId?: number
}
 */
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LastMessage {
    pub content: Option<String>,
    pub timestamp: Option<String>,
    pub username: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplyMessage {
    #[serde(rename = "_id")]
    pub msg_id: String,
    pub content: String,
    pub files: JsonValue,
    #[serde(rename = "username")]
    pub sender_name: String,
}
