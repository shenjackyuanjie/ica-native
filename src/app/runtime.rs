use tokio::runtime::Runtime;
use tokio::sync::{mpsc::UnboundedReceiver, mpsc::unbounded_channel, oneshot};

use crate::config::IcaCfg;
use crate::ica::{self, BridgeEvent, BridgeHandle};

pub struct BridgeConnection {
    pub key: String,
    pub handle: BridgeHandle,
    stop_sender: Option<oneshot::Sender<()>>,
}

impl BridgeConnection {
    pub fn stop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
    }
}

pub struct AppRuntime {
    tokio: Runtime,
    connections: Vec<BridgeConnection>,
    event_rx: Option<UnboundedReceiver<BridgeEvent>>,
}

impl AppRuntime {
    pub fn new(config: &IcaCfg) -> Self {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("创建 Tokio runtime 失败");
        let (event_tx, event_rx) = unbounded_channel();
        let mut connections = Vec::new();

        for bridge in config
            .bridges
            .iter()
            .filter(|bridge| bridge.enable)
            .cloned()
        {
            let (stop_tx, stop_rx) = oneshot::channel();
            let key = if bridge.name.trim().is_empty() {
                bridge.url.clone()
            } else {
                bridge.name.clone()
            };
            let (command_tx, command_rx) = unbounded_channel();
            connections.push(BridgeConnection {
                key: key.clone(),
                handle: BridgeHandle::new(key.clone(), command_tx),
                stop_sender: Some(stop_tx),
            });
            let tx = event_tx.clone();
            tokio.spawn(async move {
                if let Err(error) = ica::run_bridge(stop_rx, &bridge, tx, command_rx).await {
                    tracing::error!(bridge = %key, error = %error, "Socket.IO bridge 异常停止");
                }
            });
        }

        Self {
            tokio,
            connections,
            event_rx: Some(event_rx),
        }
    }

    pub fn handle(&self) -> tokio::runtime::Handle {
        self.tokio.handle().clone()
    }

    pub fn connections(&self) -> &[BridgeConnection] {
        &self.connections
    }

    pub fn connections_mut(&mut self) -> &mut [BridgeConnection] {
        &mut self.connections
    }

    pub fn take_event_receiver(&mut self) -> UnboundedReceiver<BridgeEvent> {
        self.event_rx.take().expect("bridge 事件接收器只能取一次")
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        for connection in &mut self.connections {
            connection.stop();
        }
    }
}
