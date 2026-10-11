//! 头像的共用交互：只产生已有消费者能够执行的操作，不负责下载或成员权限判断。

#[cfg(test)]
#[path = "tests/avatar_interaction.rs"]
mod interaction_tests;
#[cfg(test)]
#[path = "tests/avatar.rs"]
mod tests;

use crate::app::MessageAction;
use crate::app::media::{ImageAction, ImageSource};
use crate::ica::client::poke_target;
use crate::ica::types::RoomId;

/// 头像交互的目标描述：界面只把这些信息翻译成 `MessageAction`，
/// 不自行判断权限或发起网络请求。
pub struct AvatarTarget<'a> {
    /// 头像所属房间；群为负群号，私聊为对方 QQ（正值）。
    pub room_id: RoomId,
    /// 头像对应的 QQ 用户 id。
    pub user_id: i64,
    /// 展示用昵称，用于 @ 提及与菜单标题。
    pub name: &'a str,
    /// 消息里给出的头像地址；为空时不展示查看/保存等图片操作。
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

/// 取出头像原图地址。
///
/// 消息里的地址通常是 140 像素缩略图；恰好等于缩略图地址时换用 `s=0`
/// 的原图参数，否则原样返回（群头像可能是自定义地址）。
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
            // room_id 为负表示群聊：只有群内成员才能 @TA。
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
            // 查看发言记录与群成员管理仅群聊头像可用。
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
