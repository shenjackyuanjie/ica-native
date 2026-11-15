//! 签到间隔区间：同一轨道的两个滑块，拖动时锁定端点且禁止交叉。

#[cfg(test)]
#[path = "tests/delay_range.rs"]
mod tests;

use super::{MAX_SIGN_DELAY_SECONDS, MIN_SIGN_DELAY_SECONDS};
use egui::{Response, Sense, Ui};

pub fn render(ui: &mut Ui, min: &mut f64, max: &mut f64) {
    ui.horizontal(|ui| {
        ui.label("随机间隔");
        ui.add(
            egui::DragValue::new(min)
                .range(MIN_SIGN_DELAY_SECONDS..=*max)
                .speed(0.01)
                .fixed_decimals(2)
                .suffix(" 秒"),
        );
        ui.label("至");
        ui.add(
            egui::DragValue::new(max)
                .range(*min..=MAX_SIGN_DELAY_SECONDS)
                .speed(0.01)
                .fixed_decimals(2)
                .suffix(" 秒"),
        );
    });
    range_slider(ui, min, max);
}

fn range_slider(ui: &mut Ui, min: &mut f64, max: &mut f64) -> Response {
    let height = ui.spacing().interact_size.y;
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        Sense::click_and_drag(),
    );
    let radius = 7.0;
    let rail = rect.shrink2(egui::vec2(radius, 0.0));
    let position = |value: f64| {
        egui::lerp(
            rail.x_range(),
            ((value - MIN_SIGN_DELAY_SECONDS) / (MAX_SIGN_DELAY_SECONDS - MIN_SIGN_DELAY_SECONDS))
                as f32,
        )
    };
    let selection_id = response.id.with("upper_handle");
    if (response.drag_started() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let origin = ui
            .input(|input| input.pointer.press_origin())
            .unwrap_or(pointer);
        let lower_x = position(*min);
        let upper_x = position(*max);
        // 两端重合时，向左选择下限，向右选择上限，始终能够重新展开区间。
        let upper = if (upper_x - lower_x).abs() < 0.5 {
            pointer.x > lower_x
        } else {
            (origin.x - upper_x).abs() < (origin.x - lower_x).abs()
        };
        ui.data_mut(|data| data.insert_temp(selection_id, upper));
        response.request_focus();
    }
    let upper = ui
        .data(|data| data.get_temp::<bool>(selection_id))
        .unwrap_or(false);
    let mut value = None;
    if (response.dragged() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        value = Some(egui::remap_clamp(
            pointer.x,
            rail.x_range(),
            MIN_SIGN_DELAY_SECONDS as f32..=MAX_SIGN_DELAY_SECONDS as f32,
        ) as f64);
    }
    if response.has_focus() {
        let direction = ui.input(|input| {
            i32::from(input.key_pressed(egui::Key::ArrowRight))
                - i32::from(input.key_pressed(egui::Key::ArrowLeft))
        });
        if direction != 0 {
            value = Some(if upper { *max } else { *min } + f64::from(direction) * 0.01);
        }
    }
    if let Some(value) = value {
        let value = (value * 100.0).round() / 100.0;
        let target = if upper {
            value.clamp(*min, MAX_SIGN_DELAY_SECONDS)
        } else {
            value.clamp(MIN_SIGN_DELAY_SECONDS, *max)
        };
        let endpoint = if upper { &mut *max } else { &mut *min };
        if *endpoint != target {
            *endpoint = target;
            response.mark_changed();
        }
    }
    let lower_x = position(*min);
    let upper_x = position(*max);
    let y = rect.center().y;
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();
    painter.line_segment(
        [egui::pos2(rail.left(), y), egui::pos2(rail.right(), y)],
        egui::Stroke::new(4.0, ui.visuals().widgets.inactive.bg_fill),
    );
    painter.line_segment(
        [egui::pos2(lower_x, y), egui::pos2(upper_x, y)],
        egui::Stroke::new(4.0, ui.visuals().selection.bg_fill),
    );
    for x in [lower_x, upper_x] {
        painter.circle(egui::pos2(x, y), radius, visuals.bg_fill, visuals.fg_stroke);
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Slider,
            ui.is_enabled(),
            format!("随机间隔 {min:.2} 至 {max:.2} 秒"),
        )
    });
    response.on_hover_text("拖动两个滑块调整上下限；选中滑块后可用左右键微调 0.01 秒")
}
