//! 音频设备只由一个惰性工作线程拥有，GUI 不等待麦克风、下载或解码。
//!
//! 该模块位于 `crate::app::audio` 之下，是音频子系统真正触达硬件/网络的底层：
//! - 用 `cpal` 打开默认麦克风采集采样，用 `rodio` 打开默认输出设备播放；
//! - 语音 URL 的下载在独立的 tokio 当前线程任务里进行，解码与播放都在本工作线程；
//! - 所有与 GUI 的交互都通过 `WorkerCommand` 通道收、通过共享 `AudioState` 与
//!   `ctx.request_repaint()` 反馈，GUI 线程从不阻塞等待设备或网络。
//!
//! 设计要点：
//! - 线程惰性创建、收到 `Shutdown` 或通道断开即退出，设备随线程析构而释放；
//! - 下载/输出失败通过 `generation` 编号回传，保证只处理“仍然是当前这一代”的结果；
//! - 采样/解码/下载都设有上限，防止坏数据或超长音频把内存撑爆。

#[cfg(test)]
#[path = "tests/device.rs"]
mod tests;

use super::{
    AudioState, DraftStage, MAX_RECORD_SECONDS, PlaybackItem, PlaybackKey, PlaybackStage,
    PlaybackView, lock, wav,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::Source;
use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// 语音下载的字节上限（16 MiB），超出即拒绝，避免异常响应撑爆内存。
const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
/// 解码后采样数上限（与下载上限同数量级），用于约束播放前的峰值内存。
const MAX_DECODED_SAMPLES: usize = 16 * 1024 * 1024;
/// 允许播放的最长秒数（10 分钟），超出即拒绝解码。
const MAX_PLAY_SECONDS: usize = 600;

/// 播放的音频来源：内存中的音频字节，或需要下载的远端 URL。
pub enum AudioSource {
    /// 已在内存中的音频字节（如刚录制的预览）。
    Memory(Arc<[u8]>),
    /// 需要先下载的远端地址。
    Remote(String),
}

/// GUI 线程发给音频工作线程的命令，以及工作线程内部回传的完成/失败通知。
pub enum WorkerCommand {
    /// 开始录音，参数为草稿 ID。
    StartRecording(u64),
    /// 停止录音并生成预览，参数为草稿 ID。
    StopRecording(u64),
    /// 取消录音，参数为草稿 ID。
    CancelRecording(u64),
    /// 播放指定来源的音频。
    Play {
        /// 播放键，用于把进度匹配回 UI。
        key: PlaybackKey,
        /// 音频来源。
        source: AudioSource,
    },
    /// 跳转到指定播放位置。
    Seek {
        /// 目标播放键。
        key: PlaybackKey,
        /// 目标位置。
        position: Duration,
    },
    /// 下载任务完成回传（由下载线程发回本工作线程）。
    Downloaded {
        /// 发起下载时的 generation，用于丢弃过期结果。
        generation: u64,
        /// 对应播放键。
        key: PlaybackKey,
        /// 下载结果：音频字节或错误。
        result: Result<Arc<[u8]>, String>,
    },
    /// 输出设备报错回传（如设备被拔出）。
    OutputFailed {
        /// 对应 generation。
        generation: u64,
        /// 错误描述。
        message: String,
    },
    /// 请求工作线程退出。
    Shutdown,
}

/// 启动音频工作线程并返回其命令发送端。
///
/// - 参数 `state`：与 GUI 共享的音频状态；
/// - 参数 `ctx`：egui 上下文，用于在处理完命令后请求重绘；
/// - 返回：命令发送端；线程创建失败返回错误字符串。
///
/// 线程以 50ms 超时轮询命令，超时则执行一次 `tick` 推进录音/播放状态。
pub fn spawn(
    state: Arc<Mutex<AudioState>>,
    ctx: egui::Context,
) -> Result<mpsc::Sender<WorkerCommand>, String> {
    let (tx, rx) = mpsc::channel();
    let sender = tx.clone();
    std::thread::Builder::new()
        .name("ica-audio".to_string())
        .spawn(move || {
            let mut worker = AudioWorker {
                state,
                ctx,
                tx,
                capture: None,
                player: None,
                generation: Arc::new(AtomicU64::new(0)),
            };
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(WorkerCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Ok(command) => worker.handle(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                worker.tick();
            }
            worker.generation.fetch_add(1, Ordering::Relaxed);
            // capture / player 在设备所属线程析构，退出时不会遗留麦克风流。
        })
        .map_err(|_| "无法启动音频线程".to_string())?;
    Ok(sender)
}

/// 采集回调写入的共享数据：单声道采样与最近一次设备错误。
#[derive(Default)]
struct CaptureData {
    /// 归一化到 `[-1, 1]` 的单声道采样。
    samples: Vec<f32>,
    /// 采集过程中的设备错误（若有）。
    error: Option<String>,
}

/// 一次进行中的麦克风采集。
struct Capture {
    /// 对应草稿 ID。
    id: u64,
    /// 与采集回调共享的采样缓冲。
    data: Arc<Mutex<CaptureData>>,
    /// cpal 输入流；drop 时停止采集。
    stream: cpal::Stream,
    /// 麦克风采样率。
    sample_rate: u32,
    /// 开始采集的时刻，用于超时与最长时长判断。
    started: Instant,
}

/// 一次进行中的播放。
struct Player {
    /// 对应播放键。
    key: PlaybackKey,
    /// rodio 播放 sink。
    // Sink 必须比 OutputStream 先释放。
    sink: rodio::Player,
    /// 输出流；字段以下划线保留，确保它在 sink 之后才析构。
    _stream: rodio::MixerDeviceSink,
    /// 音频总时长。
    duration: Duration,
}

/// 音频工作线程主体：串行处理命令并 tick 推进状态。
struct AudioWorker {
    /// 与 GUI 共享的音频状态。
    state: Arc<Mutex<AudioState>>,
    /// egui 上下文，用于请求重绘。
    ctx: egui::Context,
    /// 命令通道发送端，用于回传下载/输出结果。
    tx: mpsc::Sender<WorkerCommand>,
    /// 当前采集（若有）。
    capture: Option<Capture>,
    /// 当前播放（若有）。
    player: Option<Player>,
    /// 代际编号，自增以使旧的下载/输出回调结果失效。
    generation: Arc<AtomicU64>,
}

/// `AudioWorker` 的固有实现：命令处理、采集/播放控制与每 tick 状态推进。
impl AudioWorker {
    /// 处理一条命令，并在结束时请求一次重绘。
    fn handle(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::StartRecording(id) => {
                let current = lock(&self.state)
                    .draft
                    .as_ref()
                    .is_some_and(|draft| draft.id == id && draft.stage == DraftStage::Starting);
                if !current {
                    return;
                }
                self.stop_playback();
                self.capture = None;
                match start_capture(id) {
                    Ok(capture) => {
                        let mut state = lock(&self.state);
                        if let Some(draft) = state.draft.as_mut()
                            && draft.id == id
                        {
                            if draft.stage == DraftStage::Starting {
                                draft.stage = DraftStage::Recording;
                            }
                            self.capture = Some(capture);
                        }
                    }
                    Err(error) => self.fail_recording(id, error),
                }
            }
            WorkerCommand::StopRecording(id) => {
                if self
                    .capture
                    .as_ref()
                    .is_some_and(|capture| capture.id == id)
                {
                    self.finish_capture();
                }
            }
            WorkerCommand::CancelRecording(id) => {
                if self
                    .capture
                    .as_ref()
                    .is_some_and(|capture| capture.id == id)
                {
                    self.capture = None;
                }
                let preview = lock(&self.state)
                    .playback
                    .as_ref()
                    .is_some_and(|view| view.key.item == PlaybackItem::Preview(id));
                if preview {
                    self.stop_playback();
                }
            }
            WorkerCommand::Play { key, source } => self.play(key, source),
            WorkerCommand::Seek { key, position } => {
                if let Some(player) = &self.player
                    && player.key == key
                    && let Err(_) = player.sink.try_seek(position.min(player.duration))
                {
                    self.fail_playback(&key, "此语音不支持跳转，请重新播放".to_string());
                    self.player = None;
                }
            }
            WorkerCommand::Downloaded {
                generation,
                key,
                result,
            } => {
                if generation == self.generation.load(Ordering::Relaxed) {
                    match result {
                        Ok(bytes) => self.start_player(key, bytes, generation),
                        Err(error) => self.fail_playback(&key, error),
                    }
                }
            }
            WorkerCommand::OutputFailed {
                generation,
                message,
            } => {
                if generation == self.generation.load(Ordering::Relaxed)
                    && let Some(player) = self.player.take()
                {
                    self.fail_playback(&player.key, message);
                }
            }
            WorkerCommand::Shutdown => {}
        }
        self.ctx.request_repaint();
    }

    /// 停止当前播放：自增 generation 使旧的下载/输出回调失效，并清空播放态。
    fn stop_playback(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.player = None;
        lock(&self.state).playback = None;
    }

    /// 播放/暂停/重试指定音频。
    ///
    /// 同一键再次点击在播放与暂停间切换；新来源则进入 `Loading`，
    /// 内存来源直接解码播放，远端来源另起下载线程。
    fn play(&mut self, key: PlaybackKey, source: AudioSource) {
        if self.capture.is_some()
            || lock(&self.state).draft.as_ref().is_some_and(|draft| {
                matches!(
                    draft.stage,
                    DraftStage::Starting | DraftStage::Recording | DraftStage::Stopping
                )
            })
        {
            return;
        }
        if let Some(player) = &self.player
            && player.key == key
            && !player.sink.empty()
        {
            if player.sink.is_paused() {
                player.sink.play();
            } else {
                player.sink.pause();
            }
            return;
        }
        let loading_same = lock(&self.state)
            .playback
            .as_ref()
            .is_some_and(|view| view.key == key && view.stage == PlaybackStage::Loading);
        self.stop_playback();
        if loading_same {
            return;
        }
        let generation = self.generation.load(Ordering::Relaxed);
        lock(&self.state).playback = Some(PlaybackView {
            key: key.clone(),
            stage: PlaybackStage::Loading,
            position: Duration::ZERO,
            duration: None,
            error: None,
        });
        match source {
            AudioSource::Memory(bytes) => self.start_player(key, bytes, generation),
            AudioSource::Remote(url) => {
                let sender = self.tx.clone();
                let generation_counter = self.generation.clone();
                let download_key = key.clone();
                let task = std::thread::Builder::new()
                    .name("ica-audio-download".to_string())
                    .spawn(move || {
                        let result = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|_| "无法启动语音下载任务".to_string())
                            .and_then(|runtime| {
                                runtime.block_on(download(&url, &generation_counter, generation))
                            });
                        if generation_counter.load(Ordering::Relaxed) == generation {
                            let _ = sender.send(WorkerCommand::Downloaded {
                                generation,
                                key: download_key,
                                result,
                            });
                        }
                    });
                if task.is_err() {
                    self.fail_playback(&key, "无法启动语音下载线程".to_string());
                }
            }
        }
    }

    /// 解码音频字节并打开输出设备开始播放；失败则把播放标记为 `Failed`。
    fn start_player(&mut self, key: PlaybackKey, bytes: Arc<[u8]>, generation: u64) {
        let result = (|| {
            let source = decode(bytes)?;
            let duration = source.total_duration().unwrap_or_default();
            let sender = self.tx.clone();
            let mut stream = rodio::DeviceSinkBuilder::from_default_device()
                .map_err(|_| "没有可用的音频输出设备".to_string())?
                .with_error_callback(move |_error| {
                    let _ = sender.send(WorkerCommand::OutputFailed {
                        generation,
                        message: "音频输出设备中断，请重新选择系统默认设备后重试".to_string(),
                    });
                })
                .open_stream()
                .map_err(|_| "无法打开音频输出设备".to_string())?;
            stream.log_on_drop(false);
            let sink = rodio::Player::connect_new(stream.mixer());
            sink.append(source);
            Ok::<_, String>(Player {
                key: key.clone(),
                sink,
                _stream: stream,
                duration,
            })
        })();
        match result {
            Ok(player) => {
                if let Some(view) = lock(&self.state).playback.as_mut()
                    && view.key == key
                {
                    view.stage = PlaybackStage::Playing;
                    view.duration = Some(player.duration);
                }
                self.player = Some(player);
            }
            Err(error) => self.fail_playback(&key, error),
        }
        self.ctx.request_repaint();
    }

    /// 把匹配的播放视图标记为失败并写入错误信息。
    fn fail_playback(&self, key: &PlaybackKey, error: String) {
        if let Some(view) = lock(&self.state).playback.as_mut()
            && &view.key == key
        {
            view.stage = PlaybackStage::Failed;
            view.error = Some(error);
        }
    }

    /// 录音失败：丢弃对应草稿并把错误作为一次性通知展示。
    fn fail_recording(&self, id: u64, error: String) {
        let mut state = lock(&self.state);
        if state.draft.as_ref().is_some_and(|draft| draft.id == id)
            && let Some(draft) = state.draft.take()
        {
            state.notice = Some((draft.owner, error));
        }
    }

    /// 结束采集：丢弃输入流，把采样编码为 WAV 写入草稿并进入 `Ready`。
    fn finish_capture(&mut self) {
        let Some(capture) = self.capture.take() else {
            return;
        };
        drop(capture.stream);
        let mut data = lock(&capture.data);
        let result = match data.error.take() {
            Some(error) => Err(error),
            None => wav::encode(&data.samples, capture.sample_rate),
        };
        drop(data);
        match result {
            Ok(bytes) => {
                let mut state = lock(&self.state);
                if let Some(draft) = state.draft.as_mut()
                    && draft.id == capture.id
                {
                    draft.duration = Duration::from_secs_f64((bytes.len() - 44) as f64 / 48_000.0);
                    draft.wav = Some(Arc::from(bytes));
                    draft.stage = DraftStage::Ready;
                }
            }
            Err(error) => self.fail_recording(capture.id, error),
        }
        self.ctx.request_repaint();
    }

    /// 每个轮询周期推进一次状态：更新录音时长/超时收尾，刷新播放进度。
    fn tick(&mut self) {
        if let Some(capture) = &self.capture {
            let (length, error) = {
                let data = lock(&capture.data);
                (data.samples.len(), data.error.clone())
            };
            let duration = Duration::from_secs_f64(length as f64 / f64::from(capture.sample_rate));
            if let Some(draft) = lock(&self.state).draft.as_mut()
                && draft.id == capture.id
            {
                draft.duration = duration;
            }
            if let Some(error) = error {
                let id = capture.id;
                self.capture = None;
                self.fail_recording(id, error);
            } else if length == 0 && capture.started.elapsed() >= Duration::from_secs(5) {
                // 打开成功但 5 秒内一个采样都没有，多半是权限/设备被占用，直接报错收尾。
                let id = capture.id;
                self.capture = None;
                self.fail_recording(
                    id,
                    "麦克风未返回采样数据，请检查系统权限和输入设备".to_string(),
                );
            } else if duration >= Duration::from_secs(MAX_RECORD_SECONDS)
                || capture.started.elapsed() >= Duration::from_secs(MAX_RECORD_SECONDS + 1)
            {
                self.finish_capture();
            }
            self.ctx.request_repaint();
        }
        if let Some(player) = &self.player {
            let mut state = lock(&self.state);
            if let Some(view) = state.playback.as_mut()
                && view.key == player.key
            {
                if player.sink.empty() {
                    view.position = player.duration;
                    view.stage = PlaybackStage::Finished;
                } else {
                    view.position = player.sink.get_pos().min(player.duration);
                    view.stage = if player.sink.is_paused() {
                        PlaybackStage::Paused
                    } else {
                        PlaybackStage::Playing
                    };
                    if view.stage == PlaybackStage::Playing {
                        self.ctx.request_repaint();
                    }
                }
            }
        }
    }
}

/// 打开默认麦克风并建立输入流，返回一个 `Capture`。
///
/// 依据设备的采样格式选择对应的泛型输入流；采样率/声道数不在支持范围时直接报错。
fn start_capture(id: u64) -> Result<Capture, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| "没有可用的麦克风，请检查系统输入设备".to_string())?;
    let config = device
        .default_input_config()
        .map_err(|_| "无法读取麦克风配置，请检查系统录音权限".to_string())?;
    let sample_rate = config.sample_rate();
    if !(8_000..=192_000).contains(&sample_rate) || !(1..=32).contains(&config.channels()) {
        return Err("麦克风默认采样率或声道数不受支持".to_string());
    }
    let sample_format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let data = Arc::new(Mutex::new(CaptureData::default()));
    let stream = match sample_format {
        cpal::SampleFormat::F32 => input_stream::<f32>(&device, config, data.clone()),
        cpal::SampleFormat::F64 => input_stream::<f64>(&device, config, data.clone()),
        cpal::SampleFormat::I8 => input_stream::<i8>(&device, config, data.clone()),
        cpal::SampleFormat::I16 => input_stream::<i16>(&device, config, data.clone()),
        cpal::SampleFormat::I32 => input_stream::<i32>(&device, config, data.clone()),
        cpal::SampleFormat::I64 => input_stream::<i64>(&device, config, data.clone()),
        cpal::SampleFormat::U8 => input_stream::<u8>(&device, config, data.clone()),
        cpal::SampleFormat::U16 => input_stream::<u16>(&device, config, data.clone()),
        cpal::SampleFormat::U32 => input_stream::<u32>(&device, config, data.clone()),
        cpal::SampleFormat::U64 => input_stream::<u64>(&device, config, data.clone()),
        _ => return Err("麦克风默认采样格式不受支持".to_string()),
    }
    .map_err(|_| "无法打开麦克风，请检查系统录音权限或设备占用".to_string())?;
    stream
        .play()
        .map_err(|_| "无法启动麦克风录音".to_string())?;
    Ok(Capture {
        id,
        data,
        stream,
        sample_rate,
        started: Instant::now(),
    })
}

/// 为某种采样类型 `T` 建立 cpal 输入流：回调里把多声道混成单声道追加进共享缓冲。
fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    data: Arc<Mutex<CaptureData>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let max_samples = config.sample_rate as usize * MAX_RECORD_SECONDS as usize;
    let on_error = data.clone();
    device.build_input_stream(
        &config,
        move |input: &[T], _| {
            let mut captured = lock(&data);
            append_mono(input, channels, max_samples, &mut captured.samples);
        },
        move |_error| {
            lock(&on_error).error = Some("麦克风设备中断，请检查连接后重新录制".to_string());
        },
        None,
    )
}

/// 把一帧多声道采样平均成单声道并追加到输出，同时做有限值/幅值裁剪与长度上限保护。
fn append_mono<T>(input: &[T], channels: usize, max_samples: usize, output: &mut Vec<f32>)
where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    if channels == 0 {
        return;
    }
    let remaining = max_samples.saturating_sub(output.len());
    output.extend(input.chunks_exact(channels).take(remaining).map(|frame| {
        frame
            .iter()
            .map(|sample| {
                let value: f32 = sample.to_sample();
                if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .sum::<f32>()
            / channels as f32
    }));
}

/// 解码后有明确长度，避免 OGG 没有 duration 元数据时进度条永久停在零。
fn decode(bytes: Arc<[u8]>) -> Result<rodio::buffer::SamplesBuffer, String> {
    let decoder = rodio::Decoder::try_from(Cursor::new(bytes)).map_err(|_| {
        "无法解码语音；支持 WAV、OGG/Vorbis、MP3、FLAC，请等待 Bridge 完成转换".to_string()
    })?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    if channels.get() > 8 || rate.get() > 192_000 {
        return Err("语音的声道数或采样率不受支持".to_string());
    }
    let limit = (usize::from(channels.get()) * rate.get() as usize * MAX_PLAY_SECONDS)
        .min(MAX_DECODED_SAMPLES);
    let samples: Vec<f32> = decoder.take(limit + 1).collect();
    if samples.is_empty() {
        return Err("语音没有可播放的采样数据".to_string());
    }
    if samples.len() > limit {
        return Err("语音超过播放时长或解码内存上限".to_string());
    }
    Ok(rodio::buffer::SamplesBuffer::new(channels, rate, samples))
}

/// 下载远端语音字节，并在 `generation` 变化（播放被切换/停止）时尽快取消。
async fn download(url: &str, generation: &AtomicU64, expected: u64) -> Result<Arc<[u8]>, String> {
    // 不使用账号 token，不访问本地路径；错误中不得暴露远程 URL 的认证查询参数。
    let parsed = reqwest::Url::parse(url).map_err(|_| "语音地址无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("语音仅支持 HTTP(S) 下载".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|_| "无法创建语音下载客户端".to_string())?;
    let request = async {
        let mut response = client
            .get(parsed)
            .send()
            .await
            .map_err(|_| "语音下载失败，请检查网络后重试".to_string())?;
        if !response.status().is_success() {
            return Err(format!("语音下载失败：HTTP {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
        {
            return Err("语音超过 16 MiB 下载上限".to_string());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "语音下载中断，请重试".to_string())?
        {
            if chunk.len() > MAX_DOWNLOAD_BYTES - bytes.len() {
                return Err("语音超过 16 MiB 下载上限".to_string());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err("语音下载结果为空".to_string());
        }
        Ok(Arc::from(bytes))
    };
    tokio::select! {
        result = request => result,
        _ = async {
            while generation.load(Ordering::Relaxed) == expected { tokio::time::sleep(Duration::from_millis(100)).await; }
        } => Err("已取消语音下载".to_string()),
    }
}
