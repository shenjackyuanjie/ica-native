use super::*;

#[test]
fn group_color_becomes_darker_as_member_count_grows() {
    let small = relation_group_color(Some(10));
    let medium = relation_group_color(Some(1_000));
    let large = relation_group_color(Some(50_000));
    let brightness =
        |color: egui::Color32| u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b());

    assert!(brightness(small) > brightness(medium));
    assert!(brightness(medium) > brightness(large));
    assert_eq!(relation_group_color(None), RelationNodeKind::Group.color());
}
