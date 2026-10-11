//! 输入框场景共用的内存消息与图片数据，不包含测试用例。

use crate::ica::types::message::Message;

pub fn message(id: &str, sender_id: i64, content: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "_id": id, "senderId": sender_id, "username": "测试成员", "content": content,
    }))
    .unwrap()
}

pub fn image(url: &str) -> crate::ica::types::files::MessageFile {
    crate::ica::types::files::MessageFile {
        file_type: "image/png".into(),
        url: url.into(),
        size: None,
        name: None,
        fid: None,
    }
}
