use std::{
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
use lru::LruCache;
use tracing::debug;

use crate::cfg;

type Cache = Arc<Mutex<LruCache<String, Entry>>>;

#[derive(Clone)]
pub struct TrackingImageLoader {
    cache: Cache,
    total_decoded_bytes: Arc<AtomicU64>,
    max_cache_bytes: u64,
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
        let cfg = cfg::get_cfg_snapshot();
        Self {
            cache: Arc::new(Mutex::new(LruCache::unbounded())),
            total_decoded_bytes: Arc::new(AtomicU64::new(0)),
            max_cache_bytes: cfg.image_cache_max_bytes,
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

    fn entry_bytes(entry: &Entry) -> u64 {
        match entry {
            Entry::Ready { byte_size, .. } => *byte_size,
            Entry::Error(err) => err.len() as u64,
            Entry::Pending => 0,
        }
    }

    fn evict_if_needed(
        cache: &mut LruCache<String, Entry>,
        total_decoded_bytes: &AtomicU64,
        max_cache_bytes: u64,
    ) {
        let mut total = total_decoded_bytes.load(Ordering::Relaxed);
        if max_cache_bytes == 0 {
            while let Some((uri, entry)) = cache.pop_lru() {
                let bytes = Self::entry_bytes(&entry);
                if bytes > 0 {
                    total = total.saturating_sub(bytes);
                    total_decoded_bytes.fetch_sub(bytes, Ordering::Relaxed);
                }
                debug!("image cache evict (max=0): uri={} bytes={}", uri, bytes);
            }
            return;
        }

        while total > max_cache_bytes {
            let Some((uri, entry)) = cache.pop_lru() else {
                break;
            };
            let bytes = Self::entry_bytes(&entry);
            if bytes > 0 {
                total = total.saturating_sub(bytes);
                total_decoded_bytes.fetch_sub(bytes, Ordering::Relaxed);
            }
            debug!(
                "image cache evict: uri={} bytes={} total={} max={}",
                uri, bytes, total, max_cache_bytes
            );
        }
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
            debug!("image cache hit: uri={}", uri);
            return match entry {
                Entry::Ready { image, .. } => Ok(ImagePoll::Ready {
                    image: image.clone(),
                }),
                Entry::Error(err) => Err(LoadError::Loading(err.clone())),
                Entry::Pending => Ok(ImagePoll::Pending { size: None }),
            };
        }
        debug!("image cache miss: uri={}", uri);

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                if let Some(mime) = mime
                    && !Self::is_supported_mime(&mime)
                {
                    return Err(LoadError::FormatNotSupported {
                        detected_format: Some(mime),
                    });
                }

                cache_lock.put(uri.to_string(), Entry::Pending);
                drop(cache_lock);

                let cache = self.cache.clone();
                let total_decoded_bytes = self.total_decoded_bytes.clone();
                let uri = uri.to_string();
                let ctx = ctx.clone();
                let max_cache_bytes = self.max_cache_bytes;

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
                                debug!(
                                    "image decoded: uri={} size={}x{} bytes={} total={} max={}",
                                    uri, w, h, bytes_h, total_h, format_bytes(max_cache_bytes)
                                );
                                cache.put(
                                    uri.clone(),
                                    Entry::Ready {
                                        image,
                                        byte_size,
                                    },
                                );
                                Self::evict_if_needed(
                                    &mut cache,
                                    &total_decoded_bytes,
                                    max_cache_bytes,
                                );
                            }
                            Err(err) => {
                                debug!("image decode failed: uri={} err={}", uri, err);
                                cache.put(uri.clone(), Entry::Error(err));
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
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.pop(uri) {
            let bytes = Self::entry_bytes(&entry);
            if bytes > 0 {
                self.total_decoded_bytes
                    .fetch_sub(bytes, Ordering::Relaxed);
            }
            debug!("image cache forget: uri={} bytes={}", uri, bytes);
        }
    }

    fn forget_all(&self) {
        let mut cache = self.cache.lock();
        let mut total_removed = 0u64;
        for (_, entry) in cache.iter() {
            total_removed = total_removed.saturating_add(Self::entry_bytes(entry));
        }
        cache.clear();
        if total_removed > 0 {
            self.total_decoded_bytes
                .fetch_sub(total_removed, Ordering::Relaxed);
        }
        debug!("image cache cleared: bytes={}", total_removed);
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .iter()
            .map(|(_, entry)| match entry {
                Entry::Ready { byte_size, .. } => *byte_size as usize,
                Entry::Error(err) => err.len(),
                Entry::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.cache
            .lock()
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Pending))
    }
}

pub fn install_tracking_image_loader(ctx: &Context) {
    if ctx.is_loader_installed(TrackingImageLoader::ID) {
        return;
    }
    ctx.add_image_loader(Arc::new(TrackingImageLoader::new()));
    debug!("installed TrackingImageLoader (lru)");
}