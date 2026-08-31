use crate::app::IcaApp;
use crate::app::state::GroupAnnouncementViewerState;
use crate::ica::IcaCommand;
use crate::ica::types::RoomId;
use crate::ica::types::announcement::GroupAnnouncement;

/// 折叠状态下正文最多展示的行数。
const COLLAPSED_TEXT_LINES: usize = 4;

/// 公告发布时间是秒级时间戳，按本地时区展示。
fn format_publish_time(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "时间未知".to_string();
    }
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "时间未知".to_string())
}

/// 折叠正文：只保留前若干行，并告知是否发生了截断。
fn collapse_text(text: &str) -> (String, bool) {
    let mut lines = text.lines();
    let head = lines
        .by_ref()
        .take(COLLAPSED_TEXT_LINES)
        .collect::<Vec<_>>();
    let truncated = lines.next().is_some();
    (head.join("\n"), truncated)
}

fn announcement_headline(announcement: &GroupAnnouncement) -> String {
    let title = announcement.title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    // QQ 允许发布没有标题的公告，用正文首行兜底，便于在列表里区分。
    announcement
        .text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "(无标题公告)".to_string())
}

fn render_announcement(ui: &mut egui::Ui, announcement: &GroupAnnouncement, expanded: &mut bool) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong(announcement_headline(announcement));
        ui.horizontal_wrapped(|ui| {
            ui.weak(format_publish_time(announcement.publish_time));
            if announcement.sender_id > 0 {
                ui.weak(format!("· 发布者 {}", announcement.sender_id));
            }
            if let Some(read_count) = announcement.read_count {
                ui.weak(format!("· {read_count} 人已读"));
            }
            if announcement.confirm_required {
                ui.weak("· 需确认");
            }
        });

        if announcement.text.trim().is_empty() {
            ui.weak("(正文为空)");
        } else {
            let (collapsed, truncated) = collapse_text(&announcement.text);
            let body = if *expanded {
                &announcement.text
            } else {
                &collapsed
            };
            ui.add(egui::Label::new(body).wrap());
            if truncated || *expanded {
                let label = if *expanded { "收起" } else { "展开全文" };
                if ui.small_button(label).clicked() {
                    *expanded = !*expanded;
                }
            }
        }

        if !announcement.images.is_empty() {
            // 手Q H5 自己拼接配图 URL，规则未确认前先如实说明，不展示可能失效的图片。
            ui.weak(format!(
                "含 {} 张配图，暂不支持在客户端渲染",
                announcement.images.len()
            ));
        }

        ui.horizontal_wrapped(|ui| {
            if ui.small_button("复制正文").clicked() {
                ui.ctx().copy_text(announcement.text.clone());
            }
            if ui.small_button("复制原始 JSON").clicked() {
                ui.ctx().copy_text(
                    serde_json::to_string_pretty(&announcement.raw)
                        .unwrap_or_else(|_| announcement.raw.to_string()),
                );
            }
            if !announcement.fid.is_empty() {
                ui.weak(format!("fid {}", announcement.fid));
            }
        });
    });
}

fn render_viewer(ui: &mut egui::Ui, viewer: &mut GroupAnnouncementViewerState) {
    ui.horizontal_wrapped(|ui| {
        ui.heading(if viewer.room_name.is_empty() {
            viewer.room_id.abs().to_string()
        } else {
            viewer.room_name.clone()
        });
        if ui
            .add_enabled(!viewer.loading, egui::Button::new("刷新"))
            .clicked()
        {
            viewer.reload_requested = true;
        }
        if viewer.loading {
            ui.spinner();
            ui.weak("正在拉取群公告…");
        }
    });

    if let Some(error) = &viewer.last_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    ui.separator();

    if viewer.announcements.is_empty() {
        if !viewer.loading && viewer.last_error.is_none() {
            ui.weak("这个群没有公告");
        }
        return;
    }

    let mut toggled_fid = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for announcement in &viewer.announcements {
            let mut expanded = viewer.expanded_fid.as_deref() == Some(announcement.fid.as_str());
            let was_expanded = expanded;
            render_announcement(ui, announcement, &mut expanded);
            if expanded != was_expanded {
                toggled_fid = Some((announcement.fid.clone(), expanded));
            }
        }
    });
    if let Some((fid, expanded)) = toggled_fid {
        viewer.expanded_fid = expanded.then_some(fid);
    }
}

impl IcaApp {
    /// 打开群公告窗口并立即拉取一次。
    pub(crate) fn open_group_announcements(&mut self, bridge_idx: usize, room_id: RoomId) {
        if room_id >= 0 {
            self.bridge_states[bridge_idx].last_error = Some("只有群聊才有群公告".to_string());
            return;
        }
        let room_name = self.bridge_states[bridge_idx]
            .rooms
            .iter()
            .find(|room| room.room_id == room_id)
            .map(|room| room.room_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| room_id.abs().to_string());
        let request_id = self.bridge_states[bridge_idx]
            .group_announcement_viewer
            .lock()
            .unwrap()
            .begin_request(room_id, room_name);
        self.send_group_announcements_request(bridge_idx, room_id, request_id);
    }

    fn send_group_announcements_request(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        request_id: u64,
    ) {
        // bkn 优先取 onlineData；为空时由 IO 线程按 Cookie 里的 skey 现算。
        let bkn = self.bridge_states[bridge_idx].online_data.bkn;
        if let Err(error) =
            self.bridge_states[bridge_idx].send(IcaCommand::FetchGroupAnnouncements {
                request_id,
                room_id,
                bkn,
            })
        {
            self.bridge_states[bridge_idx]
                .group_announcement_viewer
                .lock()
                .unwrap()
                .fail(request_id, room_id, format!("群公告请求发送失败: {error}"));
        }
    }

    pub(in crate::app) fn render_group_announcements_window(&mut self, ctx: &egui::Context) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let viewer_state = self.bridge_states[bridge_idx]
            .group_announcement_viewer
            .clone();

        let reload_target = {
            let mut viewer = viewer_state.lock().unwrap();
            std::mem::take(&mut viewer.reload_requested)
                .then(|| (viewer.room_id, viewer.room_name.clone()))
        };
        if let Some((room_id, room_name)) = reload_target {
            let request_id = viewer_state
                .lock()
                .unwrap()
                .begin_request(room_id, room_name);
            self.send_group_announcements_request(bridge_idx, room_id, request_id);
        }

        let (open, room_name) = {
            let viewer = viewer_state.lock().unwrap();
            (viewer.open, viewer.room_name.clone())
        };
        if !open {
            return;
        }

        let viewport_id = egui::ViewportId::from_hash_of((
            "group_announcements",
            &self.bridge_states[bridge_idx].bridge_key,
        ));
        let parent_viewport_id = ctx.viewport_id();
        let title = if room_name.is_empty() {
            "群公告".to_string()
        } else {
            format!("群公告 - {room_name}")
        };
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([520.0, 660.0])
            .with_min_inner_size([360.0, 320.0]);
        let viewport_state = viewer_state.clone();
        ctx.show_viewport_deferred(
            viewport_id,
            viewport_builder,
            move |viewport_ctx, _class| {
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    viewport_state.lock().unwrap().open = false;
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                    return;
                }

                egui::CentralPanel::default().show(viewport_ctx, |ui| {
                    render_viewer(ui, &mut viewport_state.lock().unwrap());
                });

                // 刷新要回到主视口才能发命令，这里主动唤醒父窗口。
                if viewport_state.lock().unwrap().reload_requested {
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{announcement_headline, collapse_text};
    use crate::ica::types::announcement::parse_announcement_list;
    use serde_json::json;

    fn announcement(title: &str, text: &str) -> crate::ica::types::announcement::GroupAnnouncement {
        parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [{ "fid": "1", "msg": { "title": title, "text": text } }]
        }))
        .expect("构造测试公告")
        .remove(0)
    }

    #[test]
    fn headline_falls_back_to_first_non_empty_body_line_when_title_is_absent() {
        // QQ 允许发布没有标题的公告，列表里必须仍然可区分。
        assert_eq!(
            announcement_headline(&announcement("公告标题", "正文")),
            "公告标题"
        );
        assert_eq!(
            announcement_headline(&announcement("   ", "\n\n  首行  \n次行")),
            "首行"
        );
        assert_eq!(
            announcement_headline(&announcement("", "   \n  ")),
            "(无标题公告)"
        );
    }

    #[test]
    fn collapsing_reports_truncation_only_when_lines_are_actually_dropped() {
        let (text, truncated) = collapse_text("1\n2\n3\n4");
        assert_eq!(text, "1\n2\n3\n4");
        assert!(!truncated, "正好 4 行不应显示展开按钮");

        let (text, truncated) = collapse_text("1\n2\n3\n4\n5");
        assert_eq!(text, "1\n2\n3\n4");
        assert!(truncated);
    }
}
