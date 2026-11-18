//! 主窗口的设置中心：聚合既有设置页面，不另设配置状态。

#[cfg(test)]
#[path = "tests/settings_center.rs"]
mod tests;

use crate::app::IcaApp;
use egui::{Context, Event, Key, Ui, ViewportId};

impl IcaApp {
    /// 主窗口前台的 Ctrl+,（macOS 也接受 Command+,），不受当前 Bridge 或控件焦点限制。
    pub fn handle_settings_shortcut(&mut self, ctx: &Context) {
        if ctx.viewport_id() != ViewportId::ROOT
            || !ctx
                .input(|input| input.focused && input.viewport().focused.unwrap_or(input.focused))
            || self.ime_composing
            || self.ime_event_this_frame
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|memory| memory.top_modal_layer().is_some())
        {
            return;
        }
        let activated = ctx.input_mut(|input| {
            let index = input.events.iter().position(|event| {
                matches!(event, Event::Key { key: Key::Comma, pressed: true, repeat: false, modifiers, .. }
                    if !modifiers.alt && !modifiers.shift && (modifiers.ctrl ^ modifiers.mac_cmd))
            });
            if let Some(index) = index {
                input.events.remove(index);
            }
            index.is_some()
        });
        if activated {
            self.open_page.settings = true;
        }
    }

    /// 通知菜单和设置中心共用同一份运行时选项。
    pub fn render_notification_options(&mut self, ui: &mut Ui) {
        ui.label("通知启用级别 1-5");
        ui.add(egui::Slider::new(&mut self.notify_level, 1..=5));
        ui.checkbox(&mut self.mute_any, "禁用任何通知");
        if !self.mute_any {
            ui.checkbox(&mut self.mute_all, "禁用 @ 全体 通知");
        }
    }

    pub fn render_settings_center(&mut self, ctx: &Context) {
        let mut open = self.open_page.settings;
        if !open {
            return;
        }
        let mut opened_subpage = false;
        egui::Window::new("设置中心")
            .open(&mut open)
            .default_width(350.0)
            .show(ctx, |ui| {
                ui.heading("聊天界面");
                if ui.button("定制聊天界面（ica）…").clicked() {
                    self.open_page.custom_chat_ica = true;
                    opened_subpage = true;
                }
                if ui.button("定制聊天界面（原生扩展）…").clicked() {
                    self.open_page.custom_chat_extra = true;
                    opened_subpage = true;
                }
                ui.separator();
                ui.heading("通知");
                self.render_notification_options(ui);
                if ui.button("通知等级说明…").clicked() {
                    self.open_page.notify_level = true;
                    opened_subpage = true;
                }
                ui.separator();
                ui.heading("服务与配置");
                if ui.button("Agent 上下文设置…").clicked() {
                    self.open_page.agent_context_settings = true;
                    opened_subpage = true;
                }
                if ui.button("Noticer 设置…").clicked() {
                    self.open_page.noticer_settings = true;
                    opened_subpage = true;
                }
                if ui.button("配置文件编辑…").clicked() {
                    self.open_page.raw_config = true;
                    opened_subpage = true;
                }
            });
        self.open_page.settings = open && !opened_subpage;
    }
}
