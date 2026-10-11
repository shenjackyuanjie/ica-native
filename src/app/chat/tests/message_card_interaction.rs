use super::*;
use crate::app::test_support::test_app;
use crate::ica::types::message::Message;

struct CardFrame {
    action: Option<MessageAction>,
    rect: egui::Rect,
    texts: Vec<(String, egui::Rect)>,
}

fn frame(
    ctx: &egui::Context,
    app: &IcaApp,
    message: &Message,
    forward: bool,
    time: f64,
    events: Vec<egui::Event>,
) -> CardFrame {
    let mut result = CardFrame {
        action: None,
        rect: egui::Rect::NOTHING,
        texts: Vec::new(),
    };
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            let rendered = ui.scope(|ui| {
                app.render_message_card(
                    ui,
                    -123,
                    999,
                    message,
                    MessageRenderOptions {
                        show_sender_name: true,
                        show_separator_before: false,
                        forward_mode_active: forward,
                        forward_selected: false,
                    },
                )
            });
            result.action = rendered.inner;
            result.rect = rendered.response.rect;
        },
    );
    for shape in &output.shapes {
        if let egui::Shape::Text(text) = &shape.shape {
            result.texts.push((
                text.galley.job.text.clone(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            ));
        }
    }
    output.textures_delta.clear();
    result
}

fn double_click(
    app: &IcaApp,
    message: &Message,
    forward: bool,
    target: &str,
) -> Option<MessageAction> {
    let ctx = egui::Context::default();
    frame(&ctx, app, message, forward, 0.0, Vec::new());
    let layout = frame(&ctx, app, message, forward, 0.5, Vec::new());
    let body = layout
        .texts
        .iter()
        .find(|(text, _)| text == &message.content)
        .map(|(_, rect)| *rect)
        .unwrap_or(layout.rect);
    let pos = match target {
        "空白" => egui::pos2(layout.rect.right() - 6.0, body.center().y),
        "气泡" => egui::pos2(body.left() - 3.0, body.center().y),
        "头像" => egui::pos2(layout.rect.left() + 16.0, layout.rect.bottom() - 20.0),
        "文字" => body.center(),
        _ => panic!("未知测试区域"),
    };
    let mut action = None;
    for (index, pressed) in [true, false, true, false].into_iter().enumerate() {
        action = frame(
            &ctx,
            app,
            message,
            forward,
            1.0 + index as f64 * 0.05,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
        .action;
    }
    action
}

fn message(content: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "_id": "interaction-message", "senderId": 456, "username": "Alice", "content": content,
    }))
    .unwrap()
}

#[test]
fn message_background_double_click_replies_but_text_and_links_keep_their_interaction() {
    let (app, _rx) = test_app();
    let plain = message("selectable body text");
    for target in ["空白", "气泡"] {
        assert!(
            matches!(double_click(&app, &plain, false, target), Some(MessageAction::Reply { room_id: -123, reply }) if reply.msg_id == plain.msg_id),
            "{target}双击应回复"
        );
    }
    assert!(
        double_click(&app, &plain, false, "文字").is_none(),
        "文本双击必须保留选词"
    );
    assert!(
        double_click(&app, &message("https://example.com"), false, "文字").is_none(),
        "链接不能触发回复"
    );
}

#[test]
fn group_avatar_double_click_pokes_instead_of_replying() {
    let (mut app, _rx) = test_app();
    app.custom_chat.show_message_avatar = true;
    app.custom_chat.hide_group_member_avatar = false;
    assert!(matches!(
        double_click(&app, &message("avatar message"), false, "头像"),
        Some(MessageAction::Poke {
            room_id: -123,
            target_id: 456
        })
    ));
}

#[test]
fn forward_selection_hidden_deleted_and_flash_messages_never_double_click_reply() {
    let (app, _rx) = test_app();
    let plain = message("forward selection body");
    assert!(double_click(&app, &plain, true, "空白").is_none());
    for flag in ["hide", "deleted", "flash", "system"] {
        let mut protected = plain.clone();
        match flag {
            "hide" => protected.hide = true,
            "deleted" => protected.deleted = true,
            "flash" => protected.flash = true,
            "system" => protected.system = true,
            _ => unreachable!(),
        }
        assert!(
            double_click(&app, &protected, false, "空白").is_none(),
            "{flag} 不允许双击回复"
        );
    }
}
