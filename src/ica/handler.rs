use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;

use crate::ica::types::message::SendMessage;

use super::client;
use super::command::{IcaCommand, emit_ui_event};

mod announcement;
mod contacts;
mod file_upload;
mod forward;
mod history;
mod http_send;
mod message_payload;

// 命令分发的具体实现按领域拆分，本文件只保留「命令 -> 处理函数」的映射。
mod account_commands;
mod bridge_api_commands;
mod context;
mod forward_commands;
mod group_commands;
mod history_commands;
mod message_commands;
mod room_commands;

use context::CommandContext;
use file_upload::upload_and_send_file;
use http_send::{http_send_message, request_send_token};
use message_payload::build_multi_image_message;

fn ack_payload_values(payload: &Payload) -> Vec<JsonValue> {
    match payload {
        Payload::Text(values) => {
            if let Some(JsonValue::Array(args)) = values.first()
                && values.len() == 1
            {
                return args.clone();
            }
            values.clone()
        }
        Payload::Binary(bytes) => vec![json!(bytes.to_vec())],
        _ => Vec::new(),
    }
}

fn ack_payload_first(payload: &Payload) -> Option<JsonValue> {
    ack_payload_values(payload).into_iter().next()
}

fn normalize_ack_list(mut values: Vec<JsonValue>) -> JsonValue {
    if values.len() == 1 {
        return match values.remove(0) {
            JsonValue::Array(items) => JsonValue::Array(items),
            value => JsonValue::Array(vec![value]),
        };
    }
    JsonValue::Array(values)
}

async fn send_message(
    message: SendMessage,
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    api_base_url: &str,
) {
    let room_id = message.room_id;
    if message.has_base64_media() {
        match request_send_token(client).await {
            Ok(token) => {
                if let Err(e) = http_send_message(api_base_url, &token, &message).await {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "sendMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    );
                }
            }
            Err(e) => {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendMessage",
                        "roomId": room_id,
                        "message": e,
                    }),
                );
            }
        }
    } else if !client::send_message(client, &message).await {
        emit_ui_event(
            event_tx,
            bridge_key,
            "commandFailed",
            json!({
                "kind": "sendMessage",
                "roomId": room_id,
                "message": "sendMessage 失败",
            }),
        );
    }
}

/// 把一条 GUI 命令交给对应领域的实现。
///
/// 这里刻意保持穷尽匹配：新增 IcaCommand 变体时会在本函数编译失败，
/// 提醒补上对应的处理函数，而不会被默认分支悄悄吞掉。
pub async fn handle_command(
    command: IcaCommand,
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    socket_url: &str,
    api_base_url: &str,
) {
    let ctx = CommandContext {
        client,
        event_tx,
        bridge_key,
        socket_url,
        api_base_url,
    };
    match command {
        IcaCommand::FetchMessages(room_id) => history_commands::fetch_messages(ctx, room_id).await,
        IcaCommand::FetchLatestHistory {
            room_id,
            current_loaded_messages,
        } => history_commands::fetch_latest_history(ctx, room_id, current_loaded_messages).await,
        IcaCommand::FetchOlderMessages {
            room_id,
            before_time,
            before_id,
        } => history_commands::fetch_older_messages(ctx, room_id, before_time, before_id).await,
        IcaCommand::FetchGroupMembers { room_id } => {
            history_commands::fetch_group_members(ctx, room_id).await
        }
        IcaCommand::FetchGroupAnnouncements {
            request_id,
            room_id,
            bkn,
        } => group_commands::fetch_group_announcements(ctx, request_id, room_id, bkn).await,
        IcaCommand::PublishGroupAnnouncement {
            request_id,
            room_id,
            bkn,
            draft,
        } => group_commands::publish_group_announcement(ctx, request_id, room_id, bkn, draft).await,
        IcaCommand::DeleteGroupAnnouncement {
            request_id,
            room_id,
            bkn,
            fid,
        } => group_commands::delete_group_announcement(ctx, request_id, room_id, bkn, fid).await,
        IcaCommand::FetchMessagesBySender {
            request_id,
            room_id,
            sender_id,
            offset,
            snapshot_time,
        } => {
            history_commands::fetch_messages_by_sender(
                ctx,
                request_id,
                room_id,
                sender_id,
                offset,
                snapshot_time,
            )
            .await
        }
        IcaCommand::SetGroupBan {
            room_id,
            target_id,
            duration,
        } => group_commands::set_group_ban(ctx, room_id, target_id, duration).await,
        IcaCommand::GetSystemMsg => account_commands::get_system_msg(ctx).await,
        IcaCommand::FetchContacts { request_id } => {
            account_commands::fetch_contacts(ctx, request_id).await
        }
        IcaCommand::AddRoom(room) => room_commands::add_room(ctx, room).await,
        IcaCommand::SendMessage(message) => message_commands::send_chat_message(ctx, message).await,
        IcaCommand::SendImageMessage {
            room_id,
            content,
            reply_to,
            mentions,
            image_type,
            image_data,
        } => {
            message_commands::send_image_message(
                ctx, room_id, content, reply_to, mentions, image_type, image_data,
            )
            .await
        }
        IcaCommand::SendMultiImageMessage {
            room_id,
            content,
            reply_to,
            mentions,
            images,
        } => {
            message_commands::send_multi_image_message(
                ctx, room_id, content, reply_to, mentions, images,
            )
            .await
        }
        IcaCommand::SendRawMessage { room_id, content } => {
            message_commands::send_raw_message(ctx, room_id, content).await
        }
        IcaCommand::SearchMessages {
            room_id,
            keyword,
            offset,
        } => history_commands::search_messages(ctx, room_id, keyword, offset).await,
        IcaCommand::FetchForwardMessages {
            request_id,
            res_id,
            file_name,
            fallback_res_id,
        } => {
            forward_commands::fetch_forward_messages(
                ctx,
                request_id,
                res_id,
                file_name,
                fallback_res_id,
            )
            .await
        }
        IcaCommand::SendMergedForward {
            nodes,
            direct_message,
            origin,
            target_room_id,
        } => {
            forward_commands::send_merged_forward(
                ctx,
                nodes,
                direct_message,
                origin,
                target_room_id,
            )
            .await
        }
        IcaCommand::SocketApiCall {
            event,
            args,
            expect_ack,
        } => bridge_api_commands::socket_api_call(ctx, event, args, expect_ack).await,
        IcaCommand::FileManagerCall {
            gin,
            event,
            args,
            expect_ack,
        } => bridge_api_commands::file_manager_call(ctx, gin, event, args, expect_ack).await,
        IcaCommand::UploadGroupFile {
            group_id,
            parent_id,
            file_name,
            file_data,
        } => {
            group_commands::upload_group_file(ctx, group_id, parent_id, file_name, file_data).await
        }
        IcaCommand::PinRoom { room_id, pin } => room_commands::pin_room(ctx, room_id, pin).await,
        IcaCommand::RemoveChat(room_id) => room_commands::remove_chat(ctx, room_id).await,
        IcaCommand::IgnoreChat { room_id, room_name } => {
            room_commands::ignore_chat(ctx, room_id, room_name).await
        }
        IcaCommand::RemoveIgnoredChat(room_id) => {
            room_commands::remove_ignored_chat(ctx, room_id).await
        }
        IcaCommand::SetRoomPriority { room_id, priority } => {
            room_commands::set_room_priority(ctx, room_id, priority).await
        }
        IcaCommand::ClearRoomUnread { room_id } => {
            room_commands::clear_room_unread(ctx, room_id).await
        }
        IcaCommand::ReportRead {
            room_id,
            message_id,
        } => room_commands::report_read(ctx, room_id, message_id).await,
        IcaCommand::SetOnlineStatus(status) => {
            account_commands::set_online_status(ctx, status).await
        }
        IcaCommand::SendGroupSign { room_id } => {
            group_commands::send_group_sign(ctx, room_id).await
        }
        IcaCommand::SendGroupPoke { room_id, target_id } => {
            group_commands::send_group_poke(ctx, room_id, target_id).await
        }
        IcaCommand::StopFetchingHistory => history_commands::stop_fetching_history(ctx).await,
        IcaCommand::HideMessage {
            room_id,
            message_id,
        } => message_commands::hide_message(ctx, room_id, message_id).await,
        IcaCommand::RevealMessage {
            room_id,
            message_id,
        } => message_commands::reveal_message(ctx, room_id, message_id).await,
        IcaCommand::RenewMessage {
            room_id,
            message_id,
        } => message_commands::renew_message(ctx, room_id, message_id).await,
        IcaCommand::DeleteMessage(message) => message_commands::delete_message(ctx, message).await,
        IcaCommand::AddChatGroup {
            name,
            rooms,
            include_all_personal,
        } => room_commands::add_chat_group(ctx, name, rooms, include_all_personal).await,
        IcaCommand::RemoveChatGroup { name } => room_commands::remove_chat_group(ctx, name).await,
        IcaCommand::UpdateChatGroup {
            name,
            rooms,
            include_all_personal,
        } => room_commands::update_chat_group(ctx, name, rooms, include_all_personal).await,
        IcaCommand::HandleRequest {
            request_type,
            flag,
            accept,
        } => account_commands::handle_request(ctx, request_type, flag, accept).await,
        IcaCommand::SendFileMessage {
            room_id,
            content,
            reply_to,
            mentions,
            file_name,
            file_type,
            file_data,
        } => {
            message_commands::send_file_message(
                ctx,
                room_id,
                content,
                reply_to,
                mentions,
                message_commands::OutgoingFile {
                    name: file_name,
                    file_type,
                    data: file_data,
                },
            )
            .await
        }
    }
}
