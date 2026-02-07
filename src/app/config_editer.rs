use crate::cfg::IcaCfg;

/// 配置编辑器
///
/// 代码参考: https://github.com/emilk/egui/blob/main/crates/egui_demo_lib/src/demo/code_editor.rs
pub struct ConfigEditer {
    pub raw_cache: String,
    pub raw_serde_err_msg: Option<String>,
}

impl ConfigEditer {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let Self {
            raw_cache: code,
            raw_serde_err_msg,
        } = self;
        ui.heading("\\toml/");

        if ui.button("重新加载当前配置").clicked() {
            let cfg = crate::cfg::get_cfg_snapshot();
            *code = cfg.to_string();
        }
        if ui.button("重新加载配置文件").clicked() && crate::cfg::reload_cfg().is_err() {
            // todo: 显示错误信息
        }
        if ui.button("保存并关闭").clicked() {
            match toml::from_str::<IcaCfg>(code) {
                Ok(new_cfg) => {
                    crate::cfg::update_and_save_cfg(|cfg| *cfg = new_cfg);
                    ui.close_kind(egui::UiKind::Window);
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    *raw_serde_err_msg = Some(err_msg);
                }
            }
        }
        if let Some(msg) = raw_serde_err_msg {
            let text = egui::RichText::new(msg.clone()).code();
            let label = egui::Label::new(text);
            ui.add(label);
        }

        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());

        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut layout_job = egui_extras::syntax_highlighting::highlight(
                ui.ctx(),
                ui.style(),
                &theme,
                buf.as_str(),
                "toml",
            );
            layout_job.wrap.max_width = wrap_width;
            ui.ctx().fonts_mut(|f| f.layout_job(layout_job))
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(code)
                    .code_editor()
                    .desired_rows(20)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            )
        });
    }
}

impl Default for ConfigEditer {
    fn default() -> Self {
        let cfg = crate::cfg::get_cfg_snapshot();
        Self {
            raw_cache: cfg.to_string(),
            raw_serde_err_msg: None,
        }
    }
}
