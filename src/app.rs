use std::sync::Arc;

use eframe::CreationContext;

use crate::assets;

pub struct IcaApp {
    pub connected: bool,
    pub open_verify: bool,
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
        Self { connected: false, open_verify: false }
    }
}

impl eframe::App for IcaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Icalingua++ native", |ui| {
                    ui.label(crate::VERSION);
                    ui.label("Github (todo)");
                    if ui.button("验证消息").clicked() {
                        ui.close();
                        self.open_verify = true;
                    }
                });
                ui.menu_button("文件", |ui| {
                    if ui.button("新建").clicked() {
                        ui.close();
                    }
                    if ui.button("打开").clicked() {
                        ui.close();
                    }
                    if ui.button("保存").clicked() {
                        ui.close();
                    }
                });
                ui.menu_button("编辑", |ui| {
                    if ui.button("撤销").clicked() {
                        ui.close();
                    }
                    if ui.button("重做").clicked() {
                        ui.close();
                    }
                    if ui.button("复制").clicked() {
                        ui.close();
                    }
                    if ui.button("粘贴").clicked() {
                        ui.close();
                    }
                });
                ui.menu_button("视图", |ui| {
                    if ui.button("放大").clicked() {
                        ui.close();
                    }
                    if ui.button("缩小").clicked() {
                        ui.close();
                    }
                    if ui.button("重置缩放").clicked() {
                        ui.close();
                    }
                });
                ui.menu_button("帮助", |ui| {
                    if ui.button("文档").clicked() {
                        ui.close();
                    }
                    if ui.button("关于").clicked() {
                        ui.close();
                    }
                });
            })
        });

        egui::SidePanel::left("side_panel").show(ctx, |ui| {
            ui.label("消息栏");
            if ui.button("按钮1").clicked() {
                // 处理按钮1点击事件
            }
            if ui.button("按钮2").clicked() {
                // 处理按钮2点击事件
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("egui app ica");
        });

        if self.open_verify {

            egui::Window::new("新页面")
                .default_size(egui::vec2(400.0, 300.0))
                .show(ctx, |ui| {
                    ui.heading("这是一个新页面");
                    ui.label("在这里添加你的内容。");
                    if ui.button("关闭").clicked() {
                        // ui.close();
                        self.open_verify = false;
                    }
                });
        }
    }
}
