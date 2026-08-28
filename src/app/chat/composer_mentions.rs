use crate::app::IcaApp;

const MENTION_RESULTS_MAX_HEIGHT: f32 = 260.0;

#[derive(Default)]
pub(super) struct MentionPickerAction {
    pub selected: Option<(i64, String)>,
    pub refresh: bool,
    pub close: bool,
    pub focus_composer: bool,
}

impl IcaApp {
    pub(super) fn render_mention_picker_overlay(
        &mut self,
        ctx: &egui::Context,
        active_bridge_idx: usize,
        room_id: i64,
        anchor_rect: egui::Rect,
        opened_this_frame: bool,
    ) -> MentionPickerAction {
        let mut action = MentionPickerAction::default();
        let content_rect = ctx.content_rect();
        let max_popup_width = (content_rect.width() - 16.0).max(120.0);
        let popup_width = (anchor_rect.width() + 140.0)
            .clamp(300.0, 460.0)
            .min(max_popup_width);
        let min_popup_x = content_rect.left() + 8.0;
        let max_popup_x = (content_rect.right() - popup_width - 8.0).max(min_popup_x);
        let popup_x = anchor_rect.left().clamp(min_popup_x, max_popup_x);
        let popup_anchor = egui::pos2(popup_x, anchor_rect.top() - 6.0);
        let results_height =
            MENTION_RESULTS_MAX_HEIGHT.min((content_rect.height() * 0.45).max(120.0));

        let area_response = egui::Area::new(egui::Id::new((
            "mention_picker_overlay",
            active_bridge_idx,
            room_id,
        )))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(popup_anchor)
        .constrain_to(content_rect)
        .show(ctx, |ui| {
            ui.set_width(popup_width);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(popup_width - ui.spacing().item_spacing.x);
                ui.horizontal(|ui| {
                    ui.strong("@ 群成员");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("关闭")
                            .on_hover_text("关闭 (Esc)")
                            .clicked()
                        {
                            action.close = true;
                            action.focus_composer = true;
                        }
                        if ui
                            .small_button("刷新")
                            .on_hover_text("刷新群成员")
                            .clicked()
                        {
                            action.refresh = true;
                        }
                    });
                });

                let close_with_keyboard = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                let move_down = ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                });
                let move_up = ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                });
                let confirm_with_keyboard = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                if close_with_keyboard {
                    action.close = true;
                    action.focus_composer = true;
                }

                let mention_search_id =
                    egui::Id::new(("mention_search", active_bridge_idx, room_id));
                let search_response = ui.add_sized(
                    [ui.available_width(), 30.0],
                    egui::TextEdit::singleline(&mut self.mention_search_query)
                        .id(mention_search_id)
                        .hint_text("搜索群名片、昵称或 QQ"),
                );
                if self.mention_search_focus_requested {
                    search_response.request_focus();
                    self.mention_search_focus_requested = false;
                }
                let search_changed = search_response.changed();
                if search_changed {
                    self.mention_selected_index = 0;
                }

                let conversation = self.bridge_states[active_bridge_idx].conversation(room_id);
                let has_members =
                    conversation.is_some_and(|conversation| conversation.group_members_loaded);
                let loading =
                    conversation.is_some_and(|conversation| conversation.loading_group_members);
                let query = self.mention_search_query.trim().to_lowercase();
                let mut entries = vec![(1, "全体成员".to_string())];
                if let Some(members) = conversation.map(|conversation| &conversation.group_members)
                {
                    entries.extend(
                        members
                            .iter()
                            .filter(|member| member.matches_search(&query))
                            .map(|member| {
                                (member.user_id, safe_mention_text(member.display_name()))
                            }),
                    );
                }
                if !query.is_empty() {
                    entries.retain(|(user_id, name)| {
                        *user_id != 1
                            || name.to_lowercase().contains(&query)
                            || "all".contains(&query)
                    });
                }

                if entries.is_empty() {
                    self.mention_selected_index = 0;
                } else {
                    self.mention_selected_index = self
                        .mention_selected_index
                        .min(entries.len().saturating_sub(1));
                    if move_down {
                        self.mention_selected_index =
                            (self.mention_selected_index + 1) % entries.len();
                    }
                    if move_up {
                        self.mention_selected_index = if self.mention_selected_index == 0 {
                            entries.len() - 1
                        } else {
                            self.mention_selected_index - 1
                        };
                    }
                    if confirm_with_keyboard {
                        action.selected = entries.get(self.mention_selected_index).cloned();
                    }
                }

                if loading && !has_members {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("正在加载群成员...");
                    });
                } else if entries.is_empty() {
                    ui.add_space(8.0);
                    ui.weak("没有匹配的群成员");
                    ui.add_space(8.0);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("mention_picker", active_bridge_idx, room_id))
                        .max_height(results_height)
                        .show_rows(ui, 38.0, entries.len(), |ui, rows| {
                            for row in rows {
                                let (user_id, name) = &entries[row];
                                let selected = row == self.mention_selected_index;
                                let response = if *user_id == 1 {
                                    ui.add_sized(
                                        [ui.available_width(), 34.0],
                                        egui::Button::selectable(
                                            selected,
                                            egui::RichText::new("@全体成员  ·  all").size(14.0),
                                        ),
                                    )
                                } else {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Image::new(format!(
                                                "https://q1.qlogo.cn/g?b=qq&nk={user_id}&s=40"
                                            ))
                                            .fit_to_exact_size(egui::vec2(28.0, 28.0))
                                            .corner_radius(14.0),
                                        );
                                        ui.add_sized(
                                            [ui.available_width(), 32.0],
                                            egui::Button::selectable(
                                                selected,
                                                egui::RichText::new(format!(
                                                    "{name}  ·  {user_id}"
                                                ))
                                                .size(14.0),
                                            ),
                                        )
                                    })
                                    .inner
                                };
                                if response.hovered() {
                                    self.mention_selected_index = row;
                                }
                                if selected && (search_changed || move_down || move_up) {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                if response.clicked() {
                                    action.selected = Some((*user_id, name.clone()));
                                }
                            }
                        });
                }

                if has_members {
                    ui.separator();
                    ui.small("方向键选择 | Enter 确认 | Esc 关闭");
                }
            });
        });

        if !opened_this_frame
            && ctx.input(|input| {
                input.pointer.primary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|position| !area_response.response.rect.contains(position))
            })
        {
            action.close = true;
        }

        action
    }
}

fn safe_mention_text(text: &str) -> String {
    const MAX_CHARS: usize = 48;

    let mut output = String::new();
    let mut last_was_space = false;
    let mut truncated = false;

    for ch in text.chars() {
        if output.chars().count() >= MAX_CHARS {
            truncated = true;
            break;
        }

        let normalized = match ch {
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            ch if ch.is_control() => Some(' '),
            ch => Some(ch),
        };

        let Some(ch) = normalized else {
            continue;
        };

        if ch.is_whitespace() {
            if last_was_space {
                continue;
            }
            output.push(' ');
            last_was_space = true;
        } else {
            output.push(ch);
            last_was_space = false;
        }
    }

    let output = output.trim();
    if output.is_empty() {
        return "未命名成员".to_string();
    }

    if truncated {
        format!("{output}…")
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_mention_text;

    #[test]
    fn mention_text_removes_layout_controls() {
        assert_eq!(
            safe_mention_text("Alice\n\u{202E}Bob\tCarol"),
            "Alice Bob Carol"
        );
    }

    #[test]
    fn mention_text_limits_long_names() {
        let sanitized = safe_mention_text("a".repeat(80).as_str());

        assert_eq!(sanitized.chars().count(), 49);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn mention_text_uses_fallback_for_blank_names() {
        assert_eq!(safe_mention_text("\n\t\u{202E}"), "未命名成员");
    }
}
