use std::{borrow::Cow, sync::Arc};

use gpui::{App, AppContext, Bounds, Font, Pixels, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

struct IcaThemeSettingsProvider {
    ui_font: Font,
    buffer_font: Font,
}

impl IcaThemeSettingsProvider {
    fn new() -> Self {
        Self {
            ui_font: gpui::font("Noto Sans CJK SC"),
            buffer_font: gpui::font("Noto Sans CJK SC"),
        }
    }
}

impl theme::ThemeSettingsProvider for IcaThemeSettingsProvider {
    fn ui_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.ui_font
    }

    fn buffer_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.buffer_font
    }

    fn ui_font_size(&self, _: &App) -> Pixels {
        px(14.)
    }

    fn buffer_font_size(&self, _: &App) -> Pixels {
        px(14.)
    }

    fn ui_density(&self, _: &App) -> theme::UiDensity {
        theme::UiDensity::Default
    }
}

pub mod app;
pub mod assets;
pub mod config;
pub mod face_data;
pub mod ica;
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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(level_name))
        .init();
}

fn main() -> anyhow::Result<()> {
    init_logging();
    memory_probe::log("main:start");
    let config_store = config::ConfigStore::load()?;
    let config = config_store.snapshot();
    let width = config.screen.width.max(760.0);
    let height = config.screen.height.max(560.0);
    let runtime = app::runtime::AppRuntime::new(&config);
    let runtime_handle = runtime.handle();

    application()
        .with_assets(zed_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_tokio::init_from_handle(cx, runtime_handle);
            cx.set_http_client(Arc::new(
                reqwest_client::ReqwestClient::user_agent(concat!(
                    "ica-native/",
                    env!("CARGO_PKG_VERSION")
                ))
                .expect("创建 GPUI HTTP 客户端失败"),
            ));
            theme::init(theme::LoadThemes::All(Box::new(zed_assets::Assets)), cx);
            theme::set_theme_settings_provider(Box::new(IcaThemeSettingsProvider::new()), cx);
            if let Err(error) = cx.text_system().add_fonts(vec![
                Cow::Borrowed(assets::fonts::FONT_思源黑体),
                Cow::Borrowed(assets::fonts::FONT_UNIFONT),
            ]) {
                tracing::warn!(%error, "加载内置中文字体失败，将使用系统回退字体");
            }
            app::apply_configured_theme(&config, cx);
            app::bind_keys(cx);

            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(700.), px(500.))),
                    app_id: Some("ica-native".to_string()),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| app::IcaApp::new(runtime, config_store, window, cx)),
            )
            .expect("打开 GPUI 主窗口失败");

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}
