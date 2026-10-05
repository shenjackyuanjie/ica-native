use super::IcaApp;

impl IcaApp {
    fn save_agent_context_settings(&mut self) {
        let previous = self.config.snapshot();
        let mut updated = previous.clone();
        updated.agent_context = self.agent_context_ui.draft.clone();
        let result = self
            .config
            .replace(updated)
            .and_then(|()| self.config.save());
        match result {
            Ok(()) => {
                self.agent_context_ui.settings_error = None;
                self.agent_context_ui.settings_notice =
                    Some("已保存；服务和授权策略立即更新，待处理请求作废。".into());
                self.sync_agent_context_config();
            }
            Err(error) => {
                let _ = self.config.replace(previous);
                self.agent_context_ui.settings_error = Some(error.to_string());
            }
        }
    }
    pub fn render_agent_context_settings(&mut self, ctx: &egui::Context) {
        if !self.open_page.agent_context_settings {
            return;
        }
        let contexts = self.opened_agent_contexts();
        let service_status = self.runtime.agent_context_controller.status();
        let mut open = true;
        let mut save = false;
        egui::Window::new("Agent 聊天上下文设置").id(egui::Id::new("agent_context_settings"))
            .open(&mut open).default_width(560.0).show(ctx, |ui| {
                let state = &mut self.agent_context_ui;
                ui.label(service_status);
                ui.weak("独立只读服务，固定监听 127.0.0.1；关闭窗口不关闭服务，取消启用并保存才会停用。");
                ui.checkbox(&mut state.draft.enabled, "启用 Agent 上下文接口");
                ui.horizontal(|ui| {
                    ui.label("端口");
                    ui.add(egui::DragValue::new(&mut state.draft.port).range(1..=65535));
                });
                ui.label("独立 Bearer Token（不要复用 Noticer Token）");
                ui.add(egui::TextEdit::singleline(&mut state.draft.auth_token).password(true));
                ui.horizontal(|ui| {
                    if ui.button("生成新 Token").clicked() { state.draft.auth_token = hex::encode(rand::random::<[u8; 32]>()); }
                    if ui.add_enabled(!state.draft.auth_token.is_empty(), egui::Button::new("复制 Token")).clicked() { ui.ctx().copy_text(state.draft.auth_token.clone()); }
                });
                ui.separator();
                ui.label("白名单：允许调用方直接读取，但会话仍必须处于打开状态。");
                let mut remove = None;
                for (index, target) in state.draft.allowlist.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} / {}", target.bridge, target.room_id));
                        if ui.small_button("移除").clicked() { remove = Some(index); }
                    });
                }
                if let Some(index) = remove { state.draft.allowlist.remove(index); }
                ui.label("从已打开会话加入白名单：");
                egui::ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                    for context in &contexts {
                        if !state.draft.allows(&context.target)
                            && ui.button(format!("添加 {} / {}（{}）", context.target.bridge, context.room_name, context.target.room_id)).clicked() {
                            state.draft.allowlist.push(context.target.clone());
                        }
                    }
                });
                ui.weak("空白名单不会直接开放记录；每次都需要你批准。非白名单批准不会自动加入白名单。");
                ui.horizontal(|ui| {
                    save = ui.button("保存并应用").clicked();
                    if ui.button("放弃未保存修改").clicked() { state.draft = state.loaded.clone(); state.settings_error = None; state.settings_notice = None; }
                });
                if let Some(error) = &state.settings_error { ui.colored_label(egui::Color32::RED, error); }
                if let Some(notice) = &state.settings_notice { ui.label(notice); }
            });
        self.open_page.agent_context_settings = open;
        if save {
            self.save_agent_context_settings();
        }
    }
}
