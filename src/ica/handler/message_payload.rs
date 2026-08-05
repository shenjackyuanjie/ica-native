use std::sync::Arc;

use crate::ica::types::message::{Mention, ReplyMessage, SendMessage};

pub(super) fn build_multi_image_message(
    room_id: i64,
    content: &str,
    reply_to: Option<&ReplyMessage>,
    mentions: &[Mention],
    images: &[(String, Arc<[u8]>)],
) -> SendMessage {
    let mut message = SendMessage::new(content.to_string(), room_id, reply_to.cloned());
    message.set_mentions(mentions);
    for (file_type, bytes) in images {
        message.add_img(bytes, file_type);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_image_payload_uses_media_array() {
        let payload = build_multi_image_message(
            -123,
            "文字[Face: 14]",
            None,
            &[],
            &[
                ("image/png".to_string(), Arc::from([1_u8, 2, 3])),
                ("image/jpeg".to_string(), Arc::from([4_u8, 5, 6])),
            ],
        );
        let value = payload.as_value();
        assert_eq!(value["content"], "文字[Face: 14]");
        assert_eq!(value["media"].as_array().map(Vec::len), Some(2));
        assert_eq!(value["media"][0]["b64"], "data:image/png;base64,AQID");
        assert_eq!(value["media"][0]["type"], "image/png");
        assert_eq!(value["media"][1]["b64"], "data:image/jpeg;base64,BAUG");
    }

    #[test]
    fn multi_image_payload_preserves_mentions() {
        let payload = build_multi_image_message(
            -123,
            "你好 @测试用户 ",
            None,
            &[Mention {
                user_id: 456,
                text: "@测试用户".to_string(),
            }],
            &[("image/png".to_string(), Arc::from([1_u8]))],
        );
        let value = payload.as_value();
        assert_eq!(value["at"][0]["id"], 456);
        assert_eq!(value["at"][0]["text"], "@测试用户");
        assert_eq!(value["media"].as_array().map(Vec::len), Some(1));
    }
}
