use super::SidebarUnread;
use crate::app::test_support::test_app;
use crate::config::chat_groups::ChatGroup;
use crate::ica::types::message::{At, LastMessage};
use crate::ica::types::{RoomId, room::Room};
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Shape};

fn room(room_id: RoomId, priority: u8, unread_count: u64, at: At) -> Room {
    Room {
        room_id,
        room_name: room_id.to_string(),
        index: 0,
        unread_count,
        priority,
        utime: 0,
        users: serde_json::Value::Null,
        at,
        last_message: LastMessage {
            content: None,
            timestamp: None,
            username: None,
            user_id: None,
        },
    }
}

#[test]
fn sidebar_group_badges_share_threshold_and_mentions() {
    let rooms = [
        room(11, 3, 1, At::None),
        room(12, 1, 1, At::None),
        room(-21, 1, 1, At::Bool(true)),
        room(-22, 1, 1, At::All),
        room(-23, 5, 0, At::Bool(true)),
    ];
    let unread = SidebarUnread::from_rooms(&rooms, 3, false);
    let mut personal = ChatGroup::new_empty("私聊");
    personal.include_all_personal = true;
    let mentions = ChatGroup::new("全体", vec![-22]);
    assert!(unread.private && unread.group && unread.in_custom_group(&personal));
    assert_eq!(unread.rooms.len(), 2);
    assert!(unread.rooms.contains(&11) && unread.rooms.contains(&-21));
    assert!(!unread.in_custom_group(&mentions));
    assert!(SidebarUnread::from_rooms(&rooms, 3, true).in_custom_group(&mentions));
}

#[test]
fn drawing_an_idle_sidebar_preserves_the_visible_room_index_cache() {
    let (mut app, _) = test_app();
    app.bridge_states[0].rooms.push(room(-123, 3, 1, At::None));
    assert_eq!(app.visible_room_indices(0), vec![0]);
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.render_left_groups_panel(ui);
    });
    output.textures_delta.clear();
    assert!(app.bridge_states[0].visible_room_indices_cache.is_some());
}

#[test]
fn edit_group_label_stays_on_one_line_and_opens_editor_after_scrolling() {
    let (mut app, _) = test_app();
    app.bridge_states[0].chat_groups.groups = (0..8)
        .map(|index| ChatGroup::new_empty(format!("Team {index}")))
        .collect();
    let ctx = egui::Context::default();
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 190.0))),
                events,
                ..Default::default()
            },
            |ui| app.render_left_groups_panel(ui),
        );
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                Shape::Text(text) if text.galley.job.text == "Edit" => {
                    let label = Rect::from_min_size(text.pos, text.galley.size());
                    shape.clip_rect.contains_rect(label).then(|| {
                        assert_eq!(text.galley.rows.len(), 1);
                        label.center()
                    })
                }
                _ => None,
            })
    };

    assert!(draw(vec![]).is_none(), "长列表顶部不能显示编辑入口");
    draw(vec![Event::PointerMoved(egui::pos2(30.0, 90.0))]);
    draw(vec![
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: egui::Vec2::ZERO,
            phase: egui::TouchPhase::Start,
            modifiers: Modifiers::NONE,
        },
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -1200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ]);
    let pos = draw(vec![]).expect("滚到底后应完整显示单行 Edit");
    draw(vec![Event::PointerMoved(pos)]);
    for pressed in [true, false] {
        draw(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    assert!(app.open_page.chat_group_editor);
}
