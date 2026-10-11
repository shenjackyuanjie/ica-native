//! 顶部菜单栏：应用信息、通知设置、选项与帮助。
//!
//! 顶栏只放菜单按钮，把入口收敛成四个菜单，避免占满一行按钮。其中
//! “选项”菜单是各功能窗口的总开关，直接勾选 `open_page` 上的布尔值；
//! 需要即时生效的设置（Noticer 启停）在勾选当时就保存并应用。

use crate::app::IcaApp;
use egui::Hyperlink;

impl IcaApp {
    /// 绘制顶部菜单栏容器。
    pub fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("顶栏").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.render_top_menus(ui);
            });
        });
    }

    // 合并后的顶部菜单：包含 Icalingua 信息、通知设置、选项、帮助
    /// 绘制四个顶层菜单的内容。
    ///
    /// - Icalingua++ native：版本、仓库链接与验证消息入口；
    /// - 通知设置：通知级别滑块与两类免打扰开关；
    /// - 选项：即时生效的服务开关与各功能窗口的打开/关闭勾选；
    /// - 帮助：文档链接与关于窗口。
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
            let mut noticer_enabled = self.config.snapshot().noticer.enabled;
            if ui.checkbox(&mut noticer_enabled, "启用 Noticer").changed() {
                // 启停 Noticer 服务不等保存：勾选当时就写盘并应用到运行时。
                let config_store = self.config.clone();
                let config = self
                    .noticer_editor
                    .set_enabled(noticer_enabled, &config_store);
                if let Some(config) = config {
                    self.runtime.apply_noticer_config(config);
                }
            }
            if ui.button("Agent 上下文设置…").clicked() {
                self.open_page.agent_context_settings = true;
                ui.close();
            }
            if ui.button("Noticer 设置…").clicked() {
                self.open_page.noticer_settings = true;
                ui.close();
            }
            ui.separator();
            ui.label("打开选项页面");
            let _ = ui.checkbox(&mut self.open_page.settings, "设置");
            let _ = ui.checkbox(&mut self.open_page.custom_chat_ica, "定制聊天界面(ica)");
            let _ = ui.checkbox(&mut self.open_page.custom_chat_extra, "定制聊天界面(extra)");
            let _ = ui.checkbox(&mut self.open_page.online_status, "在线状态");
            let _ = ui.checkbox(&mut self.open_page.socketio_status, "Socket.IO 状态");
            let contacts_was_open = self.open_page.contacts;
            let _ = ui.checkbox(&mut self.open_page.contacts, "联系人");
            if !contacts_was_open && self.open_page.contacts {
                // 勾选即打开：联系人窗口需要立即发起一次列表请求。
                self.open_contacts();
            }
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
