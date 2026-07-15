use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::app::PendingImage;
use crate::ica::types::RoomId;

use super::stickers::{StickerEntry, StickerStore, detect_image};
use super::{IcaApp, ImageViewerState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSource {
    pub url: String,
    pub room_id: Option<RoomId>,
    pub message_id: Option<String>,
}

impl ImageSource {
    pub fn message(url: String, room_id: RoomId, message_id: String) -> Self {
        Self {
            url,
            room_id: Some(room_id),
            message_id: Some(message_id),
        }
    }

    pub fn url(url: String) -> Self {
        Self {
            url,
            room_id: None,
            message_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ImageAction {
    CopyImage(ImageSource),
    AddSticker(ImageSource),
    CopyUrl(ImageSource),
    Open(ImageSource),
    Save(ImageSource),
    SaveAs(ImageSource),
    Locate(ImageSource),
}

impl ImageAction {
    pub fn source(&self) -> &ImageSource {
        match self {
            Self::CopyImage(source)
            | Self::AddSticker(source)
            | Self::CopyUrl(source)
            | Self::Open(source)
            | Self::Save(source)
            | Self::SaveAs(source)
            | Self::Locate(source) => source,
        }
    }
}

impl IcaApp {
    pub(super) fn handle_image_action(
        &mut self,
        ctx: &egui::Context,
        bridge_idx: usize,
        action: ImageAction,
    ) {
        match action {
            ImageAction::CopyUrl(source) => {
                ctx.copy_text(source.url);
                self.media_error = None;
                self.media_notice = Some("图片 URL 已复制".to_string());
            }
            ImageAction::Open(source) => {
                let gallery = source
                    .room_id
                    .and_then(|room_id| {
                        self.bridge_states
                            .get(bridge_idx)
                            .and_then(|session| session.conversation(room_id))
                            .map(|conversation| {
                                image_sources_for_messages(room_id, &conversation.messages)
                            })
                    })
                    .unwrap_or_else(|| vec![source.clone()]);
                self.open_image_viewer_with_sources(source, gallery);
            }
            ImageAction::CopyImage(source) => {
                let cached = cached_image_bytes(ctx, &source.url);
                self.spawn_media_task(ctx, MediaTask::CopyImage { source, cached });
            }
            ImageAction::AddSticker(source) => {
                let cached = cached_image_bytes(ctx, &source.url);
                self.spawn_media_task(
                    ctx,
                    MediaTask::AddSticker {
                        source,
                        cached,
                        store: self.sticker_store.clone(),
                        sort_newest_first: self.custom_chat.sort_stickers_by_time,
                    },
                );
            }
            ImageAction::Save(source) => {
                let cached = cached_image_bytes(ctx, &source.url);
                self.spawn_media_task(
                    ctx,
                    MediaTask::SaveImage {
                        source,
                        cached,
                        save_as: false,
                    },
                );
            }
            ImageAction::SaveAs(source) => {
                let cached = cached_image_bytes(ctx, &source.url);
                self.spawn_media_task(
                    ctx,
                    MediaTask::SaveImage {
                        source,
                        cached,
                        save_as: true,
                    },
                );
            }
            ImageAction::Locate(source) => {
                let (Some(room_id), Some(message_id)) = (source.room_id, source.message_id) else {
                    self.media_error = Some("这张图片没有可定位的聊天消息".to_string());
                    return;
                };
                if bridge_idx >= self.bridge_states.len() {
                    self.media_error = Some("图片所属 bridge 已关闭".to_string());
                    return;
                }
                if self.active_bridge_idx != Some(bridge_idx) {
                    self.switch_active_bridge(bridge_idx);
                }
                self.select_active_room(room_id);
                let conversation = self.bridge_states[bridge_idx].conversation_mut(room_id);
                conversation.scroll_to_message_id = Some(message_id);
                conversation.scroll_to_message_attempts = 0;
                self.image_viewer = None;
                self.media_error = None;
                self.media_notice = Some("正在定位图片消息".to_string());
            }
        }
    }

    pub(super) fn open_image_viewer_with_sources(
        &mut self,
        current: ImageSource,
        sources: Vec<ImageSource>,
    ) {
        self.image_viewer = Some(std::sync::Arc::new(std::sync::Mutex::new(
            ImageViewerState::with_sources(current, sources),
        )));
    }
}

pub(super) fn image_sources_for_messages(
    room_id: RoomId,
    messages: &[crate::ica::types::message::Message],
) -> Vec<ImageSource> {
    messages
        .iter()
        .flat_map(|message| {
            message
                .files
                .iter()
                .filter(|file| {
                    super::chat::is_image_file_type(&file.file_type) && !file.url.is_empty()
                })
                .map(move |file| {
                    ImageSource::message(file.url.clone(), room_id, message.msg_id.clone())
                })
        })
        .collect()
}

fn cached_image_bytes(ctx: &egui::Context, url: &str) -> Option<Arc<[u8]>> {
    if let Some(bytes) = crate::image_loader::cached_original_bytes(ctx, url) {
        return Some(bytes);
    }
    match ctx.try_load_bytes(url) {
        Ok(egui::load::BytesPoll::Ready { bytes, .. }) => Some(Arc::<[u8]>::from(bytes.as_ref())),
        _ => None,
    }
}

#[derive(Debug)]
pub enum MediaTask {
    CopyImage {
        source: ImageSource,
        cached: Option<Arc<[u8]>>,
    },
    AddSticker {
        source: ImageSource,
        cached: Option<Arc<[u8]>>,
        store: StickerStore,
        sort_newest_first: bool,
    },
    SaveImage {
        source: ImageSource,
        cached: Option<Arc<[u8]>>,
        save_as: bool,
    },
    RefreshStickers {
        store: StickerStore,
        sort_newest_first: bool,
    },
    LoadSticker {
        store: StickerStore,
        entry: StickerEntry,
        bridge_key: String,
        room_id: RoomId,
    },
}

#[derive(Debug)]
pub enum MediaEvent {
    Completed(String),
    Failed {
        operation: String,
        error: String,
    },
    StickersRefreshed(usize),
    StickerLoaded {
        bridge_key: String,
        room_id: RoomId,
        image: PendingImage,
    },
}

impl MediaTask {
    pub async fn run(self) -> MediaEvent {
        match self {
            Self::CopyImage { source, cached } => {
                let result = async {
                    let bytes = load_source_bytes(&source, cached).await?;
                    tokio::task::spawn_blocking(move || copy_image_to_clipboard(&bytes))
                        .await
                        .context("复制图片后台任务异常")??;
                    Result::<()>::Ok(())
                }
                .await;
                media_result("复制图片", result, "图片已复制到剪贴板")
            }
            Self::AddSticker {
                source,
                cached,
                store,
                sort_newest_first,
            } => {
                let result = async {
                    let bytes = load_source_bytes(&source, cached).await?;
                    let store_for_write = store.clone();
                    tokio::task::spawn_blocking(move || store_for_write.add_bytes(&bytes))
                        .await
                        .context("收藏表情后台任务异常")??;
                    let store_for_refresh = store.clone();
                    tokio::task::spawn_blocking(move || {
                        store_for_refresh.refresh(sort_newest_first)
                    })
                    .await
                    .context("刷新收藏表情后台任务异常")??;
                    Result::<()>::Ok(())
                }
                .await;
                media_result("添加为表情", result, "已添加到收藏表情")
            }
            Self::SaveImage {
                source,
                cached,
                save_as,
            } => {
                let result = async {
                    let bytes = load_source_bytes(&source, cached).await?;
                    tokio::task::spawn_blocking(move || save_image_bytes(&bytes, save_as))
                        .await
                        .context("保存图片后台任务异常")?
                }
                .await;
                match result {
                    Ok(Some(path)) => {
                        MediaEvent::Completed(format!("图片已保存到 {}", path.display()))
                    }
                    Ok(None) => MediaEvent::Completed("已取消保存图片".to_string()),
                    Err(error) => MediaEvent::Failed {
                        operation: "保存图片".to_string(),
                        error: error.to_string(),
                    },
                }
            }
            Self::RefreshStickers {
                store,
                sort_newest_first,
            } => {
                match tokio::task::spawn_blocking(move || store.refresh(sort_newest_first)).await {
                    Ok(Ok(count)) => MediaEvent::StickersRefreshed(count),
                    Ok(Err(error)) => MediaEvent::Failed {
                        operation: "刷新收藏表情".to_string(),
                        error: error.to_string(),
                    },
                    Err(error) => MediaEvent::Failed {
                        operation: "刷新收藏表情".to_string(),
                        error: error.to_string(),
                    },
                }
            }
            Self::LoadSticker {
                store,
                entry,
                bridge_key,
                room_id,
            } => match tokio::task::spawn_blocking(move || {
                let bytes = store.read_entry(&entry)?;
                let detected = detect_image(&bytes)?;
                Ok::<_, anyhow::Error>(PendingImage::new(
                    entry.name,
                    detected.mime_type.to_string(),
                    bytes,
                ))
            })
            .await
            {
                Ok(Ok(image)) => MediaEvent::StickerLoaded {
                    bridge_key,
                    room_id,
                    image,
                },
                Ok(Err(error)) => MediaEvent::Failed {
                    operation: "读取收藏表情".to_string(),
                    error: error.to_string(),
                },
                Err(error) => MediaEvent::Failed {
                    operation: "读取收藏表情".to_string(),
                    error: error.to_string(),
                },
            },
        }
    }
}

fn media_result(operation: &str, result: Result<()>, success: &str) -> MediaEvent {
    match result {
        Ok(()) => MediaEvent::Completed(success.to_string()),
        Err(error) => MediaEvent::Failed {
            operation: operation.to_string(),
            error: error.to_string(),
        },
    }
}

async fn load_source_bytes(source: &ImageSource, cached: Option<Arc<[u8]>>) -> Result<Arc<[u8]>> {
    if let Some(bytes) = cached {
        return Ok(bytes);
    }
    if source.url.starts_with("bytes://") {
        anyhow::bail!("图片原始字节已从缓存释放，请重新打开图片后再试");
    }
    if let Some(path) = source.url.strip_prefix("file://") {
        #[cfg(windows)]
        let path = PathBuf::from(path.trim_start_matches('/'));
        #[cfg(not(windows))]
        let path = PathBuf::from(path);
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .context("读取本地图片后台任务异常")??;
        return Ok(bytes.into());
    }
    let response = reqwest::get(&source.url)
        .await
        .with_context(|| format!("下载图片失败: {}", source.url))?
        .error_for_status()
        .with_context(|| format!("下载图片返回错误状态: {}", source.url))?;
    let bytes = response.bytes().await.context("读取下载图片内容失败")?;
    Ok(Arc::<[u8]>::from(bytes.as_ref()))
}

fn copy_image_to_clipboard(bytes: &[u8]) -> Result<()> {
    let image = image::load_from_memory(bytes).context("图片解码失败")?;
    // DynamicImage uses the first frame for animated formats, matching system
    // clipboard capabilities and Icalingua++ behavior.
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut clipboard = arboard::Clipboard::new().context("无法打开系统剪贴板")?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(rgba.into_raw()),
        })
        .context("系统剪贴板拒绝写入图片")?;
    Ok(())
}

fn save_image_bytes(bytes: &[u8], save_as: bool) -> Result<Option<PathBuf>> {
    static SAVE_LOCK: Mutex<()> = Mutex::new(());
    let _save_guard = SAVE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let detected = detect_image(bytes)?;
    let default_name = format!(
        "ica-image-{}.{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        detected.extension
    );
    if !save_as && let Some(downloads) = default_download_dir() {
        let destination = unique_path(downloads.join(&default_name));
        match std::fs::create_dir_all(&downloads)
            .map_err(anyhow::Error::from)
            .and_then(|()| atomic_write(&destination, bytes))
        {
            Ok(()) => return Ok(Some(destination)),
            Err(error) => {
                tracing::warn!("下载目录不可用，转为另存为: {error}");
            }
        }
    }

    let destination = choose_save_path(&default_name, detected.extension);
    let Some(mut destination) = destination else {
        return Ok(None);
    };
    destination.set_extension(detected.extension);
    atomic_write(&destination, bytes)?;
    Ok(Some(destination))
}

fn choose_save_path(default_name: &str, extension: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("图片", &[extension])
        .set_file_name(default_name)
        .save_file()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

    let parent = path.parent().context("保存路径缺少父目录")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image"),
        std::process::id(),
        NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("写入图片临时文件失败: {}", temporary.display()))?;

    #[cfg(not(windows))]
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("提交图片文件失败: {}", path.display()));
    }

    #[cfg(windows)]
    replace_media_file_on_windows(path, &temporary)?;
    Ok(())
}

#[cfg(windows)]
fn replace_media_file_on_windows(path: &Path, temporary: &Path) -> Result<()> {
    if !path.exists() {
        return std::fs::rename(temporary, path)
            .with_context(|| format!("提交图片文件失败: {}", path.display()));
    }

    let backup = path.with_extension(format!("ica-backup-{}", std::process::id()));
    if backup.exists() {
        std::fs::remove_file(&backup)
            .with_context(|| format!("清理旧图片备份失败: {}", backup.display()))?;
    }
    std::fs::rename(path, &backup)
        .with_context(|| format!("备份已有图片失败: {}", path.display()))?;
    if let Err(error) = std::fs::rename(temporary, path) {
        let _ = std::fs::rename(&backup, path);
        let _ = std::fs::remove_file(temporary);
        return Err(error).with_context(|| format!("提交图片文件失败: {}", path.display()));
    }
    if let Err(error) = std::fs::remove_file(&backup) {
        tracing::warn!("清理图片备份 {} 失败: {error}", backup.display());
    }
    Ok(())
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("image");
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("png");
    for suffix in 1_u32.. {
        let candidate = parent.join(format!("{stem}-{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn default_download_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|home| home.join("Downloads"));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Downloads"));
    }
    #[allow(unreachable_code)]
    None
}
