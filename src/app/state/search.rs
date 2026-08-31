//! 聊天记录搜索与成员发言记录的分页状态。

use crate::ica::types::{RoomId, message::Message};

#[derive(Debug, Clone)]
pub struct MessageSearchState {
    pub open: bool,
    pub room_id: Option<RoomId>,
    pub room_name: String,
    pub keyword: String,
    pub searched_keyword: String,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub has_more: bool,
    pub last_error: Option<String>,
}

impl Default for MessageSearchState {
    fn default() -> Self {
        Self {
            open: false,
            room_id: None,
            room_name: String::new(),
            keyword: String::new(),
            searched_keyword: String::new(),
            messages: Vec::new(),
            loading: false,
            has_more: true,
            last_error: None,
        }
    }
}

impl MessageSearchState {
    pub fn open_for_room(&mut self, room_id: RoomId, room_name: String) {
        if self.room_id != Some(room_id) {
            self.keyword.clear();
            self.searched_keyword.clear();
            self.messages.clear();
            self.has_more = true;
            self.loading = false;
            self.last_error = None;
        }
        self.open = true;
        self.room_id = Some(room_id);
        self.room_name = room_name;
    }

    pub fn start_request(&mut self, keyword: String, offset: usize) {
        if offset == 0 || self.searched_keyword != keyword {
            self.messages.clear();
            self.has_more = true;
        }
        self.searched_keyword = keyword;
        self.loading = true;
        self.last_error = None;
    }

    pub fn apply_response(
        &mut self,
        room_id: RoomId,
        keyword: String,
        offset: usize,
        messages: Vec<Message>,
    ) {
        if self.room_id != Some(room_id) || self.searched_keyword != keyword {
            return;
        }

        self.loading = false;
        self.last_error = None;
        self.has_more = messages.len() >= 20;

        if offset == 0 {
            self.messages = messages;
            return;
        }

        for message in messages {
            if !self
                .messages
                .iter()
                .any(|existing| existing.msg_id == message.msg_id)
            {
                self.messages.push(message);
            }
        }
    }

    pub fn fail(&mut self, error: String) {
        self.loading = false;
        self.last_error = Some(error);
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemberHistoryState {
    pub open: bool,
    pub room_id: RoomId,
    pub sender_id: i64,
    pub sender_name: String,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub exhausted: bool,
    pub request_id: u64,
    /// 首次打开成员发言记录时固定的毫秒时间戳，确保分页期间的新消息不会改变结果集。
    pub snapshot_time: i64,
}
