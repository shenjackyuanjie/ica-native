use crate::app::{IcaApp, SelectedChatGroup};
use egui::{Button, Image, Label, RichText};

impl IcaApp {
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

                let (private_has_unread, group_has_unread) = self
                    .active_bridge_state()
                    .map(|state| {
                        (
                            state
                                .rooms
                                .iter()
                                .any(|room| room.room_id > 0 && room.unread_count > 0),
                            (0..self.chat_groups.groups.len())
                                .map(|idx| self.chat_groups.has_unread_in_group(idx, &state.rooms))
                                .collect::<Vec<_>>(),
                        )
                    })
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
                            if private_has_unread {
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
}
