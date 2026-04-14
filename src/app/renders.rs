use super::*;
use egui::{Button, Hyperlink, Image, Label, RichText};

impl IcaApp {
    // 顶栏：将多个 menu 合并为一个“功能块”
    pub fn render_top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("顶栏").show(ctx, |ui| {
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

    // 左侧群组面板：合并“所有聊天按钮”和“群列表”渲染为一个函数
    pub fn render_left_groups_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("群聊组")
            .resizable(false)
            .exact_width(70.0)
            .show(ctx, |ui| {
                ui.label("消息栏");
                ui.label("头像占位");
                // 渲染头像
                ui.spacing_mut().item_spacing.x = 0.5;

                ui.vertical_centered(|ui| {
                    // 所有聊天按钮
                    let img = Image::new(crate::assets::svg::CHAT_GROUP)
                        .fit_to_exact_size([24.0, 24.0].into())
                        .alt_text("chat_group_icon");
                    let btn = Button::image(img.clone());
                    if ui.add(btn).clicked() {
                        self.chat_group_selected = false;
                    };
                    let mut text = RichText::new("所有聊天");
                    if !self.chat_group_selected {
                        text = text.strong();
                    }
                    let label = Label::new(text).selectable(false);
                    ui.add(label);

                    // 群组列表
                    let img = Image::new(crate::assets::svg::CHAT_GROUP)
                        .fit_to_exact_size([24.0, 24.0].into())
                        .alt_text("chat_group_icon");
                    for (idx, group) in self.chat_groups.group_names().iter().enumerate() {
                        let btn = Button::image(img.clone());
                        if ui.add(btn).clicked() {
                            self.chat_group_selected = true;
                            self.chat_group_idx = idx;
                        };
                        let mut text: egui::RichText = group.into();
                        if idx == self.chat_group_idx && self.chat_group_selected {
                            text = text.strong();
                        }
                        let label = Label::new(text).selectable(false);
                        ui.add(label);
                    }
                });
            });
    }

    // 聊天列表面板：把 header + rooms + 房间渲染整合为更少的函数（内部仍有一个私有房间渲染辅助）
    pub fn render_chat_list_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("聊天列表")
            .resizable(true)
            .width_range(300.0..=700.0)
            .show(ctx, |ui| {
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
                        self.request_room_messages(bridge_idx, room_id);
                    }
                    if ui.button("顶部").clicked() {
                        self.chat_list_scroll_target = ChatListScrollTarget::Top;
                    }
                    if ui.button("底部").clicked() {
                        self.chat_list_scroll_target = ChatListScrollTarget::Bottom;
                    }
                });
                ui.separator();

                let Some(active_bridge_idx) = self.active_bridge_idx else {
                    ui.weak("当前没有启用的 bridge");
                    return;
                };

                let room_count = self.bridge_states[active_bridge_idx].rooms.len();
                // 内容矩形顶部内边距（头像与文字一起下移）
                let content_top_padding = 4.0;
                let content_height = 50.0;
                let row_spacing = ui.spacing().item_spacing.y;
                let row_height = content_height + content_top_padding + row_spacing;
                let total_height = row_height * room_count as f32;

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

                    for idx in start..end {
                        let selected_room_id = self.bridge_states[active_bridge_idx].selected_room_id;
                        let room_id = self.bridge_states[active_bridge_idx].rooms[idx].room_id;
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

                        let bg_color = if is_selected {
                            egui::Color32::from_gray(55)
                        } else if response.hovered() {
                            egui::Color32::from_gray(45)
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(row_rect, 4.0, bg_color);

                        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                // 左侧内边距：把整行内容从分割线向右挪一点
                                ui.add_space(4.0);
                                let room = &self.bridge_states[active_bridge_idx].rooms[idx];
                                self.render_room(ui, room);
                            });
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
            });
    }

    pub fn render_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
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

                ui.horizontal(|ui| {
                    ui.heading(room_name);
                    ui.separator();
                    ui.label(format!("Bridge: {}", bridge_key));
                    ui.label(format!("Socket: {}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                    if ui.button("重新拉取历史").clicked() {
                        self.request_room_messages(active_bridge_idx, room_id);
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.heading(&bridge_key);
                    ui.separator();
                    ui.label(format!("Socket: {}", socket_state));
                    ui.label(format!("认证: {}", auth_state));
                });
            }

            if let Some(last_error) = last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, last_error);
            }

            ui.separator();

            if selected_room_id.is_none() {
                ui.label(format!("QQ: {}", online_data.qqid));
                ui.label(format!("昵称: {}", online_data.nick));
                ui.label(format!("在线: {}", if online_data.online { "是" } else { "否" }));
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

            egui::ScrollArea::vertical()
                .id_salt(("message_list", active_bridge_idx, room_id))
                .show(ui, |ui| {
                    match self.bridge_states[active_bridge_idx].messages_by_room.get(&room_id) {
                        Some(messages) if !messages.is_empty() => {
                            for message in messages {
                                self.render_message_card(ui, self_id, message);
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
                });

            ui.separator();

            let mut should_send = false;
            ui.horizontal(|ui| {
                let input_width = (ui.available_width() - 72.0).max(120.0);
                let draft = self.bridge_states[active_bridge_idx]
                    .draft_by_room
                    .entry(room_id)
                    .or_default();
                let response = ui.add_sized(
                    [input_width, 0.0],
                    egui::TextEdit::singleline(draft).hint_text("输入消息，Enter 发送"),
                );
                let enter_pressed = response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                should_send = enter_pressed
                    || ui.add_sized([64.0, 0.0], Button::new("发送")).clicked();
            });

            if should_send {
                self.send_current_message();
            }
        });
    }

    // 合并房间内的头像 / 名称行 / 预览行 为一个方法，减少外部碎片函数
    fn render_room(&self, ui: &mut egui::Ui, room: &Room) {
        // 左侧：头像区域（方形，固定大小）
        // 群聊时右下角叠加发送者头像
        // 使用 LayerId 叠加两个头像（保留原注释以便后续改进）
        let is_group = room.room_id < 0;
        let avatar_size = 48.0;
        let sender_avatar_size = 18.0;

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
                        ui.label(
                            RichText::new(timestamp)
                                .size(10.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                    }

                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let name_text = if room.room_name.is_empty() {
                            "未命名聊天"
                        } else {
                            &room.room_name
                        };
                        let mut text = RichText::new(name_text);
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
                            ui.label(
                                RichText::new(format!("{}:", username))
                                    .size(12.0)
                                    .color(egui::Color32::LIGHT_BLUE),
                            );
                        }
                        if let Some(ref content) = room.last_message.content
                            && !content.is_empty()
                        {
                            ui.label(RichText::new(content).size(12.0));
                        }
                    });
                });
            },
        );
    }

    fn render_message_card(&self, ui: &mut egui::Ui, self_id: i64, message: &crate::ica::types::message::Message) {
        let is_self = self_id > 0 && message.sender_id == self_id;
        let title_color = if message.deleted {
            egui::Color32::GRAY
        } else if message.system {
            egui::Color32::LIGHT_YELLOW
        } else if is_self {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::LIGHT_BLUE
        };

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(title_color, &message.sender_name);
                ui.weak(message.time.format("%H:%M:%S").to_string());
                if message.deleted {
                    ui.weak("已撤回");
                }
                if message.system {
                    ui.weak("系统消息");
                }
            });

            if let Some(reply) = &message.reply {
                ui.weak(format!("回复 {}: {}", reply.sender_name, reply.content));
            }

            if !message.content.is_empty() {
                ui.label(&message.content);
            } else if !message.files.is_empty() {
                ui.label(format!("[{} 个文件]", message.files.len()));
            } else {
                ui.weak("[空消息]");
            }
        });
        ui.add_space(4.0);
    }

    // 将所有窗口渲染相关的独立函数合并到一个功能块里（内部分支式处理每个窗口）
    pub fn render_windows(&mut self, ctx: &egui::Context) {
        // 定制聊天界面 (ica)
        egui::Window::new("定制聊天界面 (ica)")
            .open(&mut self.open_page.custom_chat_ica)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_ica_ui(ui);
            });

        // 定制聊天界面 (extra)
        egui::Window::new("定制聊天界面 (extra)")
            .open(&mut self.open_page.custom_chat_extra)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_extra_ui(ui);
            });

        // 在线状态
        let active_bridge_info = self.active_bridge_state().map(|state| {
            (
                state.bridge_key.clone(),
                state.online_data.qqid,
                state.online_data.nick.clone(),
                state.online_data.online,
            )
        });
        egui::Window::new("在线状态")
            .open(&mut self.open_page.online_status)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("在线状态");
                if let Some((bridge_key, qqid, nick, online)) = &active_bridge_info {
                    ui.label(format!("当前 bridge: {}", bridge_key));
                    ui.label(format!("QQ: {}", qqid));
                    ui.label(format!("昵称: {}", nick));
                    ui.label(format!("在线: {}", if *online { "是" } else { "否" }));
                    ui.separator();
                }
                ui.label("选择在线状态");
                let _ = ui.selectable_value(&mut self.online_mode, OnlineMode::Online, "在线");
                let _ = ui.selectable_value(&mut self.online_mode, OnlineMode::Left, "离开");
                let _ = ui.selectable_value(&mut self.online_mode, OnlineMode::Hidden, "隐身");
                let _ = ui.selectable_value(&mut self.online_mode, OnlineMode::Busy, "忙碌");
                let _ = ui.selectable_value(&mut self.online_mode, OnlineMode::PingMe, "Q我吧");
                let _ = ui.selectable_value(
                    &mut self.online_mode,
                    OnlineMode::DoNotDisturb,
                    "请勿打扰",
                );
            });

        let verify_message_data = self.active_bridge_state().map(|state| {
            (state.bridge_key.clone(), state.join_requests.clone())
        });

        // 验证消息
        egui::Window::new("验证消息")
            .default_size(egui::vec2(400.0, 300.0))
            .open(&mut self.open_page.verify_message)
            .show(ctx, |ui| {
                let Some((bridge_key, join_requests)) = &verify_message_data else {
                    ui.heading("验证消息");
                    ui.weak("当前没有启用的 bridge");
                    return;
                };

                ui.heading(format!("{} 的验证消息", bridge_key));
                ui.label(format!("当前共 {} 条", join_requests.len()));
                ui.separator();

                if join_requests.is_empty() {
                    ui.weak("当前没有收到新的验证消息");
                    ui.weak("后端推送 handleRequest 后会显示在这里。");
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for request in join_requests {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.strong(&request.group_name);
                                ui.separator();
                                ui.label(format!("群号 {}", request.group_id));
                            });
                            ui.label(format!("申请人: {} ({})", request.nickname, request.user_id));
                            ui.label(format!("类型: {}/{}", request.request_type, request.sub_type));
                            if !request.comment.trim().is_empty() {
                                ui.label(format!("附言: {}", request.comment));
                            }
                            if !request.tips.trim().is_empty() {
                                ui.weak(format!("提示: {}", request.tips));
                            }
                            ui.monospace(format!("flag: {}", request.flag));
                        });
                        ui.add_space(6.0);
                    }
                });
            });

        // 关于
        egui::Window::new("关于 Icalingua++ native")
            .open(&mut self.open_page.about)
            .collapsible(true)
            .show(ctx, |ui| {
                ui.heading("Icalingua++ native");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("版本：");
                    ui.monospace(crate::VERSION);
                });
                // 标题与正文之间留出一点垂直间距
                ui.add_space(6.0);
                ui.label("一个使用 Rust + egui 开发的跨平台原生 ica 客户端。");
                // 正文与“开源信息”分组之间的垂直间距
                ui.add_space(8.0);
                ui.collapsing("开源信息", |ui| {
                    ui.label("本项目基于开源许可证发布，欢迎 Star、Issue 与 PR。");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("项目地址：");
                        let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
                        ui.add(link);
                    });
                });
                // “开源信息”和“致谢”分组之间的垂直间距
                ui.add_space(8.0);
                ui.collapsing("致谢", |ui| {
                    ui.label("感谢所有贡献者与所使用的开源项目：");
                    ui.label("Icalingua 作者以及各位用户");
                    ui.label("Rust 语言与生态");
                    ui.label("egui/eframe 图形界面框架");
                    ui.label("以及社区用户的反馈与支持");
                });
            });

        // Socketio 状态
        egui::Window::new("Socketio 状态")
            .open(&mut self.open_page.socketio_status)
            .collapsible(true)
            .show(ctx, |ui| {
                ui.heading("Socketio 状态");
                if self.bridge_states.is_empty() {
                    ui.weak("当前没有启用的 bridge");
                    return;
                }

                for state in &self.bridge_states {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong(&state.bridge_key);
                            ui.separator();
                            ui.label(format!("Socket: {}", state.socket_state));
                            ui.label(format!("认证: {}", state.auth_state));
                        });
                        ui.label(format!("房间数: {}", state.rooms.len()));
                        ui.label(format!("已缓存会话: {}", state.messages_by_room.len()));
                        ui.label(format!("验证消息: {}", state.join_requests.len()));
                        ui.label(format!("QQ: {}", state.online_data.qqid));
                        if let Some(last_event) = &state.last_event {
                            ui.label(format!("最近事件: {}", last_event));
                        }
                        if let Some(last_error) = &state.last_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, last_error);
                        }
                    });
                    ui.add_space(6.0);
                }
            });

        // 配置文件编辑
        egui::Window::new("配置文件编辑")
            .open(&mut self.open_page.raw_config)
            .collapsible(true)
            .show(ctx, |ui| {
                self.config_editer.ui(ui);
            });

        // 通知等级说明（以窗口方式展示图片）
        if self.open_page.notify_level {
            // 在新页面展示一张图
            let size = ctx.content_rect();
            egui::Window::new("通知等级说明")
                .open(&mut self.open_page.notify_level)
                .collapsible(false)
                .default_size((size.width() / 2.0, size.height() / 2.0))
                .resizable(true)
                .show(ctx, |ui| {
                    ui.image(crate::assets::webp::NOTIFICATION);
                });
            // todo
            // 这里应该新开一个页面的
            // egui::Context::show_viewport_deferred(&self, new_viewport_id, viewport_builder, viewport_ui_cb);
            // ctx.show_viewport_deferred("info", viewport_builder, viewport_ui_cb);
        }
    }
}
