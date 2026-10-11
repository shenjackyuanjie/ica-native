//! 内建 Noticer 的设置编辑器。
//!
//! Noticer 是内置的 Webhook 服务：把消息经 HTTP 投递给外部系统。本编辑器
//! 管理它的服务与鉴权、投递限制、房间路由。编辑采用“草稿 + 已加载”双份
//! 模式：用户改动只进草稿，点保存才校验、写盘并即时应用；外部（其他窗口）
//! 改过配置时，若本地没有未保存修改则自动同步，避免相互覆盖。

use crate::config::{ConfigStore, IcaBridge, NoticerConfig, NoticerRoom};
use crate::noticer::{
    HARD_MAX_BODY_SIZE, HARD_MAX_IDEMPOTENCY_ENTRIES, HARD_MAX_IDEMPOTENCY_TTL_SECONDS,
    HARD_MAX_IMAGE_COUNT, HARD_MAX_IMAGE_SIZE, HARD_MAX_QUEUE_CAPACITY,
    HARD_MAX_QUEUED_IMAGE_BYTES, HARD_MAX_RETRY_ATTEMPTS, HARD_MAX_RETRY_DELAY_SECONDS,
    HARD_MAX_SEND_TIMEOUT_SECONDS, HARD_MAX_TOTAL_IMAGE_SIZE, SUPPORTED_IMAGE_TYPES,
};

/// Noticer 设置编辑器；持有一份草稿与一份“已保存”配置用于比对。
#[derive(Debug, Clone)]
pub struct NoticerEditor {
    /// 编辑中的配置草稿。
    draft: NoticerConfig,
    /// 最近一次保存/加载时的配置，用于判断有无未保存修改。
    loaded: NoticerConfig,
    /// “新增房间”表单：名称。
    new_room_name: String,
    /// “新增房间”表单：所属 bridge key。
    new_room_bridge: String,
    /// “新增房间”表单：Room ID 文本（提交时解析为 i64）。
    new_room_id: String,
    /// “新增房间”表单：描述。
    new_room_description: String,
    /// 当前错误提示（校验/保存失败）。
    error: Option<String>,
    /// 保存成功提示。
    saved_notice: Option<String>,
    /// 已启用 bridge 的配置副本，供下拉框选择。
    bridge_cfgs: Vec<IcaBridge>,
}

impl NoticerEditor {
    /// 从配置存储加载当前 Noticer 配置与 bridge 列表。
    pub fn new(store: &ConfigStore) -> Self {
        let noticer = store.snapshot().noticer;
        let bridge_cfgs = store.snapshot().bridges;
        Self {
            new_room_bridge: noticer.default_bridge.clone(),
            draft: noticer.clone(),
            loaded: noticer,
            new_room_name: String::new(),
            new_room_id: String::new(),
            new_room_description: String::new(),
            error: None,
            saved_notice: None,
            bridge_cfgs,
        }
    }

    /// 从配置存储刷新 bridge 列表与草稿。
    ///
    /// 仅当草稿与已保存配置一致（用户没有未保存修改）时才跟随外部变化，
    /// 否则保留用户正在编辑的内容。
    fn sync_from_store(&mut self, store: &ConfigStore) {
        let config = store.snapshot();
        let current = config.noticer;
        self.bridge_cfgs = config.bridges;
        if self.draft == self.loaded {
            self.draft = current.clone();
            self.loaded = current;
            if self.new_room_bridge.is_empty() {
                self.new_room_bridge = self.draft.default_bridge.clone();
            }
        }
    }

    /// 校验并保存草稿到配置存储。
    ///
    /// 成功时把草稿记为“已保存”并返回它（供调用方即时应用到运行中的服务）；
    /// 失败时记录错误并返回 `None`。
    fn save(&mut self, store: &ConfigStore) -> Option<NoticerConfig> {
        let mut config = store.snapshot();
        config.noticer = self.draft.clone();
        let result = config
            .validate_private_keys()
            .and_then(|()| store.replace(config))
            .and_then(|()| store.save())
            .map_err(|error| error.to_string());
        match result {
            Ok(()) => {
                self.loaded = self.draft.clone();
                self.error = None;
                self.saved_notice =
                    Some("配置已保存并已即时应用；服务启停和监听参数无需重启客户端".to_string());
                Some(self.draft.clone())
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    /// 切换 Noticer 启用状态并立即保存。
    pub fn set_enabled(&mut self, enabled: bool, store: &ConfigStore) -> Option<NoticerConfig> {
        self.draft.enabled = enabled;
        self.save(store)
    }

    /// 绘制设置界面（丢弃“已应用的配置”返回值时使用）。
    pub fn ui(&mut self, ui: &mut egui::Ui, store: &ConfigStore) {
        let _ = self.ui_with_apply(ui, store);
    }

    /// 绘制设置界面，并在发生保存时返回最新配置供调用方即时应用。
    ///
    /// 界面分四节：服务与鉴权、投递与限制、房间路由、不可突破的硬上限
    /// （只读展示）；底部提供保存与撤销未保存修改。
    pub fn ui_with_apply(
        &mut self,
        ui: &mut egui::Ui,
        store: &ConfigStore,
    ) -> Option<NoticerConfig> {
        self.sync_from_store(store);
        ui.heading("Noticer Webhook");
        ui.weak("保存后立即启停服务并应用监听、Token、队列和路由参数。");
        ui.separator();
        ui.heading("Bridge 协议兼容");
        ui.weak(
            "较新的客户端连接旧版 Bridge 时可能缺少部分功能；开启后会显示协议告警并尝试继续连接。",
        );
        for bridge in self.bridge_cfgs.iter_mut().filter(|bridge| bridge.enable) {
            let bridge_label = format!("{} ({}) 允许旧/不匹配协议", bridge.key(), bridge.url);
            let changed = ui
                .checkbox(&mut bridge.allow_protocol_mismatch, bridge_label)
                .changed();
            if changed {
                // 协议兼容开关影响“下次重连”，不等待底部保存，立即写盘；
                // 写盘失败时把开关弹回原值，避免界面显示一个并未持久化的状态。
                let mut config = store.snapshot();
                if let Some(current) = config
                    .bridges
                    .iter_mut()
                    .find(|current| current.key() == bridge.key())
                {
                    current.allow_protocol_mismatch = bridge.allow_protocol_mismatch;
                }
                match config
                    .validate_private_keys()
                    .and_then(|()| store.replace(config))
                {
                    Ok(()) => match store.save() {
                        Ok(()) => {
                            self.saved_notice =
                                Some("Bridge 兼容设置已保存，重连时生效".to_string());
                        }
                        Err(error) => {
                            let mut rollback = store.snapshot();
                            if let Some(current) = rollback
                                .bridges
                                .iter_mut()
                                .find(|current| current.key() == bridge.key())
                            {
                                current.allow_protocol_mismatch = !bridge.allow_protocol_mismatch;
                                bridge.allow_protocol_mismatch = !bridge.allow_protocol_mismatch;
                            }
                            let _ = store.replace(rollback);
                            self.error = Some(error.to_string());
                        }
                    },
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }

        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if let Some(notice) = &self.saved_notice {
            ui.colored_label(egui::Color32::LIGHT_GREEN, notice);
        }

        let mut applied = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::CollapsingHeader::new("服务与鉴权")
                .default_open(true)
                .show(ui, |ui| {
                    ui.checkbox(&mut self.draft.enabled, "启用内建 Noticer");
                    egui::Grid::new("noticer_service_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("监听地址");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.draft.host)
                                    .desired_width(220.0),
                            );
                            ui.end_row();
                            ui.label("监听端口");
                            ui.add(egui::DragValue::new(&mut self.draft.port).range(1..=65535));
                            ui.end_row();
                            ui.label("认证 Token");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.draft.auth_token)
                                    .password(true)
                                    .desired_width(220.0),
                            );
                            ui.end_row();
                            ui.label("Direct API Token");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.draft.direct_token)
                                    .password(true)
                                    .desired_width(220.0),
                            );
                            ui.end_row();
                        });
                });

            egui::CollapsingHeader::new("投递与限制")
                .default_open(true)
                .show(ui, |ui| {
                    let bridges = store
                        .snapshot()
                        .bridges
                        .into_iter()
                        .filter(|bridge| bridge.enable)
                        .map(|bridge| bridge.key().to_string())
                        .collect::<Vec<_>>();
                    egui::Grid::new("noticer_delivery_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("默认 Bridge");
                            egui::ComboBox::from_id_salt("noticer_default_bridge")
                                .selected_text(if self.draft.default_bridge.is_empty() {
                                    "未选择"
                                } else {
                                    &self.draft.default_bridge
                                })
                                .show_ui(ui, |ui| {
                                    for bridge in &bridges {
                                        ui.selectable_value(
                                            &mut self.draft.default_bridge,
                                            bridge.clone(),
                                            bridge,
                                        );
                                    }
                                });
                            ui.end_row();
                            number_row(ui, "队列容量", &mut self.draft.queue_capacity);
                            number_row(ui, "发送超时（秒）", &mut self.draft.send_timeout_seconds);
                            number_row(ui, "重试次数", &mut self.draft.retry_attempts);
                            number_row(ui, "重试间隔（秒）", &mut self.draft.retry_delay_seconds);
                            number_row(
                                ui,
                                "请求体上限（字节）",
                                &mut self.draft.max_body_size_bytes,
                            );
                            number_row(
                                ui,
                                "单图上限（字节）",
                                &mut self.draft.max_image_size_bytes,
                            );
                            number_row(ui, "单次图片数量上限", &mut self.draft.max_image_count);
                            number_row(
                                ui,
                                "多图总大小上限（字节）",
                                &mut self.draft.max_total_image_size_bytes,
                            );
                            number_row(
                                ui,
                                "队列图片内存上限（字节）",
                                &mut self.draft.max_queued_image_bytes,
                            );
                            number_row(
                                ui,
                                "幂等缓存 TTL（秒）",
                                &mut self.draft.idempotency_ttl_seconds,
                            );
                            number_row(
                                ui,
                                "幂等缓存条目上限",
                                &mut self.draft.idempotency_max_entries,
                            );
                        });
                    ui.separator();
                    ui.label(format!(
                        "支持图片类型：{}",
                        SUPPORTED_IMAGE_TYPES.join(", ")
                    ));
                });

            egui::CollapsingHeader::new("房间路由")
                .default_open(true)
                .show(ui, |ui| {
                    let bridges = store
                        .snapshot()
                        .bridges
                        .into_iter()
                        .filter(|bridge| bridge.enable)
                        .map(|bridge| bridge.key().to_string())
                        .collect::<Vec<_>>();
                    let mut removed = None;
                    for (name, room) in &mut self.draft.rooms {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.strong(name);
                                if ui.small_button("删除").clicked() {
                                    removed = Some(name.clone());
                                }
                            });
                            egui::Grid::new(("noticer_room", name))
                                .num_columns(2)
                                .show(ui, |ui| {
                                    ui.label("Bridge");
                                    egui::ComboBox::from_id_salt(("noticer_room_bridge", name))
                                        .selected_text(&room.bridge)
                                        .show_ui(ui, |ui| {
                                            for bridge in &bridges {
                                                ui.selectable_value(
                                                    &mut room.bridge,
                                                    bridge.clone(),
                                                    bridge,
                                                );
                                            }
                                        });
                                    ui.end_row();
                                    ui.label("Room ID");
                                    ui.add(egui::DragValue::new(&mut room.room_id));
                                    ui.end_row();
                                    ui.label("描述");
                                    ui.add(egui::TextEdit::singleline(&mut room.description));
                                    ui.end_row();
                                });
                        });
                        ui.add_space(4.0);
                    }
                    if let Some(name) = removed {
                        self.draft.rooms.remove(&name);
                    }
                    ui.separator();
                    ui.label("新增房间");
                    egui::Grid::new("noticer_new_room_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("名称");
                            ui.add(egui::TextEdit::singleline(&mut self.new_room_name));
                            ui.end_row();
                            ui.label("Bridge");
                            ui.add(egui::TextEdit::singleline(&mut self.new_room_bridge));
                            ui.end_row();
                            ui.label("Room ID");
                            ui.add(egui::TextEdit::singleline(&mut self.new_room_id));
                            ui.end_row();
                            ui.label("描述");
                            ui.add(egui::TextEdit::singleline(&mut self.new_room_description));
                            ui.end_row();
                        });
                    if ui.button("添加房间").clicked() {
                        // 名称非空 + Room ID 为非零整数才允许加入；
                        // 群号写作正数，Room ID 为 0 视为无效输入。
                        match self.new_room_id.trim().parse::<i64>() {
                            Ok(room_id)
                                if !self.new_room_name.trim().is_empty() && room_id != 0 =>
                            {
                                self.draft.rooms.insert(
                                    self.new_room_name.trim().to_string(),
                                    NoticerRoom {
                                        bridge: self.new_room_bridge.trim().to_string(),
                                        room_id,
                                        description: self.new_room_description.trim().to_string(),
                                    },
                                );
                                self.new_room_name.clear();
                                self.new_room_id.clear();
                                self.new_room_description.clear();
                            }
                            _ => {
                                self.error =
                                    Some("房间名称不能为空，Room ID 必须是非零整数".to_string())
                            }
                        }
                    }
                });

            egui::CollapsingHeader::new("不可突破的硬上限")
                .default_open(false)
                .show(ui, |ui| {
                    egui::Grid::new("noticer_hard_limits_grid")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            limit_row(ui, "请求体", HARD_MAX_BODY_SIZE);
                            limit_row(ui, "单图", HARD_MAX_IMAGE_SIZE);
                            limit_row(ui, "图片数量", HARD_MAX_IMAGE_COUNT);
                            limit_row(ui, "多图总大小", HARD_MAX_TOTAL_IMAGE_SIZE);
                            limit_row(ui, "队列图片内存", HARD_MAX_QUEUED_IMAGE_BYTES);
                            limit_row(ui, "队列容量", HARD_MAX_QUEUE_CAPACITY);
                            limit_row(ui, "发送超时", HARD_MAX_SEND_TIMEOUT_SECONDS);
                            limit_row(ui, "重试次数", HARD_MAX_RETRY_ATTEMPTS);
                            limit_row(ui, "重试间隔", HARD_MAX_RETRY_DELAY_SECONDS);
                            limit_row(ui, "幂等 TTL", HARD_MAX_IDEMPOTENCY_TTL_SECONDS);
                            limit_row(ui, "幂等条目", HARD_MAX_IDEMPOTENCY_ENTRIES);
                        });
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    applied = self.save(store);
                }
                if ui.button("撤销未保存修改").clicked() {
                    self.draft = self.loaded.clone();
                    self.error = None;
                    self.saved_notice = None;
                }
            });
        });
        applied
    }
}

/// 绘制一行“标签 + 数值拖动框”。
fn number_row<T>(ui: &mut egui::Ui, label: &str, value: &mut T)
where
    T: egui::emath::Numeric,
{
    ui.label(label);
    ui.add(egui::DragValue::new(value).speed(1.0));
    ui.end_row();
}

/// 绘制一行“标签 + 只读等宽数值”（用于硬上限展示）。
fn limit_row<T: std::fmt::Display>(ui: &mut egui::Ui, label: &str, value: T) {
    ui.label(label);
    ui.monospace(value.to_string());
    ui.end_row();
}
