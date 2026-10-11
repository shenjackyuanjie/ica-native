use super::super::tests::support::{image, message};
use super::*;

#[test]
fn reedit_skips_unresendable_messages_and_restores_all_images_reply_and_mentions() {
    let mut own = message("own", 42, "你好 <IcaAt qq=7>@A&amp;B</IcaAt>");
    own.files = vec![
        image("https://example.invalid/1.png"),
        image("https://example.invalid/2.png"),
    ];
    own.reply = Some(message("reply", 7, "被引用内容").as_reply());
    let mut file = message("file", 42, "不能只恢复这段正文");
    file.files = vec![crate::ica::types::files::MessageFile {
        file_type: "application/zip".into(),
        ..image("https://example.invalid/a.zip")
    }];
    let mut flash = message("flash", 42, "闪照");
    flash.flash = true;
    let mut conversation = ConversationState {
        messages: vec![own, file, flash, message("other", 7, "别人的消息")],
        ..Default::default()
    };
    assert!(edit_last_own_message(&mut conversation, 42));
    assert_eq!(conversation.editing_message_id.as_deref(), Some("own"));
    assert_eq!(conversation.draft, "你好 @A&B");
    assert_eq!(conversation.mentions[0].user_id, 7);
    assert_eq!(conversation.mentions[0].text, "@A&B");
    assert_eq!(conversation.pending_remote_images.len(), 2);
    assert_eq!(
        conversation.pending_remote_images[1].url,
        "https://example.invalid/2.png"
    );
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "reply");
}

#[test]
fn up_never_overwrites_draft_attachments_or_reply() {
    let base = ConversationState {
        messages: vec![message("own", 42, "旧消息")],
        ..Default::default()
    };
    let mut cases = [base.clone(), base.clone(), base.clone(), base];
    cases[0].draft = " ".into();
    cases[1].pending_images.push(crate::app::PendingImage::new(
        "测试.png".into(),
        "image/png".into(),
        vec![1],
    ));
    cases[2].reply_to = Some(message("reply", 7, "引用").as_reply());
    cases[3]
        .pending_remote_images
        .push(image("https://example.invalid/image.png"));
    for conversation in &mut cases {
        assert!(!edit_last_own_message(conversation, 42));
        assert!(conversation.editing_message_id.is_none());
    }
    assert_eq!(cases[0].draft, " ");
    assert_eq!(cases[1].pending_images.len(), 1);
    assert!(cases[2].reply_to.is_some());
    assert_eq!(cases[3].pending_remote_images.len(), 1);
}

#[test]
fn reedit_preserves_legacy_and_malformed_at_markup() {
    let (text, mentions) = restore_edit_mentions(
        "<IcalinguaAt qq=7>%40A%20B</IcalinguaAt> <IcaAt qq=1>@全体成员</IcaAt> <IcaAt qq=bad>@无效</IcaAt> <IcaAt qq=2>未闭合",
    );
    assert_eq!(
        text,
        "@A B @全体成员 <IcaAt qq=bad>@无效</IcaAt> <IcaAt qq=2>未闭合"
    );
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].user_id, 7);
    assert_eq!(mentions[1].user_id, 1);
}
