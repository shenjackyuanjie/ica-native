//! 自定义图片加载器，带 LRU 缓存与解码统计。
//! 仅处理非动图/非 SVG/WEBP/GIF 的图片，并在后台线程解码。
//! 缓存大小由配置项 `image_cache_max_bytes` 控制。
//!
//! Powered by GPT-5.2 Codex (github copilot 啥时候上 GPT 5.3 Codex 啊啊啊啊)
//!
//! (起因是我让他写了个图片加载内存使用量追踪)
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    mem::size_of,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use egui::{
    ColorImage, Context, decode_animated_image_uri,
    load::{Bytes, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
};
use image::ImageFormat;
use lru::LruCache;
use tracing::{debug, info, warn};

use crate::cfg;

/// 缓存容器，使用 LRU 管理已解码的图片。
type Cache = Arc<Mutex<LruCache<String, Entry>>>;

/// 具有缓存与字节统计功能的自定义图片加载器。
#[derive(Clone)]
pub struct TrackingImageLoader {
    cache: Cache,
    total_decoded_bytes: Arc<AtomicU64>,
    max_cache_bytes: u64,
    /// 磁盘缓存目录，为 None 时禁用落盘。
    disk_cache_dir: Option<PathBuf>,
    /// 磁盘缓存最大字节数。
    disk_max_bytes: u64,
}

/// 缓存条目状态。
enum Entry {
    /// 等待解码完成。
    Pending,
    /// 已解码完成的图片。
    Ready {
        image: Arc<ColorImage>,
        byte_size: u64,
    },
    /// 解码错误信息。
    Error(String),
}

impl TrackingImageLoader {
    /// 图片加载器在 `egui` 中的唯一 ID。
    pub const ID: &'static str = egui::generate_loader_id!(TrackingImageLoader);

    /// 创建新的加载器实例，读取配置中的缓存上限。
    pub fn new() -> Self {
        let cfg = cfg::get_cfg_snapshot();
        let disk_cache_dir = {
            let path = cfg.get_image_cache_path();
            match std::fs::create_dir_all(&path) {
                Ok(()) => {
                    info!("disk image cache dir: {:?}", path);
                    Some(path)
                }
                Err(e) => {
                    warn!("无法创建磁盘缓存目录 {:?}: {}, 将禁用磁盘缓存", path, e);
                    None
                }
            }
        };
        Self {
            cache: Arc::new(Mutex::new(LruCache::unbounded())),
            total_decoded_bytes: Arc::new(AtomicU64::new(0)),
            max_cache_bytes: cfg.image_cache_max_bytes,
            disk_cache_dir,
            disk_max_bytes: cfg.disk_image_cache_max_bytes,
        }
    }

    /// 根据文件扩展名判断是否支持（排除 svg/gif/webp）。
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

    /// 根据 MIME 判断是否支持（排除 svg/gif/webp）。
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

    /// 在 MIME 缺失时，从内容判断是否为 SVG。
    fn is_svg_bytes(bytes: &Bytes) -> bool {
        let Ok(text) = std::str::from_utf8(bytes.as_ref()) else {
            return false;
        };
        let text = text.trim_start();
        text.starts_with("<svg") || text.starts_with("<?xml") && text.contains("<svg")
    }

    /// 解码图片字节为 `egui::ColorImage`。
    fn decode_image(bytes: &Bytes) -> Result<Arc<ColorImage>, String> {
        let img = image::load_from_memory(bytes.as_ref()).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let pixels = rgba.into_raw();
        let color = ColorImage::from_rgba_unmultiplied(size, &pixels);
        Ok(Arc::new(color))
    }

    /// 统计缓存条目占用的字节数（仅计入已解码图片）。
    fn entry_bytes(entry: &Entry) -> u64 {
        match entry {
            Entry::Ready { byte_size, .. } => *byte_size,
            Entry::Error(_) => 0,
            Entry::Pending => 0,
        }
    }

    /// 当总字节数超过上限时按 LRU 进行逐出。
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

    /// 判断 URI 是否为头像类 URL（qlogo.cn 域名）。
    fn is_avatar_uri(uri: &str) -> bool {
        uri.contains("qlogo.cn")
    }

    /// URI → 磁盘缓存文件路径（通过哈希映射）。
    /// 头像放入 `avatar/`，其他图片放入 `image/`。
    fn uri_to_cache_path(&self, uri: &str) -> Option<PathBuf> {
        let dir = self.disk_cache_dir.as_ref()?;
        let mut hasher = DefaultHasher::new();
        uri.hash(&mut hasher);
        let hash = hasher.finish();
        let subdir = if Self::is_avatar_uri(uri) {
            "avatar"
        } else {
            "image"
        };
        Some(dir.join(subdir).join(format!("{:016x}.img", hash)))
    }

    /// 尝试从磁盘缓存读取原始字节。
    fn try_load_from_disk(&self, uri: &str) -> Option<Vec<u8>> {
        let path = self.uri_to_cache_path(uri)?;
        std::fs::read(&path).ok()
    }

    /// 在后台线程解码并缓存图片。
    /// `disk_save_path` 为 Some 时，解码成功后将原始字节保存到磁盘。
    fn spawn_decode_and_cache(
        &self,
        uri: String,
        bytes: Bytes,
        ctx: Context,
        disk_save_path: Option<PathBuf>,
    ) {
        let cache = self.cache.clone();
        let total_decoded_bytes = self.total_decoded_bytes.clone();
        let max_cache_bytes = self.max_cache_bytes;
        let disk_cache_dir = self.disk_cache_dir.clone();
        let disk_max_bytes = self.disk_max_bytes;

        std::thread::Builder::new()
            .name(format!("TrackingImageLoader::load({uri:?})"))
            .spawn(move || {
                let result = Self::decode_image(&bytes);
                let mut cache = cache.lock();
                match result {
                    Ok(image) => {
                        // 写入磁盘缓存
                        if let Some(disk_path) = disk_save_path {
                            if let Some(parent) = disk_path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(e) = std::fs::write(&disk_path, bytes.as_ref()) {
                                warn!(
                                    "disk cache write failed: path={} err={}",
                                    disk_path.display(),
                                    e
                                );
                            } else {
                                debug!(
                                    "disk cache saved: uri={} path={} raw_bytes={}",
                                    uri,
                                    disk_path.display(),
                                    format_bytes(bytes.len() as u64)
                                );
                                // 磁盘淮汰
                                if let Some(ref dir) = disk_cache_dir {
                                    Self::evict_disk_if_needed(dir, disk_max_bytes);
                                }
                            }
                        }

                        let byte_size =
                            (image.pixels.len() * size_of::<egui::Color32>()) as u64;
                        let total = total_decoded_bytes
                            .fetch_add(byte_size, Ordering::Relaxed)
                            + byte_size;
                        let [w, h] = image.size;
                        info!(
                            "image decoded: uri={} size={}x{} bytes={} total={} max={}",
                            uri,
                            w,
                            h,
                            format_bytes(byte_size),
                            format_bytes(total),
                            format_bytes(max_cache_bytes)
                        );
                        cache.put(uri.clone(), Entry::Ready { image, byte_size });
                        Self::evict_if_needed(
                            &mut cache,
                            &total_decoded_bytes,
                            max_cache_bytes,
                        );
                    }
                    Err(err) => {
                        warn!("image decode failed: uri={} err={}", uri, err);
                        cache.put(uri.clone(), Entry::Error(err));
                    }
                }
                ctx.request_repaint();
            })
            .expect("failed to spawn image decode thread");
    }

    /// 当磁盘缓存总大小超过上限时，按修改时间从旧到新逐出。
    /// 优先清除普通图片，头像放后面。
    fn evict_disk_if_needed(dir: &PathBuf, max_bytes: u64) {
        if max_bytes == 0 {
            // 0 表示不限制
            return;
        }
        // 先收集 image/，再收集 avatar/，这样排序后同时间的图片排在头像前面
        let subdirs = [dir.join("image"), dir.join("avatar")];
        let mut image_files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut avatar_files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut total: u64 = 0;
        for (idx, subdir) in subdirs.iter().enumerate() {
            let entries = match std::fs::read_dir(subdir) {
                Ok(e) => e,
                Err(_) => continue, // 子目录可能不存在
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    let size = meta.len();
                    let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    total += size;
                    if idx == 0 {
                        image_files.push((path, size, modified));
                    } else {
                        avatar_files.push((path, size, modified));
                    }
                }
            }
        }
        if total <= max_bytes {
            return;
        }
        // 普通图片按时间升序排在前面，头像按时间升序排在后面
        image_files.sort_by_key(|(_, _, t)| *t);
        avatar_files.sort_by_key(|(_, _, t)| *t);
        let all_files = image_files.iter().chain(avatar_files.iter());
        for (path, size, _) in all_files {
            if total <= max_bytes {
                break;
            }
            if let Err(e) = std::fs::remove_file(path) {
                warn!("disk cache evict: remove failed: {} err={}", path.display(), e);
            } else {
                debug!(
                    "disk cache evict: removed {} size={} total_after={}",
                    path.display(),
                    format_bytes(*size),
                    format_bytes(total.saturating_sub(*size))
                );
                total = total.saturating_sub(*size);
            }
        }
    }
}

/// 将字节数格式化为可读字符串。
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

fn normalize_image_load_error(uri: &str, err: LoadError) -> LoadError {
    let err_text = err.to_string();
    let lower_text = err_text.to_ascii_lowercase();
    if lower_text.contains("download url has expired")
        || err_text.contains("retcode\":-5503007")
        || err_text.contains("retcode=-5503007")
    {
        warn!("image url expired: uri={} err={}", uri, err_text);
        LoadError::Loading("图片链接已过期，需要重新获取 URL".to_string())
    } else {
        LoadError::Loading(err_text)
    }
}

impl Default for TrackingImageLoader {
    /// 等同于 `TrackingImageLoader::new`。
    fn default() -> Self {
        Self::new()
    }
}

impl ImageLoader for TrackingImageLoader {
    /// 返回加载器 ID。
    fn id(&self) -> &str {
        Self::ID
    }

    /// 加载图片（必要时后台解码），并返回 `egui` 轮询结果。
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
        info!("image cache miss: uri={}", uri);

        // 1. 尝试磁盘缓存
        if let Some(disk_bytes) = self.try_load_from_disk(uri) {
            info!(
                "disk cache hit: uri={} raw_bytes={}",
                uri,
                format_bytes(disk_bytes.len() as u64)
            );
            let bytes: Bytes = Bytes::Shared(Arc::from(disk_bytes));
            if Self::is_svg_bytes(&bytes) {
                return Err(LoadError::NotSupported);
            }
            cache_lock.put(uri.to_string(), Entry::Pending);
            drop(cache_lock);
            // 从磁盘加载，不需要再写回磁盘
            self.spawn_decode_and_cache(uri.to_string(), bytes, ctx.clone(), None);
            return Ok(ImagePoll::Pending { size: None });
        }

        // 2. 网络加载
        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, mime, .. }) => {
                if let Some(ref mime) = mime
                    && !Self::is_supported_mime(mime)
                {
                    return Err(LoadError::NotSupported);
                }
                if mime.is_none() && Self::is_svg_bytes(&bytes) {
                    return Err(LoadError::NotSupported);
                }

                cache_lock.put(uri.to_string(), Entry::Pending);
                drop(cache_lock);

                // 解码成功后写入磁盘缓存
                let disk_save_path = self.uri_to_cache_path(uri);
                self.spawn_decode_and_cache(
                    uri.to_string(),
                    bytes,
                    ctx.clone(),
                    disk_save_path,
                );

                Ok(ImagePoll::Pending { size: None })
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(err) => {
                warn!("image byte load failed: uri={} err={}", uri, err);
                let normalized = normalize_image_load_error(uri, err);
                if let LoadError::Loading(message) = &normalized {
                    cache_lock.put(uri.to_string(), Entry::Error(message.clone()));
                }
                Err(normalized)
            }
        }
    }

    /// 忘记指定 URI 的缓存条目。
    fn forget(&self, uri: &str) {
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.pop(uri) {
            let bytes = Self::entry_bytes(&entry);
            if bytes > 0 {
                self.total_decoded_bytes.fetch_sub(bytes, Ordering::Relaxed);
            }
            debug!("image cache forget: uri={} bytes={}", uri, bytes);
        }
    }

    /// 清空所有缓存条目。
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

    /// 返回当前缓存占用的字节数（仅计入已解码图片）。
    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .iter()
            .map(|(_, entry)| match entry {
                Entry::Ready { byte_size, .. } => *byte_size as usize,
                Entry::Error(_) => 0,
                Entry::Pending => 0,
            })
            .sum()
    }

    /// 是否存在正在解码的条目。
    fn has_pending(&self) -> bool {
        self.cache
            .lock()
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Pending))
    }
}

/// 在 `egui::Context` 上安装该图片加载器（若尚未安装）。
pub fn install_tracking_image_loader(ctx: &Context) {
    if ctx.is_loader_installed(TrackingImageLoader::ID) {
        return;
    }
    ctx.add_image_loader(Arc::new(TrackingImageLoader::new()));
    info!("installed TrackingImageLoader (lru)");
}
