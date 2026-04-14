use crate::app::{IcaApp, MessageAction};
use crate::ica::types::files::MessageFile;
use crate::ica::types::RoomId;
use egui::{Hyperlink, Image, Label};

use super::format_message_content;

fn is_image_file(file: &MessageFile) -> bool {
    let file_type = file.file_type.to_ascii_lowercase();
    file_type == "image" || file_type.starts_with("image/")
}

fn render_message_image(ui: &mut egui::Ui, url: &str, max_width: f32) {
    match ui.ctx().try_load_texture(
        url,
        egui::TextureOptions::default(),
        egui::load::SizeHint::default(),
    ) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            ui.add(
                Image::from_texture(texture)
                    .max_width(max_width)
                    .max_height(240.0)
                    .maintain_aspect_ratio(true),
            );
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            ui.add(egui::Spinner::new());
            ui.weak("图片加载中...");
        }
        Err(err) => {
            let err_text = err.to_string();
            if err_text.contains("图片链接已过期") {
                ui.colored_label(egui::Color32::LIGHT_RED, "图片链接已过期");
                ui.weak("需要重新获取该图片 URL");
            } else {
                ui.colored_label(egui::Color32::LIGHT_RED, "图片加载失败");
                ui.weak(err_text);
            }
        }
    }
}

pub(super) struct MessageRenderOptions {
    pub show_sender_name: bool,
    pub show_separator_before: bool,
    pub forward_mode_active: bool,
    pub forward_selected: bool,
}

impl IcaApp {
    pub(super) fn render_message_card(
        &self,
        ui: &mut egui::Ui,
        room_id: RoomId,
        self_id: i64,
        message: &crate::ica::types::message::Message,
        options: MessageRenderOptions,
    ) -> Option<MessageAction> {
        let is_self = self_id > 0 && message.sender_id == self_id;
        let formatted_content = format_message_content(&message.content);
        let message_is_hidden = (message.deleted || message.hide) && !message.reveal;
        let pure_text_mode = self.custom_chat.hide_group_member_avatar;

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
                                ui.colored_label(sys_text, &formatted_content);
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
        let row_width = ui.available_width();
        let selection_width = if options.forward_mode_active { 24.0 } else { 0.0 };
        let content_row_width = (row_width - selection_width).max(48.0);
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
                ui.horizontal(|ui| {
                    if options.forward_mode_active {
                        let mut checked = options.forward_selected;
                        if ui.checkbox(&mut checked, "").changed() {
                            action = Some(MessageAction::ToggleForwardSelection {
                                room_id,
                                message_id: message.msg_id.clone(),
                            });
                        }
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
                                ui.style_mut().interaction.selectable_labels = false;
                                ui.with_layout(egui::Layout::top_down(content_align), |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        if options.show_sender_name {
                                            ui.colored_label(title_color, &message.sender_name);
                                        }
                                        ui.weak(message.time.format("%H:%M:%S").to_string());
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
                                        if !message.deleted && !message.hide && ui.small_button("回复").clicked() {
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
                                        let formatted_reply_content =
                                            format_message_content(&reply.content);
                                        if pure_text_mode {
                                            ui.add(
                                                Label::new(format!(
                                                    "回复 {}: {}",
                                                    reply.sender_name, formatted_reply_content
                                                ))
                                                .wrap(),
                                            );
                                        } else {
                                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                                ui.weak(format!("回复 {}", reply.sender_name));
                                                ui.add(Label::new(formatted_reply_content).wrap());
                                            });
                                        }
                                    }

                                    let mut has_body = false;

                                    if !formatted_content.is_empty() {
                                        has_body = true;
                                        ui.add(Label::new(formatted_content.as_str()).wrap());
                                    }

                                    if !message.files.is_empty() {
                                        has_body = true;
                                        ui.with_layout(egui::Layout::top_down(content_align), |ui| {
                                            for file in &message.files {
                                                let is_image = is_image_file(file);

                                                if is_image && !file.url.is_empty() {
                                                    let image_max_width = ui.available_width().min(240.0);
                                                    render_message_image(ui, &file.url, image_max_width);
                                                } else {
                                                    let label = file
                                                        .get_name()
                                                        .cloned()
                                                        .unwrap_or_else(|| file.file_type.clone());
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
                                        });
                                    }

                                    if !has_body {
                                        ui.weak("[空消息]");
                                    }
                                });
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

                                if !message.deleted && !message.hide {
                                    if ui.button("回复").clicked() {
                                        action = Some(MessageAction::Reply {
                                            room_id,
                                            reply: message.as_reply(),
                                        });
                                        ui.close();
                                    }
                                }
                                if ui.button("复制到编辑区").clicked() {
                                    action = Some(MessageAction::CopyToDraft {
                                        room_id,
                                        message_id: message.msg_id.clone(),
                                    });
                                    ui.close();
                                }
                                if !message.content.trim().is_empty() && ui.button("复制文本").clicked() {
                                    ui.ctx().copy_text(message.content.clone());
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
                                if ui
                                    .button(if options.forward_selected { "移出多选" } else { "多选" })
                                    .clicked()
                                {
                                    action = Some(MessageAction::ToggleForwardSelection {
                                        room_id,
                                        message_id: message.msg_id.clone(),
                                    });
                                    ui.close();
                                }
                                if is_self && !message.deleted && ui.button("撤回").clicked() {
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
                });
            },
        );
        ui.add_space(if pure_text_mode { 2.0 } else { 4.0 });
        action
    }
}
