use super::*;

#[test]
fn private_header_avatar_double_click_targets_the_peer() {
    let ctx = egui::Context::default();
    let mut action = None;
    let mut rect = egui::Rect::NOTHING;
    for step in 0..6 {
        let pos = rect.center();
        let events = if step < 2 {
            Vec::new()
        } else {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: step % 2 == 0,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(step as f64 * 0.05),
                events,
                ..Default::default()
            },
            |ui| {
                let rendered = ui.scope(|ui| render_private_header_avatar(ui, 456, "私聊对象"));
                rect = rendered.response.rect;
                action = rendered.inner;
            },
        );
        output.textures_delta.clear();
    }
    assert!(matches!(
        action,
        Some(MessageAction::Poke {
            room_id: 456,
            target_id: 456
        })
    ));
}
