use crate::app::IcaApp;
use crate::app::media::ImageSource;
use crate::app::state::GroupAnnouncementViewerState;
use crate::ica::IcaCommand;
use crate::ica::types::RoomId;
use crate::ica::types::announcement::{GroupAnnouncement, GroupAnnouncementDraft};

/// 折叠状态下正文最多展示的行数。
const COLLAPSED_TEXT_LINES: usize = 4;

/// 配图在列表里的最大显示宽度。
const IMAGE_DISPLAY_WIDTH: f32 = 320.0;

/// 无法从 CGI 拿到宽高时的占位高度，避免加载过程中列表反复跳动。
const IMAGE_FALLBACK_HEIGHT: f32 = 180.0;

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

/// 右键菜单里产生的、需要回写 viewer 的动作。
enum MenuAction {
    /// 用这条公告预填编辑器。
    Edit(GroupAnnouncementDraft),
    /// 请求删除，回到 viewer 后二次确认。
    RequestDelete(String),
}

fn render_announcement(
    ui: &mut egui::Ui,
    announcement: &GroupAnnouncement,
    expanded: &mut bool,
    pending_image_url: &mut Option<String>,
    menu_action: &mut Option<MenuAction>,
) {
    let frame = egui::Frame::group(ui.style()).show(ui, |ui| {
        // 让整块公告撑满可用宽度，否则窗口拉宽后卡片只占内容宽度，右键区域也会跟着缩水。
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            if announcement.to_new {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "新成员");
            }
            if announcement.pinned {
                ui.colored_label(egui::Color32::LIGHT_BLUE, "置顶");
            }
            ui.strong(announcement_headline(announcement));
        });
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

        for image in &announcement.images {
            let width = IMAGE_DISPLAY_WIDTH.min(ui.available_width());
            // 图片解码完成前 egui 不知道真实尺寸，先按 CGI 声明的宽高比占位。
            let height = image
                .display_height(width)
                .unwrap_or(IMAGE_FALLBACK_HEIGHT)
                .min(width * 2.0);
            let response = ui.add_sized(
                [width, height],
                egui::Image::from_uri(image.thumbnail_url())
                    .maintain_aspect_ratio(true)
                    .sense(egui::Sense::click()),
            );
            if response.on_hover_text("点击查看原图").clicked() {
                *pending_image_url = Some(image.original_url());
            }
        }
    });

    // 复制类操作放进右键菜单，正文区域保持干净。
    frame.response.context_menu(|ui| {
        if ui.button("复制正文").clicked() {
            ui.ctx().copy_text(announcement.text.clone());
            ui.close();
        }
        if ui.button("复制原始 JSON").clicked() {
            ui.ctx().copy_text(
                serde_json::to_string_pretty(&announcement.raw)
                    .unwrap_or_else(|_| announcement.raw.to_string()),
            );
            ui.close();
        }
        if !announcement.fid.is_empty() {
            ui.separator();
            if ui.button("编辑").clicked() {
                *menu_action = Some(MenuAction::Edit(GroupAnnouncementDraft::from_announcement(
                    announcement,
                )));
                ui.close();
            }
            if ui.button("删除").clicked() {
                *menu_action = Some(MenuAction::RequestDelete(announcement.fid.clone()));
                ui.close();
            }
        }
        if !announcement.fid.is_empty() && ui.button("复制公告 ID").clicked() {
            ui.ctx().copy_text(announcement.fid.clone());
            ui.close();
        }
    });
}

/// 发布/编辑公告的表单。
fn render_editor(ui: &mut egui::Ui, viewer: &mut GroupAnnouncementViewerState) {
    ui.strong(if viewer.draft.is_edit() {
        "编辑公告"
    } else {
        "发布新公告"
    });
    ui.add(
        egui::TextEdit::multiline(&mut viewer.draft.text)
            .hint_text("公告正文")
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );

    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut viewer.draft.pinned, "置顶");
        ui.checkbox(&mut viewer.draft.to_new, "发给新成员");
        ui.checkbox(&mut viewer.draft.confirm_required, "需确认收到");
        ui.checkbox(&mut viewer.draft.show_edit_card, "允许改群名片");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("提醒方式");
        ui.radio_value(&mut viewer.draft.tip_window_type, 0, "弹窗提醒");
        ui.radio_value(&mut viewer.draft.tip_window_type, 1, "仅发到群里");
    });

    if let Some(error) = viewer.editor_error.clone() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    ui.horizontal_wrapped(|ui| {
        let can_submit = !viewer.submitting && !viewer.draft.text.trim().is_empty();
        if ui
            .add_enabled(can_submit, egui::Button::new("发布"))
            .clicked()
        {
            viewer.editor_error = None;
            viewer.pending_submit = true;
        }
        if viewer.submitting {
            ui.spinner();
            ui.weak("正在提交…");
        }
        if ui
            .add_enabled(!viewer.submitting, egui::Button::new("取消"))
            .clicked()
        {
            viewer.close_editor();
        }
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
        if ui.button("新建公告").clicked() {
            viewer.open_editor(GroupAnnouncementDraft::default());
        }
        if let Some(raw) = viewer.raw_response.clone()
            && ui
                .small_button("复制完整响应")
                .on_hover_text("复制公告接口的原始 JSON，便于反馈字段差异")
                .clicked()
        {
            ui.ctx().copy_text(raw);
        }
    });

    if let Some(error) = &viewer.last_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    ui.separator();

    if viewer.editor_open {
        render_editor(ui, viewer);
        return;
    }

    if let Some(fid) = viewer.delete_confirm_fid.clone() {
        let headline = viewer
            .announcements
            .iter()
            .find(|item| item.fid == fid)
            .map(announcement_headline)
            .unwrap_or_else(|| "这条公告".to_string());
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(egui::Color32::YELLOW, format!("确认删除「{headline}」？"));
            if ui.button("确认删除").clicked() {
                viewer.pending_delete_fid = Some(fid);
                viewer.delete_confirm_fid = None;
            }
            if ui.button("取消").clicked() {
                viewer.delete_confirm_fid = None;
            }
        });
    }

    if viewer.announcements.is_empty() {
        if !viewer.loading && viewer.last_error.is_none() {
            ui.weak("这个群没有公告");
        }
        return;
    }

    // 借用期内不能直接改 `viewer`，先把交互结果收集起来，滚动区结束后统一写回。
    let mut toggled_fid = None;
    let mut pending_image_url = None;
    let mut menu_action = None;
    let expanded_fid = viewer.expanded_fid.clone();
    // auto_shrink 关掉后滚动区始终占满窗口，滚动条贴在窗口右边而不是内容右边。
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for announcement in &viewer.announcements {
                let mut expanded = expanded_fid.as_deref() == Some(announcement.fid.as_str());
                let was_expanded = expanded;
                render_announcement(
                    ui,
                    announcement,
                    &mut expanded,
                    &mut pending_image_url,
                    &mut menu_action,
                );
                if expanded != was_expanded {
                    toggled_fid = Some((announcement.fid.clone(), expanded));
                }
            }
        });
    if let Some((fid, expanded)) = toggled_fid {
        viewer.expanded_fid = expanded.then_some(fid);
    }
    if pending_image_url.is_some() {
        viewer.pending_image_url = pending_image_url;
    }
    match menu_action {
        Some(MenuAction::Edit(draft)) => viewer.open_editor(draft),
        Some(MenuAction::RequestDelete(fid)) => viewer.delete_confirm_fid = Some(fid),
        None => {}
    }
}

impl IcaApp {
    /// 打开群公告窗口并立即拉取一次。
    pub fn open_group_announcements(&mut self, bridge_idx: usize, room_id: RoomId) {
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

    pub fn render_group_announcements_window(&mut self, ctx: &egui::Context) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let viewer_state = self.bridge_states[bridge_idx]
            .group_announcement_viewer
            .clone();

        // 配图预览要复用主界面的图片查看器，只能回到主循环里打开。
        let pending_image_url = viewer_state.lock().unwrap().pending_image_url.take();
        if let Some(url) = pending_image_url {
            let sources = viewer_state
                .lock()
                .unwrap()
                .announcements
                .iter()
                .flat_map(|announcement| announcement.images.iter())
                .map(|image| ImageSource::url(image.original_url()))
                .collect::<Vec<_>>();
            self.open_image_viewer_with_sources(ImageSource::url(url), sources);
        }

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

        // 编辑器与删除确认都发生在子视口里，真正的命令发送只能回到主循环。
        let pending_submit = {
            let mut viewer = viewer_state.lock().unwrap();
            std::mem::take(&mut viewer.pending_submit)
        };
        if pending_submit {
            let (room_id, draft) = {
                let viewer = viewer_state.lock().unwrap();
                (viewer.room_id, viewer.draft.clone())
            };
            viewer_state.lock().unwrap().submitting = true;
            let bkn = self.bridge_states[bridge_idx].online_data.bkn;
            if let Err(error) =
                self.bridge_states[bridge_idx].send(IcaCommand::PublishGroupAnnouncement {
                    request_id: 0,
                    room_id,
                    bkn,
                    draft,
                })
            {
                viewer_state
                    .lock()
                    .unwrap()
                    .action_failed(format!("发布请求发送失败: {error}"));
            }
        }

        if let Some(fid) = viewer_state.lock().unwrap().pending_delete_fid.take() {
            viewer_state.lock().unwrap().submitting = true;
            let room_id = viewer_state.lock().unwrap().room_id;
            let bkn = self.bridge_states[bridge_idx].online_data.bkn;
            if let Err(error) =
                self.bridge_states[bridge_idx].send(IcaCommand::DeleteGroupAnnouncement {
                    request_id: 0,
                    room_id,
                    bkn,
                    fid,
                })
            {
                viewer_state
                    .lock()
                    .unwrap()
                    .action_failed(format!("删除请求发送失败: {error}"));
            }
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

                // 刷新与打开原图都要回到主视口才能处理，这里主动唤醒父窗口。
                let viewer = viewport_state.lock().unwrap();
                if viewer.reload_requested || viewer.pending_image_url.is_some() {
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
