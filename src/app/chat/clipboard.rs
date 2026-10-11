//! 输入区附件的来源：系统剪贴板图片与本地文件。
//!
//! 剪贴板读取在 Windows 上直连 Win32（优先取 PNG 原始字节，其次 DIB 解码），
//! 其他平台使用 `arboard`；两种来源最终都统一编码为 PNG 的 `PendingImage`。
//! 本地文件按扩展名推断 MIME，分别构造 `PendingImage` 与 `PendingFile`。
//! 另提供 Ctrl+V 系统快捷键探测：egui-winit 在文本粘贴失败时会吞掉该事件，
//! 需要直接查询键盘状态补获"粘贴图片"意图。

use crate::app::{IcaApp, PendingFile, PendingImage};
use std::path::Path;

impl IcaApp {
    /// 按扩展名推断图片 MIME；只识别常见图片格式，其他返回 `None`。
    fn image_mime_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            "tif" | "tiff" => Some("image/tiff"),
            _ => None,
        }
    }

    /// 从系统剪贴板读取图片并转为待发送的 `PendingImage`。
    ///
    /// Windows 上先尝试直连剪贴板取原始字节（保真、免重编码），失败或非
    /// Windows 平台回退到 `arboard` 的 RGBA 数据并重编码为 PNG。
    pub fn load_clipboard_image() -> anyhow::Result<PendingImage> {
        #[cfg(windows)]
        if let Ok(image) = Self::load_clipboard_image_windows() {
            return Ok(image);
        }

        let mut clipboard = arboard::Clipboard::new()?;
        let img = clipboard.get_image()?;
        tracing::debug!(
            "剪贴板图片: {}x{}, {} bytes",
            img.width,
            img.height,
            img.bytes.len()
        );

        Self::rgba_clipboard_image_to_pending(img.width, img.height, img.bytes.into_owned())
    }

    /// 把 RGBA 字节包装为图片缓冲；尺寸与数据长度不符时报错。
    fn rgba_clipboard_image_to_pending(
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    ) -> anyhow::Result<PendingImage> {
        let buf = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
            .ok_or_else(|| anyhow::anyhow!("剪贴板图片数据尺寸不正确"))?;
        Self::encode_rgba_image_to_pending(buf)
    }

    /// 把 RGBA 图片编码为 PNG 字节，统一以 `clipboard.png` 之名作为待发送图片。
    fn encode_rgba_image_to_pending(buf: image::RgbaImage) -> anyhow::Result<PendingImage> {
        let mut png_data = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(std::io::Cursor::new(&mut png_data));
        image::ImageEncoder::write_image(
            encoder,
            buf.as_raw(),
            buf.width(),
            buf.height(),
            image::ExtendedColorType::Rgba8,
        )?;

        Ok(PendingImage::new(
            "clipboard.png".to_string(),
            "image/png".to_string(),
            png_data,
        ))
    }

    /// Windows 专用：直连 Win32 剪贴板读取图片。
    ///
    /// 优先取注册的 `PNG` 格式原始字节；否则依次尝试 `CF_DIBV5`/`CF_DIB`
    /// 位图并解码。打开剪贴板后用 `ClipboardGuard` 确保关闭。
    #[cfg(windows)]
    fn load_clipboard_image_windows() -> anyhow::Result<PendingImage> {
        use windows_sys::Win32::System::{
            DataExchange::{CloseClipboard, OpenClipboard, RegisterClipboardFormatA},
            Ole::{CF_DIB, CF_DIBV5},
        };

        struct ClipboardGuard;

        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }

        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return Err(anyhow::anyhow!("无法打开系统剪贴板"));
            }
        }
        let _guard = ClipboardGuard;

        let png_format = unsafe { RegisterClipboardFormatA(c"PNG".as_ptr().cast()) };
        if png_format != 0
            && let Some(data) = unsafe { Self::read_windows_clipboard_format(png_format)? }
        {
            tracing::debug!("从 Windows 剪贴板读取 PNG 图片: {} bytes", data.len());
            return Ok(PendingImage::new(
                "clipboard.png".to_string(),
                "image/png".to_string(),
                data,
            ));
        }

        for (format, name) in [(CF_DIBV5 as u32, "CF_DIBV5"), (CF_DIB as u32, "CF_DIB")] {
            if let Some(data) = unsafe { Self::read_windows_clipboard_format(format)? } {
                tracing::debug!("从 Windows 剪贴板读取 {} 图片: {} bytes", name, data.len());
                return Self::dib_clipboard_image_to_pending(&data);
            }
        }

        Err(anyhow::anyhow!("剪贴板里没有可支持的图片格式"))
    }

    /// Windows 专用：按剪贴板格式号读取全局内存中的原始字节。
    ///
    /// 格式不可用返回 `Ok(None)`；句柄为空、数据为空、锁定失败分别报错。
    /// 调用方需已打开剪贴板。
    #[cfg(windows)]
    unsafe fn read_windows_clipboard_format(format: u32) -> anyhow::Result<Option<Vec<u8>>> {
        use windows_sys::Win32::System::{
            DataExchange::{GetClipboardData, IsClipboardFormatAvailable},
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        };

        if unsafe { IsClipboardFormatAvailable(format) } == 0 {
            return Ok(None);
        }

        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            return Err(anyhow::anyhow!("剪贴板格式 {} 数据句柄为空", format));
        }

        let size = unsafe { GlobalSize(handle) };
        if size == 0 {
            return Err(anyhow::anyhow!("剪贴板格式 {} 数据为空", format));
        }

        let ptr = unsafe { GlobalLock(handle) };
        if ptr.is_null() {
            return Err(anyhow::anyhow!("剪贴板格式 {} 数据锁定失败", format));
        }

        let data = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
        unsafe {
            GlobalUnlock(handle);
        }
        Ok(Some(data))
    }

    /// Windows 专用：把无文件头的 DIB 字节解码为 RGBA，再编码为 PNG。
    #[cfg(windows)]
    fn dib_clipboard_image_to_pending(data: &[u8]) -> anyhow::Result<PendingImage> {
        let decoder =
            image::codecs::bmp::BmpDecoder::new_without_file_header(std::io::Cursor::new(data))?;
        let rgba = image::DynamicImage::from_decoder(decoder)?.into_rgba8();
        Self::encode_rgba_image_to_pending(rgba)
    }

    /// 查询系统级 Ctrl+V 是否刚按下（Windows 为真实探测，其他平台恒为 false）。
    ///
    /// 用于在 egui 文本粘贴失败后仍能捕获"粘贴图片"意图。
    pub fn system_paste_shortcut_pressed() -> bool {
        system_paste_shortcut_pressed()
    }

    /// 从本地路径读取图片文件作为待发送图片；不支持的格式报错。
    ///
    /// 文件名取路径末段，缺失时退化为 `image`。
    pub fn load_pending_image(path: &Path) -> anyhow::Result<PendingImage> {
        let mime_type = Self::image_mime_type(path)
            .ok_or_else(|| anyhow::anyhow!("不支持的图片格式: {}", path.display()))?;
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());

        Ok(PendingImage::new(name, mime_type.to_string(), data))
    }

    /// 按扩展名推断任意文件的 MIME；未知类型回退为 `application/octet-stream`。
    ///
    /// 覆盖图片、音频、视频与常见文档/压缩格式，供附件上传声明类型。
    pub fn guess_mime_type(path: &Path) -> String {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "tif" | "tiff" => "image/tiff",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "flac" => "audio/flac",
            "mp4" => "video/mp4",
            "pdf" => "application/pdf",
            "zip" => "application/zip",
            "txt" => "text/plain",
            _ => "application/octet-stream",
        }
        .to_string()
    }

    /// 从本地路径读取任意文件作为待发送附件，MIME 由 `guess_mime_type` 推断。
    pub fn load_pending_file(path: &Path) -> anyhow::Result<PendingFile> {
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let file_type = Self::guess_mime_type(path);

        Ok(PendingFile::new(name, file_type, data))
    }
}

/// Windows 实现：用 `GetAsyncKeyState` 识别 Ctrl 按住且 V 键刚按下。
///
/// egui-winit 在文本粘贴失败时会吞掉 Ctrl+V 的 RawInput，只能直接查询键盘
/// 状态；按最低位判断"本次刚按下"，避免每一帧重复触发。
#[cfg(windows)]
fn system_paste_shortcut_pressed() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_V};

    // egui-winit consumes Ctrl+V before it reaches RawInput when text paste fails.
    // 通过最低位识别 V 键本次是否刚按下，避免每一帧都重复尝试。
    unsafe {
        let ctrl_down = GetAsyncKeyState(VK_CONTROL as i32) < 0;
        let v_pressed_since_last_check = (GetAsyncKeyState(VK_V as i32) & 1) != 0;
        ctrl_down && v_pressed_since_last_check
    }
}

/// 非 Windows 平台没有对应的系统快捷键探测，恒为 false。
#[cfg(not(windows))]
fn system_paste_shortcut_pressed() -> bool {
    false
}
