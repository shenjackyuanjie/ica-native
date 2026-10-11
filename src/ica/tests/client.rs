use super::poke_target;

#[test]
fn group_poke_uses_positive_group_id_without_changing_the_member() {
    assert_eq!(poke_target(-123, 456), Some((123, 456)));
}

#[test]
fn private_poke_uses_the_peer_for_both_oicq_arguments() {
    assert_eq!(poke_target(123, 123), Some((123, 123)));
    assert_eq!(poke_target(123, 456), Some((123, 123)));
}

#[test]
fn invalid_poke_targets_are_rejected_without_overflow() {
    for (room_id, target_id) in [(0, 123), (i64::MIN, 123), (-123, 0), (-123, -1)] {
        assert_eq!(poke_target(room_id, target_id), None);
    }
}
