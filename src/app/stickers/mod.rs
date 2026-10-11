//! 收藏表情（贴纸）存储：目录解析、扫描、增删与分类管理。
//!
//! 目录结构与 Icalingua++ 互通：根目录存放“默认”分类，一级子目录是其他分类。
//! 首选目录不可写时回退到 ica-native 数据目录，并通过 `fallback_notice`
//! 向界面提示；所有写入经存储内部的锁串行化，并用“临时文件 + 重命名”提交。

#[cfg(test)]
mod tests;

use crate::config::{ConfigPaths, IcaCfg};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 根目录直下文件所属的默认分类名。
const DEFAULT_CATEGORY: &str = "默认";

/// 表情选择器的页签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickerPickerTab {
    /// QQ 系统表情。
    #[default]
    QqFaces,
    /// 收藏表情。
    Favorites,
}

/// 一个收藏表情文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerEntry {
    /// 文件绝对路径。
    pub path: PathBuf,
    /// 文件名（含扩展名）。
    pub name: String,
    /// 所属分类名。
    pub category: String,
    /// MIME 类型（按内容探测，非扩展名猜测）。
    pub mime_type: String,
    /// 修改时间（Unix 毫秒），用于按时间排序。
    pub modified_millis: u128,
}

/// 收藏表情存储；内部状态经 `Arc<RwLock<_>>` 共享，可跨界面克隆使用。
#[derive(Debug, Clone)]
pub struct StickerStore {
    inner: Arc<RwLock<StickerState>>,
}

/// 存储的内部状态：根目录、已扫描条目与回退提示。
#[derive(Debug)]
struct StickerState {
    root: PathBuf,
    entries: Vec<StickerEntry>,
    fallback_notice: Option<String>,
}

/// 按内容探测出的图片格式信息。
#[derive(Debug, Clone, Copy)]
pub struct DetectedImage {
    /// 文件扩展名（不含点）。
    pub extension: &'static str,
    /// MIME 类型。
    pub mime_type: &'static str,
}

impl StickerStore {
    /// 构造一个“不可用”的存储：条目为空，仅携带回退提示。
    pub fn unavailable(root: PathBuf, error: impl ToString) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StickerState {
                root,
                entries: Vec::new(),
                fallback_notice: Some(format!("收藏表情目录不可用：{}", error.to_string())),
            })),
        }
    }

    /// 按配置解析收藏表情目录。
    ///
    /// 优先使用配置的 `sticker_path`，其次 Icalingua++ 的默认 stickers 目录；
    /// 都不可写时回退到数据目录下的 `stickers` 并记录提示。两个目录都不可用
    /// 才返回错误。
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

    /// 返回当前生效的根目录。
    pub fn root(&self) -> PathBuf {
        self.inner
            .read()
            .expect("收藏表情存储锁被污染")
            .root
            .clone()
    }

    /// 发生目录回退时的提示文本；无回退时为 `None`。
    pub fn fallback_notice(&self) -> Option<String> {
        self.inner
            .read()
            .expect("收藏表情存储锁被污染")
            .fallback_notice
            .clone()
    }

    /// 返回全部已扫描条目（拷贝）。
    pub fn entries(&self) -> Vec<StickerEntry> {
        self.inner
            .read()
            .expect("收藏表情存储锁被污染")
            .entries
            .clone()
    }

    /// 返回根目录和一级子目录中的分类；与 Icalingua++ 的 stickers 目录结构互通。
    pub fn categories(&self) -> Vec<String> {
        let state = self.inner.read().expect("收藏表情存储锁被污染");
        let mut categories = vec![DEFAULT_CATEGORY.to_string()];
        if let Ok(items) = std::fs::read_dir(&state.root) {
            for item in items.flatten() {
                let path = item.path();
                if path.is_dir()
                    && let Some(name) = path.file_name().and_then(OsStr::to_str)
                    && valid_category_name(name)
                {
                    categories.push(name.to_string());
                }
            }
        }
        for entry in &state.entries {
            if entry.category != DEFAULT_CATEGORY && !categories.contains(&entry.category) {
                categories.push(entry.category.clone());
            }
        }
        categories[1..].sort_unstable();
        categories.dedup();
        categories
    }

    /// 返回指定分类下的条目。
    pub fn entries_in_category(&self, category: &str) -> Vec<StickerEntry> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.category == category)
            .collect()
    }

    /// 扫描根目录及一级分类目录，避免递归读取意外的深层文件树。
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
            if path.is_file() {
                collect_sticker_entry(&item, DEFAULT_CATEGORY, &mut entries);
            } else if path.is_dir()
                && let Some(category) = path.file_name().and_then(OsStr::to_str)
                && valid_category_name(category)
                && let Ok(category_items) = std::fs::read_dir(&path)
            {
                for category_item in category_items.flatten() {
                    collect_sticker_entry(&category_item, category, &mut entries);
                }
            }
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
        self.inner.write().expect("收藏表情存储锁被污染").entries = entries;
        Ok(count)
    }

    /// 把图片字节写入默认分类，返回新条目。
    pub fn add_bytes(&self, bytes: &[u8]) -> Result<StickerEntry> {
        self.add_bytes_to_category(bytes, DEFAULT_CATEGORY)
    }

    /// 把图片字节写入指定分类。
    ///
    /// 文件名取“毫秒时间戳 + 内容哈希前 8 位”，重名时追加序号；
    /// 先写隐藏临时文件再重命名，避免收藏中断留下半个文件。
    pub fn add_bytes_to_category(&self, bytes: &[u8], category: &str) -> Result<StickerEntry> {
        let detected = detect_image(bytes)?;
        // 串行化经由此存储执行的写入，避免两个并发收藏选择相同的时间戳/哈希目标。
        let state = self.inner.write().expect("收藏表情存储锁被污染");
        let root = state.root.clone();
        let category = normalize_category_name(category)?;
        let destination_dir = category_directory(&root, &category);
        ensure_writable_directory(&destination_dir)?;

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
            let candidate = destination_dir.join(format!("{stem}.{}", detected.extension));
            if !candidate.exists() {
                break candidate;
            }
            suffix = suffix.saturating_add(1);
        };
        let temporary = destination_dir.join(format!(
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
            category,
            path: destination,
            mime_type: detected.mime_type.to_string(),
            modified_millis: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis()),
        };
        Ok(entry)
    }

    /// 读取一个条目的文件内容；拒绝根目录之外的路径，防止越权读取。
    pub fn read_entry(&self, entry: &StickerEntry) -> Result<Vec<u8>> {
        let root = self.root();
        anyhow::ensure!(
            is_sticker_path_in_root(&entry.path, &root),
            "收藏表情不属于当前目录: {}",
            entry.path.display()
        );
        std::fs::read(&entry.path)
            .with_context(|| format!("读取收藏表情失败: {}", entry.path.display()))
    }

    /// 创建新分类（“默认”分类无需创建）。
    pub fn create_category(&self, category: &str) -> Result<()> {
        let root = self.root();
        let category = normalize_category_name(category)?;
        anyhow::ensure!(category != DEFAULT_CATEGORY, "“默认”分类无需创建");
        ensure_writable_directory(&category_directory(&root, &category))
    }

    /// 把条目移动到指定分类；目标存在同名文件时自动追加序号。
    pub fn move_entry(&self, entry: &StickerEntry, category: &str) -> Result<()> {
        let root = self.root();
        anyhow::ensure!(
            is_sticker_path_in_root(&entry.path, &root),
            "收藏表情不属于当前目录: {}",
            entry.path.display()
        );
        let category = normalize_category_name(category)?;
        let destination_dir = category_directory(&root, &category);
        ensure_writable_directory(&destination_dir)?;
        if entry.path.parent() == Some(destination_dir.as_path()) {
            return Ok(());
        }
        let file_name = entry
            .path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("收藏表情缺少文件名"))?;
        let mut destination = destination_dir.join(file_name);
        let mut duplicate = 1_u32;
        while destination.exists() {
            let stem = entry
                .path
                .file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or("sticker");
            let extension = entry
                .path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or("png");
            destination = destination_dir.join(format!("{stem}_{duplicate}.{extension}"));
            duplicate = duplicate.saturating_add(1);
        }
        std::fs::rename(&entry.path, &destination).with_context(|| {
            format!(
                "移动收藏表情失败: {} -> {}",
                entry.path.display(),
                destination.display()
            )
        })?;
        Ok(())
    }
}

/// 把一个目录项收集为表情条目。
///
/// 跳过非文件、写入中的临时文件、不可读文件与损坏图片，并记录警告。
fn collect_sticker_entry(
    item: &std::fs::DirEntry,
    category: &str,
    entries: &mut Vec<StickerEntry>,
) {
    let path = item.path();
    if !path.is_file() || path.file_name().is_some_and(is_temporary_name) {
        return;
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!("跳过不可读收藏表情 {}: {error}", path.display());
            return;
        }
    };
    let detected = match detect_image(&bytes) {
        Ok(detected) => detected,
        Err(error) => {
            tracing::warn!("跳过损坏收藏表情 {}: {error}", path.display());
            return;
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
        category: category.to_string(),
        path,
        mime_type: detected.mime_type.to_string(),
        modified_millis,
    });
}

/// 校验分类名：非空、非 `.`/`..`、不含路径分隔符与控制字符。
fn valid_category_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(|character| character.is_control())
}

/// 规范化用户输入的分类名（去空白并校验长度 ≤ 32）。
fn normalize_category_name(category: &str) -> Result<String> {
    let category = category.trim();
    anyhow::ensure!(valid_category_name(category), "表情分类名称无效");
    anyhow::ensure!(category.chars().count() <= 32, "表情分类名称最多 32 个字符");
    Ok(category.to_string())
}

/// 返回分类对应的目录：“默认”分类即根目录本身。
fn category_directory(root: &Path, category: &str) -> PathBuf {
    if category == DEFAULT_CATEGORY {
        root.to_path_buf()
    } else {
        root.join(category)
    }
}

/// 判断条目路径是否位于根目录或其一级子目录内。
fn is_sticker_path_in_root(path: &Path, root: &Path) -> bool {
    path.parent() == Some(root) || path.parent().and_then(Path::parent) == Some(root)
}

/// 探测字节内容的图片格式并校验可解码；仅支持常见静态图片格式。
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

/// 判断是否为写入中的临时文件名（隐藏且包含 `.tmp-`）。
fn is_temporary_name(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.') && name.to_string_lossy().contains(".tmp-")
}

/// 确保目录存在且可写：创建目录后写入并删除一个探针文件。
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

/// 返回 Icalingua++ 的默认收藏表情目录（按平台推导），失败时回退临时目录。
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
