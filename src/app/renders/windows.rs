use crate::app::IcaApp;
use crate::app::SelectedChatGroup;
use crate::app::online_mode::OnlineMode;
use crate::cfg::{self, ReEditDraftConflictMode};
use egui::Hyperlink;
use std::sync::atomic::Ordering;

impl IcaApp {
    // 将所有窗口渲染相关的独立函数合并到一个功能块里（内部分支式处理每个窗口）
    pub fn render_windows(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let custom_chat_before = self.custom_chat.clone();
        // 定制聊天界面 (ica)
        egui::Window::new("定制聊天界面 (ica)")
            .open(&mut self.open_page.custom_chat_ica)
            .resizable(false)
            .show(&ctx, |ui| {
                self.custom_chat.show_ica_ui(ui);
            });

        // 定制聊天界面 (extra)
        let mut custom_chat_extra_open = self.open_page.custom_chat_extra;
        let mut clear_on_select = self.clear_search_on_room_select;
        let mut clear_on_select_changed = false;
        let mut scroll_on_send = self.scroll_to_bottom_after_send;
        let mut scroll_on_send_changed = false;
        let mut reedit_mode = self.reedit_draft_conflict_mode;
        let mut reedit_mode_changed = false;
        egui::Window::new("定制聊天界面 (extra)")
            .open(&mut custom_chat_extra_open)
            .resizable(false)
            .show(&ctx, |ui| {
                let clear_on_select_before = clear_on_select;
                let scroll_on_send_before = scroll_on_send;
                self.custom_chat
                    .show_extra_ui(ui, &mut clear_on_select, &mut scroll_on_send);
                clear_on_select_changed |= clear_on_select != clear_on_select_before;
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
        if scroll_on_send_changed {
            self.set_scroll_to_bottom_after_send(scroll_on_send);
        }
        if reedit_mode_changed {
            self.set_reedit_draft_conflict_mode(reedit_mode);
        }
        if self.custom_chat != custom_chat_before {
            let custom_chat = self.custom_chat.clone();
            cfg::update_and_save_cfg(|cfg| {
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
                let mut target_room_id = None;

                egui::Window::new("选择转发目标")
                    .open(&mut picker_open)
                    .default_size(egui::vec2(360.0, 420.0))
                    .show(&ctx, |ui| {
                        ui.label(format!("来源会话: {}", source_room_name));
                        ui.label(format!("已选 {} 条消息", selected_count));
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
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for room in &rooms {
                                    if !query.is_empty()
                                        && !room.room_name.to_uppercase().contains(&query)
                                        && !room.room_id.to_string().contains(query.as_str())
                                    {
                                        continue;
                                    }

                                    let title = if room.room_name.is_empty() {
                                        room.room_id.to_string()
                                    } else if room.room_id == source_room_id {
                                        format!("{} (当前会话)", room.room_name)
                                    } else {
                                        room.room_name.clone()
                                    };

                                    if ui.button(title).clicked() {
                                        target_room_id = Some(room.room_id);
                                    }
                                }
                            });
                        ui.separator();
                        if ui.button("取消").clicked() {
                            ui.close();
                        }
                    });

                self.bridge_states[active_bridge_idx].forward_target_picker_open = picker_open;
                if let Some(target_room_id) = target_room_id {
                    self.forward_selected_messages_to_room(target_room_id);
                }
            }
        }

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
            });

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
        egui::Window::new("Socketio 状态")
            .open(&mut self.open_page.socketio_status)
            .collapsible(true)
            .show(&ctx, |ui| {
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

        // 聊天分组编辑器
        let chat_group_editor_open = self.open_page.chat_group_editor;
        let groups_before = self.chat_groups.clone();
        let mut groups_clone = self.chat_groups.clone();
        let rooms_for_editor = self
            .active_bridge_state()
            .map(|state| state.rooms.clone())
            .unwrap_or_default();
        let mut dirty = false;
        egui::Window::new("聊天分组管理")
            .open(&mut self.open_page.chat_group_editor)
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
        if dirty {
            self.chat_groups = groups_clone;
            self.save_chat_groups();
            if let Some(bridge_idx) = self.active_bridge_idx {
                self.sync_chat_groups_to_bridge(bridge_idx, &groups_before);
            }
        }
        // 如果删除分组导致当前选中失效
        if chat_group_editor_open && !self.open_page.chat_group_editor {
            if let SelectedChatGroup::Custom(idx) = &self.selected_chat_group {
                if *idx >= self.chat_groups.groups.len() {
                    self.selected_chat_group = SelectedChatGroup::All;
                }
            }
        }

        // 配置文件编辑
        egui::Window::new("配置文件编辑")
            .open(&mut self.open_page.raw_config)
            .collapsible(true)
            .show(&ctx, |ui| {
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
                .show(&ctx, |ui| {
                    ui.image(crate::assets::webp::NOTIFICATION);
                });
            // todo
            // 这里应该新开一个页面的
            // egui::Context::show_viewport_deferred(&self, new_viewport_id, viewport_builder, viewport_ui_cb);
            // ctx.show_viewport_deferred("info", viewport_builder, viewport_ui_cb);
        }

        // 图片查看器关闭信号检测
        if let Some(ref viewer) = self.image_viewer {
            if viewer.lock().unwrap().closed.load(Ordering::Relaxed) {
                self.image_viewer = None;
            }
        }

        // 图片查看器（独立系统窗口，带工具栏和缩放）
        if let Some(viewer_state) = self.image_viewer.clone() {
            let viewport_id = egui::ViewportId::from_hash_of("image_preview");
            let viewport_builder = egui::ViewportBuilder::default()
                .with_title("图片预览")
                .with_inner_size([800.0, 600.0]);
            ctx.show_viewport_deferred(viewport_id, viewport_builder, move |ui, _class| {
                // 检测窗口关闭请求
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    viewer_state
                        .lock()
                        .unwrap()
                        .closed
                        .store(true, Ordering::Relaxed);
                    return;
                }

                let url = viewer_state.lock().unwrap().url.clone();

                // 顶部工具栏
                egui::Panel::top("image_viewer_toolbar").show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        // 适应窗口
                        if ui.button("⊡ 适应窗口").clicked() {
                            viewer_state.lock().unwrap().fit_to_window();
                        }
                        // 1:1 原始尺寸
                        if ui.button("1:1 原始").clicked() {
                            viewer_state.lock().unwrap().request_original_size = true;
                        }
                        ui.separator();
                        // 缩小
                        if ui.button("🔍− 缩小").clicked() {
                            viewer_state.lock().unwrap().zoom_out();
                        }
                        // 缩放百分比显示
                        let zoom_text = viewer_state.lock().unwrap().zoom_percent_text();
                        ui.monospace(&zoom_text);
                        // 放大
                        if ui.button("🔍+ 放大").clicked() {
                            viewer_state.lock().unwrap().zoom_in();
                        }
                        ui.separator();
                        // 下载保存
                        if ui.button("💾 保存").clicked() {
                            // 通过 egui 的 byte loader 获取已缓存的图片数据
                            match ui.ctx().try_load_bytes(&url) {
                                Ok(egui::load::BytesPoll::Ready { bytes, .. }) => {
                                    let data = bytes.to_vec();
                                    // 根据图片头部判断扩展名
                                    let ext = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
                                        "png"
                                    } else if data.starts_with(&[0xFF, 0xD8]) {
                                        "jpg"
                                    } else if data.starts_with(b"GIF") {
                                        "gif"
                                    } else if data.starts_with(b"RIFF")
                                        && data.len() > 11
                                        && &data[8..12] == b"WEBP"
                                    {
                                        "webp"
                                    } else {
                                        "png"
                                    };
                                    let default_name = format!("image.{}", ext);
                                    std::thread::spawn(move || {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .set_file_name(&default_name)
                                            .save_file()
                                        {
                                            if let Err(e) = std::fs::write(&path, &data) {
                                                tracing::error!("保存图片失败: {}", e);
                                            } else {
                                                tracing::info!("图片已保存到: {:?}", path);
                                            }
                                        }
                                    });
                                }
                                _ => {
                                    tracing::warn!("图片数据尚未加载完成，无法保存");
                                }
                            }
                        }
                    });
                });

                // 图片内容区域
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    // 键盘缩放: Ctrl+↑/↓
                    let (ctrl_up, ctrl_down, escape) = ui.input(|i| {
                        let ctrl = i.modifiers.ctrl;
                        (
                            ctrl && i.key_pressed(egui::Key::ArrowUp),
                            ctrl && i.key_pressed(egui::Key::ArrowDown),
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    if ctrl_up {
                        viewer_state.lock().unwrap().zoom_in();
                    }
                    if ctrl_down {
                        viewer_state.lock().unwrap().zoom_out();
                    }
                    if escape {
                        viewer_state
                            .lock()
                            .unwrap()
                            .closed
                            .store(true, Ordering::Relaxed);
                        return;
                    }

                    match ui.ctx().try_load_texture(
                        &url,
                        egui::TextureOptions::default(),
                        egui::load::SizeHint::default(),
                    ) {
                        Ok(egui::load::TexturePoll::Ready { texture }) => {
                            let content_rect = ui.available_rect_before_wrap();
                            let response =
                                ui.allocate_rect(content_rect, egui::Sense::click_and_drag());
                            let available = content_rect.size();
                            let img_size = texture.size;

                            // 适应窗口的基础缩放
                            let base_scale_x = available.x / img_size[0] as f32;
                            let base_scale_y = available.y / img_size[1] as f32;
                            let base_scale = base_scale_x.min(base_scale_y).max(0.01);

                            // 更新 base_scale 并处理 1:1 请求
                            {
                                let mut state = viewer_state.lock().unwrap();
                                state.base_scale = base_scale;
                                if state.request_original_size {
                                    state.request_original_size = false;
                                    // 1:1 = 原始像素大小，需要 zoom = 1/base_scale
                                    state.zoom = if base_scale > 0.0 {
                                        1.0 / base_scale
                                    } else {
                                        1.0
                                    };
                                    state.pan_offset = egui::Vec2::ZERO;
                                }
                            }

                            // 重新读取 zoom 和 pan_offset（可能刚被修改）
                            let (mut zoom, mut pan_offset) = {
                                let s = viewer_state.lock().unwrap();
                                (s.zoom, s.pan_offset)
                            };

                            // 滚轮缩放：使用原始滚轮事件按幅度缩放，避免 smooth_scroll_delta
                            // 在多个 frame 中重复触发导致缩放过快。
                            if response.hovered() {
                                let (wheel_delta_y, pointer_pos, pinch_zoom_delta) =
                                    ui.input(|i| {
                                        let wheel_delta_y =
                                            i.events.iter().fold(0.0, |acc, event| match event {
                                                egui::Event::MouseWheel { unit, delta, .. } => {
                                                    let points = match unit {
                                                        egui::MouseWheelUnit::Point => delta.y,
                                                        egui::MouseWheelUnit::Line => {
                                                            delta.y * 40.0
                                                        }
                                                        egui::MouseWheelUnit::Page => {
                                                            delta.y * available.y
                                                        }
                                                    };
                                                    acc + points
                                                }
                                                _ => acc,
                                            });
                                        (wheel_delta_y, i.pointer.hover_pos(), i.zoom_delta())
                                    });

                                let zoom_factor = if wheel_delta_y != 0.0 {
                                    (wheel_delta_y / 200.0).exp()
                                } else if (pinch_zoom_delta - 1.0).abs() > f32::EPSILON {
                                    pinch_zoom_delta
                                } else {
                                    1.0
                                };

                                if (zoom_factor - 1.0).abs() > f32::EPSILON {
                                    let old_zoom = zoom;
                                    let new_zoom = (zoom * zoom_factor).clamp(0.05, 20.0);
                                    if (new_zoom - old_zoom).abs() > f32::EPSILON {
                                        let old_display_w =
                                            img_size[0] as f32 * base_scale * old_zoom;
                                        let old_display_h =
                                            img_size[1] as f32 * base_scale * old_zoom;
                                        let old_center_offset = egui::Vec2::new(
                                            (available.x - old_display_w) / 2.0,
                                            (available.y - old_display_h) / 2.0,
                                        );
                                        let new_display_w =
                                            img_size[0] as f32 * base_scale * new_zoom;
                                        let new_display_h =
                                            img_size[1] as f32 * base_scale * new_zoom;
                                        let new_center_offset = egui::Vec2::new(
                                            (available.x - new_display_w) / 2.0,
                                            (available.y - new_display_h) / 2.0,
                                        );
                                        let anchor = pointer_pos
                                            .filter(|pos| content_rect.contains(*pos))
                                            .unwrap_or(content_rect.center())
                                            - content_rect.min;
                                        let image_anchor = anchor - old_center_offset - pan_offset;
                                        let scale_change = new_zoom / old_zoom;
                                        pan_offset = anchor
                                            - new_center_offset
                                            - image_anchor * scale_change;
                                        zoom = new_zoom;

                                        let mut state = viewer_state.lock().unwrap();
                                        state.zoom = zoom;
                                        state.pan_offset = pan_offset;
                                    }
                                }
                            }

                            let display_w = img_size[0] as f32 * base_scale * zoom;
                            let display_h = img_size[1] as f32 * base_scale * zoom;

                            // 图片居中 + 偏移
                            let center_offset = egui::Vec2::new(
                                (available.x - display_w) / 2.0,
                                (available.y - display_h) / 2.0,
                            );

                            let paint_pos = content_rect.min + center_offset + pan_offset;
                            let paint_rect = egui::Rect::from_min_size(
                                paint_pos,
                                egui::Vec2::new(display_w, display_h),
                            );

                            // 绘制图片
                            let uv = egui::Rect::from_min_max(
                                egui::Pos2::new(0.0, 0.0),
                                egui::Pos2::new(1.0, 1.0),
                            );
                            ui.painter()
                                .image(texture.id, paint_rect, uv, egui::Color32::WHITE);

                            if response.dragged() {
                                let delta = response.drag_delta();
                                viewer_state.lock().unwrap().pan_offset += delta;
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            } else if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                            }
                        }
                        Ok(egui::load::TexturePoll::Pending { .. }) => {
                            ui.centered_and_justified(|ui| {
                                ui.add(egui::Spinner::new());
                            });
                        }
                        Err(err) => {
                            ui.centered_and_justified(|ui| {
                                ui.colored_label(
                                    egui::Color32::LIGHT_RED,
                                    format!("图片加载失败: {}", err),
                                );
                            });
                        }
                    }
                });
            });
        }
    }
}
