//! tracking 图片加载器的内存态状态机。
//!
//! 位于 `crate::image_loader` 之下，是 pending / ready / error 与字节统计的唯一真相来源。
//! 用“代际编号（generation）”防止 `forget` / 重试后旧任务结果回写；ready 按字节软限制
//! 做 LRU 淘汰并保留活跃宽限，error 用有界 LRU 避免坏 URL 无限堆积。

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;

use super::util::format_bytes;
use egui::{ColorImage, Vec2};
use lru::LruCache;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::debug;

/// 错误缓存只保留一个很小的 LRU：
/// - 可以避免同一坏 URL 每帧都重新报错；
/// - 又不会像旧实现那样无限堆积失败条目。
const MAX_ERROR_ENTRIES: usize = 128;
/// 同一屏图片允许短暂突破缓存预算，避免可见工作集大于预算时来回解码。
const ACTIVE_IMAGE_GRACE: Duration = Duration::from_millis(750);

pub enum PrepareLoad {
    /// 内存已命中，直接可用的已解码图片。
    Ready(Arc<ColorImage>),
    /// 仍在等待底层 `BytesLoader` 返回结果。
    ///
    /// 这种状态不能直接短路返回，后续帧仍然要继续 poll `ctx.try_load_bytes`。
    WaitingBytes { generation: u64 },
    /// 原始字节已经拿到，后台 decode job 也已经提交。
    Decoding { size: Option<Vec2> },
    /// 已缓存的加载错误（含用户可读信息）。
    Error(Arc<str>),
}

/// pending 条目的两个阶段：等待底层字节 / 后台解码中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPhase {
    /// 正在等待底层 bytes loader 返回字节。
    WaitingBytes,
    /// 字节已就绪，后台解码任务已提交。
    Decoding,
}

/// 一个 pending 请求的代际、尺寸提示与当前阶段。
#[derive(Debug, Clone, Copy)]
struct PendingEntry {
    /// 代际编号，用于丢弃过期结果。
    generation: u64,
    /// 尺寸提示（若有），用于占位。
    size: Option<Vec2>,
    /// 当前阶段。
    phase: PendingPhase,
}

/// 一个已就绪的缓存图片及其记账信息。
struct ReadyEntry {
    /// 已解码图片。
    image: Arc<ColorImage>,
    /// 该图片占用的像素字节。
    byte_size: u64,
    /// 最近一次被使用的时间，用于 LRU 与活跃宽限。
    last_used: Instant,
}

/// 一个缓存的加载错误。
struct ErrorEntry {
    /// 用户可读的错误信息。
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
    /// 已就绪图片的字节软上限。
    max_ready_bytes: u64,
    /// 下一个代际编号。
    next_generation: u64,
    /// 当前 ready 图片占用的总字节。
    ready_bytes: u64,
    // 和 ready 图片一样统一按 u64 记账，避免混用不同整数宽度。
    /// 当前 error 字符串占用的总字节。
    error_bytes: u64,
    /// 已就绪图片的 LRU（不限条目数，仅按字节软限制淘汰）。
    ready: LruCache<String, ReadyEntry>,
    /// 有界 LRU 的加载错误。
    errors: LruCache<String, ErrorEntry>,
    /// 进行中的请求（等待字节或解码中）。
    pending: HashMap<String, PendingEntry>,
}

/// `LoaderState` 的状态机转移、缓存与淘汰。
impl LoaderState {
    /// 用给定的 ready 字节上限创建状态；error 用有界 LRU，ready 不限条目数。
    pub fn new(max_ready_bytes: u64) -> Self {
        let error_capacity =
            NonZeroUsize::new(MAX_ERROR_ENTRIES).expect("MAX_ERROR_ENTRIES 必须大于零");

        Self {
            max_ready_bytes,
            next_generation: 1,
            ready_bytes: 0,
            error_bytes: 0,
            // 已就绪缓存按总字节数做软限制，因此不直接限制条目数。
            ready: LruCache::unbounded(),
            // 错误缓存本身就是按条目数限长，直接用有界 LRU 更清晰。
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

    /// 在等待字节阶段更新某 pending 的尺寸提示；代际不匹配或已进入解码阶段则忽略。
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
                debug!("淘汰图片错误缓存: uri={}", evicted_uri);
            }
        }
        self.error_bytes = self.error_bytes.saturating_add(message.len() as u64);

        true
    }

    /// 从 pending / ready / error 中移除某 URI 的全部内存态。
    pub fn forget(&mut self, uri: &str) {
        self.pending.remove(uri);
        self.remove_ready(uri);
        self.remove_error(uri);
    }

    /// 清空所有内存态并把字节记账归零。
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

    /// 返回当前 ready 图片占用的总字节。
    pub fn ready_byte_size(&self) -> u64 {
        self.ready_bytes
    }

    /// 是否还有 pending 请求。
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 用于 worker 在真正开始 decode 前做一次快速 stale 检查。
    pub fn is_pending_generation(&self, uri: &str, generation: u64) -> bool {
        self.pending
            .get(uri)
            .is_some_and(|entry| entry.generation == generation)
    }

    /// 分配一个非零代际编号（跳过 0，绕回时从 1 重新开始）。
    fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        generation
    }

    /// 从 ready 中移除某 URI 并扣减字节记账。
    fn remove_ready(&mut self, uri: &str) {
        if let Some(entry) = self.ready.pop(uri) {
            self.ready_bytes = self.ready_bytes.saturating_sub(entry.byte_size);
        }
    }

    /// 从 error 中移除某 URI 并扣减字节记账。
    fn remove_error(&mut self, uri: &str) {
        if let Some(entry) = self.errors.pop(uri) {
            self.error_bytes = self.error_bytes.saturating_sub(entry.message.len() as u64);
        }
    }

    /// 超过 ready 字节上限时从 LRU 头部淘汰；仍在活跃宽限内的条目暂时保留。
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
                "淘汰已就绪图片缓存: uri={} bytes={} total_after={} max={}",
                uri,
                format_bytes(entry.byte_size),
                format_bytes(self.ready_bytes),
                format_bytes(self.max_ready_bytes)
            );
        }
    }

    /// 是否已超过 ready 字节预算；注意 `max_ready_bytes == 0` 时恒视为超限。
    fn ready_limit_exceeded(&self) -> bool {
        self.max_ready_bytes == 0 || self.ready_bytes > self.max_ready_bytes
    }
}
