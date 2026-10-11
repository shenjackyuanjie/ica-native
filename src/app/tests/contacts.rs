use super::{ContactDirectory, create_contact_room};
use crate::ica::types::contact::FriendContact;

#[test]
fn stale_contact_responses_do_not_replace_newer_refresh() {
    let mut contacts = ContactDirectory::default();
    let old_request = contacts.begin_refresh();
    let current_request = contacts.begin_refresh();
    contacts.apply_friends(
        old_request,
        vec![FriendContact {
            uin: 1,
            nick: "old".to_string(),
            remark: String::new(),
        }],
    );
    assert!(contacts.friends.is_empty());

    contacts.apply_friends(
        current_request,
        vec![FriendContact {
            uin: 2,
            nick: "new".to_string(),
            remark: String::new(),
        }],
    );
    assert_eq!(contacts.friends[0].uin, 2);
}

#[test]
fn contact_rooms_match_icalingua_priorities_and_legacy_users() {
    let friend = create_contact_room(10001, "好友".to_string(), 123);
    let group = create_contact_room(-20002, "群".to_string(), 456);

    assert_eq!(friend.priority, 4);
    assert_eq!(friend.users.as_array().unwrap().len(), 2);
    assert_eq!(group.priority, 2);
    assert_eq!(group.users.as_array().unwrap().len(), 3);
}
