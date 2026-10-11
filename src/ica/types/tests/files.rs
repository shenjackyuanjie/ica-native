use super::MessageFile;
use serde_json::json;

#[test]
fn history_file_without_url_defaults_to_empty() {
    let file: MessageFile = serde_json::from_value(json!({
        "type": "application/octet-stream",
        "name": "archive.bin"
    }))
    .expect("历史消息文件应当可以反序列化");

    assert!(file.url.is_empty());
}
