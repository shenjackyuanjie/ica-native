use super::range_slider;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect};

struct SliderFixture {
    ctx: egui::Context,
    min: f64,
    max: f64,
    rect: Rect,
    frame: u32,
}

impl SliderFixture {
    fn draw(&mut self, events: Vec<Event>) {
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 120.0))),
                time: Some(f64::from(self.frame) / 60.0),
                events,
                ..Default::default()
            },
            |ui| {
                self.rect = range_slider(ui, &mut self.min, &mut self.max).rect;
            },
        );
        output.textures_delta.clear();
        self.frame += 1;
    }

    fn point(&self, value: f64) -> Pos2 {
        let rail = self.rect.shrink2(egui::vec2(7.0, 0.0));
        egui::pos2(
            egui::lerp(rail.x_range(), ((value - 0.1) / 9.9) as f32),
            rail.center().y,
        )
    }

    fn drag(&mut self, from: f64, to: f64) {
        let start = self.point(from);
        let end = self.point(to);
        self.draw(vec![Event::PointerMoved(start)]);
        self.draw(vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.draw(vec![Event::PointerMoved(end)]);
        self.draw(vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }
}

#[test]
fn both_handles_adjust_independently_and_can_reopen_a_collapsed_range() {
    let mut slider = SliderFixture {
        ctx: egui::Context::default(),
        min: 2.0,
        max: 6.0,
        rect: Rect::NOTHING,
        frame: 0,
    };
    slider.draw(vec![]);
    slider.draw(vec![]);
    slider.drag(2.0, 2.57);
    assert_eq!((slider.min, slider.max), (2.57, 6.0));
    slider.drag(6.0, 8.23);
    assert_eq!((slider.min, slider.max), (2.57, 8.23));
    // 下限不能越过上限；两个滑块重合后仍能向两侧展开。
    slider.drag(2.57, 9.5);
    assert_eq!((slider.min, slider.max), (8.23, 8.23));
    slider.drag(8.23, 1.24);
    assert_eq!((slider.min, slider.max), (1.24, 8.23));
    slider.drag(8.23, 12.0);
    assert_eq!((slider.min, slider.max), (1.24, 10.0));
}
