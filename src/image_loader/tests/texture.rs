use super::*;
use crate::app::test_support::test_app;
use crate::ica::types::{files::MessageFile, message::Message};
use egui::load::{ImageLoadResult, ImageLoader};
use std::collections::HashMap;

const FIRST: &str = "bytes://gif-continuity-first.gif";
const SECOND: &str = "bytes://gif-continuity-second.gif";

#[derive(Default)]
struct ControlledImages {
    results: Mutex<HashMap<String, ImageLoadResult>>,
    calls: Mutex<HashMap<String, usize>>,
}

impl ControlledImages {
    fn set(&self, uri: &str, result: ImageLoadResult) {
        self.results.lock().insert(uri.to_owned(), result);
    }

    fn ready(&self, uri: &str, color: egui::Color32) {
        self.set(
            uri,
            Ok(ImagePoll::Ready {
                image: Arc::new(egui::ColorImage::filled([200, 100], color)),
            }),
        );
    }
}

impl ImageLoader for ControlledImages {
    fn id(&self) -> &str {
        concat!(module_path!(), "::ControlledImages")
    }

    fn load(&self, _: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        *self.calls.lock().entry(uri.to_owned()).or_default() += 1;
        self.results
            .lock()
            .get(uri)
            .cloned()
            .unwrap_or(Ok(ImagePoll::Pending { size: None }))
    }

    // 模拟 GIF loader：上传一帧不清掉整张动画的像素。
    fn forget(&self, _: &str) {}
    fn forget_all(&self) {}
    fn byte_size(&self) -> usize {
        0
    }
}

fn fixture() -> (Context, Arc<TrackingTextureLoader>, Arc<ControlledImages>) {
    let ctx = Context::default();
    let images = Arc::new(ControlledImages::default());
    let textures = Arc::new(TrackingTextureLoader::new(1024 * 1024));
    ctx.add_image_loader(images.clone());
    ctx.add_texture_loader(textures.clone());
    (ctx, textures, images)
}

fn poll(ctx: &Context, uri: &str) -> TextureLoadResult {
    ctx.try_load_texture(uri, TextureOptions::default(), SizeHint::default())
}

fn ready_texture(result: TextureLoadResult) -> SizedTexture {
    match result {
        Ok(TexturePoll::Ready { texture }) => texture,
        Ok(TexturePoll::Pending { .. }) => panic!("应当保留可显示的纹理，却退回 Pending"),
        Err(err) => panic!("应当保留可显示的纹理，却失败: {err}"),
    }
}

#[test]
fn cached_gif_frames_still_touch_the_decoded_animation() {
    let (ctx, _, images) = fixture();
    let uri = format!("{FIRST}#0");
    images.ready(&uri, egui::Color32::RED);
    let texture = ready_texture(poll(&ctx, &uri));
    for _ in 0..4 {
        assert_eq!(ready_texture(poll(&ctx, &uri)).id, texture.id);
    }
    assert_eq!(
        images.calls.lock()[&uri],
        5,
        "GPU 命中也必须让 GIF 像素缓存知道动画仍在显示，否则会被误判为冷缓存"
    );
}

#[test]
fn pending_gif_frame_keeps_last_ready_texture_and_resumes() {
    let (ctx, _, images) = fixture();
    let first = format!("{FIRST}#0");
    let next = format!("{FIRST}#1");
    assert!(
        matches!(poll(&ctx, &first), Ok(TexturePoll::Pending { .. })),
        "首次加载没有可用画面，仍应 Pending"
    );
    images.ready(&first, egui::Color32::RED);
    let red = ready_texture(poll(&ctx, &first));
    images.set(&first, Ok(ImagePoll::Pending { size: None }));
    assert_eq!(
        ready_texture(poll(&ctx, &first)).id,
        red.id,
        "当前帧像素重载时也应保留已上传的纹理"
    );
    for _ in 0..4 {
        assert_eq!(
            ready_texture(poll(&ctx, &next)).id,
            red.id,
            "下一帧重建中应继续显示已就绪画面，不能闪回转圈"
        );
    }
    images.ready(&next, egui::Color32::BLUE);
    let blue = ready_texture(poll(&ctx, &next));
    assert_ne!(
        blue.id, red.id,
        "新帧就绪后必须恢复动画，而不是永久停在旧帧"
    );
    assert_eq!(
        ready_texture(poll(&ctx, &format!("{FIRST}#2"))).id,
        blue.id,
        "未就绪帧应使用最近实际显示的帧"
    );
}

#[test]
fn pending_gif_fallback_is_isolated_by_source_and_texture_options() {
    let (ctx, _, images) = fixture();
    images.ready(&format!("{FIRST}#0"), egui::Color32::RED);
    ready_texture(poll(&ctx, &format!("{FIRST}#0")));
    assert!(
        matches!(
            poll(&ctx, &format!("{SECOND}#1")),
            Ok(TexturePoll::Pending { .. })
        ),
        "不同 GIF 不得借用其他消息的画面"
    );
    assert!(
        matches!(
            ctx.try_load_texture(
                &format!("{FIRST}#1"),
                TextureOptions::NEAREST,
                SizeHint::default()
            ),
            Ok(TexturePoll::Pending { .. })
        ),
        "不同采样选项不得混用缓存"
    );
}

#[test]
fn cached_gif_frames_do_not_hide_loading_or_format_errors() {
    let (ctx, _, images) = fixture();
    let uri = format!("{FIRST}#0");
    images.ready(&uri, egui::Color32::RED);
    ready_texture(poll(&ctx, &uri));
    images.set(&uri, Err(LoadError::Loading("图片链接已过期".into())));
    assert!(
        matches!(poll(&ctx, &uri), Err(LoadError::Loading(message)) if message == "图片链接已过期"),
        "临时 Pending 可以保留旧画面，真实错误必须继续上报"
    );
    images.set(
        &uri,
        Err(LoadError::FormatNotSupported {
            detected_format: None,
        }),
    );
    assert!(
        matches!(
            poll(&ctx, &uri),
            Err(LoadError::NoMatchingImageLoader { .. })
        ),
        "格式变化必须交给上层移除旧 GIF 元数据并回落静态图片"
    );
}

#[test]
fn forgetting_base_gif_clears_its_frames_and_preserves_other_sources() {
    let (ctx, textures, images) = fixture();
    let first = format!("{FIRST}#0");
    let next = format!("{FIRST}#1");
    let second = format!("{SECOND}#0");
    for uri in [&first, &next, &second] {
        images.ready(uri, egui::Color32::RED);
        ready_texture(poll(&ctx, uri));
    }
    ctx.forget_image(FIRST);
    images.set(&first, Ok(ImagePoll::Pending { size: None }));
    images.set(&next, Ok(ImagePoll::Pending { size: None }));
    assert!(
        matches!(poll(&ctx, &first), Ok(TexturePoll::Pending { .. })),
        "显式清理源 URI 后不能继续显示旧帧"
    );
    assert_eq!(textures.state.lock().textures.len(), 1);
    ready_texture(poll(&ctx, &second));
    assert_eq!(textures.byte_size(), 200 * 100 * 4);
}

#[test]
fn gif_fallback_remains_within_existing_texture_eviction_budget() {
    let (ctx, textures, images) = fixture();
    let first = format!("{FIRST}#0");
    images.ready(&first, egui::Color32::RED);
    ready_texture(poll(&ctx, &first));
    {
        let mut state = textures.state.lock();
        state.max_bytes = 1;
        for (_, entry) in state.textures.iter_mut() {
            entry.last_used = Instant::now() - ACTIVE_TEXTURE_GRACE - Duration::from_secs(1);
        }
    }
    let second = format!("{SECOND}#0");
    images.ready(&second, egui::Color32::BLUE);
    ready_texture(poll(&ctx, &second));
    assert_eq!(
        textures.state.lock().textures.len(),
        1,
        "保留最后画面不增加独立的永久纹理缓存"
    );
    assert!(
        matches!(
            poll(&ctx, &format!("{FIRST}#1")),
            Ok(TexturePoll::Pending { .. })
        ),
        "真正离屏且被预算淘汰的动画仍按冷加载处理"
    );
}

#[test]
fn static_texture_cache_hits_do_not_repeat_image_decode_polling() {
    let (ctx, _, images) = fixture();
    let uri = "bytes://static-continuity.png";
    images.ready(uri, egui::Color32::RED);
    let texture = ready_texture(poll(&ctx, uri));
    for _ in 0..4 {
        assert_eq!(ready_texture(poll(&ctx, uri)).id, texture.id);
    }
    assert_eq!(
        images.calls.lock()[uri],
        1,
        "静态图片仍只在纹理未命中时访问像素加载器"
    );
}

#[test]
fn chat_keeps_drawing_gif_pixels_during_pending_frame_reload() {
    let (ctx, _, images) = fixture();
    let (mut app, _rx) = test_app();
    app.custom_chat.show_message_avatar = false;
    let mut message: Message = serde_json::from_value(serde_json::json!({
        "_id": "gif-continuity-message", "senderId": 456, "username": "动画测试",
        "content": "", "time": 1_700_000_000_000_i64,
    }))
    .unwrap();
    message.files.push(MessageFile {
        file_type: "image/gif".into(),
        url: FIRST.into(),
        size: None,
        name: None,
        fid: None,
    });
    let conversation = app.bridge_states[0].conversation_mut(-123);
    conversation.messages = vec![message];
    conversation.no_more_history = true;
    conversation.scroll_to_bottom = true;
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(FIRST),
            egui::FrameDurations::new(vec![Duration::from_millis(40), Duration::from_millis(40)]),
        )
    });
    images.ready(&format!("{FIRST}#0"), egui::Color32::RED);
    let ready_id = ready_texture(poll(&ctx, &format!("{FIRST}#0"))).id;
    let mut displayed_id = ready_id;
    for (frame, time) in [
        0.001, 0.002, 0.003, 0.050, 0.051, 0.052, 0.060, 0.061, 0.062,
    ]
    .into_iter()
    .enumerate()
    {
        if frame == 6 {
            images.ready(&format!("{FIRST}#1"), egui::Color32::BLUE);
            displayed_id = ready_texture(poll(&ctx, &format!("{FIRST}#1"))).id;
            assert_ne!(displayed_id, ready_id, "新帧就绪后必须实际恢复播放");
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 600.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ui| app.render_central_panel(ui),
        );
        let image_drawn = ctx
            .tessellate(output.shapes.clone(), output.pixels_per_point)
            .iter()
            .any(|primitive| {
                matches!(&primitive.primitive, egui::epaint::Primitive::Mesh(mesh)
                if mesh.texture_id == displayed_id && mesh.calc_bounds().is_positive())
            });
        output.textures_delta.clear();
        assert!(
            image_drawn,
            "第 {frame} 帧必须实际画出 GIF 像素，不能退回加载转圈"
        );
    }
}
