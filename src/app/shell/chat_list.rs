use crate::app::{ChatListScrollTarget, IcaApp};
use crate::ica::types::room::Room;

use crate::app::chat::format_message_content;

fn visible_row_range(
    viewport_top: f32,
    viewport_bottom: f32,
    row_height: f32,
    row_count: usize,
) -> std::ops::Range<usize> {
    let start = ((viewport_top / row_height).floor() as isize - 2).max(0) as usize;
    let end = ((viewport_bottom / row_height).ceil() as isize + 2).max(0) as usize;
    start.min(row_count)..end.min(row_count)
}

impl IcaApp {
    pub fn render_chat_list_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("聊天列表")
            .resizable(true)
            .size_range(300.0..=700.0)
            .show(ui, |ui| {
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
                                        self.switch_active_bridge(idx);
                                    }
                                }
                            });
                    }
                });

                // 标题栏
                ui.horizontal(|ui| {
                    ui.label("聊天列表");
                    if ui.button("联系人").clicked() {
                        self.open_contacts();
                    }
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

                let visible_room_indices = self.visible_room_indices(active_bridge_idx);
                if visible_room_indices.is_empty() {
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

                let room_count = visible_room_indices.len();
                // 内容矩形顶部内边距（头像与文字一起下移）
                let content_top_padding = 4.0;
                let content_height = 50.0;
                let row_spacing = ui.spacing().item_spacing.y;
                let row_height = content_height + content_top_padding + row_spacing;
                let total_height = row_height * room_count as f32;
                let mut pending_pin_change = None;
                let mut pending_remove_chat = None;
                let mut pending_ignore_chat: Option<(i64, String)> = None;
                let mut pending_message_search: Option<(i64, String)> = None;
                let mut pending_room_selection = None;

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

                    let visible_range = visible_row_range(
                        viewport.top(),
                        viewport.bottom(),
                        row_height,
                        room_count,
                    );
                    let start = visible_range.start;
                    let end = visible_range.end;

                    for (offset, &room_idx) in visible_room_indices[start..end].iter().enumerate() {
                        let idx = start + offset;
                        let room = &self.bridge_states[active_bridge_idx].rooms[room_idx];
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

                        self.render_room(ui, content_rect, room);

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
                            if ui.button("搜索聊天记录").clicked() {
                                pending_message_search = Some((room_id, room.room_name.clone()));
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
                            pending_room_selection = Some(room_id);
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

                if let Some(room_id) = pending_room_selection {
                    self.select_active_room(room_id);
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
                if let Some((room_id, room_name)) = pending_message_search {
                    self.open_message_search(active_bridge_idx, room_id, room_name);
                }
            });
    }

    fn render_room(&self, ui: &mut egui::Ui, rect: egui::Rect, room: &Room) {
        // 聊天列表是手写虚拟列表：快速滚动时，同一个屏幕 rect 会在 egui
        // 的多次 pass 之间对应到不同 room。这里刻意不用 Label/Button 等子
        // widget，只保留外层 row 的 interact id，内部全部 painter 绘制，避免
        // 行内自动生成的 widget id 在同一 rect 上来回变化并触发 egui warning。
        let is_group = room.room_id < 0;
        let dark_mode = ui.visuals().dark_mode;
        let avatar_size = 40.0;
        let sender_avatar_size = 20.0;
        // 统一裁剪到当前行，防止长文本或图片越界污染相邻行。
        let painter = ui.painter().with_clip_rect(rect);

        // 头像不参与布局分配，位置必须由 row_rect 直接推导；这样虚拟滚动时
        // 不会因为 allocate 顺序变化产生额外 widget id。
        let avatar_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 4.0, rect.center().y - avatar_size / 2.0),
            egui::vec2(avatar_size, avatar_size),
        );

        let avatar_url = room.avatar_url();
        egui::Image::from_uri(avatar_url)
            .fit_to_exact_size(egui::vec2(avatar_size, avatar_size))
            .corner_radius(8.0)
            .paint_at(ui, avatar_rect);

        // 群聊头像右下角叠加最后发言人的头像，只做绘制，不单独注册 hover/click。
        if is_group && let Some(user_id) = room.last_message.user_id {
            let sender_url = format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", user_id);
            let sender_rect = egui::Rect::from_min_size(
                egui::pos2(
                    avatar_rect.right() - sender_avatar_size - 2.0,
                    avatar_rect.bottom() - sender_avatar_size - 2.0,
                ),
                egui::vec2(sender_avatar_size, sender_avatar_size),
            );
            egui::Image::from_uri(sender_url)
                .fit_to_exact_size(egui::vec2(sender_avatar_size, sender_avatar_size))
                .corner_radius(4.0)
                .paint_at(ui, sender_rect);
        }

        // 两行文本的固定基线。row 高度在上层虚拟列表中固定，文字位置也要固定，
        // 否则滚动时内容高度估算和实际绘制会逐渐偏离。
        let text_left = avatar_rect.right() + 8.0;
        let text_right = rect.right() - 8.0;
        let name_y = rect.top() + 5.0;
        let preview_y = rect.top() + 28.0;

        let muted_color = if dark_mode {
            egui::Color32::from_rgb(0xb3, 0xba, 0xc9)
        } else {
            egui::Color32::from_rgb(0x60, 0x62, 0x66)
        };
        let pin_color = if dark_mode {
            egui::Color32::from_rgb(0xC0, 0xC4, 0xCC)
        } else {
            egui::Color32::from_rgb(0x90, 0x93, 0x99)
        };
        let name_color = ui.visuals().text_color();

        // 第一行右侧先画时间和置顶标记，并把 right_limit 向左推进。
        // 名称随后裁剪在 [text_left, right_limit] 内，避免和右侧状态文字重叠。
        let mut right_limit = text_right;
        if let Some(timestamp) = &room.last_message.timestamp
            && !timestamp.is_empty()
        {
            let galley = painter.layout_no_wrap(
                timestamp.clone(),
                egui::FontId::proportional(11.0),
                muted_color,
            );
            let pos = egui::pos2(right_limit - galley.size().x, name_y + 3.0);
            painter.galley(pos, galley.clone(), muted_color);
            right_limit = pos.x - 6.0;
        }
        if room.index > 0 {
            let galley =
                painter.layout_no_wrap("↑".to_owned(), egui::FontId::proportional(11.0), pin_color);
            let pos = egui::pos2(right_limit - galley.size().x, name_y + 3.0);
            painter.galley(pos, galley, pin_color);
            right_limit = pos.x - 6.0;
        }

        // 房间名可能很长，使用独立 clip rect 做截断；不要改回 Label::truncate，
        // 否则会重新引入行内 widget id。
        let name_text = if room.room_name.is_empty() {
            "未命名聊天"
        } else {
            &room.room_name
        };
        let name_font = egui::FontId::proportional(16.0);
        let name_clip = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.top()),
            egui::pos2(right_limit.max(text_left), rect.bottom()),
        );
        let name_painter = painter.with_clip_rect(name_clip);
        let name_galley = name_painter.layout_no_wrap(name_text.to_owned(), name_font, name_color);
        name_painter.galley(egui::pos2(text_left, name_y), name_galley, name_color);

        // 第二行右侧先画未读胶囊，再让消息预览在剩余宽度里裁剪。
        let mut preview_right = text_right;
        if room.unread_count > 0 {
            let unread_text = room.unread_count.to_string();
            let font_size = 12.0;
            let badge_color = match room.at {
                crate::ica::types::message::At::All => egui::Color32::ORANGE,
                crate::ica::types::message::At::Bool(true) => egui::Color32::RED,
                _ => egui::Color32::from_gray(140),
            };
            let galley = painter.layout_no_wrap(
                unread_text,
                egui::FontId::proportional(font_size),
                egui::Color32::WHITE,
            );
            let padding = egui::vec2(5.0, 1.0);
            let badge_size = galley.size() + padding * 2.0;
            let badge_rect = egui::Rect::from_min_size(
                egui::pos2(preview_right - badge_size.x, preview_y + 2.0),
                badge_size,
            );
            painter.rect_filled(badge_rect, badge_size.y / 2.0, badge_color);
            painter.galley(badge_rect.min + padding, galley, egui::Color32::WHITE);
            preview_right = badge_rect.left() - 8.0;
        }

        // 群聊预览前缀显示发送者名称；preview_x 会继续向右推进，
        // 后面的消息内容只占用剩下的空间。
        let mut preview_x = text_left;
        if is_group
            && let Some(username) = &room.last_message.username
            && !username.is_empty()
        {
            let username_color = if dark_mode {
                egui::Color32::from_rgb(0x52, 0xa3, 0xe8)
            } else {
                egui::Color32::from_rgb(0x19, 0x76, 0xd2)
            };
            let galley = painter.layout_no_wrap(
                format!("{username}:"),
                egui::FontId::proportional(12.0),
                username_color,
            );
            painter.galley(
                egui::pos2(preview_x, preview_y),
                galley.clone(),
                username_color,
            );
            preview_x += galley.size().x + 4.0;
        }
        // 消息内容也通过 painter clip 截断。这里用 layout_no_wrap 是为了保持
        // 单行预览，和旧的 TextWrapMode::Truncate 行为一致。
        if let Some(content) = &room.last_message.content
            && !content.is_empty()
            && preview_x < preview_right
        {
            let preview = format_message_content(content).into_owned();
            let preview_clip = egui::Rect::from_min_max(
                egui::pos2(preview_x, rect.top()),
                egui::pos2(preview_right, rect.bottom()),
            );
            let preview_painter = painter.with_clip_rect(preview_clip);
            let galley = preview_painter.layout_no_wrap(
                preview,
                egui::FontId::proportional(12.0),
                muted_color,
            );
            preview_painter.galley(egui::pos2(preview_x, preview_y), galley, muted_color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::visible_row_range;

    #[test]
    fn visible_range_is_empty_when_stale_scroll_offset_exceeds_shortened_list() {
        assert_eq!(visible_row_range(2_500.0, 3_000.0, 54.0, 17), 17..17);
    }

    #[test]
    fn visible_range_keeps_overscan_inside_list_bounds() {
        assert_eq!(visible_row_range(108.0, 324.0, 54.0, 20), 0..8);
    }
}
