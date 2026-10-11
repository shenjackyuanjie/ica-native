use super::{StickerStore, detect_image};

#[test]
fn detects_image_from_content_not_filename() {
    let png = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/png/icon_512x512.png"
    ));
    let detected = detect_image(png).unwrap();
    assert_eq!(detected.extension, "png");
    assert_eq!(detected.mime_type, "image/png");
}

#[test]
fn rejects_corrupt_data() {
    assert!(detect_image(b"not an image").is_err());
}

#[test]
fn favorite_keeps_original_bytes_and_scans_first_level_categories() {
    let unique = format!(
        "ica-native-sticker-test-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let root = std::env::temp_dir().join(unique);
    let config = crate::config::IcaCfg {
        sticker_path: Some(root.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let config_store =
        crate::config::ConfigStore::from_config(config.clone(), root.join("test-config.toml"));
    let store = StickerStore::resolve(&config, config_store.paths()).unwrap();
    let original = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/png/icon_512x512.png"
    ));

    let entry = store.add_bytes(original).unwrap();
    assert_eq!(store.read_entry(&entry).unwrap(), original);
    assert!(entry.name.ends_with(".png"));
    assert_eq!(entry.category, "默认");
    assert_eq!(store.refresh(false).unwrap(), 1);

    let nested = root.join("existing-icalingua-group");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("nested.png"), original).unwrap();
    assert_eq!(store.refresh(false).unwrap(), 2);
    assert_eq!(
        store.entries_in_category("existing-icalingua-group").len(),
        1
    );

    store.create_category("常用").unwrap();
    let root_entry = store.entries_in_category("默认").pop().unwrap();
    store.move_entry(&root_entry, "常用").unwrap();
    assert_eq!(store.refresh(false).unwrap(), 2);
    assert_eq!(store.entries_in_category("常用").len(), 1);

    std::fs::remove_dir_all(root).unwrap();
}
