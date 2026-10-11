use super::*;

#[test]
fn avatar_actions_use_full_size_qq_image_but_preserve_custom_sources() {
    let target = AvatarTarget {
        room_id: -123,
        user_id: 456,
        name: "测试成员",
        image_url: "https://q1.qlogo.cn/g?b=qq&nk=456&s=140",
        is_qq_user: true,
    };
    assert_eq!(
        full_size_avatar_url(&target),
        "https://q1.qlogo.cn/g?b=qq&nk=456&s=0"
    );
    let custom_url = "https://example.invalid/avatar.png?size=140";
    assert_eq!(
        full_size_avatar_url(&AvatarTarget {
            image_url: custom_url,
            ..target
        }),
        custom_url,
    );
}
