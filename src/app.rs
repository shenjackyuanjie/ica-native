use std::sync::Arc;

use eframe::CreationContext;
use egui::{Button, Hyperlink, Image, Label};
use tokio::runtime::Runtime;

use crate::{assets, ica::IcaClient};

pub mod chat_groups;
pub mod config_editer;
pub mod custom_chat;
pub mod online_mode;
pub mod open_page;

use chat_groups::ChatGroups;
use config_editer::ConfigEditer;
use custom_chat::CustomChat;
use online_mode::OnlineMode;
use open_page::AppOpenPage;

use crate::ica::types::{RoomId, room::Room};

pub struct IcaApp {
    /// 是否连接上了
    pub connected: bool,
    /// 聊天界面定制选项
    pub custom_chat: CustomChat,
    /// 在线模式
    pub online_mode: OnlineMode,
    /// 打开了什么页面
    pub open_page: AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
    /// 所有聊天
    pub chat_rooms: Vec<Room>,
    /// 是否选中某个聊天组
    pub chat_group_selected: bool,
    /// 选中了哪个聊天组
    pub chat_group_idx: usize,
    /// 聊天组
    pub chat_groups: ChatGroups,
    /// 配置文件修改
    pub config_editer: ConfigEditer,
    /// 选中的聊天室 ID
    pub selected_room_id: Option<RoomId>,
    /// tokio rt
    /// 用来开 socketio
    pub runtime: Runtime,
    /// Socketio 列表
    /// 一些 Socketio 连接
    pub ica_clients: Vec<IcaClient>,
}

impl IcaApp {
    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        let font_sy_data = egui::FontData::from_static(assets::fonts::FONT_思源黑体);
        let font_unifont_data = egui::FontData::from_static(assets::fonts::FONT_UNIFONT);

        let sy_font_name = "notosans".to_string();
        let unifont_name = "unifont".to_string();

        fonts
            .font_data
            .insert(sy_font_name.clone(), Arc::new(font_sy_data));

        fonts
            .font_data
            .insert(unifont_name.clone(), Arc::new(font_unifont_data));

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, unifont_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(unifont_name.clone());

        ctx.set_fonts(fonts);
    }

    fn setup_async_rt() -> Runtime {
        let config = crate::cfg::get_cfg_snapshot();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("faild to build tokio rt")
    }

    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self {
            connected: false,
            custom_chat: CustomChat::default(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_rooms: Vec::new(),
            chat_group_selected: false,
            chat_group_idx: 0,
            chat_groups: ChatGroups::new(),
            config_editer: ConfigEditer::default(),
            selected_room_id: None,
            runtime: Self::setup_async_rt(),
            ica_clients: Vec::new(),
        }
    }
}

impl IcaApp {
    fn render_top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("顶栏").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.render_menu_icalingua(ui);
                self.render_menu_notification(ui);
                self.render_menu_options(ui);
                self.render_menu_help(ui);
            })
        });
    }

    fn render_menu_icalingua(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Icalingua++ native", |ui| {
            ui.label(crate::VERSION);
            let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
            ui.add(link);
            if ui.button("验证消息").clicked() {
                ui.close();
                self.open_page.verify_message = true;
            }
        });
    }

    fn render_menu_notification(&mut self, ui: &mut egui::Ui) {
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
    }

    fn render_menu_options(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("选项", |ui| {
            ui.label("这里显示你打开了哪些选项页面");
            let _ = ui.checkbox(&mut self.open_page.settings, "设置");
            let _ = ui.checkbox(&mut self.open_page.custom_chat_ica, "定制聊天界面(ica)");
            let _ =
                ui.checkbox(&mut self.open_page.custom_chat_extra, "定制聊天界面(extra)");
            let _ = ui.checkbox(&mut self.open_page.online_status, "在线状态");
            let _ = ui.checkbox(&mut self.open_page.socketio_status, "Socketio 状态");
            let _ = ui.checkbox(&mut self.open_page.raw_config, "配置文件编辑");
        });
    }

    fn render_menu_help(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("帮助", |ui| {
            let link = Hyperlink::from_label_and_url("Github(文档)", crate::GITHUB_LINK);
            ui.add(link);
            if ui.button("关于").clicked() {
                self.open_page.about = true;
            }
        });
    }

    fn render_left_groups_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("群聊组")
            .resizable(false)
            .exact_width(70.0)
            .show(ctx, |ui| {
                ui.label("消息栏");
                ui.label("头像占位");
                // 渲染头像
                ui.spacing_mut().item_spacing.x = 0.5;

                ui.vertical_centered(|ui| {
                    self.render_all_chats_button(ui);
                    self.render_chat_groups(ui);
                });
            });
    }

    fn render_all_chats_button(&mut self, ui: &mut egui::Ui) {
        let img = Image::new(crate::assets::svg::CHAT_GROUP)
            .fit_to_exact_size([24.0, 24.0].into())
            .alt_text("chat_group_icon");
        let btn = Button::image(img.clone());
        if ui.add(btn).clicked() {
            self.chat_group_selected = false;
        };
        let mut text = egui::RichText::new("所有聊天");
        if !self.chat_group_selected {
            text = text.strong();
        }
        let label = Label::new(text).selectable(false);
        ui.add(label);
    }

    fn render_chat_groups(&mut self, ui: &mut egui::Ui) {
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
    }

    fn render_chat_list_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("聊天列表")
            .resizable(true)
            .width_range(150.0..=500.0)
            .show(ctx, |ui| {
                // 让聊天列表条目的背景能"铺满"左右分割线之间的整块区域：
                // 关键点：用 `ui.max_rect()` 的宽度来分配条目 rect，而不是 `ui.available_width()`
                // 因为 `available_width()` 会受当前 layout/indent/scroll 内容区影响而变窄，导致背景留白。
                let full_row_width = ui.max_rect().width();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.render_chat_list_header(ui);
                    self.render_chat_rooms(ui, full_row_width);
                });
            });
    }

    fn render_chat_list_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("聊天列表");
            if ui.button("刷新").clicked() {
                // 刷新聊天列表的逻辑
            }
        });
        ui.separator();
    }

    fn render_chat_rooms(&mut self, ui: &mut egui::Ui, full_row_width: f32) {
        let room_count = self.chat_rooms.len();
        for idx in 0..room_count {
            let room = &self.chat_rooms[idx];
            let room_id = room.room_id;
            let is_selected = self.selected_room_id == Some(room_id);

            // 使用 scope 来避免同时借用 self
            let clicked = {
                let mut clicked = false;
                ui.scope(|ui| {
                    // 先分配空间并检测交互：宽度用整个 panel 行宽（左右分割线之间）
                    let desired_size = egui::vec2(full_row_width, 56.0);
                    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

                    // 先绘制背景（在内容下面）
                    let bg_color = if is_selected {
                        egui::Color32::from_gray(55)
                    } else if response.hovered() {
                        egui::Color32::from_gray(45)
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 4.0, bg_color);

                    // 在背景上渲染内容
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        // 内边距: 上
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // 内边距: 左
                            ui.add_space(4.0);
                            self.render_room_avatar(ui, room);
                            // 头像 和 信息 的间距
                            ui.add_space(2.0);
                            // 右侧：群名和消息预览
                            self.render_room_info(ui, room);
                        });
                    });

                    if response.clicked() {
                        clicked = true;
                    }
                });
                clicked
            };

            if clicked {
                self.selected_room_id = Some(room_id);
            }

            ui.add_space(4.0);
            ui.separator();
        }
    }

    fn render_room_avatar(&self, ui: &mut egui::Ui, room: &Room) {
        // 左侧：头像区域（方形，固定大小）
        // 群聊时右下角叠加发送者头像
        let is_group = room.room_id < 0;
        let avatar_size = 48.0;
        let sender_avatar_size = 18.0;

        // 使用 LayerId 叠加两个头像
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(avatar_size, avatar_size),
            egui::Sense::hover(),
        );

        // 主头像（群头像或私聊头像）
        let avatar_url = room.avatar_url();
        ui.put(
            rect,
            egui::Image::from_uri(avatar_url)
                .fit_to_exact_size(egui::vec2(avatar_size, avatar_size))
                .corner_radius(4.0),
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
                    .corner_radius(2.0),
            );
        }
    }

    fn render_room_info(&self, ui: &mut egui::Ui, room: &Room) {
        ui.vertical(|ui| {
            self.render_room_name_line(ui, room);
            self.render_room_message_preview(ui, room);
        });
    }

    fn render_room_name_line(&self, ui: &mut egui::Ui, room: &Room) {
        // 第一行：群名 @提醒 (未读数)
        ui.horizontal(|ui| {
            let name_text = if room.room_name.is_empty() {
                "未命名聊天"
            } else {
                &room.room_name
            };
            let mut text = egui::RichText::new(name_text);
            if room.unread_count > 0 {
                text = text.strong();
            }
            ui.label(text);

            match room.at {
                crate::ica::types::message::At::All => {
                    ui.colored_label(egui::Color32::YELLOW, "[@全体]");
                }
                crate::ica::types::message::At::Bool(true) => {
                    ui.colored_label(egui::Color32::YELLOW, "[@我]");
                }
                _ => {}
            }

            if room.unread_count > 0 {
                ui.colored_label(
                    egui::Color32::RED,
                    format!("({})", room.unread_count),
                );
            }
        });
    }

    fn render_room_message_preview(&self, ui: &mut egui::Ui, room: &Room) {
        // 第二行：群聊显示 "人名: 内容"，私聊直接显示 "内容"
        let is_group = room.room_id < 0;
        ui.horizontal(|ui| {
            if is_group
                && let Some(ref username) = room.last_message.username
                && !username.is_empty()
            {
                ui.label(
                    egui::RichText::new(format!("{}: ", username))
                        .size(12.0)
                        .color(egui::Color32::LIGHT_BLUE),
                );
            }
            if let Some(ref content) = room.last_message.content
                && !content.is_empty()
            {
                let preview = if content.chars().count() > 20 {
                    format!(
                        "{}...",
                        content.chars().take(20).collect::<String>()
                    )
                } else {
                    content.clone()
                };
                ui.label(egui::RichText::new(preview).size(12.0));
            }
        });
    }

    fn render_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("egui app ica");
        });
    }

    fn render_windows(&mut self, ctx: &egui::Context) {
        self.render_window_custom_chat_ica(ctx);
        self.render_window_custom_chat_extra(ctx);
        self.render_window_online_status(ctx);
        self.render_window_verify_message(ctx);
        self.render_window_about(ctx);
        self.render_window_socketio_status(ctx);
        self.render_window_config_editor(ctx);
        self.render_window_notify_level(ctx);
    }

    fn render_window_custom_chat_ica(&mut self, ctx: &egui::Context) {
        egui::Window::new("定制聊天界面 (ica)")
            .open(&mut self.open_page.custom_chat_ica)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_ica_ui(ui);
            });
    }

    fn render_window_custom_chat_extra(&mut self, ctx: &egui::Context) {
        egui::Window::new("定制聊天界面 (extra)")
            .open(&mut self.open_page.custom_chat_extra)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_extra_ui(ui);
            });
    }

    fn render_window_online_status(&mut self, ctx: &egui::Context) {
        egui::Window::new("在线状态")
            .open(&mut self.open_page.online_status)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("在线状态");
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
    }

    fn render_window_verify_message(&mut self, ctx: &egui::Context) {
        egui::Window::new("验证消息")
            .default_size(egui::vec2(400.0, 300.0))
            .open(&mut self.open_page.verify_message)
            .show(ctx, |ui| {
                ui.heading("这是一个新页面");
                ui.label("在这里添加你的内容。");
            });
    }

    fn render_window_about(&mut self, ctx: &egui::Context) {
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
                ui.add_space(6.0);
                ui.label("一个使用 Rust + egui 开发的跨平台原生 ica 客户端。");
                ui.add_space(8.0);
                ui.collapsing("开源信息", |ui| {
                    ui.label("本项目基于开源许可证发布，欢迎 Star、Issue 与 PR。");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("项目地址：");
                        let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
                        ui.add(link);
                    });
                });
                ui.add_space(8.0);
                ui.collapsing("致谢", |ui| {
                    ui.label("感谢所有贡献者与所使用的开源项目：");
                    ui.label("Icalingua 作者以及各位用户");
                    ui.label("Rust 语言与生态");
                    ui.label("egui/eframe 图形界面框架");
                    ui.label("以及社区用户的反馈与支持");
                });
            });
    }

    fn render_window_socketio_status(&mut self, ctx: &egui::Context) {
        egui::Window::new("Socketio 状态")
            .open(&mut self.open_page.socketio_status)
            .collapsible(true)
            .show(ctx, |ui| {
                ui.heading("Socketio 状态");
            });
    }

    fn render_window_config_editor(&mut self, ctx: &egui::Context) {
        egui::Window::new("配置文件编辑")
            .open(&mut self.open_page.raw_config)
            .collapsible(true)
            .show(ctx, |ui| {
                self.config_editer.ui(ui);
            });
    }

    fn render_window_notify_level(&mut self, ctx: &egui::Context) {
        if self.open_page.notify_level {
            // 在新页面展示一张图
            let size = ctx.screen_rect();
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

impl eframe::App for IcaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 检测 ESC 键取消选择
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected_room_id = None;
        }

        self.render_top_panel(ctx);
        self.render_left_groups_panel(ctx);
        self.render_chat_list_panel(ctx);
        self.render_central_panel(ctx);
        self.render_windows(ctx);
    }
}
