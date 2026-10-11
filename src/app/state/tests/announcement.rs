use super::GroupAnnouncementViewerState;
use crate::ica::types::announcement::{GroupAnnouncement, parse_announcement_list};
use serde_json::json;

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
