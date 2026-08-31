use crate::app::IcaApp;
use egui::Hyperlink;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

fn changelog_releases() -> impl Iterator<Item = (&'static str, &'static str)> {
    CHANGELOG.split("\n## ").skip(1).filter_map(|release| {
        let (title, content) = release.split_once('\n').unwrap_or((release, ""));
        let title = title.trim();
        (!title.is_empty()).then_some((title, content.trim_end()))
    })
}

fn changelog_section_name(name: &str) -> &str {
    match name {
        "Added" => "新增",
        "Changed" => "变更",
        "Deprecated" => "弃用",
        "Removed" => "移除",
        "Fixed" => "修复",
        "Security" => "安全",
        _ => name,
    }
}

fn render_release_notes(ui: &mut egui::Ui, content: &str) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(3.0);
        } else if let Some(section) = trimmed.strip_prefix("### ") {
            ui.add_space(4.0);
            ui.strong(changelog_section_name(section));
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            let nested = line.starts_with(char::is_whitespace);
            ui.horizontal_wrapped(|ui| {
                ui.label(if nested { "◦" } else { "•" });
                ui.add(egui::Label::new(item).wrap());
            });
        } else {
            ui.add(egui::Label::new(trimmed).wrap());
        }
    }
}

fn render_changelog(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .id_salt("about_changelog")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, (title, content)) in changelog_releases().enumerate() {
                egui::CollapsingHeader::new(title)
                    .id_salt(("changelog_release", title))
                    .default_open(index == 0)
                    .show(ui, |ui| render_release_notes(ui, content));
            }
        });
}

impl IcaApp {
    pub fn render_about_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("关于 Icalingua++ native")
            .open(&mut self.open_page.about)
            .collapsible(true)
            .default_size([640.0, 680.0])
            .min_size([420.0, 360.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Icalingua++ native");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("版本：");
                    ui.monospace(crate::VERSION);
                });
                // 标题与正文之间留出一点垂直间距
                ui.add_space(6.0);
                ui.label("一个使用 Rust + egui 开发的跨平台原生 ica 客户端。");
                // 正文与“开源信息”分组之间留出一点垂直间距
                ui.add_space(8.0);
                ui.collapsing("开源信息", |ui| {
                    ui.label("本项目基于开源许可证发布，欢迎 Star、Issue 与 PR。");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("项目地址：");
                        let link = Hyperlink::from_label_and_url("Github", crate::GITHUB_LINK);
                        ui.add(link);
                    });
                });
                // “开源信息”和“致谢”分组之间留出一点垂直间距
                ui.add_space(8.0);
                ui.collapsing("致谢", |ui| {
                    ui.label("感谢所有贡献者与所使用的开源项目：");
                    ui.label("Icalingua 作者以及各位用户");
                    ui.label("Rust 语言与生态");
                    ui.label("egui/eframe 图形界面框架");
                    ui.label("以及社区用户的反馈与支持");
                });
                ui.add_space(8.0);
                ui.separator();
                ui.heading("更新日志");
                render_changelog(ui);
            });
    }
}
