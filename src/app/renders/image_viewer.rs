use std::sync::atomic::Ordering;

use crate::app::IcaApp;

use super::{should_probe_gif_after_static_error, try_load_gif_texture};

impl IcaApp {
    pub(super) fn render_image_viewer(&mut self, ctx: &egui::Context) {
        // 图片查看器关闭信号检测
        if let Some(ref viewer) = self.image_viewer
            && viewer.lock().unwrap().closed.load(Ordering::Relaxed)
        {
            self.image_viewer = None;
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

                let (escape, previous, next) = ui.ctx().input(|input| {
                    (
                        input.key_pressed(egui::Key::Escape),
                        input.key_pressed(egui::Key::ArrowLeft) && !input.modifiers.ctrl,
                        input.key_pressed(egui::Key::ArrowRight) && !input.modifiers.ctrl,
                    )
                });
                if escape {
                    viewer_state
                        .lock()
                        .unwrap()
                        .closed
                        .store(true, Ordering::Relaxed);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                if previous {
                    viewer_state.lock().unwrap().navigate(-1);
                } else if next {
                    viewer_state.lock().unwrap().navigate(1);
                }

                let url = viewer_state.lock().unwrap().url.clone();

                // 顶部工具栏
                egui::Panel::top("image_viewer_toolbar").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (image_index, image_count) = {
                            let state = viewer_state.lock().unwrap();
                            (state.image_index, state.images.len())
                        };
                        if ui
                            .add_enabled(image_index > 0, egui::Button::new("← 上一张"))
                            .clicked()
                        {
                            viewer_state.lock().unwrap().navigate(-1);
                        }
                        if ui
                            .add_enabled(
                                image_index + 1 < image_count,
                                egui::Button::new("下一张 →"),
                            )
                            .clicked()
                        {
                            viewer_state.lock().unwrap().navigate(1);
                        }
                        ui.weak(format!("{} / {}", image_index + 1, image_count));
                        ui.separator();
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
                                    } else if data.starts_with(b"MM\0*")
                                        || data.starts_with(b"II*\0")
                                    {
                                        "tiff"
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
                        ui.separator();
                        if ui.button("关闭 (Esc)").clicked() {
                            viewer_state
                                .lock()
                                .unwrap()
                                .closed
                                .store(true, Ordering::Relaxed);
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });

                // 图片内容区域
                egui::CentralPanel::default().show(ui, |ui| {
                    // 键盘缩放: Ctrl+↑/↓
                    let (ctrl_up, ctrl_down) = ui.input(|i| {
                        let ctrl = i.modifiers.ctrl;
                        (
                            ctrl && i.key_pressed(egui::Key::ArrowUp),
                            ctrl && i.key_pressed(egui::Key::ArrowDown),
                        )
                    });
                    if ctrl_up {
                        viewer_state.lock().unwrap().zoom_in();
                    }
                    if ctrl_down {
                        viewer_state.lock().unwrap().zoom_out();
                    }
                    match load_viewer_texture(ui.ctx(), &url) {
                        Ok(egui::load::TexturePoll::Ready { texture }) => {
                            let content_rect = ui.available_rect_before_wrap();
                            let response =
                                ui.allocate_rect(content_rect, egui::Sense::click_and_drag());
                            let available = content_rect.size();
                            let img_size = texture.size;

                            // 适应窗口的基础缩放
                            let base_scale_x = available.x / img_size[0];
                            let base_scale_y = available.y / img_size[1];
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
                                        let old_display_w = img_size[0] * base_scale * old_zoom;
                                        let old_display_h = img_size[1] * base_scale * old_zoom;
                                        let old_center_offset = egui::Vec2::new(
                                            (available.x - old_display_w) / 2.0,
                                            (available.y - old_display_h) / 2.0,
                                        );
                                        let new_display_w = img_size[0] * base_scale * new_zoom;
                                        let new_display_h = img_size[1] * base_scale * new_zoom;
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

                            let display_w = img_size[0] * base_scale * zoom;
                            let display_h = img_size[1] * base_scale * zoom;

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

fn load_viewer_texture(ctx: &egui::Context, url: &str) -> egui::load::TextureLoadResult {
    match ctx.try_load_texture(
        url,
        egui::TextureOptions::default(),
        egui::load::SizeHint::default(),
    ) {
        Err(err) if should_probe_gif_after_static_error(&err) => try_load_gif_texture(
            ctx,
            url,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        )
        .unwrap_or(Err(err)),
        result => result,
    }
}
