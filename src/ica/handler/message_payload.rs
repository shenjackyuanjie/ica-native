use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value as JsonValue, json};

use crate::ica::types::message::{Mention, ReplyMessage};

fn push_text_and_face_elements(chain: &mut Vec<JsonValue>, content: &str) {
    let mut remaining = content;
    while let Some(start) = remaining.find("[Face: ") {
        if start > 0 {
            chain.push(json!({"type": "text", "data": {"text": &remaining[..start]}}));
        }
        let after = &remaining[start + 7..];
        let Some(end) = after.find(']') else {
            remaining = &remaining[start..];
            break;
        };
        let Ok(face_id) = after[..end].parse::<u16>() else {
            chain.push(json!({
                "type": "text",
                "data": {"text": &remaining[start..start + 8 + end]},
            }));
            remaining = &after[end + 1..];
            continue;
        };
        chain.push(json!({"type": "face", "data": {"id": face_id}}));
        remaining = &after[end + 1..];
    }
    if !remaining.is_empty() {
        chain.push(json!({"type": "text", "data": {"text": remaining}}));
    }
}

fn push_legacy_markup_elements(chain: &mut Vec<JsonValue>, content: &str) {
    const OPEN_TAG: &str = "<IcalinguaAt qq=";
    const CLOSE_TAG: &str = "</IcalinguaAt>";

    let mut remaining = content;
    while let Some(start) = remaining.find(OPEN_TAG) {
        push_text_and_face_elements(chain, &remaining[..start]);
        let tagged = &remaining[start..];
        let Some(tag_end) = tagged.find('>') else {
            break;
        };
        let Some(user_id) = tagged[OPEN_TAG.len()..tag_end].parse::<i64>().ok() else {
            break;
        };
        let body = &tagged[tag_end + 1..];
        let Some(close) = body.find(CLOSE_TAG) else {
            break;
        };
        let visible_text = urlencoding::decode(&body[..close])
            .map_or_else(|_| body[..close].to_string(), |text| text.into_owned());
        chain.push(json!({
            "type": "at",
            "data": {
                "qq": if user_id == 1 { json!("all") } else { json!(user_id) },
                "text": visible_text,
            },
        }));
        remaining = &body[close + CLOSE_TAG.len()..];
    }
    push_text_and_face_elements(chain, remaining);
}

fn push_content_with_mentions(chain: &mut Vec<JsonValue>, content: &str, mentions: &[Mention]) {
    let mut remaining = content;
    while !remaining.is_empty() {
        let next = mentions
            .iter()
            .filter(|mention| !mention.text.is_empty())
            .filter_map(|mention| remaining.find(&mention.text).map(|index| (index, mention)))
            .min_by_key(|(index, _)| *index);
        let Some((index, mention)) = next else {
            push_legacy_markup_elements(chain, remaining);
            break;
        };
        push_legacy_markup_elements(chain, &remaining[..index]);
        chain.push(json!({
            "type": "at",
            "data": {
                "qq": if mention.user_id == 1 { json!("all") } else { json!(mention.user_id) },
                "text": mention.text,
            },
        }));
        remaining = &remaining[index + mention.text.len()..];
    }
}

pub(super) fn build_multi_image_raw_payload(
    room_id: i64,
    content: &str,
    reply_to: Option<&ReplyMessage>,
    mentions: &[Mention],
    images: &[(String, Arc<[u8]>)],
) -> JsonValue {
    let mut chain = Vec::with_capacity(images.len() + mentions.len() + 2);
    if let Some(reply) = reply_to {
        chain.push(json!({
            "type": "reply",
            "data": {"id": reply.msg_id, "text": reply.content},
        }));
    }
    push_content_with_mentions(&mut chain, content, mentions);
    for (_, bytes) in images {
        chain.push(json!({
            "type": "image",
            "data": {
                "file": format!("base64://{}", general_purpose::STANDARD.encode(bytes)),
                "type": "image",
                "sub_type": 0,
            },
        }));
    }

    json!({
        "messageType": "raw",
        "roomId": room_id,
        "content": JsonValue::Array(chain).to_string(),
        "at": [],
        "sticker": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_image_payload_contains_one_raw_chain() {
        let payload = build_multi_image_raw_payload(
            -123,
            "文字[Face: 14]",
            None,
            &[],
            &[
                ("image/png".to_string(), Arc::from([1_u8, 2, 3])),
                ("image/jpeg".to_string(), Arc::from([4_u8, 5, 6])),
            ],
        );
        let chain: Vec<JsonValue> =
            serde_json::from_str(payload["content"].as_str().unwrap()).unwrap();
        assert_eq!(payload["messageType"], "raw");
        assert_eq!(chain.len(), 4);
        assert_eq!(chain[1]["type"], "face");
        assert_eq!(chain[2]["data"]["file"], "base64://AQID");
        assert_eq!(chain[3]["data"]["file"], "base64://BAUG");
    }

    #[test]
    fn multi_image_payload_preserves_mentions() {
        let payload = build_multi_image_raw_payload(
            -123,
            "你好 @测试用户 ",
            None,
            &[Mention {
                user_id: 456,
                text: "@测试用户".to_string(),
            }],
            &[("image/png".to_string(), Arc::from([1_u8]))],
        );
        let chain: Vec<JsonValue> =
            serde_json::from_str(payload["content"].as_str().unwrap()).unwrap();
        assert_eq!(chain[1]["type"], "at");
        assert_eq!(chain[1]["data"]["qq"], 456);
        assert_eq!(chain[3]["type"], "image");
    }
}
