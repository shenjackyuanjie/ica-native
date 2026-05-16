use crate::app::{ChatListScrollTarget, IcaApp};
use crate::ica::types::room::Room;
use egui::{Label, RichText};

use super::format_message_content;

impl IcaApp {
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

                let scroll_area =
                    egui::ScrollArea::vertical().id_salt(("chat_list_scroll", active_bridge_idx));

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

                        let id =
                            ui.make_persistent_id(("chat_list_row", active_bridge_idx, room_id));
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
