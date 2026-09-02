//! 单条聊天消息的渲染。
//!
//! 这个文件只负责「一条消息怎么画」，包括文本富文本（@、表情、链接、合并转发占位）、
//! 图片/文件附件与回复引用。它本身不做布局缓存——行高估算与可见行裁剪由
//! `central_panel` 统一处理（`message_row_heights` / `message_row_layouts` 缓存 +
//! `show_viewport` 只渲染可见行），所以本文件在长列表里即使逐条执行也很轻量。

use std::borrow::Cow;

use crate::app::media::{ImageAction, ImageSource};
use crate::app::{IcaApp, MessageAction};
use crate::ica::types::{RoomId, files::MessageFile, message::ReplyMessage};
use chrono::Datelike;

use egui::{Hyperlink, Image, Label};
use linkify::{LinkFinder, LinkKind};

use super::{
    format_message_content, forward::forward_reference, image_url_looks_like_gif,
    is_image_file_type, is_video_file_type, should_probe_gif_after_static_error,
    try_load_gif_texture,
};

const LEGACY_AT_OPEN_TAG: &str = "<IcalinguaAt qq=";
const LEGACY_AT_CLOSE_TAG: &str = "</IcalinguaAt>";
const AT_OPEN_TAG: &str = "<IcaAt qq=";
const AT_CLOSE_TAG: &str = "</IcaAt>";
const FACE_OPEN_TAG: &str = "[Face: ";
const FORWARD_OPEN_TAG: &str = "[Forward: ";
const NESTED_FORWARD_OPEN_TAG: &str = "[NestedForward: ";
const MESSAGE_AVATAR_SIZE: f32 = 32.0;
const MESSAGE_AVATAR_ROW_WIDTH: f32 = MESSAGE_AVATAR_SIZE + 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentMarker {
    Face,
    Mention,
    Forward,
    NestedForward,
}

fn sender_role_label(role: &str) -> Option<Cow<'_, str>> {
    let role = role.trim();
    match role.to_ascii_lowercase().as_str() {
        "" | "member" | "unknown" => None,
        "owner" => Some(Cow::Borrowed("群主")),
        "admin" | "administrator" => Some(Cow::Borrowed("管理员")),
        _ => Some(Cow::Borrowed(role)),
    }
}

fn should_show_group_identity(room_id: RoomId, mirai: &serde_json::Value) -> bool {
    room_id < 0
        && mirai
            .pointer("/eqq/type")
            .and_then(serde_json::Value::as_str)
            != Some("tg")
}

/// 与 ICA 一致地按消息距离当前日期的远近显示时间。
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

#[derive(Debug, PartialEq, Eq)]
enum ContentSegment<'a> {
    Text(&'a str),
    Link { text: &'a str, target: Cow<'a, str> },
    Face(u16),
    Mention { user_id: i64, text: Cow<'a, str> },
    Control,
}

fn normalize_link_target(link: &str) -> Cow<'_, str> {
    if link.contains("://") {
        Cow::Borrowed(link)
    } else {
        Cow::Owned(format!("https://{link}"))
    }
}

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
            push_text_segments(&mut segments, &remaining[..full_len]);
        }
        remaining = &remaining[full_len..];
    }

    segments
}

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

struct RichContentRender {
    response: egui::Response,
    text_response: Option<egui::Response>,
}

fn merge_text_response(combined: &mut Option<egui::Response>, response: egui::Response) {
    if let Some(combined) = combined {
        *combined |= response;
    } else {
        *combined = Some(response);
    }
}

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
pub fn render_rich_content(
    ui: &mut egui::Ui,
    content: &str,
    high_contrast: bool,
) -> egui::Response {
    render_rich_content_with_prefix(ui, None, content, high_contrast).response
}

fn reply_image_file(reply: &ReplyMessage) -> Option<&MessageFile> {
    reply
        .file
        .iter()
        .chain(&reply.files)
        .find(|file| is_image_file_type(&file.file_type))
}

/// 渲染消息中的图片缩略图并返回统一图片动作。
pub fn render_message_image(
    ui: &mut egui::Ui,
    source: &ImageSource,
    file_type: &str,
    max_width: f32,
    max_height: f32,
) -> Option<ImageAction> {
    if should_try_gif_preview(&source.url, file_type)
        && let Some(action) = render_gif_message_image(ui, source, max_width, max_height)
    {
        return action;
    }

    match ui.ctx().try_load_texture(
        &source.url,
        egui::TextureOptions::default(),
        egui::load::SizeHint::default(),
    ) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            let response = ui.add(
                Image::from_texture(texture)
                    .max_width(max_width)
                    .max_height(max_height)
                    .maintain_aspect_ratio(true)
                    .sense(egui::Sense::click()),
            );
            return image_response_action(response, source);
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            ui.add(egui::Spinner::new());
            ui.weak("图片加载中...");
        }
        Err(err) => {
            if should_probe_gif_after_static_error(&err)
                && let Some(action) = render_gif_message_image(ui, source, max_width, max_height)
            {
                return action;
            }
            show_image_load_error(ui, &err);
        }
    }
    None
}

fn should_try_gif_preview(url: &str, file_type: &str) -> bool {
    let file_type = file_type.to_ascii_lowercase();
    if file_type.contains("gif") {
        return true;
    }

    image_url_looks_like_gif(url)
}

fn render_gif_message_image(
    ui: &mut egui::Ui,
    source: &ImageSource,
    max_width: f32,
    max_height: f32,
) -> Option<Option<ImageAction>> {
    match try_load_gif_texture(
        ui.ctx(),
        &source.url,
        egui::TextureOptions::default(),
        egui::load::SizeHint::default(),
    )? {
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            ui.add(egui::Spinner::new());
            ui.weak("图片加载中...");
            Some(None)
        }
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            let response = ui.add(
                Image::from_texture(texture)
                    .max_width(max_width)
                    .max_height(max_height)
                    .maintain_aspect_ratio(true)
                    .sense(egui::Sense::click()),
            );
            Some(image_response_action(response, source))
        }
        Err(err) => {
            show_image_load_error(ui, &err);
            Some(None)
        }
    }
}

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

fn show_image_load_error(ui: &mut egui::Ui, err: &egui::load::LoadError) {
    let err_text = err.to_string();
    if err_text.contains("图片链接已过期") {
        ui.colored_label(egui::Color32::LIGHT_RED, "图片链接已过期");
        ui.weak("需要重新获取该图片 URL");
    } else {
        ui.colored_label(egui::Color32::LIGHT_RED, "图片加载失败");
        ui.weak(err_text);
    }
}

pub struct MessageRenderOptions {
    pub show_sender_name: bool,
    pub show_separator_before: bool,
    pub forward_mode_active: bool,
    pub forward_selected: bool,
}

impl IcaApp {
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

        if pure_text_mode && options.show_separator_before {
            ui.separator();
            ui.add_space(4.0);
        }

        ui.allocate_ui_with_layout(
            egui::vec2(row_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
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

                        if let Some(avatar_url) = &sender_avatar_url {
                            ui.add(
                                Image::from_uri(avatar_url.clone())
                                    .fit_to_exact_size(egui::vec2(
                                        MESSAGE_AVATAR_SIZE,
                                        MESSAGE_AVATAR_SIZE,
                                    ))
                                    .corner_radius(MESSAGE_AVATAR_SIZE / 2.0),
                            );
                            ui.add_space(8.0);
                        }

                        if is_self {
                            let leading_space = (content_row_width - bubble_width).max(0.0);
                            if leading_space > 0.0 {
                                ui.add_space(leading_space);
                            }
                        }

                        ui.allocate_ui_with_layout(
                            egui::vec2(bubble_width, 0.0),
                            egui::Layout::top_down(content_align),
                            |ui| {
                                let mut render_message_contents = |ui: &mut egui::Ui| {
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
                                                for file in &message.files {
                                                    let is_image =
                                                        is_image_file_type(&file.file_type);

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

                                // 可选择文字会优先接收点击；合并其响应后，正文上的右键也能打开消息菜单。
                                let response =
                                    if let Some(body_text_response) = body_text_response.take() {
                                        response.union(body_text_response)
                                    } else {
                                        response
                                    };

                                response.context_menu(|ui| {
                                    if message_is_hidden {
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
                                            fallback_res_id: reference.fallback_res_id.clone(),
                                            inline_messages: reference.inline_messages.clone(),
                                        });
                                        ui.close();
                                    }
                                    if !is_self && ui.button("戳一戳").clicked() {
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
                                        action = Some(MessageAction::ToggleForwardSelection {
                                            room_id,
                                            message_id: message.msg_id.clone(),
                                        });
                                        ui.close();
                                    }
                                    if is_self && !message.deleted && ui.button("撤回").clicked()
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
                    },
                );
            },
        );
        ui.add_space(if pure_text_mode { 2.0 } else { 4.0 });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sender_roles_use_group_friendly_labels() {
        assert_eq!(sender_role_label("owner").as_deref(), Some("群主"));
        assert_eq!(sender_role_label("admin").as_deref(), Some("管理员"));
        assert_eq!(
            sender_role_label("administrator").as_deref(),
            Some("管理员")
        );
        assert_eq!(sender_role_label("member"), None);
        assert_eq!(sender_role_label("unknown"), None);
        assert_eq!(sender_role_label(" bot ").as_deref(), Some("bot"));
    }

    #[test]
    fn group_identity_is_hidden_for_private_and_telegram_messages() {
        assert!(should_show_group_identity(-123, &serde_json::Value::Null));
        assert!(!should_show_group_identity(123, &serde_json::Value::Null));
        assert!(!should_show_group_identity(
            -123,
            &json!({ "eqq": { "type": "tg" } })
        ));
    }

    #[test]
    fn message_avatar_prefers_mirai_avatar_before_qq_fallback() {
        assert_eq!(
            message_avatar_url(123, &json!({ "eqq": { "avatarMd5": "aBcD" } })),
            Some("https://gchat.qpic.cn/gchatpic_new/0/0-0-ABCD/0".to_string())
        );
        assert_eq!(
            message_avatar_url(
                123,
                &json!({ "eqq": { "avatarUrl": "https://example.com/avatar.png" } })
            ),
            Some("https://example.com/avatar.png".to_string())
        );
        assert_eq!(
            message_avatar_url(123, &serde_json::Value::Null),
            Some("https://q1.qlogo.cn/g?b=qq&nk=123&s=140".to_string())
        );
        assert_eq!(message_avatar_url(-1, &serde_json::Value::Null), None);
    }

    #[test]
    fn parses_links_without_swallowing_surrounding_punctuation() {
        assert_eq!(
            parse_content_segments("见 https://example.com/docs?q=1，或者稍后再看。"),
            vec![
                ContentSegment::Text("见 "),
                ContentSegment::Link {
                    text: "https://example.com/docs?q=1",
                    target: Cow::Borrowed("https://example.com/docs?q=1"),
                },
                ContentSegment::Text("，或者稍后再看。"),
            ]
        );
    }

    #[test]
    fn normalizes_bare_domains_for_browser_opening() {
        assert_eq!(
            parse_content_segments("www.example.com/path"),
            vec![ContentSegment::Link {
                text: "www.example.com/path",
                target: Cow::Owned("https://www.example.com/path".to_owned()),
            }]
        );
    }

    #[test]
    fn link_parsing_preserves_message_markers() {
        assert_eq!(
            parse_content_segments(
                "https://example.com <IcalinguaAt qq=42>%40Alice</IcalinguaAt>[Face: 0]"
            ),
            vec![
                ContentSegment::Link {
                    text: "https://example.com",
                    target: Cow::Borrowed("https://example.com"),
                },
                ContentSegment::Text(" "),
                ContentSegment::Mention {
                    user_id: 42,
                    text: Cow::Owned("@Alice".to_owned()),
                },
                ContentSegment::Face(0),
            ]
        );
    }
}
