use super::super::tests::support::message;
use super::*;

#[test]
fn reply_navigation_skips_system_and_flash_and_clears_after_latest() {
    let first = message("first", 2, "第一条");
    let mut system = message("system", 2, "系统提示");
    system.system = true;
    let mut flash = message("flash", 2, "闪照");
    flash.flash = true;
    let latest = message("latest", 2, "最新消息");
    let mut conversation = ConversationState {
        messages: vec![first, system, flash, latest],
        ..Default::default()
    };
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert!(conversation.reply_to.is_none());
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "latest");
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "first");
    assert_eq!(conversation.scroll_to_message_id.as_deref(), Some("first"));
    assert_eq!(conversation.highlight_message_id.as_deref(), Some("first"));
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "first");
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert!(conversation.reply_to.is_none());
    assert!(conversation.highlight_message_id.is_none());
    assert!(conversation.scroll_to_message_id.is_none());
}

#[test]
fn reply_outside_loaded_history_is_located_without_switching_to_another_message() {
    let mut conversation = ConversationState {
        messages: vec![message("latest", 2, "最新消息")],
        reply_to: Some(message("older", 2, "旧消息").as_reply()),
        ..Default::default()
    };
    move_reply_selection(&mut conversation, ReplyDirection::Next);
    assert_eq!(conversation.reply_to.as_ref().unwrap().msg_id, "older");
    assert!(conversation.scroll_to_message_id.is_none());
    move_reply_selection(&mut conversation, ReplyDirection::Previous);
    assert_eq!(conversation.scroll_to_message_id.as_deref(), Some("older"));
}
