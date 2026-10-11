//! QQ 表情数据：ID 表、名称映射与按需加载的 APNG 字节缓存。
//!
//! 设计取舍：只把表情 ID（及名称）编译进二进制，APNG 文件在运行时从
//! `assets/face/{id:03}.apng` 读取，并用有界 LRU（见 [`FaceCache`]）缓存，
//! 避免数百个动图全量驻留内存。`names` 子模块由脚本自动生成，请勿手改。

/// (表情ID, 表情名称)
#[path = "face_data/names.rs"]
mod names;

use names::FACE_NAMES;
use std::sync::Arc;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

// 自动生成的表情数据模块
// 只内嵌表情 ID，APNG 文件按需从 assets/face 读取并做小型 LRU 缓存。

/// LRU 缓存的条目上限（按表情个数计），超出后从最久未使用的条目开始淘汰。
///
/// 96 约等于表情选择器一屏多的容量，够覆盖连续翻页又不无限占用内存。
const FACE_CACHE_LIMIT: usize = 96;

/// 全部可用表情 ID，按升序排列。
///
/// 该顺序同时是「二分查找」（`has_face`）和「展示顺序」（`face_id_at`）的依据，
/// 因此必须保持有序，新增表情应插入到正确位置。
static FACE_IDS: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 41, 42, 43, 46, 49, 50, 53, 54, 55, 56, 57,
    59, 60, 61, 63, 64, 66, 67, 69, 74, 75, 76, 77, 78, 79, 81, 85, 86, 89, 96, 97, 98, 99, 100,
    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119,
    120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138,
    139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157,
    158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176,
    177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195,
    196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214,
    215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233,
    234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 260, 261, 262, 263, 264,
    265, 266, 267, 268, 269, 270, 271, 272, 273, 274, 276, 277, 278, 279, 280, 281, 282, 283, 284,
    285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303,
    304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, 322,
    323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341,
    342, 343, 344, 345, 346, 347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360,
    361, 362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379,
    380, 381, 382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398,
    399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 415, 416, 417, 418,
    419, 420, 421, 422, 423, 424, 425, 426, 427, 428, 429, 430, 431, 432,
];

/// APNG 字节的小型 LRU 缓存。
///
/// `lru` 尾部是最近使用的条目，淘汰总是从头部（最久未使用）开始，
/// 因此命中后需要把对应 ID 重新移到尾部。
struct FaceCache {
    /// 表情 ID → 字节数据；`Arc` 让多个 UI 引用共享同一份缓冲。
    bytes_by_id: HashMap<u16, Arc<[u8]>>,
    /// 使用顺序记录，只存 ID，靠位置先后表达新旧。
    lru: Vec<u16>,
}

impl FaceCache {
    /// 创建空缓存。
    fn new() -> Self {
        Self {
            bytes_by_id: HashMap::new(),
            lru: Vec::new(),
        }
    }

    /// 取某个表情的字节：命中则刷新 LRU 顺序，未命中则读盘、入缓存并淘汰。
    ///
    /// 返回 `None` 表示文件不存在或读取失败，调用方按「无该表情」处理。
    /// 副作用：可能触发一次磁盘读取与一次缓存淘汰。
    fn get(&mut self, id: u16) -> Option<Arc<[u8]>> {
        if let Some(bytes) = self.bytes_by_id.get(&id).cloned() {
            self.touch(id);
            return Some(bytes);
        }

        let bytes = std::fs::read(face_path(id)).ok()?;
        let bytes = Arc::<[u8]>::from(bytes);
        self.bytes_by_id.insert(id, Arc::clone(&bytes));
        self.touch(id);
        self.evict_if_needed();
        Some(bytes)
    }

    /// 把 ID 移到 LRU 尾部，标记为最近使用（已在尾部时等价于无操作）。
    fn touch(&mut self, id: u16) {
        if let Some(index) = self.lru.iter().position(|existing| *existing == id) {
            self.lru.remove(index);
        }
        self.lru.push(id);
    }

    /// 超出 `FACE_CACHE_LIMIT` 时从头部淘汰，直到满足上限。
    fn evict_if_needed(&mut self) {
        while self.lru.len() > FACE_CACHE_LIMIT {
            let id = self.lru.remove(0);
            self.bytes_by_id.remove(&id);
        }
    }
}

/// 进程级表情缓存的惰性单例，首次使用时初始化。
///
/// 返回的 `Mutex` 只做短临界区保护（读盘期间持锁），不跨 await 点。
fn face_cache() -> &'static Mutex<FaceCache> {
    static CACHE: OnceLock<Mutex<FaceCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FaceCache::new()))
}

/// 表情 ID 到 APNG 文件路径：`{CARGO_MANIFEST_DIR}/assets/face/{id:03}.apng`。
///
/// 用编译期清单目录定位，保证开发态与安装态都能找到资源。
fn face_path(id: u16) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("face")
        .join(format!("{id:03}.apng"))
}

/// 判断表情 ID 是否存在（对有序的 `FACE_IDS` 做二分查找）。
///
/// 返回 `true` 表示该 ID 有对应的 APNG 资源。
pub fn has_face(id: u16) -> bool {
    FACE_IDS.binary_search(&id).is_ok()
}

/// 根据表情 ID 获取表情图片字节数据，命中后保存在小型 LRU 中。
///
/// ID 不存在或读盘失败时返回 `None`；成功时返回共享的 `Arc<[u8]>`，
/// 调用方无需再拷贝整份缓冲。
pub fn get_face(id: u16) -> Option<Arc<[u8]>> {
    if !has_face(id) {
        return None;
    }
    face_cache().lock().ok()?.get(id)
}

/// 获取所有可用的表情 ID
pub fn all_face_ids() -> impl Iterator<Item = u16> {
    FACE_IDS.iter().copied()
}

/// 按展示顺序获取表情 ID，用于虚拟化表情选择器。
///
/// `index` 超出范围时返回 `None`；顺序即 `FACE_IDS` 的排列顺序。
pub fn face_id_at(index: usize) -> Option<u16> {
    FACE_IDS.get(index).copied()
}

/// 可用表情总数
pub const FACE_COUNT: usize = FACE_IDS.len();

// 表情名称映射（自动生成自 faceNames.js）
// 共 306 个有名称的表情

/// 根据表情 ID 获取表情名称
pub fn get_face_name(id: u16) -> Option<&'static str> {
    FACE_NAMES
        .binary_search_by_key(&id, |(fid, _)| *fid)
        .ok()
        .map(|idx| FACE_NAMES[idx].1)
}
