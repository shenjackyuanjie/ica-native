//! 通知栏回归只绘制内存中的应用，不读取真实配置、聊天或连接 Bridge。
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2};

use super::actions::avatar_action_tests::test_app;
use crate::app::IcaApp;
use crate::ica::IcaCommand;

struct PaintedText {
    text: String,
    rect: Rect,
}

struct NoticeFixture {
    app: IcaApp,
    ctx: egui::Context,
    frame: usize,
    size: Vec2,
    top_bottom: f32,
    texts: Vec<PaintedText>,
    _rx: tokio::sync::mpsc::UnboundedReceiver<IcaCommand>,
}

impl NoticeFixture {
    fn new() -> Self {
        let (app, rx) = test_app();
        Self {
            app,
            ctx: egui::Context::default(),
            frame: 0,
            size: egui::vec2(900.0, 600.0),
            top_bottom: 0.0,
            texts: Vec::new(),
            _rx: rx,
        }
    }

    fn draw(&mut self, events: Vec<Event>) {
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.frame as f64 / 60.0),
                events,
                ..Default::default()
            },
            |ui| {
                self.app.render_top_panel(ui);
                self.top_bottom = ui.available_rect_before_wrap().top();
                if self.app.active_chat_is_detached() {
                    self.app.render_detached_chat_placeholder(ui);
                } else {
                    self.app.render_central_panel(ui);
                }
            },
        );
        self.texts = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(PaintedText {
                        text: text.galley.job.text.clone(),
                        rect: Rect::from_min_size(text.pos, text.galley.size()),
                    })
                } else {
                    None
                }
            })
            .collect();
        output.textures_delta.clear();
        self.frame += 1;
    }

    fn settle(&mut self) {
        self.draw(vec![]);
        self.draw(vec![]);
    }

    fn text(&self, expected: &str) -> &PaintedText {
        self.texts
            .iter()
            .find(|text| text.text == expected)
            .unwrap_or_else(|| panic!("未绘制提示：{expected}"))
    }

    fn assert_count(&self, expected: &str, count: usize) {
        assert_eq!(
            self.texts
                .iter()
                .filter(|text| text.text == expected)
                .count(),
            count,
            "提示绘制次数不符：{expected}"
        );
    }

    fn clear(&mut self, message: &str) {
        let message_y = self.text(message).rect.center().y;
        let pos = self
            .texts
            .iter()
            .filter(|text| text.text == "清除")
            .min_by(|a, b| {
                (a.rect.center().y - message_y)
                    .abs()
                    .total_cmp(&(b.rect.center().y - message_y).abs())
            })
            .expect("统一通知栏应有清除按钮")
            .rect
            .center();
        self.draw(vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.draw(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.settle();
    }
}

#[test]
fn notice_bar_media_moves_below_chat_header_without_growing_top_panel() {
    let mut fixture = NoticeFixture::new();
    fixture.settle();
    let menu_bottom = fixture.top_bottom;
    fixture.app.media_notice = Some("收藏表情目录不可用：测试目录".into());
    fixture.settle();

    assert!(
        (fixture.top_bottom - menu_bottom).abs() < 0.1,
        "应用提示不能继续占用窗口顶栏"
    );
    fixture.assert_count("收藏表情目录不可用：测试目录", 1);
    assert!(
        fixture.text("收藏表情目录不可用：测试目录").rect.top()
            >= fixture.text("-123").rect.bottom(),
        "应用提示应位于聊天标题下方"
    );
    fixture.assert_count("关闭", 0);
    fixture.assert_count("清除", 1);
}

#[test]
fn notice_bar_error_priority_and_clearing_keep_media_and_bridge_independent() {
    let mut fixture = NoticeFixture::new();
    fixture.app.media_error = Some("应用错误".into());
    fixture.app.media_notice = Some("应用提示".into());
    fixture.app.bridge_states[0].last_error = Some("连接错误".into());
    fixture.app.bridge_states[0].last_notice = Some("连接提示".into());
    fixture.settle();
    fixture.assert_count("应用错误", 1);
    fixture.assert_count("应用提示", 0);
    fixture.assert_count("连接错误", 1);
    fixture.assert_count("连接提示", 1);
    assert_eq!(
        fixture.app.bridge_states[0].status_history.len(),
        2,
        "应用提示不应写进 Bridge 诊断历史"
    );

    fixture.clear("应用错误");
    fixture.assert_count("应用错误", 0);
    fixture.assert_count("应用提示", 1);
    fixture.assert_count("连接错误", 1);
    fixture.assert_count("连接提示", 1);
    assert!(fixture.app.media_error.is_none());

    fixture.clear("连接错误");
    fixture.assert_count("连接错误", 0);
    fixture.assert_count("连接提示", 1);
    fixture.assert_count("应用提示", 1);
    assert!(fixture.app.bridge_states[0].last_error.is_none());

    fixture.clear("连接提示");
    fixture.assert_count("连接提示", 0);
    fixture.assert_count("应用提示", 1);
    assert!(fixture.app.bridge_states[0].last_notice.is_none());
    assert_eq!(
        fixture.app.bridge_states[0].status_history.len(),
        2,
        "清除当前提示不能连带清空诊断历史"
    );

    fixture.clear("应用提示");
    fixture.assert_count("应用提示", 0);
    assert!(fixture.app.media_notice.is_none());
    fixture.assert_count("清除", 0);
}

#[test]
fn notice_bar_remains_visible_and_clearable_without_a_selected_room_or_bridge() {
    for no_bridge in [false, true] {
        let mut fixture = NoticeFixture::new();
        fixture.app.bridge_states[0].selected_room_id = None;
        if no_bridge {
            fixture.app.active_bridge_idx = None;
        }
        fixture.app.media_error = Some("未配置时仍需显示的错误".into());
        fixture.settle();
        fixture.assert_count("未配置时仍需显示的错误", 1);
        assert!(fixture.text("未配置时仍需显示的错误").rect.top() >= fixture.top_bottom);
        fixture.clear("未配置时仍需显示的错误");
        fixture.assert_count("未配置时仍需显示的错误", 0);
        assert!(fixture.app.media_error.is_none());
    }
}

#[test]
fn notice_bar_detached_chat_placeholder_uses_the_same_notifications() {
    let mut fixture = NoticeFixture::new();
    fixture.app.bridge_states[0].detached_room_ids.insert(-123);
    fixture.app.media_notice = Some("主窗口应用提示".into());
    fixture.app.bridge_states[0].last_error = Some("主窗口连接错误".into());
    fixture.settle();
    fixture.assert_count("此会话已在独立窗口打开，关闭该窗口后可在这里继续聊天。", 1);
    fixture.assert_count("主窗口应用提示", 1);
    fixture.assert_count("主窗口连接错误", 1);
    fixture.clear("主窗口应用提示");
    fixture.assert_count("主窗口应用提示", 0);
    fixture.assert_count("主窗口连接错误", 1);
}

#[test]
fn notice_bar_long_media_notice_wraps_inside_narrow_chat_panel() {
    let mut fixture = NoticeFixture::new();
    fixture.size = egui::vec2(420.0, 600.0);
    let message = "收藏表情目录不可用；请检查共享目录和回退目录。".repeat(5);
    fixture.app.media_notice = Some(message.clone());
    fixture.settle();
    fixture.assert_count(&message, 1);
    let notice = fixture.text(&message);
    assert!(notice.rect.top() >= fixture.text("-123").rect.bottom());
    assert!(
        notice.rect.width() <= fixture.size.x - 16.0 + 1.0,
        "长提示应在聊天面板内换行"
    );
    assert!(notice.rect.height() > 25.0, "测试必须覆盖多行提示");
    assert!(
        fixture.text("清除").rect.right() <= fixture.size.x,
        "清除按钮不能被挤出窄窗"
    );
}
