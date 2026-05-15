use super::*;
use egui::{Button, Hyperlink, Image, Label, RichText};

mod message_card;
mod windows;

use message_card::MessageRenderOptions;

fn format_message_content(content: &str) -> String {
    let open_tag = "<IcalinguaAt qq=";
    let close_tag = "</IcalinguaAt>";
    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some(start_idx) = remaining.find(open_tag) {
        let (before, after_start) = remaining.split_at(start_idx);
        result.push_str(before);

        let Some(tag_end_idx) = after_start.find('>') else {
            result.push_str(after_start);
            return result;
        };
        let tag_body = &after_start[tag_end_idx + 1..];
        let Some(close_idx) = tag_body.find(close_tag) else {
            result.push_str(after_start);
            return result;
        };

        let encoded_name = &tag_body[..close_idx];
        match urlencoding::decode(encoded_name) {
            Ok(decoded) => result.push_str(decoded.as_ref()),
            Err(_) => result.push_str(encoded_name),
        }

        remaining = &tag_body[close_idx + close_tag.len()..];
    }

    result.push_str(remaining);
    result
}

impl IcaApp {
    // 顶栏：将多个 menu 合并为一个"功能块"
    pub fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("顶栏").show_inside(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.render_top_menus(ui);
            })
        });
    }

    // 合并后的顶部菜单：包含 Icalingua 信息、通知设置、选项、帮助
    pub fn render_top_menus(&mut self, ui: &mut egui::Ui) {
        // Icalingua 菜单
        ui.menu_button("Icalingua++ native", |ui| {
            ui.label(crate::VERSION);
            let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
            ui.add(link);
            let verify_message_count = self
                .active_bridge_state()
                .map(|state| state.join_requests.len())
                .unwrap_or(0);
            if ui
                .button(format!("验证消息 ({})", verify_message_count))
                .clicked()
            {
                if let Some(active_bridge_idx) = self.active_bridge_idx {
                    self.request_system_messages(active_bridge_idx);
                }
                ui.close();
                self.open_page.verify_message = true;
            }
        });

        // 通知设置
        ui.menu_button("通知设置", |ui| {
            ui.label("通知启用级别 1-5");
            let _ = ui.add(egui::Slider::new(&mut self.notify_level, 1..=5));
            if ui.button("通知等级说明").clicked() {
                ui.close();
                self.open_page.notify_level = true;
            }
            let _ = ui.checkbox(&mut self.mute_any, "禁用任何通知");
            if !self.mute_any {
                let _ = ui.checkbox(&mut self.mute_all, "禁用 @ 全体 通知");
            }
        });

        // 选项（把原先多个 checkbox 合并在同一菜单内）
        ui.menu_button("选项", |ui| {
            ui.label("这里显示你打开了哪些选项页面");
            let _ = ui.checkbox(&mut self.open_page.settings, "设置");
            let _ = ui.checkbox(&mut self.open_page.custom_chat_ica, "定制聊天界面(ica)");
            let _ = ui.checkbox(&mut self.open_page.custom_chat_extra, "定制聊天界面(extra)");
            let _ = ui.checkbox(&mut self.open_page.online_status, "在线状态");
            let _ = ui.checkbox(&mut self.open_page.socketio_status, "Socketio 状态");
            let _ = ui.checkbox(&mut self.open_page.raw_config, "配置文件编辑");
        });

        // 帮助
        ui.menu_button("帮助", |ui| {
            let link = Hyperlink::from_label_and_url("Github(文档)", crate::GITHUB_LINK);
            ui.add(link);
            if ui.button("关于").clicked() {
                self.open_page.about = true;
            }
        });
    }

    // 左侧群组面板
    pub fn render_left_groups_panel(&mut self, ui: &mut egui::Ui) {
        let disable_groups = self.custom_chat.disable_chat_group;
        let disable_dot = self.custom_chat.disable_chat_group_dot;

        egui::Panel::left("群聊组")
            .resizable(false)
            .exact_size(70.0)
            .show_inside(ui, |ui| {
                let img = Image::new(crate::assets::svg::CHAT_GROUP)
                    .fit_to_exact_size([24.0, 24.0].into())
                    .alt_text("chat_group_icon");

                let rooms_snapshot = self
                    .active_bridge_state()
                    .map(|state| state.rooms.clone())
                    .unwrap_or_default();

                ui.spacing_mut().item_spacing.x = 0.5;
                ui.vertical_centered(|ui| {
                    // 所有聊天
                    {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            self.selected_chat_group = SelectedChatGroup::All;
                        }
                        let mut text = RichText::new("所有聊天");
                        if self.selected_chat_group == SelectedChatGroup::All {
                            text = text.strong();
                        }
                        ui.add(Label::new(text).selectable(false));
                    }

                    // 私聊
                    {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            self.selected_chat_group = SelectedChatGroup::Private;
                        }
                        let mut text = RichText::new("私聊");
                        if self.selected_chat_group == SelectedChatGroup::Private {
                            text = text.strong();
                        }
                        // 私聊未读红点
                        if !disable_groups
                            && !disable_dot
                            && self.selected_chat_group != SelectedChatGroup::Private
                        {
                            let has_unread = rooms_snapshot
                                .iter()
                                .any(|r| r.room_id > 0 && r.unread_count > 0);
                            if has_unread {
                                let dot_radius = 3.0;
                                let dot_pos =
                                    resp.rect.right_top() + egui::vec2(-dot_radius, dot_radius);
                                ui.painter()
                                    .circle_filled(dot_pos, dot_radius, egui::Color32::RED);
                            }
                        }
                        ui.add(Label::new(text).selectable(false));
                    }

                    // 用户自定义分组
                    for (idx, group_name) in self.chat_groups.group_names().iter().enumerate() {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        let is_selected = matches!(
                            &self.selected_chat_group,
                            SelectedChatGroup::Custom(i) if *i == idx
                        );
                        if resp.clicked() {
                            self.selected_chat_group = SelectedChatGroup::Custom(idx);
                        }

                        // 未读红点
                        if !disable_groups && !disable_dot && !is_selected {
                            if self.chat_groups.has_unread_in_group(idx, &rooms_snapshot) {
                                let dot_radius = 3.0;
                                let dot_pos =
                                    resp.rect.right_top() + egui::vec2(-dot_radius, dot_radius);
                                ui.painter()
                                    .circle_filled(dot_pos, dot_radius, egui::Color32::RED);
                            }
                        }

                        // 右键菜单
                        resp.context_menu(|ui| {
                            if let Some(room_id) =
                                self.active_bridge_state().and_then(|s| s.selected_room_id)
                            {
                                let in_group = self.chat_groups.is_room_in_group(idx, room_id);
                                let label = if in_group {
                                    "移出当前会话"
                                } else {
                                    "加入当前会话"
                                };
                                if ui.button(label).clicked() {
                                    self.chat_groups.toggle_room_in_group(idx, room_id);
                                    self.save_chat_groups();
                                    if let Some(bridge_idx) = self.active_bridge_idx {
                                        if let Some(group) = self.chat_groups.groups.get(idx) {
                                            self.send_update_chat_group(
                                                bridge_idx,
                                                &group.name,
                                                &group.rooms,
                                                group.include_all_personal,
                                            );
                                        }
                                    }
                                    ui.close();
                                }
                            }
                            if ui.button("编辑分组").clicked() {
                                self.open_page.chat_group_editor = true;
                                ui.close();
                            }
                        });

                        let mut text: egui::RichText = group_name.as_str().into();
                        if is_selected {
                            text = text.strong();
                        }
                        ui.add(Label::new(text).selectable(false));
                    }

                    // 管理按钮
                    ui.add_space(8.0);
                    if ui
                        .add_sized([24.0, 24.0], Button::new(RichText::new("+").size(16.0)))
                        .clicked()
                    {
                        self.open_page.chat_group_editor = true;
                    }
                    if ui
                        .add_sized([24.0, 24.0], Button::new(RichText::new("⚙").size(14.0)))
                        .clicked()
                    {
                        self.open_page.chat_group_editor = true;
                    }
                });
            });
    }

    // 聊天列表面板：把 header + rooms + 房间渲染整合为更少的函数（内部仍有一个私有房间渲染辅助）
    pub fn render_chat_list_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("聊天列表")
            .resizable(true)
            .size_range(300.0..=700.0)
            .show_inside(ui, |ui| {
                // 让聊天列表条目的背景能"铺满"左右分割线之间的整块区域：
                // 关键点：用 `ui.max_rect()` 的宽度来分配条目 rect，而不是 `ui.available_width()`
                // 因为 `available_width()` 会受当前 layout/indent/scroll 内容区影响而变窄，导致背景留白。
                let full_row_width = ui.max_rect().width();

                ui.horizontal(|ui| {
                    ui.label("Bridge");
                    if self.bridge_states.is_empty() {
                        ui.weak("没有启用的 bridge");
                    } else {
                        let selected_text = self
                            .active_bridge_state()
                            .map(|state| state.bridge_key.clone())
                            .unwrap_or_else(|| "未选择".to_string());

                        egui::ComboBox::from_id_salt("bridge_selector")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for idx in 0..self.bridge_states.len() {
                                    let bridge_key = self.bridge_states[idx].bridge_key.clone();
                                    if ui
                                        .selectable_label(
                                            self.active_bridge_idx == Some(idx),
                                            bridge_key,
                                        )
                                        .clicked()
                                    {
                                        self.active_bridge_idx = Some(idx);
                                    }
                                }
                            });
                    }
                });

                // 标题栏
                ui.horizontal(|ui| {
                    ui.label("聊天列表");
                    if ui.button("刷新").clicked()
                        && let Some(bridge_idx) = self.active_bridge_idx
                        && let Some(room_id) = self
                            .active_bridge_state()
                            .and_then(|state| state.selected_room_id)
                    {
                        self.request_room_messages(bridge_idx, room_id, false);
                    }
                    if ui.button("顶部").clicked() {
                        self.chat_list_scroll_target = ChatListScrollTarget::Top;
                    }
                    if ui.button("底部").clicked() {
                        self.chat_list_scroll_target = ChatListScrollTarget::Bottom;
                    }
                });

                let Some(active_bridge_idx) = self.active_bridge_idx else {
                    ui.weak("当前没有启用的 bridge");
                    return;
                };

                ui.horizontal(|ui| {
                    if let Some(state) = self.bridge_states.get_mut(active_bridge_idx) {
                        ui.add_sized(
                            [ui.available_width(), 0.0],
                            egui::TextEdit::singleline(&mut state.room_search_query)
                                .hint_text("会话名或 QQ/群号"),
                        );
                    }
                });
                ui.separator();

                let visible_rooms = self.visible_rooms(active_bridge_idx);
                if visible_rooms.is_empty() {
                    let has_query = self
                        .bridge_states
                        .get(active_bridge_idx)
                        .map(|state| !state.room_search_query.trim().is_empty())
                        .unwrap_or(false);
                    if has_query {
                        ui.weak("没有匹配的会话");
                    } else {
                        ui.weak("当前 bridge 还没有会话");
                    }
                    return;
                }

                let room_count = visible_rooms.len();
                // 内容矩形顶部内边距（头像与文字一起下移）
                let content_top_padding = 4.0;
                let content_height = 50.0;
                let row_spacing = ui.spacing().item_spacing.y;
                let row_height = content_height + content_top_padding + row_spacing;
                let total_height = row_height * room_count as f32;
                let mut pending_pin_change = None;
                let mut pending_remove_chat = None;
                let mut pending_ignore_chat: Option<(i64, String)> = None;

                let scroll_area = egui::ScrollArea::vertical().id_salt("chat_list_scroll");

                scroll_area.show_viewport(ui, |ui, viewport| {
                    let (list_rect, _) = ui.allocate_exact_size(
                        egui::vec2(full_row_width, total_height),
                        egui::Sense::hover(),
                    );

                    match self.chat_list_scroll_target {
                        ChatListScrollTarget::Top => {
                            ui.scroll_to_rect(list_rect, Some(egui::Align::Min));
                        }
                        ChatListScrollTarget::Bottom => {
                            ui.scroll_to_rect(list_rect, Some(egui::Align::Max));
                        }
                        ChatListScrollTarget::None => {}
                    }

                    if room_count == 0 {
                        return;
                    }

                    let viewport_top = viewport.top();
                    let viewport_bottom = viewport.bottom();

                    let mut start = (viewport_top / row_height).floor() as isize - 2;
                    let mut end = (viewport_bottom / row_height).ceil() as isize + 2;

                    if start < 0 {
                        start = 0;
                    }
                    if end < 0 {
                        end = 0;
                    }

                    let start = start as usize;
                    let end = (end as usize).min(room_count);

                    for (offset, room) in visible_rooms[start..end].iter().enumerate() {
                        let idx = start + offset;
                        let selected_room_id =
                            self.bridge_states[active_bridge_idx].selected_room_id;
                        let room_id = room.room_id;
                        let is_pinned = room.index > 0;
                        let is_selected = selected_room_id == Some(room_id);

                        let y = list_rect.top() + idx as f32 * row_height;
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(list_rect.left(), y),
                            egui::vec2(full_row_width, row_height),
                        );
                        let content_rect = egui::Rect::from_min_size(
                            egui::pos2(row_rect.left(), row_rect.top() + content_top_padding),
                            egui::vec2(full_row_width, content_height),
                        );

                        let id = ui.make_persistent_id(("chat_list_row", idx));
                        let response = ui.interact(row_rect, id, egui::Sense::click());

                        let dark_mode = ui.visuals().dark_mode;
                        let bg_color = if is_selected {
                            if dark_mode {
                                egui::Color32::from_rgb(0x22, 0x24, 0x2a)
                            } else {
                                egui::Color32::from_rgb(0xe5, 0xef, 0xfa)
                            }
                        } else if response.hovered() {
                            if dark_mode {
                                egui::Color32::from_rgb(0x1e, 0x1e, 0x25)
                            } else {
                                egui::Color32::from_rgb(0xf2, 0xf6, 0xfc)
                            }
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(row_rect, 4.0, bg_color);

                        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                // 左侧内边距：把整行内容从分割线向右挪一点
                                ui.add_space(4.0);
                                self.render_room(ui, room);
                            });
                        });

                        response.context_menu(|ui| {
                            // 房间名 + ID (disabled header)
                            ui.add_enabled(
                                false,
                                egui::Button::new(format!(
                                    "{} ({})",
                                    room.room_name,
                                    room_id.abs()
                                )),
                            );
                            ui.separator();

                            let label = if is_pinned { "取消置顶" } else { "置顶" };
                            if ui.button(label).clicked() {
                                pending_pin_change = Some((room_id, !is_pinned));
                                ui.close();
                            }
                            if ui.button("删除会话").clicked() {
                                pending_remove_chat = Some(room_id);
                                ui.close();
                            }
                            if ui.button("屏蔽消息").clicked() {
                                pending_ignore_chat = Some((room_id, room.room_name.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("复制名称").clicked() {
                                ui.ctx().copy_text(room.room_name.clone());
                                ui.close();
                            }
                            if ui.button("复制 ID").clicked() {
                                ui.ctx().copy_text(room_id.abs().to_string());
                                ui.close();
                            }
                            if ui.button("复制头像 URL").clicked() {
                                ui.ctx().copy_text(room.avatar_url());
                                ui.close();
                            }
                        });

                        if response.clicked() {
                            self.select_active_room(room_id);
                        }

                        // // 分隔线稍微往上提，避免紧贴行底
                        // let sep_y = row_rect.bottom() - row_spacing * 0.25;
                        // ui.painter().line_segment(
                        //     [
                        //         egui::pos2(list_rect.left(), sep_y),
                        //         egui::pos2(list_rect.right(), sep_y),
                        //     ],
                        //     ui.visuals().widgets.noninteractive.bg_stroke,
                        // );
                    }
                });

                if !matches!(self.chat_list_scroll_target, ChatListScrollTarget::None) {
                    self.chat_list_scroll_target = ChatListScrollTarget::None;
                }

                if let Some((room_id, pin)) = pending_pin_change {
                    self.set_room_pinned(active_bridge_idx, room_id, pin);
                }
                if let Some(room_id) = pending_remove_chat {
                    self.remove_chat(active_bridge_idx, room_id);
                }
                if let Some((room_id, room_name)) = pending_ignore_chat {
                    self.ignore_chat(active_bridge_idx, room_id, room_name);
                }
            });
    }

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
            let online_data = self.bridge_states[active_bridge_idx].online_data.clone();
            let last_error = self.bridge_states[active_bridge_idx].last_error.clone();
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

            ui.add_space(4.0);

            if selected_room_id.is_none() {
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
            let has_pending_image = self.bridge_states[active_bridge_idx]
                .pending_image_by_room
                .contains_key(&room_id);
            let has_pending_file = self.bridge_states[active_bridge_idx]
                .pending_file_by_room
                .contains_key(&room_id);
            let composer_reserved_height = 36.0
                + if forward_mode_active { 54.0 } else { 0.0 }
                + if has_reply_banner { 54.0 } else { 0.0 }
                + if has_pending_image { 54.0 } else { 0.0 }
                + if has_pending_file { 54.0 } else { 0.0 }
                + if self.show_face_picker { 220.0 } else { 0.0 };
            let message_list_height = (ui.available_height() - composer_reserved_height).max(120.0);
            let mut pending_action = None;

            let scroll_to_target = self.bridge_states[active_bridge_idx]
                .scroll_to_message_id
                .take();
            let scroll_output = egui::ScrollArea::vertical()
                .id_salt(("message_list", active_bridge_idx, room_id))
                .max_height(message_list_height)
                .show(ui, |ui| {
                    ui.set_min_width(ui.max_rect().width());

                    match self.bridge_states[active_bridge_idx]
                        .messages_by_room
                        .get(&room_id)
                    {
                        Some(messages) if !messages.is_empty() => {
                            let pure_text_mode = self.custom_chat.hide_group_member_avatar;
                            let mut previous_sender_id = None;
                            for message in messages {
                                let forward_selected = forward_mode_active
                                    && forward_selected_ids
                                        .iter()
                                        .any(|selected_id| selected_id == &message.msg_id);
                                let show_sender_name = !pure_text_mode
                                    || previous_sender_id != Some(message.sender_id);
                                let show_separator_before = pure_text_mode
                                    && previous_sender_id.is_some()
                                    && previous_sender_id != Some(message.sender_id);
                                let is_scroll_target =
                                    scroll_to_target.as_deref() == Some(&message.msg_id);
                                let msg_resp = ui.scope(|ui| {
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
                                if is_scroll_target {
                                    ui.scroll_to_rect(
                                        msg_resp.response.rect,
                                        Some(egui::Align::Center),
                                    );
                                }
                                previous_sender_id = Some(message.sender_id);
                            }
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
                    }
                    MessageAction::Delete {
                        room_id,
                        message_id,
                    } => {
                        self.send_delete_message(room_id, message_id);
                    }
                    MessageAction::ReEdit { room_id, content } => {
                        self.restore_deleted_message_to_draft(room_id, content);
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
                        self.bridge_states[active_bridge_idx].scroll_to_message_id = Some(msg_id);
                    }
                    MessageAction::RenewMessage {
                        room_id,
                        message_id,
                    } => {
                        self.send_renew_message(room_id, message_id);
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
            let mut paste_image: Option<PendingImage> = None;
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), composer_reserved_height),
                egui::Layout::bottom_up(egui::Align::Min),
                |ui| {
                    let control_height = 30.0;
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
                                    .desired_rows(1)
                                    .hint_text("Enter 发送, Shift+Enter 换行"),
                            );
                            // 检测 Enter 键（不带 Shift/Ctrl）→ 发送
                            let enter_no_mod = response.has_focus()
                                && ui.input(|input| {
                                    input.key_pressed(egui::Key::Enter)
                                        && !input.modifiers.shift
                                        && !input.modifiers.ctrl
                                });
                            if enter_no_mod {
                                // multiline TextEdit 已经插入了 '\n'，需要撤销
                                while draft.ends_with('\n') || draft.ends_with('\r') {
                                    draft.pop();
                                }
                            }
                            // Ctrl+V 粘贴图片
                            // egui_winit 消费了 Ctrl+V 键事件，当剪贴板是图片时
                            // raw_input_hook 会设置 clipboard_paste_failed 标志
                            if response.has_focus()
                                && self.clipboard_paste_failed
                                && !has_pending_image
                            {
                                match arboard::Clipboard::new() {
                                    Ok(mut clipboard) => match clipboard.get_image() {
                                        Ok(img) => {
                                            tracing::debug!(
                                                "剪贴板图片: {}x{}, {} bytes",
                                                img.width,
                                                img.height,
                                                img.bytes.len()
                                            );
                                            let rgba: Vec<u8> = img.bytes.into_owned();
                                            if let Some(buf) = image::RgbaImage::from_raw(
                                                img.width as u32,
                                                img.height as u32,
                                                rgba,
                                            ) {
                                                let mut png_data = Vec::new();
                                                let encoder = image::codecs::png::PngEncoder::new(
                                                    std::io::Cursor::new(&mut png_data),
                                                );
                                                if image::ImageEncoder::write_image(
                                                    encoder,
                                                    buf.as_raw(),
                                                    buf.width(),
                                                    buf.height(),
                                                    image::ExtendedColorType::Rgba8,
                                                )
                                                .is_ok()
                                                {
                                                    paste_image = Some(PendingImage {
                                                        name: "clipboard.png".to_string(),
                                                        mime_type: "image/png".to_string(),
                                                        data: png_data,
                                                    });
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::debug!("剪贴板无图片: {}", e);
                                        }
                                    },
                                    Err(e) => {
                                        tracing::warn!("无法打开剪贴板: {}", e);
                                    }
                                }
                            }
                            let enter_pressed = enter_no_mod;
                            // 表情按钮（窄宽度时隐藏）
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
                            should_send = enter_pressed
                                || ui
                                    .add_sized(
                                        [button_width, control_height],
                                        Button::new(RichText::new("↗").size(15.0)),
                                    )
                                    .clicked();
                        },
                    );

                    if forward_mode_active {
                        ui.add_space(6.0);
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
                    }

                    if let Some(image) = self.bridge_states[active_bridge_idx]
                        .pending_image_by_room
                        .get(&room_id)
                        .cloned()
                    {
                        ui.add_space(6.0);
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.weak("待发送图片");
                                if ui.button("取消").clicked() {
                                    clear_image = true;
                                }
                            });
                            ui.add(
                                Label::new(format!(
                                    "{} ({:.1} KB)",
                                    image.name,
                                    image.data.len() as f32 / 1024.0
                                ))
                                .wrap(),
                            );
                        });
                    }

                    if let Some(file) = self.bridge_states[active_bridge_idx]
                        .pending_file_by_room
                        .get(&room_id)
                        .cloned()
                    {
                        ui.add_space(6.0);
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.weak("待发送文件");
                                if ui.button("取消").clicked() {
                                    clear_file = true;
                                }
                            });
                            let size = file.data.len() as f64;
                            let size_str = if size >= 1024.0 * 1024.0 {
                                format!("{:.1} MB", size / (1024.0 * 1024.0))
                            } else {
                                format!("{:.1} KB", size / 1024.0)
                            };
                            ui.add(Label::new(format!("📎 {} ({})", file.name, size_str)).wrap());
                        });
                    }

                    if let Some(reply) = self.bridge_states[active_bridge_idx]
                        .reply_to_by_room
                        .get(&room_id)
                        .cloned()
                    {
                        ui.add_space(6.0);
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.weak(format!("正在回复 {}", reply.sender_name));
                                if ui.button("取消").clicked() {
                                    clear_reply = true;
                                }
                            });
                            ui.add(Label::new(format_message_content(&reply.content)).wrap());
                        });
                    }

                    // 表情选择器面板
                    if self.show_face_picker {
                        ui.add_space(4.0);
                        let face_panel_height = 200.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), face_panel_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height(face_panel_height - 10.0)
                                        .show(ui, |ui| {
                                            let face_size = 32.0;
                                            let spacing = 4.0;
                                            let avail = ui.available_width();
                                            // Button 有内边距，实际每个按钮宽度约为 face_size + 2 * button_padding
                                            let button_padding =
                                                ui.spacing().button_padding.x * 2.0;
                                            let cell_width = face_size + button_padding + spacing;
                                            let cols =
                                                ((avail + spacing) / cell_width).max(1.0) as usize;
                                            egui::Grid::new("face_picker_grid")
                                                .spacing([spacing, spacing])
                                                .show(ui, |ui| {
                                                    for (i, face_id) in
                                                        crate::face_data::all_face_ids().enumerate()
                                                    {
                                                        let bytes =
                                                            crate::face_data::get_face(face_id)
                                                                .unwrap();
                                                        let uri = format!("bytes://face_{face_id}");
                                                        let img = Image::from_bytes(uri, bytes)
                                                            .fit_to_exact_size(egui::vec2(
                                                                face_size, face_size,
                                                            ));
                                                        let btn = ui.add(Button::image(img));
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
                                                            draft.push_str(&format!(
                                                                "[Face: {}]",
                                                                face_id
                                                            ));
                                                            self.show_face_picker = false;
                                                        }
                                                        if (i + 1) % cols == 0 {
                                                            ui.end_row();
                                                        }
                                                    }
                                                });
                                        });
                                });
                            },
                        );
                    }
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

            if clear_file {
                self.bridge_states[active_bridge_idx]
                    .pending_file_by_room
                    .remove(&room_id);
            }

            if let Some(image) = paste_image {
                self.bridge_states[active_bridge_idx]
                    .pending_image_by_room
                    .insert(room_id, image);
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

            if let Some(file) = dropped.into_iter().next() {
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

                if !data.is_empty() {
                    let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
                    let image_exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
                    if image_exts.contains(&ext.as_str()) {
                        let mime = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                        self.bridge_states[active_bridge_idx]
                            .pending_image_by_room
                            .insert(
                                room_id,
                                PendingImage {
                                    name: file_name,
                                    mime_type: mime,
                                    data,
                                },
                            );
                    } else {
                        let ft = IcaApp::guess_mime_type(std::path::Path::new(&file_name));
                        self.bridge_states[active_bridge_idx]
                            .pending_file_by_room
                            .insert(
                                room_id,
                                PendingFile {
                                    name: file_name,
                                    file_type: ft,
                                    data,
                                },
                            );
                    }
                }
            }
        });
    }

    // 合并房间内的头像 / 名称行 / 预览行 为一个方法，减少外部碎片函数
    fn render_room(&self, ui: &mut egui::Ui, room: &Room) {
        ui.style_mut().interaction.selectable_labels = false;

        // 左侧：头像区域（方形，固定大小）
        // 群聊时右下角叠加发送者头像
        // 使用 LayerId 叠加两个头像（保留原注释以便后续改进）
        let is_group = room.room_id < 0;
        let dark_mode = ui.visuals().dark_mode;
        let avatar_size = 40.0;
        let sender_avatar_size = 20.0;

        // 使用 LayerId 叠加两个头像
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(avatar_size, avatar_size), egui::Sense::hover());

        // 主头像（群头像或私聊头像）
        let avatar_url = room.avatar_url();
        ui.put(
            rect,
            egui::Image::from_uri(avatar_url)
                .fit_to_exact_size(egui::vec2(avatar_size, avatar_size))
                .corner_radius(8.0),
        );
        // 群聊时叠加发送者头像在右下角
        if is_group && let Some(user_id) = room.last_message.user_id {
            let sender_url = format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", user_id);
            let sender_rect = egui::Rect::from_min_size(
                egui::pos2(
                    rect.right() - sender_avatar_size - 2.0,
                    rect.bottom() - sender_avatar_size - 2.0,
                ),
                egui::vec2(sender_avatar_size, sender_avatar_size),
            );
            ui.put(
                sender_rect,
                egui::Image::from_uri(sender_url)
                    .fit_to_exact_size(egui::vec2(sender_avatar_size, sender_avatar_size))
                    .corner_radius(4.0),
            );
        }

        // 头像 与 信息 的间距
        // ui.add_space(2.0);

        // 内容区：名称 + 预览
        let content_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(content_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                // 文字部分相对于头像部分额外的顶部内边距
                // 用来让文字部分看着居中
                ui.add_space(2.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                // 第一行：名称、@ 提示、未读数
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(ref timestamp) = room.last_message.timestamp
                        && !timestamp.is_empty()
                    {
                        let ts_color = if dark_mode {
                            egui::Color32::from_rgb(0xb3, 0xba, 0xc9)
                        } else {
                            egui::Color32::from_rgb(0x60, 0x62, 0x66)
                        };
                        ui.label(RichText::new(timestamp).size(11.0).color(ts_color));
                    }
                    if room.index > 0 {
                        let pin_color = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(0xC0, 0xC4, 0xCC)
                        } else {
                            egui::Color32::from_rgb(0x90, 0x93, 0x99)
                        };
                        ui.label(RichText::new("↑").size(11.0).color(pin_color));
                    }

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let name_text = if room.room_name.is_empty() {
                            "未命名聊天"
                        } else {
                            &room.room_name
                        };
                        let mut text = RichText::new(name_text).size(16.0);
                        if room.unread_count > 0 {
                            text = text.strong();
                        }
                        ui.label(text);
                    });
                });

                // 第二行：消息预览（群聊显示用户名: 内容）+ 未读数胶囊
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if room.unread_count > 0 {
                        let unread_text = room.unread_count.to_string();
                        let font_size = 12.0;
                        let badge_color = match room.at {
                            crate::ica::types::message::At::All => egui::Color32::ORANGE,
                            crate::ica::types::message::At::Bool(true) => egui::Color32::RED,
                            _ => egui::Color32::from_gray(140),
                        };
                        let galley = ui.painter().layout_no_wrap(
                            unread_text.clone(),
                            egui::FontId::proportional(font_size),
                            egui::Color32::WHITE,
                        );
                        let text_width = galley.size().x;
                        let text_height = galley.size().y;
                        let padding_x = 5.0;
                        let padding_y = 1.0;
                        let badge_width = text_width + padding_x * 2.0;
                        let badge_height = text_height + padding_y * 2.0;

                        let (badge_rect, _) = ui.allocate_exact_size(
                            egui::vec2(badge_width, badge_height),
                            egui::Sense::hover(),
                        );
                        let rounding = badge_height / 2.0;
                        ui.painter().rect_filled(badge_rect, rounding, badge_color);
                        ui.painter().text(
                            badge_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            unread_text,
                            egui::FontId::proportional(font_size),
                            egui::Color32::WHITE,
                        );
                    }

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if is_group
                            && let Some(ref username) = room.last_message.username
                            && !username.is_empty()
                        {
                            ui.add(
                                Label::new(
                                    RichText::new(format!("{}:", username)).size(12.0).color(
                                        if dark_mode {
                                            egui::Color32::from_rgb(0x52, 0xa3, 0xe8)
                                        } else {
                                            egui::Color32::from_rgb(0x19, 0x76, 0xd2)
                                        },
                                    ),
                                )
                                .selectable(false),
                            );
                        }
                        if let Some(ref content) = room.last_message.content
                            && !content.is_empty()
                        {
                            let preview_color = if dark_mode {
                                egui::Color32::from_rgb(0xb3, 0xba, 0xc9)
                            } else {
                                egui::Color32::from_rgb(0x60, 0x62, 0x66)
                            };
                            ui.add(
                                Label::new(
                                    RichText::new(format_message_content(content))
                                        .size(12.0)
                                        .color(preview_color),
                                )
                                .selectable(false),
                            );
                        }
                    });
                });
            },
        );
    }
}
