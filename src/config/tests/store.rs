use super::*;

#[test]
fn server_owned_chat_groups_are_not_persisted_or_loaded() {
    let mut config = IcaCfg::default();
    config
        .chat_groups
        .groups
        .push(super::super::chat_groups::ChatGroup::new(
            "服务端分组",
            vec![123],
        ));

    let encoded = toml::to_string(&config).unwrap();
    assert!(!encoded.contains("chat_groups"));

    let decoded: IcaCfg = toml::from_str(
        r#"
            [[chat_groups.groups]]
            name = "旧本地分组"
            rooms = [123]
        "#,
    )
    .unwrap();
    assert!(decoded.chat_groups.groups.is_empty());
}

#[test]
fn command_line_config_path_has_priority() {
    let (path, source) = resolve_config_path(["--config", "custom.toml"]);
    assert_eq!(path, PathBuf::from("custom.toml"));
    assert_eq!(source, ConfigPathSource::CommandLine);
}
