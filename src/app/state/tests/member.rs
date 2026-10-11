use super::GroupMember;
use serde_json::json;

fn member(user_id: i64, role: &str) -> GroupMember {
    serde_json::from_value(json!({
        "user_id": user_id,
        "nickname": user_id.to_string(),
        "role": role,
        "shutup_time": 100,
    }))
    .unwrap()
}

#[test]
fn mute_boundary_and_moderation_permissions_match_group_roles() {
    let owner = member(1, "owner");
    let admin = member(2, "admin");
    let regular = member(3, "member");

    assert!(regular.is_muted_at(99));
    assert!(!regular.is_muted_at(100));
    assert_eq!(regular.remaining_mute_seconds_at(98), 2);
    assert!(GroupMember::moderation_denial_reason(Some(&owner), &admin, 1).is_none());
    assert!(GroupMember::moderation_denial_reason(Some(&admin), &regular, 2).is_none());
    assert!(GroupMember::moderation_denial_reason(Some(&admin), &owner, 2).is_some());
    assert!(GroupMember::moderation_denial_reason(Some(&regular), &admin, 3).is_some());
    assert!(GroupMember::moderation_denial_reason(Some(&owner), &owner, 1).is_some());
}
