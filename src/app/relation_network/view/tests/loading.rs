use super::{RelationBridgeSnapshot, relation_member_load_progress};

#[test]
fn member_load_progress_uses_live_group_total_and_clamps_loaded_count() {
    let snapshot = RelationBridgeSnapshot {
        rooms_len: 12,
        total_groups: 5,
        loaded_groups: 8,
        loading_groups: 0,
        pending_groups: 0,
    };

    let progress = relation_member_load_progress(&snapshot);

    assert_eq!(progress.loaded, 5);
    assert_eq!(progress.total, 5);
    assert_eq!(progress.ratio, 1.0);
}

#[test]
fn member_load_progress_keeps_empty_total_visible_without_dividing_by_zero() {
    let snapshot = RelationBridgeSnapshot {
        rooms_len: 0,
        total_groups: 0,
        loaded_groups: 0,
        loading_groups: 0,
        pending_groups: 0,
    };

    let progress = relation_member_load_progress(&snapshot);

    assert_eq!(progress.loaded, 0);
    assert_eq!(progress.total, 0);
    assert_eq!(progress.ratio, 0.0);
}
