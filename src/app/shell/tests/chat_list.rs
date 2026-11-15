use super::visible_row_range;

#[test]
fn visible_range_is_empty_when_stale_scroll_offset_exceeds_shortened_list() {
    assert_eq!(visible_row_range(2_500.0, 3_000.0, 54.0, 17), 17..17);
}

#[test]
fn visible_range_keeps_overscan_inside_list_bounds() {
    assert_eq!(visible_row_range(108.0, 324.0, 54.0, 20), 0..8);
}

#[test]
fn long_sender_is_elided_without_covering_preview_or_unread_badge() {
    let (app, _rx) = crate::app::test_support::test_app();
    let room = crate::ica::types::room::Room {
        room_id: -123,
        room_name: "A very long room name ".repeat(10),
        index: 0,
        unread_count: 328,
        priority: 1,
        utime: 0,
        users: serde_json::Value::Null,
        at: Default::default(),
        last_message: crate::ica::types::message::LastMessage {
            username: Some("LongSender昵称".repeat(30)),
            content: Some("Visible message preview ".repeat(20)),
            timestamp: Some("00:31".into()),
            user_id: None,
        },
    };
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let row = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 54.0));
        app.render_room(ui, row, &room, None);
    });
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                Some((shape.clip_rect, text))
            } else {
                None
            }
        })
        .collect();
    let (_, title) = texts
        .iter()
        .find(|(_, text)| text.galley.job.text == room.room_name)
        .unwrap();
    let (sender_clip, sender) = texts
        .iter()
        .find(|(_, text)| text.galley.job.text.starts_with("LongSender"))
        .unwrap();
    let (body_clip, body) = texts
        .iter()
        .find(|(_, text)| text.galley.job.text.starts_with("Visible message"))
        .unwrap();
    let (_, unread) = texts
        .iter()
        .find(|(_, text)| text.galley.job.text == "328")
        .unwrap();
    assert!(title.galley.elided);
    assert!(sender.galley.elided);
    assert!(body.galley.elided);
    assert!(body.pos.x > sender.pos.x + sender.galley.size().x);
    let badge_left = unread.pos.x - 5.0;
    assert!(sender_clip.right() <= badge_left - 8.0);
    assert!(body_clip.right() <= badge_left - 8.0);
    assert!(body.pos.x + body.galley.size().x <= body_clip.right() + 0.5);
    assert!(body.galley.size().x > 30.0, "长昵称之后仍应能看到正文");
    output.textures_delta.clear();
}
