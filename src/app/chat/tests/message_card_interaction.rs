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

#[test]
fn hidden_image_viewer_opens_only_the_clicked_attachment_of_a_visible_message() {
    let (mut app, _rx) = test_app();
    app.custom_chat.hide_chat_img = true;
    app.custom_chat.hide_group_member_avatar = false;
    app.custom_chat.disable_img_swap_in_chat = false;
    let quoted_url = "https://example.invalid/quoted.png";
    let body_url = "https://example.invalid/body.png";
    let visible: Message = serde_json::from_value(serde_json::json!({
        "_id": "visible", "senderId": 456, "username": "Alice", "content": "正文",
        "files": [{"type": "image/png", "url": body_url}],
        "replyMessage": {
            "_id": "quoted", "username": "Bob", "content": "引用",
            "file": {"type": "image/png", "url": quoted_url}
        }
    }))
    .unwrap();
    let mut quoted = message("缓存中的原消息");
    quoted.msg_id = "quoted".into();
    quoted.files = vec![MessageFile {
        file_type: "image/png".into(),
        url: quoted_url.into(),
        size: None,
        name: None,
        fid: None,
    }];
    app.bridge_states[0].conversation_mut(-123).messages = vec![quoted, visible.clone()];

    let click_image = |app: &IcaApp, index: usize| {
        let ctx = egui::Context::default();
        frame(&ctx, app, &visible, false, 0.0, Vec::new());
        let layout = frame(&ctx, app, &visible, false, 0.5, Vec::new());
        let pos = layout
            .texts
            .iter()
            .filter(|(text, _)| text == "查看此图")
            .nth(index)
            .expect("正文和引用应分别显示查看入口")
            .1
            .center();
        let pointer = |pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        frame(&ctx, app, &visible, false, 1.0, pointer(true));
        let Some(MessageAction::Image(action)) =
            frame(&ctx, app, &visible, false, 1.1, pointer(false)).action
        else {
            panic!("点击应查看图片而不是定位引用");
        };
        (ctx, action)
    };

    for (index, url) in [quoted_url, body_url].into_iter().enumerate() {
        let (ctx, action) = click_image(&app, index);
        app.handle_image_action(&ctx, 0, action);
        let viewer = app.image_viewer.take().unwrap();
        let viewer = viewer.lock().unwrap();
        assert_eq!(viewer.sources.len(), 1, "不能暴露同会话其他隐藏图片");
        assert_eq!(viewer.current_source().url, url);
    }

    app.bridge_states[0].conversation_mut(-123).messages[0].deleted = true;
    let (ctx, action) = click_image(&app, 0);
    app.handle_image_action(&ctx, 0, action);
    assert!(app.image_viewer.is_none(), "不能通过引用查看已撤回原图");

    let mut hidden = visible;
    hidden.hide = true;
    let frame = frame(
        &egui::Context::default(),
        &app,
        &hidden,
        false,
        0.0,
        Vec::new(),
    );
    assert!(frame.texts.iter().all(|(text, _)| text != "查看此图"));
}
