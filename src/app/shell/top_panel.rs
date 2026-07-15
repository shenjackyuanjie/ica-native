use crate::app::IcaApp;
use egui::Hyperlink;

impl IcaApp {
    pub fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("顶栏").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.render_top_menus(ui);
            });
            if let Some(error) = self.media_error.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                    if ui.small_button("关闭").clicked() {
                        self.media_error = None;
                    }
                });
            } else if let Some(notice) = self.media_notice.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, notice);
                    if ui.small_button("关闭").clicked() {
                        self.media_notice = None;
                    }
                });
            }
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
                if let Some(active_bridge_idx) = self.active_bridge_idx {
                    self.request_system_messages(active_bridge_idx);
                }
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
            let _ = ui.checkbox(&mut self.open_page.group_tools, "群/成员管理");
            let _ = ui.checkbox(&mut self.open_page.account_tools, "账号/登录设备");
            let _ = ui.checkbox(&mut self.open_page.file_tools, "文件/资源工具");
            let _ = ui.checkbox(&mut self.open_page.message_tools, "消息检索/历史");
            let _ = ui.checkbox(&mut self.open_page.room_tools, "会话设置");
            let _ = ui.checkbox(&mut self.open_page.auto_sign, "全群自动签到");
            let _ = ui.checkbox(&mut self.open_page.relation_network, "QQ 关系网");
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

    // 左侧群组面板
}
