use eframe::egui;
use egui::IconData;

pub mod app;
pub mod assets;
pub mod cfg;
pub mod face_data;
pub mod ica;
pub mod image_loader;
pub mod memory_probe;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_LINK: &str = "https://github.com/shenjackyuanjie/ica-native";

pub type StopGetter = tokio::sync::oneshot::Receiver<()>;

fn cli_log_level_from_args<I, S>(args: I) -> tracing::Level
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut level = tracing::Level::INFO;
    for arg in args {
        match arg.as_ref() {
            "--vv" => return tracing::Level::TRACE,
            "--v" => level = tracing::Level::DEBUG,
            _ => {}
        }
    }
    level
}

fn init_logging() {
    let level = cli_log_level_from_args(std::env::args().skip(1));
    let level_name = match level {
        tracing::Level::ERROR => "error",
        tracing::Level::WARN => "warn",
        tracing::Level::INFO => "info",
        tracing::Level::DEBUG => "debug",
        tracing::Level::TRACE => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{level_name},egui_winit::clipboard=off,ica_native::image_loader=off"
    ));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn main() -> anyhow::Result<()> {
    init_logging();
    memory_probe::log("main:start");
    egui_main()
}

fn egui_main() -> anyhow::Result<()> {
    memory_probe::log("egui_main:start");
    cfg::init_cfg();
    memory_probe::log("cfg:init");

    // 获取一个 cfg 快照
    let config = cfg::get_cfg_snapshot();
    memory_probe::log("cfg:snapshot");

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
    memory_probe::log("icon:loaded");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.screen.width, config.screen.height])
            .with_drag_and_drop(true)
            .with_icon(icon),
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(
            eframe::egui_wgpu::SurfaceConfig {
                present_mode: if config.screen.vsync {
                    eframe::wgpu::PresentMode::AutoVsync
                } else {
                    eframe::wgpu::PresentMode::AutoNoVsync
                },
                desired_maximum_frame_latency: eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
                    .desired_maximum_frame_latency,
            },
        ),
        centered: config.screen.centered,
        ..Default::default()
    };

    eframe::run_native(
        "ica native",
        options,
        Box::new(|cc| {
            // 安装 egui extra
            egui_extras::install_image_loaders(&cc.egui_ctx);
            memory_probe::log("egui:image_loaders");
            // 安装图片统计加载器
            image_loader::install_tracking_image_loader(&cc.egui_ctx);
            memory_probe::log("egui:tracking_loader");
            let app = app::IcaApp::new(cc);
            memory_probe::log("app:new");
            Ok(Box::new(app))
        }),
    )
    .expect("error in eframe::run_native");

    memory_probe::log("egui_main:exit");
    cfg::write_back_cfg()?;
    memory_probe::log("cfg:write_back");
    Ok(())
}
