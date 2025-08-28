use eframe::egui;
use egui::IconData;

pub mod app;
pub mod assets;
pub mod cfg;
pub mod client;
pub mod ica;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_LINK: &str = "https://github.com/shenjackyuanjie/ica-native";

pub type StopGetter = tokio::sync::oneshot::Receiver<()>;

fn main() -> anyhow::Result<()> {
    egui_main()
}

fn egui_main() -> anyhow::Result<()> {
    let config = cfg::init_cfg();
    

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
            .with_inner_size([config.screen.width, config.screen.height])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };

    let async_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4) // TODO: read from cfg
        .enable_all()
        .build()?;

    eframe::run_native(
        "ica native",
        options,
        Box::new(|cc| {
            // 安装 egui extra
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::IcaApp::new(cc, async_rt)))
        }),
    )
    .expect("error in eframe::run_native");
    Ok(())
}
