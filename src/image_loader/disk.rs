//! 原始图片字节的磁盘缓存：跨会话/跨重启复用，减少重复网络请求。
//!
//! 位于 `crate::image_loader` 之下，与内存态状态机故意分离：
//! 内存追求“立即可见”的正确性，磁盘是 best-effort 优化，不参与 `generation` 逻辑。
//! 文件按 URI 的 SHA-256 命名，分 `image` / `avatar` 两个子目录，并按修改时间淘汰。

use super::util::format_bytes;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info, warn};

/// 负责磁盘缓存的读写和落盘淘汰。
///
/// 内存状态和磁盘状态故意分离：
/// - 内存部分追求“立即可见”的正确性；
/// - 磁盘部分是跨帧/跨会话的 best-effort 优化。
///
/// 因此磁盘缓存不会参与 `generation` 逻辑，只做原始字节复用。
#[derive(Debug)]
pub struct DiskCache {
    /// 缓存根目录。
    root: PathBuf,
    /// 字节上限；`0` 表示不限制。
    max_bytes: u64,
    /// 增量记账的当前总字节，避免每次写入都全盘扫描。
    tracked_bytes: AtomicU64,
    /// 串行化写/删操作的锁，防止并发落盘相互覆盖。
    mutation_lock: Mutex<()>,
}

/// `DiskCache` 的磁盘读写与淘汰实现。
impl DiskCache {
    /// 创建磁盘缓存并确保目录布局；布局失败返回 `None`（禁用磁盘缓存）。
    ///
    /// 启动时扫描一次现有总量，作为增量记账的初值。
    pub fn new(root: PathBuf, max_bytes: u64) -> Option<Self> {
        let cache = Self {
            root,
            max_bytes,
            tracked_bytes: AtomicU64::new(0),
            mutation_lock: Mutex::new(()),
        };

        if let Err(err) = cache.ensure_layout() {
            warn!(
                "无法创建磁盘图片缓存目录 {:?}: {}, 将禁用磁盘缓存",
                cache.root, err
            );
            return None;
        }
        cache
            .tracked_bytes
            .store(cache.scan_total_bytes(), Ordering::Relaxed);

        info!("图片磁盘缓存目录: {:?}", cache.root);
        Some(cache)
    }

    /// 读取某 URI 对应的缓存字节；文件不存在或读取失败返回 `None`。
    pub fn load(&self, uri: &str) -> Option<Vec<u8>> {
        let path = self.cache_path(uri);
        fs::read(path).ok()
    }

    /// 把某 URI 的原始字节写入磁盘缓存（持修改锁）；空字节直接忽略。
    pub fn save(&self, uri: &str, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let _guard = self.mutation_lock.lock().expect("磁盘缓存修改锁被污染");

        if let Err(err) = self.save_locked(uri, bytes) {
            warn!("写入图片磁盘缓存失败: uri={} err={}", uri, err);
        }
    }

    /// 删除某 URI 的缓存文件并相应减少记账；文件不存在时视为成功。
    pub fn remove(&self, uri: &str) {
        let _guard = self.mutation_lock.lock().expect("磁盘缓存修改锁被污染");

        let path = self.cache_path(uri);
        let old_size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        match fs::remove_file(&path) {
            Ok(()) => {
                let _ =
                    self.tracked_bytes
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                            Some(total.saturating_sub(old_size))
                        });
                debug!("已删除图片磁盘缓存: {}", path.display());
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                warn!("删除图片磁盘缓存失败: path={} err={}", path.display(), err);
            }
        }
    }

    /// `save` 的实际落盘逻辑（调用方需持有修改锁）：临时文件 + rename 原子替换。
    fn save_locked(&self, uri: &str, bytes: &[u8]) -> io::Result<()> {
        self.ensure_layout()?;

        let path = self.cache_path(uri);
        let old_size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        fs::create_dir_all(parent)?;

        // 用临时文件 + rename，避免读取过程中拿到半写入文件。
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = path.with_extension(format!("tmp-{unique}"));

        fs::write(&tmp_path, bytes)?;

        // Windows 下重命名到已存在文件可能失败，所以先删旧文件。
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(err);
            }
        }

        if let Err(err) = fs::rename(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        let _ = self
            .tracked_bytes
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(
                    total
                        .saturating_sub(old_size)
                        .saturating_add(bytes.len() as u64),
                )
            });

        debug!(
            "图片磁盘缓存已保存: uri={} path={} raw_bytes={}",
            uri,
            path.display(),
            format_bytes(bytes.len() as u64)
        );

        self.evict_if_needed_locked();
        Ok(())
    }

    /// 写入后按需淘汰：超过上限时扫描目录，按“先普通图片后头像”的顺序删到上限内。
    ///
    /// 调用方需持有修改锁。
    fn evict_if_needed_locked(&self) {
        if self.max_bytes == 0 {
            // 0 = 不限制大小。
            return;
        }

        // 大多数写入都没有达到上限，直接通过增量记账判断，避免每保存一张
        // 图片就 O(n) 扫描整个缓存目录。
        if self.tracked_bytes.load(Ordering::Relaxed) <= self.max_bytes {
            return;
        }

        // 这里仍然采用 save 后一次性扫描目录的简单实现：
        // 复杂度是 O(n)，但它只发生在后台线程，并且避免了额外的持久化索引复杂度。

        let subdirs = [self.root.join("image"), self.root.join("avatar")];
        let mut image_files = Vec::new();
        let mut avatar_files = Vec::new();
        let mut total_bytes = 0u64;

        for (idx, subdir) in subdirs.iter().enumerate() {
            let Ok(entries) = fs::read_dir(subdir) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let Ok(metadata) = entry.metadata() else {
                    continue;
                };

                let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
                let size = metadata.len();
                total_bytes = total_bytes.saturating_add(size);

                if idx == 0 {
                    image_files.push((path, size, modified));
                } else {
                    avatar_files.push((path, size, modified));
                }
            }
        }

        if total_bytes <= self.max_bytes {
            return;
        }

        image_files.sort_by_key(|(_, _, modified)| *modified);
        avatar_files.sort_by_key(|(_, _, modified)| *modified);

        // 头像会在会话列表、消息列表里被高频复用，
        // 所以这里有意优先淘汰普通图片，把 avatar 留得更久一些以提高命中率。
        for (path, size, _) in image_files.iter().chain(avatar_files.iter()) {
            if total_bytes <= self.max_bytes {
                break;
            }

            match fs::remove_file(path) {
                Ok(()) => {
                    total_bytes = total_bytes.saturating_sub(*size);
                    debug!(
                        "已淘汰图片磁盘缓存: removed={} size={} total_after={}",
                        path.display(),
                        format_bytes(*size),
                        format_bytes(total_bytes)
                    );
                }
                Err(err) => {
                    warn!("淘汰图片磁盘缓存失败: path={} err={}", path.display(), err);
                }
            }
        }
        self.tracked_bytes.store(total_bytes, Ordering::Relaxed);
    }

    /// 确保 `image` / `avatar` 两个子目录存在。
    fn ensure_layout(&self) -> io::Result<()> {
        fs::create_dir_all(self.root.join("image"))?;
        fs::create_dir_all(self.root.join("avatar"))?;
        Ok(())
    }

    /// 扫描两个子目录，累加所有缓存文件的当前字节数。
    fn scan_total_bytes(&self) -> u64 {
        [self.root.join("image"), self.root.join("avatar")]
            .into_iter()
            .filter_map(|directory| fs::read_dir(directory).ok())
            .flat_map(|entries| entries.flatten())
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .fold(0_u64, |total, metadata| {
                total.saturating_add(metadata.len())
            })
    }

    /// 计算某 URI 的缓存文件路径：头像走 `avatar/`，其余走 `image/`，文件名为 URI 的 SHA-256。
    fn cache_path(&self, uri: &str) -> PathBuf {
        let subdir = if is_avatar_uri(uri) {
            "avatar"
        } else {
            "image"
        };
        self.root
            .join(subdir)
            .join(format!("{}.img", sha256_hex(uri)))
    }
}

/// 判断是否为 QQ 头像 URI（含 `qlogo.cn`），用于分目录与差异化淘汰。
fn is_avatar_uri(uri: &str) -> bool {
    uri.contains("qlogo.cn")
}

/// 计算 URI 的 SHA-256 十六进制字符串，作为缓存文件名。
fn sha256_hex(uri: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(uri.as_bytes());
    hex::encode(hasher.finalize())
}
