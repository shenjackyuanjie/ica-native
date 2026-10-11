use super::*;
use image::ImageEncoder as _;
use std::{sync::mpsc, time::Duration};

#[test]
fn stale_gif_frame_uris_are_rejected_by_the_real_static_loader_chain() {
    // 手动构造无磁盘实例，不调用可能创建缓存目录的生产构造函数。
    let loader = Arc::new(TrackingImageLoader {
        state: Arc::new(Mutex::new(LoaderState::new(1024))),
        disk_cache: None,
        workers: Arc::new(DecodeWorkerPool::new(1, "static-frame-regression")),
        raw_cache: raw::RawImageCache::new(1024),
    });
    let ctx = Context::default();
    raw::install(&ctx, loader.raw_cache.clone());
    ctx.add_image_loader(loader.clone());
    gif::install(&ctx, 1024);
    texture::install(&ctx, 1024);

    let uri = "bytes://changed_animation.gif";
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &[255, 0, 0, 255, 0, 255, 0, 255],
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    ctx.include_bytes(uri, png);
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(uri),
            egui::FrameDurations::new(vec![Duration::from_millis(40), Duration::from_millis(70)]),
        );
    });

    // GIF 变成 PNG 后，真实 loader 链必须把格式失配传回上层，而不是
    // 由静态 loader 偷接管 #frame 并返回 Pending/Ready，挡住元数据回落。
    for index in 0..2 {
        let frame_uri = format!("{uri}#{index}");
        assert!(matches!(
            ctx.try_load_image(&frame_uri, SizeHint::default()),
            Err(LoadError::NoMatchingImageLoader { .. })
        ));
        assert!(matches!(
            ctx.try_load_texture(&frame_uri, Default::default(), SizeHint::default()),
            Err(LoadError::NoMatchingImageLoader { .. })
        ));
        assert!(!loader.has_pending(), "动画帧 URI 不得触发静态解码");
    }

    assert!(matches!(
        ctx.try_load_image(uri, SizeHint::default()),
        Ok(ImagePoll::Pending { .. })
    ));
    // 队列标记保证真实后台 PNG 解码已完成，避免睡眠与计时猜测。
    let (done_tx, done_rx) = mpsc::channel();
    loader
        .workers
        .schedule(move || {
            let _ = done_tx.send(());
        })
        .unwrap();
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    match ctx.try_load_image(uri, SizeHint::default()) {
        Ok(ImagePoll::Ready { image }) => {
            assert_eq!(image.size, [2, 1]);
            assert_eq!(image.pixels, vec![egui::Color32::RED, egui::Color32::GREEN]);
        }
        Ok(ImagePoll::Pending { .. }) => panic!("原 URI 的静态图应当已解码"),
        Err(err) => panic!("原 URI 的静态图加载失败: {err}"),
    }
    // 即使 base 的静态像素已经在缓存，也不得借用它满足动画帧请求。
    assert!(matches!(
        ctx.try_load_image(&format!("{uri}#1"), SizeHint::default()),
        Err(LoadError::NoMatchingImageLoader { .. })
    ));
    match ctx.try_load_texture(uri, Default::default(), SizeHint::default()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            assert_eq!(texture.size, egui::vec2(2.0, 1.0));
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => panic!("原 URI 应当能上传静态纹理"),
        Err(err) => panic!("原 URI 的静态纹理加载失败: {err}"),
    }
}
