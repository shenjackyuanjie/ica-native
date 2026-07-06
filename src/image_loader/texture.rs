use std::{
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    Context, TextureHandle, TextureOptions,
    load::{
        BytesLoader as _, ImagePoll, LoadError, SizeHint, SizedTexture, TextureLoadResult,
        TextureLoader, TexturePoll,
    },
    mutex::Mutex,
};
use lru::LruCache;

const ACTIVE_TEXTURE_GRACE: Duration = Duration::from_millis(750);

#[derive(Clone, Eq)]
struct TextureKey {
    uri: String,
    options: TextureOptions,
}

impl PartialEq for TextureKey {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri && self.options == other.options
    }
}

impl Hash for TextureKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
        self.options.hash(state);
    }
}

struct TextureEntry {
    handle: TextureHandle,
    source_size: egui::Vec2,
    byte_size: u64,
    last_used: Instant,
}

struct TextureState {
    max_bytes: u64,
    total_bytes: u64,
    textures: LruCache<TextureKey, TextureEntry>,
}

impl TextureState {
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            total_bytes: 0,
            textures: LruCache::unbounded(),
        }
    }

    fn evict_stale(&mut self, now: Instant) {
        while self.textures.len() > 1 && (self.max_bytes == 0 || self.total_bytes > self.max_bytes)
        {
            let Some((key, entry)) = self.textures.pop_lru() else {
                break;
            };
            if now.saturating_duration_since(entry.last_used) <= ACTIVE_TEXTURE_GRACE {
                self.textures.put(key, entry);
                break;
            }
            self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
        }
    }
}

/// 为普通位图提供有预算的 GPU 纹理缓存；SVG 继续交给 egui 默认加载器，
/// 保留其按尺寸缓存的专门行为。
pub struct TrackingTextureLoader {
    state: Mutex<TextureState>,
}

impl TrackingTextureLoader {
    pub const ID: &'static str = egui::generate_loader_id!(TrackingTextureLoader);

    pub fn new(max_bytes: u64) -> Self {
        Self {
            state: Mutex::new(TextureState::new(max_bytes)),
        }
    }
}

impl TextureLoader for TrackingTextureLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(
        &self,
        ctx: &Context,
        uri: &str,
        texture_options: TextureOptions,
        size_hint: SizeHint,
    ) -> TextureLoadResult {
        if uri
            .split(['?', '#'])
            .next()
            .is_some_and(|path| path.ends_with(".svg"))
        {
            return Err(LoadError::NotSupported);
        }

        let key = TextureKey {
            uri: uri.to_string(),
            options: texture_options,
        };
        let now = Instant::now();
        {
            let mut state = self.state.lock();
            if let Some(entry) = state.textures.get_mut(&key) {
                entry.last_used = now;
                let texture = SizedTexture::new(entry.handle.id(), entry.source_size);
                state.evict_stale(now);
                return Ok(TexturePoll::Ready { texture });
            }
            state.evict_stale(now);
        }

        let image = match ctx.try_load_image(uri, size_hint)? {
            ImagePoll::Ready { image } => image,
            ImagePoll::Pending { size } => return Ok(TexturePoll::Pending { size }),
        };
        let source_size = image.source_size;
        let handle = ctx.load_texture(uri, image, texture_options);
        let texture = SizedTexture::new(handle.id(), source_size);
        let byte_size = handle.byte_size() as u64;
        let mut state = self.state.lock();
        if let Some(old) = state.textures.put(
            key,
            TextureEntry {
                handle,
                source_size,
                byte_size,
                last_used: now,
            },
        ) {
            state.total_bytes = state.total_bytes.saturating_sub(old.byte_size);
        }
        state.total_bytes = state.total_bytes.saturating_add(byte_size);
        state.evict_stale(now);
        drop(state);

        // GPU 上传后释放可重建的 HTTP/文件原始字节和 CPU 像素；bytes://
        // 通常是内嵌表情或待发送图片，仍需保留其源字节以便纹理淘汰后重建。
        let loaders = ctx.loaders();
        if !uri.starts_with("bytes://") {
            loaders.include.forget(uri);
            for loader in loaders.bytes.lock().iter().rev() {
                loader.forget(uri);
            }
        }
        for loader in loaders.image.lock().iter().rev() {
            loader.forget(uri);
        }

        Ok(TexturePoll::Ready { texture })
    }

    fn forget(&self, uri: &str) {
        let mut state = self.state.lock();
        let keys = state
            .textures
            .iter()
            .filter(|(key, _)| key.uri == uri)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entry) = state.textures.pop(&key) {
                state.total_bytes = state.total_bytes.saturating_sub(entry.byte_size);
            }
        }
    }

    fn forget_all(&self) {
        let mut state = self.state.lock();
        state.textures.clear();
        state.total_bytes = 0;
    }

    fn end_pass(&self, _pass_index: u64) {
        self.state.lock().evict_stale(Instant::now());
    }

    fn byte_size(&self) -> usize {
        self.state.lock().total_bytes.min(usize::MAX as u64) as usize
    }
}

pub fn install(ctx: &Context, max_bytes: u64) {
    if !ctx.is_loader_installed(TrackingTextureLoader::ID) {
        ctx.add_texture_loader(Arc::new(TrackingTextureLoader::new(max_bytes)));
    }
}
