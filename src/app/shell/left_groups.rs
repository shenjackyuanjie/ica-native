use crate::app::{IcaApp, SelectedChatGroup};
use egui::{Button, Image, Label, RichText};

impl IcaApp {
    pub fn render_left_groups_panel(&mut self, ui: &mut egui::Ui) {
        let disable_groups = self.custom_chat.disable_chat_group;
        let disable_dot = self.custom_chat.disable_chat_group_dot;
        let active_bridge_idx = self.active_bridge_idx;
        let (mut chat_groups, mut selected_chat_group, rooms, selected_room_id) = active_bridge_idx
            .and_then(|idx| self.bridge_states.get(idx))
            .map(|state| {
                (
                    state.chat_groups.clone(),
                    state.selected_chat_group.clone(),
                    state.rooms.clone(),
                    state.selected_room_id,
                )
            })
            .unwrap_or_else(|| {
                (
                    crate::config::ChatGroups::default(),
                    SelectedChatGroup::All,
                    Vec::new(),
                    None,
                )
            });
        let private_has_unread = rooms
            .iter()
            .any(|room| room.room_id > 0 && room.unread_count > 0);
        let group_has_unread = (0..chat_groups.groups.len())
            .map(|idx| chat_groups.has_unread_in_group(idx, &rooms))
            .collect::<Vec<_>>();
        let mut updated_group = None;

        egui::Panel::left("群聊组")
            .resizable(false)
            .exact_size(70.0)
            .show(ui, |ui| {
                let img = Image::new(crate::assets::svg::CHAT_GROUP)
                    .fit_to_exact_size([24.0, 24.0].into())
                    .alt_text("chat_group_icon");

                ui.spacing_mut().item_spacing.x = 0.5;
                ui.vertical_centered(|ui| {
                    // 所有聊天
                    {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            selected_chat_group = SelectedChatGroup::All;
                        }
                        let mut text = RichText::new("所有聊天");
                        if selected_chat_group == SelectedChatGroup::All {
                            text = text.strong();
                        }
                        ui.add(Label::new(text).selectable(false));
                    }

                    // 私聊
                    {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            selected_chat_group = SelectedChatGroup::Private;
                        }
                        let mut text = RichText::new("私聊");
                        if selected_chat_group == SelectedChatGroup::Private {
                            text = text.strong();
                        }
                        // 私聊未读红点
                        if !disable_groups
                            && !disable_dot
                            && selected_chat_group != SelectedChatGroup::Private
                            && private_has_unread
                        {
                            let dot_radius = 3.0;
                            let dot_pos =
                                resp.rect.right_top() + egui::vec2(-dot_radius, dot_radius);
                            ui.painter()
                                .circle_filled(dot_pos, dot_radius, egui::Color32::RED);
                        }
                        ui.add(Label::new(text).selectable(false));
                    }

                    // 用户自定义分组
                    for (idx, group_name) in chat_groups.group_names().iter().enumerate() {
                        let btn = Button::image(img.clone());
                        let resp = ui.add(btn);
                        let is_selected = matches!(
                            &selected_chat_group,
                            SelectedChatGroup::Custom(i) if *i == idx
                        );
                        if resp.clicked() {
                            selected_chat_group = SelectedChatGroup::Custom(idx);
                        }

                        // 未读红点
                        if !disable_groups
                            && !disable_dot
                            && !is_selected
                            && group_has_unread.get(idx).copied().unwrap_or(false)
                        {
                            let dot_radius = 3.0;
                            let dot_pos =
                                resp.rect.right_top() + egui::vec2(-dot_radius, dot_radius);
                            ui.painter()
                                .circle_filled(dot_pos, dot_radius, egui::Color32::RED);
                        }

                        // 右键菜单
                        resp.context_menu(|ui| {
                            if let Some(room_id) = selected_room_id {
                                let in_group = chat_groups.is_room_in_group(idx, room_id);
                                let label = if in_group {
                                    "移出当前会话"
                                } else {
                                    "加入当前会话"
                                };
                                if ui.button(label).clicked() {
                                    chat_groups.toggle_room_in_group(idx, room_id);
                                    updated_group = Some(idx);
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

        if let Some(bridge_idx) = active_bridge_idx
            && let Some(state) = self.bridge_states.get_mut(bridge_idx)
        {
            state.chat_groups = chat_groups;
            state.selected_chat_group = selected_chat_group;
            state.invalidate_visible_room_indices();
        }
        if let Some(group_idx) = updated_group {
            self.save_chat_groups();
            if let Some(bridge_idx) = active_bridge_idx
                && let Some(group) = self.bridge_states[bridge_idx]
                    .chat_groups
                    .groups
                    .get(group_idx)
                    .cloned()
            {
                self.send_update_chat_group(
                    bridge_idx,
                    &group.name,
                    &group.rooms,
                    group.include_all_personal,
                );
            }
        }
    }

    // 聊天列表面板：把 header + rooms + 房间渲染整合为更少的函数（内部仍有一个私有房间渲染辅助）
}
