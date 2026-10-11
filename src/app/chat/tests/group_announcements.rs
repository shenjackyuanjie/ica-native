use super::{announcement_headline, collapse_text};
use crate::ica::types::announcement::parse_announcement_list;
use serde_json::json;

fn announcement(title: &str, text: &str) -> crate::ica::types::announcement::GroupAnnouncement {
    parse_announcement_list(&json!({
        "ec": 0,
        "feeds": [{ "fid": "1", "msg": { "title": title, "text": text } }]
    }))
    .expect("构造测试公告")
    .remove(0)
}

#[test]
fn headline_falls_back_to_first_non_empty_body_line_when_title_is_absent() {
    // QQ 允许发布没有标题的公告，列表里必须仍然可区分。
    assert_eq!(
        announcement_headline(&announcement("公告标题", "正文")),
        "公告标题"
    );
    assert_eq!(
        announcement_headline(&announcement("   ", "\n\n  首行  \n次行")),
        "首行"
    );
    assert_eq!(
        announcement_headline(&announcement("", "   \n  ")),
        "(无标题公告)"
    );
}

#[test]
fn collapsing_reports_truncation_only_when_lines_are_actually_dropped() {
    let (text, truncated) = collapse_text("1\n2\n3\n4");
    assert_eq!(text, "1\n2\n3\n4");
    assert!(!truncated, "正好 4 行不应显示展开按钮");

    let (text, truncated) = collapse_text("1\n2\n3\n4\n5");
    assert_eq!(text, "1\n2\n3\n4");
    assert!(truncated);
}
