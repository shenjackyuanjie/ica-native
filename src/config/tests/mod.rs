use super::*;

fn bridge(name: &str) -> IcaBridge {
    IcaBridge {
        name: name.to_string(),
        url: "ws://127.0.0.1:9999".to_string(),
        private_key: String::new(),
        enable: true,
        allow_protocol_mismatch: false,
    }
}

#[test]
fn noticer_non_loopback_listener_requires_authentication() {
    let mut noticer = NoticerConfig {
        enabled: true,
        host: "0.0.0.0".to_string(),
        default_bridge: "main".to_string(),
        ..NoticerConfig::default()
    };
    let bridges = [bridge("main")];
    let error = noticer.validate(&bridges).unwrap_err();
    assert!(error.to_string().contains("必须配置 auth_token"));

    noticer.auth_token = "configured".to_string();
    noticer.validate(&bridges).unwrap();
}

#[test]
fn noticer_routes_must_reference_an_enabled_bridge() {
    let mut noticer = NoticerConfig {
        enabled: true,
        default_bridge: "main".to_string(),
        ..NoticerConfig::default()
    };
    noticer.rooms.insert(
        "warning".to_string(),
        NoticerRoom {
            bridge: "secondary".to_string(),
            room_id: -456,
            description: String::new(),
        },
    );
    let error = noticer.validate(&[bridge("main")]).unwrap_err();
    assert!(error.to_string().contains("未启用的 bridge"));

    noticer
        .validate(&[bridge("main"), bridge("secondary")])
        .unwrap();
}
