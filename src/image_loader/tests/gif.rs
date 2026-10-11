use super::*;
use crate::image_loader::texture;
use egui::load::{BytesLoader, TexturePoll};
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

// --- 动画像素缓存的活跃判定与淘汰：跨 loader 的可见性回归 ---

/// 测试用字节加载器：模拟“源字节随时可以重新取到”的网络层。
///
/// forget 故意不丢字节：一旦某帧回落到取源，就会真的重新下载并重新解码，
/// 与生产里源字节被释放后重新走 HTTP 的行为一致；forget 调用次数单独记账，
/// 供“源字节有没有被释放”的断言使用。
struct AlwaysAvailableGifBytes {
    prefix: &'static str,
    bytes: Bytes,
    loads: AtomicUsize,
    forgets: AtomicUsize,
}

impl AlwaysAvailableGifBytes {
    fn new(prefix: &'static str, bytes: Bytes) -> Self {
        Self {
            prefix,
            bytes,
            loads: AtomicUsize::new(0),
            forgets: AtomicUsize::new(0),
        }
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn forgets(&self) -> usize {
        self.forgets.load(Ordering::SeqCst)
    }
}

impl BytesLoader for AlwaysAvailableGifBytes {
    fn id(&self) -> &str {
        concat!(module_path!(), "::AlwaysAvailableGifBytes")
    }

    fn load(&self, _: &egui::Context, uri: &str) -> egui::load::BytesLoadResult {
        if !uri.starts_with(self.prefix) {
            return Err(LoadError::NotSupported);
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(BytesPoll::Ready {
            size: None,
            bytes: self.bytes.clone(),
            mime: Some("image/gif".to_owned()),
        })
    }

    fn forget(&self, uri: &str) {
        if uri.starts_with(self.prefix) {
            self.forgets.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        self.bytes.len()
    }
}

/// 搭好真实链路：可重新取源的字节层 + 有预算的 GIF loader + 有预算的纹理 loader。
///
/// GIF 像素预算取 1 字节：两张动画同时就绪即超预算，每次淘汰检查都必须在候选里
/// 真的做取舍，不必造大图。
fn gif_budget_fixture() -> (
    egui::Context,
    Arc<BoundedGifLoader>,
    Arc<AlwaysAvailableGifBytes>,
) {
    let ctx = egui::Context::default();
    let bytes = Arc::new(AlwaysAvailableGifBytes::new(
        "bytes://gif_budget_",
        gif_bytes(),
    ));
    ctx.add_bytes_loader(bytes.clone());
    // 纹理预算给足：本用例只观察 GIF 像素缓存（BoundedGifLoader）的淘汰决策。
    texture::install(&ctx, u64::MAX);
    let loader = Arc::new(BoundedGifLoader::new(1));
    ctx.add_image_loader(loader.clone());
    (ctx, loader, bytes)
}

/// 通过真实纹理链（TrackingTextureLoader → BoundedGifLoader）取某一帧纹理。
///
/// 帧解码在后台线程完成，Pending 时先排空 GIF 解码任务再重试；不含计时等待。
fn ready_frame_texture(
    ctx: &egui::Context,
    loader: &BoundedGifLoader,
    uri: &str,
    index: usize,
) -> egui::load::SizedTexture {
    let frame_uri = format!("{uri}#{index}");
    for _ in 0..3 {
        match ctx.try_load_texture(
            &frame_uri,
            egui::TextureOptions::default(),
            SizeHint::default(),
        ) {
            Ok(TexturePoll::Ready { texture }) => return texture,
            Ok(TexturePoll::Pending { .. }) => drain_worker(loader),
            Err(err) => panic!("GIF 帧纹理应当可加载: uri={frame_uri} err={err}"),
        }
    }
    panic!("排空解码任务后 GIF 帧纹理仍未就绪: uri={frame_uri}");
}

/// 请求某一帧纹理并断言它可显示，用于“当前正在显示的帧”。
fn expect_ready_frame_texture(
    ctx: &egui::Context,
    uri: &str,
    index: usize,
    reason: &str,
) -> egui::load::SizedTexture {
    match ctx.try_load_texture(
        &format!("{uri}#{index}"),
        egui::TextureOptions::default(),
        SizeHint::default(),
    ) {
        Ok(TexturePoll::Ready { texture }) => texture,
        Ok(TexturePoll::Pending { .. }) => panic!("{reason}"),
        Err(err) => panic!("{reason}: {err}"),
    }
}

/// 通过真实 GIF loader 取某一帧像素，Pending 时排空解码任务再重试。
fn ready_gif_frame(
    loader: &BoundedGifLoader,
    ctx: &egui::Context,
    uri: &str,
    index: usize,
) -> Arc<ColorImage> {
    let frame_uri = format!("{uri}#{index}");
    for _ in 0..3 {
        match loader.load(ctx, &frame_uri, SizeHint::default()) {
            Ok(ImagePoll::Ready { image }) => return image,
            Ok(ImagePoll::Pending { .. }) => drain_worker(loader),
            Err(err) => panic!("GIF 帧应当可加载: uri={frame_uri} err={err}"),
        }
    }
    panic!("排空解码任务后 GIF 帧仍未就绪: uri={frame_uri}");
}

/// 注入“像素条目已超过活跃宽限没被 GIF loader 触达”的状态。
///
/// 用 peek_mut 而不是 get_mut：本次只改年龄，不改 LRU 里的访问顺序，
/// “淘汰检查会挑中这一条”仍由真实访问顺序决定。
fn age_past_active_grace(loader: &BoundedGifLoader, uri: &str) {
    let mut state = loader.state.lock();
    let ready = &mut state.ready;
    let entry = ready.peek_mut(uri).expect("待注入年龄的动画必须已解码就绪");
    entry.last_used = Instant::now() - ACTIVE_GIF_GRACE - Duration::from_millis(1);
}

/// 把某动画标记为“刚刚还在显示”，让可见性判定不依赖测试机器的快慢。
fn mark_recently_used(loader: &BoundedGifLoader, uri: &str) {
    let mut state = loader.state.lock();
    let ready = &mut state.ready;
    if let Some(entry) = ready.peek_mut(uri) {
        entry.last_used = Instant::now();
    }
}

/// 可见动画的已解码帧不得被“另一张动画的帧请求”触发的预算检查淘汰。
///
/// 真实链路里动画播到某一帧后，该帧纹理已在 GPU 纹理缓存中：后续对同一帧 URI 的
/// 请求由纹理缓存直接命中，GIF loader 不再被调用，它记录的 last_used 便一直停在最近
/// 一次真正的帧解码上。本用例显式注入这种“像素条目已超活跃宽限”的状态（等价于纹理
/// 缓存把 GIF loader 挡在链路外），再让另一张动画请求自己尚未上传的帧，检查预算检查
/// 是否把仍在显示的动画误判成冷缓存。误判的后果是下一帧只能重新取源 + 重新解码，
/// 聊天里表现为画面闪断、中途出现加载转圈。
#[test]
fn visible_animation_keeps_ready_frames_when_another_gif_frame_checks_the_budget() {
    const VISIBLE: &str = "bytes://gif_budget_visible.gif";
    const OTHER: &str = "bytes://gif_budget_other.gif";

    let (ctx, loader, bytes) = gif_budget_fixture();
    // 可见动画当前显示的 #0 帧：解码并上传纹理；#1 帧纹理尚未上传。
    ready_frame_texture(&ctx, &loader, VISIBLE, 0);
    // 另一张动画同样已就绪，它自己的 #1 帧纹理尚未上传。
    ready_frame_texture(&ctx, &loader, OTHER, 0);

    let state = Arc::clone(&loader.state);
    {
        let state = state.lock();
        assert_eq!(state.ready.len(), 2, "两张动画都应当已解码就绪");
        assert!(
            state.total_bytes > state.max_bytes,
            "本用例的前提是像素缓存已超预算"
        );
    }
    age_past_active_grace(&loader, VISIBLE);

    // 显示可见动画的当前帧：由纹理缓存命中提供。命中是否顺带让 GIF 像素缓存知道
    // 动画仍在显示，正是本用例要区分的行为。
    let current_frame = expect_ready_frame_texture(&ctx, VISIBLE, 0, "可见动画的当前帧应当可显示");

    // 另一张动画请求自己未缓存的帧：这次请求会真正走进 GIF loader，触发预算检查。
    expect_ready_frame_texture(&ctx, OTHER, 1, "另一张动画自己的帧应当可显示");
    assert!(
        state.lock().ready.contains(VISIBLE),
        "仍在显示的动画不得因预算检查丢掉已解码帧"
    );

    // 占住唯一的 GIF 解码线程：万一可见动画已被误淘汰，重新取源 + 重新解码只能得到
    // Pending（聊天里的转圈），不会赶在断言之前侥幸完成。
    let release_worker = block_worker(&loader);
    let loads_before = bytes.loads();
    match ctx.try_load_texture(
        &format!("{VISIBLE}#1"),
        egui::TextureOptions::default(),
        SizeHint::default(),
    ) {
        Ok(TexturePoll::Ready { texture }) => assert_ne!(
            texture.id, current_frame.id,
            "可见动画的下一帧必须是新解码出来的帧，而不是退回上一帧画面"
        ),
        Ok(TexturePoll::Pending { .. }) => {
            panic!("可见动画被误淘汰后帧缺失，只能重新取源并重新解码")
        }
        Err(err) => panic!("可见动画的下一帧应当仍可显示，却加载失败: {err}"),
    }
    drop(release_worker);
    assert_eq!(bytes.loads(), loads_before, "可见动画不得重新取源");
    assert!(
        !state.lock().pending.contains_key(VISIBLE),
        "可见动画不得重新进入解码"
    );
}

/// 真正离屏、不再被请求的动画仍必须能被 GIF 预算淘汰。
///
/// 与上一条用例配对：活跃判定必须区分“仍在每帧请求的动画”和“已经没人要的动画”，
/// 修复误淘汰不能把预算变成只进不出。
#[test]
fn offscreen_animation_is_still_evicted_by_the_gif_budget() {
    const COLD: &str = "bytes://gif_budget_cold.gif";
    const VISIBLE: &str = "bytes://gif_budget_visible_offscreen.gif";
    const NEWCOMER: &str = "bytes://gif_budget_new.gif";

    let (ctx, loader, _bytes) = gif_budget_fixture();
    // 冷动画先就绪，此后不再被请求（已离屏）。
    ready_frame_texture(&ctx, &loader, COLD, 0);
    // 仍在显示的动画随后就绪：冷动画被留在 LRU 头部。
    ready_frame_texture(&ctx, &loader, VISIBLE, 0);
    age_past_active_grace(&loader, COLD);

    // 可见动画每帧都在请求自己的当前帧；GPU 命中即代表它仍在显示。
    expect_ready_frame_texture(&ctx, VISIBLE, 0, "可见动画的当前帧应当可显示");
    mark_recently_used(&loader, VISIBLE);

    // 另一张动画进入屏幕：插入新条目时按预算淘汰，第一个候选是离屏的冷动画。
    let newcomer = AnimatedImage::load_gif(gif_bytes().as_ref()).unwrap();
    let mut state = loader.state.lock();
    state.insert_ready(NEWCOMER.to_owned(), Arc::new(newcomer), Instant::now());

    assert!(!state.ready.contains(COLD), "离屏动画必须仍可被淘汰");
    assert!(state.ready.contains(VISIBLE), "可见动画必须保留");
    assert!(state.ready.contains(NEWCOMER), "新进入的动画必须保留");
}

/// bytes:// 源在首次 Ready 后必须保留，丢掉了像素也能重新解码回来。
///
/// 与静态纹理路径同一份合同（texture.rs 中只在非 bytes:// 时才释放源字节）：
/// bytes:// 通常是内嵌表情或待发送图片，源字节是像素与纹理缓存失效后唯一的可重建
/// 来源。若首次 Ready 就释放掉源字节，GIF 像素条目一旦被预算淘汰或显式忘记，
/// 帧就再也重建不出来，聊天里的动画只能一直转圈。
#[test]
fn bytes_source_is_kept_after_first_ready_and_rebuilds_frames() {
    let loader = BoundedGifLoader::new(1024);
    let ctx = egui::Context::default();
    let uri = "bytes://gif_rebuild_from_include.gif";
    // 用真实 include 源，不接测试替身：要验的正是源字节有没有被释放。
    ctx.include_bytes(uri, gif_bytes());

    let first = ready_gif_frame(&loader, &ctx, uri, 0);
    assert_eq!(first.pixels[0], egui::Color32::RED);
    assert!(
        matches!(ctx.try_load_bytes(uri), Ok(BytesPoll::Ready { .. })),
        "bytes:// 源在首次 Ready 后不得被释放"
    );

    // 丢掉像素（等价于预算淘汰或显式 forget）后，必须能从保留的源重新解码。
    loader.forget(uri);
    assert!(!loader.state.lock().ready.contains(uri));
    let rebuilt = ready_gif_frame(&loader, &ctx, uri, 1);
    assert_eq!(rebuilt.pixels[0], egui::Color32::BLUE);
}

/// 非 bytes:// 源（HTTP、文件）在首次 Ready 后仍必须释放，避免原始字节长期驻留。
///
/// 与 bytes:// 保留合同配对：释放只针对可以重新获取的远端或磁盘源；像素与纹理缓存
/// 都重建得了的场合不该继续占着原始字节。字节由测试替身提供，不发起真实网络请求。
#[test]
fn non_bytes_source_is_still_released_after_first_ready() {
    for (prefix, uri) in [
        (
            "https://gif-budget.test/",
            "https://gif-budget.test/released.gif",
        ),
        (
            "file://gif-budget-test/",
            "file://gif-budget-test/released.gif",
        ),
    ] {
        let loader = BoundedGifLoader::new(1024);
        let ctx = egui::Context::default();
        let bytes = Arc::new(AlwaysAvailableGifBytes::new(prefix, gif_bytes()));
        ctx.add_bytes_loader(bytes.clone());

        let first = ready_gif_frame(&loader, &ctx, uri, 0);
        assert_eq!(first.pixels[0], egui::Color32::RED);
        assert!(bytes.forgets() > 0, "非 bytes:// 源仍必须释放: {prefix}");
    }
}
