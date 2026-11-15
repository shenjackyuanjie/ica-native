use crate::app::test_support::test_app;
use eframe::App;
use egui::{Pos2, RawInput, Rect, Vec2};

#[test]
fn root_panels_follow_viewport_growth_and_shrink_without_unpainted_edges() {
    let (mut app, _rx) = test_app();
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    // 从原始窗口向右下拉大，再缩小穿过窄布局阈值。
    for size in [
        Vec2::new(1100.0, 840.0),
        Vec2::new(1420.0, 990.0),
        Vec2::new(600.0, 700.0),
    ] {
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.ui(ui, &mut frame),
            );
            let corner = screen.max - Vec2::splat(2.0);
            let corner_painted = output.shapes.iter().any(|shape| {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    rect.fill != egui::Color32::TRANSPARENT
                        && rect.rect.contains(corner)
                        && shape.clip_rect.contains(corner)
                } else {
                    false
                }
            });
            // 无渲染后端时也必须消费纹理更新，断言失败前同样清理。
            output.textures_delta.clear();
            assert!(
                corner_painted,
                "窗口尺寸 {size:?} 的右下角应由根面板背景覆盖"
            );
        }
    }
}
