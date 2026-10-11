//! 无窗口滚动回归：只使用内存消息和假纹理，不连接 Bridge、不读取用户配置。

use crate::app::chat::message_card::render_message_image;
use crate::app::test_support::test_app;
use crate::app::{IcaApp, media::ImageSource};
use crate::ica::types::message::Message;
use egui::load::{
    LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum PreviewState {
    Pending(Option<egui::Vec2>),
    Ready(egui::Vec2),
    Error(String),
    ChangedToStatic,
}

struct FakeTextures(Mutex<PreviewState>);

impl FakeTextures {
    fn set(&self, state: PreviewState) {
        *self.0.lock().unwrap() = state;
    }
}

impl TextureLoader for FakeTextures {
    fn id(&self) -> &str {
        concat!(module_path!(), "::FakeTextures")
    }
    fn load(
        &self,
        _: &egui::Context,
        uri: &str,
        _: egui::TextureOptions,
        _: SizeHint,
    ) -> TextureLoadResult {
        if !uri.starts_with("bytes://scroll-test") {
            return Err(LoadError::NotSupported);
        }
        match self.0.lock().unwrap().clone() {
            PreviewState::Pending(size) => Ok(TexturePoll::Pending { size }),
            PreviewState::Ready(size) => Ok(TexturePoll::Ready {
                texture: SizedTexture {
                    id: egui::TextureId::User(42),
                    size,
                },
            }),
            PreviewState::Error(message) => Err(LoadError::Loading(message)),
            PreviewState::ChangedToStatic if uri.contains('#') => {
                Err(LoadError::FormatNotSupported {
                    detected_format: None,
                })
            }
            PreviewState::ChangedToStatic => Ok(TexturePoll::Ready {
                texture: SizedTexture {
                    id: egui::TextureId::User(42),
                    size: egui::vec2(200.0, 100.0),
                },
            }),
        }
    }
    fn forget(&self, _: &str) {}
    fn forget_all(&self) {}
    fn byte_size(&self) -> usize {
        0
    }
}

fn context() -> (egui::Context, Arc<FakeTextures>) {
    let ctx = egui::Context::default();
    let textures = Arc::new(FakeTextures(Mutex::new(PreviewState::Pending(None))));
    ctx.add_texture_loader(textures.clone());
    (ctx, textures)
}

fn input(frame: usize, size: egui::Vec2, events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(frame as f64 / 60.0),
        events,
        ..Default::default()
    }
}

fn assert_close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() <= 1.0,
        "{label}: {actual} != {expected}"
    );
}

#[test]
fn image_slot_geometry_survives_loading_error_and_eviction() {
    for gif in [false, true] {
        for (width, height) in [(240.0, 240.0), (96.0, 96.0), (48.0, 240.0)] {
            let (ctx, textures) = context();
            let url = if gif {
                "bytes://scroll-test.gif"
            } else {
                "bytes://scroll-test.png"
            };
            if gif {
                ctx.include_bytes(url, &b"GIF89a"[..]);
            }
            let source = ImageSource::url(url.into());
            let mut baseline = None;
            let states = [
                PreviewState::Pending(None),
                PreviewState::Pending(Some(egui::vec2(800.0, 100.0))),
                PreviewState::Ready(egui::vec2(800.0, 100.0)),
                PreviewState::Ready(egui::vec2(100.0, 800.0)),
                PreviewState::Ready(egui::vec2(400.0, 400.0)),
                PreviewState::Ready(egui::vec2(16.0, 12.0)),
                PreviewState::Error("错误细节非常长，不能撑大消息行。".repeat(80)),
                PreviewState::Error("图片链接已过期".into()),
                PreviewState::Pending(None), // 模拟纹理淘汰后重新加载。
            ];
            for (frame, state) in states.into_iter().cycle().take(90).enumerate() {
                textures.set(state);
                let mut measured = egui::Vec2::ZERO;
                let mut output = ctx.run_ui(
                    input(frame, egui::vec2(width + 16.0, 500.0), vec![]),
                    |ui| {
                        let before = ui.cursor().min.y;
                        let response = ui
                            .scope(|ui| {
                                render_message_image(
                                    ui,
                                    &source,
                                    if gif { "image/gif" } else { "image/png" },
                                    width,
                                    height,
                                )
                            })
                            .response;
                        measured = egui::vec2(response.rect.width(), ui.cursor().min.y - before);
                    },
                );
                output.textures_delta.clear();
                let expected = *baseline.get_or_insert(measured);
                assert_close(measured.y, expected.y, "加载状态改变了图片槽高度");
                assert!(measured.x <= width + 1.0, "窄窗图片槽溢出: {measured:?}");
            }
        }
    }
}

struct ChatFixture {
    app: IcaApp,
    ctx: egui::Context,
    textures: Arc<FakeTextures>,
    frame: usize,
    size: egui::Vec2,
    list_rect: egui::Rect,
    texts: std::collections::HashMap<String, f32>,
    _rx: tokio::sync::mpsc::UnboundedReceiver<crate::ica::IcaCommand>,
}

impl ChatFixture {
    fn new(images: bool) -> Self {
        let (mut app, rx) = test_app();
        app.custom_chat.show_message_avatar = false;
        let conversation = app.bridge_states[0].conversation_mut(-123);
        conversation.no_more_history = true;
        conversation.messages = (0..80).map(|i| {
            let mut message: Message = serde_json::from_value(serde_json::json!({
                "_id": format!("row-{i}"), "senderId": 456, "username": "测试发言人",
                "content": format!("第 {i} 条：用于滚动回归的内存消息。"), "time": 1_700_000_000_000_i64,
            })).unwrap();
            if images {
                message.files.push(crate::ica::types::files::MessageFile {
                    file_type: "image/png".into(), url: "bytes://scroll-test.png".into(),
                    size: None, name: None, fid: None,
                });
            }
            message
        }).collect();
        conversation.scroll_to_bottom = true;
        let (ctx, textures) = context();
        Self {
            app,
            ctx,
            textures,
            frame: 0,
            size: egui::vec2(640.0, 600.0),
            list_rect: egui::Rect::NOTHING,
            texts: Default::default(),
            _rx: rx,
        }
    }

    fn draw(&mut self, events: Vec<egui::Event>) {
        let mut output = self.ctx.run_ui(input(self.frame, self.size, events), |ui| {
            self.app.render_central_panel(ui);
        });
        self.texts.clear();
        self.list_rect = egui::Rect::NOTHING;
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                if text.galley.job.text.starts_with("第 ") && shape.clip_rect.is_positive() {
                    self.list_rect = self.list_rect.union(shape.clip_rect);
                }
                self.texts.insert(text.galley.job.text.clone(), text.pos.y);
            }
        }
        output.textures_delta.clear();
        self.frame += 1;
    }

    fn settle(&mut self) {
        for _ in 0..12 {
            self.draw(vec![]);
        }
    }

    fn scroll_up(&mut self, points: f32) {
        assert!(self.list_rect.is_positive(), "测试必须先绘制实际消息列表");
        self.draw(vec![
            egui::Event::PointerMoved(self.list_rect.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::Vec2::ZERO,
                phase: egui::TouchPhase::Start,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, points),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }

    // 注入必定会被 overscan 重测的误差，避免底部锁测试被帧末自然解锁掩盖。
    fn inflate_row_above_anchor(&mut self) -> (String, f32) {
        let conversation = self.app.bridge_states[0].conversation_mut(-123);
        let offset = conversation.message_scroll_offset.unwrap();
        let anchor = conversation
            .message_row_layouts
            .partition_point(|row| row.top + row.height <= offset);
        assert!(anchor > 0);
        let above = anchor - 1;
        let id = conversation.messages[above].msg_id.clone();
        let height = conversation.message_row_layouts[above].height;
        conversation
            .message_row_heights
            .insert(id.clone(), height + 100.0);
        conversation.message_row_layouts[above].height += 100.0;
        for row in &mut conversation.message_row_layouts[anchor..] {
            row.top += 100.0;
        }
        conversation.message_scroll_offset = Some(offset + 100.0);
        conversation.last_content_height = conversation.last_content_height.map(|h| h + 100.0);
        (id, height)
    }

    fn conversation(&self) -> &crate::app::ConversationState {
        self.app.bridge_states[0].conversation(-123).unwrap()
    }
}

#[test]
fn image_load_cycles_keep_list_extent_and_reading_anchor_stable() {
    let mut fixture = ChatFixture::new(true);
    fixture.settle();
    fixture.scroll_up(2500.0);
    fixture.settle();
    let baseline = fixture.conversation().clone();
    let offset = baseline.message_scroll_offset.unwrap();
    let anchor = baseline
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= offset);
    assert!(anchor > 0 && anchor < baseline.messages.len() - 1);
    let anchor_y = baseline.message_row_layouts[anchor].top - offset;
    let content_height = baseline.last_content_height.unwrap();
    for cycle in 0..60 {
        fixture.textures.set(match cycle % 5 {
            0 => PreviewState::Ready(egui::vec2(800.0, 100.0)),
            1 => PreviewState::Ready(egui::vec2(100.0, 800.0)),
            2 => PreviewState::Error("解码失败".repeat(50)),
            3 => PreviewState::Pending(Some(egui::vec2(800.0, 100.0))),
            _ => PreviewState::Pending(None),
        });
        fixture.draw(vec![]);
        fixture.draw(vec![]);
        let conversation = fixture.conversation();
        assert_close(
            conversation.last_content_height.unwrap(),
            content_height,
            "图片循环改变了滚动条范围",
        );
        assert_eq!(
            conversation.message_row_layouts.len(),
            baseline.message_row_layouts.len()
        );
        assert_close(
            conversation.message_row_layouts[anchor].height,
            baseline.message_row_layouts[anchor].height,
            "图片消息行高改变",
        );
        assert_close(
            conversation.message_row_layouts[anchor + 1].top,
            baseline.message_row_layouts[anchor + 1].top,
            "后续消息坐标改变",
        );
        assert_close(
            conversation.message_row_layouts[anchor].top
                - conversation.message_scroll_offset.unwrap(),
            anchor_y,
            "阅读锚点漂移",
        );
    }
}

#[test]
fn measuring_overscan_rows_preserves_the_visible_message() {
    let mut fixture = ChatFixture::new(false);
    fixture.settle();
    fixture.scroll_up(1600.0);
    fixture.settle();
    let conversation = fixture.app.bridge_states[0].conversation_mut(-123);
    let offset = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= offset);
    assert!(anchor > 1);
    // 模拟上方一条冷消息的行高估算多了 100px；真实绘制必须修正，但不能拖动正在阅读的消息。
    let above = anchor - 1;
    let id = conversation.messages[above].msg_id.clone();
    let old_height = conversation.message_row_layouts[above].height;
    conversation
        .message_row_heights
        .insert(id, old_height + 100.0);
    conversation.message_row_layouts[above].height += 100.0;
    for row in &mut conversation.message_row_layouts[anchor..] {
        row.top += 100.0;
    }
    conversation.message_scroll_offset = Some(offset + 100.0);
    let anchor_y = conversation.message_row_layouts[anchor].top - (offset + 100.0);
    fixture.settle();
    let conversation = fixture.conversation();
    assert_close(
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap(),
        anchor_y,
        "行高校准改变了阅读位置",
    );
}

#[test]
fn wheel_input_cancels_initial_bottom_lock_before_layout_settles() {
    let mut fixture = ChatFixture::new(true);
    fixture.settle();
    fixture.scroll_up(1000.0);
    fixture.settle();
    let (cold_id, expected_height) = fixture.inflate_row_above_anchor();
    fixture.app.bridge_states[0]
        .conversation_mut(-123)
        .scroll_to_bottom = true;
    fixture.scroll_up(300.0);
    assert_close(
        fixture.conversation().message_row_heights[&cold_id],
        expected_height,
        "测试帧必须真实校准冷行，不能走帧末自然解锁",
    );
    assert!(
        !fixture.conversation().scroll_to_bottom,
        "用户向上滚动仍被首屏底部锁占用"
    );
    let conversation = fixture.conversation();
    let after_wheel = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= after_wheel);
    let anchor_y = conversation.message_row_layouts[anchor].top - after_wheel;
    fixture.settle();
    let conversation = fixture.conversation();
    // 冷行校准可能合法改变绝对 offset；用户正在看的消息不得被底部锁吸走。
    assert_close(
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap(),
        anchor_y,
        "停止滚动后被吸回底部",
    );
    assert!(!conversation.near_bottom);
}
#[test]
fn clicking_the_list_cancels_initial_bottom_lock_during_height_calibration() {
    let mut fixture = ChatFixture::new(true);
    fixture.settle();
    fixture.scroll_up(1000.0);
    fixture.settle();
    let (cold_id, expected_height) = fixture.inflate_row_above_anchor();
    let conversation = fixture.conversation();
    let before = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= before);
    let anchor_y = conversation.message_row_layouts[anchor].top - before;
    fixture.app.bridge_states[0]
        .conversation_mut(-123)
        .scroll_to_bottom = true;
    let click = fixture.list_rect.center();
    assert!(fixture.list_rect.contains(click));
    fixture.draw(vec![
        egui::Event::PointerMoved(click),
        egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    assert_close(
        fixture.conversation().message_row_heights[&cold_id],
        expected_height,
        "点击帧必须真实校准冷行，不能靠帧末自然解锁",
    );
    assert!(
        !fixture.conversation().scroll_to_bottom,
        "列表内点击仍被底部锁占用"
    );
    assert!(
        !fixture.conversation().near_bottom,
        "列表点击不应把中部阅读位置吸到底部"
    );
    fixture.draw(vec![egui::Event::PointerButton {
        pos: click,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    fixture.settle();
    let conversation = fixture.conversation();
    assert_close(
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap(),
        anchor_y,
        "点击并松开后阅读位置发生漂移",
    );
}

#[test]
fn narrowing_the_window_preserves_the_message_being_read() {
    let mut fixture = ChatFixture::new(false);
    for message in &mut fixture.app.bridge_states[0].conversation_mut(-123).messages {
        message
            .content
            .push_str(&"较长正文用于测试窗口宽度改变之后换行重新测量是否保持阅读锚点。".repeat(12));
    }
    fixture.settle();
    fixture.scroll_up(2400.0);
    fixture.settle();
    let conversation = fixture.conversation();
    let before = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= before);
    assert!(anchor > 0 && anchor < conversation.messages.len() - 1);
    let anchor_y = conversation.message_row_layouts[anchor].top - before;
    fixture.size.x = 520.0;
    fixture.settle();
    let conversation = fixture.conversation();
    assert_close(
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap(),
        anchor_y,
        "窗口变窄后正在阅读的消息被其它消息替代",
    );
    assert!(!conversation.near_bottom, "改变窗口宽度不应重新吸到底部");
}

#[test]
fn ready_gif_does_not_reload_released_source_and_can_reprobe_a_changed_format() {
    let (ctx, textures) = context();
    let uri = "bytes://scroll-test.gif";
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(uri),
            egui::FrameDurations::new(vec![std::time::Duration::from_millis(50)]),
        )
    });
    textures.set(PreviewState::Ready(egui::vec2(200.0, 100.0)));
    // 故意不提供源 bytes：已就绪动画应直接请求帧，不能因源被释放而回到 Pending/Error。
    assert!(matches!(
        crate::app::chat::try_load_gif_texture(&ctx, uri, Default::default(), Default::default()),
        Some(Ok(TexturePoll::Ready { .. }))
    ));
    textures.set(PreviewState::ChangedToStatic);
    ctx.include_bytes(uri, &b"not a gif anymore"[..]);
    assert!(
        crate::app::chat::try_load_gif_texture(&ctx, uri, Default::default(), Default::default())
            .is_none()
    );
    assert!(
        ctx.data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(uri)))
            .is_none()
    );
}

#[test]
fn small_preview_keeps_natural_size_and_only_image_pixels_are_clickable() {
    for align in [egui::Align::Min, egui::Align::Max] {
        for blank in [false, true] {
            let (ctx, textures) = context();
            textures.set(PreviewState::Ready(egui::vec2(16.0, 12.0)));
            let source = ImageSource::url("bytes://scroll-test.png".into());
            let mut image_rect = egui::Rect::NOTHING;
            let mut slot_rect = egui::Rect::NOTHING;
            for frame in 0..2 {
                let mut output = ctx.run_ui(input(frame, egui::vec2(400.0, 400.0), vec![]), |ui| {
                    slot_rect = ui
                        .scope_builder(
                            egui::UiBuilder::new().layout(egui::Layout::top_down(align)),
                            |ui| render_message_image(ui, &source, "image/png", 240.0, 240.0),
                        )
                        .response
                        .rect;
                });
                for primitive in ctx.tessellate(output.shapes.clone(), output.pixels_per_point) {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive
                        && mesh.texture_id == egui::TextureId::User(42)
                    {
                        image_rect = mesh.calc_bounds();
                    }
                }
                output.textures_delta.clear();
            }
            assert_close(image_rect.width(), 16.0, "小图片不应被放大");
            assert_close(image_rect.height(), 12.0, "小图片不应被拉伸");
            assert_close(slot_rect.height(), 240.0, "父级必须保留固定槽");
            if align == egui::Align::Max {
                assert_close(
                    image_rect.right(),
                    slot_rect.right(),
                    "自身消息的图片应保持右对齐",
                );
            } else {
                assert_close(
                    image_rect.left(),
                    slot_rect.left(),
                    "他人消息的图片应保持左对齐",
                );
            }
            let pos = if blank {
                slot_rect.center()
            } else {
                image_rect.center()
            };
            let mut action = None;
            for (index, pressed) in [true, false].into_iter().enumerate() {
                let mut output = ctx.run_ui(
                    input(
                        2 + index,
                        egui::vec2(400.0, 400.0),
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    ),
                    |ui| {
                        // 每一帧保持同一 UI 层级和 ID，与真实消息的连续绘制一致。
                        action = ui
                            .scope_builder(
                                egui::UiBuilder::new().layout(egui::Layout::top_down(align)),
                                |ui| render_message_image(ui, &source, "image/png", 240.0, 240.0),
                            )
                            .inner;
                    },
                );
                output.textures_delta.clear();
            }
            if blank {
                assert!(action.is_none(), "点击预览槽留白不能打开图片");
            } else {
                assert!(
                    matches!(action, Some(crate::app::media::ImageAction::Open(_))),
                    "图片点击动作失效"
                );
            }
        }
    }
}

#[test]
fn ordinary_mouse_wheel_is_not_consumed_twice_or_pulled_back_after_its_tail() {
    let mut fixture = ChatFixture::new(false);
    fixture.settle();
    fixture.scroll_up(500.0);
    fixture.settle();
    let conversation = fixture.conversation();
    let offset = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= offset);
    let initial_y = conversation.message_row_layouts[anchor].top - offset;
    // 与触摸板不同，普通鼠标 Move 事件会走 egui 的原生平滑滚轮分支。
    fixture.draw(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::Vec2::ZERO,
        phase: egui::TouchPhase::End,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..5 {
        fixture.draw(vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 250.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 100.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
    }
    for _ in 0..40 {
        fixture.draw(vec![]);
    }
    let conversation = fixture.conversation();
    let final_y =
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap();
    assert_close(final_y - initial_y, 500.0, "滚轮步进丢失或重复消费");
    fixture.settle();
    let conversation = fixture.conversation();
    assert_close(
        conversation.message_row_layouts[anchor].top - conversation.message_scroll_offset.unwrap(),
        final_y,
        "滚轮尾帧结束后仍漂移",
    );
}
#[test]
fn history_prepend_keeps_the_same_message_on_screen_in_the_first_frame() {
    let mut fixture = ChatFixture::new(false);
    fixture.settle();
    fixture.scroll_up(1000.0);
    fixture.settle();
    let conversation = fixture.conversation();
    let offset = conversation.message_scroll_offset.unwrap();
    let anchor = conversation
        .message_row_layouts
        .partition_point(|row| row.top + row.height <= offset)
        + 1;
    let anchor_id = conversation.messages[anchor].msg_id.clone();
    let anchor_text = conversation.messages[anchor].content.clone();
    let before_y = fixture.texts[&anchor_text];
    let event = crate::ica::BridgeEventKind::AppendOlderMessages(serde_json::json!([{
        "roomId": -123, "messages": [
            { "_id": "older-1", "senderId": 456, "username": "测试发言人", "content": "更早的内存消息", "time": 1_699_900_000_000_i64 },
            { "_id": "older-2", "senderId": 456, "username": "测试发言人", "content": "第二条更早的内存消息", "time": 1_699_900_000_000_i64 }
        ]
    }]));
    IcaApp::apply_bridge_event(&mut fixture.app.bridge_states[0], &event);
    assert_eq!(
        fixture.conversation().messages.len(),
        82,
        "历史事件必须实际前插消息"
    );
    assert!(fixture.conversation().prepend_scroll_fix);
    for _ in 0..12 {
        fixture.draw(vec![]);
        let actual_y = *fixture
            .texts
            .get(&anchor_text)
            .unwrap_or_else(|| panic!("前插历史使阅读消息离开渲染区域: {anchor_id}"));
        assert_close(actual_y, before_y, "前插历史改变了当前消息的屏幕位置");
    }
}
