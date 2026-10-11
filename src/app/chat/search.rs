//! 聊天记录搜索的界面入口与命令分发。
//!
//! 搜索状态由 `MessageSearchState` 按会话维护，本模块只负责：打开搜索窗、
//! 校验关键词、把请求转成 `IcaCommand::SearchMessages` 交给 IO 线程，并在
//! 命令发送失败时把错误回填到搜索状态，由界面展示。

use crate::app::IcaApp;
use crate::ica::IcaCommand;
use crate::ica::types::RoomId;

impl IcaApp {
    /// 为指定 bridge 的会话打开消息搜索窗，并带入房间号与名称。
    pub fn open_message_search(&mut self, bridge_idx: usize, room_id: RoomId, room_name: String) {
        if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
            state.message_search.open_for_room(room_id, room_name);
        }
    }

    /// 发起一次聊天记录搜索。
    ///
    /// - `keyword` 先 trim，为空时直接让搜索状态报错，不发命令；
    /// - `offset` 供结果分页“加载更多”复用，由调用方递增；
    /// - 命令发送失败时记录警告并把错误写回搜索状态。
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
