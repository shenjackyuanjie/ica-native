use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::{ConfigPaths, IcaCfg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickerPickerTab {
    #[default]
    QqFaces,
    Favorites,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerEntry {
    pub path: PathBuf,
    pub name: String,
    pub mime_type: String,
    pub modified_millis: u128,
}

#[derive(Debug, Clone)]
pub struct StickerStore {
    inner: Arc<RwLock<StickerState>>,
}

#[derive(Debug)]
struct StickerState {
    root: PathBuf,
    entries: Vec<StickerEntry>,
    fallback_notice: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct DetectedImage {
    pub extension: &'static str,
    pub mime_type: &'static str,
}

impl StickerStore {
    pub fn unavailable(root: PathBuf, error: impl ToString) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StickerState {
                root,
                entries: Vec::new(),
                fallback_notice: Some(format!("收藏表情目录不可用：{}", error.to_string())),
            })),
        }
    }

    pub fn resolve(config: &IcaCfg, paths: &ConfigPaths) -> Result<Self> {
        let preferred = config
            .sticker_path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(default_icalingua_sticker_dir);
        let fallback = paths.data_dir().join("stickers");

        let (root, fallback_notice) = match ensure_writable_directory(&preferred) {
            Ok(()) => (preferred, None),
            Err(error) => {
                ensure_writable_directory(&fallback).with_context(|| {
                    format!(
                        "收藏表情共享目录和回退目录均不可写；共享目录: {}, 回退目录: {}",
                        preferred.display(),
                        fallback.display()
                    )
                })?;
                (
                    fallback,
                    Some(format!(
                        "收藏表情目录 {} 不可写，已回退到 ica-native 数据目录：{error}",
                        preferred.display()
                    )),
                )
            }
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(StickerState {
                root,
                entries: Vec::new(),
                fallback_notice,
            })),
        })
    }

    pub fn root(&self) -> PathBuf {
        self.inner
            .read()
            .expect("sticker store poisoned")
            .root
            .clone()
    }

    pub fn fallback_notice(&self) -> Option<String> {
        self.inner
            .read()
            .expect("sticker store poisoned")
            .fallback_notice
            .clone()
    }

    pub fn entries(&self) -> Vec<StickerEntry> {
        self.inner
            .read()
            .expect("sticker store poisoned")
            .entries
            .clone()
    }

    /// Scan only the root directory. Existing Icalingua++ subdirectories are
    /// intentionally left untouched.
    pub fn refresh(&self, sort_newest_first: bool) -> Result<usize> {
        let root = self.root();
        let mut entries = Vec::new();
        for item in std::fs::read_dir(&root)
            .with_context(|| format!("读取收藏表情目录失败: {}", root.display()))?
        {
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    tracing::warn!("读取收藏表情目录项失败: {error}");
                    continue;
                }
            };
            let path = item.path();
            if !path.is_file() || path.file_name().is_some_and(is_temporary_name) {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!("跳过不可读收藏表情 {}: {error}", path.display());
                    continue;
                }
            };
            let detected = match detect_image(&bytes) {
                Ok(detected) => detected,
                Err(error) => {
                    tracing::warn!("跳过损坏收藏表情 {}: {error}", path.display());
                    continue;
                }
            };
            let modified_millis = item
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_millis());
            entries.push(StickerEntry {
                name: path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("sticker")
                    .to_string(),
                path,
                mime_type: detected.mime_type.to_string(),
                modified_millis,
            });
        }
        if sort_newest_first {
            entries.sort_by(|left, right| {
                right
                    .modified_millis
                    .cmp(&left.modified_millis)
                    .then_with(|| right.name.cmp(&left.name))
            });
        } else {
            entries.sort_by(|left, right| left.name.cmp(&right.name));
        }
        let count = entries.len();
        self.inner.write().expect("sticker store poisoned").entries = entries;
        Ok(count)
    }

    pub fn add_bytes(&self, bytes: &[u8]) -> Result<StickerEntry> {
        let detected = detect_image(bytes)?;
        // Serialize writes performed through this store so two simultaneous
        // favorites cannot choose the same timestamp/hash destination.
        let state = self.inner.write().expect("sticker store poisoned");
        let root = state.root.clone();
        ensure_writable_directory(&root)?;

        let digest = Sha256::digest(bytes);
        let hash = hex::encode(&digest[..8]);
        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut suffix = 0_u32;
        let destination = loop {
            let stem = if suffix == 0 {
                format!("{timestamp}_{hash}")
            } else {
                format!("{timestamp}_{hash}_{suffix}")
            };
            let candidate = root.join(format!("{stem}.{}", detected.extension));
            if !candidate.exists() {
                break candidate;
            }
            suffix = suffix.saturating_add(1);
        };
        let temporary = root.join(format!(
            ".{}.tmp-{}",
            destination
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("sticker"),
            std::process::id()
        ));
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .with_context(|| format!("创建收藏表情临时文件失败: {}", temporary.display()))?;
            if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                let _ = std::fs::remove_file(&temporary);
                return Err(error).context("写入收藏表情失败");
            }
        }
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error)
                .with_context(|| format!("提交收藏表情失败: {}", destination.display()));
        }

        let entry = StickerEntry {
            name: destination
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("sticker")
                .to_string(),
            path: destination,
            mime_type: detected.mime_type.to_string(),
            modified_millis: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis()),
        };
        Ok(entry)
    }

    pub fn read_entry(&self, entry: &StickerEntry) -> Result<Vec<u8>> {
        let root = self.root();
        anyhow::ensure!(
            entry.path.parent() == Some(root.as_path()),
            "收藏表情不属于当前目录: {}",
            entry.path.display()
        );
        std::fs::read(&entry.path)
            .with_context(|| format!("读取收藏表情失败: {}", entry.path.display()))
    }
}

pub fn detect_image(bytes: &[u8]) -> Result<DetectedImage> {
    let format = image::guess_format(bytes).context("无法识别图片格式")?;
    image::load_from_memory_with_format(bytes, format).context("图片内容损坏或不完整")?;
    let detected = match format {
        image::ImageFormat::Png => DetectedImage {
            extension: "png",
            mime_type: "image/png",
        },
        image::ImageFormat::Jpeg => DetectedImage {
            extension: "jpg",
            mime_type: "image/jpeg",
        },
        image::ImageFormat::Gif => DetectedImage {
            extension: "gif",
            mime_type: "image/gif",
        },
        image::ImageFormat::WebP => DetectedImage {
            extension: "webp",
            mime_type: "image/webp",
        },
        image::ImageFormat::Bmp => DetectedImage {
            extension: "bmp",
            mime_type: "image/bmp",
        },
        image::ImageFormat::Tiff => DetectedImage {
            extension: "tiff",
            mime_type: "image/tiff",
        },
        other => anyhow::bail!("不支持收藏的图片格式: {other:?}"),
    };
    Ok(detected)
}

fn is_temporary_name(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.') && name.to_string_lossy().contains(".tmp-")
}

fn ensure_writable_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("创建目录失败: {}", path.display()))?;
    let probe = path.join(format!(".ica-native-write-test-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .with_context(|| format!("目录不可写: {}", path.display()))?;
    std::fs::remove_file(&probe)
        .with_context(|| format!("清理目录写入测试失败: {}", probe.display()))?;
    Ok(())
}

fn default_icalingua_sticker_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("icalingua/stickers");
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(config_home).join("icalingua/stickers");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".config/icalingua/stickers");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/icalingua/stickers");
        }
    }
    std::env::temp_dir().join("icalingua/stickers")
}

#[cfg(test)]
mod tests {
    use super::{StickerStore, detect_image};

    #[test]
    fn detects_image_from_content_not_filename() {
        let png = include_bytes!("../../../assets/png/icon_512x512.png");
        let detected = detect_image(png).unwrap();
        assert_eq!(detected.extension, "png");
        assert_eq!(detected.mime_type, "image/png");
    }

    #[test]
    fn rejects_corrupt_data() {
        assert!(detect_image(b"not an image").is_err());
    }

    #[test]
    fn favorite_keeps_original_bytes_and_scans_only_root() {
        let unique = format!(
            "ica-native-sticker-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let root = std::env::temp_dir().join(unique);
        let config = crate::config::IcaCfg {
            sticker_path: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let config_store =
            crate::config::ConfigStore::from_config(config.clone(), root.join("test-config.toml"));
        let store = StickerStore::resolve(&config, config_store.paths()).unwrap();
        let original = include_bytes!("../../../assets/png/icon_512x512.png");

        let entry = store.add_bytes(original).unwrap();
        assert_eq!(store.read_entry(&entry).unwrap(), original);
        assert!(entry.name.ends_with(".png"));
        assert_eq!(store.refresh(false).unwrap(), 1);

        let nested = root.join("existing-icalingua-group");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("nested.png"), original).unwrap();
        assert_eq!(store.refresh(false).unwrap(), 1);

        std::fs::remove_dir_all(root).unwrap();
    }
}
