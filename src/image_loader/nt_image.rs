//! QQ NT 图片失败恢复：通过图片所属 Bridge 刷新 rkey，并最多重试一次。
//! 普通消息、回复、合并转发和预览共用字节加载器，不把刷新逻辑分散到 UI。

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
    mutex::Mutex,
};
use lru::LruCache;
use serde_json::Value;
use tokio::sync::{Semaphore, oneshot};

use crate::ica::{BridgeEvent, BridgeHandle, IcaCommand};

const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const ERROR_RETRY_DELAY: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 256;
const MAX_BINDINGS: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtImageIdentity {
    pub file_id: String,
    pub app_id: String,
}

impl NtImageIdentity {
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

#[derive(Clone)]
struct DownloadedImage {
    bytes: Arc<[u8]>,
    url: String,
}

#[derive(Clone)]
enum Entry {
    Pending(u64),
    Ready(DownloadedImage),
    Failed { error: String, retry_after: Instant },
}

struct State {
    // 同一 URL 出现在多个 Bridge 时无法凭 URL 判定来源；None 表示歧义，禁止刷新。
    bindings: LruCache<String, Option<BridgeHandle>>,
    entries: LruCache<String, Entry>,
    resolved: LruCache<String, String>,
    next_generation: u64,
    max_bytes: usize,
}

impl State {
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

pub struct NtImageLoader {
    state: Arc<Mutex<State>>,
    runtime: tokio::runtime::Handle,
    client: reqwest::Client,
    slots: Arc<Semaphore>,
}

fn loader_id() -> egui::Id {
    egui::Id::new("ica_nt_image_bytes_loader")
}

impl NtImageLoader {
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

pub fn resolved_url(ctx: &egui::Context, uri: &str) -> String {
    let loader = ctx.data(|data| data.get_temp::<Arc<NtImageLoader>>(loader_id()));
    loader
        .and_then(|loader| loader.state.lock().resolved.get(uri).cloned())
        .unwrap_or_else(|| uri.to_string())
}

impl BytesLoader for NtImageLoader {
    fn id(&self) -> &str {
        "ica_native_nt_image_bytes"
    }

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

    fn forget(&self, uri: &str) {
        self.state
            .lock()
            .entries
            .pop(super::decode::normalize_uri(uri));
    }
    fn forget_all(&self) {
        self.state.lock().entries.clear();
    }
    fn byte_size(&self) -> usize {
        self.state.lock().bytes()
    }
    fn has_pending(&self) -> bool {
        self.state
            .lock()
            .entries
            .iter()
            .any(|(_, entry)| matches!(entry, Entry::Pending(_)))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const URL: &str =
        "https://multimedia.nt.qq.com.cn/download?appid=1407&fileid=test%2Bfile&rkey=old";

    fn state() -> State {
        State {
            bindings: LruCache::new(NonZeroUsize::new(8).unwrap()),
            entries: LruCache::new(NonZeroUsize::new(8).unwrap()),
            resolved: LruCache::new(NonZeroUsize::new(8).unwrap()),
            next_generation: 0,
            max_bytes: 8,
        }
    }

    #[tokio::test]
    async fn expired_url_refreshes_once_and_retries_the_new_url() {
        let requests = std::sync::Mutex::new(Vec::new());
        let refreshed = URL.replace("rkey=old", "rkey=new");
        let result = recover_image(
            URL,
            &NtImageIdentity::from_url(URL).unwrap(),
            |url| {
                requests.lock().unwrap().push(url.clone());
                std::future::ready(if url == URL {
                    Err("地址失效".into())
                } else {
                    Ok(Arc::from([1_u8, 2]))
                })
            },
            || std::future::ready(Ok(refreshed.clone())),
        )
        .await
        .unwrap();
        assert_eq!(
            *requests.lock().unwrap(),
            vec![URL.to_string(), refreshed.clone()]
        );
        assert_eq!(result.url, refreshed);
    }

    #[tokio::test]
    async fn valid_download_does_not_ask_bridge_for_a_new_key() {
        let result = recover_image(
            URL,
            &NtImageIdentity::from_url(URL).unwrap(),
            |_| std::future::ready(Ok(Arc::from([1_u8]))),
            || async { panic!("图片可用时不应刷新链接") },
        )
        .await
        .unwrap();
        assert_eq!(result.url, URL);
    }

    #[tokio::test]
    async fn repeated_download_failure_stops_after_one_refresh() {
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let result = recover_image(
            URL,
            &NtImageIdentity::from_url(URL).unwrap(),
            |_| {
                requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(Err("下载失败".into()))
            },
            || std::future::ready(Ok(URL.replace("rkey=old", "rkey=new"))),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn invalid_or_unchanged_refresh_is_not_downloaded() {
        for url in [
            URL.to_string(),
            "https://example.invalid/image.png".into(),
            URL.replace("test%2Bfile", "other"),
        ] {
            let requests = std::sync::atomic::AtomicUsize::new(0);
            let result = recover_image(
                URL,
                &NtImageIdentity::from_url(URL).unwrap(),
                |_| {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::future::ready(Err("原链接不可用".into()))
                },
                || std::future::ready(Ok(url)),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn nt_identity_allows_rotating_keys_but_rejects_wrong_targets() {
        let identity = NtImageIdentity::from_url(URL).unwrap();
        assert_eq!(identity.file_id, "test+file");
        assert_eq!(
            NtImageIdentity::from_url(&URL.replace("rkey=old", "rkey=new")),
            Some(identity.clone())
        );
        assert_eq!(
            NtImageIdentity::from_url(&URL.replace("multimedia.nt.qq.com.cn", "gchat.qpic.cn")),
            Some(identity.clone())
        );
        assert_ne!(
            NtImageIdentity::from_url(&URL.replace("1407", "1406")),
            Some(identity)
        );
        for invalid in [
            URL.replace("https:", "http:"),
            URL.replace("qq.com.cn", "qq.com.cn.invalid"),
            URL.replace("/download?", "/other?"),
            URL.replace("1407", "9999"),
            format!("{URL}&fileid=other"),
            URL.replace("https://", "https://user@"),
        ] {
            assert!(NtImageIdentity::from_url(&invalid).is_none());
        }
    }

    #[test]
    fn nested_reply_and_forward_images_are_registered_without_guessing_current_bridge() {
        let (tx_a, _rx_a) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, _rx_b) = tokio::sync::mpsc::unbounded_channel();
        let handles = HashMap::from([
            ("a".to_string(), BridgeHandle::new("a".into(), tx_a)),
            ("b".to_string(), BridgeHandle::new("b".into(), tx_b)),
        ]);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let ctx = egui::Context::default();
        let loader = NtImageLoader::install(&ctx, runtime.handle().clone(), 1024);
        let event = BridgeEvent::from_protocol(
            "b",
            "forwardMessagesResponse",
            json!({
                "messages": [{"files": [{"url": URL}], "replyMessage": {"file": {"url": URL}}}]
            }),
        );
        loader.observe(&event, &handles);
        assert_eq!(
            loader
                .state
                .lock()
                .bindings
                .peek(URL)
                .unwrap()
                .as_ref()
                .unwrap()
                .bridge_key(),
            "b"
        );
        let unrelated = "https://example.invalid/image.png";
        assert!(!loader.state.lock().bindings.contains(unrelated));
        loader.observe(
            &BridgeEvent::from_protocol("a", "setMessages", json!([{"url": URL}])),
            &handles,
        );
        loader.observe(&event, &handles);
        assert!(
            loader.state.lock().bindings.peek(URL).unwrap().is_none(),
            "跨 Bridge 的相同 URL 必须停止刷新，不能靠最后收到的事件猜测来源"
        );
    }

    #[test]
    fn forgotten_download_cannot_restore_stale_result_and_byte_budget_evicts_old_images() {
        let mut state = state();
        state.entries.put(URL.into(), Entry::Pending(1));
        state.entries.pop(URL);
        state.entries.put(URL.into(), Entry::Pending(2));
        assert!(!state.complete(URL, 1, Err("旧请求失败".into())));
        assert!(state.complete(
            URL,
            2,
            Ok(DownloadedImage {
                bytes: vec![1; 6].into(),
                url: URL.into()
            })
        ));
        state.entries.put("second".into(), Entry::Pending(3));
        state.complete(
            "second",
            3,
            Ok(DownloadedImage {
                bytes: vec![2; 6].into(),
                url: "second".into(),
            }),
        );
        assert!(state.entries.peek(URL).is_none());
        assert_eq!(state.bytes(), 6);
    }

    #[test]
    fn gpu_byte_eviction_keeps_resolved_url_for_copy_and_future_reloads() {
        let mut state = state();
        let refreshed = URL.replace("rkey=old", "rkey=new");
        state.entries.put(URL.into(), Entry::Pending(1));
        state.complete(
            URL,
            1,
            Ok(DownloadedImage {
                bytes: vec![1].into(),
                url: refreshed.clone(),
            }),
        );
        state.entries.pop(URL);
        assert_eq!(state.resolved.get(URL), Some(&refreshed));
    }

    #[test]
    fn failed_request_has_cooldown_instead_of_per_frame_retry() {
        let mut state = state();
        state.entries.put(URL.into(), Entry::Pending(1));
        state.complete(URL, 1, Err("刷新失败".into()));
        assert!(
            matches!(state.entries.peek(URL), Some(Entry::Failed { retry_after, .. }) if *retry_after > Instant::now())
        );
    }
}
