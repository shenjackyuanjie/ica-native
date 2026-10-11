//! 有预算的 GIF（动图）加载器。
//!
//! 位于 `crate::image_loader` 之下，专门接管 egui 内部用 `...#frame=...` 表示的动图帧：
//! 把 GIF 解成一组帧与帧时长，按 LRU + 字节预算缓存，并在单线程里串行解码，
//! 避免放大像素内存峰值。SVG 与静态位图不在此处理。

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

/// 动图缓存项的“活跃宽限”：最近用过的条目即使超预算也暂不淘汰，避免可见动图被反复解码。
const ACTIVE_GIF_GRACE: Duration = Duration::from_millis(750);
/// 单个 GIF 解码缓存的默认字节上限（64 MiB）。
const DEFAULT_GIF_CACHE_CAP: u64 = 64 * 1024 * 1024;
/// GIF 解码缓存的下限（16 MiB），防止配置给得过小导致动图几乎无法缓存。
const MIN_GIF_CACHE_CAP: u64 = 16 * 1024 * 1024;
/// 单个 GIF 最多解码的帧数，防止异常长图无限解码。
const MAX_GIF_FRAMES: usize = 180;
/// 单个 GIF 解码后的字节上限（64 MiB）。
const MAX_GIF_DECODED_BYTES: u64 = 64 * 1024 * 1024;
/// GIF 错误缓存的最大条目数，避免坏图无限堆积。
const MAX_GIF_ERROR_ENTRIES: usize = 128;

/// 一个已解码的动图：全部帧 + 帧时长 + 估算字节。
#[derive(Debug)]
struct AnimatedImage {
    /// 所有帧的像素图。
    frames: Vec<Arc<ColorImage>>,
    /// 与帧一一对应的时长序列。
    frame_durations: FrameDurations,
    /// 估算占用字节（帧像素 + 时长）。
    byte_size: u64,
}

/// `AnimatedImage` 的解码与取帧。
impl AnimatedImage {
    /// 从 GIF 字节解码出所有帧；受 `MAX_GIF_FRAMES` / `MAX_GIF_DECODED_BYTES` 约束，
    /// 超限时至少保留首帧以保证有可显示内容。
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

    /// 按帧索引取帧（越界时取模循环），供 egui 请求某一帧时使用。
    fn get_image(&self, index: usize) -> Arc<ColorImage> {
        Arc::clone(&self.frames[index % self.frames.len()])
    }
}

/// 动图缓存中的一个就绪条目。
struct GifEntry {
    /// 解码后的动图。
    image: Arc<AnimatedImage>,
    /// 最近一次被使用的时间，用于 LRU 与活跃宽限判断。
    last_used: Instant,
    /// 首次被消费后是否释放源字节（原始 GIF / bytes loader 缓存）。
    release_source_bytes: bool,
}

/// 动图加载器的内存态：就绪 LRU + pending 代际 + 有界错误缓存。
struct GifState {
    /// 字节预算；`0` 表示不限制。
    max_bytes: u64,
    /// 当前就绪条目占用的总字节。
    total_bytes: u64,
    /// 已解码动图的 LRU。
    ready: LruCache<String, GifEntry>,
    /// 正在解码的 URI -> 代际编号。
    pending: HashMap<String, u64>,
    /// 解码错误的有界 LRU。
    errors: LruCache<String, Arc<str>>,
    /// 下一个代际编号。
    next_generation: u64,
}

/// `GifState` 的状态转移与淘汰。
impl GifState {
    /// 用给定预算创建动图状态；错误缓存为有界 LRU。
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

    /// 为一个新 URI 开始解码并分配代际编号；已在就绪/错误/pending 中则返回 `None`。
    fn begin_decode(&mut self, uri: &str) -> Option<u64> {
        if self.ready.contains(uri) || self.errors.contains(uri) || self.pending.contains_key(uri) {
            return None;
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.pending.insert(uri.to_owned(), generation);
        Some(generation)
    }

    /// 判断某 URI 的 pending 代际是否仍匹配（用于丢弃过期任务）。
    fn is_pending_generation(&self, uri: &str, generation: u64) -> bool {
        self.pending.get(uri) == Some(&generation)
    }

    /// 取消匹配代际的 pending 条目。
    fn cancel_pending(&mut self, uri: &str, generation: u64) {
        if self.is_pending_generation(uri, generation) {
            self.pending.remove(uri);
        }
    }

    /// 后台线程提交解码结果：代际匹配才写入就绪或错误缓存，返回是否被接受。
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

    /// 把解码好的动图插入就绪 LRU 并更新字节记账，随后按需淘汰。
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

    /// 按需淘汰就绪动图：超过预算时从 LRU 头部淘汰，但跳过仍在活跃宽限内的条目。
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

/// 由图片内存预算推导动图缓存预算：取一半并夹在上下限之间，且不超过原预算。
fn gif_cache_limit(image_cache_max_bytes: u64) -> u64 {
    if image_cache_max_bytes == 0 {
        return 0;
    }

    (image_cache_max_bytes / 2)
        .clamp(MIN_GIF_CACHE_CAP, DEFAULT_GIF_CACHE_CAP)
        .min(image_cache_max_bytes)
}

/// 有预算的 GIF 动图加载器（实现 egui `ImageLoader`）。
pub struct BoundedGifLoader {
    /// 动图内存态。
    state: Arc<Mutex<GifState>>,
    /// 单线程解码池：串行解码以压低像素内存峰值。
    workers: DecodeWorkerPool,
}

/// `BoundedGifLoader` 的构造与解码调度。
impl BoundedGifLoader {
    /// 本 loader 在 egui 中的唯一 ID。
    pub const ID: &'static str = egui::generate_loader_id!(BoundedGifLoader);

    /// 用给定图片内存预算创建动图加载器。
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

    /// 把一个动图解码任务提交给单线程池；代际不匹配或队列繁忙时安全跳过。
    ///
    /// `decode` 以闭包注入具体解码函数（当前为 `AnimatedImage::load_gif`），便于测试替换。
    /// 队列已满不视为失败，仅稍后重排；线程池关闭才返回错误。
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

/// 为 `BoundedGifLoader` 实现 egui `ImageLoader`。
impl ImageLoader for BoundedGifLoader {
    /// 返回本 loader 的唯一 ID。
    fn id(&self) -> &str {
        Self::ID
    }

    /// egui 请求某一动图帧时的入口。
    ///
    /// 先从 `frame_uri` 解出真实图片 URI 与帧索引；命中就绪缓存直接返回该帧，
    /// 否则经底层 bytes loader 取字节、确认 GIF 魔数后调度解码。
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

    /// 忘记某动图 URI 的就绪/错误/pending 态；帧 URI（带 `#frame=`）不处理。
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

    /// 清空所有动图缓存态并重置字节记账。
    fn forget_all(&self) {
        let mut state = self.state.lock();
        state.ready.clear();
        state.pending.clear();
        state.errors.clear();
        state.total_bytes = 0;
    }

    /// 返回动图缓存占用字节（帧像素 + 错误字符串）。
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

    /// 是否还有正在解码的动图。
    fn has_pending(&self) -> bool {
        !self.state.lock().pending.is_empty()
    }
}

/// 在 `egui::Context` 上安装动图加载器（若尚未安装）。
pub fn install(ctx: &egui::Context, image_cache_max_bytes: u64) {
    if !ctx.is_loader_installed(BoundedGifLoader::ID) {
        ctx.add_image_loader(Arc::new(BoundedGifLoader::new(image_cache_max_bytes)));
    }
}
