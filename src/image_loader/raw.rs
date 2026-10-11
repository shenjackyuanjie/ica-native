//! 原始（未解码）图片字节的进程内缓存。
//!
//! 位于 `crate::image_loader` 之下，与“已解码像素”的缓存相互独立：
//! tracking loader 在解码前把原始字节存进来，上层（如引用原图、合并转发）可再取回。
//! 通过 egui 临时数据挂在 `Context` 上，按字节上限做 LRU 淘汰。

use egui::mutex::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 原始图片字节的 LRU 缓存；内部用 `Arc<Mutex<..>>` 以便跨 loader 克隆共享。
#[derive(Clone)]
pub struct RawImageCache(Arc<Mutex<RawImageCacheState>>);

/// 原始字节缓存的内部状态：条目 + LRU 顺序 + 字节记账 + 上限。
struct RawImageCacheState {
    /// URI -> 原始字节。
    entries: HashMap<String, Arc<[u8]>>,
    /// LRU 访问顺序（头部最旧）。
    order: VecDeque<String>,
    /// 当前占用字节。
    bytes: u64,
    /// 字节上限；`0` 表示不限制。
    max_bytes: u64,
}

/// `RawImageCache` 的增删查与淘汰。
impl RawImageCache {
    /// 用给定字节上限创建空缓存。
    pub fn new(max_bytes: u64) -> Self {
        Self(Arc::new(Mutex::new(RawImageCacheState {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
        })))
    }

    /// 取某 URI 的原始字节并刷新其 LRU 位置；不存在返回 `None`。
    pub fn get(&self, uri: &str) -> Option<Arc<[u8]>> {
        let mut state = self.0.lock();
        let value = state.entries.get(uri)?.clone();
        state.order.retain(|entry| entry != uri);
        state.order.push_back(uri.to_string());
        Some(value)
    }

    /// 插入/更新某 URI 的原始字节，更新记账并按上限淘汰最旧条目。
    pub fn insert(&self, uri: String, bytes: Arc<[u8]>) {
        let mut state = self.0.lock();
        if let Some(previous) = state.entries.remove(&uri) {
            state.bytes = state.bytes.saturating_sub(previous.len() as u64);
        }
        state.order.retain(|entry| entry != &uri);
        state.bytes = state.bytes.saturating_add(bytes.len() as u64);
        state.entries.insert(uri.clone(), bytes);
        state.order.push_back(uri);

        while state.max_bytes > 0 && state.bytes > state.max_bytes && state.entries.len() > 1 {
            let Some(oldest) = state.order.pop_front() else {
                break;
            };
            if let Some(removed) = state.entries.remove(&oldest) {
                state.bytes = state.bytes.saturating_sub(removed.len() as u64);
            }
        }
    }
}

/// 返回缓存在 `egui` 临时数据中的存储键。
fn cache_id() -> egui::Id {
    egui::Id::new("ica_native_original_image_bytes")
}

/// 把缓存实例挂到 `egui::Context` 的临时数据上，供全局 `remember`/`get` 访问。
pub fn install(ctx: &egui::Context, cache: RawImageCache) {
    ctx.data_mut(|data| data.insert_temp(cache_id(), cache));
}

/// 把某 URI 的原始字节记入挂载在 `Context` 上的缓存（若已安装）。
pub fn remember(ctx: &egui::Context, uri: &str, bytes: &[u8]) {
    let cache = ctx.data(|data| data.get_temp::<RawImageCache>(cache_id()));
    if let Some(cache) = cache {
        cache.insert(uri.to_string(), Arc::<[u8]>::from(bytes));
    }
}

/// 从挂载在 `Context` 上的缓存取某 URI 的原始字节；未安装或未命中返回 `None`。
pub fn get(ctx: &egui::Context, uri: &str) -> Option<Arc<[u8]>> {
    ctx.data(|data| data.get_temp::<RawImageCache>(cache_id()))?
        .get(uri)
}
