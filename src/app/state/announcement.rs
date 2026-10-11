//! 群公告查看器状态。
//!
//! 该状态在主窗口与独立的公告窗口之间通过 `Arc<Mutex<..>>` 共享：
//! 公告窗口只负责渲染与收集输入，真正的命令发送、响应回填都由主循环完成，
//! 因此这里出现大量“置位标志 + 主循环消费”的字段（`reload_requested`、
//! `pending_submit` 等）。请求序号 `request_id` 用于丢弃过期响应。

#[cfg(test)]
#[path = "tests/announcement.rs"]
mod tests;

use crate::ica::types::RoomId;
use crate::ica::types::announcement::{GroupAnnouncement, GroupAnnouncementDraft};

/// 群公告查看器状态，通过 `Arc<Mutex<..>>` 在主窗口和公告窗口之间共享。
#[derive(Debug, Clone, Default)]
pub struct GroupAnnouncementViewerState {
    /// 公告窗口是否打开。
    pub open: bool,
    /// 当前查看的群号；窗口内所有操作都针对该群。
    pub room_id: RoomId,
    /// 当前查看的群名称，仅用于窗口标题与提示文案展示。
    pub room_name: String,
    /// 已拉取到的公告列表；排序见 `apply_response`。
    pub announcements: Vec<GroupAnnouncement>,
    /// 是否正在拉取公告列表。
    pub loading: bool,
    /// 最近一次拉取失败的原因；成功后被清空。
    pub last_error: Option<String>,
    /// 展开正文的公告 fid；同一时刻只展开一条。
    pub expanded_fid: Option<String>,
    /// 公告窗口跑在自己的 egui 上下文里，拿不到 `IcaApp`，
    /// 因此“刷新”只置位，真正的命令发送仍由主循环完成。
    pub reload_requested: bool,
    /// 待打开的配图原图 URL；同样由主循环消费后交给图片预览窗口。
    pub pending_image_url: Option<String>,
    /// 最近一次公告接口的完整响应，供排查字段差异时整体复制。
    pub raw_response: Option<String>,

    // ---- 发布 / 编辑 / 删除 ----
    /// 编辑器是否展开（就在公告窗口内，不另开窗口）。
    pub editor_open: bool,
    /// 当前编辑器里的草稿。
    pub draft: GroupAnnouncementDraft,
    /// 写操作进行中，禁用重复提交。
    pub submitting: bool,
    /// 编辑器内的错误提示（提交失败等）。
    pub editor_error: Option<String>,
    /// 编辑器点了发布；由主循环消费后发命令。
    pub pending_submit: bool,
    /// 待删除的公告 fid；由主循环消费后发删除命令。
    pub pending_delete_fid: Option<String>,
    /// 右键菜单点删除后、真正提交前的二次确认。
    pub delete_confirm_fid: Option<String>,

    /// 单调递增的请求序号，用于丢弃过期响应。
    request_id: u64,
}

impl GroupAnnouncementViewerState {
    /// 开始一次拉取，返回本次请求序号。
    pub fn begin_request(&mut self, room_id: RoomId, room_name: String) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        if self.room_id != room_id {
            // 换群时先清空上一群的公告，避免加载期间看到别的群的内容。
            self.announcements.clear();
            self.expanded_fid = None;
        }
        self.open = true;
        self.room_id = room_id;
        self.room_name = room_name;
        self.loading = true;
        self.last_error = None;
        self.reload_requested = false;
        self.pending_image_url = None;
        self.request_id
    }

    /// 记录整份响应；与 `apply_response` 分开，失败时也能留下原始报文。
    pub fn set_raw_response(&mut self, request_id: u64, room_id: RoomId, raw: String) {
        if self.accepts(request_id, room_id) {
            self.raw_response = Some(raw);
        }
    }

    /// 用一次成功响应填充公告列表。
    ///
    /// `request_id` / `room_id` 与当前请求不匹配时丢弃并返回 `false`；
    /// 匹配时写入列表、结束加载态并返回 `true`。
    pub fn apply_response(
        &mut self,
        request_id: u64,
        room_id: RoomId,
        mut announcements: Vec<GroupAnnouncement>,
    ) -> bool {
        if !self.accepts(request_id, room_id) {
            return false;
        }
        // 展示顺序与手Q 一致：发给新成员的公告 > 置顶公告 > 其余。
        // `sort_by_key` 是稳定排序，因此每组内部都保持 CGI 下发的发布时间倒序。
        announcements.sort_by_key(|announcement| (!announcement.to_new, !announcement.pinned));
        self.expanded_fid = self
            .expanded_fid
            .take()
            .filter(|fid| announcements.iter().any(|item| &item.fid == fid));
        self.announcements = announcements;
        self.loading = false;
        self.last_error = None;
        true
    }

    /// 记录一次拉取失败：清空列表、结束加载态并留下错误信息。
    ///
    /// 过期请求（`request_id` / `room_id` 不匹配）同样被忽略并返回 `false`。
    pub fn fail(&mut self, request_id: u64, room_id: RoomId, error: String) -> bool {
        if !self.accepts(request_id, room_id) {
            return false;
        }
        self.announcements.clear();
        self.expanded_fid = None;
        self.loading = false;
        self.last_error = Some(error);
        true
    }

    /// 打开编辑器：新建时草稿为空，编辑时从已有公告预填。
    pub fn open_editor(&mut self, draft: GroupAnnouncementDraft) {
        self.draft = draft;
        self.editor_open = true;
        self.editor_error = None;
        self.submitting = false;
        self.delete_confirm_fid = None;
    }

    /// 关闭编辑器并清空与编辑器相关的中间状态（草稿、错误、提交标志）。
    pub fn close_editor(&mut self) {
        self.editor_open = false;
        self.editor_error = None;
        self.submitting = false;
        self.pending_submit = false;
    }

    /// 写操作成功：关闭编辑器并请求刷新列表。
    pub fn action_done(&mut self) {
        self.submitting = false;
        self.editor_open = false;
        self.editor_error = None;
        self.pending_submit = false;
        self.delete_confirm_fid = None;
        self.reload_requested = true;
    }

    /// 写操作失败：留在编辑器里展示原因。
    pub fn action_failed(&mut self, error: String) {
        self.submitting = false;
        self.pending_submit = false;
        self.editor_error = Some(error);
    }

    /// 只接受“当前群的最新一次请求”的响应。
    ///
    /// 用户可能在等待期间切换群或反复点刷新，两个条件缺一都会让旧响应覆盖新内容。
    fn accepts(&self, request_id: u64, room_id: RoomId) -> bool {
        request_id == self.request_id && room_id == self.room_id
    }
}
