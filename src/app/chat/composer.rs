use crate::app::media::MediaTask;
use crate::app::stickers::{StickerEntry, StickerPickerTab};
use crate::app::{IcaApp, PendingImage};
use egui::{Button, Image, Label, RichText};

use super::{
    format_message_content, format_pending_size, insert_mention_at_saved_cursor,
    insert_text_at_saved_cursor, saved_cursor_preceded_by,
};

const MENTION_RESULTS_MAX_HEIGHT: f32 = 260.0;

#[derive(Default)]
struct MentionPickerAction {
    selected: Option<(i64, String)>,
    refresh: bool,
    close: bool,
    focus_composer: bool,
}

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
    fn render_mention_picker_overlay(
        &mut self,
        ctx: &egui::Context,
        active_bridge_idx: usize,
        room_id: i64,
        anchor_rect: egui::Rect,
        opened_this_frame: bool,
    ) -> MentionPickerAction {
        let mut action = MentionPickerAction::default();
        let content_rect = ctx.content_rect();
        let max_popup_width = (content_rect.width() - 16.0).max(120.0);
        let popup_width = (anchor_rect.width() + 140.0)
            .clamp(300.0, 460.0)
            .min(max_popup_width);
        let min_popup_x = content_rect.left() + 8.0;
        let max_popup_x = (content_rect.right() - popup_width - 8.0).max(min_popup_x);
        let popup_x = anchor_rect.left().clamp(min_popup_x, max_popup_x);
        let popup_anchor = egui::pos2(popup_x, anchor_rect.top() - 6.0);
        let results_height =
            MENTION_RESULTS_MAX_HEIGHT.min((content_rect.height() * 0.45).max(120.0));

        let area_response = egui::Area::new(egui::Id::new((
            "mention_picker_overlay",
            active_bridge_idx,
            room_id,
        )))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(popup_anchor)
        .constrain_to(content_rect)
        .show(ctx, |ui| {
            ui.set_width(popup_width);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(popup_width - ui.spacing().item_spacing.x);
                ui.horizontal(|ui| {
                    ui.strong("@ 群成员");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("关闭")
                            .on_hover_text("关闭 (Esc)")
                            .clicked()
                        {
                            action.close = true;
                            action.focus_composer = true;
                        }
                        if ui
                            .small_button("刷新")
                            .on_hover_text("刷新群成员")
                            .clicked()
                        {
                            action.refresh = true;
                        }
                    });
                });

                let close_with_keyboard = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                let move_down = ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                });
                let move_up = ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                });
                let confirm_with_keyboard = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                if close_with_keyboard {
                    action.close = true;
                    action.focus_composer = true;
                }

                let mention_search_id =
                    egui::Id::new(("mention_search", active_bridge_idx, room_id));
                let search_response = ui.add_sized(
                    [ui.available_width(), 30.0],
                    egui::TextEdit::singleline(&mut self.mention_search_query)
                        .id(mention_search_id)
                        .hint_text("搜索群名片、昵称或 QQ"),
                );
                if self.mention_search_focus_requested {
                    search_response.request_focus();
                    self.mention_search_focus_requested = false;
                }
                let search_changed = search_response.changed();
                if search_changed {
                    self.mention_selected_index = 0;
                }

                let conversation = self.bridge_states[active_bridge_idx].conversation(room_id);
                let has_members =
                    conversation.is_some_and(|conversation| conversation.group_members_loaded);
                let loading =
                    conversation.is_some_and(|conversation| conversation.loading_group_members);
                let query = self.mention_search_query.trim().to_lowercase();
                let mut entries = vec![(1, "全体成员".to_string())];
                if let Some(members) = conversation.map(|conversation| &conversation.group_members)
                {
                    entries.extend(
                        members
                            .iter()
                            .filter(|member| member.matches_search(&query))
                            .map(|member| {
                                (member.user_id, safe_mention_text(member.display_name()))
                            }),
                    );
                }
                if !query.is_empty() {
                    entries.retain(|(user_id, name)| {
                        *user_id != 1
                            || name.to_lowercase().contains(&query)
                            || "all".contains(&query)
                    });
                }

                if entries.is_empty() {
                    self.mention_selected_index = 0;
                } else {
                    self.mention_selected_index = self
                        .mention_selected_index
                        .min(entries.len().saturating_sub(1));
                    if move_down {
                        self.mention_selected_index =
                            (self.mention_selected_index + 1) % entries.len();
                    }
                    if move_up {
                        self.mention_selected_index = if self.mention_selected_index == 0 {
                            entries.len() - 1
                        } else {
                            self.mention_selected_index - 1
                        };
                    }
                    if confirm_with_keyboard {
                        action.selected = entries.get(self.mention_selected_index).cloned();
                    }
                }

                if loading && !has_members {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("正在加载群成员...");
                    });
                } else if entries.is_empty() {
                    ui.add_space(8.0);
                    ui.weak("没有匹配的群成员");
                    ui.add_space(8.0);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("mention_picker", active_bridge_idx, room_id))
                        .max_height(results_height)
                        .show_rows(ui, 38.0, entries.len(), |ui, rows| {
                            for row in rows {
                                let (user_id, name) = &entries[row];
                                let selected = row == self.mention_selected_index;
                                let response = if *user_id == 1 {
                                    ui.add_sized(
                                        [ui.available_width(), 34.0],
                                        egui::Button::selectable(
                                            selected,
                                            egui::RichText::new("@全体成员  ·  all").size(14.0),
                                        ),
                                    )
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            Image::new(format!(
                                                "https://q1.qlogo.cn/g?b=qq&nk={user_id}&s=40"
                                            ))
                                            .fit_to_exact_size(egui::vec2(28.0, 28.0))
                                            .corner_radius(14.0),
                                        );
                                        ui.add_sized(
                                            [ui.available_width(), 32.0],
                                            egui::Button::selectable(
                                                selected,
                                                egui::RichText::new(format!(
                                                    "{name}  ·  {user_id}"
                                                ))
                                                .size(14.0),
                                            ),
                                        )
                                    })
                                    .inner
                                };
                                if response.hovered() {
                                    self.mention_selected_index = row;
                                }
                                if selected && (search_changed || move_down || move_up) {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                if response.clicked() {
                                    action.selected = Some((*user_id, name.clone()));
                                }
                            }
                        });
                }

                if has_members {
                    ui.separator();
                    ui.small("方向键选择 | Enter 确认 | Esc 关闭");
                }
            });
        });

        if !opened_this_frame
            && ctx.input(|input| {
                input.pointer.primary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|position| !area_response.response.rect.contains(position))
            })
        {
            action.close = true;
        }

        action
    }

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
        let mut open_merged_forward_picker = false;
        let mut open_individual_forward_picker = false;
        let mut plus_one_forward = false;

        let mut should_send = false;
        let mut choose_image = false;
        let mut choose_file = false;
        let paste_images = Vec::new();
        let mut remove_pending_image_idx = None;
        let mut selected_mention = None::<(i64, String)>;
        let mut mention_anchor_rect = None;
        let mut mention_opened_this_frame = false;
        let mut request_group_members = false;
        let mut force_refresh_group_members = false;
        let mut refresh_stickers = false;
        let mut selected_sticker = None::<StickerEntry>;
        let mut open_pending_image = None::<(String, Vec<(String, std::sync::Arc<[u8]>)>)>;
        let mut composer_draft = self.state.bridge_states[active_bridge_idx]
            .state()
            .conversation(room_id)
            .map(|conversation| conversation.draft.clone())
            .unwrap_or_default();
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), composer_reserved_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                if forward_mode_active {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format!("已选 {} 条消息", forward_selected_count));
                            if ui.button("合并转发").clicked() {
                                open_merged_forward_picker = true;
                            }
                            if ui.button("逐条转发").clicked() {
                                open_individual_forward_picker = true;
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
                    .conversation(room_id)
                    .and_then(|conversation| conversation.pending_file.clone())
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
                    .conversation(room_id)
                    .and_then(|conversation| conversation.reply_to.clone())
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

                // 表情选择器面板
                if self.show_face_picker {
                    let face_panel_height = 200.0;
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), face_panel_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.selectable_value(
                                        &mut self.sticker_picker_tab,
                                        StickerPickerTab::QqFaces,
                                        "QQ 表情",
                                    );
                                    ui.selectable_value(
                                        &mut self.sticker_picker_tab,
                                        StickerPickerTab::Favorites,
                                        "收藏表情",
                                    );
                                    if self.sticker_picker_tab == StickerPickerTab::Favorites
                                        && ui.small_button("刷新").clicked()
                                    {
                                        refresh_stickers = true;
                                    }
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
                                match self.sticker_picker_tab {
                                    StickerPickerTab::QqFaces => {
                                        let total_rows =
                                            crate::face_data::FACE_COUNT.div_ceil(cols);
                                        egui::ScrollArea::vertical()
                                            .id_salt(("qq_face_picker", active_bridge_idx, room_id))
                                            .max_height(face_panel_height - 36.0)
                                            .show_rows(
                                                ui,
                                                button_size,
                                                total_rows,
                                                |ui, row_range| {
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
                                                                let Some(bytes) =
                                                                    crate::face_data::get_face(face_id)
                                                                else {
                                                                    continue;
                                                                };
                                                                let uri =
                                                                    format!("bytes://face_{face_id}");
                                                                let img = Image::from_bytes(uri, bytes)
                                                                    .fit_to_exact_size(egui::vec2(
                                                                        face_size, face_size,
                                                                    ));
                                                                let btn = ui.add_sized(
                                                                    [button_size, button_size],
                                                                    Button::image(img),
                                                                );
                                                                let clicked = btn.clicked();
                                                                if let Some(name) =
                                                                    crate::face_data::get_face_name(face_id)
                                                                {
                                                                    btn.on_hover_text(name);
                                                                }
                                                                if clicked {
                                                                    let face_markup = format!(
                                                                        "[Face: {}]",
                                                                        face_id
                                                                    );
                                                                    insert_text_at_saved_cursor(
                                                                        ui.ctx(),
                                                                        composer_id,
                                                                        &mut composer_draft,
                                                                        &face_markup,
                                                                    );
                                                                    self.show_face_picker = false;
                                                                    request_composer_focus = true;
                                                                }
                                                            }
                                                        });
                                                    }
                                                },
                                            );
                                    }
                                    StickerPickerTab::Favorites => {
                                        let entries = self.sticker_store.entries();
                                        if entries.is_empty() {
                                            ui.weak("暂无收藏表情，可从消息图片右键添加");
                                        } else {
                                            let total_rows = entries.len().div_ceil(cols);
                                            egui::ScrollArea::vertical()
                                                .id_salt((
                                                    "favorite_sticker_picker",
                                                    active_bridge_idx,
                                                    room_id,
                                                ))
                                                .max_height(face_panel_height - 36.0)
                                                .show_rows(
                                                    ui,
                                                    button_size,
                                                    total_rows,
                                                    |ui, row_range| {
                                                        ui.spacing_mut().item_spacing.x = spacing;
                                                        for row in row_range {
                                                            ui.horizontal(|ui| {
                                                                let start = row * cols;
                                                                let end =
                                                                    (start + cols).min(entries.len());
                                                                for entry in &entries[start..end] {
                                                                    let path = entry
                                                                        .path
                                                                        .to_string_lossy()
                                                                        .replace('\\', "/");
                                                                    let image = Image::new(format!(
                                                                        "file:///{}",
                                                                        path.trim_start_matches('/')
                                                                    ))
                                                                    .fit_to_exact_size(egui::vec2(
                                                                        face_size, face_size,
                                                                    ));
                                                                    let button = ui.add_sized(
                                                                        [button_size, button_size],
                                                                        Button::image(image),
                                                                    );
                                                                    let clicked = button.clicked();
                                                                    button.on_hover_text(&entry.name);
                                                                    if clicked {
                                                                        selected_sticker =
                                                                            Some(entry.clone());
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    },
                                                );
                                        }
                                    }
                                }
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
                        let mention_shortcut = room_id < 0
                            && ui.memory(|memory| memory.has_focus(composer_id))
                            && ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::CTRL, egui::Key::M)
                            });
                        let response = ui.add_sized(
                            [input_width, control_height],
                            egui::TextEdit::multiline(&mut composer_draft)
                                .id(composer_id)
                                .desired_rows(composer_rows)
                                .hint_text("Enter 发送, Shift+Enter 换行"),
                        );
                        mention_anchor_rect = Some(response.rect);
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
                            while composer_draft.ends_with('\n')
                                || composer_draft.ends_with('\r')
                            {
                                composer_draft.pop();
                            }
                        }
                        if response.changed() && room_id < 0 && !self.ime_composing {
                            if saved_cursor_preceded_by(
                                ui.ctx(),
                                composer_id,
                                &composer_draft,
                                '@',
                            ) {
                                let was_open = self.show_mention_picker;
                                self.show_mention_picker = true;
                                self.mention_replace_trigger = true;
                                self.mention_search_query.clear();
                                self.mention_search_focus_requested = true;
                                self.mention_selected_index = 0;
                                mention_opened_this_frame = !was_open;
                                self.show_face_picker = false;
                                request_group_members = true;
                            } else if self.show_mention_picker && self.mention_replace_trigger {
                                self.show_mention_picker = false;
                                self.mention_search_query.clear();
                                self.mention_search_focus_requested = false;
                                self.mention_replace_trigger = false;
                                self.mention_selected_index = 0;
                            }
                        }
                        if mention_shortcut {
                            self.show_mention_picker = true;
                            self.mention_replace_trigger = false;
                            self.mention_search_query.clear();
                            self.mention_search_focus_requested = true;
                            self.mention_selected_index = 0;
                            self.show_face_picker = false;
                            mention_opened_this_frame = true;
                            request_group_members = true;
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
                            self.mention_search_focus_requested = self.show_mention_picker;
                            self.mention_selected_index = 0;
                            self.show_face_picker = false;
                            if self.show_mention_picker {
                                mention_opened_this_frame = true;
                                request_group_members = true;
                            } else {
                                ui.memory_mut(|memory| memory.request_focus(composer_id));
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
                            self.mention_search_query.clear();
                            self.mention_search_focus_requested = false;
                            self.mention_replace_trigger = false;
                            self.mention_selected_index = 0;
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
                        let can_send = !composer_draft.trim().is_empty()
                            || has_pending_image
                            || has_pending_file;
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

        if self.show_mention_picker
            && room_id < 0
            && let Some(anchor_rect) = mention_anchor_rect
        {
            let picker_action = self.render_mention_picker_overlay(
                ui.ctx(),
                active_bridge_idx,
                room_id,
                anchor_rect,
                mention_opened_this_frame,
            );
            if let Some(mention) = picker_action.selected {
                selected_mention = Some(mention);
            }
            force_refresh_group_members |= picker_action.refresh;
            if picker_action.close {
                self.show_mention_picker = false;
                self.mention_search_query.clear();
                self.mention_search_focus_requested = false;
                self.mention_replace_trigger = false;
                self.mention_selected_index = 0;
            }
            if picker_action.focus_composer {
                ui.memory_mut(|memory| memory.request_focus(composer_id));
            }
        }

        self.state.bridge_states[active_bridge_idx]
            .state_mut()
            .conversation_mut(room_id)
            .draft = composer_draft;

        if force_refresh_group_members {
            self.request_group_members(active_bridge_idx, room_id, true);
        } else if request_group_members {
            self.request_group_members(active_bridge_idx, room_id, false);
        }

        if refresh_stickers {
            self.spawn_media_task(
                ui.ctx(),
                MediaTask::RefreshStickers {
                    store: self.sticker_store.clone(),
                    sort_newest_first: self.custom_chat.sort_stickers_by_time,
                },
            );
        }

        if let Some(entry) = selected_sticker {
            let bridge_key = self.bridge_states[active_bridge_idx].bridge_key.clone();
            self.spawn_media_task(
                ui.ctx(),
                MediaTask::LoadSticker {
                    store: self.sticker_store.clone(),
                    entry,
                    bridge_key,
                    room_id,
                },
            );
        }

        if let Some((user_id, name)) = selected_mention {
            let visible_text = format!("@{name}");
            let mention_text = format!("{visible_text} ");
            let replace_trigger = self.mention_replace_trigger;
            let draft = self.state.bridge_states[active_bridge_idx]
                .state_mut()
                .conversation_mut(room_id);
            insert_mention_at_saved_cursor(
                ui.ctx(),
                composer_id,
                &mut draft.draft,
                &mention_text,
                replace_trigger,
            );
            let mentions = &mut self.bridge_states[active_bridge_idx]
                .conversation_mut(room_id)
                .mentions;
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
            self.mention_search_focus_requested = false;
            self.mention_replace_trigger = false;
            self.mention_selected_index = 0;
            ui.memory_mut(|memory| memory.request_focus(composer_id));
        }

        if clear_reply {
            self.bridge_states[active_bridge_idx]
                .conversation_mut(room_id)
                .reply_to = None;
        }

        if clear_image {
            self.bridge_states[active_bridge_idx]
                .conversation_mut(room_id)
                .pending_images
                .clear();
        }

        if let Some(index) = remove_pending_image_idx {
            self.remove_pending_image_at(active_bridge_idx, room_id, index);
        }

        if clear_file {
            self.bridge_states[active_bridge_idx]
                .conversation_mut(room_id)
                .pending_file = None;
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

        if open_merged_forward_picker {
            self.open_forward_target_picker_with_mode(room_id, true);
        }

        if open_individual_forward_picker {
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
            self.show_mention_picker = false;
            self.mention_search_query.clear();
            self.mention_search_focus_requested = false;
            self.mention_replace_trigger = false;
            self.mention_selected_index = 0;
            self.send_current_message();
        }

        self.handle_composer_drop(ui, active_bridge_idx, room_id);
    }
}

fn safe_mention_text(text: &str) -> String {
    const MAX_CHARS: usize = 48;

    let mut output = String::new();
    let mut last_was_space = false;
    let mut truncated = false;

    for ch in text.chars() {
        if output.chars().count() >= MAX_CHARS {
            truncated = true;
            break;
        }

        let normalized = match ch {
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            ch if ch.is_control() => Some(' '),
            ch => Some(ch),
        };

        let Some(ch) = normalized else {
            continue;
        };

        if ch.is_whitespace() {
            if last_was_space {
                continue;
            }
            output.push(' ');
            last_was_space = true;
        } else {
            output.push(ch);
            last_was_space = false;
        }
    }

    let output = output.trim();
    if output.is_empty() {
        return "未命名成员".to_string();
    }

    if truncated {
        format!("{output}…")
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_mention_text;

    #[test]
    fn mention_text_removes_layout_controls() {
        assert_eq!(
            safe_mention_text("Alice\n\u{202E}Bob\tCarol"),
            "Alice Bob Carol"
        );
    }

    #[test]
    fn mention_text_limits_long_names() {
        let sanitized = safe_mention_text("a".repeat(80).as_str());

        assert_eq!(sanitized.chars().count(), 49);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn mention_text_uses_fallback_for_blank_names() {
        assert_eq!(safe_mention_text("\n\t\u{202E}"), "未命名成员");
    }
}
