use crate::app::media::{ImageAction, image_sources_for_messages};
use crate::app::{IcaApp, MessageAction};

use super::message_card::MessageRenderOptions;

impl IcaApp {
    pub(in crate::app) fn render_message_search_window(&mut self, ctx: &egui::Context) {
        let Some(active_bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if !self
            .bridge_states
            .get(active_bridge_idx)
            .is_some_and(|state| state.message_search.open)
        {
            return;
        }

        let (
            room_id,
            room_name,
            mut open,
            mut keyword,
            searched_keyword,
            messages,
            loading,
            has_more,
            last_error,
            self_id,
        ) = {
            let state = &self.bridge_states[active_bridge_idx];
            let search = &state.message_search;
            let Some(room_id) = search.room_id else {
                return;
            };
            (
                room_id,
                search.room_name.clone(),
                search.open,
                search.keyword.clone(),
                search.searched_keyword.clone(),
                search.messages.clone(),
                search.loading,
                search.has_more,
                search.last_error.clone(),
                state.online_data.qqid,
            )
        };

        let mut request_search = false;
        let mut request_more = false;
        let mut pending_action = None;

        egui::Window::new(format!("{} - 搜索聊天记录", room_name))
            .open(&mut open)
            .default_size(egui::vec2(560.0, 640.0))
            .min_size(egui::vec2(360.0, 300.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("关键词");
                    let response = ui.add_sized(
                        [ui.available_width().min(320.0), 0.0],
                        egui::TextEdit::singleline(&mut keyword),
                    );
                    let enter_pressed = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui
                        .add_enabled(!loading, egui::Button::new("搜索"))
                        .clicked()
                        || enter_pressed
                    {
                        request_search = true;
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!("会话: {} ({})", room_name, room_id.abs()));
                    if !searched_keyword.is_empty() {
                        ui.separator();
                        ui.weak(format!("当前结果: {}", searched_keyword));
                    }
                });

                if let Some(error) = &last_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if loading {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("正在搜索...");
                    });
                }

                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt(("message_search_results", active_bridge_idx, room_id))
                    .show(ui, |ui| {
                        if messages.is_empty() {
                            if !searched_keyword.is_empty() && !loading {
                                ui.weak("没有匹配的聊天记录");
                            } else {
                                ui.weak("输入关键词后开始搜索");
                            }
                            return;
                        }

                        for (idx, message) in messages.iter().enumerate() {
                            if idx > 0 {
                                ui.separator();
                            }
                            if let Some(action) = self.render_message_card(
                                ui,
                                room_id,
                                self_id,
                                message,
                                MessageRenderOptions {
                                    show_sender_name: true,
                                    show_separator_before: false,
                                    forward_mode_active: false,
                                    forward_selected: false,
                                },
                            ) {
                                pending_action = Some(action);
                            }
                        }

                        ui.add_space(8.0);
                        if has_more {
                            if ui
                                .add_enabled(!loading, egui::Button::new("加载更多"))
                                .clicked()
                            {
                                request_more = true;
                            }
                        } else {
                            ui.weak("没有更多结果");
                        }
                    });
            });

        if let Some(state) = self.bridge_states.get_mut(active_bridge_idx) {
            state.message_search.open = open;
            state.message_search.keyword = keyword.clone();
        }

        if request_search {
            self.request_message_search(active_bridge_idx, room_id, keyword, 0);
        } else if request_more {
            self.request_message_search(
                active_bridge_idx,
                room_id,
                searched_keyword,
                messages.len(),
            );
        }

        if let Some(action) = pending_action {
            self.handle_message_search_action(ctx, active_bridge_idx, room_id, &messages, action);
        }
    }

    fn handle_message_search_action(
        &mut self,
        ctx: &egui::Context,
        active_bridge_idx: usize,
        search_room_id: i64,
        search_messages: &[crate::ica::types::message::Message],
        action: MessageAction,
    ) {
        match action {
            MessageAction::Reply { room_id, reply } => {
                self.select_active_room(room_id);
                self.queue_reply(room_id, reply);
            }
            MessageAction::Delete {
                room_id,
                message_id,
            } => {
                self.send_delete_message(room_id, message_id);
            }
            MessageAction::ReEdit { room_id, content } => {
                self.select_active_room(room_id);
                self.restore_deleted_message_to_draft(room_id, content);
            }
            MessageAction::SetReveal {
                room_id,
                message_id,
                reveal,
            } => {
                self.set_message_reveal(room_id, message_id, reveal);
            }
            MessageAction::CopyToDraft {
                room_id,
                message_id,
            } => {
                self.select_active_room(room_id);
                self.copy_message_to_draft(room_id, message_id);
            }
            MessageAction::PlusOne {
                room_id,
                message_id,
            } => {
                self.plus_one_message(room_id, message_id);
            }
            MessageAction::ToggleForwardSelection {
                room_id,
                message_id,
            } => {
                self.toggle_forward_message_selection(room_id, message_id);
            }
            MessageAction::StartForward {
                room_id,
                message_id,
            } => {
                self.begin_forward_selection(room_id, message_id, true);
            }
            MessageAction::Image(ImageAction::Open(source)) => {
                let sources = image_sources_for_messages(search_room_id, search_messages);
                self.open_image_viewer_with_sources(source, sources);
            }
            MessageAction::Image(action) => {
                self.handle_image_action(ctx, active_bridge_idx, action);
            }
            MessageAction::ScrollToMessage { msg_id } => {
                self.select_active_room(search_room_id);
                let bridge_state = &mut self.bridge_states[active_bridge_idx];
                let conversation = bridge_state.conversation_mut(search_room_id);
                conversation.scroll_to_message_id = Some(msg_id);
                conversation.scroll_to_message_attempts = 0;
            }
            MessageAction::RenewMessage {
                room_id,
                message_id,
            } => {
                self.send_renew_message(room_id, message_id);
            }
            MessageAction::Poke { room_id, target_id } => {
                self.send_group_poke(room_id, target_id);
            }
        }
    }
}
