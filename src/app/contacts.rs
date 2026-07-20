use crate::app::IcaApp;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    contact::{FriendContact, GroupContact},
    message::{At, LastMessage},
    room::Room,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ContactTab {
    #[default]
    Friends,
    Groups,
}

#[derive(Debug, Default, Clone)]
pub struct ContactDirectory {
    friends: Vec<FriendContact>,
    groups: Vec<GroupContact>,
    search_query: String,
    tab: ContactTab,
    request_id: u64,
    requested_once: bool,
    friends_loading: bool,
    groups_loading: bool,
    friends_loaded: bool,
    groups_loaded: bool,
    friends_error: Option<String>,
    groups_error: Option<String>,
}

impl ContactDirectory {
    pub(in crate::app) fn begin_refresh(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.requested_once = true;
        self.friends_loading = true;
        self.groups_loading = true;
        self.friends_error = None;
        self.groups_error = None;
        self.request_id
    }

    pub(in crate::app) fn needs_initial_load(&self) -> bool {
        !self.requested_once
    }

    pub(in crate::app) fn apply_friends(&mut self, request_id: u64, friends: Vec<FriendContact>) {
        if request_id != self.request_id {
            return;
        }
        self.friends = friends;
        self.friends_loading = false;
        self.friends_loaded = true;
        self.friends_error = None;
    }

    pub(in crate::app) fn apply_groups(&mut self, request_id: u64, groups: Vec<GroupContact>) {
        if request_id != self.request_id {
            return;
        }
        self.groups = groups;
        self.groups_loading = false;
        self.groups_loaded = true;
        self.groups_error = None;
    }

    pub(in crate::app) fn fail_part(&mut self, request_id: u64, part: &str, error: String) -> bool {
        if request_id != self.request_id {
            return false;
        }
        match part {
            "friends" => {
                self.friends_loading = false;
                self.friends_error = Some(error);
            }
            "groups" => {
                self.groups_loading = false;
                self.groups_error = Some(error);
            }
            _ => return false,
        }
        true
    }

    fn fail_all(&mut self, request_id: u64, error: String) {
        self.fail_part(request_id, "friends", error.clone());
        self.fail_part(request_id, "groups", error);
    }

    fn is_loading(&self) -> bool {
        self.friends_loading || self.groups_loading
    }

    fn error_message(&self) -> Option<String> {
        match (&self.friends_error, &self.groups_error) {
            (Some(friends), Some(groups)) if friends == groups => Some(friends.clone()),
            (Some(friends), Some(groups)) => Some(format!("好友: {friends}; 群: {groups}")),
            (Some(friends), None) => Some(format!("好友: {friends}")),
            (None, Some(groups)) => Some(format!("群: {groups}")),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ContactTarget {
    room_id: RoomId,
    room_name: String,
}

fn create_contact_room(room_id: RoomId, room_name: String, timestamp: i64) -> Room {
    let mut users = vec![
        serde_json::json!({ "_id": 1, "username": "1" }),
        serde_json::json!({ "_id": 2, "username": "2" }),
    ];
    if room_id < 0 {
        users.push(serde_json::json!({ "_id": 3, "username": "3" }));
    }

    Room {
        room_id,
        room_name,
        index: 0,
        unread_count: 0,
        priority: if room_id > 0 { 4 } else { 2 },
        utime: timestamp,
        users: serde_json::Value::Array(users),
        at: At::None,
        last_message: LastMessage {
            content: Some(String::new()),
            timestamp: Some(String::new()),
            username: None,
            user_id: None,
        },
    }
}

impl IcaApp {
    pub fn open_contacts(&mut self) {
        self.open_page.contacts = true;
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if self.bridge_states[bridge_idx].contacts.needs_initial_load() {
            self.refresh_contacts(bridge_idx);
        }
    }

    fn refresh_contacts(&mut self, bridge_idx: usize) {
        let Some(session) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let request_id = session.contacts.begin_refresh();
        if let Err(error) = session.send(IcaCommand::FetchContacts { request_id }) {
            let message = format!("联系人刷新命令发送失败: {error}");
            session.contacts.fail_all(request_id, message.clone());
            session.last_error = Some(message);
        }
    }

    fn start_contact_chat(&mut self, bridge_idx: usize, target: ContactTarget) {
        if target.room_id == 0 {
            return;
        }

        let room_exists = self.bridge_states[bridge_idx]
            .rooms
            .iter()
            .any(|room| room.room_id == target.room_id);
        if !room_exists {
            let room = create_contact_room(
                target.room_id,
                target.room_name,
                chrono::Utc::now().timestamp_millis(),
            );
            if let Err(error) =
                self.bridge_states[bridge_idx].send(IcaCommand::AddRoom(room.clone()))
            {
                self.bridge_states[bridge_idx].last_error =
                    Some(format!("创建会话命令发送失败: {error}"));
                return;
            }
            self.bridge_states[bridge_idx].rooms.insert(0, room);
            self.bridge_states[bridge_idx].bump_rooms_revision();
        }

        if self.active_bridge_idx != Some(bridge_idx) {
            self.switch_active_bridge(bridge_idx);
        }
        self.select_active_room(target.room_id);
        self.open_page.contacts = false;
    }

    pub fn render_contacts_window(&mut self, ctx: &egui::Context) {
        if !self.open_page.contacts {
            return;
        }
        let Some(bridge_idx) = self.active_bridge_idx else {
            let mut open = self.open_page.contacts;
            egui::Window::new("联系人").open(&mut open).show(ctx, |ui| {
                ui.weak("当前没有启用的 bridge");
            });
            self.open_page.contacts = open;
            return;
        };

        if self.bridge_states[bridge_idx].contacts.needs_initial_load() {
            self.refresh_contacts(bridge_idx);
        }

        let bridge_key = self.bridge_states[bridge_idx].bridge_key.clone();
        let mut open = self.open_page.contacts;
        let mut refresh_requested = false;
        let mut selected_target = None;

        egui::Window::new(format!("联系人 - {bridge_key}"))
            .open(&mut open)
            .default_size(egui::vec2(440.0, 620.0))
            .min_size(egui::vec2(320.0, 360.0))
            .resizable(true)
            .show(ctx, |ui| {
                let directory = &mut self.bridge_states[bridge_idx].contacts;

                ui.horizontal(|ui| {
                    ui.add_sized(
                        [ui.available_width() - 72.0, 0.0],
                        egui::TextEdit::singleline(&mut directory.search_query)
                            .hint_text("搜索昵称、备注或 QQ/群号"),
                    );
                    if ui
                        .add_enabled(!directory.is_loading(), egui::Button::new("刷新"))
                        .clicked()
                    {
                        refresh_requested = true;
                    }
                    if directory.is_loading() {
                        ui.spinner();
                    }
                });

                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut directory.tab,
                        ContactTab::Friends,
                        format!("好友 ({})", directory.friends.len()),
                    );
                    ui.selectable_value(
                        &mut directory.tab,
                        ContactTab::Groups,
                        format!("群 ({})", directory.groups.len()),
                    );
                });

                if let Some(error) = directory.error_message() {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.separator();

                let query = directory.search_query.trim().to_uppercase();
                match directory.tab {
                    ContactTab::Friends => {
                        let filtered = directory
                            .friends
                            .iter()
                            .enumerate()
                            .filter_map(|(index, friend)| {
                                friend.matches_query(&query).then_some(index)
                            })
                            .collect::<Vec<_>>();
                        if filtered.is_empty() {
                            let message = if directory.friends_loading {
                                "正在加载好友..."
                            } else if directory.friends_loaded {
                                "没有匹配的好友"
                            } else {
                                "好友列表尚未加载"
                            };
                            ui.weak(message);
                        } else {
                            let row_height = 54.0 + ui.spacing().item_spacing.y;
                            egui::ScrollArea::vertical().show_rows(
                                ui,
                                row_height,
                                filtered.len(),
                                |ui, rows| {
                                    for row in rows {
                                        let friend = &directory.friends[filtered[row]];
                                        let display_name = friend.display_name();
                                        let secondary = if friend.nick.trim().is_empty()
                                            || friend.nick.trim() == display_name
                                        {
                                            format!("QQ {}", friend.uin.abs())
                                        } else {
                                            format!(
                                                "{} · QQ {}",
                                                friend.nick.trim(),
                                                friend.uin.abs()
                                            )
                                        };
                                        let response = Self::render_contact_row(
                                            ui,
                                            &display_name,
                                            &secondary,
                                            &friend.avatar_url(),
                                        );
                                        Self::contact_context_menu(
                                            &response,
                                            &display_name,
                                            friend.uin,
                                            &friend.avatar_url(),
                                        );
                                        if response.clicked() {
                                            selected_target = Some(ContactTarget {
                                                room_id: friend.room_id(),
                                                room_name: display_name,
                                            });
                                        }
                                    }
                                },
                            );
                        }
                    }
                    ContactTab::Groups => {
                        let filtered = directory
                            .groups
                            .iter()
                            .enumerate()
                            .filter_map(|(index, group)| {
                                group.matches_query(&query).then_some(index)
                            })
                            .collect::<Vec<_>>();
                        if filtered.is_empty() {
                            let message = if directory.groups_loading {
                                "正在加载群..."
                            } else if directory.groups_loaded {
                                "没有匹配的群"
                            } else {
                                "群列表尚未加载"
                            };
                            ui.weak(message);
                        } else {
                            let row_height = 54.0 + ui.spacing().item_spacing.y;
                            egui::ScrollArea::vertical().show_rows(
                                ui,
                                row_height,
                                filtered.len(),
                                |ui, rows| {
                                    for row in rows {
                                        let group = &directory.groups[filtered[row]];
                                        let display_name = group.display_name();
                                        let secondary = if group.group_name.trim().is_empty()
                                            || group.group_name.trim() == display_name
                                        {
                                            format!("群 {}", group.group_id.abs())
                                        } else {
                                            format!(
                                                "{} · 群 {}",
                                                group.group_name.trim(),
                                                group.group_id.abs()
                                            )
                                        };
                                        let response = Self::render_contact_row(
                                            ui,
                                            &display_name,
                                            &secondary,
                                            &group.avatar_url(),
                                        );
                                        Self::contact_context_menu(
                                            &response,
                                            &display_name,
                                            group.group_id,
                                            &group.avatar_url(),
                                        );
                                        if response.clicked() {
                                            selected_target = Some(ContactTarget {
                                                room_id: group.room_id(),
                                                room_name: group.room_name(),
                                            });
                                        }
                                    }
                                },
                            );
                        }
                    }
                }
            });

        self.open_page.contacts = open;
        if refresh_requested {
            self.refresh_contacts(bridge_idx);
        }
        if let Some(target) = selected_target {
            self.start_contact_chat(bridge_idx, target);
        }
    }

    fn render_contact_row(
        ui: &mut egui::Ui,
        title: &str,
        secondary: &str,
        avatar_url: &str,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
        let background = if response.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 4.0, background);

        let avatar_rect =
            egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 5.0), egui::vec2(44.0, 44.0));
        egui::Image::from_uri(avatar_url.to_string())
            .fit_to_exact_size(avatar_rect.size())
            .corner_radius(6.0)
            .paint_at(ui, avatar_rect);

        let text_left = avatar_rect.right() + 10.0;
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.top()),
            egui::pos2(rect.right() - 6.0, rect.bottom()),
        );
        let painter = ui.painter().with_clip_rect(text_rect);
        let title_galley = painter.layout_no_wrap(
            title.to_string(),
            egui::FontId::proportional(16.0),
            ui.visuals().text_color(),
        );
        painter.galley(
            egui::pos2(text_left, rect.top() + 7.0),
            title_galley,
            ui.visuals().text_color(),
        );
        let secondary_color = ui.visuals().weak_text_color();
        let secondary_galley = painter.layout_no_wrap(
            secondary.to_string(),
            egui::FontId::proportional(12.0),
            secondary_color,
        );
        painter.galley(
            egui::pos2(text_left, rect.top() + 31.0),
            secondary_galley,
            secondary_color,
        );

        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn contact_context_menu(
        response: &egui::Response,
        display_name: &str,
        id: i64,
        avatar_url: &str,
    ) {
        response.context_menu(|ui| {
            if ui.button("复制名称").clicked() {
                ui.ctx().copy_text(display_name.to_string());
                ui.close();
            }
            if ui.button("复制 ID").clicked() {
                ui.ctx().copy_text(id.abs().to_string());
                ui.close();
            }
            if ui.button("复制头像 URL").clicked() {
                ui.ctx().copy_text(avatar_url.to_string());
                ui.close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::ica::types::contact::FriendContact;

    use super::{ContactDirectory, create_contact_room};

    #[test]
    fn stale_contact_responses_do_not_replace_newer_refresh() {
        let mut contacts = ContactDirectory::default();
        let old_request = contacts.begin_refresh();
        let current_request = contacts.begin_refresh();
        contacts.apply_friends(
            old_request,
            vec![FriendContact {
                uin: 1,
                nick: "old".to_string(),
                remark: String::new(),
            }],
        );
        assert!(contacts.friends.is_empty());

        contacts.apply_friends(
            current_request,
            vec![FriendContact {
                uin: 2,
                nick: "new".to_string(),
                remark: String::new(),
            }],
        );
        assert_eq!(contacts.friends[0].uin, 2);
    }

    #[test]
    fn contact_rooms_match_icalingua_priorities_and_legacy_users() {
        let friend = create_contact_room(10001, "好友".to_string(), 123);
        let group = create_contact_room(-20002, "群".to_string(), 456);

        assert_eq!(friend.priority, 4);
        assert_eq!(friend.users.as_array().unwrap().len(), 2);
        assert_eq!(group.priority, 2);
        assert_eq!(group.users.as_array().unwrap().len(), 3);
    }
}
