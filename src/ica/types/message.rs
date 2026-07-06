use std::fmt;

use chrono::DateTime;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use serde_json::{Value as JsonValue, json};

use crate::ica::types::{MessageId, RoomId, UserId, files::MessageFile};

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

/*
export default interface Message {
    _id: string | number
    senderId?: number
    username: string
    content: string
    code?: string
    timestamp?: string
    date?: string
    role?: string
    file?: MessageFile
    files: MessageFile[]
    time?: number
    replyMessage?: Message
    at?: boolean | 'all'
    deleted?: boolean
    system?: boolean
    mirai?: MessageMirai
    reveal?: boolean
    flash?: boolean
    title?: string
    anonymousId?: number
    anonymousflag?: string
    hide?: boolean
    bubble_id?: number
    subid?: number
    head_img?: string
}*/
/// {"message": {"_id":"idddddd","anonymousId":null,"anonymousflag":null,"bubble_id":0,"content":"test","date":"2024/02/18","files":[],"role":"admin","senderId":123456,"subid":1,"time":1708267062000_i64,"timestamp":"22:37:42","title":"索引管理员","username":"shenjack"},"roomId":-123456}
#[derive(Debug, Clone)]
pub struct Message {
    // /// 房间 id
    // pub room_id: RoomId,
    /// 消息 id
    pub msg_id: MessageId,
    /// 发送者 id
    pub sender_id: UserId,
    /// 发送者名字
    pub sender_name: String,
    /// 消息内容
    pub content: String,
    /// xml / json 内容
    pub code: JsonValue,
    /// 消息时间
    pub time: DateTime<chrono::Utc>,
    /// 身份
    pub role: String,
    /// 文件
    pub files: Vec<MessageFile>,
    /// 回复的消息
    pub reply: Option<ReplyMessage>,
    /// At
    pub at: At,
    /// 是否已撤回
    pub deleted: bool,
    /// 是否是系统消息
    pub system: bool,
    /// mirai?
    pub mirai: JsonValue,
    /// reveal ?
    pub reveal: bool,
    /// flash
    pub flash: bool,
    /// "群主授予的头衔"
    pub title: String,
    /// anonymous id
    pub anonymous_id: Option<i64>,
    /// 是否已被隐藏
    pub hide: bool,
    /// 气泡 id
    pub bubble_id: i64,
    /// 子? id
    pub subid: i64,
    /// 头像 img?
    pub head_img: JsonValue,
    /// 原始消息 (准确来说是 json["message"])
    pub raw_msg: JsonValue,
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // 先反序列化为 serde_json::Value，然后从中二次解析成我们需要的类型
        let json = JsonValue::deserialize(deserializer)?;

        // 消息 id，必须存在
        let msg_id = match json.get("_id") {
            Some(JsonValue::String(value)) => value.clone(),
            Some(JsonValue::Number(value)) => value.to_string(),
            _ => return Err(de::Error::custom("missing or invalid _id")),
        };

        // 发送者 id (Optional)
        let sender_id = json.get("senderId").and_then(|v| v.as_i64()).unwrap_or(-1);

        // 发送者名字 必有
        let sender_name = json
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| de::Error::custom("missing or invalid username"))?
            .to_string();

        // 消息内容
        let content = json
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| de::Error::custom("missing or invalid content"))?
            .to_string();

        // xml / json 内容
        let code = json.get("code").cloned().unwrap_or(JsonValue::Null);

        // 消息时间 (优先使用 time 字段，没有则使用当前时间)
        let current = chrono::Utc::now();
        let time = json
            .get("time")
            .and_then(|v| v.as_i64())
            .map(|t| DateTime::from_timestamp_micros(t).unwrap_or(current))
            .unwrap_or(current);

        // 身份
        let role = json
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // 文件列表
        let value_files = json
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(Vec::new);
        let mut files = Vec::with_capacity(value_files.len());
        for file in &value_files {
            if let Ok(file) = serde_json::from_value::<MessageFile>(file.clone()) {
                files.push(file);
            }
        }
        if let Some(file_value) = json.get("file")
            && !file_value.is_null()
            && let Ok(file) = serde_json::from_value::<MessageFile>(file_value.clone())
            && !files.iter().any(|existing| existing == &file)
        {
            files.push(file);
        }
        for file in &files {
            let file_type = file.file_type.to_ascii_lowercase();
            if (file_type == "image" || file_type.starts_with("image/"))
                && file.url.trim().is_empty()
            {
                tracing::warn!(
                    "image file missing url: msg_id={} sender={} raw_file={}",
                    msg_id,
                    sender_name,
                    serde_json::to_string(file)
                        .unwrap_or_else(|_| "<serialize failed>".to_string())
                );
            }
        }

        // 回复的消息
        let reply: Option<ReplyMessage> = match json.get("replyMessage") {
            Some(value) => {
                if !value.is_null() {
                    serde_json::from_value::<ReplyMessage>(value.clone()).ok()
                } else {
                    None
                }
            }
            None => None,
        };

        // At
        let at = serde_json::from_value::<At>(json.get("at").cloned().unwrap_or(JsonValue::Null))
            .unwrap_or(At::None);

        // 是否已撤回
        let deleted = json
            .get("deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // 是否是系统消息
        let system = json
            .get("system")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // mirai
        let mirai = json.get("mirai").cloned().unwrap_or(JsonValue::Null);
        // reveal
        let reveal = json
            .get("reveal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // flash
        let flash = json.get("flash").and_then(|v| v.as_bool()).unwrap_or(false);
        // "群主授予的头衔"
        let title = json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // anonymous id
        let anonymous_id = json.get("anonymousId").and_then(|v| v.as_i64());
        // 是否已被隐藏
        let hide = json.get("hide").and_then(|v| v.as_bool()).unwrap_or(false);
        // 气泡 id
        let bubble_id = json.get("bubble_id").and_then(|v| v.as_i64()).unwrap_or(1);
        // 子? id
        let subid = json.get("subid").and_then(|v| v.as_i64()).unwrap_or(1);
        // 头像 img?
        let head_img = json.get("head_img").cloned().unwrap_or(JsonValue::Null);
        // 原始消息 (有些场景 message 会出现在外层)
        let raw_msg = json.get("message").cloned().unwrap_or(json.clone());

        Ok(Self {
            msg_id,
            sender_id,
            sender_name,
            content,
            code,
            time,
            role,
            files,
            reply,
            at,
            deleted,
            system,
            mirai,
            reveal,
            flash,
            title,
            anonymous_id,
            hide,
            bubble_id,
            subid,
            head_img,
            raw_msg,
        })
    }
}

impl Message {
    pub fn output(&self) -> String {
        format!(
            // >10 >10 >15
            // >10 >15
            "{:>12}|{:<20}|{}",
            self.sender_id, self.sender_name, self.content
        )
    }

    /// 作为回复消息使用
    pub fn as_reply(&self) -> ReplyMessage {
        ReplyMessage {
            // 虽然其实只要这一条就行
            msg_id: self.msg_id.clone(),
            // 但是懒得动上面的了, 就这样吧
            content: self.content.clone(),
            files: json!([]),
            sender_name: self.sender_name.clone(),
        }
    }

    /// 获取回复
    pub fn get_reply(&self) -> Option<&ReplyMessage> {
        self.reply.as_ref()
    }

    pub fn get_reply_mut(&mut self) -> Option<&mut ReplyMessage> {
        self.reply.as_mut()
    }
}

/// 这才是 NewMessage
#[derive(Debug, Clone, Deserialize)]
pub struct NewMessage {
    #[serde(rename = "roomId")]
    pub room_id: RoomId,
    #[serde(rename = "message")]
    pub msg: Message,
}

impl NewMessage {
    pub fn new(room_id: RoomId, msg: Message) -> Self {
        Self { room_id, msg }
    }

    /// 创建一条对这条消息的回复
    pub fn reply_with(&self, content: &str) -> SendMessage {
        SendMessage::new(content.to_string(), self.room_id, Some(self.msg.as_reply()))
    }

    /// 作为被删除的消息
    pub fn as_deleted(&self) -> DeleteMessage {
        DeleteMessage {
            room_id: self.room_id,
            message_id: self.msg.msg_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachment {
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessage {
    /// 就是消息内容
    pub content: String,
    /// 发送的房间 id
    #[serde(rename = "roomId")]
    pub room_id: RoomId,
    /// 回复的消息
    #[serde(rename = "replyMessage")]
    pub reply_to: Option<ReplyMessage>,
    /// @ 谁
    #[serde(rename = "at")]
    pub at: JsonValue,
    /// base64 的图片
    #[serde(rename = "b64img")]
    file_data: Option<String>,
    /// 文件附件（分块上传后使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileAttachment>,
    /// 是否当作表情发送
    ///
    /// 默认 false
    pub sticker: bool,
}

impl SendMessage {
    pub fn new(content: String, room_id: RoomId, reply_to: Option<ReplyMessage>) -> Self {
        Self {
            content,
            room_id,
            reply_to,
            at: json!([]),
            file_data: None,
            file: None,
            sticker: false,
        }
    }

    pub fn as_value(&self) -> JsonValue {
        serde_json::to_value(self).unwrap()
    }

    pub fn has_b64img(&self) -> bool {
        self.file_data.is_some()
    }

    /// 设置消息的图片
    ///
    /// as_sticker: 是否当作表情发送
    /// file: 图片数据
    /// file_type: 图片类型(MIME) (image/png; image/jpeg)
    pub fn set_img(&mut self, file: &[u8], file_type: &str, as_sticker: bool) {
        self.sticker = as_sticker;
        use base64::{Engine as _, engine::general_purpose};
        let base64_data = general_purpose::STANDARD.encode(file);
        self.file_data = Some(format!("data:{file_type};base64,{base64_data}"));
    }
}

/// 被删除的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMessage {
    pub room_id: RoomId,
    pub message_id: MessageId,
}

impl DeleteMessage {
    pub fn new(room_id: RoomId, message_id: MessageId) -> Self {
        Self {
            room_id,
            message_id,
        }
    }

    pub fn as_value(&self) -> JsonValue {
        serde_json::to_value(self).unwrap()
    }
}
