use eframe::egui;
use egui::IconData;

pub mod app;
pub mod assets;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_LINK: &str = "https://github.com/shenjackyuanjie/ica-native";

fn main() -> anyhow::Result<()> {
    let icon = {
        let img =
            image::load_from_memory_with_format(assets::png::ICON_512X, image::ImageFormat::Png)?;
        let rgba_image = img.into_rgba8();
        let (w, h) = (rgba_image.width(), rgba_image.height());
        IconData {
            rgba: rgba_image.into_raw(),
            width: w,
            height: h,
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 768.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "ica native",
        options,
        Box::new(|cc| {
            // 安装 egui extra
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::IcaApp::new(cc)))
        }),
    )
    .expect("error in eframe::run_native");
    Ok(())
}
