use crate::{app::test_support::app_with_bridges, config::IcaCfg};
use egui::{Context, Event, Key, Modifiers, RawInput};

#[test]
fn settings_shortcut_opens_without_an_active_bridge_but_leaves_ime_alone() {
    let (mut app, _) = app_with_bridges(IcaCfg::default(), Vec::new());
    let event = Event::Key {
        key: Key::Comma,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL,
    };
    for composing in [true, false] {
        // 每次使用独立键盘状态，避免 egui 将连续未松开的按键识别为长按。
        let ctx = Context::default();
        app.ime_composing = composing;
        let mut output = ctx.run_ui(
            RawInput {
                focused: true,
                events: vec![event.clone()],
                ..Default::default()
            },
            |ui| {
                app.handle_settings_shortcut(ui.ctx());
                let kept = ui.ctx().input(|input| input.events.contains(&event));
                assert_eq!(kept, composing);
                assert_eq!(app.open_page.settings, !composing);
                app.render_settings_center(ui.ctx());
            },
        );
        output.textures_delta.clear();
    }
}
