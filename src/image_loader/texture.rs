//! 有预算的 GPU 纹理缓存（实现 egui `TextureLoader`）。
//!
//! 位于 `crate::image_loader` 之下，为普通位图把已解码图片上传成 GPU 纹理并按字节预算
//! 做 LRU；SVG 继续交给 egui 默认加载器，保留其按尺寸缓存的专门行为。
//! 纹理上传成功后会释放可重建的原始字节/CPU 像素，仅在确需保留时（如 `bytes://`）例外。

#[cfg(test)]
#[path = "tests/texture.rs"]
mod tests;

use egui::{
    Context, TextureHandle, TextureOptions,
    load::{
        BytesLoader as _, ImagePoll, LoadError, SizeHint, SizedTexture, TextureLoadResult,
        TextureLoader, TexturePoll,
    },
    mutex::Mutex,
};
use lru::LruCache;
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

/// 纹理缓存项的“活跃宽限”：最近用过的纹理即使超预算也暂不淘汰。
const ACTIVE_TEXTURE_GRACE: Duration = Duration::from_millis(750);

/// 纹理缓存键：同一 URI 在不同 `TextureOptions` 下是不同的缓存项。
#[derive(Clone, Eq)]
struct TextureKey {
    /// 图片 URI。
    uri: String,
    /// 纹理选项（缩放/采样等）。
    options: TextureOptions,
}

/// `TextureKey` 的手动判等实现。
impl PartialEq for TextureKey {
    /// 按 URI 与纹理选项判等。
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri && self.options == other.options
    }
}

/// `TextureKey` 的手动哈希实现。
impl Hash for TextureKey {
    /// 按 URI 与纹理选项计算哈希，与 `PartialEq` 保持一致。
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
        self.options.hash(state);
    }
}

/// 一个缓存的 GPU 纹理及其记账信息。
struct TextureEntry {
    /// egui 纹理句柄。
    handle: TextureHandle,
    /// 源图片尺寸。
    source_size: egui::Vec2,
    /// 纹理占用的字节。
    byte_size: u64,
    /// 最近一次被使用的时间，用于 LRU 与活跃宽限。
    last_used: Instant,
}

/// 纹理缓存的内部状态：LRU + 字节记账 + 上限。
struct TextureState {
    /// 字节上限；`0` 表示不限制。
    max_bytes: u64,
    /// 当前占用总字节。
    total_bytes: u64,
    /// 纹理 LRU。
    textures: LruCache<TextureKey, TextureEntry>,
}

/// `TextureState` 的构造与淘汰。
impl TextureState {
    /// 用给定上限创建空纹理状态（LRU 不限条目数，仅按字节软限制淘汰）。
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            total_bytes: 0,
            textures: LruCache::unbounded(),
        }
    }

    /// 命中时刷新纹理活跃状态并返回可显示的尺寸与 ID。
    fn cached_texture(&mut self, key: &TextureKey, now: Instant) -> Option<SizedTexture> {
        let texture = self.textures.get_mut(key).map(|entry| {
            entry.last_used = now;
            SizedTexture::new(entry.handle.id(), entry.source_size)
        });
        self.evict_stale(now);
        texture
    }

    /// 下一帧重建中，复用同一动画、同一采样选项最近显示的帧。
    /// 仍使用原有 LRU 的句柄与预算，离屏陈旧帧可以正常被淘汰。
    fn cached_gif_frame(
        &mut self,
        image_uri: &str,
        options: TextureOptions,
        now: Instant,
    ) -> Option<SizedTexture> {
        let key = self
            .textures
            .iter()
            .filter(|(key, _)| {
                key.options == options
                    && egui::decode_animated_image_uri(&key.uri)
                        .is_ok_and(|(base_uri, _)| base_uri == image_uri)
            })
            .max_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())?;
        self.cached_texture(&key, now)
    }

    /// 超过字节上限时从 LRU 头部淘汰；仍在活跃宽限内的纹理暂时保留。
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

/// `TrackingTextureLoader` 的构造。
impl TrackingTextureLoader {
    /// 本 loader 在 egui 中的唯一 ID。
    pub const ID: &'static str = egui::generate_loader_id!(TrackingTextureLoader);

    /// 用给定字节上限创建纹理加载器。
    pub fn new(max_bytes: u64) -> Self {
        Self {
            state: Mutex::new(TextureState::new(max_bytes)),
        }
    }
}

/// 为 `TrackingTextureLoader` 实现 egui `TextureLoader`。
impl TextureLoader for TrackingTextureLoader {
    /// 返回本 loader 的唯一 ID。
    fn id(&self) -> &str {
        Self::ID
    }

    /// egui 请求纹理时的入口：静态缓存命中直接返回，动画命中同步刷新像素缓存活跃状态。
    ///
    /// 动画临时 Pending 时复用可用帧；未命中则取图片、上传 GPU 并记账。
    /// SVG 让给默认 loader；上传成功后按需释放可重建的原始字节/CPU 像素。
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
        let gif_uri = egui::decode_animated_image_uri(uri)
            .ok()
            .map(|(base_uri, _)| base_uri);
        let cached = self.state.lock().cached_texture(&key, now);
        if gif_uri.is_none()
            && let Some(texture) = cached
        {
            return Ok(TexturePoll::Ready { texture });
        }

        // GIF 的 GPU 命中仍要访问像素加载器，让持续播放的动画刷新活跃状态；
        // 否则它会在帧纹理已缓存后被误判为离屏，下一次帧未命中又重新解码。
        // 只在 Pending 时保留可用画面，格式变化和真实错误继续交给上层处理。
        let image = match ctx.try_load_image(uri, size_hint)? {
            ImagePoll::Ready { image } => {
                if let Some(texture) = cached {
                    return Ok(TexturePoll::Ready { texture });
                }
                image
            }
            ImagePoll::Pending { size } => {
                let texture = cached.or_else(|| {
                    gif_uri.and_then(|base_uri| {
                        self.state
                            .lock()
                            .cached_gif_frame(base_uri, texture_options, now)
                    })
                });
                return Ok(texture.map_or(TexturePoll::Pending { size }, |texture| {
                    TexturePoll::Ready { texture }
                }));
            }
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

    /// 忘记某 URI 的所有纹理缓存项；源 URI 同时清理其动画帧，帧 URI 只清理该帧。
    fn forget(&self, uri: &str) {
        let mut state = self.state.lock();
        let keys = state
            .textures
            .iter()
            .filter(|(key, _)| {
                key.uri == uri
                    || egui::decode_animated_image_uri(&key.uri)
                        .is_ok_and(|(base_uri, _)| base_uri == uri)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(entry) = state.textures.pop(&key) {
                state.total_bytes = state.total_bytes.saturating_sub(entry.byte_size);
            }
        }
    }

    /// 清空所有纹理缓存并重置记账。
    fn forget_all(&self) {
        let mut state = self.state.lock();
        state.textures.clear();
        state.total_bytes = 0;
    }

    /// 每遍渲染结束时做一次惰性淘汰，把超预算的陈旧纹理回收。
    fn end_pass(&self, _pass_index: u64) {
        self.state.lock().evict_stale(Instant::now());
    }

    /// 返回纹理缓存占用的总字节。
    fn byte_size(&self) -> usize {
        self.state.lock().total_bytes.min(usize::MAX as u64) as usize
    }
}

/// 在 `egui::Context` 上安装纹理加载器（若尚未安装）。
pub fn install(ctx: &Context, max_bytes: u64) {
    if !ctx.is_loader_installed(TrackingTextureLoader::ID) {
        ctx.add_texture_loader(Arc::new(TrackingTextureLoader::new(max_bytes)));
    }
}
