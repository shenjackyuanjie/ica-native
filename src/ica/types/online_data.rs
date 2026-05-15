use serde_json::Value as JsonValue;
use tracing::warn;

#[derive(Debug, Clone, Hash)]
pub struct IcalinguaInfo {
    pub ica_version: String,
    pub os_info: String,
    pub resident_set_size: String,
    pub heap_used: String,
    pub load: String,
    pub server_node: String,
    pub client_count: u16,
}

impl IcalinguaInfo {
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
                            warn!("client_count parse error: {}|raw: {}", e, info);
                            1
                        }),
                );
            }
        }

        Self {
            ica_version: ica_version.unwrap_or_else(|| {
                warn!("ica_version failed to parse");
                "UNKNOWN".to_string()
            }),
            os_info: os_info.unwrap_or_else(|| {
                warn!("os_info failed to parse");
                "UNKNOWN".to_string()
            }),
            resident_set_size: resident_set_size.unwrap_or_else(|| {
                warn!("resident_set_size failed to parse");
                "UNKNOWN".to_string()
            }),
            heap_used: heap_used.unwrap_or_else(|| {
                warn!("heap_used failed to parse");
                "UNKNOWN".to_string()
            }),
            load: load.unwrap_or_else(|| {
                warn!("load failed to parse");
                "UNKNOWN".to_string()
            }),
            server_node: server_node.unwrap_or_else(|| {
                warn!("server_node failed to parse");
                "UNKNOWN".to_string()
            }),
            client_count: client_count.unwrap_or_else(|| {
                warn!("client_count failed to parse");
                1
            }),
        }
    }
}

#[derive(Debug, Clone, Hash)]
pub struct OnlineData {
    pub bkn: i64,
    pub nick: String,
    pub online: bool,
    pub qqid: i64,
    pub icalingua_info: IcalinguaInfo,
}

impl OnlineData {
    pub fn new_from_json(value: &JsonValue) -> Self {
        let bkn = value["bkn"].as_i64().unwrap_or_else(|| {
            warn!("bkn not found in online data");
            -1
        });
        let nick = value["nick"]
            .as_str()
            .unwrap_or_else(|| {
                warn!("nick not found in online data");
                "UNKNOWN"
            })
            .to_string();
        let online = value["online"].as_bool().unwrap_or_else(|| {
            warn!("online not found in online data");
            false
        });
        let qqid = value["uin"].as_i64().unwrap_or_else(|| {
            warn!("uin not found in online data");
            -1
        });
        let sys_info = value["sysInfo"].as_str().unwrap_or_else(|| {
            warn!("sysInfo not found in online data");
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
