//! 图片预览回落的无窗口回归：只用内存假纹理与头部字节，不连 Bridge、不读配置、不落盘。
//! 覆盖预览格式回落、错误槽，以及首次下载、解码和就绪之间的占位可见性与固定几何。
#![cfg(test)]

use crate::app::chat::message_card::render_message_image;
use crate::app::media::ImageSource;
use egui::load::{
    BytesLoadResult, BytesLoader, BytesPoll, LoadError, SizeHint, SizedTexture, TextureLoadResult,
    TextureLoader, TexturePoll,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PREVIEW_PREFIX: &str = "bytes://preview-regression";
/// 假纹理的固定 id 与尺寸，用来在 tessellate 结果里分辨「真的画出了图片」。
const IMAGE_TEXTURE: egui::TextureId = egui::TextureId::User(42);
const IMAGE_SIZE: egui::Vec2 = egui::vec2(200.0, 100.0);

/// 假纹理加载器要模拟的三种真实故障。
#[derive(Clone, Copy)]
enum PreviewMode {
    /// 普通 URI 静态解码报格式不支持（后缀 PNG、字节却是 GIF），只有帧 URI 能出图。
    StaticFormatErrorThenGif,
    /// 普通 URI 是静态图，帧 URI 报格式不支持（GIF 帧元数据已过期）。
    StaticImageWithStaleFrame,
    /// 两个 URI 都报「图片链接已过期」，用于真实错误槽的绘制断言。
    ExpiredLink,
}

struct FakeTextures(Mutex<PreviewMode>);

impl TextureLoader for FakeTextures {
    fn id(&self) -> &str {
        concat!(module_path!(), "::FakeTextures")
    }

    fn load(
        &self,
        _: &egui::Context,
        uri: &str,
        _: egui::TextureOptions,
        _: SizeHint,
    ) -> TextureLoadResult {
        if !uri.starts_with(PREVIEW_PREFIX) {
            return Err(LoadError::NotSupported);
        }
        // 帧 URI 由上层拼成 "{源 URI}#{帧号}"，与生产 gif_frame_uri 的形状一致。
        let is_frame_uri = uri.contains('#');
        match *self.0.lock().unwrap() {
            PreviewMode::StaticFormatErrorThenGif if !is_frame_uri => static_format_error(),
            PreviewMode::StaticImageWithStaleFrame if is_frame_uri => static_format_error(),
            PreviewMode::ExpiredLink => Err(LoadError::Loading("图片链接已过期".into())),
            _ => Ok(TexturePoll::Ready {
                texture: SizedTexture {
                    id: IMAGE_TEXTURE,
                    size: IMAGE_SIZE,
                },
            }),
        }
    }

    fn forget(&self, _: &str) {}
    fn forget_all(&self) {}
    fn byte_size(&self) -> usize {
        0
    }
}

fn static_format_error() -> TextureLoadResult {
    Err(LoadError::FormatNotSupported {
        detected_format: None,
    })
}

fn context(mode: PreviewMode) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.add_texture_loader(Arc::new(FakeTextures(Mutex::new(mode))));
    ctx
}

/// 递归收集形状里的文本（文本也可能嵌在 Shape::Vec 里）。
fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.job.text.clone()),
        egui::Shape::Vec(inner) => inner.iter().for_each(|shape| collect_texts(shape, texts)),
        _ => {}
    }
}

/// 确定性绘制两帧，返回第二帧的 (User(42) 图片网格, 固定预览槽, 全部文本)。
fn draw_two_frames(
    ctx: &egui::Context,
    source: &ImageSource,
    file_type: &str,
) -> (Option<egui::Rect>, egui::Rect, Vec<String>) {
    let mut result = (None, egui::Rect::NOTHING, Vec::new());
    for frame in 0..2 {
        let mut slot_rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 400.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ui| {
                // 每帧保持同一 UI 层级与尺寸，与真实消息卡片的连续绘制一致。
                slot_rect = ui
                    .scope(|ui| {
                        render_message_image(ui, source, file_type, 240.0, 240.0);
                    })
                    .response
                    .rect;
            },
        );
        let mut texts = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|clipped| collect_texts(&clipped.shape, &mut texts));
        let mut image_rect = None;
        for primitive in ctx.tessellate(output.shapes.clone(), output.pixels_per_point) {
            if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive
                && mesh.texture_id == IMAGE_TEXTURE
            {
                image_rect = Some(mesh.calc_bounds());
            }
        }
        // 测试没有渲染后端，显式丢弃本帧生成的字体纹理更新。
        output.textures_delta.clear();
        result = (image_rect, slot_rect, texts);
    }
    result
}

/// 断言本帧真的画出了图片像素，而不是退回错误槽。
fn assert_image_drawn(image_rect: Option<egui::Rect>, texts: &[String], why: &str) {
    let rect = image_rect.unwrap_or_else(|| panic!("{why}；实际文本: {texts:?}"));
    assert!(
        rect.width() > 0.0 && rect.height() > 0.0,
        "{why}，但网格尺寸为空: {rect:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("图片加载失败")),
        "{why}，不应出现错误槽: {texts:?}"
    );
}

#[test]
fn png_url_with_gif_bytes_falls_back_to_gif_frame() {
    // file_type 与后缀都是 PNG，字节却是 GIF：静态解码先报格式不支持，
    // message_card 的 GIF 回落必须重新探测并画出帧纹理，而不是停在错误槽。
    let ctx = context(PreviewMode::StaticFormatErrorThenGif);
    let url = format!("{PREVIEW_PREFIX}-mismatch.png");
    ctx.include_bytes(url.clone(), &b"GIF89a"[..]);
    let (image_rect, slot_rect, texts) = draw_two_frames(&ctx, &ImageSource::url(url), "image/png");
    assert_image_drawn(
        image_rect,
        &texts,
        "静态解码报格式不支持后必须回落探测 GIF 帧",
    );
    assert!(
        (slot_rect.height() - 240.0).abs() <= 1.0,
        "回落过程必须保持固定预览槽高度，实际 {}",
        slot_rect.height()
    );
}

#[test]
fn gif_type_with_static_bytes_drops_stale_frame_metadata() {
    // 旧帧时长元数据 + 已成静态图的字节：必须清掉过期元数据并落到静态 Ready，
    // 否则后续每一帧都会拿 "#0" 去探测一个不再存在的动画。
    let ctx = context(PreviewMode::StaticImageWithStaleFrame);
    let url = format!("{PREVIEW_PREFIX}-stale.gif");
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(url.as_str()),
            egui::FrameDurations::new(vec![Duration::from_millis(40)]),
        )
    });
    ctx.include_bytes(url.clone(), &b"\x89PNG\r\n\x1a\n"[..]);
    let (image_rect, _, texts) = draw_two_frames(&ctx, &ImageSource::url(url.clone()), "image/gif");
    assert_image_drawn(image_rect, &texts, "GIF 帧元数据过期时必须落回静态图片");
    assert!(
        ctx.data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(url.as_str())))
            .is_none(),
        "过期 GIF 帧元数据必须被移除，否则每帧都会重复探测失败"
    );
}

#[test]
fn expired_link_error_slot_keeps_label_and_fixed_height() {
    // 真实错误槽：过期链接必须给出可读文案，并且不得撑大或压缩父级的固定预览槽。
    let ctx = context(PreviewMode::ExpiredLink);
    let source = ImageSource::url(format!("{PREVIEW_PREFIX}-expired.png"));
    let (image_rect, slot_rect, texts) = draw_two_frames(&ctx, &source, "image/png");
    assert!(image_rect.is_none(), "过期链接不得画出任何图片像素");
    assert!(
        texts.iter().any(|text| text.as_str() == "图片链接已过期"),
        "错误槽必须给出过期提示，实际文本: {texts:?}"
    );
    assert!(
        (slot_rect.height() - 240.0).abs() <= 1.0,
        "错误槽必须保持固定高度，实际 {}",
        slot_rect.height()
    );
}

/// 首次加载的两层状态由测试逐帧推进，不需要网络或后台任务的计时猜测。
#[derive(Clone, Copy)]
enum ColdStage {
    Downloading(Option<egui::Vec2>),
    Decoding(Option<egui::Vec2>),
    Ready,
    Failed,
}

struct ColdPreview {
    stage: Mutex<ColdStage>,
    requested_frames: Mutex<Vec<String>>,
}

impl BytesLoader for ColdPreview {
    fn id(&self) -> &str {
        concat!(module_path!(), "::ColdBytes")
    }

    fn load(&self, _: &egui::Context, uri: &str) -> BytesLoadResult {
        if !uri.starts_with(PREVIEW_PREFIX) {
            return Err(LoadError::NotSupported);
        }
        match *self.stage.lock().unwrap() {
            ColdStage::Downloading(size) => Ok(BytesPoll::Pending { size }),
            ColdStage::Failed => Err(LoadError::Loading("图片链接已过期".into())),
            _ => Ok(BytesPoll::Ready {
                bytes: egui::load::Bytes::Static(b"GIF89a"),
                mime: Some("image/gif".into()),
                size: None,
            }),
        }
    }

    fn forget(&self, _: &str) {}
    fn forget_all(&self) {}
    fn byte_size(&self) -> usize {
        0
    }
}

impl TextureLoader for ColdPreview {
    fn id(&self) -> &str {
        concat!(module_path!(), "::ColdTextures")
    }

    fn load(
        &self,
        ctx: &egui::Context,
        uri: &str,
        _: egui::TextureOptions,
        _: SizeHint,
    ) -> TextureLoadResult {
        if !uri.starts_with(PREVIEW_PREFIX) {
            return Err(LoadError::NotSupported);
        }
        self.requested_frames.lock().unwrap().push(uri.into());
        match *self.stage.lock().unwrap() {
            ColdStage::Downloading(size) | ColdStage::Decoding(size) => {
                Ok(TexturePoll::Pending { size })
            }
            ColdStage::Ready => {
                // 真实 GIF 加载器也在首帧就绪的同次调用中安装时长元数据。
                let (base, _) = egui::decode_animated_image_uri(uri).unwrap();
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(base),
                        egui::FrameDurations::new(vec![Duration::from_millis(40); 2]),
                    );
                });
                Ok(TexturePoll::Ready {
                    texture: SizedTexture {
                        id: IMAGE_TEXTURE,
                        size: IMAGE_SIZE,
                    },
                })
            }
            ColdStage::Failed => Err(LoadError::Loading("图片链接已过期".into())),
        }
    }

    fn forget(&self, _: &str) {}
    fn forget_all(&self) {}
    fn byte_size(&self) -> usize {
        0
    }
}

fn cold_preview() -> (egui::Context, Arc<ColdPreview>, ImageSource) {
    let ctx = egui::Context::default();
    let loader = Arc::new(ColdPreview {
        stage: Mutex::new(ColdStage::Downloading(None)),
        requested_frames: Mutex::new(Vec::new()),
    });
    ctx.add_bytes_loader(loader.clone());
    ctx.add_texture_loader(loader.clone());
    let source = ImageSource::url(format!("{PREVIEW_PREFIX}-cold.gif"));
    (ctx, loader, source)
}

struct ColdFrame {
    slot: egui::Rect,
    arcs: Vec<egui::epaint::PathShape>,
    image: Option<egui::Rect>,
    texts: Vec<String>,
    repaint_delay: Duration,
    progress_indicators: usize,
}

fn draw_cold_frame(
    ctx: &egui::Context,
    source: &ImageSource,
    time: f64,
    visible: bool,
) -> ColdFrame {
    let mut slot = egui::Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 400.0),
            )),
            time: Some(time),
            ..Default::default()
        },
        |ui| {
            if !visible {
                ui.set_clip_rect(egui::Rect::from_min_size(
                    egui::pos2(300.0, 380.0),
                    egui::Vec2::splat(10.0),
                ));
            }
            slot = ui
                .scope(|ui| {
                    render_message_image(ui, source, "image/gif", 240.0, 240.0);
                })
                .response
                .rect;
        },
    );
    let mut arcs = Vec::new();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
        if let egui::Shape::Path(path) = &clipped.shape
            && !path.closed
            && !path.stroke.is_empty()
            && slot.contains_rect(path.visual_bounding_rect())
        {
            arcs.push(path.clone());
        }
    }
    let image = ctx
        .tessellate(output.shapes.clone(), output.pixels_per_point)
        .into_iter()
        .find_map(|primitive| match primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == IMAGE_TEXTURE => {
                Some(mesh.calc_bounds())
            }
            _ => None,
        });
    let repaint_delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
    let progress_indicators =
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .map_or(0, |update| {
                update
                    .nodes
                    .iter()
                    .filter(|(_, node)| node.role() == egui::accesskit::Role::ProgressIndicator)
                    .count()
            });
    output.textures_delta.clear();
    ColdFrame {
        slot,
        arcs,
        image,
        texts,
        repaint_delay,
        progress_indicators,
    }
}

#[test]
fn pending_preview_arc_does_not_shrink_or_blink_during_download_and_decode() {
    let (ctx, loader, source) = cold_preview();
    let mut baseline: Option<(egui::Rect, f32, egui::epaint::PathStroke)> = None;
    // 横跨默认 Spinner 的多个收缩零点；尺寸提示与下载/解码阶段也不能重置占位。
    for frame in 0..361 {
        *loader.stage.lock().unwrap() = match frame % 4 {
            0 => ColdStage::Downloading(None),
            1 => ColdStage::Downloading(Some(egui::vec2(200.0, 100.0))),
            2 => ColdStage::Decoding(None),
            _ => ColdStage::Decoding(Some(egui::vec2(400.0, 300.0))),
        };
        let drawn = draw_cold_frame(
            &ctx,
            &source,
            frame as f64 * std::f64::consts::PI / 90.0,
            true,
        );
        assert!(drawn.image.is_none(), "首次 Pending 不能提前画出图片");
        assert_eq!(drawn.arcs.len(), 1, "加载中始终应有一个可见旋转弧");
        let arc = &drawn.arcs[0];
        let length: f32 = arc.points.windows(2).map(|p| p[0].distance(p[1])).sum();
        assert!(length >= 30.0, "第 {frame} 帧加载弧缩成点或消失: {length}");
        if let Some((slot, first_length, ref stroke)) = baseline {
            assert_eq!(drawn.slot, slot, "加载阶段切换不能改变预览几何");
            assert!((length - first_length).abs() < 0.1, "旋转弧长度必须恒定");
            assert_eq!(&arc.stroke, stroke, "加载指示不能周期改变亮度或线宽");
        } else {
            baseline = Some((drawn.slot, length, arc.stroke.clone()));
        }
    }
}

#[test]
fn cold_preview_replaces_loading_arc_with_pixels_without_an_empty_frame() {
    let (ctx, loader, source) = cold_preview();
    let initial = draw_cold_frame(&ctx, &source, 0.25, true).slot;
    for (time, stage) in [
        (0.50, ColdStage::Downloading(None)),
        (0.55, ColdStage::Downloading(Some(egui::vec2(400.0, 300.0)))),
        (0.60, ColdStage::Decoding(None)),
        (0.65, ColdStage::Decoding(Some(egui::vec2(200.0, 100.0)))),
        (0.70, ColdStage::Ready),
        // 80ms 周期的 70ms 处请求第 1 帧，避免“帧号永远为 0”也通过。
        (0.79, ColdStage::Ready),
        (0.83, ColdStage::Failed),
    ] {
        *loader.stage.lock().unwrap() = stage;
        loader.requested_frames.lock().unwrap().clear();
        let drawn = draw_cold_frame(&ctx, &source, time, true);
        assert_eq!(drawn.slot, initial, "从下载到图片出现不能改变槽高或位置");
        match stage {
            ColdStage::Ready => {
                assert_image_drawn(drawn.image, &drawn.texts, "首帧及后续帧必须直接绘制图片");
                assert!(drawn.arcs.is_empty(), "就绪后不能继续叠加加载动画");
                let index = if time < 0.75 { 0 } else { 1 };
                let expected = format!("{}#{index}", source.url);
                assert_eq!(
                    loader.requested_frames.lock().unwrap().last(),
                    Some(&expected)
                );
            }
            ColdStage::Failed => {
                assert!(drawn.image.is_none() && drawn.arcs.is_empty());
                assert!(drawn.texts.iter().any(|text| text == "图片链接已过期"));
            }
            _ => {
                assert!(drawn.image.is_none());
                assert_eq!(drawn.arcs.len(), 1, "下载与解码之间不能空白一帧");
            }
        }
    }
}

#[test]
fn pending_preview_keeps_progress_semantics_until_pixels_or_error_replace_it() {
    let (ctx, loader, source) = cold_preview();
    ctx.enable_accesskit();
    let mut slot = None;
    for (frame, stage) in [
        ColdStage::Downloading(None),
        ColdStage::Decoding(None),
        ColdStage::Ready,
        ColdStage::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        *loader.stage.lock().unwrap() = stage;
        let drawn = draw_cold_frame(&ctx, &source, frame as f64 / 10.0, true);
        let expected = match stage {
            ColdStage::Downloading(_) | ColdStage::Decoding(_) => 1,
            _ => 0,
        };
        assert_eq!(
            drawn.progress_indicators, expected,
            "加载占位应向辅助技术报告进度，就绪或失败后应由对应内容替换"
        );
        if let Some(initial) = slot {
            assert_eq!(drawn.slot, initial, "无障碍标记不能改变预览槽布局");
        } else {
            slot = Some(drawn.slot);
        }
    }
}

#[test]
fn pending_preview_repaint_is_paced_and_stops_when_indicator_is_clipped() {
    let (ctx, _, source) = cold_preview();
    let mut last = None;
    // 先耗尽首次布局的额外重绘；egui 记录的延迟已扣除 predicted_dt，
    // 因此这里只检查延迟为正且有界，不把记录值误认为请求的完整 33ms。
    for frame in 0..6 {
        last = Some(draw_cold_frame(
            &ctx,
            &source,
            1.0 + frame as f64 / 30.0,
            true,
        ));
    }
    let visible = last.unwrap();
    assert!(
        visible.repaint_delay > Duration::ZERO,
        "加载占位不应请求满速空转"
    );
    assert!(
        visible.repaint_delay <= Duration::from_millis(33),
        "可见占位必须继续转动"
    );
    let hidden = draw_cold_frame(&ctx, &source, 1.3, false);
    assert!(hidden.arcs.is_empty(), "裁剪外的占位不需要绘制");
    assert_eq!(
        hidden.repaint_delay,
        Duration::MAX,
        "离屏占位不能驱动动画重绘"
    );
}
