use std::sync::Arc;

use eframe::CreationContext;
use egui::{Button, Hyperlink, Image, Label};

use crate::assets;

pub mod custom_chat;
pub mod online_mode;
pub mod chat_groups;
pub mod open_page;

use custom_chat::CustomChat;
use online_mode::OnlineMode;
use chat_groups::ChatGroup;

pub type RoomId = i32;

pub struct IcaApp {
    /// 是否连接上了
    pub connected: bool,
    /// 聊天界面定制选项
    pub custom_chat: CustomChat,
    /// 在线模式
    pub online_mode: OnlineMode,
    /// 打开了什么页面
    pub open_page: open_page::AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
    /// 所有聊天
    pub chat_rooms: Vec<RoomId>,
    /// 是否选中某个聊天组
    pub chat_group_selected: bool,
    /// 选中了哪个聊天组
    pub chat_group_idx: usize,
    /// 聊天组
    pub chat_groups: Vec<ChatGroup>,
}

impl IcaApp {
    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        let font_yh_data = egui::FontData::from_static(assets::fonts::FONT_微软新雅黑);

        fonts
            .font_data
            .insert("msyh".to_string(), Arc::new(font_yh_data));

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "msyh".to_string());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("msyh".to_string());

        ctx.set_fonts(fonts);
    }

    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self {
            connected: false,
            custom_chat: CustomChat::default(),
            online_mode: OnlineMode::default(),
            open_page: open_page::AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_rooms: Vec::new(),
            chat_group_selected: false,
            chat_group_idx: 0,
            chat_groups: Vec::new(),
        }
    }

    /// 后面再写的加载配置文件
    pub fn new_with_cfg(_cc: &CreationContext<'_>) {
        todo!()
    }
}

impl eframe::App for IcaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Icalingua++ native", |ui| {
                    ui.label(crate::VERSION);
                    let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
                    ui.add(link);
                    if ui.button("验证消息").clicked() {
                        ui.close();
                        self.open_page.verify_message = true;
                    }
                });
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
                ui.menu_button("选项", |ui| {
                    ui.label("这里显示你打开了哪些选项页面");
                    let _ = ui.checkbox(&mut self.open_page.custom_chat_ica, "定制聊天界面(ica)");
                    let _ =
                        ui.checkbox(&mut self.open_page.custom_chat_extra, "定制聊天界面(extra)");
                    let _ = ui.checkbox(&mut self.open_page.online_status, "在线状态");
                });
                ui.menu_button("帮助", |ui| {
                    if ui.button("文档").clicked() {
                        ui.close();
                    }
                    if ui.button("关于").clicked() {
                        ui.close();
                        self.open_page.about = true;
                    }
                });
            })
        });

        egui::SidePanel::left("群聊组")
            .resizable(false)
            .exact_width(70.0)
            .show(ctx, |ui| {
                ui.label("消息栏");
                ui.label("头像占位");
                let chat_groups = vec!["群组1", "群组2", "群组3"];
                // let chat_groups = self.chat_groups

                ui.vertical_centered(|ui| {
                    let img = Image::new(crate::assets::svg::CHAT_GROUP)
                        .max_width(24.0)
                        .fit_to_exact_size([24.0, 24.0].into())
                        .alt_text("chat_group_icon");
                    // all chats
                    let btn = Button::image(img.clone());
                    if ui.add(btn).clicked() {
                        self.chat_group_selected = false;
                    };
                    {
                        let mut text = egui::RichText::new("所有聊天");
                        if !self.chat_group_selected {
                            text = text.strong();
                        }
                        let label = Label::new(text).selectable(false);
                        ui.add(label);
                    }
                    for (idx, group) in chat_groups.iter().enumerate() {
                        // icon + text
                        let btn = Button::image(img.clone());
                        if ui.add(btn).clicked() {
                            self.chat_group_selected = true;
                            self.chat_group_idx = idx;
                        };
                        let mut text: egui::RichText = (*group).into();
                        if idx == self.chat_group_idx && self.chat_group_selected {
                            text = text.strong();
                        }
                        let label = Label::new(text).selectable(false);
                        ui.add(label);
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("egui app ica");
        });

        egui::Window::new("定制聊天界面 (ica)")
            .open(&mut self.open_page.custom_chat_ica)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_ica_ui(ui);
            });

        egui::Window::new("定制聊天界面 (extra)")
            .open(&mut self.open_page.custom_chat_extra)
            .resizable(false)
            .show(ctx, |ui| {
                self.custom_chat.show_extra_ui(ui);
            });

        egui::Window::new("在线状态")
            .open(&mut self.open_page.online_status)
            .resizable(false)
            .show(ctx, |ui| {
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

        // if self.open_page.verify_message {
        egui::Window::new("验证消息")
            .default_size(egui::vec2(400.0, 300.0))
            .open(&mut self.open_page.verify_message)
            .show(ctx, |ui| {
                ui.heading("这是一个新页面");
                ui.label("在这里添加你的内容。");
            });
        // }

        // if self.open_page.about {
        egui::Window::new("关于 Icalingua++ native")
            .default_size(egui::vec2(420.0, 320.0))
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
        // }

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
