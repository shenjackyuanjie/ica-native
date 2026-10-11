//! 配置文件（TOML）编辑器窗口。
//!
//! 面向高级用户：直接以文本形式查看和修改整份 `ica_native*.toml`。
//! 编辑的是内存中的文本副本，只有“保存并关闭”才会尝试解析、替换并写盘；
//! 解析或写盘失败时保留文本与错误信息，用户不会丢失未保存的修改。

use crate::config::{ConfigStore, IcaCfg};

/// 配置编辑器
///
/// 代码参考: https://github.com/emilk/egui/blob/main/crates/egui_demo_lib/src/demo/code_editor.rs
#[derive(Default)]
pub struct ConfigEditor {
    /// 编辑器中的 TOML 文本；与磁盘内容可能不一致。
    pub raw_cache: String,
    /// 最近一次解析/保存失败的原因；成功后被清空。
    pub raw_serde_err_msg: Option<String>,
}

impl ConfigEditor {
    /// 用当前配置快照的序列化文本初始化编辑器。
    pub fn new(store: &ConfigStore) -> Self {
        Self {
            raw_cache: store.snapshot().to_string(),
            raw_serde_err_msg: None,
        }
    }

    /// 绘制编辑器窗口内容。
    ///
    /// 提供三个动作：重新加载当前配置（丢弃未保存修改）、重新加载配置文件
    /// （从磁盘重读）、保存并关闭（解析为 `IcaCfg` 后替换并写盘）。任一环节
    /// 失败都只在界面上展示错误，不关闭窗口。
    pub fn ui(&mut self, ui: &mut egui::Ui, store: &ConfigStore) {
        let Self {
            raw_cache: code,
            raw_serde_err_msg,
        } = self;
        ui.heading("\\toml/");

        if ui.button("重新加载当前配置").clicked() {
            let cfg = store.snapshot();
            *code = cfg.to_string();
        }
        if ui.button("重新加载配置文件").clicked() {
            match store.reload() {
                Ok(()) => *code = store.snapshot().to_string(),
                Err(error) => *raw_serde_err_msg = Some(error.to_string()),
            }
        }
        if ui.button("保存并关闭").clicked() {
            match toml::from_str::<IcaCfg>(code) {
                Ok(new_cfg) => {
                    let result = store.replace(new_cfg).and_then(|()| store.save());
                    match result {
                        Ok(()) => ui.close_kind(egui::UiKind::Window),
                        Err(error) => *raw_serde_err_msg = Some(error.to_string()),
                    }
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
