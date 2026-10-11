use super::*;

#[test]
fn escape_cancels_text_reply_and_edit_before_removing_attachments() {
    let reply: ReplyMessage = serde_json::from_value(serde_json::json!({
        "_id": "引用", "content": "被引用消息", "username": "测试成员"
    }))
    .unwrap();
    let mut conversation = ConversationState {
        draft: "草稿".into(),
        reply_to: Some(reply),
        editing_message_id: Some("旧消息".into()),
        highlight_message_id: Some("引用".into()),
        scroll_to_message_id: Some("引用".into()),
        pending_file: Some(PendingFile::new(
            "测试.txt".into(),
            "text/plain".into(),
            vec![1],
        )),
        pending_images: vec![PendingImage::new(
            "测试.png".into(),
            "image/png".into(),
            vec![2],
        )],
        pending_remote_images: vec![MessageFile {
            file_type: "image/png".into(),
            url: "https://example.invalid/image.png".into(),
            size: None,
            name: None,
            fid: None,
        }],
        ..Default::default()
    };
    assert!(conversation.cancel_composer_layer());
    assert!(conversation.draft.is_empty());
    assert!(conversation.reply_to.is_none());
    assert!(conversation.editing_message_id.is_none());
    assert!(conversation.highlight_message_id.is_none());
    assert!(conversation.scroll_to_message_id.is_none());
    assert!(conversation.pending_file.is_some());
    assert_eq!(conversation.pending_images.len(), 1);
    assert_eq!(conversation.pending_remote_images.len(), 1);
    assert!(conversation.cancel_composer_layer());
    assert!(!conversation.has_composer_content());
    assert!(!conversation.cancel_composer_layer());
}
