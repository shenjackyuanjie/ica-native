use serde::{Deserialize, Serialize};

/*interface MessageFile {
    type: string
    url: string
    size?: number
    name?: string
    fid?: string
}
 */
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageFile {
    #[serde(rename = "type")]
    pub file_type: String,
    // 历史文件消息在 URL 尚未解析出来时会省略该字段。
    #[serde(default)]
    pub url: String,
    pub size: Option<i64>,
    pub name: Option<String>,
    pub fid: Option<String>,
}

impl MessageFile {
    pub fn get_name(&self) -> Option<&String> {
        self.name.as_ref()
    }
    pub fn get_fid(&self) -> Option<&String> {
        self.fid.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::MessageFile;

    #[test]
    fn accepts_history_file_without_url_and_large_size() {
        let file: MessageFile = serde_json::from_value(json!({
            "type": "application/octet-stream",
            "size": 3_000_000_000_i64,
            "name": "archive.bin"
        }))
        .expect("历史消息文件应当可以反序列化");

        assert!(file.url.is_empty());
        assert_eq!(file.size, Some(3_000_000_000));
    }
}
