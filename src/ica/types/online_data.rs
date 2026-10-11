//! 在线状态与 Bridge 自身信息。
//!
//! `OnlineData` 由 Bridge 的 `onlineData` 事件下发；`IcalinguaInfo` 解析
//! Bridge 打印的多行状态文本，缺字段时记警告并回退默认值，保证界面总能展示。

use serde_json::Value as JsonValue;
use tracing::warn;

/// Bridge 运行时信息，来自 `onlineData` 里的 `sysInfo` 多行文本。
#[derive(Debug, Clone, Hash)]
pub struct IcalinguaInfo {
    /// Bridge 版本。
    pub ica_version: String,
    /// 运行环境（操作系统信息）。
    pub os_info: String,
    /// 常驻内存大小。
    pub resident_set_size: String,
    /// 堆占用。
    pub heap_used: String,
    /// 负载。
    pub load: String,
    /// 服务节点。
    pub server_node: String,
    /// 已连接客户端数。
    pub client_count: u16,
}

impl IcalinguaInfo {
    /// 从 Bridge 的多行状态文本解析；每个字段缺失都会记警告并回退默认值。
    pub fn new_from_str(s: &str) -> Self {
        let mut ica_version = None;
        let mut os_info = None;
        let mut resident_set_size = None;
        let mut heap_used = None;
        let mut load = None;
        let mut server_node = None;
        let mut client_count = None;

        for info in s.split('\n') {
            if info.starts_with("icalingua-bridge-oicq") {
                ica_version = Some(info.split_at(22).1.to_string());
            } else if info.starts_with("Running on") {
                os_info = Some(info.split_at(11).1.to_string());
            } else if info.starts_with("Resident Set Size") {
                resident_set_size = Some(info.split_at(18).1.to_string());
            } else if info.starts_with("Heap used") {
                heap_used = Some(info.split_at(10).1.to_string());
            } else if info.starts_with("Load") {
                load = Some(info.split_at(5).1.to_string());
            } else if info.starts_with("Server Node") {
                server_node = Some(info.split_at(12).1.to_string());
            } else if info.ends_with("clients connected") {
                client_count = Some(
                    info.split(' ')
                        .next()
                        .unwrap_or("1")
                        .parse::<u16>()
                        .unwrap_or_else(|e| {
                            warn!("client_count 解析失败: {}|原始数据: {}", e, info);
                            1
                        }),
                );
            }
        }

        Self {
            ica_version: ica_version.unwrap_or_else(|| {
                warn!("ica_version 解析失败");
                "UNKNOWN".to_string()
            }),
            os_info: os_info.unwrap_or_else(|| {
                warn!("os_info 解析失败");
                "UNKNOWN".to_string()
            }),
            resident_set_size: resident_set_size.unwrap_or_else(|| {
                warn!("resident_set_size 解析失败");
                "UNKNOWN".to_string()
            }),
            heap_used: heap_used.unwrap_or_else(|| {
                warn!("heap_used 解析失败");
                "UNKNOWN".to_string()
            }),
            load: load.unwrap_or_else(|| {
                warn!("load 解析失败");
                "UNKNOWN".to_string()
            }),
            server_node: server_node.unwrap_or_else(|| {
                warn!("server_node 解析失败");
                "UNKNOWN".to_string()
            }),
            client_count: client_count.unwrap_or_else(|| {
                warn!("client_count 解析失败");
                1
            }),
        }
    }
}

/// 账号在线状态与 Bridge 信息，由 `onlineData` 事件下发。
#[derive(Debug, Clone, Hash)]
pub struct OnlineData {
    /// 鉴权用 bkn；缺失时为 -1。
    pub bkn: i64,
    /// 账号昵称。
    pub nick: String,
    /// 是否在线。
    pub online: bool,
    /// 账号 QQ。
    pub qqid: i64,
    /// Bridge 自身信息。
    pub icalingua_info: IcalinguaInfo,
}

impl OnlineData {
    /// 从 `onlineData` 事件的 JSON 解析；字段缺失时记警告并回退默认值。
    pub fn new_from_json(value: &JsonValue) -> Self {
        let bkn = value["bkn"].as_i64().unwrap_or_else(|| {
            warn!("online data 中缺少 bkn");
            -1
        });
        let nick = value["nick"]
            .as_str()
            .unwrap_or_else(|| {
                warn!("online data 中缺少 nick");
                "UNKNOWN"
            })
            .to_string();
        let online = value["online"].as_bool().unwrap_or_else(|| {
            warn!("online data 中缺少 online");
            false
        });
        let qqid = value["uin"].as_i64().unwrap_or_else(|| {
            warn!("online data 中缺少 uin");
            -1
        });
        let sys_info = value["sysInfo"].as_str().unwrap_or_else(|| {
            warn!("online data 中缺少 sysInfo");
            ""
        });

        Self {
            bkn,
            nick,
            online,
            qqid,
            icalingua_info: IcalinguaInfo::new_from_str(sys_info),
        }
    }
}

/// 未收到 `onlineData` 时的占位值：bkn 为 -1 表示不可用。
impl Default for OnlineData {
    fn default() -> Self {
        Self {
            bkn: -1,
            nick: "UNKNOWN".to_string(),
            online: false,
            qqid: -1,
            icalingua_info: IcalinguaInfo {
                ica_version: "UNKNOWN".to_string(),
                os_info: "UNKNOWN".to_string(),
                resident_set_size: "UNKNOWN".to_string(),
                heap_used: "UNKNOWN".to_string(),
                load: "UNKNOWN".to_string(),
                server_node: "UNKNOWN".to_string(),
                client_count: 1,
            },
        }
    }
}
