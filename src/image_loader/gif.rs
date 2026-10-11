use std::{
    collections::HashMap,
    io::Cursor,
    mem::size_of,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
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
use tracing::{debug, warn};

use super::{
    util::format_bytes,
    worker::{DecodeWorkerPool, ScheduleError},
};

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

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    use super::*;

    const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

    // 只用内存中的两个不同帧，不访问配置、文件、网络或真实会话。
    fn gif_bytes_with_frames(count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames((0..count).map(|index| {
                let color = if index % 2 == 0 {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 255, 255])
                };
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(2, 1, color),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(if index % 2 == 0 { 40 } else { 70 }, 1),
                )
            }))
            .unwrap();
        bytes
    }

    fn gif_bytes() -> Bytes {
        gif_bytes_with_frames(2).into()
    }

    // 即使断言失败，也释放测试阻塞器，避免遗留永久占用 worker 的线程。
    struct ReleaseOnDrop(mpsc::Sender<()>);

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    fn block_worker(loader: &BoundedGifLoader) -> ReleaseOnDrop {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = ReleaseOnDrop(release_tx);
        loader
            .workers
            .schedule(move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
            })
            .unwrap();
        entered_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
        release
    }

    fn drain_worker(loader: &BoundedGifLoader) {
        let (done_tx, done_rx) = mpsc::channel();
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let done_tx = done_tx.clone();
            match loader.workers.schedule(move || {
                let _ = done_tx.send(());
            }) {
                Ok(()) => break,
                Err(ScheduleError::Full) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) => panic!("GIF 测试任务未能排队: {err:?}"),
            }
        }
        done_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
    }

    fn ready_frame(
        loader: &BoundedGifLoader,
        ctx: &egui::Context,
        uri: &str,
        index: usize,
    ) -> Arc<ColorImage> {
        match loader.load(ctx, &format!("{uri}#{index}"), SizeHint::default()) {
            Ok(ImagePoll::Ready { image }) => image,
            Ok(ImagePoll::Pending { .. }) => panic!("GIF 应当已就绪，却仍在等待"),
            Err(err) => panic!("GIF 应当已就绪，却加载失败: {err}"),
        }
    }

    #[test]
    fn cold_poll_does_not_wait_for_decode_and_completion_wakes_ui() {
        let loader = BoundedGifLoader::new(1024);
        let ctx = egui::Context::default();
        let uri = "bytes://gif_async_poll.gif";
        ctx.include_bytes(uri, gif_bytes());
        // 清除 Context 首帧自身的 repaint，后面只观察解码完成的唤醒。
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |_| {});
        }
        let (repaint_tx, repaint_rx) = mpsc::channel();
        ctx.set_request_repaint_callback(move |_| {
            let _ = repaint_tx.send(());
        });
        let release = block_worker(&loader);
        for index in 0..4 {
            assert!(matches!(
                loader.load(&ctx, &format!("{uri}#{index}"), SizeHint::default()),
                Ok(ImagePoll::Pending { .. })
            ));
        }
        assert!(loader.has_pending());
        assert_eq!(loader.state.lock().pending.len(), 1);
        assert!(matches!(
            ctx.try_load_bytes(uri),
            Ok(BytesPoll::Ready { .. })
        ));
        // 纹理加载器只释放某一帧时，不得取消整张动画的待解码任务。
        loader.forget(&format!("{uri}#0"));
        assert!(loader.has_pending());
        assert!(matches!(
            repaint_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(release);
        repaint_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
        assert!(!loader.has_pending());
        assert_eq!(
            ready_frame(&loader, &ctx, uri, 0).pixels[0],
            egui::Color32::RED
        );
        assert_eq!(
            ready_frame(&loader, &ctx, uri, 1).pixels[0],
            egui::Color32::BLUE
        );
        assert_eq!(
            ready_frame(&loader, &ctx, uri, 3).pixels[0],
            egui::Color32::BLUE
        );
        let durations = ctx
            .data(|data| data.get_temp::<FrameDurations>(Id::new(uri)))
            .unwrap();
        assert_eq!(
            durations.all().copied().collect::<Vec<_>>(),
            vec![Duration::from_millis(40), Duration::from_millis(70)]
        );
    }

    #[test]
    fn forget_during_running_decode_discards_success_and_error_then_allows_reload() {
        for forget_all in [false, true] {
            for fail_decode in [false, true] {
                let loader = BoundedGifLoader::new(1024);
                let ctx = egui::Context::default();
                let uri = "bytes://gif_invalidated.gif";
                let (entered_tx, entered_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let release = ReleaseOnDrop(release_tx);
                loader
                    .schedule_decode(&ctx, uri, gif_bytes(), move |bytes| {
                        let _ = entered_tx.send(());
                        let _ = release_rx.recv();
                        if fail_decode {
                            Err("失效任务的解码错误".to_owned())
                        } else {
                            AnimatedImage::load_gif(bytes)
                        }
                    })
                    .unwrap();
                entered_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
                let old_generation = loader.state.lock().pending[uri];
                if forget_all {
                    loader.forget_all();
                } else {
                    loader.forget(uri);
                }
                assert!(!loader.has_pending());
                drop(release);
                drain_worker(&loader);
                {
                    let state = loader.state.lock();
                    assert!(!state.ready.contains(uri));
                    assert!(!state.errors.contains(uri));
                    assert_eq!(state.total_bytes, 0);
                }
                assert!(
                    ctx.data(|data| data.get_temp::<FrameDurations>(Id::new(uri)))
                        .is_none()
                );
                let release = block_worker(&loader);
                loader
                    .schedule_decode(&ctx, uri, gif_bytes(), AnimatedImage::load_gif)
                    .unwrap();
                assert_ne!(loader.state.lock().pending[uri], old_generation);
                drop(release);
                drain_worker(&loader);
                assert_eq!(
                    ready_frame(&loader, &ctx, uri, 1).pixels[0],
                    egui::Color32::BLUE
                );
            }
        }
    }

    #[test]
    fn forgotten_queued_job_is_skipped_before_decoding() {
        let loader = BoundedGifLoader::new(1024);
        let ctx = egui::Context::default();
        let release = block_worker(&loader);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_job = Arc::clone(&calls);
        let uri = "bytes://gif_queued_stale.gif";
        loader
            .schedule_decode(&ctx, uri, gif_bytes(), move |bytes| {
                calls_for_job.fetch_add(1, Ordering::SeqCst);
                AnimatedImage::load_gif(bytes)
            })
            .unwrap();
        loader.forget(uri);
        drop(release);
        drain_worker(&loader);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!loader.state.lock().ready.contains(uri));
    }

    #[test]
    fn gif_queue_is_bounded_deduplicates_uris_and_retries_when_full() {
        let loader = BoundedGifLoader::new(1024);
        let ctx = egui::Context::default();
        let release = block_worker(&loader);
        let calls = Arc::new(AtomicUsize::new(0));
        // 一个 worker 已阻塞，最多还能容纳四个不同 URI 的任务。
        for index in 0..4 {
            let uri = format!("bytes://gif_queue_{index}.gif");
            for _ in 0..3 {
                let calls = Arc::clone(&calls);
                loader
                    .schedule_decode(&ctx, &uri, gif_bytes(), move |bytes| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        AnimatedImage::load_gif(bytes)
                    })
                    .unwrap();
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(loader.state.lock().pending.len(), 4);
        let overflow = "bytes://gif_queue_overflow.gif";
        ctx.include_bytes(overflow, gif_bytes());
        assert!(matches!(
            loader.load(&ctx, &format!("{overflow}#0"), SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
        {
            let state = loader.state.lock();
            assert!(!state.pending.contains_key(overflow));
            assert!(!state.errors.contains(overflow));
            assert_eq!(state.pending.len(), 4);
        }
        drop(release);
        drain_worker(&loader);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(matches!(
            loader.load(&ctx, &format!("{overflow}#0"), SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
        drain_worker(&loader);
        assert_eq!(
            ready_frame(&loader, &ctx, overflow, 0).pixels[0],
            egui::Color32::RED
        );
    }

    #[test]
    fn decode_failure_is_cached_until_forget_and_error_cache_is_bounded() {
        let loader = BoundedGifLoader::new(1024);
        let ctx = egui::Context::default();
        let uri = "bytes://gif_broken.gif";
        ctx.include_bytes(uri, b"GIF89a".to_vec());
        assert!(matches!(
            loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
        drain_worker(&loader);
        for _ in 0..3 {
            assert!(matches!(
                loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
                Err(LoadError::Loading(_))
            ));
            assert!(!loader.has_pending());
        }
        loader.forget(uri);
        // 解码缓存与 include 源字节缓存独立；替换同 URI 的内容前必须遗忘旧源。
        ctx.loaders().include.forget(uri);
        ctx.include_bytes(uri, gif_bytes());
        assert!(matches!(
            loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
        drain_worker(&loader);
        assert_eq!(
            ready_frame(&loader, &ctx, uri, 0).pixels[0],
            egui::Color32::RED
        );
        let mut state = GifState::new(1024);
        for index in 0..=MAX_GIF_ERROR_ENTRIES {
            let uri = format!("bad_{index}");
            let generation = state.begin_decode(&uri).unwrap();
            assert!(state.complete_decode(&uri, generation, Err("损坏的 GIF".to_owned())));
        }
        assert_eq!(state.errors.len(), MAX_GIF_ERROR_ENTRIES);
        assert!(!state.errors.contains("bad_0"));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn frame_limit_and_active_cache_soft_budget_are_preserved() {
        let bytes = gif_bytes_with_frames(MAX_GIF_FRAMES + 1);
        let image = AnimatedImage::load_gif(&bytes).unwrap();
        assert_eq!(image.frames.len(), MAX_GIF_FRAMES);
        assert_eq!(image.frame_durations.all().count(), MAX_GIF_FRAMES);
        assert_eq!(
            image.get_image(MAX_GIF_FRAMES).pixels[0],
            image.get_image(0).pixels[0]
        );
        let now = Instant::now();
        let mut state = GifState::new(1);
        let first = Arc::new(AnimatedImage::load_gif(gif_bytes().as_ref()).unwrap());
        let second = Arc::new(AnimatedImage::load_gif(gif_bytes().as_ref()).unwrap());
        state.insert_ready("first".to_owned(), first, now);
        state.insert_ready("second".to_owned(), second, now);
        assert_eq!(state.ready.len(), 2);
        let later = now + ACTIVE_GIF_GRACE + Duration::from_millis(1);
        // 与 Ready 的 load 路径一致：实际访问会更新 LRU 和 last_used，而非只靠插入顺序。
        state.ready.get_mut("second").unwrap().last_used = later;
        state.evict_if_needed(later);
        assert_eq!(state.ready.len(), 1);
        assert!(!state.ready.contains("first"));
        assert!(state.ready.contains("second"));
        assert!(state.total_bytes > state.max_bytes);
    }
}
