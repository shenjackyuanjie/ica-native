use super::*;
use serde_json::json;

const URL: &str = "https://multimedia.nt.qq.com.cn/download?appid=1407&fileid=test%2Bfile&rkey=old";

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
