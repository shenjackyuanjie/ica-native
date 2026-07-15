use std::{
    io::Cursor,
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    ColorImage, FrameDurations, Id, decode_animated_image_uri, has_gif_magic_header,
    load::{
        BytesLoader as _, BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint,
    },
    mutex::Mutex,
};
use image::AnimationDecoder as _;
use lru::LruCache;
use tracing::{debug, warn};

use super::util::format_bytes;

const ACTIVE_GIF_GRACE: Duration = Duration::from_millis(750);
const DEFAULT_GIF_CACHE_CAP: u64 = 64 * 1024 * 1024;
const MIN_GIF_CACHE_CAP: u64 = 16 * 1024 * 1024;
const MAX_GIF_FRAMES: usize = 180;
const MAX_GIF_DECODED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
struct AnimatedImage {
    frames: Vec<Arc<ColorImage>>,
    frame_durations: FrameDurations,
    byte_size: u64,
}

impl AnimatedImage {
    fn load_gif(data: &[u8]) -> Result<Self, String> {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(data))
            .map_err(|err| format!("Failed to decode gif: {err}"))?;

        let mut frames = Vec::new();
        let mut durations = Vec::new();
        let mut byte_size = size_of::<Self>() as u64;

        for frame in decoder.into_frames() {
            let frame = frame.map_err(|err| format!("Failed to decode gif: {err}"))?;
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
}

struct GifState {
    max_bytes: u64,
    total_bytes: u64,
    ready: LruCache<String, GifEntry>,
}

impl GifState {
    fn new(image_cache_max_bytes: u64) -> Self {
        Self {
            max_bytes: gif_cache_limit(image_cache_max_bytes),
            total_bytes: 0,
            ready: LruCache::unbounded(),
        }
    }

    fn insert_ready(&mut self, uri: String, image: Arc<AnimatedImage>, now: Instant) {
        if let Some(old) = self.ready.put(
            uri,
            GifEntry {
                image: Arc::clone(&image),
                last_used: now,
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
                "gif cache evict: uri={} bytes={} total_after={} max={}",
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
    state: Mutex<GifState>,
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
            state: Mutex::new(state),
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

        {
            let mut state = self.state.lock();
            if let Some(entry) = state.ready.get_mut(image_uri) {
                entry.last_used = now;
                let image = Arc::clone(&entry.image);
                ctx.data_mut(|data| {
                    *data.get_temp_mut_or_default(Id::new(image_uri)) =
                        image.frame_durations.clone();
                });
                state.evict_if_needed(now);
                return Ok(ImagePoll::Ready {
                    image: image.get_image(frame_index),
                });
            }
            state.evict_if_needed(now);
        }

        match ctx.try_load_bytes(image_uri) {
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Ok(BytesPoll::Ready { bytes, .. }) => {
                super::raw::remember(ctx, image_uri, bytes.as_ref());
                if !has_gif_magic_header(&bytes) {
                    return Err(LoadError::FormatNotSupported {
                        detected_format: None,
                    });
                }

                let image = Arc::new(AnimatedImage::load_gif(&bytes).map_err(|err| {
                    warn!("gif decode failed: uri={} err={}", image_uri, err);
                    LoadError::Loading(err)
                })?);
                ctx.data_mut(|data| {
                    *data.get_temp_mut_or_default(Id::new(image_uri)) =
                        image.frame_durations.clone();
                });

                {
                    let loaders = ctx.loaders();
                    loaders.include.forget(image_uri);
                    for loader in loaders.bytes.lock().iter().rev() {
                        loader.forget(image_uri);
                    }
                }

                let frame = image.get_image(frame_index);
                let byte_size = image.byte_size;
                self.state
                    .lock()
                    .insert_ready(image_uri.to_owned(), image, now);
                debug!(
                    "gif decoded: uri={} bytes={} frame={}",
                    image_uri,
                    format_bytes(byte_size),
                    frame_index
                );
                Ok(ImagePoll::Ready { image: frame })
            }
            Err(err) => Err(err),
        }
    }

    fn forget(&self, uri: &str) {
        if decode_animated_image_uri(uri).is_ok() {
            return;
        }

        let mut state = self.state.lock();
        if let Some(entry) = state.ready.pop(uri) {
            state.total_bytes = state.total_bytes.saturating_sub(entry.image.byte_size);
        }
    }

    fn forget_all(&self) {
        let mut state = self.state.lock();
        state.ready.clear();
        state.total_bytes = 0;
    }

    fn byte_size(&self) -> usize {
        self.state.lock().total_bytes.min(usize::MAX as u64) as usize
    }
}

pub fn install(ctx: &egui::Context, image_cache_max_bytes: u64) {
    if !ctx.is_loader_installed(BoundedGifLoader::ID) {
        ctx.add_image_loader(Arc::new(BoundedGifLoader::new(image_cache_max_bytes)));
    }
}
