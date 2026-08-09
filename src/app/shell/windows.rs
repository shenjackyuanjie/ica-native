use crate::app::IcaApp;
use crate::app::online_mode::OnlineMode;
use crate::config::ReEditDraftConflictMode;
use egui::Hyperlink;

impl IcaApp {
    // 将所有窗口渲染相关的独立函数合并到一个功能块里（内部分支式处理每个窗口）
    pub fn render_windows(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let custom_chat_before = self.custom_chat.clone();
        // 定制聊天界面 (ica)
        let mut custom_chat_ica_open = self.open_page.custom_chat_ica;
        egui::Window::new("定制聊天界面 (ica)")
            .open(&mut custom_chat_ica_open)
            .resizable(false)
            .show(&ctx, |ui| {
                self.custom_chat.show_ica_ui(ui);
            });
        self.open_page.custom_chat_ica = custom_chat_ica_open;

        // 定制聊天界面 (extra)
        let mut custom_chat_extra_open = self.open_page.custom_chat_extra;
        let mut clear_on_select = self.clear_search_on_room_select;
        let mut clear_on_select_changed = false;
        let mut auto_fetch_history_on_select = self.auto_fetch_history_on_room_select;
        let mut auto_fetch_history_on_select_changed = false;
        let mut scroll_on_send = self.scroll_to_bottom_after_send;
        let mut scroll_on_send_changed = false;
        let mut reedit_mode = self.reedit_draft_conflict_mode;
        let mut reedit_mode_changed = false;
        egui::Window::new("定制聊天界面 (extra)")
            .open(&mut custom_chat_extra_open)
            .resizable(false)
            .show(&ctx, |ui| {
                let clear_on_select_before = clear_on_select;
                let auto_fetch_history_on_select_before = auto_fetch_history_on_select;
                let scroll_on_send_before = scroll_on_send;
                self.custom_chat.show_extra_ui(
                    ui,
                    &mut clear_on_select,
                    &mut auto_fetch_history_on_select,
                    &mut scroll_on_send,
                );
                clear_on_select_changed |= clear_on_select != clear_on_select_before;
                auto_fetch_history_on_select_changed |=
                    auto_fetch_history_on_select != auto_fetch_history_on_select_before;
                scroll_on_send_changed |= scroll_on_send != scroll_on_send_before;
                ui.separator();
                ui.label("重新编辑草稿冲突处理");
                ui.horizontal_wrapped(|ui| {
                    reedit_mode_changed |= ui
                        .selectable_value(
                            &mut reedit_mode,
                            ReEditDraftConflictMode::Overwrite,
                            "覆盖",
                        )
                        .changed();
                    reedit_mode_changed |= ui
                        .selectable_value(&mut reedit_mode, ReEditDraftConflictMode::Append, "追加")
                        .changed();
                    reedit_mode_changed |= ui
                        .selectable_value(
                            &mut reedit_mode,
                            ReEditDraftConflictMode::SkipIfNonEmpty,
                            "草稿非空时不执行",
                        )
                        .changed();
                });
            });
        self.open_page.custom_chat_extra = custom_chat_extra_open;
        if clear_on_select_changed {
            self.set_clear_search_on_room_select(clear_on_select);
        }
        if auto_fetch_history_on_select_changed {
            self.set_auto_fetch_history_on_room_select(auto_fetch_history_on_select);
        }
        if scroll_on_send_changed {
            self.set_scroll_to_bottom_after_send(scroll_on_send);
        }
        if reedit_mode_changed {
            self.set_reedit_draft_conflict_mode(reedit_mode);
        }
        if self.custom_chat != custom_chat_before {
            let custom_chat = self.custom_chat.clone();
            self.update_config(|cfg| {
                cfg.custom_chat = custom_chat;
            });
        }

        if let Some(active_bridge_idx) = self.active_bridge_idx {
            let picker_state = self.bridge_states.get(active_bridge_idx).and_then(|state| {
                let source_room_id = state.forward_room_id?;
                if !state.forward_target_picker_open
                    || state.forward_selected_message_ids.is_empty()
                {
                    return None;
                }
                Some((
                    source_room_id,
                    state.forward_selected_message_ids.len(),
                    state.rooms.clone(),
                ))
            });

            if let Some((source_room_id, selected_count, rooms)) = picker_state {
                let source_room_name = rooms
                    .iter()
                    .find(|room| room.room_id == source_room_id)
                    .map(|room| room.room_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| source_room_id.to_string());
                let mut picker_open =
                    self.bridge_states[active_bridge_idx].forward_target_picker_open;
                let mut target_room_ids = None;

                egui::Window::new("选择转发目标")
                    .open(&mut picker_open)
                    .default_size(egui::vec2(360.0, 420.0))
                    .show(&ctx, |ui| {
                        ui.label(format!("来源会话: {}", source_room_name));
                        ui.label(format!("已选 {} 条消息", selected_count));
                        ui.horizontal(|ui| {
                            ui.label("发送方式");
                            ui.selectable_value(
                                &mut self.bridge_states[active_bridge_idx].forward_target_as_merged,
                                true,
                                "合并转发",
                            );
                            ui.selectable_value(
                                &mut self.bridge_states[active_bridge_idx].forward_target_as_merged,
                                false,
                                "逐条转发",
                            );
                        });
                        ui.add_space(6.0);
                        ui.add_sized(
                            [ui.available_width(), 0.0],
                            egui::TextEdit::singleline(
                                &mut self.bridge_states[active_bridge_idx]
                                    .forward_target_search_query,
                            )
                            .hint_text("搜索会话名或 QQ/群号"),
                        );
                        ui.separator();

                        let query = self.bridge_states[active_bridge_idx]
                            .forward_target_search_query
                            .trim()
                            .to_uppercase();
                        let visible_room_ids = rooms
                            .iter()
                            .filter(|room| {
                                query.is_empty()
                                    || room.room_name.to_uppercase().contains(&query)
                                    || room.room_id.to_string().contains(query.as_str())
                            })
                            .map(|room| room.room_id)
                            .collect::<Vec<_>>();
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("全选当前结果").clicked() {
                                self.bridge_states[active_bridge_idx]
                                    .add_forward_targets(visible_room_ids.iter().copied());
                            }
                            if ui.button("清空目标").clicked() {
                                self.bridge_states[active_bridge_idx]
                                    .forward_target_room_ids
                                    .clear();
                            }
                            ui.weak(format!(
                                "已选 {} 个会话",
                                self.bridge_states[active_bridge_idx]
                                    .forward_target_room_ids
                                    .len()
                            ));
                        });
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for room in &rooms {
                                    if !visible_room_ids.contains(&room.room_id) {
                                        continue;
                                    }

                                    let title = if room.room_name.is_empty() {
                                        room.room_id.to_string()
                                    } else if room.room_id == source_room_id {
                                        format!("{} (当前会话)", room.room_name)
                                    } else {
                                        room.room_name.clone()
                                    };

                                    let mut selected = self.bridge_states[active_bridge_idx]
                                        .forward_target_room_ids
                                        .contains(&room.room_id);
                                    if ui.checkbox(&mut selected, title).changed() {
                                        self.bridge_states[active_bridge_idx]
                                            .set_forward_target_selected(room.room_id, selected);
                                    }
                                }
                            });
                        ui.separator();
                        ui.horizontal(|ui| {
                            let selected_count = self.bridge_states[active_bridge_idx]
                                .forward_target_room_ids
                                .len();
                            if ui
                                .add_enabled(
                                    selected_count > 0,
                                    egui::Button::new(format!("发送到 {selected_count} 个会话")),
                                )
                                .clicked()
                            {
                                target_room_ids = Some(
                                    self.bridge_states[active_bridge_idx]
                                        .forward_target_room_ids
                                        .clone(),
                                );
                            }
                            if ui.button("取消").clicked() {
                                ui.close();
                            }
                        });
                    });

                self.bridge_states[active_bridge_idx].forward_target_picker_open = picker_open;
                if let Some(target_room_ids) = target_room_ids {
                    self.forward_selected_messages_to_rooms(target_room_ids);
                }
            }
        }

        self.render_forward_viewer_window(&ctx);

        // 在线状态
        let mut online_status_open = self.open_page.online_status;
        let mut apply_online_status = false;
        let active_bridge_info = self.active_bridge_state().map(|state| {
            (
                state.bridge_key.clone(),
                state.online_data.qqid,
                state.online_data.nick.clone(),
                state.online_data.online,
            )
        });
        egui::Window::new("在线状态")
            .open(&mut online_status_open)
            .resizable(false)
            .show(&ctx, |ui| {
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
                ui.add_space(6.0);
                if ui.button("应用到 bridge").clicked() {
                    apply_online_status = true;
                }
            });
        self.open_page.online_status = online_status_open;
        if apply_online_status {
            self.apply_online_status();
        }

        let verify_message_data = self
            .active_bridge_state()
            .map(|state| (state.bridge_key.clone(), state.join_requests.clone()));
        let mut pending_request_action = None;
        let mut refresh_system_messages = false;

        // 验证消息
        egui::Window::new("验证消息")
            .default_size(egui::vec2(400.0, 300.0))
            .open(&mut self.open_page.verify_message)
            .show(&ctx, |ui| {
                let Some((bridge_key, join_requests)) = &verify_message_data else {
                    ui.heading("验证消息");
                    ui.weak("当前没有启用的 bridge");
                    return;
                };

                ui.horizontal(|ui| {
                    ui.heading(format!("{} 的验证消息", bridge_key));
                    if ui.button("刷新").clicked() {
                        refresh_system_messages = true;
                    }
                });
                ui.label(format!("当前共 {} 条", join_requests.len()));
                ui.separator();

                if join_requests.is_empty() {
                    ui.weak("当前没有收到新的验证消息");
                    ui.weak("打开窗口或点击刷新后，会主动执行 getSystemMsg。后端推送 handleRequest 后也会显示在这里。");
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for request in join_requests {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            if request.request_type == "group" {
                                ui.horizontal(|ui| {
                                    let title = if request.sub_type == "invite" {
                                        "邀请加入群"
                                    } else {
                                        "申请加入群"
                                    };
                                    ui.strong(title);
                                    if !request.group_name.is_empty() {
                                        ui.separator();
                                        ui.label(&request.group_name);
                                    }
                                    if let Some(group_id) = request.group_id {
                                        ui.label(format!("({})", group_id));
                                    }
                                });
                            } else {
                                ui.strong("好友请求");
                            }

                            ui.label(format!("申请人: {} ({})", request.nickname, request.user_id));
                            if request.request_type == "friend" && !request.source.trim().is_empty() {
                                ui.label(format!("来源: {}", request.source));
                            }
                            ui.label(format!("类型: {}/{}", request.request_type, request.sub_type));
                            if !request.comment.trim().is_empty() {
                                ui.label(format!("附言: {}", request.comment));
                            }
                            if !request.tips.trim().is_empty() {
                                ui.weak(format!("提示: {}", request.tips));
                            }
                            ui.monospace(format!("flag: {}", request.flag));
                            ui.horizontal(|ui| {
                                if ui.button("同意").clicked() {
                                    pending_request_action = Some((
                                        request.request_type.clone(),
                                        request.flag.clone(),
                                        true,
                                    ));
                                }
                                if ui.button("拒绝").clicked() {
                                    pending_request_action = Some((
                                        request.request_type.clone(),
                                        request.flag.clone(),
                                        false,
                                    ));
                                }
                            });
                        });
                        ui.add_space(6.0);
                    }
                });
            });

        if let Some((request_type, flag, accept)) = pending_request_action {
            self.handle_join_request(request_type, flag, accept);
        }

        if refresh_system_messages && let Some(active_bridge_idx) = self.active_bridge_idx {
            self.request_system_messages(active_bridge_idx);
        }

        // 关于
        egui::Window::new("关于 Icalingua++ native")
            .open(&mut self.open_page.about)
            .collapsible(true)
            .show(&ctx, |ui| {
                ui.heading("Icalingua++ native");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("版本：");
                    ui.monospace(crate::VERSION);
                });
                // 标题与正文之间留出一点垂直间距
                ui.add_space(6.0);
                ui.label("一个使用 Rust + egui 开发的跨平台原生 ica 客户端。");
                // 正文与"开源信息"分组之间的垂直间距
                ui.add_space(8.0);
                ui.collapsing("开源信息", |ui| {
                    ui.label("本项目基于开源许可证发布，欢迎 Star、Issue 与 PR。");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("项目地址：");
                        let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
                        ui.add(link);
                    });
                });
                // "开源信息"和"致谢"分组之间的垂直间距
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
        let mut socketio_status_open = self.open_page.socketio_status;
        let mut send_socket_api = false;
        egui::Window::new("Socket.IO 状态")
            .open(&mut socketio_status_open)
            .collapsible(true)
            .show(&ctx, |ui| {
                ui.heading("Socket.IO 状态");
                if self.bridge_states.is_empty() {
                    ui.weak("当前没有启用的 bridge");
                    return;
                }

                for state in &self.bridge_states {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong(&state.bridge_key);
                            ui.separator();
                            ui.label(format!("连接状态：{}", state.socket_state));
                            ui.label(format!("认证: {}", state.auth_state));
                        });
                        ui.label(format!("房间数: {}", state.rooms.len()));
                        ui.label(format!("已缓存会话: {}", state.conversations.len()));
                        ui.label(format!("验证消息: {}", state.join_requests.len()));
                        ui.label(format!("QQ: {}", state.online_data.qqid));
                        ui.label(format!(
                            "禁言: {}",
                            if state.is_shut_up { "是" } else { "否" }
                        ));
                        if let Some(last_event) = &state.last_event {
                            ui.label(format!("最近事件: {}", last_event));
                        }
                        if let Some(last_notice) = &state.last_notice {
                            ui.weak(format!("最近提示: {}", last_notice));
                        }
                        if let Some(last_error) = &state.last_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, last_error);
                        }
                        if let Some(setup_requested) = &state.setup_requested {
                            ui.collapsing("登录/初始化请求", |ui| {
                                ui.monospace(setup_requested);
                            });
                        }
                        if let Some(fatal_error) = &state.fatal_error {
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                format!("致命错误: {}", fatal_error),
                            );
                        }
                    });
                    ui.add_space(6.0);
                }

                ui.separator();
                ui.collapsing("高级 Socket API", |ui| {
                    let presets = Self::socket_api_presets();
                    let selected_label = presets
                        .get(self.socket_api_preset_idx)
                        .map(|preset| preset.label)
                        .unwrap_or("自定义");
                    let mut selected_preset = self.socket_api_preset_idx;
                    egui::ComboBox::from_label("预设")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for (idx, preset) in presets.iter().enumerate() {
                                ui.selectable_value(&mut selected_preset, idx, preset.label);
                            }
                        });
                    if selected_preset != self.socket_api_preset_idx {
                        self.apply_socket_api_preset(selected_preset);
                    }
                    if let Some(preset) = presets.get(self.socket_api_preset_idx)
                        && !preset.note.is_empty()
                    {
                        ui.weak(preset.note);
                    }
                    ui.add_sized(
                        [ui.available_width(), 0.0],
                        egui::TextEdit::singleline(&mut self.socket_api_event)
                            .hint_text("事件名，例如 getGroupMembers"),
                    );
                    ui.add_sized(
                        [ui.available_width(), 96.0],
                        egui::TextEdit::multiline(&mut self.socket_api_args)
                            .hint_text("JSON 参数数组，例如 [123456]"),
                    );
                    ui.checkbox(&mut self.socket_api_expect_ack, "等待 ack");
                    if ui.button("发送").clicked() {
                        send_socket_api = true;
                    }
                    if let Some(active) = self.active_bridge_state()
                        && let Some(response) = &active.last_socket_api_response
                    {
                        ui.separator();
                        ui.label("最近响应");
                        ui.monospace(response);
                    }
                });
            });
        self.open_page.socketio_status = socketio_status_open;
        if send_socket_api {
            self.send_socket_api_call();
        }

        self.render_group_tools_window(&ctx);
        self.render_contacts_window(&ctx);
        self.render_account_tools_window(&ctx);
        self.render_file_tools_window(&ctx);
        self.render_message_tools_window(&ctx);
        self.render_message_search_window(&ctx);
        self.render_room_tools_window(&ctx);
        self.render_auto_sign_window(&ctx);
        self.render_relation_network_window(&ctx);

        // 聊天分组编辑器
        let chat_group_editor_open = self.open_page.chat_group_editor;
        let mut chat_group_editor_is_open = chat_group_editor_open;
        let groups_before = self
            .active_bridge_state()
            .map(|state| state.chat_groups.clone())
            .unwrap_or_default();
        let mut groups_clone = groups_before.clone();
        let rooms_for_editor = self
            .active_bridge_state()
            .map(|state| state.rooms.clone())
            .unwrap_or_default();
        let mut dirty = false;
        egui::Window::new("聊天分组管理")
            .open(&mut chat_group_editor_is_open)
            .default_size(egui::vec2(420.0, 500.0))
            .min_size(egui::vec2(260.0, 180.0))
            .resizable(true)
            .collapsible(false)
            .show(&ctx, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                dirty = self
                    .chat_group_editor
                    .ui(ui, &mut groups_clone, &rooms_for_editor);
            });
        self.open_page.chat_group_editor = chat_group_editor_is_open;
        if dirty {
            if let Some(state) = self.active_bridge_state_mut() {
                state.chat_groups = groups_clone;
                state.invalidate_visible_room_indices();
            }
            if let Some(bridge_idx) = self.active_bridge_idx {
                self.sync_chat_groups_to_bridge(bridge_idx, &groups_before);
            }
        }
        // 如果删除分组导致当前选中失效
        if chat_group_editor_open && !self.open_page.chat_group_editor {
            self.ensure_selected_chat_group_valid();
        }

        // 配置文件编辑
        let config_store = self.config.clone();
        let mut raw_config_open = self.open_page.raw_config;
        egui::Window::new("配置文件编辑")
            .open(&mut raw_config_open)
            .collapsible(true)
            .show(&ctx, |ui| {
                self.config_editor.ui(ui, &config_store);
            });
        self.open_page.raw_config = raw_config_open;

        // 通知等级说明（以窗口方式展示图片）
        if self.open_page.notify_level {
            // 在新页面展示一张图
            let size = ctx.content_rect();
            egui::Window::new("通知等级说明")
                .open(&mut self.open_page.notify_level)
                .collapsible(false)
                .default_size((size.width() / 2.0, size.height() / 2.0))
                .resizable(true)
                .show(&ctx, |ui| {
                    ui.image(crate::assets::webp::NOTIFICATION);
                });
            // todo
            // 这里应该新开一个页面的
            // egui::Context::show_viewport_deferred(&self, new_viewport_id, viewport_builder, viewport_ui_cb);
            // ctx.show_viewport_deferred("info", viewport_builder, viewport_ui_cb);
        }

        self.render_image_viewer(&ctx);
    }
}
