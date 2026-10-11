use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

// 只用内存中的两个不同帧，不访问配置、文件、网络或真实会话。
fn gif_bytes_with_frames(count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut bytes)
        .encode_frames((0..count).map(|index| {
            let color = if index % 2 == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
            image::Frame::from_parts(
                image::RgbaImage::from_pixel(2, 1, color),
                0,
                0,
                image::Delay::from_numer_denom_ms(if index % 2 == 0 { 40 } else { 70 }, 1),
            )
        }))
        .unwrap();
    bytes
}

fn gif_bytes() -> Bytes {
    gif_bytes_with_frames(2).into()
}

// 即使断言失败，也释放测试阻塞器，避免遗留永久占用 worker 的线程。
struct ReleaseOnDrop(mpsc::Sender<()>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn block_worker(loader: &BoundedGifLoader) -> ReleaseOnDrop {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release = ReleaseOnDrop(release_tx);
    loader
        .workers
        .schedule(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        })
        .unwrap();
    entered_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
    release
}

fn drain_worker(loader: &BoundedGifLoader) {
    let (done_tx, done_rx) = mpsc::channel();
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let done_tx = done_tx.clone();
        match loader.workers.schedule(move || {
            let _ = done_tx.send(());
        }) {
            Ok(()) => break,
            Err(ScheduleError::Full) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(err) => panic!("GIF 测试任务未能排队: {err:?}"),
        }
    }
    done_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
}

fn ready_frame(
    loader: &BoundedGifLoader,
    ctx: &egui::Context,
    uri: &str,
    index: usize,
) -> Arc<ColorImage> {
    match loader.load(ctx, &format!("{uri}#{index}"), SizeHint::default()) {
        Ok(ImagePoll::Ready { image }) => image,
        Ok(ImagePoll::Pending { .. }) => panic!("GIF 应当已就绪，却仍在等待"),
        Err(err) => panic!("GIF 应当已就绪，却加载失败: {err}"),
    }
}

#[test]
fn cold_poll_does_not_wait_for_decode_and_completion_wakes_ui() {
    let loader = BoundedGifLoader::new(1024);
    let ctx = egui::Context::default();
    let uri = "bytes://gif_async_poll.gif";
    ctx.include_bytes(uri, gif_bytes());
    // 清除 Context 首帧自身的 repaint，后面只观察解码完成的唤醒。
    for _ in 0..3 {
        let _ = ctx.run_ui(Default::default(), |_| {});
    }
    let (repaint_tx, repaint_rx) = mpsc::channel();
    ctx.set_request_repaint_callback(move |_| {
        let _ = repaint_tx.send(());
    });
    let release = block_worker(&loader);
    for index in 0..4 {
        assert!(matches!(
            loader.load(&ctx, &format!("{uri}#{index}"), SizeHint::default()),
            Ok(ImagePoll::Pending { .. })
        ));
    }
    assert!(loader.has_pending());
    assert_eq!(loader.state.lock().pending.len(), 1);
    assert!(matches!(
        ctx.try_load_bytes(uri),
        Ok(BytesPoll::Ready { .. })
    ));
    // 纹理加载器只释放某一帧时，不得取消整张动画的待解码任务。
    loader.forget(&format!("{uri}#0"));
    assert!(loader.has_pending());
    assert!(matches!(
        repaint_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(release);
    repaint_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
    assert!(!loader.has_pending());
    assert_eq!(
        ready_frame(&loader, &ctx, uri, 0).pixels[0],
        egui::Color32::RED
    );
    assert_eq!(
        ready_frame(&loader, &ctx, uri, 1).pixels[0],
        egui::Color32::BLUE
    );
    assert_eq!(
        ready_frame(&loader, &ctx, uri, 3).pixels[0],
        egui::Color32::BLUE
    );
    let durations = ctx
        .data(|data| data.get_temp::<FrameDurations>(Id::new(uri)))
        .unwrap();
    assert_eq!(
        durations.all().copied().collect::<Vec<_>>(),
        vec![Duration::from_millis(40), Duration::from_millis(70)]
    );
}

#[test]
fn forget_during_running_decode_discards_success_and_error_then_allows_reload() {
    for forget_all in [false, true] {
        for fail_decode in [false, true] {
            let loader = BoundedGifLoader::new(1024);
            let ctx = egui::Context::default();
            let uri = "bytes://gif_invalidated.gif";
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release = ReleaseOnDrop(release_tx);
            loader
                .schedule_decode(&ctx, uri, gif_bytes(), move |bytes| {
                    let _ = entered_tx.send(());
                    let _ = release_rx.recv();
                    if fail_decode {
                        Err("失效任务的解码错误".to_owned())
                    } else {
                        AnimatedImage::load_gif(bytes)
                    }
                })
                .unwrap();
            entered_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
            let old_generation = loader.state.lock().pending[uri];
            if forget_all {
                loader.forget_all();
            } else {
                loader.forget(uri);
            }
            assert!(!loader.has_pending());
            drop(release);
            drain_worker(&loader);
            {
                let state = loader.state.lock();
                assert!(!state.ready.contains(uri));
                assert!(!state.errors.contains(uri));
                assert_eq!(state.total_bytes, 0);
            }
            assert!(
                ctx.data(|data| data.get_temp::<FrameDurations>(Id::new(uri)))
                    .is_none()
            );
            let release = block_worker(&loader);
            loader
                .schedule_decode(&ctx, uri, gif_bytes(), AnimatedImage::load_gif)
                .unwrap();
            assert_ne!(loader.state.lock().pending[uri], old_generation);
            drop(release);
            drain_worker(&loader);
            assert_eq!(
                ready_frame(&loader, &ctx, uri, 1).pixels[0],
                egui::Color32::BLUE
            );
        }
    }
}

#[test]
fn forgotten_queued_job_is_skipped_before_decoding() {
    let loader = BoundedGifLoader::new(1024);
    let ctx = egui::Context::default();
    let release = block_worker(&loader);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_job = Arc::clone(&calls);
    let uri = "bytes://gif_queued_stale.gif";
    loader
        .schedule_decode(&ctx, uri, gif_bytes(), move |bytes| {
            calls_for_job.fetch_add(1, Ordering::SeqCst);
            AnimatedImage::load_gif(bytes)
        })
        .unwrap();
    loader.forget(uri);
    drop(release);
    drain_worker(&loader);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!loader.state.lock().ready.contains(uri));
}

#[test]
fn gif_queue_is_bounded_deduplicates_uris_and_retries_when_full() {
    let loader = BoundedGifLoader::new(1024);
    let ctx = egui::Context::default();
    let release = block_worker(&loader);
    let calls = Arc::new(AtomicUsize::new(0));
    // 一个 worker 已阻塞，最多还能容纳四个不同 URI 的任务。
    for index in 0..4 {
        let uri = format!("bytes://gif_queue_{index}.gif");
        for _ in 0..3 {
            let calls = Arc::clone(&calls);
            loader
                .schedule_decode(&ctx, &uri, gif_bytes(), move |bytes| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    AnimatedImage::load_gif(bytes)
                })
                .unwrap();
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(loader.state.lock().pending.len(), 4);
    let overflow = "bytes://gif_queue_overflow.gif";
    ctx.include_bytes(overflow, gif_bytes());
    assert!(matches!(
        loader.load(&ctx, &format!("{overflow}#0"), SizeHint::default()),
        Ok(ImagePoll::Pending { .. })
    ));
    {
        let state = loader.state.lock();
        assert!(!state.pending.contains_key(overflow));
        assert!(!state.errors.contains(overflow));
        assert_eq!(state.pending.len(), 4);
    }
    drop(release);
    drain_worker(&loader);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(matches!(
        loader.load(&ctx, &format!("{overflow}#0"), SizeHint::default()),
        Ok(ImagePoll::Pending { .. })
    ));
    drain_worker(&loader);
    assert_eq!(
        ready_frame(&loader, &ctx, overflow, 0).pixels[0],
        egui::Color32::RED
    );
}

#[test]
fn decode_failure_is_cached_until_forget_and_error_cache_is_bounded() {
    let loader = BoundedGifLoader::new(1024);
    let ctx = egui::Context::default();
    let uri = "bytes://gif_broken.gif";
    ctx.include_bytes(uri, b"GIF89a".to_vec());
    assert!(matches!(
        loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
        Ok(ImagePoll::Pending { .. })
    ));
    drain_worker(&loader);
    for _ in 0..3 {
        assert!(matches!(
            loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
            Err(LoadError::Loading(_))
        ));
        assert!(!loader.has_pending());
    }
    loader.forget(uri);
    // 解码缓存与 include 源字节缓存独立；替换同 URI 的内容前必须遗忘旧源。
    ctx.loaders().include.forget(uri);
    ctx.include_bytes(uri, gif_bytes());
    assert!(matches!(
        loader.load(&ctx, &format!("{uri}#0"), SizeHint::default()),
        Ok(ImagePoll::Pending { .. })
    ));
    drain_worker(&loader);
    assert_eq!(
        ready_frame(&loader, &ctx, uri, 0).pixels[0],
        egui::Color32::RED
    );
    let mut state = GifState::new(1024);
    for index in 0..=MAX_GIF_ERROR_ENTRIES {
        let uri = format!("bad_{index}");
        let generation = state.begin_decode(&uri).unwrap();
        assert!(state.complete_decode(&uri, generation, Err("损坏的 GIF".to_owned())));
    }
    assert_eq!(state.errors.len(), MAX_GIF_ERROR_ENTRIES);
    assert!(!state.errors.contains("bad_0"));
    assert!(state.pending.is_empty());
}

#[test]
fn frame_limit_and_active_cache_soft_budget_are_preserved() {
    let bytes = gif_bytes_with_frames(MAX_GIF_FRAMES + 1);
    let image = AnimatedImage::load_gif(&bytes).unwrap();
    assert_eq!(image.frames.len(), MAX_GIF_FRAMES);
    assert_eq!(image.frame_durations.all().count(), MAX_GIF_FRAMES);
    assert_eq!(
        image.get_image(MAX_GIF_FRAMES).pixels[0],
        image.get_image(0).pixels[0]
    );
    let now = Instant::now();
    let mut state = GifState::new(1);
    let first = Arc::new(AnimatedImage::load_gif(gif_bytes().as_ref()).unwrap());
    let second = Arc::new(AnimatedImage::load_gif(gif_bytes().as_ref()).unwrap());
    state.insert_ready("first".to_owned(), first, now);
    state.insert_ready("second".to_owned(), second, now);
    assert_eq!(state.ready.len(), 2);
    let later = now + ACTIVE_GIF_GRACE + Duration::from_millis(1);
    // 与 Ready 的 load 路径一致：实际访问会更新 LRU 和 last_used，而非只靠插入顺序。
    state.ready.get_mut("second").unwrap().last_used = later;
    state.evict_if_needed(later);
    assert_eq!(state.ready.len(), 1);
    assert!(!state.ready.contains("first"));
    assert!(state.ready.contains("second"));
    assert!(state.total_bytes > state.max_bytes);
}
