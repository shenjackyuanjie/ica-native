mod image_preview;
mod notices;
mod scroll;

use super::{format_message_content, replace_text_char_range};
use std::borrow::Cow;

#[test]
fn plain_message_uses_borrowed_fast_path() {
    let content = "普通消息 [Face: 1]";
    let formatted = format_message_content(content);

    assert!(matches!(formatted, Cow::Borrowed(_)));
    assert_eq!(formatted, content);
}

#[test]
fn at_markup_is_decoded() {
    let content = "你好 <IcalinguaAt qq=10001>Alice%20A</IcalinguaAt>";

    assert_eq!(format_message_content(content), "你好 Alice A");
}

#[test]
fn new_at_markup_is_decoded_without_double_decoding() {
    let content = "你好 <IcaAt qq=10001>A&amp;B &lt;群&gt; &amp;amp;</IcaAt>";

    assert_eq!(format_message_content(content), "你好 A&B <群> &amp;");
}

#[test]
fn malformed_at_markup_is_preserved() {
    let content = "你好 <IcalinguaAt qq=10001>Alice";

    assert_eq!(format_message_content(content), content);
}

#[test]
fn insertion_uses_character_cursor_for_unicode_text() {
    let mut content = "你ab好".to_string();

    let cursor = replace_text_char_range(&mut content, 1..1, "[Face: 1]");

    assert_eq!(content, "你[Face: 1]ab好");
    assert_eq!(cursor, 10);
}

#[test]
fn insertion_replaces_selected_characters() {
    let mut content = "你ab好".to_string();

    let cursor = replace_text_char_range(&mut content, 1..3, "表情");

    assert_eq!(content, "你表情好");
    assert_eq!(cursor, 3);
}
