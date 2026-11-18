use super::*;

#[test]
fn favorite_pagination_counts_tombstones_but_does_not_display_them() {
    let mut browser = ResourceBrowser::default();
    browser.accept(Query::Favorites(0), json!({
        "collection_list": [{"cid": "removed", "status": 2}, {"cid": "visible", "status": 1}],
        "total_count": 3
    })).unwrap();
    assert_eq!(browser.favorites.len(), 1);
    assert_eq!(browser.favorites[0].cid, "visible");
    assert!(browser.has_more);
    let (_, request) = Query::Favorites(browser.next_offset).request();
    assert_eq!(request["options"]["start_pos"], 2);

    // 某些 Bridge 会无视游标。如果同一页反复出现，不要一直允许加载更多。
    browser.accept(Query::Favorites(browser.next_offset), json!({
        "collection_list": [{"cid": "removed", "status": 2}, {"cid": "visible", "status": 1}],
        "total_count": 3
    })).unwrap();
    assert!(!browser.has_more);
    assert_eq!(browser.favorites.len(), 1);
}

#[test]
fn changing_accounts_drops_the_pending_response_and_private_resource_view() {
    let mut browser = ResourceBrowser::default();
    browser.set_owner(Some(("bridge".into(), 1)));
    let (tx, rx) = oneshot::channel();
    browser.pending = Some((Query::Favorites(0), rx));
    browser.input = "share code".into();
    browser.fileset = Some("old fileset".into());

    browser.set_owner(Some(("bridge".into(), 2)));
    assert!(
        tx.send(Ok(json!({"collection_list": [{"cid": "old favorite"}]})))
            .is_err()
    );
    assert!(browser.pending.is_none() && browser.fileset.is_none() && browser.input.is_empty());
}
