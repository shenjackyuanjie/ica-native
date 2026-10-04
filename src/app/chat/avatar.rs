//! 头像的共用交互：只产生已有消费者能够执行的操作，不负责下载或成员权限判断。

use crate::app::MessageAction;
use crate::app::media::{ImageAction, ImageSource};
use crate::ica::client::poke_target;
use crate::ica::types::RoomId;

pub struct AvatarTarget<'a> {
    pub room_id: RoomId,
    pub user_id: i64,
    pub name: &'a str,
    pub image_url: &'a str,
    /// 转接消息的虚拟发送者不能作为 QQ 用户执行提及、私聊或管理操作。
    pub is_qq_user: bool,
}

/// 仅私聊顶栏显示可戳对方的头像；群头像不能作为成员目标。
pub fn render_private_header_avatar(
    ui: &mut egui::Ui,
    room_id: RoomId,
    name: &str,
) -> Option<MessageAction> {
    if room_id <= 0 {
        return None;
    }
    let image_url = format!("https://q1.qlogo.cn/g?b=qq&nk={room_id}&s=140");
    let response = ui.add(
        egui::Image::from_uri(image_url.clone())
            .fit_to_exact_size(egui::vec2(32.0, 32.0))
            .corner_radius(16.0)
            .sense(egui::Sense::click()),
    );
    handle_avatar_response(
        response,
        AvatarTarget {
            room_id,
            user_id: room_id,
            name,
            image_url: &image_url,
            is_qq_user: true,
        },
    )
}

fn full_size_avatar_url(target: &AvatarTarget<'_>) -> String {
    let thumbnail = format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=140", target.user_id);
    if target.image_url == thumbnail {
        format!("https://q1.qlogo.cn/g?b=qq&nk={}&s=0", target.user_id)
    } else {
        target.image_url.to_string()
    }
}

/// 使用图片自身的点击响应，不在绘制结束后覆盖可交互的子控件。
/// 群消息与私聊顶栏共用此入口，新增头像位置也可直接复用。
pub fn handle_avatar_response(
    response: egui::Response,
    target: AvatarTarget<'_>,
) -> Option<MessageAction> {
    let mut action = None;
    let user_actions = target.is_qq_user
        && target.user_id > 0
        && poke_target(target.room_id, target.user_id).is_some();
    let response = if user_actions {
        response.on_hover_text("双击戳一戳，右键打开头像菜单")
    } else {
        response.on_hover_text("右键打开头像菜单")
    };
    if user_actions && response.double_clicked() {
        action = Some(MessageAction::Poke {
            room_id: target.room_id,
            target_id: target.user_id,
        });
    }
    response.context_menu(|ui| {
        if user_actions {
            if target.room_id < 0 && ui.button("@TA").clicked() {
                action = Some(MessageAction::MentionSender {
                    room_id: target.room_id,
                    target_id: target.user_id,
                    name: target.name.to_string(),
                });
                ui.close();
            }
            if ui.button("发起私聊").clicked() {
                action = Some(MessageAction::StartPrivateChat {
                    target_id: target.user_id,
                    name: target.name.to_string(),
                });
                ui.close();
            }
            if ui.button("戳一戳").clicked() {
                action = Some(MessageAction::Poke {
                    room_id: target.room_id,
                    target_id: target.user_id,
                });
                ui.close();
            }
            ui.separator();
        }
        if !target.image_url.is_empty() {
            let source = ImageSource::url(full_size_avatar_url(&target));
            for (label, image_action) in [
                ("查看头像", ImageAction::Open(source.clone())),
                ("复制头像 URL", ImageAction::CopyUrl(source.clone())),
                ("保存头像", ImageAction::SaveAs(source)),
            ] {
                if ui.button(label).clicked() {
                    action = Some(MessageAction::Image(image_action));
                    ui.close();
                }
            }
        }
        if user_actions && target.room_id < 0 {
            ui.separator();
            if ui.button("查看发言记录").clicked() {
                action = Some(MessageAction::MemberHistory {
                    room_id: target.room_id,
                    target_id: target.user_id,
                    name: target.name.to_string(),
                });
                ui.close();
            }
            if ui.button("群成员管理").clicked() {
                action = Some(MessageAction::ManageMember {
                    room_id: target.room_id,
                    target_id: target.user_id,
                });
                ui.close();
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_actions_use_full_size_qq_image_but_preserve_custom_sources() {
        let target = AvatarTarget {
            room_id: -123,
            user_id: 456,
            name: "测试成员",
            image_url: "https://q1.qlogo.cn/g?b=qq&nk=456&s=140",
            is_qq_user: true,
        };
        assert_eq!(
            full_size_avatar_url(&target),
            "https://q1.qlogo.cn/g?b=qq&nk=456&s=0"
        );
        let custom_url = "https://example.invalid/avatar.png?size=140";
        assert_eq!(
            full_size_avatar_url(&AvatarTarget {
                image_url: custom_url,
                ..target
            }),
            custom_url,
        );
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    #[test]
    fn private_header_avatar_double_click_targets_the_peer() {
        let ctx = egui::Context::default();
        let mut action = None;
        let mut rect = egui::Rect::NOTHING;
        for step in 0..6 {
            let pos = rect.center();
            let events = if step < 2 {
                Vec::new()
            } else {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: step % 2 == 0,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(step as f64 * 0.05),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let rendered = ui.scope(|ui| render_private_header_avatar(ui, 456, "私聊对象"));
                    rect = rendered.response.rect;
                    action = rendered.inner;
                },
            );
            output.textures_delta.clear();
        }
        assert!(matches!(
            action,
            Some(MessageAction::Poke {
                room_id: 456,
                target_id: 456
            })
        ));
    }
}
