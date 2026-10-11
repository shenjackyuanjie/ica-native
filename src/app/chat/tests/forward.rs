use super::{fake_forward_node, forward_image_sources, forward_reference};
use crate::ica::types::{
    files::MessageFile,
    message::{At, Message},
};
use chrono::Utc;
use serde_json::{Value as JsonValue, json};

fn message(content: &str, code: JsonValue) -> Message {
    Message {
        msg_id: "m1".to_string(),
        sender_id: 10001,
        sender_name: "Alice".to_string(),
        content: content.to_string(),
        code,
        time: Utc::now(),
        time_text: "12:00:00".to_string(),
        date_text: "2026/08/05".to_string(),
        role: String::new(),
        files: Vec::new(),
        reply: None,
        at: At::None,
        deleted: false,
        system: false,
        mirai: JsonValue::Null,
        reveal: false,
        flash: false,
        title: String::new(),
        anonymous_id: None,
        hide: false,
        bubble_id: 1,
        subid: 1,
        head_img: JsonValue::Null,
        raw_msg: None,
    }
}

#[test]
fn parses_remote_and_nested_forward_references() {
    let remote = message("摘要\n[Forward: RES123]", JsonValue::Null);
    let nested = message("[NestedForward: MultiMsg]", JsonValue::Null);

    assert_eq!(forward_reference(&remote, None).unwrap().res_id, "RES123");
    let nested = forward_reference(&nested, Some("PARENT")).unwrap();
    assert_eq!(nested.res_id, "PARENT");
    assert_eq!(nested.file_name.as_deref(), Some("MultiMsg"));
}

#[test]
fn prefers_resource_metadata_and_keeps_nested_fallback() {
    let remote = message(
        "[Forward: display-value]",
        json!({"meta": {"detail": {"resid": "CODE-RES"}}}),
    );
    assert_eq!(forward_reference(&remote, None).unwrap().res_id, "CODE-RES");

    let nested = message(
        "[NestedForward: marker-name]",
        json!({"meta": {"detail": {"resid": "INNER-RES", "uniseq": "CODE-NAME"}}}),
    );
    let reference = forward_reference(&nested, Some("PARENT-RES")).unwrap();
    assert_eq!(reference.res_id, "PARENT-RES");
    assert_eq!(reference.fallback_res_id.as_deref(), Some("INNER-RES"));
    assert_eq!(reference.file_name.as_deref(), Some("CODE-NAME"));
}

#[test]
fn parses_xml_forward_resource_metadata() {
    let message = message(
        "[Forward: display-value]",
        JsonValue::String("<msg m_resid=\"XML-RES\" m_fileName=\"MultiMsg\" />".into()),
    );
    let reference = forward_reference(&message, None).unwrap();
    assert_eq!(reference.res_id, "XML-RES");
}

#[test]
fn fake_node_prefers_original_message_chain() {
    let mut source = message("fallback", JsonValue::Null);
    source.raw_msg = Some(Box::new(json!([
        {"type": "text", "data": {"text": "raw"}},
        {"type": "face", "data": {"id": 14}}
    ])));

    let node = fake_forward_node(&source);
    assert_eq!(node["message"][0]["data"]["text"], "raw");
    assert_eq!(node["message"][1]["type"], "face");
    assert_eq!(node["consistent"], true);
}

#[test]
fn image_gallery_uses_only_forwarded_image_attachments_in_order() {
    let mut first = message("first", JsonValue::Null);
    first.files = vec![
        MessageFile {
            file_type: "image/png".to_string(),
            url: "https://example.test/one.png".to_string(),
            size: None,
            name: None,
            fid: None,
        },
        MessageFile {
            file_type: "file".to_string(),
            url: "https://example.test/archive.zip".to_string(),
            size: None,
            name: None,
            fid: None,
        },
    ];
    let mut second = message("second", JsonValue::Null);
    second.files = vec![
        MessageFile {
            file_type: "image/gif".to_string(),
            url: "https://example.test/two.gif".to_string(),
            size: None,
            name: None,
            fid: None,
        },
        MessageFile {
            file_type: "image/jpeg".to_string(),
            url: String::new(),
            size: None,
            name: None,
            fid: None,
        },
    ];

    let sources = forward_image_sources(&[first, second]);
    assert_eq!(
        sources
            .into_iter()
            .map(|source| source.url)
            .collect::<Vec<_>>(),
        [
            "https://example.test/one.png".to_string(),
            "https://example.test/two.gif".to_string(),
        ]
    );
}
