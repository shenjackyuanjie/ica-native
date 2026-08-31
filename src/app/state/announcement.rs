use crate::ica::types::RoomId;
use crate::ica::types::announcement::GroupAnnouncement;

/// 群公告查看器状态，通过 `Arc<Mutex<..>>` 在主窗口和公告窗口之间共享。
#[derive(Debug, Clone, Default)]
pub struct GroupAnnouncementViewerState {
    pub open: bool,
    pub room_id: RoomId,
    pub room_name: String,
    pub announcements: Vec<GroupAnnouncement>,
    pub loading: bool,
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

    /// 只接受“当前群的最新一次请求”的响应。
    ///
    /// 用户可能在等待期间切换群或反复点刷新，两个条件缺一都会让旧响应覆盖新内容。
    fn accepts(&self, request_id: u64, room_id: RoomId) -> bool {
        request_id == self.request_id && room_id == self.room_id
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::GroupAnnouncementViewerState;
    use crate::ica::types::announcement::{GroupAnnouncement, parse_announcement_list};

    fn announcement(fid: &str) -> GroupAnnouncement {
        pinned_announcement(fid, false)
    }

    fn pinned_announcement(fid: &str, pinned: bool) -> GroupAnnouncement {
        parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [{ "fid": fid, "pinned": i64::from(pinned) }]
        }))
        .expect("构造测试公告")
        .remove(0)
    }

    #[test]
    fn pinned_announcements_move_to_the_top_without_reordering_their_peers() {
        let mut viewer = GroupAnnouncementViewerState::default();
        let request = viewer.begin_request(-1001, "群一".to_string());
        viewer.apply_response(
            request,
            -1001,
            vec![
                announcement("普通1"),
                pinned_announcement("置顶1", true),
                announcement("普通2"),
                pinned_announcement("置顶2", true),
            ],
        );

        // CGI 下发的是按发布时间倒序的混合列表，置顶要提到最前，
        // 但两组内部的相对顺序不能被打乱。
        let order = viewer
            .announcements
            .iter()
            .map(|item| item.fid.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, ["置顶1", "置顶2", "普通1", "普通2"]);
    }

    #[test]
    fn stale_responses_from_previous_request_or_other_room_are_discarded() {
        let mut viewer = GroupAnnouncementViewerState::default();
        let first = viewer.begin_request(-1001, "群一".to_string());
        let second = viewer.begin_request(-1001, "群一".to_string());

        // 上一次请求的迟到响应不能覆盖新一次的加载状态。
        assert!(!viewer.apply_response(first, -1001, vec![announcement("old")]));
        assert!(viewer.loading);
        assert!(viewer.announcements.is_empty());

        // 序号对上但群号不对，同样丢弃：用户已经切到别的群。
        assert!(!viewer.apply_response(second, -2002, vec![announcement("other")]));
        assert!(viewer.loading);

        assert!(viewer.apply_response(second, -1001, vec![announcement("new")]));
        assert!(!viewer.loading);
        assert_eq!(viewer.announcements.len(), 1);

        // 失败响应同样要按序号与群号校验。
        assert!(!viewer.fail(first, -1001, "过期错误".to_string()));
        assert!(viewer.last_error.is_none());
    }

    #[test]
    fn switching_room_clears_previous_content_but_refresh_keeps_expansion() {
        let mut viewer = GroupAnnouncementViewerState::default();
        let request = viewer.begin_request(-1001, "群一".to_string());
        viewer.apply_response(request, -1001, vec![announcement("a"), announcement("b")]);
        viewer.expanded_fid = Some("b".to_string());

        // 同一个群刷新：内容先保留，展开项在响应里仍存在就继续展开。
        let refresh = viewer.begin_request(-1001, "群一".to_string());
        assert_eq!(viewer.announcements.len(), 2);
        assert_eq!(viewer.expanded_fid.as_deref(), Some("b"));
        viewer.apply_response(refresh, -1001, vec![announcement("a")]);
        // 展开的公告已被删除，不能继续指向不存在的 fid。
        assert_eq!(viewer.expanded_fid, None);

        viewer.expanded_fid = Some("a".to_string());
        viewer.begin_request(-2002, "群二".to_string());
        assert!(viewer.announcements.is_empty());
        assert_eq!(viewer.expanded_fid, None);
        assert_eq!(viewer.room_name, "群二");
    }
}
