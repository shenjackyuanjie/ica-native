use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use super::util::format_bytes;

/// 负责磁盘缓存的读写和落盘淘汰。
///
/// 内存状态和磁盘状态故意分离：
/// - 内存部分追求“立即可见”的正确性；
/// - 磁盘部分是跨帧/跨会话的 best-effort 优化。
///
/// 因此磁盘缓存不会参与 `generation` 逻辑，只做原始字节复用。
#[derive(Debug)]
pub struct DiskCache {
    root: PathBuf,
    max_bytes: u64,
    tracked_bytes: AtomicU64,
    mutation_lock: Mutex<()>,
}

impl DiskCache {
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

        info!("disk image cache dir: {:?}", cache.root);
        Some(cache)
    }

    pub fn load(&self, uri: &str) -> Option<Vec<u8>> {
        let path = self.cache_path(uri);
        fs::read(path).ok()
    }

    pub fn save(&self, uri: &str, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let _guard = self
            .mutation_lock
            .lock()
            .expect("disk cache mutation lock poisoned");

        if let Err(err) = self.save_locked(uri, bytes) {
            warn!("disk cache write failed: uri={} err={}", uri, err);
        }
    }

    pub fn remove(&self, uri: &str) {
        let _guard = self
            .mutation_lock
            .lock()
            .expect("disk cache mutation lock poisoned");

        let path = self.cache_path(uri);
        let old_size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
        match fs::remove_file(&path) {
            Ok(()) => {
                let _ = self.tracked_bytes.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |total| Some(total.saturating_sub(old_size)),
                );
                debug!("disk cache removed: {}", path.display());
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                warn!(
                    "disk cache remove failed: path={} err={}",
                    path.display(),
                    err
                );
            }
        }
    }

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

        // Windows 下 rename 到已存在文件可能失败，所以先删旧文件。
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
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(
                    total
                        .saturating_sub(old_size)
                        .saturating_add(bytes.len() as u64),
                )
            });

        debug!(
            "disk cache saved: uri={} path={} raw_bytes={}",
            uri,
            path.display(),
            format_bytes(bytes.len() as u64)
        );

        self.evict_if_needed_locked();
        Ok(())
    }

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
                        "disk cache evict: removed={} size={} total_after={}",
                        path.display(),
                        format_bytes(*size),
                        format_bytes(total_bytes)
                    );
                }
                Err(err) => {
                    warn!(
                        "disk cache evict failed: path={} err={}",
                        path.display(),
                        err
                    );
                }
            }
        }
        self.tracked_bytes.store(total_bytes, Ordering::Relaxed);
    }

    fn ensure_layout(&self) -> io::Result<()> {
        fs::create_dir_all(self.root.join("image"))?;
        fs::create_dir_all(self.root.join("avatar"))?;
        Ok(())
    }

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

fn is_avatar_uri(uri: &str) -> bool {
    uri.contains("qlogo.cn")
}

fn sha256_hex(uri: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(uri.as_bytes());
    hex::encode(hasher.finalize())
}
