//! 联系人窗口：好友与群列表的加载、搜索与会话发起。
//!
//! 数据来自 Bridge 的 `FetchContacts` 命令，按 `request_id` 归属写入
//! `ContactDirectory`；本模块负责把目录渲染到独立 viewport，并处理“点联系人开聊”。
//! 测试位于 `tests/contacts.rs`，经 `#[path]` 挂到本模块下以便访问私有项。

#[cfg(test)]
#[path = "tests/contacts.rs"]
mod tests;

use crate::app::IcaApp;
use crate::ica::IcaCommand;
use crate::ica::types::{
    RoomId,
    contact::{FriendContact, GroupContact},
    message::{At, LastMessage},
    room::Room,
};
use std::sync::{Arc, Mutex, atomic::Ordering};

/// 联系人窗口顶部的好友/群分页。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ContactTab {
    /// 好友列表（默认页）。
    #[default]
    Friends,
    /// 群列表。
    Groups,
}

/// 单个 Bridge 连接的联系人目录状态。
///
/// 好友与群分两批异步加载，用 `request_id` 归属响应；`pending_*` 字段是界面与后台
/// 事件之间传递意图的桥梁（点击刷新/开聊在渲染期记录，主循环再执行）。
#[derive(Debug, Default, Clone)]
pub struct ContactDirectory {
    /// 好友列表。
    friends: Vec<FriendContact>,
    /// 群列表。
    groups: Vec<GroupContact>,
    /// 搜索关键字（匹配昵称/备注/号码）。
    search_query: String,
    /// 当前展示的分页。
    tab: ContactTab,
    /// 最近一次刷新请求序号，用于丢弃过期响应。
    request_id: u64,
    /// 是否已发起过至少一次加载。
    requested_once: bool,
    /// 好友是否正在加载。
    friends_loading: bool,
    /// 群是否正在加载。
    groups_loading: bool,
    /// 好友是否已成功加载过。
    friends_loaded: bool,
    /// 群是否已成功加载过。
    groups_loaded: bool,
    /// 好友加载错误信息。
    friends_error: Option<String>,
    /// 群加载错误信息。
    groups_error: Option<String>,
    /// 是否有待处理的刷新请求。
    pending_refresh: bool,
    /// 有待发起会话的联系人目标。
    pending_target: Option<ContactTarget>,
}

/// 联系人目录的加载状态机与响应归属逻辑。
impl ContactDirectory {
    /// 开始一次新的刷新：递增并返回请求序号，重置加载/错误状态。
    ///
    /// 返回的 `request_id` 需随命令下发，后续响应只接受序号匹配者。
    pub fn begin_refresh(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.requested_once = true;
        self.friends_loading = true;
        self.groups_loading = true;
        self.friends_error = None;
        self.groups_error = None;
        self.request_id
    }

    /// 是否从未加载过（用于首次打开时自动拉取）。
    pub fn needs_initial_load(&self) -> bool {
        !self.requested_once
    }

    /// 写入好友响应；请求序号过期时忽略。
    pub fn apply_friends(&mut self, request_id: u64, friends: Vec<FriendContact>) {
        if request_id != self.request_id {
            return;
        }
        self.friends = friends;
        self.friends_loading = false;
        self.friends_loaded = true;
        self.friends_error = None;
    }

    /// 写入群响应；请求序号过期时忽略。
    pub fn apply_groups(&mut self, request_id: u64, groups: Vec<GroupContact>) {
        if request_id != self.request_id {
            return;
        }
        self.groups = groups;
        self.groups_loading = false;
        self.groups_loaded = true;
        self.groups_error = None;
    }

    /// 标记某一分页（friends/groups）加载失败；序号过期或分页名未知时返回 `false`。
    pub fn fail_part(&mut self, request_id: u64, part: &str, error: String) -> bool {
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

    /// 把同一错误同时应用到好友与群两个分页（如命令根本发不出去时）。
    fn fail_all(&mut self, request_id: u64, error: String) {
        self.fail_part(request_id, "friends", error.clone());
        self.fail_part(request_id, "groups", error);
    }

    /// 任一/两个分页仍在加载中。
    fn is_loading(&self) -> bool {
        self.friends_loading || self.groups_loading
    }

    /// 合并两个分页的错误信息用于展示：都错且相同则合一条，否则分别标注来源。
    fn error_message(&self) -> Option<String> {
        match (&self.friends_error, &self.groups_error) {
            (Some(friends), Some(groups)) if friends == groups => Some(friends.clone()),
            (Some(friends), Some(groups)) => Some(format!("好友: {friends}; 群: {groups}")),
            (Some(friends), None) => Some(format!("好友: {friends}")),
            (None, Some(groups)) => Some(format!("群: {groups}")),
            (None, None) => None,
        }
    }

    /// 取走并清空待处理的刷新/开聊意图，返回 `(是否刷新, 目标)`。
    fn take_pending_action(&mut self) -> (bool, Option<ContactTarget>) {
        (
            std::mem::take(&mut self.pending_refresh),
            self.pending_target.take(),
        )
    }
}

/// 点击联系人后要发起会话的目标（房间 ID 与展示名）。
#[derive(Debug, Clone)]
pub struct ContactTarget {
    /// 目标房间 ID（正数为好友/私聊，负数为群）。
    pub room_id: RoomId,
    /// 目标房间展示名。
    pub room_name: String,
}

/// 为尚不存在的联系人凭空构造一个本地房间占位，便于立即打开会话。
///
/// 好友/群各给一组最小化 `users` 占位；`priority`、`at` 等取中性默认值，真实数据
/// 由 Bridge 随后的 `updateRoom`/`setAllRooms` 覆盖。
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

/// 联系人窗口的打开、数据刷新、发起会话与渲染逻辑。
impl IcaApp {
    /// 打开联系人窗口；若当前 bridge 从未加载过则立即触发一次拉取。
    pub fn open_contacts(&mut self) {
        self.open_page.contacts = true;
        self.contacts_viewport_closed
            .store(false, Ordering::Relaxed);
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if self.bridge_states[bridge_idx]
            .contacts
            .lock()
            .unwrap()
            .needs_initial_load()
        {
            self.refresh_contacts(bridge_idx);
        }
    }

    /// 向指定 bridge 发送 `FetchContacts`；命令发送失败时直接把错误写回目录。
    fn refresh_contacts(&mut self, bridge_idx: usize) {
        let Some(session) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };
        let request_id = session.contacts.lock().unwrap().begin_refresh();
        if let Err(error) = session.send(IcaCommand::FetchContacts { request_id }) {
            let message = format!("联系人刷新命令发送失败: {error}");
            session
                .contacts
                .lock()
                .unwrap()
                .fail_all(request_id, message.clone());
            session.last_error = Some(message);
        }
    }

    /// 与某联系人开聊：房间不存在则先本地占位并 `AddRoom`，再切到该 bridge 并选中。
    pub fn start_contact_chat(&mut self, bridge_idx: usize, target: ContactTarget) {
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
    }

    /// 渲染联系人窗口（deferred viewport），并处理跨视口的关闭与意图回传。
    ///
    /// 无启用 bridge 时显示提示占位；有待处理的刷新或开聊意图时在主循环执行，
    /// 并通过 `request_repaint_of` 让父视口及时反映状态变化。
    pub fn render_contacts_window(&mut self, ctx: &egui::Context) {
        if self.contacts_viewport_closed.swap(false, Ordering::Relaxed) {
            self.open_page.contacts = false;
        }
        if !self.open_page.contacts {
            return;
        }
        let parent_viewport_id = ctx.viewport_id();
        let closed = self.contacts_viewport_closed.clone();
        let Some(bridge_idx) = self.active_bridge_idx else {
            let closed = closed.clone();
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("contacts"),
                egui::ViewportBuilder::default()
                    .with_title("联系人")
                    .with_inner_size([440.0, 620.0])
                    .with_min_inner_size([320.0, 360.0]),
                move |viewport_ctx, _class| {
                    if viewport_ctx.input(|input| input.viewport().close_requested()) {
                        closed.store(true, Ordering::Relaxed);
                        viewport_ctx.request_repaint_of(parent_viewport_id);
                        return;
                    }
                    egui::CentralPanel::default().show(viewport_ctx, |ui| {
                        ui.weak("当前没有启用的 bridge");
                    });
                },
            );
            return;
        };

        let directory = self.bridge_states[bridge_idx].contacts.clone();
        let (refresh_requested, selected_target, needs_initial_load) = {
            let mut directory = directory.lock().unwrap();
            let (refresh_requested, selected_target) = directory.take_pending_action();
            (
                refresh_requested,
                selected_target,
                directory.needs_initial_load(),
            )
        };
        if needs_initial_load || refresh_requested {
            self.refresh_contacts(bridge_idx);
        }
        if let Some(target) = selected_target {
            self.start_contact_chat(bridge_idx, target);
        }

        let bridge_key = self.bridge_states[bridge_idx].bridge_key.clone();
        let viewport_id = egui::ViewportId::from_hash_of(("contacts", &bridge_key));
        ctx.show_viewport_deferred(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title(format!("联系人 - {bridge_key}"))
                .with_inner_size([440.0, 620.0])
                .with_min_inner_size([320.0, 360.0]),
            move |viewport_ctx, _class| {
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    closed.store(true, Ordering::Relaxed);
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                    return;
                }
                egui::CentralPanel::default().show(viewport_ctx, |ui| {
                    Self::render_contact_directory(ui, &directory);
                });
                if directory.lock().unwrap().pending_refresh
                    || directory.lock().unwrap().pending_target.is_some()
                {
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                }
            },
        );
    }

    /// 绘制联系人目录主体：搜索框、刷新按钮、好友/群分页与结果列表。
    ///
    /// 传入共享的 `Arc<Mutex<..>>`，因为渲染闭包可能在 deferred viewport 中运行，
    /// 需要与主线程共享同一份目录状态。
    fn render_contact_directory(ui: &mut egui::Ui, directory: &Arc<Mutex<ContactDirectory>>) {
        let mut directory = directory.lock().unwrap();

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
                directory.pending_refresh = true;
            }
            if directory.is_loading() {
                ui.spinner();
            }
        });

        let friend_count = directory.friends.len();
        let group_count = directory.groups.len();
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut directory.tab,
                ContactTab::Friends,
                format!("好友 ({friend_count})"),
            );
            ui.selectable_value(
                &mut directory.tab,
                ContactTab::Groups,
                format!("群 ({group_count})"),
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
                    .filter_map(|(index, friend)| friend.matches_query(&query).then_some(index))
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
                                    format!("{} · QQ {}", friend.nick.trim(), friend.uin.abs())
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
                                    directory.pending_target = Some(ContactTarget {
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
                    .filter_map(|(index, group)| group.matches_query(&query).then_some(index))
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
                                    directory.pending_target = Some(ContactTarget {
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
    }

    /// 绘制单个联系人行（头像 + 主标题 + 副标题），返回其交互 `Response`。
    ///
    /// 手工布局而非用 `ui.horizontal`，以便对头像、两行文字做精确定位与裁切。
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

    /// 为联系人行挂右键菜单：复制名称、ID 或头像 URL。
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
