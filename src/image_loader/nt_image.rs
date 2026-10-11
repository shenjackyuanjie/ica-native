//! QQ NT 图片失败恢复：通过图片所属 Bridge 刷新 rkey，并最多重试一次。
//! 普通消息、回复、合并转发和预览共用字节加载器，不把刷新逻辑分散到 UI。
//!
//! 该模块位于 `crate::image_loader` 之下，实现 egui 的 `BytesLoader`：专门识别
//! QQ NT 的 `multimedia.nt.qq.com.cn` / `gchat.qpic.cn` 下载地址。这类地址带易过期的
//! rkey，失效时经由所属 bridge 的 `RefreshImageUrl` 命令换新地址，并用新地址重试一次。
//! 通过“URL -> bridge”绑定表判定归属，避免依赖当前选中的账号或窗口。

#[cfg(test)]
#[path = "tests/nt_image.rs"]
mod tests;

use crate::ica::{BridgeEvent, BridgeHandle, IcaCommand};
use egui::{
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
    mutex::Mutex,
};
use lru::LruCache;
use serde_json::Value;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, oneshot};

/// 单张图片下载的字节上限（64 MiB）。
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
/// 下载失败后的冷却时间：期间直接复用缓存错误，避免频繁重试。
const ERROR_RETRY_DELAY: Duration = Duration::from_secs(60);
/// 下载条目 LRU 的最大数量。
const MAX_ENTRIES: usize = 256;
/// URL -> bridge 绑定表与已解析地址表的最大数量。
const MAX_BINDINGS: usize = 8192;

/// 一张 QQ NT 图片的身份：由 URL 中解析出的 `fileid` 与 `appid`。
///
/// 用于在刷新地址后校验“换回来的还是同一张图”，并作为刷新命令的参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtImageIdentity {
    /// 文件 ID（`fileid` 查询参数）。
    pub file_id: String,
    /// 应用 ID（`appid` 查询参数），仅接受 `1406` / `1407`。
    pub app_id: String,
}

/// `NtImageIdentity` 的 URL 解析。
impl NtImageIdentity {
    /// 从一个 https URL 解析图片身份。
    ///
    /// 仅接受 `multimedia.nt.qq.com.cn` / `gchat.qpic.cn` 的 `/download`、无用户名/密码、
    /// 端口为 443（或缺省）的地址；`fileid` 缺失或 `appid` 不在白名单时返回 `None`。
    pub fn from_url(uri: &str) -> Option<Self> {
        let url = reqwest::Url::parse(uri).ok()?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("multimedia.nt.qq.com.cn" | "gchat.qpic.cn")
            )
            || url.path() != "/download"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some_and(|port| port != 443)
        {
            return None;
        }
        let mut file_id = None;
        let mut app_id = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "fileid" if file_id.is_none() => file_id = Some(value.into_owned()),
                "appid" if app_id.is_none() => app_id = Some(value.into_owned()),
                "fileid" | "appid" => return None,
                _ => {}
            }
        }
        let file_id = file_id.filter(|value| !value.is_empty())?;
        let app_id = app_id.unwrap_or_else(|| "1407".into());
        matches!(app_id.as_str(), "1406" | "1407").then_some(Self { file_id, app_id })
    }
}

/// 一次成功下载得到的图片字节及其最终 URL。
#[derive(Clone)]
struct DownloadedImage {
    /// 图片字节。
    bytes: Arc<[u8]>,
    /// 实际下载所用的 URL（可能是刷新后的新地址）。
    url: String,
}

/// 某个 URI 的下载状态机条目。
#[derive(Clone)]
enum Entry {
    /// 正在下载，值为代际编号。
    Pending(u64),
    /// 已下载完成。
    Ready(DownloadedImage),
    /// 最近一次失败，冷却到 `retry_after` 之前不再重试。
    Failed { error: String, retry_after: Instant },
}

/// NT 图片加载器的内部状态。
struct State {
    /// URL -> 所属 bridge 的绑定表。
    // 同一 URL 出现在多个 Bridge 时无法凭 URL 判定来源；None 表示歧义，禁止刷新。
    bindings: LruCache<String, Option<BridgeHandle>>,
    /// URI -> 下载条目。
    entries: LruCache<String, Entry>,
    /// URI -> 刷新后实际使用的新地址。
    resolved: LruCache<String, String>,
    /// 下一个代际编号。
    next_generation: u64,
    /// 下载条目的字节上限。
    max_bytes: usize,
}

/// `State` 的字节统计与下载结果提交。
impl State {
    /// 统计当前下载条目占用的近似字节（就绪图片字节 + 错误字符串）。
    fn bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, entry)| match entry {
                Entry::Ready(image) => image.bytes.len(),
                Entry::Failed { error, .. } => error.len(),
                Entry::Pending(_) => 0,
            })
            .sum()
    }

    /// 后台任务提交下载结果：代际匹配才写入，并按字节上限淘汰最旧条目。
    fn complete(
        &mut self,
        uri: &str,
        generation: u64,
        result: Result<DownloadedImage, String>,
    ) -> bool {
        if !matches!(self.entries.peek(uri), Some(Entry::Pending(current)) if *current == generation)
        {
            return false;
        }
        let entry = match result {
            Ok(image) => {
                self.resolved.put(uri.to_string(), image.url.clone());
                Entry::Ready(image)
            }
            Err(error) => Entry::Failed {
                error,
                retry_after: Instant::now() + ERROR_RETRY_DELAY,
            },
        };
        self.entries.put(uri.to_string(), entry);
        while self.bytes() > self.max_bytes && self.entries.len() > 1 {
            self.entries.pop_lru();
        }
        true
    }
}

/// QQ NT 图片字节加载器（实现 egui `BytesLoader`）。
pub struct NtImageLoader {
    /// 内部状态。
    state: Arc<Mutex<State>>,
    /// 用于跑下载/刷新异步任务的 tokio 运行时句柄。
    runtime: tokio::runtime::Handle,
    /// 图片下载客户端。
    client: reqwest::Client,
    /// 限制并发下载数量的信号量。
    slots: Arc<Semaphore>,
}

/// 返回本 loader 实例在 `egui` 临时数据中的存储键。
fn loader_id() -> egui::Id {
    egui::Id::new("ica_nt_image_bytes_loader")
}

/// `NtImageLoader` 的安装与事件观察。
impl NtImageLoader {
    /// 创建加载器、注册为 bytes loader 并存入 egui 临时数据，返回其 `Arc`。
    ///
    /// - 参数 `runtime`：跑下载/刷新异步任务的 tokio 句柄；
    /// - 参数 `max_bytes`：下载条目的字节上限，`0` 表示用内置默认值。
    pub fn install(
        ctx: &egui::Context,
        runtime: tokio::runtime::Handle,
        max_bytes: u64,
    ) -> Arc<Self> {
        let loader = Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                bindings: LruCache::new(NonZeroUsize::new(MAX_BINDINGS).unwrap()),
                entries: LruCache::new(NonZeroUsize::new(MAX_ENTRIES).unwrap()),
                resolved: LruCache::new(NonZeroUsize::new(MAX_BINDINGS).unwrap()),
                next_generation: 0,
                max_bytes: if max_bytes == 0 {
                    MAX_DOWNLOAD_BYTES
                } else {
                    usize::try_from(max_bytes).unwrap_or(usize::MAX)
                },
            })),
            runtime,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .expect("创建图片下载客户端失败"),
            slots: Arc::new(Semaphore::new(6)),
        });
        ctx.add_bytes_loader(loader.clone());
        ctx.data_mut(|data| data.insert_temp(loader_id(), loader.clone()));
        loader
    }

    /// 在事件交给 GUI 前登记图片归属，后台恢复不依赖当前选中的账号或窗口。
    pub fn observe(&self, event: &BridgeEvent, handles: &HashMap<String, BridgeHandle>) {
        let Some(bridge) = handles.get(&event.bridge_key) else {
            return;
        };
        let mut urls = Vec::new();
        collect_image_urls(event.payload(), 0, &mut urls);
        let mut state = self.state.lock();
        for url in urls {
            match state.bindings.peek(&url) {
                Some(Some(previous)) if previous.bridge_key() != bridge.bridge_key() => {
                    state.bindings.put(url, None);
                }
                Some(_) => {}
                None => {
                    state.bindings.put(url, Some(bridge.clone()));
                }
            }
        }
    }
}

/// 递归遍历事件 JSON，收集其中所有可识别的 NT 图片 URL（深度与数量受限）。
fn collect_image_urls(value: &Value, depth: usize, urls: &mut Vec<String>) {
    if depth > 32 || urls.len() >= MAX_BINDINGS {
        return;
    }
    match value {
        Value::String(uri) if NtImageIdentity::from_url(uri).is_some() => urls.push(uri.clone()),
        Value::Array(values) => {
            for value in values {
                collect_image_urls(value, depth + 1, urls);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_image_urls(value, depth + 1, urls);
            }
        }
        _ => {}
    }
}

/// 取某 URI 刷新后的实际地址；未刷新过则原样返回。供上层展示/复用。
pub fn resolved_url(ctx: &egui::Context, uri: &str) -> String {
    let loader = ctx.data(|data| data.get_temp::<Arc<NtImageLoader>>(loader_id()));
    loader
        .and_then(|loader| loader.state.lock().resolved.get(uri).cloned())
        .unwrap_or_else(|| uri.to_string())
}

/// 为 `NtImageLoader` 实现 egui `BytesLoader`。
impl BytesLoader for NtImageLoader {
    /// 返回本 bytes loader 的 ID。
    fn id(&self) -> &str {
        "ica_native_nt_image_bytes"
    }

    /// egui 请求字节时的入口：非 NT 图片让给其它 loader；
    /// 命中就绪/失败冷却直接复用，否则登记 pending 并异步下载（带恢复）。
    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        let uri = super::decode::normalize_uri(uri);
        let Some(identity) = NtImageIdentity::from_url(uri) else {
            return Err(LoadError::NotSupported);
        };
        let mut state = self.state.lock();
        if let Some(entry) = state.entries.get(uri) {
            match entry {
                Entry::Pending(_) => return Ok(BytesPoll::Pending { size: None }),
                Entry::Ready(image) => {
                    return Ok(BytesPoll::Ready {
                        size: None,
                        bytes: Bytes::Shared(image.bytes.clone()),
                        mime: None,
                    });
                }
                Entry::Failed { error, retry_after } if *retry_after > Instant::now() => {
                    let delay = retry_after.saturating_duration_since(Instant::now());
                    let error = error.clone();
                    drop(state);
                    ctx.request_repaint_after(delay);
                    return Err(LoadError::Loading(error));
                }
                Entry::Failed { .. } => {}
            }
        }
        let Some(bridge) = state.bindings.get(uri).cloned().flatten() else {
            return Err(LoadError::NotSupported);
        };
        let download_uri = state
            .resolved
            .get(uri)
            .cloned()
            .unwrap_or_else(|| uri.to_string());
        state.next_generation = state.next_generation.wrapping_add(1);
        let generation = state.next_generation;
        state
            .entries
            .put(uri.to_string(), Entry::Pending(generation));
        drop(state);

        let state = self.state.clone();
        let client = self.client.clone();
        let slots = self.slots.clone();
        let ctx = ctx.clone();
        let uri = uri.to_string();
        self.runtime.spawn(async move {
            let Ok(_slot) = slots.acquire_owned().await else { return; };
            if !matches!(state.lock().entries.peek(&uri), Some(Entry::Pending(current)) if *current == generation) {
                return;
            }
            let result = download_with_recovery(&client, &bridge, &download_uri, &identity).await;
            let repaint = state.lock().complete(&uri, generation, result);
            if repaint { ctx.request_repaint(); }
        });
        Ok(BytesPoll::Pending { size: None })
    }

    /// 忘记某 URI 的下载条目。
    fn forget(&self, uri: &str) {
        self.state
            .lock()
            .entries
            .pop(super::decode::normalize_uri(uri));
    }
    /// 清空所有下载条目。
    fn forget_all(&self) {
        self.state.lock().entries.clear();
    }

    /// 返回下载条目占用的近似字节。
    fn byte_size(&self) -> usize {
        self.state.lock().bytes()
    }

    /// 是否还有正在下载的条目。
    fn has_pending(&self) -> bool {
        self.state
            .lock()
            .entries
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Pending(_)))
    }
}

/// 带恢复的下载：先直下，失败则经 bridge 刷新 rkey 并用新地址重试一次。
async fn download_with_recovery(
    client: &reqwest::Client,
    bridge: &BridgeHandle,
    uri: &str,
    identity: &NtImageIdentity,
) -> Result<DownloadedImage, String> {
    recover_image(
        uri,
        identity,
        |url| async move { download_image(client, &url).await },
        || async move {
            let (result_tx, result_rx) = oneshot::channel();
            bridge
                .send(IcaCommand::RefreshImageUrl {
                    file_id: identity.file_id.clone(),
                    app_id: identity.app_id.clone(),
                    result_tx,
                })
                .map_err(|_| "图片加载失败，无法请求 Bridge 刷新地址".to_string())?;
            tokio::time::timeout(Duration::from_secs(15), result_rx)
                .await
                .map_err(|_| "图片地址刷新超时，请稍后重试".to_string())?
                .map_err(|_| "图片地址刷新请求已取消".to_string())?
        },
    )
    .await
}

/// 将有副作用的下载/刷新注入同一恢复流程，以验证真实的重试上限和目标校验。
async fn recover_image<F, FF, R, RF>(
    uri: &str,
    identity: &NtImageIdentity,
    mut fetch: F,
    refresh: R,
) -> Result<DownloadedImage, String>
where
    F: FnMut(String) -> FF,
    FF: std::future::Future<Output = Result<Arc<[u8]>, String>>,
    R: FnOnce() -> RF,
    RF: std::future::Future<Output = Result<String, String>>,
{
    if let Ok(bytes) = fetch(uri.to_string()).await {
        return Ok(DownloadedImage {
            bytes,
            url: uri.to_string(),
        });
    }
    let refreshed = refresh().await?;
    if NtImageIdentity::from_url(&refreshed).as_ref() != Some(identity) {
        return Err("Bridge 返回的图片地址不匹配，已拒绝加载".to_string());
    }
    if refreshed == uri {
        return Err("图片仍不可用，Bridge 未提供新的图片地址".to_string());
    }
    let bytes = fetch(refreshed.clone())
        .await
        .map_err(|error| format!("图片地址已刷新，但加载仍失败：{error}"))?;
    Ok(DownloadedImage {
        bytes,
        url: refreshed,
    })
}

/// 下载单张图片字节，带大小上限与“HTTP 200 但内容非图片”的兜底校验。
async fn download_image(client: &reqwest::Client, uri: &str) -> Result<Arc<[u8]>, String> {
    let mut response = client
        .get(uri)
        .send()
        .await
        .map_err(|error| format!("图片网络请求失败：{}", error.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        // 不暴露带 rkey 的 URL 或远端响应正文，也不将所有 403/404 都误判为过期。
        return Err(format!("图片服务返回 HTTP {}", status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err("图片超过 64 MiB 下载上限".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取图片失败：{}", error.without_url()))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            return Err("图片超过 64 MiB 下载上限".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    // QQ 也可能用 HTTP 200 返回错误 JSON，不能把它缓存成图片交给解码器。
    if image::guess_format(&bytes).is_err() {
        return Err("图片服务没有返回图片内容，地址可能已失效".into());
    }
    Ok(bytes.into())
}
