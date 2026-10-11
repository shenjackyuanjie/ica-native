//! 图片相关交互与后台任务：复制/保存图片、收藏表情、图片浏览器与表情加载。
//!
//! UI 线程只负责把用户意图转成 [`MediaTask`] 并 `spawn` 到 Tokio；任务完成后通过
//! [`MediaEvent`] 回到主线程更新提示。涉及磁盘/剪贴板/网络的耗时操作都放在后台，
//! 避免阻塞 egui。图片浏览器（`open_image_viewer_with_sources`）与“定位到消息”也在此。

use super::stickers::{StickerEntry, StickerStore, detect_image};
use super::{IcaApp, ImageViewerState};
use crate::app::PendingImage;
use crate::ica::types::RoomId;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 一张待操作图片的来源描述。
///
/// `url` 为必填；`room_id`/`message_id` 在图片来自某条聊天消息时填充，用于“定位到
/// 消息”与同会话图库切换；来自表情等无归属场景时为 `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSource {
    /// 图片地址（http(s)、`file://` 或 egui 的 `bytes://`）。
    pub url: String,
    /// 图片所属会话 ID（若有）。
    pub room_id: Option<RoomId>,
    /// 图片所属消息 ID（若有）。
    pub message_id: Option<String>,
}

/// `ImageSource` 的便捷构造。
impl ImageSource {
    /// 构造一条“来自某条聊天消息”的图片源，带会话与消息归属。
    pub fn message(url: String, room_id: RoomId, message_id: String) -> Self {
        Self {
            url,
            room_id: Some(room_id),
            message_id: Some(message_id),
        }
    }

    /// 构造一条无会话归属的图片源（如表情、头像）。
    pub fn url(url: String) -> Self {
        Self {
            url,
            room_id: None,
            message_id: None,
        }
    }
}

/// 用户在图片上触发的操作，由 `handle_image_action` 分发执行。
#[derive(Debug, Clone)]
pub enum ImageAction {
    /// 复制图片到系统剪贴板。
    CopyImage(ImageSource),
    /// 收藏为表情。
    AddSticker(ImageSource),
    /// 复制图片 URL 文本。
    CopyUrl(ImageSource),
    /// 在图片浏览器中打开（可带同会话图库）。
    Open(ImageSource),
    /// 保存到默认下载目录。
    Save(ImageSource),
    /// 另存为（弹出对话框选择路径）。
    SaveAs(ImageSource),
    /// 定位到图片所在的聊天消息。
    Locate(ImageSource),
}

/// `ImageAction` 的访问器。
impl ImageAction {
    /// 取出操作作用的图片源（所有变体都只包裹一个 `ImageSource`）。
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

/// 把图片相关交互挂到 `IcaApp` 上。
impl IcaApp {
    /// 执行一个图片操作：能同步完成的（复制 URL、定位、打开浏览器）直接做，
    /// 需要耗时 IO 的（复制图片、收藏、保存）转成 `MediaTask` 后台执行。
    pub fn handle_image_action(
        &mut self,
        ctx: &egui::Context,
        bridge_idx: usize,
        action: ImageAction,
    ) {
        match action {
            ImageAction::CopyUrl(source) => {
                ctx.copy_text(crate::image_loader::nt_image::resolved_url(
                    ctx,
                    &source.url,
                ));
                self.media_error = None;
                self.media_notice = Some("图片 URL 已复制".to_string());
            }
            ImageAction::Open(source) => {
                let gallery = if self.custom_chat.disable_img_swap_in_chat {
                    vec![source.clone()]
                } else {
                    source
                        .room_id
                        .and_then(|room_id| {
                            self.bridge_states
                                .get(bridge_idx)
                                .and_then(|session| session.conversation(room_id))
                                .map(|conversation| {
                                    image_sources_for_messages(room_id, &conversation.messages)
                                })
                        })
                        .unwrap_or_else(|| vec![source.clone()])
                };
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

    /// 打开图片浏览器，`current` 为当前图，`sources` 为可左右切换的图集。
    pub fn open_image_viewer_with_sources(
        &mut self,
        current: ImageSource,
        sources: Vec<ImageSource>,
    ) {
        self.image_viewer = Some(std::sync::Arc::new(std::sync::Mutex::new(
            ImageViewerState::with_sources(current, sources),
        )));
    }
}

/// 从一组消息中收集所有图片附件，构成图片浏览器可切换的图集。
///
/// 仅收录 `is_image_file_type` 判定为图片且 URL 非空的附件。
pub fn image_sources_for_messages(
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

/// 尝试取图片的已缓存原始字节，供后台任务避免重复下载。
///
/// 先查 image_loader 的原图缓存，再问 egui 的字节加载器；都没有则返回 `None`。
fn cached_image_bytes(ctx: &egui::Context, url: &str) -> Option<Arc<[u8]>> {
    if let Some(bytes) = crate::image_loader::cached_original_bytes(ctx, url) {
        return Some(bytes);
    }
    match ctx.try_load_bytes(url) {
        Ok(egui::load::BytesPoll::Ready { bytes, .. }) => Some(Arc::<[u8]>::from(bytes.as_ref())),
        _ => None,
    }
}

/// 需要在 Tokio 后台执行的媒体任务，完成后产出 [`MediaEvent`]。
#[derive(Debug)]
pub enum MediaTask {
    /// 复制图片到剪贴板；`cached` 为已缓存的原始字节（可避免重复下载）。
    CopyImage {
        /// 图片源。
        source: ImageSource,
        /// 预先取到的图片字节（若有）。
        cached: Option<Arc<[u8]>>,
    },
    /// 把图片收藏为表情，并按需刷新表情列表。
    AddSticker {
        /// 图片源。
        source: ImageSource,
        /// 预先取到的图片字节（若有）。
        cached: Option<Arc<[u8]>>,
        /// 目标表情存储。
        store: StickerStore,
        /// 刷新时是否把新表情排在最前。
        sort_newest_first: bool,
    },
    /// 保存图片；`save_as` 为 true 时弹对话框，否则存入默认下载目录。
    SaveImage {
        /// 图片源。
        source: ImageSource,
        /// 预先取到的图片字节（若有）。
        cached: Option<Arc<[u8]>>,
        /// 是否另存为。
        save_as: bool,
    },
    /// 重新扫描并刷新收藏表情列表。
    RefreshStickers {
        /// 目标表情存储。
        store: StickerStore,
        /// 刷新时是否把新表情排在最前。
        sort_newest_first: bool,
    },
    /// 读取某个收藏表情，并作为待发送图片加入指定会话。
    LoadSticker {
        /// 目标表情存储。
        store: StickerStore,
        /// 要读取的表情条目。
        entry: StickerEntry,
        /// 目标 bridge 的稳定标识。
        bridge_key: String,
        /// 目标会话 ID。
        room_id: RoomId,
    },
}

/// 媒体后台任务完成后回传给主线程的结果。
#[derive(Debug)]
pub enum MediaEvent {
    /// 任务成功，附带面向用户的提示文案。
    Completed(String),
    /// 任务失败，`operation` 为操作名，`error` 为原因。
    Failed {
        /// 失败的操作名（如“保存图片”）。
        operation: String,
        /// 具体错误信息。
        error: String,
    },
    /// 收藏表情已刷新，值为表情数量。
    StickersRefreshed(usize),
    /// 某个收藏表情已读取完成，作为待发送图片返回。
    StickerLoaded {
        /// 目标 bridge 的稳定标识。
        bridge_key: String,
        /// 目标会话 ID。
        room_id: RoomId,
        /// 读取到的待发送图片。
        image: PendingImage,
    },
}

/// `MediaTask` 的执行体。
impl MediaTask {
    /// 消费自身并执行对应后台任务，返回可回传主线程的 [`MediaEvent`]。
    ///
    /// 涉及文件写入/剪贴板的步骤用 `spawn_blocking` 移出异步 worker；任何错误都被
    /// 收敛为 `MediaEvent::Failed`，不会向上 panic。
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

/// 把 `Result<()>` 折叠成成功/失败两种 [`MediaEvent`]，统一文案拼装。
fn media_result(operation: &str, result: Result<()>, success: &str) -> MediaEvent {
    match result {
        Ok(()) => MediaEvent::Completed(success.to_string()),
        Err(error) => MediaEvent::Failed {
            operation: operation.to_string(),
            error: error.to_string(),
        },
    }
}

/// 取图片原始字节：优先用缓存，其次按 `file://` 读本地文件，最后走 HTTP 下载。
///
/// `bytes://` 表示 egui 已释放缓存的临时图，无法再取，直接报错让用户重新打开图片。
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

/// 把图片字节解码为 RGBA 并写入系统剪贴板。
fn copy_image_to_clipboard(bytes: &[u8]) -> Result<()> {
    let image = image::load_from_memory(bytes).context("图片解码失败")?;
    // DynamicImage 对动图格式使用第一帧，与系统剪贴板能力和 Icalingua++ 行为一致。
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

/// 保存图片字节：非“另存为”时优先写入默认下载目录，失败或用户选择另存为时弹对话框。
///
/// 返回 `Ok(Some(path))` 表示已保存，`Ok(None)` 表示用户取消。
fn save_image_bytes(bytes: &[u8], save_as: bool) -> Result<Option<PathBuf>> {
    // 串行化“保存图片”的对话框与落盘，避免并发任务弹出多个对话框或竞争同名文件。
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

/// 弹出“另存为”对话框，返回用户选择的路径（取消则 `None`）。
fn choose_save_path(default_name: &str, extension: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("图片", &[extension])
        .set_file_name(default_name)
        .save_file()
}

/// 原子写入图片：先写同目录临时文件再替换目标，避免半成品文件被其它程序读到。
///
/// Windows 上目标已存在时不能直接 rename 覆盖，改走 `replace_media_file_on_windows`。
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    /// 进程内自增计数，保证并发写入的临时文件名不冲突。
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

/// Windows 专用的原子替换：目标已存在时先备份再 rename，失败尽量回滚。
///
/// Windows 的 `rename` 不覆盖已存在目标，因此借助 `.ica-backup-<pid>` 备份中转。
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

/// 若路径已存在，则在文件名后追加 `-1`、`-2`… 找到第一个不冲突的路径。
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

/// 取默认下载目录：Windows 用 `%USERPROFILE%\Downloads`，类 Unix 用 `$HOME/Downloads`。
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
