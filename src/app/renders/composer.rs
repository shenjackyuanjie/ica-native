use crate::app::{IcaApp, PendingImage};
use egui::{Button, Image, Label, RichText};

use super::{
    format_message_content, format_pending_size, insert_mention_at_saved_cursor,
    insert_text_at_saved_cursor, saved_cursor_preceded_by,
};

pub(super) struct ComposerParams {
    pub active_bridge_idx: usize,
    pub room_id: i64,
    pub composer_id: egui::Id,
    pub forward_mode_active: bool,
    pub forward_selected_count: usize,
    pub pending_images: Vec<PendingImage>,
    pub pending_image_count: usize,
    pub has_pending_image: bool,
    pub has_pending_file: bool,
    pub composer_reserved_height: f32,
    pub control_height: f32,
    pub composer_rows: usize,
    pub request_composer_focus: bool,
}

impl IcaApp {
    #[allow(clippy::too_many_lines)]
    pub(super) fn render_composer(&mut self, ui: &mut egui::Ui, params: ComposerParams) {
        let ComposerParams {
            active_bridge_idx,
            room_id,
            composer_id,
            forward_mode_active,
            forward_selected_count,
            pending_images,
            pending_image_count,
            has_pending_image,
            has_pending_file,
            composer_reserved_height,
            control_height,
            composer_rows,
            mut request_composer_focus,
        } = params;
        let mut clear_reply = false;
        let mut clear_image = false;
        let mut clear_file = false;
        let mut clear_forward_selection = false;
        let mut open_forward_picker = false;
        let mut plus_one_forward = false;

        let mut should_send = false;
        let mut choose_image = false;
        let mut choose_file = false;
        let mut paste_images = Vec::new();
        let mut remove_pending_image_idx = None;
        let mut selected_mention = None::<(i64, String)>;
        let mut request_group_members = false;
        let mut force_refresh_group_members = false;
        let mut open_pending_image = None::<(String, Vec<(String, std::sync::Arc<[u8]>)>)>;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), composer_reserved_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                if forward_mode_active {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("已选 {} 条消息", forward_selected_count));
                            if ui.button("逐条转发").clicked() {
                                open_forward_picker = true;
                            }
                            if ui.button("+1").clicked() {
                                plus_one_forward = true;
                            }
                            if ui.button("清空").clicked() {
                                clear_forward_selection = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                }

                if has_pending_image {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("待发送图片（{}）", pending_image_count));
                            if ui.button("清空").clicked() {
                                clear_image = true;
                            }
                            ui.weak("点击缩略图可预览大图");
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::horizontal()
                            .id_salt(("pending_image_preview", active_bridge_idx, room_id))
                            .max_height(104.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for (idx, image) in pending_images.iter().enumerate() {
                                        ui.vertical(|ui| {
                                            let preview_uri = format!(
                                                "bytes://pending_image/{}/{}/{}-{}-{}",
                                                active_bridge_idx,
                                                room_id,
                                                image.preview_id,
                                                image.data.len(),
                                                image.name
                                            );
                                            let response = ui.add(
                                                Image::from_bytes(
                                                    preview_uri.clone(),
                                                    image.data.clone(),
                                                )
                                                .fit_to_exact_size(egui::vec2(72.0, 72.0))
                                                .corner_radius(8.0)
                                                .sense(egui::Sense::click()),
                                            );
                                            if response.clicked() {
                                                let gallery = pending_images
                                                    .iter()
                                                    .map(|pending| {
                                                        (
                                                            format!(
                                                                "bytes://pending_image/{}/{}/{}-{}-{}",
                                                                active_bridge_idx,
                                                                room_id,
                                                                pending.preview_id,
                                                                pending.data.len(),
                                                                pending.name
                                                            ),
                                                            pending.data.clone(),
                                                        )
                                                    })
                                                    .collect();
                                                open_pending_image =
                                                    Some((preview_uri.clone(), gallery));
                                            }
                                            response.on_hover_text(format!(
                                                "{}\n{} · {}",
                                                image.name,
                                                format_pending_size(image.data.len()),
                                                image.mime_type
                                            ));
                                            ui.small(format!(
                                                "#{} · {}",
                                                idx + 1,
                                                format_pending_size(image.data.len())
                                            ));
                                            if ui.small_button("移除").clicked() {
                                                remove_pending_image_idx = Some(idx);
                                            }
                                        });
                                        ui.add_space(6.0);
                                    }
                                });
                            });
                    });
                    ui.add_space(6.0);
                }

                if let Some(file) = self.bridge_states[active_bridge_idx]
                    .pending_file_by_room
                    .get(&room_id)
                    .cloned()
                {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak("待发送文件");
                            if ui.button("取消").clicked() {
                                clear_file = true;
                            }
                        });
                        let size_str = format_pending_size(file.data.len());
                        ui.add(Label::new(format!("📎 {} ({})", file.name, size_str)).wrap());
                    });
                    ui.add_space(6.0);
                }

                if let Some(reply) = self.bridge_states[active_bridge_idx]
                    .reply_to_by_room
                    .get(&room_id)
                    .cloned()
                {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("正在回复 {}", reply.sender_name));
                            if ui.button("取消").clicked() {
                                clear_reply = true;
                            }
                        });
                        let content = format_message_content(&reply.content);
                        ui.add(Label::new(content.as_ref()).wrap());
                    });
                    ui.add_space(6.0);
                }

                if self.show_mention_picker && room_id < 0 {
                    let mention_panel_height = 220.0;
                    let has_members = self.bridge_states[active_bridge_idx]
                        .group_members_by_room
                        .contains_key(&room_id);
                    let loading = self.bridge_states[active_bridge_idx]
                        .loading_group_members
                        .contains(&room_id);
                    if !has_members && !loading {
                        request_group_members = true;
                    }
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), mention_panel_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.weak("选择要 @ 的群成员");
                                    if ui.small_button("刷新").clicked() {
                                        force_refresh_group_members = true;
                                    }
                                    if ui.small_button("关闭").clicked() {
                                        self.show_mention_picker = false;
                                        self.mention_replace_trigger = false;
                                    }
                                });
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.mention_search_query,
                                    )
                                    .hint_text("搜索群名片、昵称或 QQ"),
                                );

                                if loading && !has_members {
                                    ui.horizontal(|ui| {
                                        ui.add(egui::Spinner::new().size(14.0));
                                        ui.weak("正在加载群成员...");
                                    });
                                } else if let Some(members) = self.bridge_states
                                    [active_bridge_idx]
                                    .group_members_by_room
                                    .get(&room_id)
                                {
                                    let query = self.mention_search_query.trim().to_lowercase();
                                    let filtered = members
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, member)| {
                                            query.is_empty()
                                                || member.user_id.to_string().contains(&query)
                                                || member
                                                    .display_name()
                                                    .to_lowercase()
                                                    .contains(&query)
                                        })
                                        .map(|(index, _)| index)
                                        .collect::<Vec<_>>();
                                    let total_rows = filtered.len() + 1;
                                    egui::ScrollArea::vertical()
                                        .id_salt((
                                            "mention_picker",
                                            active_bridge_idx,
                                            room_id,
                                        ))
                                        .max_height(mention_panel_height - 62.0)
                                        .show_rows(ui, 38.0, total_rows, |ui, rows| {
                                            for row in rows {
                                                if row == 0 {
                                                    if ui
                                                        .selectable_label(
                                                            false,
                                                            "@全体成员  ·  all",
                                                        )
                                                        .clicked()
                                                    {
                                                        selected_mention = Some((
                                                            1,
                                                            "全体成员".to_string(),
                                                        ));
                                                    }
                                                    continue;
                                                }
                                                let member = &members[filtered[row - 1]];
                                                ui.horizontal(|ui| {
                                                    ui.add(
                                                        Image::new(format!(
                                                            "https://q1.qlogo.cn/g?b=qq&nk={}&s=40",
                                                            member.user_id
                                                        ))
                                                        .fit_to_exact_size(egui::vec2(
                                                            28.0, 28.0,
                                                        ))
                                                        .corner_radius(14.0),
                                                    );
                                                    let label = format!(
                                                        "{}  ·  {}",
                                                        member.display_name(),
                                                        member.user_id
                                                    );
                                                    if ui
                                                        .selectable_label(false, label)
                                                        .clicked()
                                                    {
                                                        selected_mention = Some((
                                                            member.user_id,
                                                            member
                                                                .display_name()
                                                                .to_string(),
                                                        ));
                                                    }
                                                });
                                            }
                                        });
                                } else {
                                    ui.weak("暂无群成员数据");
                                }
                            });
                        },
                    );
                    ui.add_space(6.0);
                }

                // 表情选择器面板
                if self.show_face_picker {
                    let face_panel_height = 200.0;
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), face_panel_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.weak("选择表情");
                                    if ui.small_button("关闭").clicked() {
                                        self.show_face_picker = false;
                                    }
                                });
                                let face_size = 32.0;
                                let button_size = 40.0;
                                let spacing = 4.0;
                                let content_width =
                                    (ui.available_width() - 16.0).max(button_size);
                                let cols = ((content_width + spacing) / (button_size + spacing))
                                    .max(1.0)
                                    as usize;
                                let total_rows = crate::face_data::FACE_COUNT.div_ceil(cols);
                                egui::ScrollArea::vertical()
                                    .id_salt(("face_picker", active_bridge_idx, room_id))
                                    .max_height(face_panel_height - 30.0)
                                    .show_rows(ui, button_size, total_rows, |ui, row_range| {
                                        ui.spacing_mut().item_spacing.x = spacing;

                                        for row in row_range {
                                            ui.horizontal(|ui| {
                                                let start = row * cols;
                                                let end = (start + cols)
                                                    .min(crate::face_data::FACE_COUNT);
                                                for index in start..end {
                                                    let Some(face_id) =
                                                        crate::face_data::face_id_at(index)
                                                    else {
                                                        continue;
                                                    };
                                                    let bytes =
                                                        crate::face_data::get_face(face_id)
                                                            .unwrap();
                                                    let uri = format!("bytes://face_{face_id}");
                                                    let img = Image::from_bytes(uri, bytes)
                                                        .fit_to_exact_size(egui::vec2(
                                                            face_size, face_size,
                                                        ));
                                                    let btn = ui.add_sized(
                                                        [button_size, button_size],
                                                        Button::image(img),
                                                    );
                                                    let clicked = btn.clicked();
                                                    let name = crate::face_data::get_face_name(
                                                        face_id,
                                                    );
                                                    if let Some(name) = name {
                                                        btn.on_hover_text(name);
                                                    }
                                                    if clicked {
                                                        let draft = self.bridge_states
                                                            [active_bridge_idx]
                                                            .draft_by_room
                                                            .entry(room_id)
                                                            .or_default();
                                                        let face_markup =
                                                            format!("[Face: {}]", face_id);
                                                        insert_text_at_saved_cursor(
                                                            ui.ctx(),
                                                            composer_id,
                                                            draft,
                                                            &face_markup,
                                                        );
                                                        self.show_face_picker = false;
                                                        request_composer_focus = true;
                                                    }
                                                }
                                            });
                                        }
                                    });
                            });
                        },
                    );
                    ui.add_space(6.0);
                }

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), control_height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let available_width = ui.available_width();
                        let button_width = if available_width < 180.0 { 24.0 } else { 30.0 };
                        let item_spacing = if available_width < 180.0 {
                            4.0
                        } else {
                            ui.spacing().item_spacing.x
                        };
                        ui.spacing_mut().item_spacing.x = item_spacing;
                        // 窄宽度不显示表情按钮
                        let show_face_btn = available_width >= 180.0;
                        let mut btn_count = if show_face_btn { 3.0 } else { 2.0 };
                        if room_id < 0 {
                            btn_count += 1.0;
                        }
                        let input_width = (ui.available_width()
                            - button_width * btn_count
                            - item_spacing * btn_count)
                            .max(0.0);
                        let draft = self.bridge_states[active_bridge_idx]
                            .draft_by_room
                            .entry(room_id)
                            .or_default();
                        let response = ui.add_sized(
                            [input_width, control_height],
                            egui::TextEdit::multiline(draft)
                                .id(composer_id)
                                .desired_rows(composer_rows)
                                .hint_text("Enter 发送, Shift+Enter 换行"),
                        );
                        if request_composer_focus {
                            response.request_focus();
                        }
                        let enter_no_mod = response.has_focus()
                            && !self.ime_composing
                            && !self.ime_event_this_frame
                            && ui.input(|input| {
                                input.key_pressed(egui::Key::Enter)
                                    && !input.modifiers.shift
                                    && !input.modifiers.ctrl
                            });
                        if enter_no_mod {
                            while draft.ends_with('\n') || draft.ends_with('\r') {
                                draft.pop();
                            }
                        }
                        if response.changed()
                            && room_id < 0
                            && !self.ime_composing
                            && saved_cursor_preceded_by(ui.ctx(), composer_id, draft, '@')
                        {
                            self.show_mention_picker = true;
                            self.mention_replace_trigger = true;
                            self.mention_search_query.clear();
                            self.show_face_picker = false;
                            request_group_members = true;
                        }
                        if response.has_focus() && self.clipboard_paste_failed {
                            match Self::load_clipboard_image() {
                                Ok(image) => paste_images.push(image),
                                Err(e) => {
                                    tracing::debug!("剪贴板无可用图片: {}", e);
                                }
                            }
                        }
                        let enter_pressed = enter_no_mod;
                        if room_id < 0
                            && ui
                                .add_sized(
                                    [button_width, control_height],
                                    Button::new(RichText::new("@").size(16.0)),
                                )
                                .on_hover_text("@ 群成员")
                                .clicked()
                        {
                            self.show_mention_picker = !self.show_mention_picker;
                            self.mention_replace_trigger = false;
                            self.mention_search_query.clear();
                            self.show_face_picker = false;
                            if self.show_mention_picker {
                                request_group_members = true;
                            }
                        }
                        if show_face_btn
                            && ui
                                .add_sized(
                                    [button_width, control_height],
                                    Button::new(RichText::new("😀").size(15.0)),
                                )
                                .clicked()
                        {
                            self.show_face_picker = !self.show_face_picker;
                            self.show_mention_picker = false;
                        }
                        let plus_btn = ui.add_sized(
                            [button_width, control_height],
                            Button::new(RichText::new("＋").size(16.0)),
                        );
                        plus_btn.context_menu(|ui| {
                            if ui.button("📷 发送图片").clicked() {
                                choose_image = true;
                                ui.close();
                            }
                            if ui.button("📎 发送文件").clicked() {
                                choose_file = true;
                                ui.close();
                            }
                        });
                        if plus_btn.clicked() {
                            choose_image = true;
                        }
                        let can_send =
                            !draft.trim().is_empty() || has_pending_image || has_pending_file;
                        should_send = enter_pressed
                            || ui
                                .add_enabled_ui(can_send, |ui| {
                                    ui.add_sized(
                                        [button_width, control_height],
                                        Button::new(RichText::new("↗").size(15.0)),
                                    )
                                })
                                .inner
                                .clicked();
                    },
                );
            },
        );

        if force_refresh_group_members {
            self.request_group_members(active_bridge_idx, room_id, true);
        } else if request_group_members {
            self.request_group_members(active_bridge_idx, room_id, false);
        }

        if let Some((user_id, name)) = selected_mention {
            let visible_text = format!("@{name}");
            let mention_text = format!("{visible_text} ");
            let draft = self.bridge_states[active_bridge_idx]
                .draft_by_room
                .entry(room_id)
                .or_default();
            insert_mention_at_saved_cursor(
                ui.ctx(),
                composer_id,
                draft,
                &mention_text,
                self.mention_replace_trigger,
            );
            let mentions = self.bridge_states[active_bridge_idx]
                .mentions_by_room
                .entry(room_id)
                .or_default();
            if !mentions
                .iter()
                .any(|mention| mention.user_id == user_id && mention.text == visible_text)
            {
                mentions.push(crate::ica::types::message::Mention {
                    user_id,
                    text: visible_text,
                });
            }
            self.show_mention_picker = false;
            self.mention_search_query.clear();
            self.mention_replace_trigger = false;
            ui.memory_mut(|memory| memory.request_focus(composer_id));
        }

        if clear_reply {
            self.bridge_states[active_bridge_idx]
                .reply_to_by_room
                .remove(&room_id);
        }

        if clear_image {
            self.bridge_states[active_bridge_idx]
                .pending_image_by_room
                .remove(&room_id);
        }

        if let Some(index) = remove_pending_image_idx {
            self.remove_pending_image_at(active_bridge_idx, room_id, index);
        }

        if clear_file {
            self.bridge_states[active_bridge_idx]
                .pending_file_by_room
                .remove(&room_id);
        }

        if !paste_images.is_empty() {
            self.append_pending_images(active_bridge_idx, room_id, paste_images);
        }

        if let Some((preview_uri, gallery)) = open_pending_image {
            let image_urls = gallery
                .iter()
                .map(|(url, _)| url.clone())
                .collect::<Vec<_>>();
            for (url, bytes) in gallery {
                ui.ctx().include_bytes(url, bytes);
            }
            self.image_viewer = Some(std::sync::Arc::new(std::sync::Mutex::new(
                crate::app::state::ImageViewerState::with_images(preview_uri, image_urls),
            )));
        }

        if clear_forward_selection {
            self.clear_forward_selection();
        }

        if open_forward_picker {
            self.open_forward_target_picker(room_id);
        }

        if plus_one_forward {
            self.plus_one_forward_selection(room_id);
        }

        if choose_image {
            self.pick_image_for_current_room();
        }

        if choose_file {
            self.pick_file_for_current_room();
        }

        if should_send {
            self.show_face_picker = false;
            self.send_current_message();
        }

        self.handle_composer_drop(ui, active_bridge_idx, room_id);
    }
}
