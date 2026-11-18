//! 最左侧的聊天分组导航栏：账号、分组切换和未读提示。

#[cfg(test)]
#[path = "tests/left_groups.rs"]
mod tests;

use crate::app::{ChatListScrollTarget, CompactChatPanel, IcaApp, SelectedChatGroup};
use crate::config::chat_groups::ChatGroup;
use crate::ica::types::{RoomId, message::At, room::Room};
use egui::{Button, Image, Label, RichText};
use std::collections::HashSet;

#[derive(Default)]
struct SidebarUnread {
    rooms: HashSet<RoomId>,
    private: bool,
    group: bool,
}

impl SidebarUnread {
    fn from_rooms(rooms: &[Room], notify_level: u8, include_at_all: bool) -> Self {
        let mut unread = Self::default();
        for room in rooms.iter().filter(|room| {
            room.unread_count > 0
                && (room.priority >= notify_level
                    || matches!(room.at, At::Bool(true))
                    || (include_at_all && matches!(room.at, At::All)))
        }) {
            unread.rooms.insert(room.room_id);
            unread.private |= room.room_id > 0;
            unread.group |= room.room_id < 0;
        }
        unread
    }

    fn in_custom_group(&self, group: &ChatGroup) -> bool {
        (group.include_all_personal && self.private)
            || group.rooms.iter().any(|room| self.rooms.contains(room))
    }
}

fn draw_group(
    ui: &mut egui::Ui,
    image: &Image<'_>,
    name: &str,
    selected: bool,
    unread: bool,
) -> egui::Response {
    let response = ui.add(Button::image(image.clone())).on_hover_text(name);
    if unread && !selected {
        let radius = 3.0;
        let pos = response.rect.right_top() + egui::vec2(-radius, radius);
        ui.painter().circle_filled(pos, radius, egui::Color32::RED);
    }
    let text = RichText::new(name);
    let text = if selected { text.strong() } else { text };
    ui.add_sized([64.0, 18.0], Label::new(text).selectable(false).truncate());
    response
}

impl IcaApp {
    /// 分类按钮与键盘共用入口；不改变当前房间，也不持久化临时筛选。
    ///
    /// 分组功能被外观设置禁用、目标分组越界时返回 `false`；成功切换时复位
    /// 列表滚动位置，窄屏下同时回到会话列表页。
    pub fn select_active_chat_group(&mut self, group: SelectedChatGroup) -> bool {
        if self.custom_chat.disable_chat_group || self.custom_chat.hide_chat_group_sidebar {
            return false;
        }
        let Some(state) = self.active_bridge_state_mut() else {
            return false;
        };
        if matches!(&group, SelectedChatGroup::Custom(index) if *index >= state.chat_groups.groups.len())
        {
            return false;
        }
        if state.selected_chat_group != group {
            state.selected_chat_group = group;
            state.invalidate_visible_room_indices();
        }
        self.chat_list_scroll_target = ChatListScrollTarget::Top;
        self.compact_chat_panel = CompactChatPanel::Conversations;
        true
    }

    /// 账号和分组管理入口固定在顶部，分类按钮单独滚动。
    pub fn render_left_groups_panel(&mut self, ui: &mut egui::Ui) {
        if self.custom_chat.disable_chat_group || self.custom_chat.hide_chat_group_sidebar {
            return;
        }
        let active_bridge_idx = self.active_bridge_idx;
        let state = self.active_bridge_state();
        let selected = state.map_or(&SelectedChatGroup::All, |state| &state.selected_chat_group);
        // 屏蔽 @全体时，它不能让低于通知级别的会话额外触发红点。
        let unread = state
            .filter(|_| !self.custom_chat.disable_chat_group_dot)
            .map(|state| SidebarUnread::from_rooms(&state.rooms, self.notify_level, !self.mute_all))
            .unwrap_or_default();
        let mut select_group = None;
        let mut toggle_membership = None;
        let mut open_editor = false;
        let mut open_online_status = false;

        egui::Panel::left("群聊组")
            .resizable(false)
            .exact_size(70.0)
            .show(ui, |ui| {
                let image = Image::new(crate::assets::svg::CHAT_GROUP)
                    .fit_to_exact_size([24.0, 24.0].into());
                ui.spacing_mut().item_spacing.x = 0.5;
                ui.vertical_centered(|ui| {
                    if let Some(state) = state
                        && state.online_data.qqid > 0
                    {
                        let account = &state.online_data;
                        let avatar = Image::from_uri(format!(
                            "https://q1.qlogo.cn/g?b=qq&nk={}&s=100",
                            account.qqid
                        ))
                        .fit_to_exact_size([32.0, 32.0].into())
                        .corner_radius(16.0);
                        open_online_status = ui
                            .add(Button::image(avatar))
                            .on_hover_text(format!("{} ({})", account.nick, account.qqid))
                            .clicked();
                    }
                    open_editor = ui.small_button("编辑分组").clicked();
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("chat_group_sidebar", active_bridge_idx))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (category, name, has_unread) in [
                                (SelectedChatGroup::All, "所有聊天", !unread.rooms.is_empty()),
                                (SelectedChatGroup::Group, "群聊", unread.group),
                                (SelectedChatGroup::Private, "私聊", unread.private),
                            ] {
                                let is_selected = selected == &category;
                                if draw_group(ui, &image, name, is_selected, has_unread).clicked() {
                                    select_group = Some(category);
                                }
                            }
                            if let Some(state) = state {
                                for (index, group) in state.chat_groups.groups.iter().enumerate() {
                                    let category = SelectedChatGroup::Custom(index);
                                    let response = draw_group(
                                        ui,
                                        &image,
                                        &group.name,
                                        selected == &category,
                                        unread.in_custom_group(group),
                                    );
                                    if response.clicked() {
                                        select_group = Some(category);
                                    }
                                    response.context_menu(|ui| {
                                        if let Some(room_id) = state.selected_room_id {
                                            let label = if group.rooms.contains(&room_id) {
                                                "移出当前会话"
                                            } else {
                                                "加入当前会话"
                                            };
                                            if ui.button(label).clicked() {
                                                toggle_membership = Some((index, room_id));
                                                ui.close();
                                            }
                                        }
                                        if ui.button("编辑分组").clicked() {
                                            open_editor = true;
                                            ui.close();
                                        }
                                    });
                                }
                            }
                        });
                });
            });

        if open_online_status {
            self.open_page.online_status = true;
        }
        if open_editor {
            self.open_page.chat_group_editor = true;
        }
        if let Some(group) = select_group {
            self.select_active_chat_group(group);
        }
        // 只在确实改动分组成员时写回并使列表缓存失效。
        if let Some((index, room_id)) = toggle_membership
            && let Some(bridge_idx) = active_bridge_idx
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
            && index < state.chat_groups.groups.len()
        {
            state.chat_groups.toggle_room_in_group(index, room_id);
            state.invalidate_visible_room_indices();
            let group = &self.bridge_states[bridge_idx].chat_groups.groups[index];
            self.send_update_chat_group(
                bridge_idx,
                &group.name,
                &group.rooms,
                group.include_all_personal,
            );
        }
    }
}
