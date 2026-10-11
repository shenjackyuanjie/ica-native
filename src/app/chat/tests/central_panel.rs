use super::{fit_chat_body_heights, should_show_date_separator, show_constrained_composer};

#[test]
fn date_separator_only_appears_when_the_date_changes() {
    assert!(!should_show_date_separator(None, "2025/07/09"));
    assert!(!should_show_date_separator(
        Some("2025/07/09"),
        "2025/07/09"
    ));
    assert!(should_show_date_separator(Some("2025/07/09"), "2025/07/10"));
}

#[test]
fn growing_composer_shrinks_message_list_without_exceeding_available_height() {
    let (message_height, composer_height) = fit_chat_body_heights(420.0, 186.0, 3.0);

    assert_eq!(message_height, 231.0);
    assert_eq!(composer_height, 186.0);
    assert_eq!(message_height + composer_height + 3.0, 420.0);
}

#[test]
fn composer_is_clamped_to_very_short_chat_body() {
    let (message_height, composer_height) = fit_chat_body_heights(120.0, 186.0, 3.0);

    assert_eq!(message_height, 0.0);
    assert_eq!(composer_height, 117.0);
}

#[test]
fn constrained_composer_keeps_trailing_control_inside_its_viewport() {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(320.0, 120.0),
        )),
        ..Default::default()
    };
    let mut viewport = egui::Rect::NOTHING;
    let mut control = egui::Rect::NOTHING;

    for _ in 0..2 {
        let mut frame_output = ctx.run_ui(input.clone(), |ui| {
            let output = show_constrained_composer(
                ui,
                egui::Id::new("constrained_composer_test"),
                120.0,
                |ui| {
                    ui.allocate_space(egui::vec2(200.0, 180.0));
                    ui.add_sized([200.0, 30.0], egui::Button::new("发送")).rect
                },
            );
            viewport = output.inner_rect;
            control = output.inner;
        });
        // 测试没有渲染后端，显式丢弃本帧生成的字体纹理更新。
        frame_output.textures_delta.clear();
    }

    assert_eq!(viewport.height(), 120.0);
    assert!(control.top() >= viewport.top());
    assert!(control.bottom() <= viewport.bottom());
}
