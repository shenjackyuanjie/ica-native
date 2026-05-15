use egui::TextEdit;

use crate::app::RoomId;
use crate::ica::types::room::Room;

#[derive(Default)]
pub struct ChatGroupEditor {
    new_group_name: String,
    editing_index: Option<usize>,
    editing_name: String,
    error_msg: Option<String>,
}

impl ChatGroupEditor {
    /// 返回 true 表示数据已修改，需要保存
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        chat_groups: &mut super::chat_groups::ChatGroups,
        rooms: &[Room],
    ) -> bool {
        let mut dirty = false;

        ui.heading("聊天分组管理");
        ui.separator();

        if let Some(msg) = &self.error_msg {
            ui.colored_label(egui::Color32::LIGHT_RED, msg.as_str());
            ui.separator();
        }

        // 新建分组
        ui.horizontal(|ui| {
            ui.label("新建分组:");
            ui.add(
                TextEdit::singleline(&mut self.new_group_name)
                    .hint_text("分组名称（最多10个字符）")
                    .desired_width(160.0),
            );
            if ui.button("创建").clicked() {
                let name = self.new_group_name.trim().to_string();
                if name.is_empty() {
                    self.error_msg = Some("分组名称不能为空".to_string());
                } else if name.len() > 10 {
                    self.error_msg = Some("分组名称最多10个字符".to_string());
                } else if name == "所有聊天" || name == "私聊" {
                    self.error_msg = Some("不能使用内置分组名称".to_string());
                } else if chat_groups.groups.iter().any(|g| g.name == name) {
                    self.error_msg = Some("分组名称已存在".to_string());
                } else {
                    chat_groups.add_group(super::chat_groups::ChatGroup::new_empty(name));
                    self.new_group_name.clear();
                    self.error_msg = None;
                    dirty = true;
                }
            }
        });

        ui.separator();

        if chat_groups.groups.is_empty() {
            ui.weak("暂无自定义分组，请在上方创建。");
            return dirty;
        }

        egui::ScrollArea::vertical()
            .max_height(400.0)
            .show(ui, |ui| {
                let mut remove_idx: Option<usize> = None;
                let mut move_up: Option<usize> = None;
                let mut move_down: Option<usize> = None;

                let group_count = chat_groups.groups.len();
                for idx in 0..group_count {
                    let group_name = chat_groups.groups[idx].name.clone();
                    let group_room_count = chat_groups.groups[idx].rooms.len();
                    let group_include_all = chat_groups.groups[idx].include_all_personal;
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if self.editing_index == Some(idx) {
                                ui.add(
                                    TextEdit::singleline(&mut self.editing_name)
                                        .desired_width(120.0),
                                );
                                if ui.button("ok").clicked() {
                                    let new_name = self.editing_name.trim().to_string();
                                    if new_name.is_empty() {
                                        self.error_msg = Some("分组名称不能为空".to_string());
                                    } else if new_name.len() > 10 {
                                        self.error_msg = Some("分组名称最多10个字符".to_string());
                                    } else if new_name != group_name
                                        && chat_groups.groups.iter().any(|g| g.name == new_name)
                                    {
                                        self.error_msg = Some("分组名称已存在".to_string());
                                    } else {
                                        chat_groups.rename_group(idx, new_name);
                                        self.editing_index = None;
                                        self.editing_name.clear();
                                        self.error_msg = None;
                                        dirty = true;
                                    }
                                }
                                if ui.button("cancel").clicked() {
                                    self.editing_index = None;
                                    self.editing_name.clear();
                                }
                            } else {
                                ui.strong(&group_name);
                                if ui.button("rename").on_hover_text("重命名").clicked() {
                                    self.editing_index = Some(idx);
                                    self.editing_name = group_name.clone();
                                }
                            }

                            ui.separator();

                            if idx > 0 && ui.button("up").on_hover_text("上移").clicked() {
                                move_up = Some(idx);
                            }
                            if idx + 1 < group_count
                                && ui.button("down").on_hover_text("下移").clicked()
                            {
                                move_down = Some(idx);
                            }

                            let mut include_all = group_include_all;
                            if ui.checkbox(&mut include_all, "包含所有私聊").changed() {
                                chat_groups.groups[idx].include_all_personal = include_all;
                                dirty = true;
                            }

                            if ui.button("del").on_hover_text("删除分组").clicked() {
                                remove_idx = Some(idx);
                            }
                        });

                        ui.collapsing(format!("会话 ({} 个)", group_room_count), |ui| {
                            if self.render_room_list(ui, rooms, idx, chat_groups) {
                                dirty = true;
                            }
                        });
                    });
                }

                if let Some(idx) = remove_idx {
                    chat_groups.remove_group(idx);
                    if self.editing_index == Some(idx) {
                        self.editing_index = None;
                        self.editing_name.clear();
                    }
                    dirty = true;
                }
                if let Some(idx) = move_up {
                    chat_groups.move_group(idx, idx.saturating_sub(1));
                    dirty = true;
                }
                if let Some(idx) = move_down {
                    chat_groups.move_group(idx, (idx + 1).min(chat_groups.groups.len() - 1));
                    dirty = true;
                }
            });

        dirty
    }

    fn render_room_list(
        &mut self,
        ui: &mut egui::Ui,
        rooms: &[Room],
        group_idx: usize,
        chat_groups: &mut super::chat_groups::ChatGroups,
    ) -> bool {
        let mut dirty = false;
        let group_room_ids = &chat_groups.groups[group_idx].rooms;
        if group_room_ids.is_empty() {
            ui.weak("暂无会话，从下方添加。");
        } else {
            let mut remove_room: Option<RoomId> = None;
            for &room_id in group_room_ids {
                let room_name = rooms
                    .iter()
                    .find(|r| r.room_id == room_id)
                    .map(|r| {
                        if r.room_name.is_empty() {
                            r.room_id.to_string()
                        } else {
                            r.room_name.clone()
                        }
                    })
                    .unwrap_or_else(|| room_id.to_string());
                let group_type = if room_id < 0 { "群聊" } else { "私聊" };
                ui.horizontal(|ui| {
                    ui.label(format!("[{}] {}", group_type, room_name));
                    if ui.button("X").clicked() {
                        remove_room = Some(room_id);
                    }
                });
            }
            if let Some(room_id) = remove_room {
                chat_groups.toggle_room_in_group(group_idx, room_id);
                dirty = true;
            }
        }

        ui.separator();

        if self.render_add_room_panel(ui, rooms, group_idx, chat_groups) {
            dirty = true;
        }

        dirty
    }

    fn render_add_room_panel(
        &mut self,
        ui: &mut egui::Ui,
        rooms: &[Room],
        group_idx: usize,
        chat_groups: &mut super::chat_groups::ChatGroups,
    ) -> bool {
        let mut dirty = false;
        let group = &chat_groups.groups[group_idx];
        let available: Vec<_> = rooms
            .iter()
            .filter(|r| !group.rooms.contains(&r.room_id))
            .collect();

        if available.is_empty() {
            ui.weak("所有会话都已加入此分组");
            return false;
        }

        ui.collapsing("添加会话", |ui| {
            let mut search = String::new();
            let salt = format!("add_room_search_{}", group_idx);
            ui.add(
                TextEdit::singleline(&mut search)
                    .hint_text("搜索会话名或ID")
                    .desired_width(200.0)
                    .id(ui.make_persistent_id(&salt)),
            );
            let query = search.trim().to_uppercase();
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for room in &available {
                        if !query.is_empty()
                            && !room.room_name.to_uppercase().contains(&query)
                            && !room.room_id.to_string().contains(&query)
                        {
                            continue;
                        }
                        let room_name = if room.room_name.is_empty() {
                            room.room_id.to_string()
                        } else {
                            room.room_name.clone()
                        };
                        let group_type = if room.room_id < 0 { "群聊" } else { "私聊" };
                        if ui
                            .button(format!("[{}] {}", group_type, room_name))
                            .clicked()
                        {
                            chat_groups.toggle_room_in_group(group_idx, room.room_id);
                            dirty = true;
                        }
                    }
                });
        });

        dirty
    }
}
