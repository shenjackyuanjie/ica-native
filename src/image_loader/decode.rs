use std::{mem::size_of, path::Path, sync::Arc};

use egui::{
    Color32, ColorImage,
    load::{Bytes, LoadError},
};
use image::ImageFormat;
use tracing::warn;

/// `egui` 会把动图 URI 包一层 `...#frame=...` 之类的内部表示；
/// 我们在 `load/forget` 两侧都统一做一次归一化，保证 key 一致。
pub fn normalize_uri(uri: &str) -> &str {
    egui::decode_animated_image_uri(uri).map_or(uri, |(uri, _)| uri)
}

/// 对 `file://` URI 做一个非常保守的快速过滤：
/// 仅把 SVG/GIF 提前让出去。
///
/// 这样不会因为扩展名奇怪/缺失而误伤本来能通过字节探测解码的图片。
pub fn should_handle_file_uri(uri: &str) -> bool {
    let Some(ext) = uri_extension(uri) else {
        return true;
    };

    !matches!(ext.as_str(), "svg" | "gif")
}

/// MIME 过滤：明确不想自己处理的格式直接放给别的 loader。
pub fn is_supported_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    if mime.contains("image/svg") || mime.contains("image/gif") {
        return false;
    }

    // 某些服务端会把图片错误地标成 octet-stream；这时仍然允许继续走字节探测。
    const MAYBE_IMAGE_MIMES: [&str; 3] = [
        "application/octet-stream",
        "application/x-msdownload",
        "application/force-download",
    ];
    if MAYBE_IMAGE_MIMES
        .iter()
        .any(|candidate| mime.contains(candidate))
    {
        return true;
    }

    ImageFormat::from_mime_type(&mime).is_some_and(can_decode_format)
}

/// 有些响应没有 MIME，只能从内容本身判断是不是 SVG。
pub fn is_svg_bytes(bytes: &Bytes) -> bool {
    let Ok(text) = std::str::from_utf8(bytes.as_ref()) else {
        return false;
    };

    let text = text.trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

/// 判断一份已拿到的字节是否应该继续由我们这个 loader 处理。
///
/// 这里会同时参考 MIME 和字节头：
/// - MIME 已明确声明为 svg/gif 时，直接放弃；
/// - MIME 不可靠或缺失时，再用字节头做兜底；
/// - GIF 交给 egui_extras 的动图 loader，保证聊天里能预览动图。
pub fn should_handle_loaded_bytes(mime: Option<&str>, bytes: &Bytes) -> bool {
    if let Some(mime) = mime
        && !is_supported_mime(mime)
    {
        return false;
    }

    if is_svg_bytes(bytes) {
        return false;
    }

    match image::guess_format(bytes.as_ref()) {
        Ok(ImageFormat::Gif) => false,
        Ok(format) => can_decode_format(format),
        // 猜不出来时不要过早拒绝，交给真正的解码流程报错会更准确。
        Err(_) => true,
    }
}

fn can_decode_format(format: ImageFormat) -> bool {
    format.reading_enabled()
}

/// 在后台线程里把原始字节解码成 `egui::ColorImage`。
pub fn decode_image(bytes: &Bytes) -> Result<Arc<ColorImage>, String> {
    let image = image::load_from_memory(bytes.as_ref()).map_err(|err| err.to_string())?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    Ok(Arc::new(ColorImage::from_rgba_unmultiplied(size, &pixels)))
}

/// 统计一张已解码图片大致占用的像素内存。
pub fn decoded_image_byte_size(image: &ColorImage) -> u64 {
    (image.pixels.len() * size_of::<Color32>()) as u64
}

/// 判断下载错误是否属于“永久性失败”（继续重试通常也不会成功）。
///
/// 这里同时兼容上游原始报错和我们归一化后的中文提示，
/// 这样调用方即使在 `normalize_image_load_error` 之后再检查，也不会丢失语义。
pub fn is_permanent_download_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("download url has expired")
        || message.contains("retcode\":-5503007")
        || message.contains("retcode=-5503007")
        || message.contains("图片链接已过期")
}

/// 把某些服务端错误翻译成对用户更友好的提示。
pub fn normalize_image_load_error(uri: &str, err: LoadError) -> LoadError {
    match err {
        LoadError::Loading(message) => {
            if is_permanent_download_error(&message) {
                warn!("image url expired: uri={} err={}", uri, message);
                LoadError::Loading("图片链接已过期，需要重新获取 URL".to_string())
            } else {
                LoadError::Loading(message)
            }
        }
        other => other,
    }
}

fn uri_extension(uri: &str) -> Option<String> {
    let uri = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("bytes://"))
        .unwrap_or(uri);
    let uri = uri.split(['?', '#']).next().unwrap_or(uri);
    Path::new(uri)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}
