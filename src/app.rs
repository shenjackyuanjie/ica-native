use std::sync::Arc;

use eframe::CreationContext;
use egui::Hyperlink;

use crate::assets;

pub mod open_page;

pub struct IcaApp {
    /// 是否连接上了
    pub connected: bool,
    /// 打开了什么页面
    pub open_page: open_page::AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
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
        Self { connected: false, open_page: open_page::AppOpenPage::default(), mute_any: false, mute_all: false, notify_level: 3 }
    }

    /// 后面再写的加载配置文件
    pub fn new_with_cfg() {
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

        egui::SidePanel::left("side_panel").show(ctx, |ui| {
            ui.label("消息栏");
            ui.label("头像占位");
            let chat_groups = vec!["群组1", "群组2", "群组3"];
            for group in chat_groups {
                ui.label(group);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("egui app ica");
        });

        if self.open_page.verify_message {
            egui::Window::new("验证消息")
                .default_size(egui::vec2(400.0, 300.0))
                .open(&mut self.open_page.verify_message)
                .show(ctx, |ui| {
                    ui.heading("这是一个新页面");
                    ui.label("在这里添加你的内容。");
                });
        }

        if self.open_page.about {
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
        }

        if self.open_page.notify_level {
            // 在新页面展示一张图
            let size = ctx.screen_rect();
            egui::Window::new("通知等级说明")
                .open(&mut self.open_page.notify_level)
                .collapsible(false)
                .default_size((size.width() / 2.0, size.height() / 2.0))
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
