//! 消息附带的文件类型。
//!
//! 覆盖图片、语音与普通文件；字段与 Bridge 的 `MessageFile` 一致，
//! `type` 在 Rust 里是关键字，因此线上字段名经 `rename` 映射。

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
    /// 文件类型，对应线上的 `type` 字段。
    #[serde(rename = "type")]
    pub file_type: String,
    /// 文件地址；历史文件消息在 URL 尚未解析出来时会省略该字段。
    #[serde(default)]
    pub url: String,
    /// 文件大小（字节），缺失时为 None。
    pub size: Option<i64>,
    /// 文件名。
    pub name: Option<String>,
    /// 文件 ID，部分操作（如下载、转发）需要它。
    pub fid: Option<String>,
}

impl MessageFile {
    /// 文件名引用；未提供时为 None。
    pub fn get_name(&self) -> Option<&String> {
        self.name.as_ref()
    }
    /// 文件 ID 引用；未提供时为 None。
    pub fn get_fid(&self) -> Option<&String> {
        self.fid.as_ref()
    }
}
