//! 应用运行时：Tokio runtime、Bridge 会话与后台服务的装配中心。
//!
//! `AppRuntime` 持有多线程 Tokio runtime、所有启用 Bridge 的 `BridgeSession`，
//! 以及向 egui 主线程投递事件的无界通道；同时把 Bridge 事件经 noticer 注册表与
//! 图片加载器观察后转发给 UI，并启动 noticer 与 agent-context 两个后台服务。

use super::event::AppEvent;
use super::state::{BridgeSession, BridgeState};
use crate::config::IcaCfg;
use crate::ica::{self, BridgeEvent, BridgeHandle};
use crate::noticer::{self, BridgeRegistry, NoticerController};
use std::{collections::HashMap, ops::Deref, sync::Arc};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

/// 应用级运行时：聚合 Tokio runtime、Bridge 会话与跨线程事件通道。
pub struct AppRuntime {
    /// 多线程 Tokio runtime，所有 Bridge 与后台任务都在其上 spawn。
    tokio: Runtime,
    /// 各启用 Bridge 的会话（含停止信号与协议状态），`take_sessions` 后移交给 UI。
    sessions: Vec<BridgeSession>,
    /// 接收端：egui 主线程在 `poll_socketio_events` 中非阻塞消费。
    pub event_rx: UnboundedReceiver<AppEvent>,
    /// 发送端：后台任务（媒体、agent 上下文等）克隆它来投递事件。
    pub event_tx: UnboundedSender<AppEvent>,
    /// noticer 控制器，用于运行时热更新通知配置。
    noticer_controller: NoticerController,
    /// agent 上下文服务控制器，供 UI 读取状态/热更新配置。
    pub agent_context_controller: crate::agent_context::server::Controller,
    /// 跨 Bridge 的注册表，供 noticer 与图片加载器按 key 查找连接。
    noticer_registry: Arc<BridgeRegistry>,
    /// bridge_key -> 命令句柄，供 noticer 向具体连接下发命令。
    noticer_handles: HashMap<String, BridgeHandle>,
}

/// 运行时的构造与对外控制面。
impl AppRuntime {
    /// 装配运行时：建 Tokio runtime，为每个启用 Bridge 建立会话并 spawn 连接任务，
    /// 安装图片加载器，启动事件转发、noticer 与 agent-context 服务。
    ///
    /// `ctx` 用于注册 egui 图片加载器与请求重绘；`config` 决定启用哪些 Bridge、
    /// Tokio 工作线程数与各后台服务配置。
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
                // 先让 noticer 注册表与图片加载器各观察一眼，再转发给 UI；
                // 发送失败说明接收端已关闭，退出循环。每次转发都请求重绘以驱动界面刷新。
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

        let agent_context_controller = crate::agent_context::server::spawn(
            &tokio,
            config.agent_context.clone(),
            event_tx.clone(),
            ctx.clone(),
        );

        Self {
            agent_context_controller,
            tokio,
            sessions,
            event_rx,
            event_tx,
            noticer_controller,
            noticer_registry,
            noticer_handles,
        }
    }

    /// 取走全部 Bridge 会话（移交给 `AppState` 后运行时不再持有它们）。
    pub fn take_sessions(&mut self) -> Vec<BridgeSession> {
        std::mem::take(&mut self.sessions)
    }

    /// 克隆一份事件发送端，供媒体等后台任务投递 `AppEvent`。
    pub fn event_sender(&self) -> UnboundedSender<AppEvent> {
        self.event_tx.clone()
    }

    /// 热应用新的 noticer 配置（不重启连接，仅更新通知规则）。
    pub fn apply_noticer_config(&self, config: crate::config::NoticerConfig) {
        self.noticer_controller.apply(
            config,
            self.noticer_handles.clone(),
            self.noticer_registry.clone(),
        );
    }
}

/// 让 `AppRuntime` 可直接当 Tokio `Runtime` 用（如 `runtime.spawn`）。
impl Deref for AppRuntime {
    /// 解引用目标为内部的 Tokio runtime。
    type Target = Runtime;

    /// 借用内部 Tokio runtime。
    fn deref(&self) -> &Self::Target {
        &self.tokio
    }
}
