use eframe::egui;
use egui::IconData;

use crate::config::{RendererBackend, WgpuBackend};

pub mod app;
pub mod assets;
pub mod config;
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
    let config_store = config::ConfigStore::load()?;
    memory_probe::log("cfg:init");

    // 获取一个 cfg 快照
    let config = config_store.snapshot();
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

    let renderer = match config.screen.renderer {
        RendererBackend::Glow => eframe::Renderer::Glow,
        RendererBackend::Wgpu => eframe::Renderer::Wgpu,
    };
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(
        eframe::egui_wgpu::SurfaceConfig {
            present_mode: if config.screen.vsync {
                eframe::wgpu::PresentMode::AutoVsync
            } else {
                eframe::wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
                .desired_maximum_frame_latency,
        },
    );
    #[cfg(target_os = "windows")]
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        // 显式选择图形 API，避免 WGPU 自动落回会触发闪屏的 OpenGL 路径。
        setup.instance_descriptor.backends = match config.screen.wgpu_backend {
            WgpuBackend::Dx12 => eframe::wgpu::Backends::DX12,
            WgpuBackend::Vulkan => eframe::wgpu::Backends::VULKAN,
            WgpuBackend::Metal => {
                tracing::warn!("Windows 不支持 Metal，WGPU 将改用 DX12");
                eframe::wgpu::Backends::DX12
            }
        };
    }
    tracing::info!(
        renderer = ?config.screen.renderer,
        wgpu_backend = ?config.screen.wgpu_backend,
        vsync = config.screen.vsync,
        "初始化渲染后端"
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.screen.width, config.screen.height])
            .with_drag_and_drop(true)
            .with_icon(icon),
        renderer,
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: config.screen.vsync,
            ..Default::default()
        },
        wgpu_options,
        centered: config.screen.centered,
        ..Default::default()
    };
    memory_probe::log("egui:before_run_native");

    eframe::run_native(
        "ica native",
        options,
        Box::new({
            let config_store = config_store.clone();
            move |cc| {
                memory_probe::log("egui:creation_context");
                // 安装 egui extra
                egui_extras::install_image_loaders(&cc.egui_ctx);
                memory_probe::log("egui:image_loaders");
                // 安装图片统计加载器
                image_loader::install_tracking_image_loader(
                    &cc.egui_ctx,
                    image_loader::ImageCacheSettings::from_config(&config_store.snapshot()),
                );
                memory_probe::log("egui:tracking_loader");
                let app = app::IcaApp::new(cc, config_store.clone());
                memory_probe::log("app:new");
                Ok(Box::new(app))
            }
        }),
    )
    .expect("eframe::run_native 运行失败");

    memory_probe::log("egui_main:exit");
    config_store.save()?;
    memory_probe::log("cfg:write_back");
    Ok(())
}
