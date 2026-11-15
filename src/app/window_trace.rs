//! 按需记录窗口几何信息，不采集账号、消息或输入事件。
//! 设置 ICA_NATIVE_WINDOW_LOG 为日志路径后生效；不开启时不触碰文件或改变重绘策略。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq)]
pub struct Geometry {
    native_pixels: Option<[u32; 2]>,
    native_scale: Option<f64>,
    input_rect: Option<egui::Rect>,
    input_inner_rect: Option<egui::Rect>,
    viewport_rect: egui::Rect,
    content_rect: egui::Rect,
    root_rect: egui::Rect,
    root_clip: egui::Rect,
    used_rect: egui::Rect,
    pixels_per_point: f32,
    zoom: f32,
    backend: &'static str,
}

struct Trace {
    file: File,
    last: Option<Geometry>,
    last_write: Instant,
    samples: u64,
}

fn trace() -> Option<&'static Mutex<Trace>> {
    static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();
    TRACE.get_or_init(|| {
        let path = std::env::var_os("ICA_NATIVE_WINDOW_LOG").filter(|path| !path.is_empty())?;
        let mut file = match OpenOptions::new().create(true).append(true).open(path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(%error, "窗口几何日志无法打开");
                return None;
            }
        };
        let _ = writeln!(file, "--- ica-native {} 窗口几何诊断 pid={}；rect 为逻辑点，native_pixels 为客户区物理像素 ---",
            env!("CARGO_PKG_VERSION"), std::process::id());
        Some(Mutex::new(Trace { file, last: None, last_write: Instant::now(), samples: 0 }))
    }).as_ref()
}

/// 在面板布局消耗根 Ui 之前捕获输入与原生客户区尺寸。
pub fn capture(ui: &egui::Ui, frame: &eframe::Frame) -> Option<Geometry> {
    trace()?;
    let window = frame.winit_window();
    let (input_rect, input_inner_rect) =
        ui.input(|input| (input.raw.screen_rect, input.viewport().inner_rect));
    Some(Geometry {
        native_pixels: window.map(|window| {
            let size = window.inner_size();
            [size.width, size.height]
        }),
        native_scale: window.map(|window| window.scale_factor()),
        input_rect,
        input_inner_rect,
        viewport_rect: ui.ctx().viewport_rect(),
        content_rect: ui.ctx().content_rect(),
        root_rect: ui.max_rect(),
        root_clip: ui.clip_rect(),
        used_rect: egui::Rect::NOTHING,
        pixels_per_point: ui.ctx().pixels_per_point(),
        zoom: ui.ctx().zoom_factor(),
        backend: if frame.wgpu_render_state().is_some() {
            "wgpu"
        } else if frame.gl().is_some() {
            "glow"
        } else {
            "headless"
        },
    })
}

/// 帧末记录实际占用区域；只在尺寸变化或已有重绘超过一秒时写入，不主动请求重绘。
pub fn record(ui: &egui::Ui, geometry: Option<Geometry>) {
    let Some(mut geometry) = geometry else { return };
    geometry.used_rect = ui.min_rect();
    let Some(trace) = trace() else { return };
    let Ok(mut trace) = trace.lock() else { return };
    trace.samples += 1;
    if trace.last.as_ref() == Some(&geometry) && trace.last_write.elapsed() < Duration::from_secs(1)
    {
        return;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let samples = trace.samples;
    if let Err(error) = writeln!(trace.file, "t={timestamp} frame={samples} {geometry:?}") {
        tracing::warn!(%error, "窗口几何日志写入失败");
    }
    trace.last = Some(geometry);
    trace.last_write = Instant::now();
}
