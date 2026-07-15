use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use egui::mutex::Mutex;

#[derive(Clone)]
pub struct RawImageCache(Arc<Mutex<RawImageCacheState>>);

struct RawImageCacheState {
    entries: HashMap<String, Arc<[u8]>>,
    order: VecDeque<String>,
    bytes: u64,
    max_bytes: u64,
}

impl RawImageCache {
    pub fn new(max_bytes: u64) -> Self {
        Self(Arc::new(Mutex::new(RawImageCacheState {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
        })))
    }

    pub fn get(&self, uri: &str) -> Option<Arc<[u8]>> {
        let mut state = self.0.lock();
        let value = state.entries.get(uri)?.clone();
        state.order.retain(|entry| entry != uri);
        state.order.push_back(uri.to_string());
        Some(value)
    }

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

fn cache_id() -> egui::Id {
    egui::Id::new("ica_native_original_image_bytes")
}

pub fn install(ctx: &egui::Context, cache: RawImageCache) {
    ctx.data_mut(|data| data.insert_temp(cache_id(), cache));
}

pub fn remember(ctx: &egui::Context, uri: &str, bytes: &[u8]) {
    let cache = ctx.data(|data| data.get_temp::<RawImageCache>(cache_id()));
    if let Some(cache) = cache {
        cache.insert(uri.to_string(), Arc::<[u8]>::from(bytes));
    }
}

pub fn get(ctx: &egui::Context, uri: &str) -> Option<Arc<[u8]>> {
    ctx.data(|data| data.get_temp::<RawImageCache>(cache_id()))?
        .get(uri)
}
