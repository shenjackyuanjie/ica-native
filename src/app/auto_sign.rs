use std::time::{Duration, Instant};

use rand::RngExt;

use crate::ica::IcaCommand;
use crate::ica::types::{RoomId, room::Room};

use super::IcaApp;

const HOT_DEFAULT_HOURS: u32 = 12;
const WARM_PRESET_HOURS: u32 = 7 * 24;
const MIN_SIGN_DELAY_MS: u64 = 2_000;
const MAX_SIGN_DELAY_MS: u64 = 3_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutoSignMode {
    #[default]
    All,
    Hot,
}

impl AutoSignMode {
    fn label(self) -> &'static str {
        match self {
            AutoSignMode::All => "所有群",
            AutoSignMode::Hot => "活跃群",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutoSignRoom {
    pub room_id: RoomId,
    pub room_name: String,
    pub utime: i64,
}

#[derive(Debug)]
pub struct AutoSignState {
    pub mode: AutoSignMode,
    pub hot_hours: u32,
    pub running: bool,
    bridge_idx: usize,
    bridge_key: String,
    rooms: Vec<AutoSignRoom>,
    next_index: usize,
    signed_count: usize,
    failed_count: usize,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    next_tick_at: Option<Instant>,
    current_room: Option<String>,
    last_message: String,
}

impl Default for AutoSignState {
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

impl AutoSignState {
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

    fn progress(&self) -> f32 {
        if self.rooms.is_empty() {
            0.0
        } else {
            (self.signed_count + self.failed_count) as f32 / self.rooms.len() as f32
        }
    }

    fn elapsed(&self) -> Option<Duration> {
        let started_at = self.started_at?;
        Some(self.finished_at.unwrap_or_else(Instant::now) - started_at)
    }

    fn total_count(&self) -> usize {
        self.rooms.len()
    }

    fn done_count(&self) -> usize {
        self.signed_count + self.failed_count
    }

    fn next_wait(&self) -> Option<Duration> {
        let next_tick_at = self.next_tick_at?;
        Some(next_tick_at.saturating_duration_since(Instant::now()))
    }
}

impl IcaApp {
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
            let mut rng = rand::rng();
            let delay_ms = rng.random_range(MIN_SIGN_DELAY_MS..=MAX_SIGN_DELAY_MS);
            self.auto_sign.next_tick_at = Some(now + Duration::from_millis(delay_ms));
        }
    }

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

    fn room_active_millis(room: &Room) -> i64 {
        if room.utime.abs() < 10_000_000_000 {
            room.utime * 1_000
        } else {
            room.utime
        }
    }
}
