//! GUI 是已打开会话及授权状态的唯一来源，不经 Bridge 获取任何消息。

mod settings;
#[cfg(test)]
mod tests;

use super::IcaApp;
use crate::agent_context::server::{GuiRequest, Operation};
use crate::agent_context::{
    self, AgentContextConfig, ApiError, Context, QueryResult, Snapshot, Target,
};
use std::{collections::HashSet, time::Duration};

/// 待批准的读取请求；`snapshot` 为已生成但等待用户批准的快照。
pub struct PendingRead {
    /// 原始请求。
    pub request: GuiRequest,
    /// 已生成的快照（指定目标时预生成，待批准后交付）。
    pub snapshot: Option<Box<Snapshot>>,
}

/// Agent 上下文界面的草稿状态与待处理请求。
pub struct AgentContextUi {
    /// 当前待批准请求。
    pub pending: Option<PendingRead>,
    /// 设置界面中的编辑草稿。
    pub draft: AgentContextConfig,
    /// 已保存/已应用的配置，用于判断有无未保存修改。
    pub loaded: AgentContextConfig,
    /// 设置界面错误提示。
    pub settings_error: Option<String>,
    /// 设置界面成功提示。
    pub settings_notice: Option<String>,
}
impl AgentContextUi {
    /// 用当前配置初始化（草稿与已加载一致）。
    pub fn new(config: &AgentContextConfig) -> Self {
        Self {
            pending: None,
            draft: config.clone(),
            loaded: config.clone(),
            settings_error: None,
            settings_notice: None,
        }
    }
}
impl IcaApp {
    /// 把配置存储中的 Agent 上下文配置同步到运行时与草稿。
    ///
    /// 配置变化时应用新配置、作废待处理请求；本地无未保存修改时同步草稿。
    pub fn sync_agent_context_config(&mut self) {
        let current = self.config.snapshot().agent_context;
        if current != self.agent_context_ui.loaded {
            self.runtime.agent_context_controller.apply(current.clone());
            if let Some(pending) = self.agent_context_ui.pending.take() {
                pending.request.finish(Err(ApiError::cancelled()));
            }
            if self.agent_context_ui.draft == self.agent_context_ui.loaded {
                self.agent_context_ui.draft = current.clone();
            }
            self.agent_context_ui.loaded = current;
        }
        self.prune_agent_context_request();
    }

    /// 返回当前已打开的会话目录：活跃 bridge 的选中会话 + 各 bridge 的独立窗口会话。
    ///
    /// 按 (bridge, room_id) 去重，并附带房间显示名。
    pub fn opened_agent_contexts(&self) -> Vec<Context> {
        let mut targets = Vec::new();
        if let Some(index) = self.active_bridge_idx
            && let Some(session) = self.bridge_states.get(index)
            && let Some(room_id) = session.selected_room_id
        {
            targets.push(Target {
                bridge: session.bridge_key.clone(),
                room_id,
            });
        }
        for session in &self.bridge_states {
            let mut ids: Vec<_> = session.detached_room_ids.iter().copied().collect();
            ids.sort_unstable();
            targets.extend(ids.into_iter().map(|room_id| Target {
                bridge: session.bridge_key.clone(),
                room_id,
            }));
        }
        let mut seen = HashSet::new();
        targets
            .into_iter()
            .filter(|target| seen.insert(target.clone()))
            .filter_map(|target| {
                let session = self
                    .bridge_states
                    .iter()
                    .find(|session| session.bridge_key == target.bridge)?;
                let room_name = session
                    .rooms
                    .iter()
                    .find(|room| room.room_id == target.room_id)
                    .map(|room| room.room_name.clone())
                    .unwrap_or_else(|| target.room_id.to_string());
                Some(Context { target, room_name })
            })
            .collect()
    }

    /// 为指定目标生成快照：仅使用该会话已加载的消息与当前多选集合。
    ///
    /// 目标未打开或会话不存在时返回 `target_not_open`。
    fn snapshot_agent_context(
        &self,
        target: &Target,
        request: &GuiRequest,
    ) -> Result<Snapshot, ApiError> {
        let context = self
            .opened_agent_contexts()
            .into_iter()
            .find(|c| c.target == *target)
            .ok_or_else(ApiError::target_not_open)?;
        let session = self
            .bridge_states
            .iter()
            .find(|s| s.bridge_key == target.bridge)
            .ok_or_else(ApiError::target_not_open)?;
        let Operation::Read(params) = &request.operation else {
            return Err(ApiError::internal());
        };
        let messages = session
            .conversation(target.room_id)
            .map(|c| c.messages.as_slice())
            .unwrap_or(&[]);
        let selected = if session.forward_room_id == Some(target.room_id) {
            session
                .forward_selected_message_ids
                .iter()
                .map(String::as_str)
                .collect()
        } else {
            HashSet::new()
        };
        agent_context::snapshot(context, params, messages, &selected)
    }

    /// 处理来自 HTTP 服务的请求。
    ///
    /// 列表请求直接按白名单过滤返回；读取请求先校验，白名单目标直接生成
    /// 快照交付，其余进入待批准队列（同时只允许一个待批准请求）。
    pub fn handle_agent_context_request(&mut self, request: GuiRequest) {
        self.prune_agent_context_request();
        if !request.valid() || *request.config != self.config.snapshot().agent_context {
            request.finish(Err(ApiError::cancelled()));
            return;
        }
        match &request.operation {
            Operation::List => {
                let contexts = self
                    .opened_agent_contexts()
                    .into_iter()
                    .filter(|c| request.config.allows(&c.target))
                    .collect();
                request.finish(Ok(QueryResult::Contexts { contexts }));
            }
            Operation::Read(params) => {
                if let Err(error) = params.validate() {
                    request.finish(Err(error));
                    return;
                }
                if self.agent_context_ui.pending.is_some() {
                    request.finish(Err(ApiError::new(
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        "busy",
                        "已有待批准请求",
                    )));
                    return;
                }
                if let Some(target) = params.target.clone() {
                    self.choose_agent_context(request, &target);
                } else if self.opened_agent_contexts().is_empty() {
                    request.finish(Err(ApiError::target_not_open()));
                } else {
                    self.agent_context_ui.pending = Some(PendingRead {
                        request,
                        snapshot: None,
                    });
                }
            }
        }
    }

    /// 用户在批准弹窗中选定目标后：白名单直接交付，否则预生成快照转待批准。
    fn choose_agent_context(&mut self, request: GuiRequest, target: &Target) {
        if !request.valid() {
            request.finish(Err(ApiError::cancelled()));
            return;
        }
        match self.snapshot_agent_context(target, &request) {
            Err(error) => request.finish(Err(error)),
            Ok(snapshot) if request.config.allows(target) => {
                request.finish(Ok(QueryResult::Snapshot(Box::new(snapshot))))
            }
            Ok(snapshot) => {
                self.agent_context_ui.pending = Some(PendingRead {
                    request,
                    snapshot: Some(Box::new(snapshot)),
                })
            }
        }
    }

    /// 清理已失效的待处理请求（租约过期、配置变更或目标会话已关闭）。
    fn prune_agent_context_request(&mut self) {
        let Some(pending) = self.agent_context_ui.pending.as_ref() else {
            return;
        };
        let error = if !pending.request.valid()
            || *pending.request.config != self.config.snapshot().agent_context
        {
            Some(ApiError::cancelled())
        } else if pending.snapshot.as_ref().is_some_and(|snapshot| {
            !self
                .opened_agent_contexts()
                .iter()
                .any(|c| c.target == snapshot.context.target)
        }) {
            Some(ApiError::target_not_open())
        } else {
            None
        };
        if let Some(error) = error
            && let Some(pending) = self.agent_context_ui.pending.take()
        {
            pending.request.finish(Err(error));
        }
    }

    /// 结束批准流程：批准则交付快照，取消/拒绝则以 `denied` 结束。
    fn finish_agent_context_consent(&mut self, approved: bool) {
        self.prune_agent_context_request();
        if let Some(pending) = self.agent_context_ui.pending.take() {
            let result = if approved {
                pending
                    .snapshot
                    .map(QueryResult::Snapshot)
                    .ok_or_else(ApiError::internal)
            } else {
                Err(ApiError::denied())
            };
            pending.request.finish(result);
        }
    }

    /// 绘制 Agent 上下文的设置窗口与批准弹窗。
    ///
    /// 弹窗展示调用方自述用途与快照概要；白名单会话选中即交付，
    /// 非白名单需逐次批准。等待期间定时重绘以及时移除超时弹窗。
    pub fn render_agent_context(&mut self, ctx: &egui::Context) {
        self.render_agent_context_settings(ctx);
        self.prune_agent_context_request();
        let Some(pending) = self.agent_context_ui.pending.as_ref() else {
            return;
        };
        let candidates = self.opened_agent_contexts();
        let mut chosen = None;
        let mut approve = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new(("agent_context_request", pending.request.id))).show(ctx, |ui| {
            ui.set_max_width(540.0);
            ui.heading("Agent 请求聊天上下文");
            ui.label("仅交付已加载消息，不代表完整历史。消息将返回调用方，并可能被 Agent 发送给模型服务。");
            if let Operation::Read(params) = &pending.request.operation {
                ui.label(format!("读取模式：{}", match params.mode { agent_context::ReadMode::Recent => "近期消息", agent_context::ReadMode::Selected => "多选消息" }));
                if let Some(reason) = &params.reason { ui.label(format!("调用方自述用途（未经验证）：{reason}")); }
            }
            ui.separator();
            if let Some(snapshot) = &pending.snapshot {
                ui.label(format!("{} / {}（{}）", snapshot.context.target.bridge, snapshot.context.room_name, snapshot.context.target.room_id));
                ui.label(format!("本次 {} 条，已加载 {} 条；快照 {}", snapshot.returned_count, snapshot.loaded_count, snapshot.captured_at.to_rfc3339()));
                if let (Some(first), Some(last)) = (snapshot.messages.first(), snapshot.messages.last()) {
                    ui.label(format!("消息时间：{} ～ {}", first.time.to_rfc3339(), last.time.to_rfc3339()));
                }
                ui.weak("该会话不在白名单中。批准仅限本次快照，不会记住授权。");
                approve = ui.button("批准本次读取").clicked();
            } else {
                ui.label("请选择要交出的已打开会话：");
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for context in &candidates {
                        let policy = if pending.request.config.allows(&context.target) { "白名单：选中即交付" } else { "需单次批准" };
                        if ui.button(format!("{} / {}（{}）— {policy}", context.target.bridge, context.room_name, context.target.room_id)).clicked() {
                            chosen = Some(context.target.clone());
                        }
                    }
                });
            }
            ui.separator();
            cancel = ui.button("取消读取").clicked();
        });
        if cancel || response.should_close() {
            self.finish_agent_context_consent(false);
        } else if approve {
            self.finish_agent_context_consent(true);
        } else if let Some(target) = chosen
            && let Some(pending) = self.agent_context_ui.pending.take()
        {
            self.choose_agent_context(pending.request, &target);
        }
        // 即使没有 Socket 事件，也及时移除超时／已失效弹窗；不强抢系统焦点。
        if self.agent_context_ui.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}
