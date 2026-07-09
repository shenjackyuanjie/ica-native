use crate::ica::IcaCommand;
use crate::ica::types::{RoomId, message::Message};

use super::IcaApp;

impl IcaApp {
    pub fn open_message_search(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.message_search.open_for_room(room_id, room_name);
        }
    }

    pub fn request_message_search(
        &mut self,
        bridge_idx: usize,
        room_id: RoomId,
        keyword: String,
        offset: usize,
    ) {
        let keyword = keyword.trim().to_string();
        let Some(state) = self.bridge_states.get_mut(bridge_idx) else {
            return;
        };

        if keyword.is_empty() {
            state.message_search.fail("关键词不能为空".to_string());
            return;
        }

        state.message_search.start_request(keyword.clone(), offset);
        let command = IcaCommand::SearchMessages {
            room_id,
            keyword,
            offset,
        };

        if let Err(e) = self.ica_clients[bridge_idx].command_tx.send(command) {
            tracing::warn!("send searchMessages command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state
                    .message_search
                    .fail("搜索聊天记录命令发送失败".to_string());
            }
        }
    }

    pub fn search_result_image_urls(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .flat_map(|message| &message.files)
            .filter(|file| {
                super::renders::is_image_file_type(&file.file_type) && !file.url.is_empty()
            })
            .map(|file| file.url.clone())
            .collect()
    }
}
