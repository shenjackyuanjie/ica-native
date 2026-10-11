//! 图片预览回落的无窗口回归：只用内存假纹理与头部字节，不连 Bridge、不读配置、不落盘。
//! 覆盖 render_message_image 的两条真实回落：后缀/内容不符的 GIF↔静态图、GIF 帧元数据过期。
#![cfg(test)]

use crate::app::chat::message_card::render_message_image;
use crate::app::media::ImageSource;
use egui::load::{
    LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll,
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
