use crate::app::{IcaApp, MessageAction, MessageLayoutCacheKey, MessageRowLayout};

use super::message_card::MessageRenderOptions;
use super::{estimate_composer_rows, estimate_message_row_height, message_visible_range};

const DATE_SEPARATOR_ESTIMATED_HEIGHT: f32 = 42.0;

fn should_show_date_separator(previous_date: Option<&str>, current_date: &str) -> bool {
    previous_date.is_some_and(|previous_date| previous_date != current_date)
}

fn render_date_separator(ui: &mut egui::Ui, date: &str) {
    ui.add_space(6.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 0.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            let background = if ui.visuals().dark_mode {
                egui::Color32::from_rgba_premultiplied(0, 0, 0, 77)
            } else {
                egui::Color32::from_rgb(0xe5, 0xef, 0xfa)
            };
            let text_color = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(0xbe, 0xc5, 0xcc)
            } else {
                egui::Color32::from_rgb(0x50, 0x5a, 0x62)
            };
            egui::Frame::NONE
                .fill(background)
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(date)
                            .size(12.0)
                            .strong()
                            .color(text_color),
                    );
                });
        },
    );
    ui.add_space(4.0);
}

fn fit_chat_body_heights(
    available_height: f32,
    desired_composer_height: f32,
    section_spacing: f32,
) -> (f32, f32) {
    let available_height = available_height.max(0.0);
    let section_spacing = section_spacing.clamp(0.0, available_height);
    let content_height = available_height - section_spacing;
    let composer_height = desired_composer_height.clamp(0.0, content_height);
    (content_height - composer_height, composer_height)
}

fn show_constrained_composer<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    height: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .min_scrolled_height(height)
        .max_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, add_contents)
}

impl IcaApp {
    pub fn render_central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(active_bridge_idx) = self.active_bridge_idx else {
                ui.heading("未启用 bridge");
                ui.weak("请先在配置里启用至少一个 bridge。");
                return;
            };

            let bridge_key = self.bridge_states[active_bridge_idx].bridge_key.clone();
            let socket_state = self.bridge_states[active_bridge_idx].socket_state;
            let auth_state = self.bridge_states[active_bridge_idx].auth_state;
            self.bridge_states[active_bridge_idx].sync_status_history();
            let last_error = self.bridge_states[active_bridge_idx].last_error.clone();
            let last_notice = self.bridge_states[active_bridge_idx].last_notice.clone();
            let status_history = self.bridge_states[active_bridge_idx].status_history.clone();
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
                    ui.label(format!("当前 bridge：{}", bridge_key));
                    ui.label(format!("连接状态：{}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                    if is_shut_up {
                        ui.colored_label(egui::Color32::YELLOW, "禁言中");
                    }
                    if room_id < 0 {
                        let icon = egui::Image::new(crate::assets::svg::CHAT_GROUP)
                            .fit_to_exact_size(egui::vec2(18.0, 18.0))
                            .alt_text("群成员");
                        if ui
                            .add_sized([30.0, 30.0], egui::Button::image(icon))
                            .on_hover_text("群成员")
                            .clicked()
                        {
                            self.group_member_panel.open = !self.group_member_panel.open;
                            self.group_member_panel.confirmation = None;
                            if self.group_member_panel.open {
                                self.request_group_members(active_bridge_idx, room_id, true);
                            }
                        }
                    }
                    if room_id < 0 && ui.button("群签到").clicked() {
                        self.send_group_sign(room_id);
                    }
                    if ui.button("重新拉取历史").clicked() {
                        self.request_room_messages(active_bridge_idx, room_id, false);
                    }
                    if ui.button("搜索聊天记录").clicked() {
                        let room_name = self.bridge_states[active_bridge_idx]
                            .rooms
                            .iter()
                            .find(|room| room.room_id == room_id)
                            .map(|room| room.room_name.clone())
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| room_id.to_string());
                        self.open_message_search(active_bridge_idx, room_id, room_name);
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(&bridge_key);
                    ui.separator();
                    ui.label(format!("连接状态：{}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                });
            }

            let mut clear_error = false;
            let mut clear_notice = false;
            let mut clear_status_history = false;
            if let Some(last_error) = last_error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, last_error);
                    clear_error = ui.small_button("清除").clicked();
                });
            }
            if let Some(last_notice) = last_notice {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(last_notice);
                    clear_notice = ui.small_button("清除").clicked();
                });
            }
            egui::ComboBox::from_id_salt(("bridge_status_history", active_bridge_idx))
                .selected_text(format!("消息历史 ({})", status_history.len()))
                .width(ui.available_width().min(560.0))
                .show_ui(ui, |ui| {
                    ui.set_min_width((ui.ctx().content_rect().width() - 24.0).clamp(120.0, 360.0));
                    if status_history.is_empty() {
                        ui.weak("暂无历史消息");
                        return;
                    }
                    if ui.button("清空历史").clicked() {
                        clear_status_history = true;
                        ui.close();
                        return;
                    }
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("bridge_status_history_scroll", active_bridge_idx))
                        .max_height(280.0)
                        .show(ui, |ui| {
                            for entry in status_history.iter().rev() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(&entry.timestamp);
                                    let kind = egui::RichText::new(entry.kind.label()).strong();
                                    if matches!(
                                        entry.kind,
                                        crate::app::state::StatusMessageKind::Error
                                    ) {
                                        ui.colored_label(egui::Color32::LIGHT_RED, kind);
                                    } else {
                                        ui.weak(kind);
                                    }
                                    ui.label(&entry.text);
                                });
                            }
                        });
                });
            if clear_error {
                self.bridge_states[active_bridge_idx].clear_error();
            }
            if clear_notice {
                self.bridge_states[active_bridge_idx].clear_notice();
            }
            if clear_status_history {
                self.bridge_states[active_bridge_idx].clear_status_history();
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

            let room_id = selected_room_id.expect("上方已确认 selected_room_id 存在");
            let composer_id = egui::Id::new(("message_composer", active_bridge_idx, room_id));
            let self_id = self.bridge_states[active_bridge_idx].online_data.qqid;
            let mut request_composer_focus = false;
            if self.clipboard_paste_failed {
                match Self::load_clipboard_image() {
                    Ok(image) => {
                        self.append_pending_images(active_bridge_idx, room_id, [image]);
                        request_composer_focus = true;
                    }
                    Err(e) => {
                        tracing::debug!("剪贴板无可用图片: {}", e);
                    }
                }
            }
            let conversation = self.bridge_states[active_bridge_idx].conversation(room_id);
            let has_requested =
                conversation.is_some_and(|conversation| conversation.requested_snapshot);
            let should_scroll_to_bottom =
                conversation.is_some_and(|conversation| conversation.scroll_to_bottom);
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
            let has_reply_banner =
                conversation.is_some_and(|conversation| conversation.reply_to.is_some());
            let pending_images = conversation
                .map(|conversation| conversation.pending_images.clone())
                .unwrap_or_default();
            let pending_image_count = pending_images.len();
            let has_pending_image = pending_image_count > 0;
            let has_pending_file =
                conversation.is_some_and(|conversation| conversation.pending_file.is_some());
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
            let mut composer_button_count = if composer_available_width >= 180.0 {
                3.0
            } else {
                2.0
            };
            if room_id < 0 {
                composer_button_count += 1.0;
            }
            let estimated_input_width = (composer_available_width
                - composer_button_width * composer_button_count
                - composer_item_spacing * composer_button_count)
                .max(0.0);
            let composer_rows = conversation
                .map(|conversation| {
                    estimate_composer_rows(&conversation.draft, estimated_input_width)
                })
                .unwrap_or(1);
            let line_height = ui.text_style_height(&egui::TextStyle::Body);
            let control_height = (line_height * composer_rows as f32 + 12.0).clamp(30.0, 132.0);
            let desired_composer_height = control_height
                + 6.0
                + if forward_mode_active { 54.0 } else { 0.0 }
                + if has_reply_banner { 54.0 } else { 0.0 }
                + if has_pending_image { 144.0 } else { 0.0 }
                + if has_pending_file { 54.0 } else { 0.0 }
                + if self.show_face_picker { 220.0 } else { 0.0 };
            let (message_list_height, composer_reserved_height) = fit_chat_body_heights(
                ui.available_height(),
                desired_composer_height,
                ui.spacing().item_spacing.y,
            );
            let composer_is_constrained = composer_reserved_height < desired_composer_height;
            let mut pending_action = None;
            let pure_text_mode = self.custom_chat.hide_group_member_avatar;
            let message_row_width = ui.available_width().max(48.0);
            let message_layout_width =
                (message_row_width - if forward_mode_active { 24.0 } else { 0.0 }).max(48.0);
            let message_layout_key = MessageLayoutCacheKey {
                width: message_layout_width,
                pure_text_mode,
                forward_mode_active,
            };
            {
                let conversation = self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                let layout_changed = conversation
                    .message_layout_cache_key
                    .is_none_or(|old_key| !old_key.matches(message_layout_key));
                if layout_changed {
                    conversation.message_row_heights.clear();
                    conversation.message_row_layouts.clear();
                    conversation.last_content_height = None;
                    conversation.message_layout_cache_key = Some(message_layout_key);
                }
            }

            let scroll_to_target = self.bridge_states[active_bridge_idx]
                .conversation(room_id)
                .and_then(|conversation| conversation.scroll_to_message_id.clone());
            let mut scroll_target_found = scroll_to_target.is_none();
            let mut scroll_target_rendered = scroll_to_target.is_none();
            let saved_scroll_offset = self.bridge_states[active_bridge_idx]
                .conversation(room_id)
                .and_then(|conversation| conversation.message_scroll_offset)
                .unwrap_or(0.0);
            let mut measured_message_heights = Vec::<(String, f32)>::new();

            let needs_row_layout = {
                let bridge_state = &self.bridge_states[active_bridge_idx];
                let conversation = bridge_state.conversation(room_id);
                let message_count =
                    conversation.map_or(0, |conversation| conversation.messages.len());
                conversation.is_none_or(|conversation| {
                    conversation.message_row_layouts.len() != message_count
                })
            };
            if needs_row_layout {
                let rows = {
                    let bridge_state = &self.bridge_states[active_bridge_idx];
                    let conversation = bridge_state.conversation(room_id);
                    let messages = conversation.map(|conversation| &conversation.messages);
                    let cached_heights =
                        conversation.map(|conversation| &conversation.message_row_heights);
                    let row_width = message_row_width;
                    let mut rows = Vec::with_capacity(messages.map_or(0, Vec::len));

                    // 首屏从最新消息向旧消息估算，先得到底部附近需要的布局；
                    // 随后再补 top 坐标，屏幕上的阅读顺序仍保持旧 -> 新。
                    if let Some(messages) = messages {
                        for idx in (0..messages.len()).rev() {
                            let message = &messages[idx];
                            let previous_message =
                                idx.checked_sub(1).and_then(|idx| messages.get(idx));
                            let show_sender_name = !pure_text_mode
                                || previous_message
                                    .is_none_or(|previous| previous.sender_id != message.sender_id);
                            let show_separator_before = pure_text_mode
                                && previous_message.is_some_and(|previous| {
                                    previous.sender_id != message.sender_id
                                });
                            let show_date_separator = should_show_date_separator(
                                previous_message.map(|message| message.date_text.as_str()),
                                &message.date_text,
                            );
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
                                    ) + if show_date_separator {
                                        DATE_SEPARATOR_ESTIMATED_HEIGHT
                                    } else {
                                        0.0
                                    }
                                });

                            rows.push(MessageRowLayout { top: 0.0, height });
                        }
                        rows.reverse();
                    }

                    let mut total_height = 0.0;
                    for row in &mut rows {
                        row.top = total_height;
                        total_height += row.height;
                    }
                    rows
                };
                self.bridge_states[active_bridge_idx]
                    .conversation_mut(room_id)
                    .message_row_layouts = rows;
            }

            let initial_scroll_offset = if should_scroll_to_bottom {
                let content_height = self.bridge_states[active_bridge_idx]
                    .conversation(room_id)
                    .and_then(|conversation| conversation.message_row_layouts.last())
                    .map_or(0.0, |row| row.top + row.height);
                (content_height - message_list_height).max(0.0)
            } else {
                saved_scroll_offset
            };
            let scroll_output = egui::ScrollArea::vertical()
                .id_salt(("message_list", active_bridge_idx, room_id))
                .vertical_scroll_offset(initial_scroll_offset)
                .animated(!should_scroll_to_bottom)
                .stick_to_bottom(true)
                .min_scrolled_height(message_list_height)
                .max_height(message_list_height)
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    ui.set_min_width(ui.max_rect().width());

                    match self.bridge_states[active_bridge_idx]
                        .conversation(room_id)
                        .map(|conversation| &conversation.messages)
                    {
                        Some(messages) if !messages.is_empty() => {
                            let row_width = ui.available_width().max(48.0);
                            let rows = &self.bridge_states[active_bridge_idx]
                                .conversation(room_id)
                                .expect("存在消息时会话状态也应当存在")
                                .message_row_layouts;
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
                                let show_date_separator = should_show_date_separator(
                                    idx.checked_sub(1).map(|previous_idx| {
                                        messages[previous_idx].date_text.as_str()
                                    }),
                                    &message.date_text,
                                );
                                let forward_selected = forward_mode_active
                                    && forward_selected_ids
                                        .iter()
                                        .any(|selected_id| selected_id == &message.msg_id);
                                let is_scroll_target =
                                    scroll_to_target.as_deref() == Some(&message.msg_id);
                                let before_y = ui.cursor().min.y;
                                ui.scope(|ui| {
                                    if show_date_separator {
                                        render_date_separator(ui, &message.date_text);
                                    }
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
                });

            let mut load_history_for_scroll_target = false;
            if scroll_to_target.is_some() && scroll_target_rendered {
                let conversation = self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                conversation.scroll_to_message_id = None;
                conversation.scroll_to_message_attempts = 0;
            } else if scroll_to_target.is_some() && !scroll_target_found {
                const MAX_SCROLL_TARGET_ATTEMPTS: u8 = 10;
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                let conversation = bridge_state.conversation_mut(room_id);
                let notice = if conversation.no_more_history
                    || conversation.scroll_to_message_attempts >= MAX_SCROLL_TARGET_ATTEMPTS
                {
                    conversation.scroll_to_message_id = None;
                    conversation.scroll_to_message_attempts = 0;
                    Some("被引用的消息太远或已不存在".to_string())
                } else if !conversation.loading_older_messages {
                    conversation.scroll_to_message_attempts += 1;
                    load_history_for_scroll_target = true;
                    Some(format!(
                        "正在加载引用消息所在的历史记录（{}/{}）",
                        conversation.scroll_to_message_attempts, MAX_SCROLL_TARGET_ATTEMPTS
                    ))
                } else {
                    None
                };
                if let Some(notice) = notice {
                    bridge_state.last_notice = Some(notice);
                }
            }
            if load_history_for_scroll_target {
                self.request_older_messages(active_bridge_idx, room_id);
            }

            let message_heights_changed = !measured_message_heights.is_empty();
            if message_heights_changed {
                let conversation = self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                let heights = &mut conversation.message_row_heights;
                for (msg_id, height) in measured_message_heights {
                    heights.insert(msg_id, height);
                }
                conversation.message_row_layouts.clear();
                ui.ctx().request_repaint();
            }

            // 前插旧消息后调整滚动偏移量，避免无限触发加载
            let mut scroll_output = scroll_output;
            {
                let conversation = self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                let new_content_height = scroll_output.content_size.y;
                if std::mem::take(&mut conversation.prepend_scroll_fix) {
                    // 计算新增内容高度，将 scroll offset 下移相应距离
                    let old_height = conversation.last_content_height.unwrap_or(0.0);
                    let delta = new_content_height - old_height;
                    if delta > 0.0 {
                        scroll_output.state.offset.y += delta;
                        scroll_output.state.store(ui.ctx(), scroll_output.id);
                    }
                }
                conversation.last_content_height = Some(new_content_height);
                conversation.message_scroll_offset = Some(scroll_output.state.offset.y);
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
                user_scrolled_to_bottom = max_scroll < 1.0 || (max_scroll - offset_y) < 60.0;
                // 滚动到顶部检测：offset_y 接近 0
                user_scrolled_to_top = offset_y < 20.0 && content_size.y > visible_height;
            }

            {
                let conversation = self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                if user_scrolled_to_bottom {
                    conversation.near_bottom = true;
                    conversation.new_message_count = 0;
                } else {
                    conversation.near_bottom = false;
                }
            }

            // 切换会话并滚到底部的首帧里，egui 返回的 offset 仍可能是滚动前的 0。
            // 此时不能把它当成用户真的滚到了顶部，否则会立刻误触发旧消息加载。
            if user_scrolled_to_top && !should_scroll_to_bottom {
                self.request_older_messages(active_bridge_idx, room_id);
            }

            // 正在加载更旧消息时显示提示
            if self.bridge_states[active_bridge_idx]
                .conversation(room_id)
                .is_some_and(|conversation| conversation.loading_older_messages)
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
                let new_message_count = self.bridge_states[active_bridge_idx]
                    .conversation(room_id)
                    .map(|conversation| conversation.new_message_count)
                    .unwrap_or(0);
                let count_text = if new_message_count > 99 {
                    "99+".to_string()
                } else {
                    new_message_count.to_string()
                };
                let btn_size = if new_message_count > 0 {
                    egui::vec2(52.0, 32.0)
                } else {
                    egui::vec2(32.0, 32.0)
                };
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
                    let btn_text = egui::RichText::new(if new_message_count > 0 {
                        format!("↓ {count_text}")
                    } else {
                        "↓".to_string()
                    })
                    .size(if new_message_count > 0 { 13.0 } else { 18.0 });
                    let btn = egui::Button::new(btn_text)
                        .corner_radius(16.0)
                        .min_size(btn_size);
                    if ui.add_sized(btn_size, btn).clicked() {
                        let conversation =
                            self.bridge_states[active_bridge_idx].conversation_mut(room_id);
                        conversation.scroll_to_bottom = true;
                        conversation.new_message_count = 0;
                    }
                });
            }

            if should_scroll_to_bottom && !message_heights_changed {
                self.bridge_states[active_bridge_idx]
                    .conversation_mut(room_id)
                    .scroll_to_bottom = false;
            } else if should_scroll_to_bottom {
                // 首次显示时，估算高度会被真实渲染高度逐步替换。如果本帧就撤销底部
                // 锚点，内容高度变化会让视口看起来自动向上跳。保留到下一帧继续对齐，
                // 直到所有可见消息的高度稳定为止。
                ui.ctx().request_repaint();
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
                    MessageAction::OpenForward {
                        res_id,
                        file_name,
                        fallback_res_id,
                        inline_messages,
                    } => {
                        self.open_forward_reference(
                            res_id,
                            file_name,
                            fallback_res_id,
                            inline_messages,
                        );
                    }
                    MessageAction::Image(action) => {
                        self.handle_image_action(ui.ctx(), active_bridge_idx, action);
                    }
                    MessageAction::ScrollToMessage { msg_id } => {
                        let bridge_state = &mut self.bridge_states[active_bridge_idx];
                        let conversation = bridge_state.conversation_mut(room_id);
                        conversation.scroll_to_message_id = Some(msg_id);
                        conversation.scroll_to_message_attempts = 0;
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

            let mut composer_params = super::composer::ComposerParams {
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
                request_composer_focus,
            };
            if composer_is_constrained {
                composer_params.composer_reserved_height = desired_composer_height;
                show_constrained_composer(
                    ui,
                    egui::Id::new(("message_composer_scroll", active_bridge_idx, room_id)),
                    composer_reserved_height,
                    |ui| self.render_composer(ui, composer_params),
                );
            } else {
                self.render_composer(ui, composer_params);
            }
        });
    }

    // 合并房间内的头像 / 名称行 / 预览行 为一个方法，减少外部碎片函数
}

#[cfg(test)]
mod tests {
    use super::{fit_chat_body_heights, should_show_date_separator, show_constrained_composer};

    #[test]
    fn date_separator_only_appears_when_the_date_changes() {
        assert!(!should_show_date_separator(None, "2025/07/09"));
        assert!(!should_show_date_separator(
            Some("2025/07/09"),
            "2025/07/09"
        ));
        assert!(should_show_date_separator(Some("2025/07/09"), "2025/07/10"));
    }

    #[test]
    fn growing_composer_shrinks_message_list_without_exceeding_available_height() {
        let (message_height, composer_height) = fit_chat_body_heights(420.0, 186.0, 3.0);

        assert_eq!(message_height, 231.0);
        assert_eq!(composer_height, 186.0);
        assert_eq!(message_height + composer_height + 3.0, 420.0);
    }

    #[test]
    fn composer_is_clamped_to_very_short_chat_body() {
        let (message_height, composer_height) = fit_chat_body_heights(120.0, 186.0, 3.0);

        assert_eq!(message_height, 0.0);
        assert_eq!(composer_height, 117.0);
    }

    #[test]
    fn constrained_composer_keeps_trailing_control_inside_its_viewport() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 120.0),
            )),
            ..Default::default()
        };
        let mut viewport = egui::Rect::NOTHING;
        let mut control = egui::Rect::NOTHING;

        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                let output = show_constrained_composer(
                    ui,
                    egui::Id::new("constrained_composer_test"),
                    120.0,
                    |ui| {
                        ui.allocate_space(egui::vec2(200.0, 180.0));
                        ui.add_sized([200.0, 30.0], egui::Button::new("发送")).rect
                    },
                );
                viewport = output.inner_rect;
                control = output.inner;
            });
        }

        assert_eq!(viewport.height(), 120.0);
        assert!(control.top() >= viewport.top());
        assert!(control.bottom() <= viewport.bottom());
    }
}
