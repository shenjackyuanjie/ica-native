//! ica-native 的二进制入口（crate root）。
//!
//! 本文件只做「装配」：声明全部子模块、解析命令行日志级别、加载配置快照、
//! 选择渲染后端，并把 [`ConfigStore`](crate::config::ConfigStore) 交给
//! [`IcaApp`](crate::app::IcaApp)。聊天界面、Bridge 通信与各项工具分别位于
//! `app`、`ica` 等子模块中，入口不承载业务逻辑。
//!
//! 启动顺序：`main` → `init_logging` → `egui_main`；
//! 窗口关闭后由 `egui_main` 负责把运行期内可能变化的配置写回磁盘。

pub mod agent_context;
pub mod app;
pub mod assets;
pub mod config;
pub mod face_data;
pub mod ica;
pub mod image_loader;
pub mod memory_probe;
pub mod noticer;

use crate::config::{RendererBackend, WgpuBackend};
use eframe::egui;
use egui::IconData;

/// 客户端版本号，取 `Cargo.toml` 的 `version`，供关于界面与日志标识使用。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 项目仓库地址，用于「关于 / 反馈入口」中引导用户打开浏览器。
pub const GITHUB_LINK: &str = "https://github.com/shenjackyuanjie/ica-native";

/// 一次性「停止通知」接收端。
///
/// 后台服务（如 Noticer、Agent 上下文服务）把 `oneshot::Sender<()>` 交给控制器
/// 持有，需要停机时发送方 `send` 或直接 drop，都会让本接收端立即返回，
/// 从而触发 `axum` 的优雅关闭。
pub type StopGetter = tokio::sync::oneshot::Receiver<()>;

/// 从命令行参数推断日志级别：`--vv` 为 `TRACE`，`--v` 为 `DEBUG`，其余为 `INFO`。
///
/// 与参数顺序无关，无法识别的参数被忽略；`--vv` 优先于 `--v`。
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

/// 初始化全局 `tracing` 订阅者，级别由命令行参数决定。
///
/// 同时用 `EnvFilter` 静默两个高频噪声源：`egui_winit` 的剪贴板事件与
/// 自定义图片加载器的逐图日志，避免刷屏掩盖真实告警。
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

/// 程序入口：初始化日志，打出启动探针，再进入 egui 主流程。
///
/// 返回 `Err` 时由 Rust 运行时打印错误并以非零退出码结束。
fn main() -> anyhow::Result<()> {
    init_logging();
    memory_probe::log("main:start");
    egui_main()
}

/// eframe 主流程：加载配置、构造窗口选项、运行 GUI，退出前写回配置。
///
/// 关键步骤依次为：
/// 1. `ConfigStore::load` 解析配置文件（命令行 `--config` > 环境变量 > 默认路径）；
/// 2. 解码内嵌的 512×512 图标作为窗口图标；
/// 3. 按 `screen.renderer` / `screen.wgpu_backend` / `screen.vsync` 选择渲染后端；
/// 4. `run_native` 中创建 `IcaApp` 并阻塞运行，直到窗口关闭；
/// 5. 退出后 `ConfigStore::save` 把内存中的配置序列化回 TOML。
fn egui_main() -> anyhow::Result<()> {
    memory_probe::log("egui_main:start");
    let config_store = config::ConfigStore::load()?;
    memory_probe::log("cfg:init");

    // 获取一个 cfg 快照
    let config = config_store.snapshot();
    memory_probe::log("cfg:snapshot");

    // 图标走「内嵌 PNG → RGBA8 → IconData」，egui 窗口图标只接受原始像素缓冲。
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
    // 渲染后端与呈现模式在此一次性决定：VSync 决定帧提交时机，
    // 低延迟预设（LOW_LATENCY）用于压小输入到显示的延迟。
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
    // run_native 会阻塞到窗口关闭，因此这里的 save 只在正常退出后执行。
    config_store.save()?;
    memory_probe::log("cfg:write_back");
    Ok(())
}
