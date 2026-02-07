use std::{
    collections::HashMap,
    mem::size_of,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use egui::{
    decode_animated_image_uri,
    load::{Bytes, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
    ColorImage, Context,
};
use image::ImageFormat;
use tracing::info;

type Cache = Arc<Mutex<HashMap<String, Entry>>>;

#[derive(Clone)]
pub struct TrackingImageLoader {
    cache: Cache,
    total_decoded_bytes: Arc<AtomicU64>,
}

enum Entry {
    Pending,
    Ready {
        image: Arc<ColorImage>,
        byte_size: u64,
    },
    Error(String),
}

impl TrackingImageLoader {
    pub const ID: &'static str = egui::generate_loader_id!(TrackingImageLoader);

    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            total_decoded_bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    fn is_supported_uri(uri: &str) -> bool {
        let Some(ext) = std::path::Path::new(uri)
            .extension()
            .and_then(|ext| ext.to_str().map(|ext| ext.to_lowercase()))
        else {
            return true;
        };

        if ext == "svg" || ext == "gif" || ext == "webp" {
            return false;
        }

        ImageFormat::from_extension(ext).is_some_and(|format| format.reading_enabled())
    }

    fn is_supported_mime(mime: &str) -> bool {
        if mime.contains("image/svg") || mime.contains("image/gif") || mime.contains("image/webp") {
            return false;
        }

        let mimes_to_defer = [
            "application/octet-stream",
            "application/x-msdownload",
            "application/force-download",
        ];
        for m in &mimes_to_defer {
            if mime.contains(m) {
                return true;
            }
        }

        ImageFormat::from_mime_type(mime).is_some_and(|format| format.reading_enabled())
    }

    fn decode_image(bytes: &Bytes) -> Result<Arc<ColorImage>, String> {
        let img = image::load_from_memory(bytes.as_ref()).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = rgba.into_raw();
        let color = ColorImage::from_rgba_unmultiplied(size, &pixels);
        Ok(Arc::new(color))
    }


}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2}GB", b / GB)
    } else if b >= MB {
        format!("{:.2}MB", b / MB)
    } else if b >= KB {
        format!("{:.2}KB", b / KB)
    } else {
        format!("{}B", bytes)
    }
}

impl Default for TrackingImageLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageLoader for TrackingImageLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        let uri = decode_animated_image_uri(uri).map_or(uri, |(uri, _)| uri);

        if uri.starts_with("file://") && !Self::is_supported_uri(uri) {
            return Err(LoadError::NotSupported);
        }

        let mut cache_lock = self.cache.lock();
        if let Some(entry) = cache_lock.get_mut(uri) {
            return match entry {
                Entry::Ready { image, .. } => Ok(ImagePoll::Ready {
                    image: image.clone(),
                }),
                Entry::Error(err) => Err(LoadError::Loading(err.clone())),
                Entry::Pending => Ok(ImagePoll::Pending { size: None }),
            };
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                if let Some(mime) = mime
                    && !Self::is_supported_mime(&mime)
                {
                    return Err(LoadError::FormatNotSupported {
                        detected_format: Some(mime),
                    });
                }

                cache_lock.insert(uri.to_string(), Entry::Pending);
                drop(cache_lock);

                let cache = self.cache.clone();
                let total_decoded_bytes = self.total_decoded_bytes.clone();
                let uri = uri.to_string();
                let ctx = ctx.clone();

                std::thread::Builder::new()
                    .name(format!("TrackingImageLoader::load({uri:?})"))
                    .spawn(move || {
                        let result = Self::decode_image(&bytes);
                        let mut cache = cache.lock();
                        match result {
                            Ok(image) => {
                                let byte_size =
                                    (image.pixels.len() * size_of::<egui::Color32>()) as u64;
                                let total = total_decoded_bytes
                                    .fetch_add(byte_size, Ordering::Relaxed)
                                    + byte_size;
                                let bytes_h = format_bytes(byte_size);
                                let total_h = format_bytes(total);
                                let [w, h] = image.size;
                                info!(
                                    "image decoded: uri={} size={}x{} bytes={} total={}",
                                    uri, w, h, bytes_h, total_h
                                );
                                cache.insert(
                                    uri.clone(),
                                    Entry::Ready {
                                        image,
                                        byte_size,
                                    },
                                );
                            }
                            Err(err) => {
                                cache.insert(uri.clone(), Entry::Error(err));
                            }
                        }

                        ctx.request_repaint();
                    })
                    .expect("failed to spawn image decode thread");

                Ok(ImagePoll::Pending { size: None })
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(err) => Err(err),
        }
    }

    fn forget(&self, uri: &str) {
        let _ = self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .map(|entry| match entry {
                Entry::Ready { byte_size, .. } => *byte_size as usize,
                Entry::Error(err) => err.len(),
                Entry::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.cache
            .lock()
            .values()
            .any(|entry| matches!(entry, Entry::Pending))
    }
}

pub fn install_tracking_image_loader(ctx: &Context) {
    if ctx.is_loader_installed(TrackingImageLoader::ID) {
        return;
    }
    ctx.add_image_loader(Arc::new(TrackingImageLoader::new()));
    info!("installed TrackingImageLoader");
}
