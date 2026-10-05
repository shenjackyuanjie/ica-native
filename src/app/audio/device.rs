//! 音频设备只由一个惰性工作线程拥有，GUI 不等待麦克风、下载或解码。

use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::Source;

use super::{
    AudioState, DraftStage, MAX_RECORD_SECONDS, PlaybackItem, PlaybackKey, PlaybackStage,
    PlaybackView, lock, wav,
};

const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 16 * 1024 * 1024;
const MAX_PLAY_SECONDS: usize = 600;

pub enum AudioSource {
    Memory(Arc<[u8]>),
    Remote(String),
}

pub enum WorkerCommand {
    StartRecording(u64),
    StopRecording(u64),
    CancelRecording(u64),
    Play {
        key: PlaybackKey,
        source: AudioSource,
    },
    Seek {
        key: PlaybackKey,
        position: Duration,
    },
    Downloaded {
        generation: u64,
        key: PlaybackKey,
        result: Result<Arc<[u8]>, String>,
    },
    OutputFailed {
        generation: u64,
        message: String,
    },
    Shutdown,
}

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

#[derive(Default)]
struct CaptureData {
    samples: Vec<f32>,
    error: Option<String>,
}

struct Capture {
    id: u64,
    data: Arc<Mutex<CaptureData>>,
    stream: cpal::Stream,
    sample_rate: u32,
    started: Instant,
}

struct Player {
    key: PlaybackKey,
    // Sink 必须比 OutputStream 先释放。
    sink: rodio::Player,
    _stream: rodio::MixerDeviceSink,
    duration: Duration,
}

struct AudioWorker {
    state: Arc<Mutex<AudioState>>,
    ctx: egui::Context,
    tx: mpsc::Sender<WorkerCommand>,
    capture: Option<Capture>,
    player: Option<Player>,
    generation: Arc<AtomicU64>,
}

impl AudioWorker {
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

    fn stop_playback(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.player = None;
        lock(&self.state).playback = None;
    }

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

    fn fail_playback(&self, key: &PlaybackKey, error: String) {
        if let Some(view) = lock(&self.state).playback.as_mut()
            && &view.key == key
        {
            view.stage = PlaybackStage::Failed;
            view.error = Some(error);
        }
    }

    fn fail_recording(&self, id: u64, error: String) {
        let mut state = lock(&self.state);
        if state.draft.as_ref().is_some_and(|draft| draft.id == id)
            && let Some(draft) = state.draft.take()
        {
            state.notice = Some((draft.owner, error));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_frames_are_downmixed_clamped_and_bounded() {
        let mut output = Vec::new();
        append_mono(
            &[1.0_f32, -1.0, f32::NAN, 0.5, 9.0, 9.0, 0.5, 0.5],
            2,
            3,
            &mut output,
        );
        assert_eq!(output, [0.0, 0.25, 1.0]);
        append_mono(&[0.5_f32; 4], 2, 3, &mut output);
        assert_eq!(output.len(), 3, "到达时长上限后不能继续积累麦克风样本");
    }

    #[test]
    fn preview_can_be_decoded_and_has_duration_without_opening_audio_device() {
        let wav = wav::encode(&vec![0.1; 48_000], 48_000).unwrap();
        let decoded = decode(Arc::from(wav)).unwrap();
        assert_eq!(decoded.total_duration(), Some(Duration::from_secs(1)));
        assert_eq!(decoded.channels().get(), 1);
        assert_eq!(decoded.sample_rate().get(), 24_000);
        assert!(decode(Arc::from([0_u8; 32])).is_err());
    }

    #[tokio::test]
    async fn download_rejects_size_and_http_errors_without_leaking_url() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                assert!(socket.read(&mut buffer).await.unwrap() > 0);
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let error = download(
                &format!("http://{address}/record?token=do-not-expose"),
                &AtomicU64::new(0),
                0,
            )
            .await
            .unwrap_err();
            assert!(!error.contains("do-not-expose"));
            assert!(error.contains("上限") || error.contains("403"));
            server.await.unwrap();
        }
    }
}
