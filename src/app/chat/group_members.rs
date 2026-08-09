use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::IcaApp;
use crate::app::state::{GroupBanConfirmation, GroupMember, GroupMemberFilter};
use crate::ica::{GROUP_BAN_MAX_DURATION, IcaCommand};

const BAN_PRESETS: [(&str, u64); 5] = [
    ("10 分钟", 10 * 60),
    ("1 小时", 60 * 60),
    ("1 天", 24 * 60 * 60),
    ("7 天", 7 * 24 * 60 * 60),
    ("30 天", GROUP_BAN_MAX_DURATION),
];

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if days > 0 {
        format!("{days} 天 {hours} 小时")
    } else if hours > 0 {
        format!("{hours} 小时 {minutes} 分钟")
    } else if minutes > 0 {
        format!("{minutes} 分钟 {seconds} 秒")
    } else {
        format!("{seconds} 秒")
    }
}

fn queue_confirmation(
    pending: &mut Option<GroupBanConfirmation>,
    room_id: i64,
    member: &GroupMember,
    duration: u64,
) {
    *pending = Some(GroupBanConfirmation {
        room_id,
        target_id: member.user_id,
        target_name: member.display_name().to_string(),
        duration,
    });
}

impl IcaApp {
    pub fn render_group_members_panel(&mut self, ui: &mut egui::Ui) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            self.group_member_panel.open = false;
            return;
        };
        let Some(room_id) = self.bridge_states[bridge_idx]
            .selected_room_id
            .filter(|room_id| *room_id < 0)
        else {
            self.group_member_panel.open = false;
            return;
        };
        if !self.group_member_panel.open {
            return;
        }

        let self_id = self.bridge_states[bridge_idx].online_data.qqid;
        let conversation = self.bridge_states[bridge_idx].conversation(room_id);
        let members = conversation
            .map(|conversation| conversation.group_members.clone())
            .unwrap_or_default();
        let loaded = conversation.is_some_and(|conversation| conversation.group_members_loaded);
        let loading = conversation.is_some_and(|conversation| conversation.loading_group_members);
        let now = current_unix_timestamp();
        let muted_count = members
            .iter()
            .filter(|member| member.is_muted_at(now))
            .count();
        let actor = members
            .iter()
            .find(|member| member.user_id == self_id)
            .cloned();
        let mut request_refresh = false;

        egui::Panel::right("group_members_panel")
            .resizable(true)
            .default_size(320.0)
            .min_size(260.0)
            .max_size(520.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("群成员");
                    if loading {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new("×"))
                            .on_hover_text("关闭群成员面板")
                            .clicked()
                        {
                            self.group_member_panel.open = false;
                            self.group_member_panel.confirmation = None;
                        }
                    });
                });

                ui.horizontal(|ui| {
                    ui.add_sized(
                        [ui.available_width() - 36.0, 28.0],
                        egui::TextEdit::singleline(&mut self.group_member_panel.search_query)
                            .hint_text("搜索群名片、昵称或 QQ"),
                    );
                    if ui
                        .add_sized([28.0, 28.0], egui::Button::new("↻"))
                        .on_hover_text("刷新群成员")
                        .clicked()
                    {
                        request_refresh = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.group_member_panel.filter,
                        GroupMemberFilter::All,
                        format!("全部 {}", members.len()),
                    );
                    ui.selectable_value(
                        &mut self.group_member_panel.filter,
                        GroupMemberFilter::Muted,
                        format!("禁言中 {muted_count}"),
                    );
                });

                if let Some(error) = &self.group_member_panel.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.separator();

                if members.is_empty() {
                    if loading {
                        ui.weak("正在加载群成员…");
                    } else if loaded {
                        ui.weak("群成员列表为空");
                    } else {
                        ui.weak("尚未加载群成员");
                    }
                }

                let query = self.group_member_panel.search_query.trim().to_lowercase();
                let member_filter = self.group_member_panel.filter;
                egui::ScrollArea::vertical()
                    .id_salt(("group_members", bridge_idx, room_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for member in members.iter().filter(|member| {
                            member.matches_search(&query)
                                && (member_filter == GroupMemberFilter::All
                                    || member.is_muted_at(now))
                        }) {
                            ui.horizontal(|ui| {
                                let avatar = egui::Image::from_uri(format!(
                                    "https://q1.qlogo.cn/g?b=qq&nk={}&s=100",
                                    member.user_id
                                ))
                                .fit_to_exact_size(egui::vec2(38.0, 38.0))
                                .corner_radius(4.0);
                                ui.add(avatar);
                                ui.vertical(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.strong(member.display_name());
                                        if let Some(role) = member.role_label() {
                                            ui.colored_label(egui::Color32::LIGHT_BLUE, role);
                                        }
                                    });
                                    ui.weak(format!("QQ {}", member.user_id));
                                    if member.is_muted_at(now) {
                                        ui.colored_label(
                                            egui::Color32::YELLOW,
                                            format!(
                                                "剩余 {}",
                                                format_duration(
                                                    member.remaining_mute_seconds_at(now)
                                                )
                                            ),
                                        );
                                    }
                                });
                            });

                            let denial = GroupMember::moderation_denial_reason(
                                actor.as_ref(),
                                member,
                                self_id,
                            );
                            let controls = ui.add_enabled_ui(denial.is_none(), |ui| {
                                ui.horizontal(|ui| {
                                    if member.is_muted_at(now) && ui.button("解除禁言").clicked()
                                    {
                                        queue_confirmation(
                                            &mut self.group_member_panel.confirmation,
                                            room_id,
                                            member,
                                            0,
                                        );
                                    }
                                    ui.menu_button("禁言", |ui| {
                                        for (label, duration) in BAN_PRESETS {
                                            if ui.button(label).clicked() {
                                                queue_confirmation(
                                                    &mut self.group_member_panel.confirmation,
                                                    room_id,
                                                    member,
                                                    duration,
                                                );
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                        ui.label("自定义秒数");
                                        ui.add_sized(
                                            [128.0, 24.0],
                                            egui::TextEdit::singleline(
                                                &mut self.group_member_panel.custom_duration,
                                            ),
                                        );
                                        if ui.button("确认时长").clicked() {
                                            match self
                                                .group_member_panel
                                                .custom_duration
                                                .trim()
                                                .parse::<u64>()
                                            {
                                                Ok(duration)
                                                    if (1..=GROUP_BAN_MAX_DURATION)
                                                        .contains(&duration) =>
                                                {
                                                    self.group_member_panel.error = None;
                                                    queue_confirmation(
                                                        &mut self.group_member_panel.confirmation,
                                                        room_id,
                                                        member,
                                                        duration,
                                                    );
                                                    ui.close();
                                                }
                                                _ => {
                                                    self.group_member_panel.error = Some(format!(
                                                        "自定义禁言秒数必须在 1..={} 之间",
                                                        GROUP_BAN_MAX_DURATION
                                                    ));
                                                }
                                            }
                                        }
                                    });
                                });
                            });
                            if let Some(reason) = denial {
                                controls.response.on_hover_text(reason);
                            }
                            ui.separator();
                        }
                    });
            });

        if request_refresh {
            self.request_group_members(bridge_idx, room_id, true);
        }
    }

    pub(crate) fn render_group_ban_confirmation(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = self.group_member_panel.confirmation.clone() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        let title = if confirmation.duration == 0 {
            "确认解除禁言"
        } else {
            "确认禁言"
        };

        egui::Window::new(title)
            .id(egui::Id::new("group_ban_confirmation"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{}（QQ {}）",
                    confirmation.target_name, confirmation.target_id
                ));
                if confirmation.duration == 0 {
                    ui.label("解除该成员的群禁言？");
                } else {
                    ui.label(format!(
                        "禁言时长：{}（{} 秒）",
                        format_duration(confirmation.duration),
                        confirmation.duration
                    ));
                }
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    if ui.button("确认").clicked() {
                        confirm = true;
                    }
                });
            });

        if cancel {
            self.group_member_panel.confirmation = None;
        } else if confirm {
            let result = self
                .active_bridge_idx
                .and_then(|bridge_idx| self.bridge_states.get(bridge_idx))
                .ok_or_else(|| "当前没有可用 bridge".to_string())
                .and_then(|session| {
                    session.send(IcaCommand::SetGroupBan {
                        room_id: confirmation.room_id,
                        target_id: confirmation.target_id,
                        duration: confirmation.duration,
                    })
                });
            self.group_member_panel.confirmation = None;
            if let Err(error) = result
                && let Some(state) = self.active_bridge_state_mut()
            {
                tracing::warn!(error = %error, "发送群管理请求失败");
                state.last_error = Some(format!("群管理请求发送失败: {error}"));
            }
        }
    }
}
