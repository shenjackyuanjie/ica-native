#[cfg(test)]
#[path = "tests/gif.rs"]
mod tests;

use super::{
    util::format_bytes,
    worker::{DecodeWorkerPool, ScheduleError},
};
use egui::{
    ColorImage, FrameDurations, Id, decode_animated_image_uri, has_gif_magic_header,
    load::{
        Bytes, BytesLoader as _, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError,
        SizeHint,
    },
    mutex::Mutex,
};
use image::AnimationDecoder as _;
use lru::LruCache;
use std::{
    collections::HashMap,
    io::Cursor,
    mem::size_of,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, warn};

const ACTIVE_GIF_GRACE: Duration = Duration::from_millis(750);
const DEFAULT_GIF_CACHE_CAP: u64 = 64 * 1024 * 1024;
const MIN_GIF_CACHE_CAP: u64 = 16 * 1024 * 1024;
const MAX_GIF_FRAMES: usize = 180;
const MAX_GIF_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GIF_ERROR_ENTRIES: usize = 128;

#[derive(Debug)]
struct AnimatedImage {
    frames: Vec<Arc<ColorImage>>,
    frame_durations: FrameDurations,
    byte_size: u64,
}

impl AnimatedImage {
    fn load_gif(data: &[u8]) -> Result<Self, String> {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(data))
            .map_err(|err| format!("GIF 解码失败: {err}"))?;

        let mut frames = Vec::new();
        let mut durations = Vec::new();
        let mut byte_size = size_of::<Self>() as u64;

        for frame in decoder.into_frames() {
            let frame = frame.map_err(|err| format!("GIF 解码失败: {err}"))?;
            let img = frame.buffer();
            let pixels = img.as_flat_samples();
            let image = ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                pixels.as_slice(),
            );
            let frame_bytes = (image.pixels.len() * size_of::<egui::Color32>()) as u64;
            if frames.len() >= MAX_GIF_FRAMES
                || byte_size.saturating_add(frame_bytes) > MAX_GIF_DECODED_BYTES
            {
                if frames.is_empty() {
                    byte_size = byte_size
                        .saturating_add(frame_bytes)
                        .saturating_add(size_of::<Duration>() as u64);
                    frames.push(Arc::new(image));
                    durations.push(frame.delay().into());
                }
                break;
            }
            byte_size = byte_size
                .saturating_add(frame_bytes)
                .saturating_add(size_of::<Duration>() as u64);
            frames.push(Arc::new(image));
            durations.push(frame.delay().into());
        }

        if frames.is_empty() {
            return Err("GIF has no frames".to_string());
        }

        Ok(Self {
            frames,
            frame_durations: FrameDurations::new(durations),
            byte_size,
        })
    }

    fn get_image(&self, index: usize) -> Arc<ColorImage> {
        Arc::clone(&self.frames[index % self.frames.len()])
    }
}

struct GifEntry {
    image: Arc<AnimatedImage>,
    last_used: Instant,
    release_source_bytes: bool,
}

struct GifState {
    max_bytes: u64,
    total_bytes: u64,
    ready: LruCache<String, GifEntry>,
    pending: HashMap<String, u64>,
    errors: LruCache<String, Arc<str>>,
    next_generation: u64,
}

impl GifState {
    fn new(image_cache_max_bytes: u64) -> Self {
        Self {
            max_bytes: gif_cache_limit(image_cache_max_bytes),
            total_bytes: 0,
            ready: LruCache::unbounded(),
            pending: HashMap::new(),
            errors: LruCache::new(
                NonZeroUsize::new(MAX_GIF_ERROR_ENTRIES).expect("GIF 错误缓存容量必须大于零"),
            ),
            next_generation: 1,
        }
    }

    fn begin_decode(&mut self, uri: &str) -> Option<u64> {
        if self.ready.contains(uri) || self.errors.contains(uri) || self.pending.contains_key(uri) {
            return None;
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.pending.insert(uri.to_owned(), generation);
        Some(generation)
    }

    fn is_pending_generation(&self, uri: &str, generation: u64) -> bool {
        self.pending.get(uri) == Some(&generation)
    }

    fn cancel_pending(&mut self, uri: &str, generation: u64) {
        if self.is_pending_generation(uri, generation) {
            self.pending.remove(uri);
        }
    }

    fn complete_decode(
        &mut self,
        uri: &str,
        generation: u64,
        result: Result<AnimatedImage, String>,
    ) -> bool {
        if !self.is_pending_generation(uri, generation) {
            return false;
        }
        self.pending.remove(uri);
        match result {
            Ok(image) => self.insert_ready(uri.to_owned(), Arc::new(image), Instant::now()),
            Err(err) => {
                self.errors.put(uri.to_owned(), Arc::from(err));
            }
        }
        true
    }

    fn insert_ready(&mut self, uri: String, image: Arc<AnimatedImage>, now: Instant) {
        if let Some(old) = self.ready.put(
            uri,
            GifEntry {
                image: Arc::clone(&image),
                last_used: now,
                release_source_bytes: true,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(old.image.byte_size);
        }
        self.total_bytes = self.total_bytes.saturating_add(image.byte_size);
        self.evict_if_needed(now);
    }

    fn evict_if_needed(&mut self, now: Instant) {
        while self.ready.len() > 1 && (self.max_bytes == 0 || self.total_bytes > self.max_bytes) {
            let Some((uri, entry)) = self.ready.pop_lru() else {
                break;
            };

            if now.saturating_duration_since(entry.last_used) <= ACTIVE_GIF_GRACE {
                self.ready.put(uri, entry);
                break;
            }

            self.total_bytes = self.total_bytes.saturating_sub(entry.image.byte_size);
            debug!(
                "淘汰 GIF 缓存: uri={} bytes={} total_after={} max={}",
                uri,
                format_bytes(entry.image.byte_size),
                format_bytes(self.total_bytes),
                format_bytes(self.max_bytes)
            );
        }
    }
}

fn gif_cache_limit(image_cache_max_bytes: u64) -> u64 {
    if image_cache_max_bytes == 0 {
        return 0;
    }

    (image_cache_max_bytes / 2)
        .clamp(MIN_GIF_CACHE_CAP, DEFAULT_GIF_CACHE_CAP)
        .min(image_cache_max_bytes)
}

pub struct BoundedGifLoader {
    state: Arc<Mutex<GifState>>,
    workers: DecodeWorkerPool,
}

impl BoundedGifLoader {
    pub const ID: &'static str = egui::generate_loader_id!(BoundedGifLoader);

    pub fn new(image_cache_max_bytes: u64) -> Self {
        let state = GifState::new(image_cache_max_bytes);
        debug!(
            "BoundedGifLoader configured: memory_limit={}",
            format_bytes(state.max_bytes)
        );
        Self {
            state: Arc::new(Mutex::new(state)),
            // 单个 GIF 可解码到 64 MiB；串行解码、有界排队，避免放大像素内存峰值。
            workers: DecodeWorkerPool::new(1, "gif-decode"),
        }
    }

    fn schedule_decode(
        &self,
        ctx: &egui::Context,
        uri: &str,
        bytes: Bytes,
        decode: impl FnOnce(&[u8]) -> Result<AnimatedImage, String> + Send + 'static,
    ) -> Result<(), LoadError> {
        let Some(generation) = self.state.lock().begin_decode(uri) else {
            return Ok(());
        };
        let state = Arc::clone(&self.state);
        let ctx_for_job = ctx.clone();
        let uri_for_job = uri.to_owned();
        let scheduled = self.workers.schedule(move || {
            if !state.lock().is_pending_generation(&uri_for_job, generation) {
                return;
            }
            super::raw::remember(&ctx_for_job, &uri_for_job, bytes.as_ref());
            let result = decode(&bytes);
            let error = result.as_ref().err().cloned();
            let byte_size = result.as_ref().ok().map(|image| image.byte_size);
            if !state
                .lock()
                .complete_decode(&uri_for_job, generation, result)
            {
                return;
            }
            if let Some(err) = error {
                warn!("GIF 解码失败: uri={} err={}", uri_for_job, err);
            } else if let Some(byte_size) = byte_size {
                debug!(
                    "gif decoded: uri={} bytes={}",
                    uri_for_job,
                    format_bytes(byte_size)
                );
            }
            ctx_for_job.request_repaint();
        });
        match scheduled {
            Ok(()) => Ok(()),
            Err(ScheduleError::Full) => {
                self.state.lock().cancel_pending(uri, generation);
                // 繁忙不是解码失败，下一帧继续排队，不缓存成永久错误。
                ctx.request_repaint_after(Duration::from_millis(50));
                Ok(())
            }
            Err(ScheduleError::Closed) => {
                let message = "GIF 解码线程池已关闭".to_owned();
                self.state
                    .lock()
                    .complete_decode(uri, generation, Err(message.clone()));
                Err(LoadError::Loading(message))
            }
        }
    }
}

impl ImageLoader for BoundedGifLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, frame_uri: &str, _: SizeHint) -> ImageLoadResult {
        let (image_uri, frame_index) =
            decode_animated_image_uri(frame_uri).map_err(|_| LoadError::NotSupported)?;
        let now = Instant::now();

        let ready = {
            let mut state = self.state.lock();
            if let Some(entry) = state.ready.get_mut(image_uri) {
                entry.last_used = now;
                let image = Arc::clone(&entry.image);
                let release_source_bytes = std::mem::take(&mut entry.release_source_bytes);
                state.evict_if_needed(now);
                Some((image, release_source_bytes))
            } else {
                state.evict_if_needed(now);
                if let Some(err) = state.errors.get(image_uri) {
                    return Err(LoadError::Loading(err.to_string()));
                }
                if state.pending.contains_key(image_uri) {
                    return Ok(ImagePoll::Pending { size: None });
                }
                None
            }
        };
        if let Some((image, release_source_bytes)) = ready {
            ctx.data_mut(|data| {
                *data.get_temp_mut_or_default(Id::new(image_uri)) = image.frame_durations.clone();
            });
            if release_source_bytes {
                // 上层预览先探测原始 bytes，再请求帧；首次 Ready 被消费前不能释放源。
                let loaders = ctx.loaders();
                loaders.include.forget(image_uri);
                for loader in loaders.bytes.lock().iter().rev() {
                    loader.forget(image_uri);
                }
            }
            return Ok(ImagePoll::Ready {
                image: image.get_image(frame_index),
            });
        }

        match ctx.try_load_bytes(image_uri) {
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Ok(BytesPoll::Ready { bytes, size, .. }) => {
                if !has_gif_magic_header(&bytes) {
                    return Err(LoadError::FormatNotSupported {
                        detected_format: None,
                    });
                }
                self.schedule_decode(ctx, image_uri, bytes, AnimatedImage::load_gif)?;
                Ok(ImagePoll::Pending { size })
            }
            Err(err) => Err(err),
        }
    }

    fn forget(&self, uri: &str) {
        if decode_animated_image_uri(uri).is_ok() {
            return;
        }

        let mut state = self.state.lock();
        state.pending.remove(uri);
        state.errors.pop(uri);
        if let Some(entry) = state.ready.pop(uri) {
            state.total_bytes = state.total_bytes.saturating_sub(entry.image.byte_size);
        }
    }

    fn forget_all(&self) {
        let mut state = self.state.lock();
        state.ready.clear();
        state.pending.clear();
        state.errors.clear();
        state.total_bytes = 0;
    }

    fn byte_size(&self) -> usize {
        let state = self.state.lock();
        let error_bytes = state
            .errors
            .iter()
            .map(|(_, err)| err.len() as u64)
            .sum::<u64>();
        state
            .total_bytes
            .saturating_add(error_bytes)
            .min(usize::MAX as u64) as usize
    }

    fn has_pending(&self) -> bool {
        !self.state.lock().pending.is_empty()
    }
}

pub fn install(ctx: &egui::Context, image_cache_max_bytes: u64) {
    if !ctx.is_loader_installed(BoundedGifLoader::ID) {
        ctx.add_image_loader(Arc::new(BoundedGifLoader::new(image_cache_max_bytes)));
    }
}
