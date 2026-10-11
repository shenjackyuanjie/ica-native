//! 单条聊天消息的渲染。
//!
//! 这个文件只负责「一条消息怎么画」，包括文本富文本（@、表情、链接、合并转发占位）、
//! 图片/文件附件与回复引用。它本身不做布局缓存——行高估算与可见行裁剪由
//! `central_panel` 统一处理（`message_row_heights` / `message_row_layouts` 缓存 +
//! `show_viewport` 只渲染可见行），所以本文件在长列表里即使逐条执行也很轻量。
//!
//! 渲染流程分两段：先把正文解析成 `ContentSegment` 序列（文本/链接/表情/@/占位），
//! 再按序列绘制。解析与绘制分离的好处是同一份内容既可用于"是否真的有可见内容"
//! 判断（`has_visible_rich_content`），也可用于实际绘制，两者不会漂移。
//!
//! 交互上只返回 `MessageAction`，不自己发命令或改状态；因此本文件可以在 `&self`
//! 下运行，调用方（消息列表、搜索窗口、转发查看器）复用同一套右键菜单。

// 专属头像模块在本文件注册，避免与其他代理修改 chat/mod.rs 冲突。
#[path = "avatar.rs"]
pub mod avatar;
#[cfg(test)]
#[path = "tests/message_card_interaction.rs"]
mod interaction_tests;
#[cfg(test)]
#[path = "tests/message_card.rs"]
mod tests;

use super::{
    format_message_content, forward::forward_reference, image_url_looks_like_gif,
    is_image_file_type, is_video_file_type, should_probe_gif_after_static_error,
    try_load_gif_texture,
};
use crate::app::media::{ImageAction, ImageSource};
use crate::app::{IcaApp, MessageAction};
use crate::ica::types::{RoomId, files::MessageFile, message::ReplyMessage};
use avatar::{AvatarTarget, handle_avatar_response};
use chrono::Datelike;
use egui::{Hyperlink, Image, Label};
use linkify::{LinkFinder, LinkKind};
use std::borrow::Cow;
use std::time::Duration;

const LEGACY_AT_OPEN_TAG: &str = "<IcalinguaAt qq=";
const LEGACY_AT_CLOSE_TAG: &str = "</IcalinguaAt>";
const AT_OPEN_TAG: &str = "<IcaAt qq=";
const AT_CLOSE_TAG: &str = "</IcaAt>";
const FACE_OPEN_TAG: &str = "[Face: ";
const FORWARD_OPEN_TAG: &str = "[Forward: ";
const NESTED_FORWARD_OPEN_TAG: &str = "[NestedForward: ";
/// 消息头像边长（像素），同时用于圆形头像的圆角半径。
const MESSAGE_AVATAR_SIZE: f32 = 32.0;
/// 头像在行内占用的横向宽度：头像本身加 8 像素间距。
const MESSAGE_AVATAR_ROW_WIDTH: f32 = MESSAGE_AVATAR_SIZE + 8.0;

/// 正文里可识别的内联标记种类。
///
/// 解析时用"取最靠前的一个标记"的方式推进，因此需要这个枚举在多个候选里比较位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentMarker {
    /// `[Face: id]` QQ 表情。
    Face,
    /// `<IcaAt qq=..>` 或旧版 `<IcalinguaAt qq=..>` 的 @ 提及。
    Mention,
    /// `[Forward: res_id]` 合并转发占位。
    Forward,
    /// `[NestedForward: res_id]` 嵌套合并转发占位。
    NestedForward,
}

/// 把协议里的角色字段翻译成中文群头衔。
///
/// 空串、`member`、`unknown` 都视为普通成员，返回 `None` 表示不显示头衔；未识别的
/// 取值原样返回，避免新角色类型显示成空白。
fn sender_role_label(role: &str) -> Option<Cow<'_, str>> {
    let role = role.trim();
    match role.to_ascii_lowercase().as_str() {
        "" | "member" | "unknown" => None,
        "owner" => Some(Cow::Borrowed("群主")),
        "admin" | "administrator" => Some(Cow::Borrowed("管理员")),
        _ => Some(Cow::Borrowed(role)),
    }
}

/// 判断是否需要显示群身份信息（头衔、群名片）。
///
/// 仅群聊显示；来自 Telegram 桥（`mirai.eqq.type == "tg"`）的消息没有 QQ 式的群主/
/// 管理员/头衔概念，显示出来只会造成误解。
fn should_show_group_identity(room_id: RoomId, mirai: &serde_json::Value) -> bool {
    room_id < 0
        && mirai
            .pointer("/eqq/type")
            .and_then(serde_json::Value::as_str)
            != Some("tg")
}

/// 与 ICA 一致地按消息距离当前日期的远近显示时间。
///
/// 当天的消息只显示时刻（协议已给出的 `time_text`），同年显示月日，跨年才带年份，
/// 这样既不会过早出现"1970/01/01"式的冗余，也不会把旧消息显示成今天。
fn display_timestamp(message: &crate::ica::types::message::Message) -> String {
    let local_time = message.time.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    if local_time.date_naive() == now.date_naive() {
        return message.time_text.clone();
    }
    if local_time.year() == now.year() {
        return local_time.format("%m/%d %H:%M:%S").to_string();
    }
    local_time.format("%Y/%m/%d %H:%M:%S").to_string()
}

/// 按 Icalingua++ 的优先级选择聊天消息头像：Mirai 明示头像优先，否则使用 QQ 头像服务。
///
/// 依次尝试 `avatarMd5`（拼 qpic 地址）、`avatarUrl`（直接给出的地址），最后退回
/// 按 QQ 号拼 QQ 头像服务；`sender_id` 非正（系统或虚拟发送者）时不给头像。
fn message_avatar_url(sender_id: i64, mirai: &serde_json::Value) -> Option<String> {
    if let Some(md5) = mirai
        .pointer("/eqq/avatarMd5")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(format!(
            "https://gchat.qpic.cn/gchatpic_new/0/0-0-{}/0",
            md5.to_ascii_uppercase()
        ));
    }
    if let Some(url) = mirai
        .pointer("/eqq/avatarUrl")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(url.to_string());
    }
    (sender_id > 0).then(|| format!("https://q1.qlogo.cn/g?b=qq&nk={sender_id}&s=140"))
}

/// 解析后的一段正文内容。
///
/// `Text` 与 `Link` 借用原字符串；`Mention` 的显示名可能是 XML 实体解码后的新串，
/// 因此用 `Cow`。
#[derive(Debug, PartialEq, Eq)]
enum ContentSegment<'a> {
    /// 普通文本。
    Text(&'a str),
    /// 超链接：`text` 为显示文本，`target` 为规范化后的地址。
    Link { text: &'a str, target: Cow<'a, str> },
    /// QQ 表情，值为表情 id。
    Face(u16),
    /// @ 提及：`user_id == 1` 表示全体成员。
    Mention { user_id: i64, text: Cow<'a, str> },
    /// 合并转发占位，本身不绘制任何内容，只用于标记"这条消息有正文之外的实体"。
    Control,
}

/// 规范化链接地址：缺少协议时补 `https://`。
///
/// `linkify` 在 `url_must_have_scheme(false)` 下会把 `example.com` 也识别为链接，
/// 直接交给 `Hyperlink` 会被当成相对路径，因此这里统一补协议。
fn normalize_link_target(link: &str) -> Cow<'_, str> {
    if link.contains("://") {
        Cow::Borrowed(link)
    } else {
        Cow::Owned(format!("https://{link}"))
    }
}

/// 把一段纯文本按链接切成若干 `Text`/`Link` 段。
///
/// - `segments`：输出数组，追加写入。
///
/// 逐段扫描 ASCII 连续区间再交给 `linkify`：中文与 URL 混排时，`linkify` 容易把
/// 中文前后缀吞进链接里，按 ASCII 边界切可以让识别结果与肉眼判断一致。
fn push_text_segments<'a>(segments: &mut Vec<ContentSegment<'a>>, text: &'a str) {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    finder.url_must_have_scheme(false);

    let mut link_ranges = Vec::new();
    let mut ascii_start = None;
    for (index, character) in text.char_indices() {
        if character.is_ascii() {
            ascii_start.get_or_insert(index);
        } else if let Some(start) = ascii_start.take() {
            link_ranges.extend(
                finder
                    .links(&text[start..index])
                    .map(|link| (start + link.start(), start + link.end())),
            );
        }
    }
    if let Some(start) = ascii_start {
        link_ranges.extend(
            finder
                .links(&text[start..])
                .map(|link| (start + link.start(), start + link.end())),
        );
    }

    let mut cursor = 0;
    for (start, end) in link_ranges {
        if cursor < start {
            segments.push(ContentSegment::Text(&text[cursor..start]));
        }
        let link_text = &text[start..end];
        segments.push(ContentSegment::Link {
            text: link_text,
            target: normalize_link_target(link_text),
        });
        cursor = end;
    }
    if cursor < text.len() {
        segments.push(ContentSegment::Text(&text[cursor..]));
    }
}

/// 把消息正文解析成内容段序列。
///
/// 每轮从剩余文本里找出最靠前的内联标记，把它之前的部分当普通文本（含链接识别），
/// 再按标记类型处理。任何解析不完整的情况（缺 `>`、缺 `]`、id 非法）都把剩余部分
/// 原样当文本，保证用户内容不丢失。
fn parse_content_segments(content: &str) -> Vec<ContentSegment<'_>> {
    let mut segments = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        let Some((start, marker)) = [
            (remaining.find(FACE_OPEN_TAG), ContentMarker::Face),
            (remaining.find(LEGACY_AT_OPEN_TAG), ContentMarker::Mention),
            (remaining.find(AT_OPEN_TAG), ContentMarker::Mention),
            (remaining.find(FORWARD_OPEN_TAG), ContentMarker::Forward),
            (
                remaining.find(NESTED_FORWARD_OPEN_TAG),
                ContentMarker::NestedForward,
            ),
        ]
        .into_iter()
        .filter_map(|(index, marker)| index.map(|index| (index, marker)))
        .min_by_key(|(index, _)| *index) else {
            push_text_segments(&mut segments, remaining);
            break;
        };

        if start > 0 {
            push_text_segments(&mut segments, &remaining[..start]);
            remaining = &remaining[start..];
        }

        if marker == ContentMarker::Mention {
            let (open_tag, close_tag, xml_escaped) = if remaining.starts_with(LEGACY_AT_OPEN_TAG) {
                (LEGACY_AT_OPEN_TAG, LEGACY_AT_CLOSE_TAG, false)
            } else {
                (AT_OPEN_TAG, AT_CLOSE_TAG, true)
            };
            let Some(tag_end) = remaining.find('>') else {
                push_text_segments(&mut segments, remaining);
                break;
            };
            let body = &remaining[tag_end + 1..];
            let Some(close) = body.find(close_tag) else {
                push_text_segments(&mut segments, remaining);
                break;
            };
            let full_len = tag_end + 1 + close + close_tag.len();
            let user_id = remaining[open_tag.len()..tag_end]
                .parse::<i64>()
                .ok()
                .filter(|user_id| *user_id > 0);
            let encoded_text = &body[..close];
            if let Some(user_id) = user_id
                && !encoded_text.is_empty()
            {
                let text = if xml_escaped {
                    // 新版 <IcaAt> 的显示名按 XML 实体编码，旧版按 URL 编码，
                    // 两种都要解，否则 @ 后面跟着一串 %E4%B8%AD。
                    super::decode_xml_entities(encoded_text)
                } else {
                    urlencoding::decode(encoded_text).unwrap_or(Cow::Borrowed(encoded_text))
                };
                segments.push(ContentSegment::Mention { user_id, text });
            } else {
                push_text_segments(&mut segments, &remaining[..full_len]);
            }
            remaining = &remaining[full_len..];
            continue;
        }

        if matches!(
            marker,
            ContentMarker::Forward | ContentMarker::NestedForward
        ) {
            let open_tag = if marker == ContentMarker::Forward {
                FORWARD_OPEN_TAG
            } else {
                NESTED_FORWARD_OPEN_TAG
            };
            let after = &remaining[open_tag.len()..];
            let Some(end) = after.find(']') else {
                push_text_segments(&mut segments, remaining);
                break;
            };
            let full_len = open_tag.len() + end + 1;
            if after[..end].trim().is_empty() {
                push_text_segments(&mut segments, &remaining[..full_len]);
            } else {
                segments.push(ContentSegment::Control);
            }
            remaining = &remaining[full_len..];
            continue;
        }

        let after = &remaining[FACE_OPEN_TAG.len()..];
        let Some(end) = after.find(']') else {
            push_text_segments(&mut segments, remaining);
            break;
        };
        let full_len = FACE_OPEN_TAG.len() + end + 1;
        if let Ok(id) = after[..end].parse::<u16>()
            && crate::face_data::has_face(id)
        {
            segments.push(ContentSegment::Face(id));
        } else {
            // 本地没有这个表情的数据时退回纯文本：总比显示一个破损图片框好。
            push_text_segments(&mut segments, &remaining[..full_len]);
        }
        remaining = &remaining[full_len..];
    }

    segments
}

/// 判断一段正文里是否有"用户看得见"的内容。
///
/// 纯合并转发占位（`ContentSegment::Control`）不算可见：那种消息在列表里要显示成
/// "查看合并转发"按钮加预览，而不是一段空文本。转发查看器据此决定要不要补预览行。
pub fn has_visible_rich_content(content: &str) -> bool {
    parse_content_segments(content)
        .iter()
        .any(|segment| match segment {
            ContentSegment::Text(text) => !text.trim().is_empty(),
            ContentSegment::Link { .. } => true,
            ContentSegment::Face(_) | ContentSegment::Mention { .. } => true,
            ContentSegment::Control => false,
        })
}

/// 富文本绘制结果。
///
/// `response` 覆盖整块区域（含图片），`text_response` 只覆盖可选择的文本部分。
/// 两者分开是因为文本的点击会被 egui 的选词逻辑优先接管，需要单独合并到消息的
/// 右键菜单上。
struct RichContentRender {
    /// 整块富文本区域的响应。
    response: egui::Response,
    /// 仅文本部分的响应，可能为 `None`（例如整条都是图片）。
    text_response: Option<egui::Response>,
}

/// 把一个响应合并进"文本响应"累加器。
///
/// 用 `|=` 而不是替换：多个 Label 的悬停/点击状态需要合并成一个整体响应，
/// 否则只有最后一段文本能触发交互。
fn merge_text_response(combined: &mut Option<egui::Response>, response: egui::Response) {
    if let Some(combined) = combined {
        *combined |= response;
    } else {
        *combined = Some(response);
    }
}

/// 绘制富文本正文，可在前面加一个前缀（用于"回复 xxx: "）。
///
/// - `prefix`：可选前缀，随正文同一行排列；
/// - `content`：消息正文；
/// - `high_contrast`：高对比度模式，暗色下把正文提亮。
///
/// 没有前缀且整条都是普通文本时走单个 `Label` 快路：这条路径在长列表里占绝大多数，
/// 避免为每条消息都建一层 `horizontal_wrapped`。
fn render_rich_content_with_prefix(
    ui: &mut egui::Ui,
    prefix: Option<egui::RichText>,
    content: &str,
    high_contrast: bool,
) -> RichContentRender {
    let body_text_color = if high_contrast && ui.visuals().dark_mode {
        egui::Color32::from_rgb(0xf2, 0xf2, 0xf2)
    } else {
        ui.visuals().text_color()
    };
    let segments = parse_content_segments(content);
    let has_special_segment = segments
        .iter()
        .any(|segment| !matches!(segment, ContentSegment::Text(_)));
    if prefix.is_none() && !has_special_segment {
        let response =
            ui.add(Label::new(egui::RichText::new(content).color(body_text_color)).wrap());
        return RichContentRender {
            response: response.clone(),
            text_response: Some(response),
        };
    }

    let face_size = 24.0;
    let rendered = ui.horizontal_wrapped(|ui| {
        let mut text_response = None;
        // 段间距清零：文本、表情、@ 之间不能出现 egui 默认的 item_spacing，
        // 否则一句话被拆得稀碎。
        ui.spacing_mut().item_spacing.x = 0.0;
        if let Some(prefix) = prefix {
            ui.add(Label::new(prefix).wrap());
        }
        for seg in &segments {
            match seg {
                ContentSegment::Text(text) => {
                    if !text.is_empty() {
                        let response = ui.add(
                            Label::new(egui::RichText::new(*text).color(body_text_color)).wrap(),
                        );
                        merge_text_response(&mut text_response, response);
                    }
                }
                ContentSegment::Link { text, target } => {
                    let response = ui.add(
                        Hyperlink::from_label_and_url(*text, target.as_ref()).open_in_new_tab(true),
                    );
                    merge_text_response(&mut text_response, response);
                }
                ContentSegment::Face(id) => {
                    if let Some(bytes) = crate::face_data::get_face(*id) {
                        // 用 bytes:// 伪协议把内置表情喂给 egui 的图片加载器，
                        // 走统一的纹理缓存，重复出现同一表情不会重复解码。
                        let uri = format!("bytes://face_{id}");
                        let img = Image::from_bytes(uri, bytes)
                            .fit_to_exact_size(egui::vec2(face_size, face_size));
                        let response = ui.add(img);
                        if let Some(name) = crate::face_data::get_face_name(*id) {
                            response.on_hover_text(name);
                        }
                    }
                }
                ContentSegment::Mention { user_id, text } => {
                    let response = ui.add(
                        Label::new(
                            egui::RichText::new(text.as_ref())
                                .color(ui.visuals().hyperlink_color)
                                .strong(),
                        )
                        .wrap(),
                    );
                    let response = if *user_id == 1 {
                        response.on_hover_text("@全体成员")
                    } else {
                        response.on_hover_text(format!("QQ: {user_id}"))
                    };
                    merge_text_response(&mut text_response, response);
                }
                ContentSegment::Control => {}
            }
        }
        text_response
    });
    RichContentRender {
        response: rendered.response,
        text_response: rendered.inner,
    }
}

/// 渲染消息正文中的 QQ 表情和 @ 成员标记。
///
/// 返回整块区域的响应，调用方可基于它挂右键菜单或处理点击。
pub fn render_rich_content(
    ui: &mut egui::Ui,
    content: &str,
    high_contrast: bool,
) -> egui::Response {
    render_rich_content_with_prefix(ui, None, content, high_contrast).response
}

/// 从被回复消息里找出第一张图片附件。
///
/// 旧协议用单个 `file` 字段，新协议用 `files` 数组，这里把两者串起来统一查找，
/// 调用方不需要分辨版本。
fn reply_image_file(reply: &ReplyMessage) -> Option<&MessageFile> {
    reply
        .file
        .iter()
        .chain(&reply.files)
        .find(|file| is_image_file_type(&file.file_type))
}

/// 渲染消息中的图片缩略图并返回统一图片动作。
///
/// - `source`：图片地址与归属（会话/消息），用于"在聊天中定位"；
/// - `file_type`：协议给出的文件类型，参与 GIF 判定；
/// - `max_width` / `max_height`：预览槽尺寸。
///
/// 返回用户选择的 `ImageAction`（打开、复制、保存等），未交互时返回 `None`。
/// 预览槽尺寸固定，加载中/失败都不改变它，因此消息行高不会随加载状态抖动。
pub fn render_message_image(
    ui: &mut egui::Ui,
    source: &ImageSource,
    file_type: &str,
    max_width: f32,
    max_height: f32,
) -> Option<ImageAction> {
    let options = egui::TextureOptions::default();
    let hint = egui::load::SizeHint::default();
    let gif = should_try_gif_preview(&source.url, file_type)
        .then(|| try_load_gif_texture(ui.ctx(), &source.url, options, hint))
        .flatten();
    // 先按扩展名/MIME 猜是不是 GIF，猜不中再由静态加载器的报错反推：很多 CDN
    // 把 GIF 的 Content-Type 写成 image/jpeg，只有真的解不动了才发现。
    let poll = gif.unwrap_or_else(|| {
        let result = ui.ctx().try_load_texture(&source.url, options, hint);
        if result
            .as_ref()
            .is_err_and(should_probe_gif_after_static_error)
            && let Some(gif) = try_load_gif_texture(ui.ctx(), &source.url, options, hint)
        {
            return gif;
        }
        result
    });

    // 父级只分配一次固定预览槽。加载、失败和纹理淘汰都不能改写消息行高；
    // 子 UI 不参与父级 advance，长错误详情也不能把槽撑开。
    let slot_size = egui::vec2(max_width.min(ui.available_width()).max(0.0), max_height);
    let (slot, _) = ui.allocate_exact_size(slot_size, egui::Sense::hover());
    let mut preview = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(slot)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    preview.set_clip_rect(preview.clip_rect().intersect(slot));
    match poll {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            let scale = (slot.width() / texture.size.x.max(1.0))
                .min(slot.height() / texture.size.y.max(1.0))
                // 只缩小不放大：原图小于预览槽时保持原尺寸，避免糊图。
                .min(1.0);
            let image_size = texture.size * scale;
            let image_rect = egui::Align2([ui.layout().horizontal_align(), egui::Align::Min])
                .align_size_within_rect(image_size, slot);
            let response = preview.place(
                image_rect,
                Image::from_texture(texture)
                    .fit_to_exact_size(image_size)
                    .sense(egui::Sense::click()),
            );
            // 仅图片本身可点击，槽内留白不触发打开或右键菜单。
            image_response_action(response, source)
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            paint_image_loading(&mut preview, slot);
            None
        }
        Err(err) => {
            show_image_load_error(&mut preview, &err);
            None
        }
    }
}

/// 以恒定弧长显示加载状态，避免默认 Spinner 的扫角周期收缩到零。
///
/// 只为可见占位安排重绘；下载或解码完成时，加载器仍可立即唤醒界面。
fn paint_image_loading(ui: &mut egui::Ui, slot: egui::Rect) {
    let rect = egui::Rect::from_center_size(slot.center(), egui::Vec2::splat(20.0));
    // 在独立预览子 UI 中保留原 Spinner 的无障碍语义，不推进父级布局。
    ui.allocate_rect(rect, egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator));
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    let start = ui.input(|input| input.time.rem_euclid(1.0)) as f32 * std::f32::consts::TAU;
    let sweep = std::f32::consts::TAU * (2.0 / 3.0);
    let points = (0..=32)
        .map(|step| {
            let angle = start + sweep * step as f32 / 32.0;
            rect.center() + egui::vec2(angle.cos(), angle.sin()) * 8.0
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(3.0, ui.visuals().strong_text_color()),
    ));
}

/// 判断是否应该主动按 GIF 预览加载。
///
/// 文件类型里含 `gif`，或 URL 路径以 `.gif` 结尾（含 `.gif/` 这类带路径后缀的 CDN
/// 地址）时返回 `true`。
fn should_try_gif_preview(url: &str, file_type: &str) -> bool {
    let file_type = file_type.to_ascii_lowercase();
    if file_type.contains("gif") {
        return true;
    }

    image_url_looks_like_gif(url)
}

/// 给图片响应挂上统一的悬停光标与右键菜单，返回用户选择的动作。
///
/// 菜单项（复制图片、添加为表情、复制 URL、用查看器打开、保存、另存为）在所有出现
/// 图片的地方保持一致，因此收在这里供消息图片、回复图片等复用。
fn image_response_action(response: egui::Response, source: &ImageSource) -> Option<ImageAction> {
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut action = response
        .clicked()
        .then(|| ImageAction::Open(source.clone()));
    response.context_menu(|ui| {
        if ui.button("复制图片").clicked() {
            action = Some(ImageAction::CopyImage(source.clone()));
            ui.close();
        }
        if ui.button("添加为表情").clicked() {
            action = Some(ImageAction::AddSticker(source.clone()));
            ui.close();
        }
        if ui.button("复制 URL").clicked() {
            action = Some(ImageAction::CopyUrl(source.clone()));
            ui.close();
        }
        if ui.button("使用图片查看器打开").clicked() {
            action = Some(ImageAction::Open(source.clone()));
            ui.close();
        }
        if ui.button("保存图片").clicked() {
            action = Some(ImageAction::Save(source.clone()));
            ui.close();
        }
        if ui.button("另存为…").clicked() {
            action = Some(ImageAction::SaveAs(source.clone()));
            ui.close();
        }
    });
    action
}

/// 在预览槽内绘制图片加载失败提示。
///
/// "链接已过期"单独一种文案：这是 QQ 图片最常见的失败原因，提示用户"需要重新获取
/// 该图片 URL"比一串网络错误更有指导意义。详情放悬停，避免长错误文本把槽撑开。
fn show_image_load_error(ui: &mut egui::Ui, err: &egui::load::LoadError) {
    let err_text = err.to_string();
    let (label, details) = if err_text.contains("图片链接已过期") {
        ("图片链接已过期", "需要重新获取该图片 URL".to_owned())
    } else {
        ("图片加载失败", err_text)
    };
    ui.add(egui::Label::new(egui::RichText::new(label).color(egui::Color32::LIGHT_RED)).truncate())
        .on_hover_text(details);
}

/// 消息卡片的渲染选项。
///
/// 由调用方根据所处场景填写：消息列表、搜索结果、转发查看器对同一条消息的显示
/// 取舍不同，因此不做全局配置而是逐次传入。
pub struct MessageRenderOptions {
    /// 是否显示发送者名称（纯文本模式下同一人连续发送时可省略）。
    pub show_sender_name: bool,
    /// 是否在卡片前插入分隔线（纯文本模式下区分不同发送者）。
    pub show_separator_before: bool,
    /// 是否处于转发多选模式（显示勾选框）。
    pub forward_mode_active: bool,
    /// 本条是否已被选中。
    pub forward_selected: bool,
}

impl IcaApp {
    /// 绘制一条聊天消息卡片。
    ///
    /// - `room_id`：消息所属会话，决定群/私聊差异与头像是否显示；
    /// - `self_id`：当前登录账号，用于判断是否自己的消息（右侧对齐、可撤回）；
    /// - `message`：消息本体；
    /// - `options`：渲染选项。
    ///
    /// 返回用户在本条上选择的动作；没有任何交互时返回 `None`。系统消息提前返回，
    /// 不进入普通消息的头像/气泡/右键逻辑。双击气泡视为"回复"，与 Icalingua++ 一致。
    ///
    /// 本方法只读 `&self`：所有状态修改都通过返回的 `MessageAction` 由调用方完成，
    /// 因此可以在遍历消息时安全地反复调用。
    pub fn render_message_card(
        &self,
        ui: &mut egui::Ui,
        room_id: RoomId,
        self_id: i64,
        message: &crate::ica::types::message::Message,
        options: MessageRenderOptions,
    ) -> Option<MessageAction> {
        let is_self = self_id > 0 && message.sender_id == self_id;
        let formatted_content = format_message_content(&message.content);
        let forward_reference = forward_reference(message, None);
        let message_is_hidden = (message.deleted || message.hide) && !message.reveal;
        let pure_text_mode = self.custom_chat.hide_group_member_avatar;
        // 与 Icalingua++ 一致：只在群聊中为其他成员显示头像；自己、私聊和系统消息不显示。
        let sender_avatar_url =
            (!pure_text_mode && self.custom_chat.show_message_avatar && room_id < 0 && !is_self)
                .then(|| message_avatar_url(message.sender_id, &message.mirai))
                .flatten();

        // ── 系统消息：居中小字卡片，不显示头像/发送者 ──
        if message.system {
            if pure_text_mode && options.show_separator_before {
                ui.separator();
            }
            ui.add_space(2.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    let sys_bg = if ui.visuals().dark_mode {
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 77) // rgba(0,0,0,0.3)
                    } else {
                        egui::Color32::from_rgb(0xe5, 0xef, 0xfa) // #e5effa
                    };
                    let sys_text = if ui.visuals().dark_mode {
                        egui::Color32::from_rgb(0xbe, 0xc5, 0xcc) // #bec5cc
                    } else {
                        egui::Color32::from_rgb(0x50, 0x5a, 0x62) // #505a62
                    };
                    // 系统消息不参与右键菜单与头像，因此单独一个 Frame 就够了，
                    // 不需要走下面普通消息的那套 scope_builder。
                    egui::Frame::NONE
                        .fill(sys_bg)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(20, 8))
                        .show(ui, |ui| {
                            ui.style_mut().override_font_id =
                                Some(egui::FontId::proportional(12.0));
                            if !formatted_content.is_empty() {
                                ui.colored_label(sys_text, formatted_content.as_ref());
                            } else {
                                ui.colored_label(sys_text, "[系统消息]");
                            }
                        });
                },
            );
            ui.add_space(2.0);
            return None;
        }

        // ── 普通消息 ──
        let title_color = if message.deleted {
            egui::Color32::GRAY
        } else if is_self {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::LIGHT_BLUE
        };
        let mut action = None;
        let mut body_text_response = None;
        let mut bubble_double_clicked = false;
        let row_width = ui.available_width();
        let selection_width = if options.forward_mode_active {
            24.0
        } else {
            0.0
        };
        let avatar_row_width = if sender_avatar_url.is_some() {
            MESSAGE_AVATAR_ROW_WIDTH
        } else {
            0.0
        };
        let content_row_width = (row_width - selection_width - avatar_row_width).max(48.0);
        let pure_text_mode = self.custom_chat.hide_group_member_avatar;
        let bubble_width = if pure_text_mode {
            content_row_width
        } else {
            (content_row_width * 0.78)
                .clamp(72.0, 680.0)
                .min(content_row_width)
        };
        let content_align = if is_self {
            egui::Align::Max
        } else {
            egui::Align::Min
        };
        // 纯文本模式用分隔线代替头像区分发送者；有头像时不再需要分隔线。

        if pure_text_mode && options.show_separator_before {
            ui.separator();
            ui.add_space(4.0);
        }

        // 父区域先注册点击，再绘制子控件，egui 会把文本选词、图片和链接交给子控件。
        // 顺序反了的话子控件会抢走父级的点击，整条消息就点不出来了。
        let row_response = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .layout(egui::Layout::top_down(egui::Align::Min))
                    .sense(egui::Sense::click()),
                |ui| {
                    // 与 Icalingua++ 一致：头像、气泡等元素沿行底部对齐（flex-end）。
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 0.0),
                        egui::Layout::left_to_right(egui::Align::Max),
                        |ui| {
                            if options.forward_mode_active {
                                let mut checked = options.forward_selected;
                                if ui.checkbox(&mut checked, "").changed() {
                                    action = Some(MessageAction::ToggleForwardSelection {
                                        room_id,
                                        message_id: message.msg_id.clone(),
                                    });
                                }
                            }

                            if sender_avatar_url.is_some() {
                                // 为头像预留横向空间；图片本身在气泡画完之后按底部对齐放置。
                                // 先占位再画气泡，才能保证气泡宽度不受头像加载影响。
                                ui.add_space(MESSAGE_AVATAR_ROW_WIDTH);
                            }

                            if is_self {
                                let leading_space = (content_row_width - bubble_width).max(0.0);
                                if leading_space > 0.0 {
                                    ui.add_space(leading_space);
                                }
                            }

                            let bubble_inner =
                                ui.allocate_ui_with_layout(
                                    egui::vec2(bubble_width, 0.0),
                                    egui::Layout::top_down(content_align),
                                    |ui| {
                                        let mut render_message_contents =
                                            |ui: &mut egui::Ui| {
                                                ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if options.show_sender_name {
                                                    ui.colored_label(
                                                        title_color,
                                                        &message.sender_name,
                                                    );
                                                    if should_show_group_identity(
                                                        room_id,
                                                        &message.mirai,
                                                    ) {
                                                        if let Some(role) =
                                                            sender_role_label(&message.role)
                                                        {
                                                            let role_color = if message.deleted {
                                                                egui::Color32::GRAY
                                                            } else if role == "群主" {
                                                                ui.visuals().warn_fg_color
                                                            } else if role == "管理员" {
                                                                ui.visuals().hyperlink_color
                                                            } else {
                                                                ui.visuals().weak_text_color()
                                                            };
                                                            ui.colored_label(
                                                                role_color,
                                                                role.as_ref(),
                                                            );
                                                        }
                                                        let group_title = message.title.trim();
                                                        if !group_title.is_empty() {
                                                            ui.colored_label(
                                                                if message.deleted {
                                                                    egui::Color32::GRAY
                                                                } else {
                                                                    ui.visuals().weak_text_color()
                                                                },
                                                                group_title,
                                                            )
                                                            .on_hover_text("群头衔");
                                                        }
                                                    }
                                                }
                                                ui.weak(display_timestamp(message));
                                                if message.deleted {
                                                    ui.weak("已撤回");
                                                }
                                                if message.deleted
                                                    && is_self
                                                    && !message.content.trim().is_empty()
                                                    && ui.small_button("重新编辑").clicked()
                                                {
                                                    action = Some(MessageAction::ReEdit {
                                                        room_id,
                                                        content: message.content.clone(),
                                                    });
                                                }
                                                if !message.deleted
                                                    && !message.hide
                                                    && ui.small_button("回复").clicked()
                                                {
                                                    action = Some(MessageAction::Reply {
                                                        room_id,
                                                        reply: message.as_reply(),
                                                    });
                                                }
                                                if is_self
                                                    && !message.deleted
                                                    && ui.small_button("撤回").clicked()
                                                {
                                                    // 只能撤回自己的消息；别人的消息
                                                    // 协议层也不允许撤。
                                                    action = Some(MessageAction::Delete {
                                                        room_id,
                                                        message_id: message.msg_id.clone(),
                                                    });
                                                }
                                            });

                                            if message_is_hidden {
                                                ui.weak(if message.deleted {
                                                    "消息已撤回，右键显示"
                                                } else {
                                                    "消息已隐藏，右键显示"
                                                });
                                                return;
                                            }

                                            if let Some(reply) = &message.reply {
                                                let reply_msg_id = reply.msg_id.clone();
                                                let reply_image = reply_image_file(reply);
                                                if pure_text_mode {
                                                    let prefix = egui::RichText::new(format!(
                                                        "回复 {}: ",
                                                        reply.sender_name
                                                    ))
                                                    .color(ui.visuals().weak_text_color());
                                                    let content = if reply_image.is_some() {
                                                        // 纯文本模式不画引用里的小图，
                                                        // 用 [图片] 占位，否则引用区会把
                                                        // 消息卡片撑得很高。
                                                        Cow::Owned(
                                                            if reply.content.trim().is_empty() {
                                                                "[图片]".to_string()
                                                            } else {
                                                                format!("[图片] {}", reply.content)
                                                            },
                                                        )
                                                    } else {
                                                        Cow::Borrowed(reply.content.as_str())
                                                    };
                                                    if render_rich_content_with_prefix(
                                                        ui,
                                                        Some(prefix),
                                                        &content,
                                                        self.custom_chat.high_contrast_chat,
                                                    )
                                                    .response
                                                    .interact(egui::Sense::click())
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .clicked()
                                                    {
                                                        action =
                                                            Some(MessageAction::ScrollToMessage {
                                                                msg_id: reply_msg_id,
                                                            });
                                                    }
                                                } else {
                                                    let mut reply_image_action = None;
                                                    let reply_resp = egui::Frame::group(ui.style())
                                                        .show(ui, |ui| {
                                                            ui.weak(format!(
                                                                "回复 {}",
                                                                reply.sender_name
                                                            ));
                                                            if let Some(file) = reply_image {
                                                                if file.url.is_empty() {
                                                                    ui.weak("[图片]");
                                                                } else if self
                                                                    .custom_chat
                                                                    .hide_chat_img
                                                                {
                                                                    ui.weak("[图片已隐藏]");
                                                                } else {
                                                                    let source =
                                                                        ImageSource::message(
                                                                            file.url.clone(),
                                                                            room_id,
                                                                            reply.msg_id.clone(),
                                                                        );
                                                                    reply_image_action =
                                                                        render_message_image(
                                                                            ui,
                                                                            &source,
                                                                            &file.file_type,
                                                                            96.0,
                                                                            96.0,
                                                                        );
                                                                }
                                                            }
                                                            if !reply.content.trim().is_empty() {
                                                                render_rich_content(
                                                                    ui,
                                                                    &reply.content,
                                                                    self.custom_chat
                                                                        .high_contrast_chat,
                                                                );
                                                            }
                                                        });
                                                    if let Some(image_action) = reply_image_action {
                                                        action = Some(MessageAction::Image(
                                                            image_action,
                                                        ));
                                                    } else if reply_resp
                                                        .response
                                                        .interact(egui::Sense::click())
                                                        .on_hover_cursor(
                                                            egui::CursorIcon::PointingHand,
                                                        )
                                                        .clicked()
                                                    {
                                                        action =
                                                            Some(MessageAction::ScrollToMessage {
                                                                msg_id: reply_msg_id,
                                                            });
                                                    }
                                                }
                                            }

                                            let mut has_body = false;

                                            let has_visible_content =
                                                has_visible_rich_content(&message.content);
                                            if has_visible_content {
                                                has_body = true;
                                                let rendered = render_rich_content_with_prefix(
                                                    ui,
                                                    None,
                                                    &message.content,
                                                    self.custom_chat.high_contrast_chat,
                                                );
                                                body_text_response = rendered.text_response;
                                            }

                                            if !message.files.is_empty() {
                                                has_body = true;
                                                ui.with_layout(
                                            egui::Layout::top_down(content_align),
                                            |ui| {
                                                for (attachment_index, file) in message.files.iter().enumerate() {
                                                    let is_image =
                                                        is_image_file_type(&file.file_type);

                                                    if self.render_chat_voice(ui, room_id, &message.msg_id, attachment_index, file) {
                                                        // 语音有专门的渲染路径（波形+播放），
                                                        // 命中后不再按普通附件处理。
                                                        continue;
                                                    }
                                                    if is_image && self.custom_chat.hide_chat_img {
                                                        ui.weak("[图片已隐藏]");
                                                    } else if is_video_file_type(&file.file_type)
                                                        && self.custom_chat.hide_chat_video
                                                    {
                                                        ui.weak("[视频已隐藏]");
                                                    } else if is_image && !file.url.is_empty() {
                                                        let image_max_width =
                                                            ui.available_width().min(240.0);
                                                        let source = ImageSource::message(
                                                            file.url.clone(),
                                                            room_id,
                                                            message.msg_id.clone(),
                                                        );
                                                        if let Some(image_action) =
                                                            render_message_image(
                                                                ui,
                                                                &source,
                                                                &file.file_type,
                                                                image_max_width,
                                                                240.0,
                                                            )
                                                        {
                                                            action = Some(MessageAction::Image(
                                                                image_action,
                                                            ));
                                                        }
                                                    } else {
                                                        let label = file
                                                            .get_name()
                                                            .cloned()
                                                            .unwrap_or_else(|| {
                                                                file.file_type.clone()
                                                            });
                                                        // 没有 URL 的附件只是纯标签（尚未上传成功
                                                        // 或已过期），有 URL 时做成可点链接。
                                                        if file.url.is_empty() {
                                                            ui.label(label);
                                                        } else {
                                                            ui.add(Hyperlink::from_label_and_url(
                                                                label,
                                                                file.url.clone(),
                                                            ));
                                                        }
                                                    }

                                                    ui.add_space(4.0);
                                                }
                                            },
                                        );
                                            }

                                            if let Some(reference) = &forward_reference {
                                                has_body = true;
                                                if !has_visible_content {
                                                    // 纯转发消息没有正文，用预览行占位，
                                                    // 否则整条消息只有一个孤零零的按钮。
                                                    super::forward::render_forward_preview(
                                                        ui, reference,
                                                    );
                                                }
                                                let mut response = ui.button("查看合并转发");
                                                if !reference.preview.is_empty() {
                                                    response = response.on_hover_text(
                                                        reference.preview.join("\n"),
                                                    );
                                                }
                                                if response.clicked() {
                                                    action = Some(MessageAction::OpenForward {
                                                        res_id: reference.res_id.clone(),
                                                        file_name: reference.file_name.clone(),
                                                        fallback_res_id: reference
                                                            .fallback_res_id
                                                            .clone(),
                                                        inline_messages: reference
                                                            .inline_messages
                                                            .clone(),
                                                    });
                                                }
                                            }

                                            if !has_body {
                                                ui.weak("[空消息]");
                                            }
                                        },
                                    );
                                            };

                                        let frame = if pure_text_mode {
                                            egui::Frame::NONE
                                        } else {
                                            egui::Frame::group(ui.style())
                                        };
                                        let response = ui
                                            .scope_builder(
                                                egui::UiBuilder::new().sense(egui::Sense::click()),
                                                |ui| {
                                                    frame.show(ui, |ui| {
                                                        render_message_contents(ui);
                                                    });
                                                },
                                            )
                                            .response;

                                        // 必须在合并正文响应之前判断；否则文本的双击选词会被误当成回复。
                                        bubble_double_clicked |= response.double_clicked();

                                        // 可选择文字会优先接收点击；合并其响应后，正文上的右键也能打开消息菜单。
                                        let response = if let Some(body_text_response) =
                                            body_text_response.take()
                                        {
                                            response.union(body_text_response)
                                        } else {
                                            response
                                        };

                                        response.context_menu(|ui| {
                                            if message_is_hidden {
                                                // 已撤回/已隐藏的消息只给一个"显示"入口，
                                                // 其余操作对它都没有意义。
                                                if ui.button("显示").clicked() {
                                                    action = Some(MessageAction::SetReveal {
                                                        room_id,
                                                        message_id: message.msg_id.clone(),
                                                        reveal: true,
                                                    });
                                                    ui.close();
                                                }
                                                return;
                                            }

                                            if !message.deleted
                                                && !message.hide
                                                && ui.button("回复").clicked()
                                            {
                                                action = Some(MessageAction::Reply {
                                                    room_id,
                                                    reply: message.as_reply(),
                                                });
                                                ui.close();
                                            }
                                            if ui.button("复制到编辑区").clicked() {
                                                action = Some(MessageAction::CopyToDraft {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                });
                                                ui.close();
                                            }
                                            if !message.content.trim().is_empty()
                                                && ui.button("复制文本").clicked()
                                            {
                                                ui.ctx().copy_text(message.content.clone());
                                                ui.close();
                                            }
                                            if ui.button("重新获取该消息内容").clicked() {
                                                action = Some(MessageAction::RenewMessage {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                });
                                                ui.close();
                                            }
                                            if ui.button("复制消息 ID").clicked() {
                                                ui.ctx().copy_text(message.msg_id.clone());
                                                ui.close();
                                            }
                                            if ui.button("+1").clicked() {
                                                action = Some(MessageAction::PlusOne {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                });
                                                ui.close();
                                            }
                                            if ui.button("转发").clicked() {
                                                action = Some(MessageAction::StartForward {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                });
                                                ui.close();
                                            }
                                            if let Some(reference) = &forward_reference
                                                && ui.button("查看合并转发").clicked()
                                            {
                                                action = Some(MessageAction::OpenForward {
                                                    res_id: reference.res_id.clone(),
                                                    file_name: reference.file_name.clone(),
                                                    fallback_res_id: reference
                                                        .fallback_res_id
                                                        .clone(),
                                                    inline_messages: reference
                                                        .inline_messages
                                                        .clone(),
                                                });
                                                ui.close();
                                            }
                                            if !is_self
                                                && should_show_group_identity(
                                                    room_id,
                                                    &message.mirai,
                                                )
                                                && crate::ica::client::poke_target(
                                                    room_id,
                                                    message.sender_id,
                                                )
                                                .is_some()
                                                && ui.button("戳一戳").clicked()
                                            {
                                                // 与头像菜单一致：先问协议层这个目标
                                                // 能不能戳，再决定是否显示入口。
                                                action = Some(MessageAction::Poke {
                                                    room_id,
                                                    target_id: message.sender_id,
                                                });
                                                ui.close();
                                            }
                                            if ui
                                                .button(if options.forward_selected {
                                                    "移出多选"
                                                } else {
                                                    "多选"
                                                })
                                                .clicked()
                                            {
                                                action =
                                                    Some(MessageAction::ToggleForwardSelection {
                                                        room_id,
                                                        message_id: message.msg_id.clone(),
                                                    });
                                                ui.close();
                                            }
                                            if is_self
                                                && !message.deleted
                                                && ui.button("撤回").clicked()
                                            {
                                                action = Some(MessageAction::Delete {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                });
                                                ui.close();
                                            }
                                            if message.deleted
                                                && is_self
                                                && !message.content.trim().is_empty()
                                                && ui.button("重新编辑").clicked()
                                            {
                                                action = Some(MessageAction::ReEdit {
                                                    room_id,
                                                    content: message.content.clone(),
                                                });
                                                ui.close();
                                            }
                                            if (message.deleted || message.hide || message.reveal)
                                                && ui.button("隐藏").clicked()
                                            {
                                                action = Some(MessageAction::SetReveal {
                                                    room_id,
                                                    message_id: message.msg_id.clone(),
                                                    reveal: false,
                                                });
                                                ui.close();
                                            }
                                        });
                                    },
                                );
                            // 与 Icalingua++ 的 flex-end 一致：头像底边与消息气泡底边对齐。
                            // 头像在气泡之后才 put，因此这里能直接读到气泡的最终矩形。
                            if let Some(avatar_url) = &sender_avatar_url {
                                let bubble_rect = bubble_inner.response.rect;
                                let avatar_rect = egui::Rect::from_min_size(
                                    egui::pos2(
                                        bubble_rect.left() - MESSAGE_AVATAR_ROW_WIDTH,
                                        bubble_rect.bottom() - MESSAGE_AVATAR_SIZE,
                                    ),
                                    egui::vec2(MESSAGE_AVATAR_SIZE, MESSAGE_AVATAR_SIZE),
                                );
                                let response = ui.put(
                                    avatar_rect,
                                    Image::from_uri(avatar_url.clone())
                                        .sense(egui::Sense::click())
                                        .fit_to_exact_size(egui::vec2(
                                            MESSAGE_AVATAR_SIZE,
                                            MESSAGE_AVATAR_SIZE,
                                        ))
                                        .corner_radius(MESSAGE_AVATAR_SIZE / 2.0),
                                );
                                if let Some(avatar_action) = handle_avatar_response(
                                    response,
                                    AvatarTarget {
                                        room_id,
                                        user_id: message.sender_id,
                                        name: &message.sender_name,
                                        image_url: avatar_url,
                                        is_qq_user: should_show_group_identity(
                                            room_id,
                                            &message.mirai,
                                        ),
                                    },
                                ) {
                                    action = Some(avatar_action);
                                }
                            }
                        },
                    );
                },
            )
            .response;
        if action.is_none()
            && (bubble_double_clicked || row_response.double_clicked())
            && !options.forward_mode_active
            && !message.deleted
            && !message.hide
            && !message.flash
        {
            // 双击回复：多选模式下双击要保留给"勾选"，闪照不允许回复。
            action = Some(MessageAction::Reply {
                room_id,
                reply: message.as_reply(),
            });
        }
        ui.add_space(if pure_text_mode { 2.0 } else { 4.0 });
        action
    }
}
