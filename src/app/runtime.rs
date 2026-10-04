use std::{collections::HashMap, ops::Deref, sync::Arc};

use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::config::IcaCfg;
use crate::ica::{self, BridgeEvent, BridgeHandle};
use crate::noticer::{self, BridgeRegistry, NoticerController};

use super::event::AppEvent;
use super::state::{BridgeSession, BridgeState};

pub struct AppRuntime {
    tokio: Runtime,
    sessions: Vec<BridgeSession>,
    pub event_rx: UnboundedReceiver<AppEvent>,
    pub event_tx: UnboundedSender<AppEvent>,
    noticer_controller: NoticerController,
    noticer_registry: Arc<BridgeRegistry>,
    noticer_handles: HashMap<String, BridgeHandle>,
}

impl AppRuntime {
    pub fn new(ctx: &egui::Context, config: &IcaCfg) -> Self {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("创建 Tokio runtime 失败");

        let enabled_bridges = config
            .bridges
            .iter()
            .filter(|bridge| bridge.enable)
            .cloned()
            .collect::<Vec<_>>();
        let noticer_registry = Arc::new(BridgeRegistry::new(
            enabled_bridges
                .iter()
                .map(|bridge| bridge.key().to_string()),
        ));

        let (bridge_tx, mut bridge_rx) = unbounded_channel::<BridgeEvent>();
        let (event_tx, event_rx) = unbounded_channel::<AppEvent>();

        let mut sessions = Vec::new();
        let mut noticer_handles = HashMap::new();
        for bridge in enabled_bridges {
            let (stop_tx, stop_rx) = oneshot::channel();
            let bridge_key = bridge.key().to_string();
            let (command_tx, command_rx) = unbounded_channel();
            let handle = BridgeHandle::new(bridge_key.clone(), command_tx);
            noticer_handles.insert(bridge_key.clone(), handle.clone());
            let state = BridgeState::new(bridge_key.clone(), config.chat_groups.clone());
            sessions.push(BridgeSession::new(handle, state, stop_tx));

            let event_tx = bridge_tx.clone();
            tokio.spawn(async move {
                if let Err(error) = ica::run_bridge(stop_rx, &bridge, event_tx, command_rx).await {
                    tracing::error!(bridge = %bridge_key, error = %error, "Socket.IO bridge 异常停止");
                }
            });
        }

        let image_loader = crate::image_loader::nt_image::NtImageLoader::install(
            ctx,
            tokio.handle().clone(),
            config.image_cache_max_bytes,
        );
        let image_handles = noticer_handles.clone();
        let forward_tx = event_tx.clone();
        let repaint_ctx = ctx.clone();
        let registry = noticer_registry.clone();
        tokio.spawn(async move {
            while let Some(event) = bridge_rx.recv().await {
                registry.observe(&event);
                image_loader.observe(&event, &image_handles);
                if forward_tx.send(AppEvent::Bridge(event)).is_err() {
                    break;
                }
                repaint_ctx.request_repaint();
            }
        });

        let noticer_controller = noticer::spawn(
            &tokio,
            config.noticer.clone(),
            noticer_handles.clone(),
            noticer_registry.clone(),
        );

        Self {
            tokio,
            sessions,
            event_rx,
            event_tx,
            noticer_controller,
            noticer_registry,
            noticer_handles,
        }
    }

    pub fn take_sessions(&mut self) -> Vec<BridgeSession> {
        std::mem::take(&mut self.sessions)
    }

    pub fn event_sender(&self) -> UnboundedSender<AppEvent> {
        self.event_tx.clone()
    }

    pub fn apply_noticer_config(&self, config: crate::config::NoticerConfig) {
        self.noticer_controller.apply(
            config,
            self.noticer_handles.clone(),
            self.noticer_registry.clone(),
        );
    }
}

impl Deref for AppRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Self::Target {
        &self.tokio
    }
}
