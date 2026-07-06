//! 自定义图片加载器：
//! - 只处理静态位图；
//! - 维持一个按已解码像素字节计数的内存 LRU；
//! - 可选地把原始图片字节落到磁盘缓存；
//! - 通过固定大小的后台 worker 池解码，避免“一张图一个线程”。
//!
//! 这次实现和旧版本最大的差别有三点：
//! 1. **状态统一**：pending / ready / error / 统计值 全部由同一个状态机维护；
//! 2. **generation 防回写**：`forget/forget_all` 后，旧后台任务的结果不会再“复活”；
//! 3. **错误缓存有界**：不会再因为坏 URL / 过期 URL 无限堆积失败条目。

mod decode;
mod disk;
mod state;
mod texture;
mod util;
mod worker;

use std::{sync::Arc, time::Instant};

use decode::{
    decode_image, decoded_image_byte_size, is_permanent_download_error, normalize_image_load_error,
    normalize_uri, should_handle_file_uri, should_handle_loaded_bytes,
};
use disk::DiskCache;
use egui::{
    Context,
    load::{Bytes, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
};
use state::{LoaderState, PrepareLoad};
use tracing::{debug, info, warn};
use util::{decode_worker_count, format_bytes};
use worker::{DecodeWorkerPool, ScheduleError};

use crate::cfg;

/// 带内存统计的图片加载器。
#[derive(Clone)]
pub struct TrackingImageLoader {
    state: Arc<Mutex<LoaderState>>,
    disk_cache: Option<Arc<DiskCache>>,
    workers: Arc<DecodeWorkerPool>,
}

impl TrackingImageLoader {
    /// 图片加载器在 `egui` 中的唯一 ID。
    pub const ID: &'static str = egui::generate_loader_id!(TrackingImageLoader);

    /// 创建一个新的加载器实例。
    pub fn new() -> Self {
        let cfg = cfg::get_cfg_snapshot();
        let worker_count = decode_worker_count();

        let disk_cache = DiskCache::new(cfg.get_image_cache_path(), cfg.disk_image_cache_max_bytes)
            .map(Arc::new);

        info!(
            "TrackingImageLoader configured: workers={} memory_limit={} disk_limit={}",
            worker_count,
            format_bytes(cfg.image_cache_max_bytes),
            if cfg.disk_image_cache_max_bytes == 0 {
                "unlimited".to_string()
            } else {
                format_bytes(cfg.disk_image_cache_max_bytes)
            }
        );

        Self {
            state: Arc::new(Mutex::new(LoaderState::new(cfg.image_cache_max_bytes))),
            disk_cache,
            workers: Arc::new(DecodeWorkerPool::new(worker_count, "image-decode")),
        }
    }

    /// 把一份待解码字节提交给固定 worker 池。
    ///
    /// 这里最重要的是 `generation`：
    /// worker 完成后只有在“这还是当前那一代请求”时才允许写回状态，
    /// 否则直接丢弃，防止旧结果覆盖新结果。
    fn schedule_decode(
        &self,
        ctx: &Context,
        uri: String,
        generation: u64,
        bytes: Bytes,
        size_hint: Option<egui::Vec2>,
        loaded_from_disk: bool,
    ) -> Result<(), LoadError> {
        {
            let mut state = self.state.lock();
            if !state.mark_decoding(&uri, generation, size_hint) {
                debug!(
                    "skip decode schedule: stale or already-decoding uri={} generation={}",
                    uri, generation
                );
                return Ok(());
            }
        }

        let state = Arc::clone(&self.state);
        let disk_cache = self.disk_cache.clone();
        let ctx = ctx.clone();
        let uri_for_submit_error = uri.clone();

        self.workers
            .schedule(move || {
                let should_decode = {
                    let state = state.lock();
                    state.is_pending_generation(&uri, generation)
                };
                if !should_decode {
                    debug!(
                        "skip stale decode before work starts: uri={} generation={}",
                        uri, generation
                    );
                    return;
                }

                match decode_image(&bytes) {
                    Ok(image) => {
                        let decoded_bytes = decoded_image_byte_size(&image);
                        let [width, height] = image.size;

                        let (committed, total_ready_bytes) = {
                            let mut state = state.lock();
                            let committed = state.complete_ready(
                                &uri,
                                generation,
                                image,
                                decoded_bytes,
                                Instant::now(),
                            );
                            let total_ready_bytes = state.ready_byte_size();
                            (committed, total_ready_bytes)
                        };

                        if !committed {
                            debug!(
                                "discard stale decode result: uri={} generation={}",
                                uri, generation
                            );
                            return;
                        }

                        debug!(
                            "image decoded: uri={} size={}x{} bytes={} ready_total={}",
                            uri,
                            width,
                            height,
                            format_bytes(decoded_bytes),
                            format_bytes(total_ready_bytes)
                        );
                        ctx.request_repaint();

                        if !loaded_from_disk && let Some(disk_cache) = disk_cache.as_ref() {
                            disk_cache.save(&uri, bytes.as_ref());
                        }
                    }
                    Err(err) => {
                        let committed = {
                            let mut state = state.lock();
                            state.complete_error(&uri, generation, err.clone())
                        };

                        if !committed {
                            debug!(
                                "discard stale decode error: uri={} generation={}",
                                uri, generation
                            );
                            return;
                        }

                        warn!("image decode failed: uri={} err={}", uri, err);

                        // 磁盘命中的文件如果已经坏了，就顺手删掉，避免每次启动都反复踩雷。
                        if loaded_from_disk && let Some(disk_cache) = disk_cache.as_ref() {
                            disk_cache.remove(&uri);
                        }

                        ctx.request_repaint();
                    }
                }
            })
            .map_err(|err| {
                let mut state = self.state.lock();
                match err {
                    ScheduleError::Full => {
                        let _ = state.cancel_pending(&uri_for_submit_error, generation);
                        LoadError::Loading("图片解码队列繁忙，稍后重试".to_string())
                    }
                    ScheduleError::Closed => {
                        let message = "图片解码线程池已关闭".to_string();
                        let _ = state.complete_error(
                            &uri_for_submit_error,
                            generation,
                            message.clone(),
                        );
                        LoadError::Loading(message)
                    }
                }
            })
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
        let uri = normalize_uri(uri);

        // 对 file:// 先做一轮 cheap filter，避免 SVG/GIF/WEBP 被我们错误截走。
        if uri.starts_with("file://") && !should_handle_file_uri(uri) {
            return Err(LoadError::NotSupported);
        }

        let generation = match self.state.lock().prepare_load(uri, Instant::now()) {
            PrepareLoad::Ready(image) => {
                debug!("image cache hit: uri={}", uri);
                return Ok(ImagePoll::Ready { image });
            }
            PrepareLoad::Decoding { size } => {
                return Ok(ImagePoll::Pending { size });
            }
            PrepareLoad::Error(message) => {
                return Err(LoadError::Loading(message.to_string()));
            }
            PrepareLoad::WaitingBytes { generation, .. } => generation,
        };

        debug!("image cache miss or bytes-pending: uri={}", uri);

        // 先尝试磁盘缓存，减少网络请求和底层 bytes loader 压力。
        if let Some(disk_cache) = self.disk_cache.as_ref()
            && let Some(raw_bytes) = disk_cache.load(uri)
        {
            debug!(
                "disk cache hit: uri={} raw_bytes={}",
                uri,
                format_bytes(raw_bytes.len() as u64)
            );

            let bytes: Bytes = raw_bytes.into();
            if !should_handle_loaded_bytes(None, &bytes) {
                warn!("drop unsupported disk cache entry: uri={}", uri);
                disk_cache.remove(uri);
                let _ = self.state.lock().cancel_pending(uri, generation);
                return Err(LoadError::NotSupported);
            }

            self.schedule_decode(ctx, uri.to_owned(), generation, bytes, None, true)?;
            return Ok(ImagePoll::Pending { size: None });
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Pending { size }) => {
                self.state.lock().update_waiting_size(uri, generation, size);
                Ok(ImagePoll::Pending { size })
            }
            Ok(BytesPoll::Ready { bytes, mime, size }) => {
                if !should_handle_loaded_bytes(mime.as_deref(), &bytes) {
                    // 注意：这里一定要清掉 pending。
                    // 否则 URI 会永远卡在“我们已经接管过它”的状态里，
                    // 正是旧实现里最典型的状态泄漏之一。
                    let _ = self.state.lock().cancel_pending(uri, generation);
                    return Err(LoadError::NotSupported);
                }

                self.schedule_decode(ctx, uri.to_owned(), generation, bytes, size, false)?;
                Ok(ImagePoll::Pending { size })
            }
            Err(err) => {
                let normalized = normalize_image_load_error(uri, err);
                let mut state = self.state.lock();
                match &normalized {
                    // 只缓存明确不可恢复的下载错误（如 URL 已过期）；
                    // 瞬时网络错误只取消 pending，让后续帧仍有机会重新走底层加载流程。
                    LoadError::Loading(message) if is_permanent_download_error(message) => {
                        let _ = state.complete_error(uri, generation, message.clone());
                    }
                    _ => {
                        let _ = state.cancel_pending(uri, generation);
                    }
                }
                Err(normalized)
            }
        }
    }

    fn forget(&self, uri: &str) {
        let uri = normalize_uri(uri);
        // 这里只清理内存态；磁盘缓存保留为跨会话优化。
        self.state.lock().forget(uri);
        debug!("image cache forget: uri={}", uri);
    }

    fn forget_all(&self) {
        // 同上：forget_all 只清内存态，不主动清空磁盘缓存。
        self.state.lock().forget_all();
        debug!("image cache cleared");
    }

    fn byte_size(&self) -> usize {
        self.state.lock().byte_size()
    }

    fn has_pending(&self) -> bool {
        self.state.lock().has_pending()
    }
}

/// 在 `egui::Context` 上安装该图片加载器（若尚未安装）。
pub fn install_tracking_image_loader(ctx: &Context) {
    if ctx.is_loader_installed(TrackingImageLoader::ID) {
        return;
    }

    let cfg = cfg::get_cfg_snapshot();
    ctx.add_image_loader(Arc::new(TrackingImageLoader::new()));
    texture::install(ctx, cfg.image_cache_max_bytes);
    info!("installed TrackingImageLoader (state-machine + worker-pool)");
}
