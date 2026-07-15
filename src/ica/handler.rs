use rust_socketio::Payload;
use rust_socketio::asynchronous::Client;

use futures_util::future::BoxFuture;
use std::time::Duration;

use serde_json::Value as JsonValue;
use serde_json::json;

use tokio::sync::mpsc::UnboundedSender;

use crate::ica::event::BridgeEvent;

use crate::ica::types::message::SendMessage;

use super::client;
use super::command::{IcaCommand, emit_ui_event};
use super::file_manager::call_file_manager;

mod file_upload;
mod history;
mod http_send;
mod message_payload;
use file_upload::upload_and_send_file;
use http_send::{http_send_message, http_send_value, request_send_token};
use message_payload::build_multi_image_raw_payload;

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

async fn send_message(
    message: SendMessage,
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    api_base_url: &str,
) {
    let room_id = message.room_id;
    if message.has_b64img() {
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
                "message": "sendMessage failed",
            }),
        );
    }
}

pub(super) async fn handle_command(
    command: IcaCommand,
    client: &Client,
    event_tx: &Option<UnboundedSender<BridgeEvent>>,
    bridge_key: &str,
    socket_url: &str,
    api_base_url: &str,
) {
    match command {
        IcaCommand::FetchMessages(room_id) => {
            history::fetch_messages(client, event_tx, bridge_key, room_id).await
        }
        IcaCommand::FetchLatestHistory {
            room_id,
            current_loaded_messages,
        } => {
            history::fetch_latest_history(
                client,
                event_tx,
                bridge_key,
                room_id,
                current_loaded_messages,
            )
            .await
        }
        IcaCommand::FetchOlderMessages { room_id, offset } => {
            history::fetch_older_messages(client, event_tx, bridge_key, room_id, offset).await
        }
        IcaCommand::FetchGroupMembers { room_id } => {
            history::fetch_group_members(client, event_tx, bridge_key, room_id).await
        }
        IcaCommand::GetSystemMsg => {
            history::get_system_messages(client, event_tx, bridge_key).await
        }
        IcaCommand::SendMessage(message) => {
            send_message(message, client, event_tx, bridge_key, api_base_url).await;
        }
        IcaCommand::SendImageMessage {
            room_id,
            content,
            reply_to,
            mentions,
            image_type,
            image_data,
        } => {
            let encoded_message = tokio::task::spawn_blocking(move || {
                let mut message = SendMessage::new(content, room_id, reply_to);
                message.set_mentions(&mentions);
                message.set_img(image_data.as_ref(), &image_type, false);
                message
            })
            .await;
            match encoded_message {
                Ok(message) => {
                    send_message(message, client, event_tx, bridge_key, api_base_url).await;
                }
                Err(e) => emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendImageMessage",
                        "roomId": room_id,
                        "message": format!("图片编码任务失败: {e}"),
                    }),
                ),
            }
        }
        IcaCommand::SendMultiImageMessage {
            room_id,
            content,
            reply_to,
            mentions,
            images,
        } => {
            let encoded_payload = tokio::task::spawn_blocking(move || {
                build_multi_image_raw_payload(
                    room_id,
                    &content,
                    reply_to.as_ref(),
                    &mentions,
                    &images,
                )
            })
            .await;
            match encoded_payload {
                Ok(payload) => match request_send_token(client).await {
                    Ok(token) => {
                        if let Err(e) = http_send_value(api_base_url, &token, &payload).await {
                            emit_ui_event(
                                event_tx,
                                bridge_key,
                                "commandFailed",
                                json!({
                                    "kind": "sendMultiImageMessage",
                                    "roomId": room_id,
                                    "message": e,
                                }),
                            );
                        }
                    }
                    Err(e) => emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "sendMultiImageMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    ),
                },
                Err(e) => emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendMultiImageMessage",
                        "roomId": room_id,
                        "message": format!("图片编码任务失败: {e}"),
                    }),
                ),
            }
        }
        IcaCommand::SendRawMessage { room_id, content } => {
            let payload = json!({
                "messageType": "raw",
                "roomId": room_id,
                "content": content.to_string(),
            });
            if !client::send_string_message(client, &payload).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendRawMessage",
                        "roomId": room_id,
                        "message": "sendRawMessage failed",
                    }),
                );
            }
        }
        IcaCommand::SearchMessages {
            room_id,
            keyword,
            offset,
        } => {
            let tx = event_tx.clone();
            let bridge_id = bridge_key.to_string();
            let keyword_for_event = keyword.clone();
            if let Err(e) = client
                .emit_with_ack(
                    "searchMessages",
                    vec![json!(room_id), json!(keyword), json!(offset)],
                    Duration::from_secs(15),
                    move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                        let tx = tx.clone();
                        let bridge_id = bridge_id.clone();
                        let keyword = keyword_for_event.clone();
                        Box::pin(async move {
                            emit_ui_event(
                                &tx,
                                &bridge_id,
                                "searchMessagesResponse",
                                json!({
                                    "roomId": room_id,
                                    "keyword": keyword,
                                    "offset": offset,
                                    "messages": ack_payload_values(&payload),
                                }),
                            );
                        })
                    },
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "searchMessages",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SocketApiCall {
            event,
            args,
            expect_ack,
        } => {
            if expect_ack {
                let event_for_cb = event.clone();
                let tx = event_tx.clone();
                let bridge_id = bridge_key.to_string();
                if let Err(e) = client
                    .emit_with_ack(
                        event.as_str(),
                        args,
                        Duration::from_secs(15),
                        move |payload: Payload, _client: Client| -> BoxFuture<'static, ()> {
                            let tx = tx.clone();
                            let bridge_id = bridge_id.clone();
                            let event = event_for_cb.clone();
                            Box::pin(async move {
                                emit_ui_event(
                                    &tx,
                                    &bridge_id,
                                    "socketApiResponse",
                                    json!({
                                        "event": event,
                                        "ack": ack_payload_values(&payload),
                                    }),
                                );
                            })
                        },
                    )
                    .await
                {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "socketApiCall",
                            "event": event,
                            "message": e.to_string(),
                        }),
                    );
                }
            } else if let Err(e) = client.emit(event.as_str(), args).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "socketApiCall",
                        "event": event,
                        "message": e.to_string(),
                    }),
                );
            } else {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "socketApiResponse",
                    json!({
                        "event": event,
                        "sent": true,
                    }),
                );
            }
        }
        IcaCommand::FileManagerCall {
            gin,
            event,
            args,
            expect_ack,
        } => {
            match call_file_manager(
                client, event_tx, bridge_key, socket_url, gin, event, args, expect_ack,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "fileManagerCall",
                            "gin": gin,
                            "message": e,
                        }),
                    );
                }
            }
        }
        IcaCommand::PinRoom { room_id, pin } => {
            if let Err(e) = client
                .emit("pinRoom", vec![json!(room_id), json!(pin)])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "pinRoom",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveChat(room_id) => {
            if let Err(e) = client.emit("removeChat", json!(room_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::IgnoreChat { room_id, room_name } => {
            if let Err(e) = client
                .emit("ignoreChat", json!({"id": room_id, "name": room_name}))
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "ignoreChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveIgnoredChat(room_id) => {
            if let Err(e) = client.emit("removeIgnoredChat", json!(room_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeIgnoredChat",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SetRoomPriority { room_id, priority } => {
            if let Err(e) = client
                .emit("setRoomPriority", vec![json!(room_id), json!(priority)])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "setRoomPriority",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::ReportRead {
            room_id,
            message_id,
        } => {
            if let Err(e) = client.emit("reportRead", json!(message_id)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "reportRead",
                        "roomId": room_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SetOnlineStatus(status) => {
            if let Err(e) = client.emit("setOnlineStatus", json!(status)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "setOnlineStatus",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SendGroupSign { room_id } => {
            if !client::send_room_sign_in(client, room_id).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendGroupSign",
                        "roomId": room_id,
                        "message": "sendGroupSign failed",
                    }),
                );
            }
        }
        IcaCommand::SendGroupPoke { room_id, target_id } => {
            if !client::send_poke(client, room_id, target_id).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "sendGroupPoke",
                        "roomId": room_id,
                        "targetId": target_id,
                        "message": "sendGroupPoke failed",
                    }),
                );
            }
        }
        IcaCommand::StopFetchingHistory => {
            if let Err(e) = client.emit("stopFetchingHistory", json!(null)).await {
                tracing::warn!("send stopFetchingHistory failed: {}", e);
            }
        }
        IcaCommand::HideMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "hideMessage",
                    vec![json!(room_id), json!(message_id.clone())],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "hideMessage",
                        "roomId": room_id,
                        "messageId": message_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RevealMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "revealMessage",
                    vec![json!(room_id), json!(message_id.clone())],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "revealMessage",
                        "roomId": room_id,
                        "messageId": message_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RenewMessage {
            room_id,
            message_id,
        } => {
            if let Err(e) = client
                .emit(
                    "renewMessage",
                    vec![json!(room_id), json!(message_id.clone()), json!(null)],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "renewMessage",
                        "roomId": room_id,
                        "messageId": message_id,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::DeleteMessage(message) => {
            let message_id = message.message_id.clone();
            if !client::delete_message(client, &message).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "deleteMessage",
                        "messageId": message_id,
                        "message": "deleteMessage failed",
                    }),
                );
            }
        }
        IcaCommand::AddChatGroup {
            name,
            rooms,
            include_all_personal,
        } => {
            let payload = json!({
                "name": name,
                "rooms": rooms,
                "includeAllPersonal": include_all_personal,
            });
            if let Err(e) = client.emit("addChatGroup", payload).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "addChatGroup",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::RemoveChatGroup { name } => {
            if let Err(e) = client.emit("removeChatGroup", json!(name)).await {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "removeChatGroup",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::UpdateChatGroup {
            name,
            rooms,
            include_all_personal,
        } => {
            let payload = json!({
                "name": name,
                "rooms": rooms,
                "includeAllPersonal": include_all_personal,
            });
            if let Err(e) = client
                .emit("updateChatGroup", vec![json!(name), payload])
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "updateChatGroup",
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::HandleRequest {
            request_type,
            flag,
            accept,
        } => {
            if let Err(e) = client
                .emit(
                    "handleRequest",
                    vec![json!(request_type), json!(flag), json!(accept)],
                )
                .await
            {
                emit_ui_event(
                    event_tx,
                    bridge_key,
                    "commandFailed",
                    json!({
                        "kind": "handleRequest",
                        "flag": flag,
                        "message": e.to_string(),
                    }),
                );
            }
        }
        IcaCommand::SendFileMessage {
            room_id,
            content,
            reply_to,
            mentions,
            file_name,
            file_type,
            file_data,
        } => {
            match upload_and_send_file(
                client, room_id, content, reply_to, mentions, &file_name, &file_type, &file_data,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    emit_ui_event(
                        event_tx,
                        bridge_key,
                        "commandFailed",
                        json!({
                            "kind": "sendFileMessage",
                            "roomId": room_id,
                            "message": e,
                        }),
                    );
                }
            }
        }
    }
}
