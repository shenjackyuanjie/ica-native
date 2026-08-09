use std::ops::Deref;

use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::config::IcaCfg;
use crate::ica::{self, BridgeEvent, BridgeHandle};

use super::event::AppEvent;
use super::state::{BridgeSession, BridgeState};

pub struct AppRuntime {
    tokio: Runtime,
    sessions: Vec<BridgeSession>,
    pub(super) event_rx: UnboundedReceiver<AppEvent>,
    pub(super) event_tx: UnboundedSender<AppEvent>,
}

impl AppRuntime {
    pub fn new(ctx: &egui::Context, config: &IcaCfg) -> Self {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("创建 Tokio runtime 失败");

        let (bridge_tx, mut bridge_rx) = unbounded_channel::<BridgeEvent>();
        let (event_tx, event_rx) = unbounded_channel::<AppEvent>();
        let forward_tx = event_tx.clone();
        let repaint_ctx = ctx.clone();
        tokio.spawn(async move {
            while let Some(event) = bridge_rx.recv().await {
                if forward_tx.send(AppEvent::Bridge(event)).is_err() {
                    break;
                }
                repaint_ctx.request_repaint();
            }
        });

        let mut sessions = Vec::new();
        for bridge in config
            .bridges
            .iter()
            .filter(|bridge| bridge.enable)
            .cloned()
        {
            let (stop_tx, stop_rx) = oneshot::channel();
            let bridge_key = if bridge.name.is_empty() {
                bridge.url.clone()
            } else {
                bridge.name.clone()
            };
            let (command_tx, command_rx) = unbounded_channel();
            let handle = BridgeHandle::new(bridge_key.clone(), command_tx);
            let state = BridgeState::new(bridge_key.clone(), config.chat_groups.clone());
            sessions.push(BridgeSession::new(handle, state, stop_tx));

            let event_tx = bridge_tx.clone();
            tokio.spawn(async move {
                if let Err(error) = ica::run_bridge(stop_rx, &bridge, event_tx, command_rx).await {
                    tracing::error!(bridge = %bridge_key, error = %error, "Socket.IO bridge 异常停止");
                }
            });
        }

        Self {
            tokio,
            sessions,
            event_rx,
            event_tx,
        }
    }

    pub fn take_sessions(&mut self) -> Vec<BridgeSession> {
        std::mem::take(&mut self.sessions)
    }

    pub fn event_sender(&self) -> UnboundedSender<AppEvent> {
        self.event_tx.clone()
    }
}

impl Deref for AppRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Self::Target {
        &self.tokio
    }
}
