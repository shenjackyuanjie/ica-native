use super::safe_mention_text;

#[test]
fn mention_text_removes_layout_controls() {
    assert_eq!(
        safe_mention_text("Alice\n\u{202E}Bob\tCarol"),
        "Alice Bob Carol"
    );
}

#[test]
fn mention_text_limits_long_names() {
    let sanitized = safe_mention_text("a".repeat(80).as_str());

    assert_eq!(sanitized.chars().count(), 49);
    assert!(sanitized.ends_with('…'));
}

#[test]
fn mention_text_uses_fallback_for_blank_names() {
    assert_eq!(safe_mention_text("\n\t\u{202E}"), "未命名成员");
}
