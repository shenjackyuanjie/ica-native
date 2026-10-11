//! 全群自动签到：按模式挑选群，按随机间隔依次发送群签到命令。
//!
//! 这是一个在 egui 主线程按时间片推进的后台任务：`tick_auto_sign` 每帧检查是否到点，
//! 到点则向当前 bridge 发送一条 `SendGroupSign`，并用 2–3 秒随机间隔错开请求，避免
//! 触发 Bridge 频率限制。进度、耗时与房间清单由 `AutoSignState` 维护，
//! `render_auto_sign_window` 负责对应的设置/进度窗口。

use super::IcaApp;
use crate::ica::IcaCommand;
use crate::ica::types::{RoomId, room::Room};
use rand::RngExt;
use std::time::{Duration, Instant};

/// “活跃群”模式默认活跃窗口：半天（12 小时）。
const HOT_DEFAULT_HOURS: u32 = 12;
/// “7 天”预设对应的活跃窗口小时数。
const WARM_PRESET_HOURS: u32 = 7 * 24;
/// 相邻两次群签到请求的最小间隔（毫秒），用于错开请求、规避频率限制。
const MIN_SIGN_DELAY_MS: u64 = 2_000;
/// 相邻两次群签到请求的最大间隔（毫秒）。
const MAX_SIGN_DELAY_MS: u64 = 3_000;

/// 自动签到的选群模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutoSignMode {
    /// 所有群（默认）。
    #[default]
    All,
    /// 仅活跃群（按 `hot_hours` 窗口内的最后活跃时间过滤）。
    Hot,
}

/// `AutoSignMode` 的展示辅助。
impl AutoSignMode {
    /// 返回模式的中文标签，用于窗口内的文字说明。
    fn label(self) -> &'static str {
        match self {
            AutoSignMode::All => "所有群",
            AutoSignMode::Hot => "活跃群",
        }
    }
}

/// 一次签到任务中的单个待签到群快照。
#[derive(Debug, Clone)]
pub struct AutoSignRoom {
    /// 群房间 ID（负数表示群）。
    pub room_id: RoomId,
    /// 群名称，用于界面展示。
    pub room_name: String,
    /// 群最后活跃时间（毫秒时间戳），活跃模式据此过滤与排序。
    pub utime: i64,
}

/// 全群自动签到的运行状态与进度。
///
/// 采用“时间片推进”模型：`tick_auto_sign` 每帧比对 `next_tick_at`，到点处理一个群并
/// 排定下一次时间，从而不占用专门线程也能匀速推进。
#[derive(Debug)]
pub struct AutoSignState {
    /// 选群模式。
    pub mode: AutoSignMode,
    /// 活跃模式的活跃窗口（小时）。
    pub hot_hours: u32,
    /// 是否正在进行签到。
    pub running: bool,
    /// 当前任务所属 bridge 的索引。
    bridge_idx: usize,
    /// 当前任务所属 bridge 的稳定标识（用于界面展示）。
    bridge_key: String,
    /// 本次任务的待签到群队列。
    rooms: Vec<AutoSignRoom>,
    /// 下一个待处理群的下标。
    next_index: usize,
    /// 已成功请求签到的群数。
    signed_count: usize,
    /// 请求发送失败的群数。
    failed_count: usize,
    /// 任务开始时刻。
    started_at: Option<Instant>,
    /// 任务结束（完成或停止）时刻。
    finished_at: Option<Instant>,
    /// 下一次处理群的时间点。
    next_tick_at: Option<Instant>,
    /// 当前正在处理的群名。
    current_room: Option<String>,
    /// 最近一条状态/结果文案。
    last_message: String,
}

/// 未开始签到时的默认状态。
impl Default for AutoSignState {
    /// 全部计数归零，模式为“所有群”，活跃窗口取默认 12 小时。
    fn default() -> Self {
        Self {
            mode: AutoSignMode::All,
            hot_hours: HOT_DEFAULT_HOURS,
            running: false,
            bridge_idx: 0,
            bridge_key: String::new(),
            rooms: Vec::new(),
            next_index: 0,
            signed_count: 0,
            failed_count: 0,
            started_at: None,
            finished_at: None,
            next_tick_at: None,
            current_room: None,
            last_message: "尚未开始".to_string(),
        }
    }
}

/// `AutoSignState` 的生命周期与进度计算。
impl AutoSignState {
    /// 用给定的 bridge 与房间队列开启一次新任务，重置计数与计时。
    ///
    /// 队列为空时 `running` 直接为 false，并给出“没有符合条件的群”的说明。
    fn begin(&mut self, bridge_idx: usize, bridge_key: String, rooms: Vec<AutoSignRoom>) {
        self.bridge_idx = bridge_idx;
        self.bridge_key = bridge_key;
        self.rooms = rooms;
        self.next_index = 0;
        self.signed_count = 0;
        self.failed_count = 0;
        self.current_room = None;
        self.started_at = Some(Instant::now());
        self.finished_at = None;
        self.next_tick_at = Some(Instant::now());
        self.running = !self.rooms.is_empty();
        self.last_message = if self.running {
            format!("已创建 {} 个群的签到任务", self.rooms.len())
        } else {
            "没有符合条件的群".to_string()
        };
    }

    /// 正常跑完所有群后收尾，记录结束时刻与汇总文案。
    fn finish(&mut self, now: Instant) {
        self.running = false;
        self.finished_at = Some(now);
        self.next_tick_at = None;
        self.current_room = None;
        self.last_message = format!(
            "完成：已请求签到 {} 个群，失败 {} 个",
            self.signed_count, self.failed_count
        );
    }

    /// 用户手动停止：结束任务并汇总“已请求签到 x / 总数”。
    fn cancel(&mut self) {
        self.running = false;
        self.finished_at = Some(Instant::now());
        self.next_tick_at = None;
        self.current_room = None;
        self.last_message = format!(
            "已停止：已请求签到 {} / {} 个群",
            self.signed_count,
            self.rooms.len()
        );
    }

    /// 清空整个任务回到未开始状态（“重置进度”按钮）。
    fn clear_run(&mut self) {
        self.running = false;
        self.rooms.clear();
        self.next_index = 0;
        self.signed_count = 0;
        self.failed_count = 0;
        self.started_at = None;
        self.finished_at = None;
        self.next_tick_at = None;
        self.current_room = None;
        self.last_message = "尚未开始".to_string();
    }

    /// 返回 0.0–1.0 的完成度；无房间时为 0.0。
    fn progress(&self) -> f32 {
        if self.rooms.is_empty() {
            0.0
        } else {
            (self.signed_count + self.failed_count) as f32 / self.rooms.len() as f32
        }
    }

    /// 已耗时；未开始返回 `None`，进行中用当前时刻，结束后用结束时刻。
    fn elapsed(&self) -> Option<Duration> {
        let started_at = self.started_at?;
        Some(self.finished_at.unwrap_or_else(Instant::now) - started_at)
    }

    /// 任务总群数。
    fn total_count(&self) -> usize {
        self.rooms.len()
    }

    /// 已处理群数（成功 + 失败）。
    fn done_count(&self) -> usize {
        self.signed_count + self.failed_count
    }

    /// 距离下一次处理的剩余时间；未排定下一次时返回 `None`。
    fn next_wait(&self) -> Option<Duration> {
        let next_tick_at = self.next_tick_at?;
        Some(next_tick_at.saturating_duration_since(Instant::now()))
    }
}

/// 自动签到的启动、逐帧推进与设置窗口渲染。
impl IcaApp {
    /// 根据当前模式挑选群并开启一次签到任务。
    ///
    /// 只考虑群（room_id < 0）；活跃模式进一步按 `hot_hours` 窗口过滤，并按最后活跃
    /// 时间倒序，让更活跃的群先签到。
    pub fn start_auto_sign(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            self.auto_sign.last_message = "未启用 bridge，无法开始签到".to_string();
            return;
        };
        let Some(state) = self.bridge_states.get(bridge_idx) else {
            self.auto_sign.last_message = "当前 bridge 状态不存在".to_string();
            return;
        };
        let bridge_key = state.bridge_key.clone();

        let mode = self.auto_sign.mode;
        let hot_hours = self.auto_sign.hot_hours.max(1);
        let cutoff_millis =
            chrono::Utc::now().timestamp_millis() - i64::from(hot_hours) * 60 * 60 * 1_000;
        let mut rooms = state
            .rooms
            .iter()
            .filter(|room| room.room_id < 0)
            .filter(|room| {
                mode == AutoSignMode::All || Self::room_active_millis(room) >= cutoff_millis
            })
            .map(|room| AutoSignRoom {
                room_id: room.room_id,
                room_name: room.room_name.clone(),
                utime: room.utime,
            })
            .collect::<Vec<_>>();

        if mode == AutoSignMode::Hot {
            rooms.sort_by_key(|room| std::cmp::Reverse(room.utime));
        }

        self.auto_sign.begin(bridge_idx, bridge_key, rooms);
    }

    /// 每帧调用：到点则处理一个群（发送签到命令并排定下一次），否则直接返回。
    ///
    /// 运行中会持续请求短延时重绘以驱动倒计时；队列用尽时自动收尾。
    pub fn tick_auto_sign(&mut self, ctx: &egui::Context) {
        if !self.auto_sign.running {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));

        let now = Instant::now();
        if let Some(next_tick_at) = self.auto_sign.next_tick_at
            && now < next_tick_at
        {
            return;
        }

        let Some(room) = self.auto_sign.rooms.get(self.auto_sign.next_index).cloned() else {
            self.auto_sign.finish(now);
            return;
        };
        let bridge_idx = self.auto_sign.bridge_idx;
        let sent = self.bridge_states.get(bridge_idx).is_some_and(|session| {
            session
                .send(IcaCommand::SendGroupSign {
                    room_id: room.room_id,
                })
                .is_ok()
        });

        self.auto_sign.current_room = Some(room.room_name.clone());
        self.auto_sign.next_index += 1;
        if sent {
            self.auto_sign.signed_count += 1;
            self.auto_sign.last_message =
                format!("已请求签到：{} ({})", room.room_name, room.room_id);
        } else {
            self.auto_sign.failed_count += 1;
            self.auto_sign.last_message =
                format!("签到命令发送失败：{} ({})", room.room_name, room.room_id);
        }

        if self.auto_sign.next_index >= self.auto_sign.rooms.len() {
            self.auto_sign.finish(now);
        } else {
            // 用 2–3 秒随机间隔排定下一次，错开请求以规避 Bridge 频率限制。
            let mut rng = rand::rng();
            let delay_ms = rng.random_range(MIN_SIGN_DELAY_MS..=MAX_SIGN_DELAY_MS);
            self.auto_sign.next_tick_at = Some(now + Duration::from_millis(delay_ms));
        }
    }

    /// 渲染“全群自动签到”设置/进度窗口（模式选择、活跃窗口、进度条与房间清单）。
    pub fn render_auto_sign_window(&mut self, ctx: &egui::Context) {
        let mut open = self.open_page.auto_sign;
        egui::Window::new("全群自动签到")
            .open(&mut open)
            .default_size(egui::vec2(420.0, 360.0))
            .min_size(egui::vec2(320.0, 260.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("按 2–3 秒随机间隔依次请求群签到。");
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!self.auto_sign.running, |ui| {
                        ui.selectable_value(
                            &mut self.auto_sign.mode,
                            AutoSignMode::All,
                            "all · 所有群",
                        );
                        ui.selectable_value(
                            &mut self.auto_sign.mode,
                            AutoSignMode::Hot,
                            "hot · 活跃群",
                        );
                    });
                });

                ui.add_enabled_ui(!self.auto_sign.running, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("hot 活跃窗口");
                        ui.add(
                            egui::DragValue::new(&mut self.auto_sign.hot_hours)
                                .range(1..=24 * 30)
                                .suffix(" 小时"),
                        );
                        if ui.small_button("默认 12h").clicked() {
                            self.auto_sign.hot_hours = HOT_DEFAULT_HOURS;
                        }
                        if ui.small_button("7 天").clicked() {
                            self.auto_sign.hot_hours = WARM_PRESET_HOURS;
                        }
                    });
                });

                ui.weak(format!(
                    "当前模式：{}；hot 默认半天内活跃，和 bot-sign hot 一致。",
                    self.auto_sign.mode.label()
                ));
                ui.separator();

                let progress = self.auto_sign.progress();
                let total = self.auto_sign.total_count();
                let done = self.auto_sign.done_count();
                ui.add(
                    egui::ProgressBar::new(progress)
                        .show_percentage()
                        .text(format!("{done} / {total}")),
                );

                if let Some(elapsed) = self.auto_sign.elapsed() {
                    ui.label(format!("耗时：{:.1}s", elapsed.as_secs_f32()));
                }
                if let Some(wait) = self.auto_sign.next_wait()
                    && self.auto_sign.running
                    && !wait.is_zero()
                {
                    ui.label(format!("下一次签到倒计时：{:.1}s", wait.as_secs_f32()));
                }
                if let Some(current_room) = &self.auto_sign.current_room {
                    ui.label(format!("当前群：{current_room}"));
                }
                ui.label(&self.auto_sign.last_message);

                ui.horizontal(|ui| {
                    if self.auto_sign.running {
                        if ui.button("停止").clicked() {
                            self.auto_sign.cancel();
                        }
                    } else if ui.button("开始签到").clicked() {
                        self.start_auto_sign();
                    }
                    if ui.button("重置进度").clicked() {
                        self.auto_sign.clear_run();
                    }
                });

                if !self.auto_sign.rooms.is_empty() {
                    ui.separator();
                    ui.weak(format!("当前 bridge：{}", self.auto_sign.bridge_key));
                    egui::ScrollArea::vertical()
                        .id_salt("auto_sign_rooms")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (idx, room) in self.auto_sign.rooms.iter().enumerate() {
                                let marker = if idx < self.auto_sign.done_count() {
                                    "✓"
                                } else if idx == self.auto_sign.next_index {
                                    "→"
                                } else {
                                    " "
                                };
                                ui.label(format!("{marker} {} ({})", room.room_name, room.room_id));
                            }
                        });
                }
            });
        self.open_page.auto_sign = open;
    }

    /// 归一化群的活跃时间到毫秒：兼容秒级与毫秒级时间戳（小值按秒放大一千倍）。
    fn room_active_millis(room: &Room) -> i64 {
        if room.utime.abs() < 10_000_000_000 {
            room.utime * 1_000
        } else {
            room.utime
        }
    }
}
