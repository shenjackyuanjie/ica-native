use super::*;
use serde_json::json;

#[test]
fn sender_roles_use_group_friendly_labels() {
    assert_eq!(sender_role_label("owner").as_deref(), Some("群主"));
    assert_eq!(sender_role_label("admin").as_deref(), Some("管理员"));
    assert_eq!(
        sender_role_label("administrator").as_deref(),
        Some("管理员")
    );
    assert_eq!(sender_role_label("member"), None);
    assert_eq!(sender_role_label("unknown"), None);
    assert_eq!(sender_role_label(" bot ").as_deref(), Some("bot"));
}

#[test]
fn group_identity_is_hidden_for_private_and_telegram_messages() {
    assert!(should_show_group_identity(-123, &serde_json::Value::Null));
    assert!(!should_show_group_identity(123, &serde_json::Value::Null));
    assert!(!should_show_group_identity(
        -123,
        &json!({ "eqq": { "type": "tg" } })
    ));
}

#[test]
fn message_avatar_prefers_mirai_avatar_before_qq_fallback() {
    assert_eq!(
        message_avatar_url(123, &json!({ "eqq": { "avatarMd5": "aBcD" } })),
        Some("https://gchat.qpic.cn/gchatpic_new/0/0-0-ABCD/0".to_string())
    );
    assert_eq!(
        message_avatar_url(
            123,
            &json!({ "eqq": { "avatarUrl": "https://example.com/avatar.png" } })
        ),
        Some("https://example.com/avatar.png".to_string())
    );
    assert_eq!(
        message_avatar_url(123, &serde_json::Value::Null),
        Some("https://q1.qlogo.cn/g?b=qq&nk=123&s=140".to_string())
    );
    assert_eq!(message_avatar_url(-1, &serde_json::Value::Null), None);
}

#[test]
fn parses_links_without_swallowing_surrounding_punctuation() {
    assert_eq!(
        parse_content_segments("见 https://example.com/docs?q=1，或者稍后再看。"),
        vec![
            ContentSegment::Text("见 "),
            ContentSegment::Link {
                text: "https://example.com/docs?q=1",
                target: Cow::Borrowed("https://example.com/docs?q=1"),
            },
            ContentSegment::Text("，或者稍后再看。"),
        ]
    );
}

#[test]
fn normalizes_bare_domains_for_browser_opening() {
    assert_eq!(
        parse_content_segments("www.example.com/path"),
        vec![ContentSegment::Link {
            text: "www.example.com/path",
            target: Cow::Owned("https://www.example.com/path".to_owned()),
        }]
    );
}

#[test]
fn link_parsing_preserves_message_markers() {
    assert_eq!(
        parse_content_segments(
            "https://example.com <IcalinguaAt qq=42>%40Alice</IcalinguaAt>[Face: 0]"
        ),
        vec![
            ContentSegment::Link {
                text: "https://example.com",
                target: Cow::Borrowed("https://example.com"),
            },
            ContentSegment::Text(" "),
            ContentSegment::Mention {
                user_id: 42,
                text: Cow::Owned("@Alice".to_owned()),
            },
            ContentSegment::Face(0),
        ]
    );
}
