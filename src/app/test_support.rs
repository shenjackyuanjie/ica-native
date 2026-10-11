//! 应用级测试夹具：内存配置、假 Bridge 命令通道和禁用服务的运行时。
//!
//! 不读取用户配置或贴纸，不启动真实 Bridge、Noticer 或 Agent 上下文服务。
//! 需要验证 HTTP 服务的用例必须自行显式启用，并使用随机回环端口。

use super::{
    AppState, BridgeSession, BridgeState, IcaApp, runtime::AppRuntime, stickers::StickerStore,
};
use crate::config::{ChatGroups, ConfigStore, IcaCfg};
use crate::ica::{BridgeHandle, IcaCommand};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};

pub fn app_with_bridges(
    config: IcaCfg,
    bridges: Vec<BridgeState>,
) -> (IcaApp, Vec<mpsc::UnboundedReceiver<IcaCommand>>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    // 只生成独立路径，不创建目录或加载磁盘数据，避免并行用例共用配置文件。
    let path = std::env::temp_dir().join(format!(
        "ica-native-app-test-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed),
    ));
    let store = ConfigStore::from_config(config.clone(), path.join("config.toml"));
    let mut receivers = Vec::with_capacity(bridges.len());
    let sessions = bridges
        .into_iter()
        .map(|bridge| {
            let (tx, rx) = mpsc::unbounded_channel();
            receivers.push(rx);
            let (stop, _) = oneshot::channel();
            BridgeSession::new(
                BridgeHandle::new(bridge.bridge_key.clone(), tx),
                bridge,
                stop,
            )
        })
        .collect();
    let state = AppState::new(
        &config,
        &store,
        sessions,
        StickerStore::unavailable(path.join("stickers"), "测试夹具"),
    );

    // GUI 权限状态可以启用，但构造夹具不能因此启动网络服务或真实连接。
    let mut runtime_config = config.clone();
    runtime_config.bridges.clear();
    runtime_config.noticer.enabled = false;
    runtime_config.agent_context.enabled = false;
    runtime_config.tokio_rt_work_thread = 1;
    (
        IcaApp {
            runtime: AppRuntime::new(&egui::Context::default(), &runtime_config),
            config: store,
            state,
            chat_windows: Vec::new(),
        },
        receivers,
    )
}

/// 单 Bridge、选中 -123 会话的聊天夹具；接收端用于验证命令或保持通道可用。
pub fn test_app() -> (IcaApp, mpsc::UnboundedReceiver<IcaCommand>) {
    let mut bridge = BridgeState::new("chat-test".into(), ChatGroups::default());
    bridge.selected_room_id = Some(-123);
    bridge.conversation_mut(-123).requested_snapshot = true;
    let (app, mut receivers) = app_with_bridges(IcaCfg::default(), vec![bridge]);
    (app, receivers.pop().unwrap())
}
