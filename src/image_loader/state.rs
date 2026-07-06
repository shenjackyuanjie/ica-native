use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{ColorImage, Vec2};
use lru::LruCache;
use tracing::debug;

use super::util::format_bytes;

/// 错误缓存只保留一个很小的 LRU：
/// - 可以避免同一坏 URL 每帧都重新报错；
/// - 又不会像旧实现那样无限堆积失败条目。
const MAX_ERROR_ENTRIES: usize = 128;
/// 同一屏图片允许短暂突破缓存预算，避免可见工作集大于预算时来回解码。
const ACTIVE_IMAGE_GRACE: Duration = Duration::from_millis(750);

pub enum PrepareLoad {
    Ready(Arc<ColorImage>),
    /// 仍在等待底层 `BytesLoader` 返回结果。
    ///
    /// 这种状态不能直接短路返回，后续帧仍然要继续 poll `ctx.try_load_bytes`。
    WaitingBytes {
        generation: u64,
    },
    /// 原始字节已经拿到，后台 decode job 也已经提交。
    Decoding {
        size: Option<Vec2>,
    },
    Error(Arc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPhase {
    WaitingBytes,
    Decoding,
}

#[derive(Debug, Clone, Copy)]
struct PendingEntry {
    generation: u64,
    size: Option<Vec2>,
    phase: PendingPhase,
}

struct ReadyEntry {
    image: Arc<ColorImage>,
    byte_size: u64,
    last_used: Instant,
}

struct ErrorEntry {
    message: Arc<str>,
}

/// 内存态缓存的唯一真相来源。
///
/// 这次重写最重要的一点就是：
/// - 不再把 `pending`、`ready`、`error`、`统计值` 分散在多个容器里；
/// - 所有状态转移都通过这一个结构完成。
///
/// 这样就不会再出现旧实现那种：
/// - `forget/forget_all` 后旧线程结果又写回来；
/// - `network_pending` 永久残留；
/// - 统计值和 map 实际内容漂移。
pub struct LoaderState {
    max_ready_bytes: u64,
    next_generation: u64,
    ready_bytes: u64,
    // 和 ready 图片一样统一按 u64 记账，避免混用不同整数宽度。
    error_bytes: u64,
    ready: LruCache<String, ReadyEntry>,
    errors: LruCache<String, ErrorEntry>,
    pending: HashMap<String, PendingEntry>,
}

impl LoaderState {
    pub fn new(max_ready_bytes: u64) -> Self {
        let error_capacity =
            NonZeroUsize::new(MAX_ERROR_ENTRIES).expect("MAX_ERROR_ENTRIES must be non-zero");

        Self {
            max_ready_bytes,
            next_generation: 1,
            ready_bytes: 0,
            error_bytes: 0,
            // ready 按总字节数做软限制，因此不直接限制条目数。
            ready: LruCache::unbounded(),
            // error cache 本身就是按条目数限长，直接用有界 LRU 更清晰。
            errors: LruCache::new(error_capacity),
            pending: HashMap::new(),
        }
    }

    /// 读取当前缓存状态；若完全未见过该 URI，则创建一个新的 pending slot。
    pub fn prepare_load(&mut self, uri: &str, now: Instant) -> PrepareLoad {
        if let Some(entry) = self.ready.get_mut(uri) {
            entry.last_used = now;
            let image = Arc::clone(&entry.image);
            self.evict_ready_if_needed(now);
            return PrepareLoad::Ready(image);
        }

        self.evict_ready_if_needed(now);

        if let Some(entry) = self.errors.get(uri) {
            return PrepareLoad::Error(Arc::clone(&entry.message));
        }

        if let Some(entry) = self.pending.get(uri) {
            return match entry.phase {
                PendingPhase::WaitingBytes => PrepareLoad::WaitingBytes {
                    generation: entry.generation,
                },
                PendingPhase::Decoding => PrepareLoad::Decoding { size: entry.size },
            };
        }

        let generation = self.allocate_generation();
        self.pending.insert(
            uri.to_owned(),
            PendingEntry {
                generation,
                size: None,
                phase: PendingPhase::WaitingBytes,
            },
        );
        PrepareLoad::WaitingBytes { generation }
    }

    pub fn update_waiting_size(&mut self, uri: &str, generation: u64, size: Option<Vec2>) {
        let Some(entry) = self.pending.get_mut(uri) else {
            return;
        };
        if entry.generation != generation {
            return;
        }

        if entry.phase == PendingPhase::WaitingBytes {
            entry.size = size;
        }
    }

    /// 标记该 URI 已经从“等待底层 bytes loader”进入“后台解码中”。
    pub fn mark_decoding(&mut self, uri: &str, generation: u64, size: Option<Vec2>) -> bool {
        let Some(entry) = self.pending.get_mut(uri) else {
            return false;
        };
        if entry.generation != generation || entry.phase != PendingPhase::WaitingBytes {
            return false;
        }

        // 只允许 WaitingBytes -> Decoding 做一次单向转换。
        // 这样多线程/多 context 并发命中同一 URI 时，后到者不会重复提交同一代 decode job。
        entry.phase = PendingPhase::Decoding;
        if size.is_some() {
            entry.size = size;
        }
        true
    }

    /// 如果当前 pending 还是这一代请求，就取消它。
    pub fn cancel_pending(&mut self, uri: &str, generation: u64) -> bool {
        let Some(entry) = self.pending.get(uri) else {
            return false;
        };
        if entry.generation != generation {
            return false;
        }

        self.pending.remove(uri);
        true
    }

    /// 后台线程提交解码成功结果。
    ///
    /// 只有 generation 仍然匹配时才会真正写入，
    /// 这样就能防住 `forget` / 重试 / 旧任务晚到 这些竞态。
    pub fn complete_ready(
        &mut self,
        uri: &str,
        generation: u64,
        image: Arc<ColorImage>,
        byte_size: u64,
        now: Instant,
    ) -> bool {
        let Some(entry) = self.pending.get(uri) else {
            return false;
        };
        if entry.generation != generation {
            return false;
        }

        self.pending.remove(uri);
        self.remove_error(uri);

        if let Some(old) = self.ready.put(
            uri.to_owned(),
            ReadyEntry {
                image,
                byte_size,
                last_used: now,
            },
        ) {
            self.ready_bytes = self.ready_bytes.saturating_sub(old.byte_size);
        }
        self.ready_bytes = self.ready_bytes.saturating_add(byte_size);

        self.evict_ready_if_needed(now);
        true
    }

    /// 后台线程提交解码失败结果。
    pub fn complete_error(&mut self, uri: &str, generation: u64, message: String) -> bool {
        let Some(entry) = self.pending.get(uri) else {
            return false;
        };
        if entry.generation != generation {
            return false;
        }

        self.pending.remove(uri);
        self.remove_ready(uri);

        let message: Arc<str> = Arc::from(message);
        // `push` 会把“旧 key 被替换”或“容量满时被淘汰的 LRU 条目”一并返回，
        // 这样才能把 error_bytes 统计和有界 LRU 的真实内容保持一致。
        if let Some((evicted_uri, old)) = self.errors.push(
            uri.to_owned(),
            ErrorEntry {
                message: Arc::clone(&message),
            },
        ) {
            self.error_bytes = self.error_bytes.saturating_sub(old.message.len() as u64);
            if evicted_uri != uri {
                debug!("image error cache evict: uri={}", evicted_uri);
            }
        }
        self.error_bytes = self.error_bytes.saturating_add(message.len() as u64);

        true
    }

    pub fn forget(&mut self, uri: &str) {
        self.pending.remove(uri);
        self.remove_ready(uri);
        self.remove_error(uri);
    }

    pub fn forget_all(&mut self) {
        self.pending.clear();
        self.ready.clear();
        self.errors.clear();
        self.ready_bytes = 0;
        self.error_bytes = 0;
    }

    /// 这里返回的是“已缓存对象”的近似内存：
    /// - Ready 图片按像素字节统计；
    /// - Error 仅统计错误字符串；
    /// - Pending / in-flight decode 的临时内存不算在这里。
    pub fn byte_size(&self) -> usize {
        let total = self.ready_bytes.saturating_add(self.error_bytes);
        total.min(usize::MAX as u64) as usize
    }

    pub fn ready_byte_size(&self) -> u64 {
        self.ready_bytes
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 用于 worker 在真正开始 decode 前做一次快速 stale 检查。
    pub fn is_pending_generation(&self, uri: &str, generation: u64) -> bool {
        self.pending
            .get(uri)
            .is_some_and(|entry| entry.generation == generation)
    }

    fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        generation
    }

    fn remove_ready(&mut self, uri: &str) {
        if let Some(entry) = self.ready.pop(uri) {
            self.ready_bytes = self.ready_bytes.saturating_sub(entry.byte_size);
        }
    }

    fn remove_error(&mut self, uri: &str) {
        if let Some(entry) = self.errors.pop(uri) {
            self.error_bytes = self.error_bytes.saturating_sub(entry.message.len() as u64);
        }
    }

    fn evict_ready_if_needed(&mut self, now: Instant) {
        while self.ready.len() > 1 && self.ready_limit_exceeded() {
            let Some((uri, entry)) = self.ready.pop_lru() else {
                break;
            };
            if now.saturating_duration_since(entry.last_used) <= ACTIVE_IMAGE_GRACE {
                // LRU 都仍在近期可见工作集中，后面的条目只会更新；暂时允许
                // 突破软预算，等图片离屏超过 grace 后再回收，避免反复解码。
                self.ready.put(uri, entry);
                break;
            }

            self.ready_bytes = self.ready_bytes.saturating_sub(entry.byte_size);
            debug!(
                "image ready cache evict: uri={} bytes={} total_after={} max={}",
                uri,
                format_bytes(entry.byte_size),
                format_bytes(self.ready_bytes),
                format_bytes(self.max_ready_bytes)
            );
        }
    }

    fn ready_limit_exceeded(&self) -> bool {
        self.max_ready_bytes == 0 || self.ready_bytes > self.max_ready_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Arc<ColorImage> {
        Arc::new(ColorImage::new([1, 1], vec![egui::Color32::WHITE]))
    }

    fn begin(state: &mut LoaderState, uri: &str, now: Instant) -> u64 {
        match state.prepare_load(uri, now) {
            PrepareLoad::WaitingBytes { generation } => generation,
            _ => panic!("expected pending image"),
        }
    }

    #[test]
    fn active_working_set_can_temporarily_exceed_budget_then_recovers() {
        let now = Instant::now();
        let mut state = LoaderState::new(4);
        let first = begin(&mut state, "first", now);
        assert!(state.mark_decoding("first", first, None));
        assert!(state.complete_ready("first", first, image(), 4, now));

        let second = begin(&mut state, "second", now);
        assert!(state.mark_decoding("second", second, None));
        assert!(state.complete_ready("second", second, image(), 4, now));
        assert_eq!(state.ready.len(), 2);

        let later = now + ACTIVE_IMAGE_GRACE + Duration::from_millis(1);
        assert!(matches!(
            state.prepare_load("second", later),
            PrepareLoad::Ready(_)
        ));
        assert_eq!(state.ready.len(), 1);
        assert!(state.ready.contains("second"));
    }
}
