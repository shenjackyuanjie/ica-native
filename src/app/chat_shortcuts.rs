//! 主窗口的会话导航。独立聊天 viewport 固定房间，不消费这里的快捷键。
//!
//! 在更新输入法状态之后、绘制任何控件之前调用 `handle_chat_navigation_shortcuts`。
//! 普通 Tab 只允许主聊天区无控件焦点的情况；编辑器、按钮、搜索框仍使用原有 Tab。

#[cfg(test)]
#[path = "tests/chat_shortcuts.rs"]
mod tests;

use crate::app::{CompactChatPanel, IcaApp, SelectedChatGroup};
use crate::config::ChatGroups;
use crate::ica::types::{RoomId, room::Room};
use egui::{Event, Key, ViewportId};

/// 会话列表和快捷键共用筛选、排序，避免搜索、分类和置顶规则各自演化。
pub fn filtered_sorted_room_indices(
    rooms: &[Room],
    groups: &ChatGroups,
    selected_group: &SelectedChatGroup,
    disable_groups: bool,
    search: &str,
) -> Vec<usize> {
    let query = search.trim().to_uppercase();
    let mut indices: Vec<_> = rooms
        .iter()
        .enumerate()
        .filter(|(_, room)| {
            disable_groups
                || match selected_group {
                    SelectedChatGroup::All => true,
                    SelectedChatGroup::Group => room.room_id < 0,
                    SelectedChatGroup::Private => room.room_id > 0,
                    SelectedChatGroup::Custom(index) => {
                        groups.groups.get(*index).is_some_and(|group| {
                            group.rooms.contains(&room.room_id)
                                || (group.include_all_personal && room.room_id > 0)
                        })
                    }
                }
        })
        .filter(|(_, room)| {
            query.is_empty()
                || room.room_name.to_uppercase().contains(&query)
                || room.room_id.to_string().contains(&query)
        })
        .map(|(index, _)| index)
        .collect();
    // 保持列表原有的稳定排序：先置顶，再按最后活跃时间倒序，同值保持 Bridge 顺序。
    indices.sort_by(|&a, &b| {
        (rooms[b].index > 0)
            .cmp(&(rooms[a].index > 0))
            .then(rooms[b].utime.cmp(&rooms[a].utime))
    });
    indices
}

/// 主窗口支持的会话导航快捷键语义。
///
/// 由 `chat_shortcut` 从 egui 按键事件解析而来，`shortcut_room` 再据此决定目标会话。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatShortcut {
    /// 切换到上一个会话（Ctrl+Shift+Tab 或 Alt+↑）。
    Previous,
    /// 切换到下一个会话（Ctrl+Tab 或 Alt+↓）。
    Next,
    /// 跳到下一条未读会话（裸 Tab）。
    Unread,
    /// 切换到指定聊天分组（Ctrl+数字键）。
    Group(SelectedChatGroup),
}

/// 把单个 egui 按键事件解析成导航语义，非导航键返回 `None`。
///
/// 只认声明好的 Ctrl/Alt 组合，刻意排除 AltGr、Ctrl+Alt 与 mac Command，避免与
/// 系统或输入法快捷键冲突。
fn chat_shortcut(event: &Event) -> Option<ChatShortcut> {
    let Event::Key {
        key,
        pressed: true,
        repeat: false,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    // 只匹配所声明的 Ctrl/Alt 组合，避免把 AltGr、Ctrl+Alt 或系统 Command 当成导航。
    if modifiers.mac_cmd || (modifiers.command && !modifiers.ctrl) {
        return None;
    }
    if modifiers.ctrl && !modifiers.alt {
        if *key == Key::Tab {
            return Some(if modifiers.shift {
                ChatShortcut::Previous
            } else {
                ChatShortcut::Next
            });
        }
        if modifiers.shift {
            return None;
        }
        let group = match key {
            Key::Num1 => SelectedChatGroup::All,
            Key::Num2 => SelectedChatGroup::Group,
            Key::Num3 => SelectedChatGroup::Private,
            Key::Num4 => SelectedChatGroup::Custom(0),
            Key::Num5 => SelectedChatGroup::Custom(1),
            Key::Num6 => SelectedChatGroup::Custom(2),
            Key::Num7 => SelectedChatGroup::Custom(3),
            Key::Num8 => SelectedChatGroup::Custom(4),
            Key::Num9 => SelectedChatGroup::Custom(5),
            _ => return None,
        };
        return Some(ChatShortcut::Group(group));
    }
    if modifiers.alt && !modifiers.ctrl && !modifiers.shift {
        return match key {
            Key::ArrowUp => Some(ChatShortcut::Previous),
            Key::ArrowDown => Some(ChatShortcut::Next),
            _ => None,
        };
    }
    (*key == Key::Tab && modifiers.is_none()).then_some(ChatShortcut::Unread)
}

/// 根据导航语义与当前可见会话列表，算出应当跳转到的会话 ID。
///
/// `Next`/`Previous` 在可见列表内循环；`Unread` 按未读优先级 5→1 查找且不跨出当前
/// 筛选；`Group` 不指向具体会话，返回 `None`。
fn shortcut_room(
    shortcut: &ChatShortcut,
    rooms: &[Room],
    visible_indices: &[usize],
    selected: Option<RoomId>,
) -> Option<RoomId> {
    match shortcut {
        ChatShortcut::Next | ChatShortcut::Previous => {
            let first = *visible_indices.first()?;
            let current = visible_indices
                .iter()
                .position(|&index| Some(rooms[index].room_id) == selected);
            let index = match (current, shortcut) {
                (Some(index), ChatShortcut::Next) => {
                    visible_indices[(index + 1) % visible_indices.len()]
                }
                (Some(index), _) => {
                    visible_indices[(index + visible_indices.len() - 1) % visible_indices.len()]
                }
                // 尚未选房，或当前房间被筛选掉，两种方向都从当前列表首项开始。
                (None, _) => first,
            };
            Some(rooms[index].room_id)
        }
        ChatShortcut::Unread => {
            // 与 Socket.IO 客户端一致按 5→1 查找；同级按当前列表顺序，绝不跨出空筛选。
            (1..=5).rev().find_map(|priority| {
                visible_indices
                    .iter()
                    .map(|&index| &rooms[index])
                    .find(|room| room.unread_count > 0 && room.priority == priority)
                    .map(|room| room.room_id)
            })
        }
        ChatShortcut::Group(_) => None,
    }
}

/// 会话导航快捷键的处理逻辑。
impl IcaApp {
    /// 当前是否有会拦截按键的浮层/窗口打开（设置页、图片查看器、各类选择器与工具页等）。
    ///
    /// 命中时任一导航快捷键都不生效，避免在浮层里误切会话。
    fn chat_navigation_has_overlay(&self) -> bool {
        let page = &self.open_page;
        let page_open = [
            page.verify_message,
            page.about,
            page.settings,
            page.noticer_settings,
            page.notify_level,
            page.custom_chat_ica,
            page.custom_chat_extra,
            page.online_status,
            page.socketio_status,
            page.raw_config,
            page.chat_group_editor,
            page.contacts,
            page.group_tools,
            page.account_tools,
            page.file_tools,
            page.message_tools,
            page.room_tools,
            page.auto_sign,
            page.relation_network,
        ]
        .into_iter()
        .any(|open| open);
        page_open
            || self.image_viewer.is_some()
            || self.show_mention_picker
            || self.show_face_picker
            || self.group_file_panel.open
            || self.group_member_panel.confirmation.is_some()
            || self.active_bridge_state().is_some_and(|state| {
                state.forward_target_picker_open
                    || state.message_search.open
                    || state.member_history.open
                    || state
                        .forward_viewer
                        .lock()
                        .map(|viewer| viewer.open)
                        .unwrap_or(true)
                    || state
                        .group_announcement_viewer
                        .lock()
                        .map(|viewer| viewer.open)
                        .unwrap_or(true)
            })
    }

    /// 必须在本帧控件绘制之前调用，保证消费 Tab 时可同时取消 egui 的焦点移动。
    /// 对独立聊天、设置、图片等非 ROOT viewport 是无副作用的空操作。
    pub fn handle_chat_navigation_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != ViewportId::ROOT
            || !ctx
                .input(|input| input.focused && input.viewport().focused.unwrap_or(input.focused))
            || self.ime_composing
            || self.ime_event_this_frame
            || egui::Popup::is_any_open(ctx)
            || ctx.memory(|memory| memory.top_modal_layer().is_some())
            || self.chat_navigation_has_overlay()
        {
            return;
        }
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            return;
        };
        let selected = state.selected_room_id;
        let composer_id =
            selected.map(|room| egui::Id::new(("message_composer", bridge_idx, room)));
        let focused = ctx.memory(|memory| memory.focused());
        let list_row_focused = focused.is_some_and(|focused| {
            state
                .rooms
                .iter()
                .any(|room| focused == egui::Id::new(("chat_list_row", bridge_idx, room.room_id)))
        });
        if focused.is_some() && focused != composer_id && !list_row_focused {
            return;
        }
        let Some((event_index, shortcut)) =
            ctx.input(|input| {
                input.events.iter().enumerate().find_map(|(index, event)| {
                    chat_shortcut(event).map(|shortcut| (index, shortcut))
                })
            })
        else {
            return;
        };
        if shortcut == ChatShortcut::Unread
            && (focused.is_some()
                || selected.is_none()
                || self.active_chat_is_detached()
                || self.group_member_panel.open
                || (self.uses_compact_chat_layout(ctx)
                    && self.compact_chat_panel != CompactChatPanel::Chat))
        {
            return;
        }
        match &shortcut {
            ChatShortcut::Group(group) => {
                if !self.select_active_chat_group(group.clone()) {
                    return;
                }
            }
            _ => {
                let visible = self.visible_room_indices(bridge_idx);
                let Some(room_id) = shortcut_room(
                    &shortcut,
                    &self.bridge_states[bridge_idx].rooms,
                    &visible,
                    selected,
                ) else {
                    return;
                };
                self.select_active_room_preserving_filter(room_id);
                if focused.is_some() && focused == composer_id && !self.active_chat_is_detached() {
                    ctx.memory_mut(|memory| {
                        memory.request_focus(egui::Id::new((
                            "message_composer",
                            bridge_idx,
                            room_id,
                        )))
                    });
                }
            }
        }
        // 只消费确实执行的按下事件，不误吞未匹配的修饰键、重复事件或普通控件 Tab。
        ctx.input_mut(|input| {
            input.events.remove(event_index);
        });
        if shortcut == ChatShortcut::Unread {
            ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        }
    }
}
