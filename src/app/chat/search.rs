use crate::ica::IcaCommand;
use crate::ica::types::RoomId;

use crate::app::IcaApp;

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
            keyword: keyword.clone(),
            offset,
        };

        if let Err(e) = self.bridge_states[bridge_idx].send(command) {
            tracing::warn!(error = %e, room_id, keyword = %keyword, "发送 searchMessages 命令失败");
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state
                    .message_search
                    .fail("搜索聊天记录命令发送失败".to_string());
            }
        }
    }
}
