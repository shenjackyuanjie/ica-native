//! 聊天记录搜索与成员发言记录的分页状态。
//!
//! 两个窗口的分页约定不同：消息搜索用“关键词 + offset”翻页，成员发言记录
//! 用固定快照时间戳 + 请求序号翻页。响应回填时都会校验归属，丢弃过期结果。

use crate::ica::types::{RoomId, message::Message};

/// 房间内聊天记录搜索窗口的分页状态。
#[derive(Debug, Clone)]
pub struct MessageSearchState {
    /// 窗口是否打开。
    pub open: bool,
    /// 搜索目标房间；`None` 表示尚未指定。
    pub room_id: Option<RoomId>,
    /// 目标房间名称，仅用于标题展示。
    pub room_name: String,
    /// 输入框中的关键词（尚未提交）。
    pub keyword: String,
    /// 已提交并用于当前结果集的关键词。
    pub searched_keyword: String,
    /// 已加载的搜索结果。
    pub messages: Vec<Message>,
    /// 是否正在搜索。
    pub loading: bool,
    /// 是否还有更多结果可翻页。
    pub has_more: bool,
    /// 最近一次失败原因。
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
    /// 打开某房间的搜索；换房间时清空上一房间的关键词与结果。
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

    /// 发起一次搜索请求。
    ///
    /// `offset` 为 0 或关键词变化时视为全新搜索，清空旧结果；否则追加一页。
    pub fn start_request(&mut self, keyword: String, offset: usize) {
        if offset == 0 || self.searched_keyword != keyword {
            self.messages.clear();
            self.has_more = true;
        }
        self.searched_keyword = keyword;
        self.loading = true;
        self.last_error = None;
    }

    /// 回填一页搜索结果。
    ///
    /// 房间或关键词已变化（过期响应）时静默丢弃；`offset == 0` 时整体替换，
    /// 否则按 `msg_id` 去重后追加。是否还有更多由本页条数推断。
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

    /// 记录一次搜索失败。
    pub fn fail(&mut self, error: String) {
        self.loading = false;
        self.last_error = Some(error);
    }
}

/// 成员发言记录窗口的分页状态。
#[derive(Debug, Clone, Default)]
pub struct MemberHistoryState {
    /// 窗口是否打开。
    pub open: bool,
    /// 目标房间。
    pub room_id: RoomId,
    /// 目标成员 QQ 号。
    pub sender_id: i64,
    /// 目标成员昵称，仅用于标题展示。
    pub sender_name: String,
    /// 已加载的该成员发言。
    pub messages: Vec<Message>,
    /// 是否正在翻页。
    pub loading: bool,
    /// 是否已翻到最早一条（没有更早记录）。
    pub exhausted: bool,
    /// 单调递增的请求序号，用于丢弃过期响应。
    pub request_id: u64,
    /// 首次打开成员发言记录时固定的毫秒时间戳，确保分页期间的新消息不会改变结果集。
    pub snapshot_time: i64,
}
