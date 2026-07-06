use crate::app::{
    IcaApp, MessageAction, MessageLayoutCacheKey, MessageRowLayout, PendingFile, PendingImage,
};
use egui::{Button, Image, Label, RichText};

use super::message_card::MessageRenderOptions;
use super::{
    estimate_composer_rows, estimate_message_row_height, format_message_content,
    format_pending_size, insert_text_at_saved_cursor, message_visible_range,
};

impl IcaApp {
    pub fn render_central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let Some(active_bridge_idx) = self.active_bridge_idx else {
                ui.heading("未启用 bridge");
                ui.weak("请先在配置里启用至少一个 bridge。");
                return;
            };

            let bridge_key = self.bridge_states[active_bridge_idx].bridge_key.clone();
            let socket_state = self.bridge_states[active_bridge_idx].socket_state;
            let auth_state = self.bridge_states[active_bridge_idx].auth_state;
            let last_error = self.bridge_states[active_bridge_idx].last_error.clone();
            let last_notice = self.bridge_states[active_bridge_idx].last_notice.clone();
            let is_shut_up = self.bridge_states[active_bridge_idx].is_shut_up;
            let selected_room_id = self.bridge_states[active_bridge_idx].selected_room_id;

            if let Some(room_id) = selected_room_id {
                let room_name = self.bridge_states[active_bridge_idx]
                    .rooms
                    .iter()
                    .find(|room| room.room_id == room_id)
                    .map(|room| room.room_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| room_id.to_string());

                ui.horizontal_wrapped(|ui| {
                    ui.heading(room_name);
                    ui.separator();
                    ui.label(format!("Bridge: {}", bridge_key));
                    ui.label(format!("Socket: {}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                    if is_shut_up {
                        ui.colored_label(egui::Color32::YELLOW, "禁言中");
                    }
                    if room_id < 0 && ui.button("群签到").clicked() {
                        self.send_group_sign(room_id);
                    }
                    if ui.button("重新拉取历史").clicked() {
                        self.request_room_messages(active_bridge_idx, room_id, false);
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(&bridge_key);
                    ui.separator();
                    ui.label(format!("Socket: {}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                });
            }

            if let Some(last_error) = last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, last_error);
            }
            if let Some(last_notice) = last_notice {
                ui.weak(last_notice);
            }

            ui.add_space(4.0);

            if selected_room_id.is_none() {
                let online_data = &self.bridge_states[active_bridge_idx].online_data;
                ui.label(format!("QQ: {}", online_data.qqid));
                ui.label(format!("昵称: {}", online_data.nick));
                ui.label(format!(
                    "在线: {}",
                    if online_data.online { "是" } else { "否" }
                ));
                ui.label(format!(
                    "Bridge 版本: {}",
                    online_data.icalingua_info.ica_version
                ));
                ui.label(format!("系统: {}", online_data.icalingua_info.os_info));
                ui.add_space(8.0);
                ui.weak("从左侧选择一个房间后，会自动请求该房间历史消息。");
                return;
            }

            let room_id = selected_room_id.expect("selected_room_id checked above");
            let composer_id = egui::Id::new(("message_composer", active_bridge_idx, room_id));
            let self_id = self.bridge_states[active_bridge_idx].online_data.qqid;
            let has_requested = self.bridge_states[active_bridge_idx]
                .requested_rooms
                .contains(&room_id);
            let should_scroll_to_bottom = self.bridge_states[active_bridge_idx]
                .message_scroll_to_bottom
                .contains(&room_id);
            let forward_mode_active =
                self.bridge_states[active_bridge_idx].is_forward_selection_active(room_id);
            let forward_selected_ids = self.bridge_states[active_bridge_idx]
                .forward_selected_message_ids
                .clone();
            let forward_selected_count = if forward_mode_active {
                forward_selected_ids.len()
            } else {
                0
            };
            let has_reply_banner = self.bridge_states[active_bridge_idx]
                .reply_to_by_room
                .contains_key(&room_id);
            let pending_images = self.bridge_states[active_bridge_idx]
                .pending_image_by_room
                .get(&room_id)
                .cloned()
                .unwrap_or_default();
            let pending_image_count = pending_images.len();
            let has_pending_image = pending_image_count > 0;
            let has_pending_file = self.bridge_states[active_bridge_idx]
                .pending_file_by_room
                .contains_key(&room_id);
            let composer_available_width = ui.available_width();
            let composer_button_width = if composer_available_width < 180.0 {
                24.0
            } else {
                30.0
            };
            let composer_item_spacing = if composer_available_width < 180.0 {
                4.0
            } else {
                ui.spacing().item_spacing.x
            };
            let composer_button_count = if composer_available_width >= 180.0 {
                3.0
            } else {
                2.0
            };
            let estimated_input_width = (composer_available_width
                - composer_button_width * composer_button_count
                - composer_item_spacing * composer_button_count)
                .max(0.0);
            let composer_rows = self.bridge_states[active_bridge_idx]
                .draft_by_room
                .get(&room_id)
                .map(|draft| estimate_composer_rows(draft, estimated_input_width))
                .unwrap_or(1);
            let line_height = ui.text_style_height(&egui::TextStyle::Body);
            let control_height = (line_height * composer_rows as f32 + 12.0).clamp(30.0, 132.0);
            let composer_reserved_height = control_height
                + 6.0
                + if forward_mode_active { 54.0 } else { 0.0 }
                + if has_reply_banner { 54.0 } else { 0.0 }
                + if has_pending_image { 144.0 } else { 0.0 }
                + if has_pending_file { 54.0 } else { 0.0 }
                + if self.show_face_picker { 220.0 } else { 0.0 };
            let message_list_height = (ui.available_height() - composer_reserved_height).max(120.0);
            let mut pending_action = None;
            let mut request_composer_focus = false;
            let pure_text_mode = self.custom_chat.hide_group_member_avatar;
            let message_layout_width =
                (ui.available_width() - if forward_mode_active { 24.0 } else { 0.0 }).max(48.0);
            let message_layout_key = MessageLayoutCacheKey {
                width: message_layout_width,
                pure_text_mode,
                forward_mode_active,
            };
            {
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                let layout_changed = bridge_state
                    .message_layout_cache_keys
                    .get(&room_id)
                    .is_none_or(|old_key| !old_key.matches(message_layout_key));
                if layout_changed {
                    bridge_state.message_row_heights.remove(&room_id);
                    bridge_state.message_row_layouts.remove(&room_id);
                    bridge_state.last_content_height.remove(&room_id);
                    bridge_state
                        .message_layout_cache_keys
                        .insert(room_id, message_layout_key);
                }
            }

            let scroll_to_target = self.bridge_states[active_bridge_idx]
                .scroll_to_message_id
                .clone();
            let mut scroll_target_found = scroll_to_target.is_none();
            let mut scroll_target_rendered = scroll_to_target.is_none();
            let saved_scroll_offset = self.bridge_states[active_bridge_idx]
                .message_scroll_offsets
                .get(&room_id)
                .copied()
                .unwrap_or(0.0);
            let mut measured_message_heights = Vec::<(String, f32)>::new();
            let scroll_output = egui::ScrollArea::vertical()
                .id_salt(("message_list", active_bridge_idx, room_id))
                .vertical_scroll_offset(saved_scroll_offset)
                .stick_to_bottom(true)
                .max_height(message_list_height)
                .show_viewport(ui, |ui, viewport| {
                    ui.set_min_width(ui.max_rect().width());

                    let needs_row_layout = {
                        let bridge_state = &self.bridge_states[active_bridge_idx];
                        let message_count = bridge_state
                            .messages_by_room
                            .get(&room_id)
                            .map_or(0, Vec::len);
                        bridge_state
                            .message_row_layouts
                            .get(&room_id)
                            .is_none_or(|rows| rows.len() != message_count)
                    };
                    if needs_row_layout {
                        let rows = {
                            let bridge_state = &self.bridge_states[active_bridge_idx];
                            let messages = bridge_state.messages_by_room.get(&room_id);
                            let cached_heights = bridge_state.message_row_heights.get(&room_id);
                            let row_width = ui.available_width().max(48.0);
                            let line_height = ui.text_style_height(&egui::TextStyle::Body);
                            let mut rows = Vec::with_capacity(messages.map_or(0, Vec::len));
                            let mut total_height = 0.0;
                            let mut previous_sender_id = None;

                            for message in messages.into_iter().flatten() {
                                let show_sender_name = !pure_text_mode
                                    || previous_sender_id != Some(message.sender_id);
                                let show_separator_before = pure_text_mode
                                    && previous_sender_id.is_some()
                                    && previous_sender_id != Some(message.sender_id);
                                let height = cached_heights
                                    .and_then(|heights| heights.get(&message.msg_id))
                                    .copied()
                                    .unwrap_or_else(|| {
                                        estimate_message_row_height(
                                            message,
                                            row_width,
                                            line_height,
                                            pure_text_mode,
                                            forward_mode_active,
                                            show_sender_name,
                                            show_separator_before,
                                        )
                                    });

                                rows.push(MessageRowLayout {
                                    top: total_height,
                                    height,
                                });
                                total_height += height;
                                previous_sender_id = Some(message.sender_id);
                            }
                            rows
                        };
                        self.bridge_states[active_bridge_idx]
                            .message_row_layouts
                            .insert(room_id, rows);
                    }

                    match self.bridge_states[active_bridge_idx]
                        .messages_by_room
                        .get(&room_id)
                    {
                        Some(messages) if !messages.is_empty() => {
                            let row_width = ui.available_width().max(48.0);
                            let rows = &self.bridge_states[active_bridge_idx].message_row_layouts
                                [&room_id];
                            let total_height = rows.last().map_or(0.0, |row| row.top + row.height);
                            let target_index = scroll_to_target.as_deref().and_then(|target_id| {
                                messages
                                    .iter()
                                    .position(|message| message.msg_id == target_id)
                            });
                            scroll_target_found |= target_index.is_some();

                            let list_top = ui.cursor().min.y;
                            if let Some(target_index) = target_index
                                && let Some(row) = rows.get(target_index)
                            {
                                let target_rect = egui::Rect::from_min_size(
                                    egui::pos2(ui.min_rect().left(), list_top + row.top),
                                    egui::vec2(row_width, row.height),
                                );
                                ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
                            }

                            let viewport_top = viewport.top().max(0.0);
                            let viewport_bottom = viewport.bottom().max(viewport_top);
                            let (start, end) =
                                message_visible_range(rows, viewport_top, viewport_bottom);

                            if start > 0 {
                                let top_spacer =
                                    rows.get(start).map_or(total_height, |row| row.top);
                                ui.add_space(top_spacer);
                            }

                            for idx in start..end {
                                let message = &messages[idx];
                                let row = rows[idx];
                                let previous_sender_id = idx
                                    .checked_sub(1)
                                    .map(|previous_idx| messages[previous_idx].sender_id);
                                let show_sender_name = !pure_text_mode
                                    || previous_sender_id != Some(message.sender_id);
                                let show_separator_before = pure_text_mode
                                    && previous_sender_id.is_some()
                                    && previous_sender_id != Some(message.sender_id);
                                let forward_selected = forward_mode_active
                                    && forward_selected_ids
                                        .iter()
                                        .any(|selected_id| selected_id == &message.msg_id);
                                let is_scroll_target =
                                    scroll_to_target.as_deref() == Some(&message.msg_id);
                                let before_y = ui.cursor().min.y;
                                ui.scope(|ui| {
                                    if let Some(action) = self.render_message_card(
                                        ui,
                                        room_id,
                                        self_id,
                                        message,
                                        MessageRenderOptions {
                                            show_sender_name,
                                            show_separator_before,
                                            forward_mode_active,
                                            forward_selected,
                                        },
                                    ) {
                                        pending_action = Some(action);
                                    }
                                });
                                let measured_height = (ui.cursor().min.y - before_y).max(24.0);
                                if (measured_height - row.height).abs() > 1.0 {
                                    measured_message_heights
                                        .push((message.msg_id.clone(), measured_height));
                                }
                                if is_scroll_target {
                                    scroll_target_rendered = true;
                                    let message_rect = egui::Rect::from_min_size(
                                        egui::pos2(ui.min_rect().left(), before_y),
                                        egui::vec2(row_width, measured_height),
                                    );
                                    ui.scroll_to_rect(message_rect, Some(egui::Align::Center));
                                }
                            }

                            let rendered_bottom = if end > 0 {
                                rows[end - 1].top + rows[end - 1].height
                            } else {
                                0.0
                            };
                            ui.add_space((total_height - rendered_bottom).max(0.0));
                        }
                        Some(_) => {
                            ui.weak("当前会话暂无消息");
                        }
                        None if has_requested => {
                            ui.weak("当前会话暂无消息");
                        }
                        None => {
                            ui.weak("正在向 bridge 请求历史消息...");
                        }
                    }

                    if should_scroll_to_bottom {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });

            let mut load_history_for_scroll_target = false;
            if scroll_to_target.is_some() && scroll_target_rendered {
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                bridge_state.scroll_to_message_id = None;
                bridge_state.scroll_to_message_attempts = 0;
            } else if scroll_to_target.is_some() && !scroll_target_found {
                const MAX_SCROLL_TARGET_ATTEMPTS: u8 = 10;
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                if bridge_state.no_more_history.contains(&room_id)
                    || bridge_state.scroll_to_message_attempts >= MAX_SCROLL_TARGET_ATTEMPTS
                {
                    bridge_state.scroll_to_message_id = None;
                    bridge_state.scroll_to_message_attempts = 0;
                    bridge_state.last_notice = Some("被引用的消息太远或已不存在".to_string());
                } else if !bridge_state.loading_older_messages.contains(&room_id) {
                    bridge_state.scroll_to_message_attempts += 1;
                    bridge_state.last_notice = Some(format!(
                        "正在加载引用消息所在的历史记录（{}/{}）",
                        bridge_state.scroll_to_message_attempts, MAX_SCROLL_TARGET_ATTEMPTS
                    ));
                    load_history_for_scroll_target = true;
                }
            }
            if load_history_for_scroll_target {
                self.request_older_messages(active_bridge_idx, room_id);
            }

            if !measured_message_heights.is_empty() {
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                let heights = bridge_state.message_row_heights.entry(room_id).or_default();
                for (msg_id, height) in measured_message_heights {
                    heights.insert(msg_id, height);
                }
                bridge_state.message_row_layouts.remove(&room_id);
                ui.ctx().request_repaint();
            }

            // prepend 旧消息后调整 scroll offset，避免无限触发加载
            let mut scroll_output = scroll_output;
            {
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                let new_content_height = scroll_output.content_size.y;
                if bridge_state.prepend_scroll_fix.remove(&room_id) {
                    // 计算新增内容高度，将 scroll offset 下移相应距离
                    let old_height = bridge_state
                        .last_content_height
                        .get(&room_id)
                        .copied()
                        .unwrap_or(0.0);
                    let delta = new_content_height - old_height;
                    if delta > 0.0 {
                        scroll_output.state.offset.y += delta;
                        scroll_output.state.store(ui.ctx(), scroll_output.id);
                    }
                }
                bridge_state
                    .last_content_height
                    .insert(room_id, new_content_height);
                bridge_state
                    .message_scroll_offsets
                    .insert(room_id, scroll_output.state.offset.y);
            }

            // 检测是否滚动到底部：内容高度 - 滚动偏移 - 可视高度 < 阈值
            let user_scrolled_to_bottom;
            let user_scrolled_to_top;
            {
                let content_size = scroll_output.content_size;
                let inner_rect = scroll_output.inner_rect;
                let offset_y = scroll_output.state.offset.y;
                let visible_height = inner_rect.height();
                let max_scroll = (content_size.y - visible_height).max(0.0);
                user_scrolled_to_bottom = max_scroll < 1.0 || (max_scroll - offset_y) < 20.0;
                // 滚动到顶部检测：offset_y 接近 0
                user_scrolled_to_top = offset_y < 20.0 && content_size.y > visible_height;
            }

            // 滚动到顶部时自动加载更旧的历史消息
            if user_scrolled_to_top {
                self.request_older_messages(active_bridge_idx, room_id);
            }

            // 正在加载更旧消息时显示提示
            if self.bridge_states[active_bridge_idx]
                .loading_older_messages
                .contains(&room_id)
            {
                let scroll_rect = scroll_output.inner_rect;
                let indicator_pos =
                    egui::pos2(scroll_rect.center().x - 50.0, scroll_rect.top() + 8.0);
                egui::Area::new(egui::Id::new((
                    "loading_older_indicator",
                    active_bridge_idx,
                    room_id,
                )))
                .fixed_pos(indicator_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("加载历史消息...");
                    });
                });
            }

            // 未滚动到底部时，在滚动区域右下角悬浮显示 "↓" 按钮
            if !user_scrolled_to_bottom {
                let scroll_rect = scroll_output.inner_rect;
                let btn_size = egui::vec2(32.0, 32.0);
                let btn_pos = egui::pos2(
                    scroll_rect.right() - btn_size.x - 12.0,
                    scroll_rect.bottom() - btn_size.y - 12.0,
                );
                egui::Area::new(egui::Id::new((
                    "scroll_to_bottom_btn",
                    active_bridge_idx,
                    room_id,
                )))
                .fixed_pos(btn_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    ui.set_min_size(btn_size);
                    ui.set_max_size(btn_size);
                    let btn_text = egui::RichText::new("↓").size(18.0);
                    let btn = egui::Button::new(btn_text)
                        .corner_radius(16.0)
                        .min_size(btn_size);
                    if ui.add_sized(btn_size, btn).clicked() {
                        self.bridge_states[active_bridge_idx]
                            .message_scroll_to_bottom
                            .insert(room_id);
                    }
                });
            }

            if should_scroll_to_bottom {
                self.bridge_states[active_bridge_idx]
                    .message_scroll_to_bottom
                    .remove(&room_id);
            }

            if let Some(action) = pending_action {
                match action {
                    MessageAction::Reply { room_id, reply } => {
                        self.queue_reply(room_id, reply);
                        request_composer_focus = true;
                    }
                    MessageAction::Delete {
                        room_id,
                        message_id,
                    } => {
                        self.send_delete_message(room_id, message_id);
                    }
                    MessageAction::ReEdit { room_id, content } => {
                        self.restore_deleted_message_to_draft(room_id, content);
                        request_composer_focus = true;
                    }
                    MessageAction::SetReveal {
                        room_id,
                        message_id,
                        reveal,
                    } => {
                        self.set_message_reveal(room_id, message_id, reveal);
                    }
                    MessageAction::CopyToDraft {
                        room_id,
                        message_id,
                    } => {
                        self.copy_message_to_draft(room_id, message_id);
                        request_composer_focus = true;
                    }
                    MessageAction::PlusOne {
                        room_id,
                        message_id,
                    } => {
                        self.plus_one_message(room_id, message_id);
                    }
                    MessageAction::ToggleForwardSelection {
                        room_id,
                        message_id,
                    } => {
                        self.toggle_forward_message_selection(room_id, message_id);
                    }
                    MessageAction::StartForward {
                        room_id,
                        message_id,
                    } => {
                        self.begin_forward_selection(room_id, message_id, true);
                    }
                    MessageAction::PreviewImage { url } => {
                        self.image_viewer = Some(std::sync::Arc::new(std::sync::Mutex::new(
                            crate::app::state::ImageViewerState::new(url),
                        )));
                    }
                    MessageAction::ScrollToMessage { msg_id } => {
                        let bridge_state = &mut self.bridge_states[active_bridge_idx];
                        bridge_state.scroll_to_message_id = Some(msg_id);
                        bridge_state.scroll_to_message_attempts = 0;
                    }
                    MessageAction::RenewMessage {
                        room_id,
                        message_id,
                    } => {
                        self.send_renew_message(room_id, message_id);
                    }
                    MessageAction::Poke { room_id, target_id } => {
                        self.send_group_poke(room_id, target_id);
                    }
                }
            }

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
            let mut open_pending_image = None::<(String, std::sync::Arc<[u8]>)>;
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
                                                    open_pending_image = Some((
                                                        preview_uri.clone(),
                                                        image.data.clone(),
                                                    ));
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
                            let btn_count = if show_face_btn { 3.0 } else { 2.0 };
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
                            if response.has_focus() && self.clipboard_paste_failed {
                                match Self::load_clipboard_image() {
                                    Ok(image) => paste_images.push(image),
                                    Err(e) => {
                                        tracing::debug!("剪贴板无可用图片: {}", e);
                                    }
                                }
                            }
                            let enter_pressed = enter_no_mod;
                            if show_face_btn
                                && ui
                                    .add_sized(
                                        [button_width, control_height],
                                        Button::new(RichText::new("😀").size(15.0)),
                                    )
                                    .clicked()
                            {
                                self.show_face_picker = !self.show_face_picker;
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

            if let Some((preview_uri, preview_bytes)) = open_pending_image {
                ui.ctx().include_bytes(preview_uri.clone(), preview_bytes);
                self.image_viewer = Some(std::sync::Arc::new(std::sync::Mutex::new(
                    crate::app::state::ImageViewerState::new(preview_uri),
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

            // 拖放文件上传
            let hovered = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
            let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());

            if hovered {
                let screen_rect = ui.ctx().input(|i| i.viewport_rect());
                egui::Area::new(egui::Id::new("drop_overlay"))
                    .fixed_pos(screen_rect.min)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        let painter = ui.painter();
                        painter.rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(100));
                        painter.text(
                            screen_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "拖放文件到此处",
                            egui::FontId::proportional(24.0),
                            egui::Color32::WHITE,
                        );
                    });
            }

            if !dropped.is_empty() {
                let mut dropped_images = Vec::new();
                let mut dropped_file = None;
                let mut dropped_errors = Vec::new();

                for file in dropped {
                    let file_name = if !file.name.is_empty() {
                        file.name.clone()
                    } else if let Some(p) = &file.path {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    } else {
                        "unknown".to_string()
                    };

                    let data = if let Some(bytes) = file.bytes {
                        bytes.to_vec()
                    } else if let Some(path) = &file.path {
                        std::fs::read(path).unwrap_or_default()
                    } else {
                        Vec::new()
                    };

                    if data.is_empty() {
                        continue;
                    }

                    let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
                    let image_exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
                    if image_exts.contains(&ext.as_str()) {
                        let mime = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                        dropped_images.push(PendingImage::new(file_name, mime, data));
                    } else if dropped_file.is_none() {
                        let ft = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                        dropped_file = Some(PendingFile::new(file_name, ft, data));
                    } else {
                        dropped_errors
                            .push(format!("暂不支持同时拖放多份非图片文件: {}", file_name));
                    }
                }

                if !dropped_images.is_empty() {
                    self.append_pending_images(active_bridge_idx, room_id, dropped_images);
                }
                if let Some(file) = dropped_file {
                    self.bridge_states[active_bridge_idx]
                        .pending_file_by_room
                        .insert(room_id, file);
                }
                if !dropped_errors.is_empty() {
                    self.bridge_states[active_bridge_idx].last_error =
                        Some(dropped_errors.join("；"));
                }
            }
        });
    }

    // 合并房间内的头像 / 名称行 / 预览行 为一个方法，减少外部碎片函数
}
