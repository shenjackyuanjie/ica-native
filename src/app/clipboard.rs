use std::path::Path;

use super::{IcaApp, PendingFile, PendingImage};

impl IcaApp {
    fn image_mime_type(path: &Path) -> Option<&'static str> {
        let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "bmp" => Some("image/bmp"),
            _ => None,
        }
    }

    pub(super) fn load_clipboard_image() -> anyhow::Result<PendingImage> {
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

    fn rgba_clipboard_image_to_pending(
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    ) -> anyhow::Result<PendingImage> {
        let buf = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
            .ok_or_else(|| anyhow::anyhow!("剪贴板图片数据尺寸不正确"))?;
        Self::encode_rgba_image_to_pending(buf)
    }

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

        Ok(PendingImage {
            name: "clipboard.png".to_string(),
            mime_type: "image/png".to_string(),
            data: png_data,
        })
    }

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
            return Ok(PendingImage {
                name: "clipboard.png".to_string(),
                mime_type: "image/png".to_string(),
                data,
            });
        }

        for (format, name) in [(CF_DIBV5 as u32, "CF_DIBV5"), (CF_DIB as u32, "CF_DIB")] {
            if let Some(data) = unsafe { Self::read_windows_clipboard_format(format)? } {
                tracing::debug!("从 Windows 剪贴板读取 {} 图片: {} bytes", name, data.len());
                return Self::dib_clipboard_image_to_pending(&data);
            }
        }

        Err(anyhow::anyhow!("剪贴板里没有可支持的图片格式"))
    }

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

    #[cfg(windows)]
    fn dib_clipboard_image_to_pending(data: &[u8]) -> anyhow::Result<PendingImage> {
        let decoder =
            image::codecs::bmp::BmpDecoder::new_without_file_header(std::io::Cursor::new(data))?;
        let rgba = image::DynamicImage::from_decoder(decoder)?.into_rgba8();
        Self::encode_rgba_image_to_pending(rgba)
    }

    pub(super) fn system_paste_shortcut_pressed() -> bool {
        system_paste_shortcut_pressed()
    }

    pub(super) fn load_pending_image(path: &Path) -> anyhow::Result<PendingImage> {
        let mime_type = Self::image_mime_type(path)
            .ok_or_else(|| anyhow::anyhow!("不支持的图片格式: {}", path.display()))?;
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());

        Ok(PendingImage {
            name,
            mime_type: mime_type.to_string(),
            data,
        })
    }

    pub(super) fn guess_mime_type(path: &Path) -> String {
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

    pub(super) fn load_pending_file(path: &Path) -> anyhow::Result<PendingFile> {
        let data = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let file_type = Self::guess_mime_type(path);

        Ok(PendingFile {
            name,
            file_type,
            data,
        })
    }
}

#[cfg(windows)]
fn system_paste_shortcut_pressed() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_V};

    // egui-winit consumes Ctrl+V before it reaches RawInput when text paste fails.
    // The low bit lets us detect the fresh V key press without retrying every frame.
    unsafe {
        let ctrl_down = GetAsyncKeyState(VK_CONTROL as i32) < 0;
        let v_pressed_since_last_check = (GetAsyncKeyState(VK_V as i32) & 1) != 0;
        ctrl_down && v_pressed_since_last_check
    }
}

#[cfg(not(windows))]
fn system_paste_shortcut_pressed() -> bool {
    false
}
