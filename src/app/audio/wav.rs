//! Electron 录音格式：单声道、24 kHz、16 位小端 PCM WAV。
//!
//! 该模块位于 `crate::app::audio` 之下，只负责把工作线程采集到的单声道 `f32`
//! 采样线性重采样到 24 kHz，并打包成与 Electron `WavEncoder` 一致的 16 位 PCM WAV，
//! 从而让原生客户端录出的语音能被同一套 bridge / QQ 链路接收。

#[cfg(test)]
#[path = "tests/wav.rs"]
mod tests;

/// 输出 WAV 的目标采样率（24 kHz），与 Electron 保持一致。
pub const SAMPLE_RATE: u32 = 24_000;
/// 允许的最短录音时长（毫秒）；不足则拒绝，避免误触产生过短/空语音。
pub const MIN_RECORD_MILLIS: u64 = 200;

/// 把单声道 `f32` 采样线性重采样到 24 kHz 并编码为 16 位 PCM WAV。
///
/// - 参数 `samples`：以 `input_rate` 采样的单声道浮点采样；
/// - 参数 `input_rate`：输入采样率，需在 `8_000..=192_000` 之间；
/// - 返回：WAV 字节；输入采样率不受支持、时长不足 0.2 秒或超过 60 秒时返回错误。
///
/// 重采样算法与 Electron `WavEncoder` 保持一致，保证两端产物体积/听感一致。
pub fn encode(samples: &[f32], input_rate: u32) -> Result<Vec<u8>, String> {
    if !(8_000..=192_000).contains(&input_rate) {
        return Err("不支持的麦克风采样率".to_string());
    }
    if samples.len() < input_rate as usize * MIN_RECORD_MILLIS as usize / 1000 {
        return Err("录音不足 0.2 秒，请重新录制".to_string());
    }
    if samples.len() > input_rate as usize * super::MAX_RECORD_SECONDS as usize {
        return Err("录音超过 60 秒".to_string());
    }
    let output_len = (samples.len() as u64 * u64::from(SAMPLE_RATE) + u64::from(input_rate) / 2)
        / u64::from(input_rate);
    let data_len = output_len as u32 * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    // 与 Electron WavEncoder 的线性重采样保持一致。
    for i in 0..output_len {
        let position = i as f64 * f64::from(input_rate) / f64::from(SAMPLE_RATE);
        let lower = (position as usize).min(samples.len() - 1);
        let upper = (lower + 1).min(samples.len() - 1);
        let weight = (position - lower as f64) as f32;
        let value = samples[lower] + (samples[upper] - samples[lower]) * weight;
        let value = if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let sample = (value * if value < 0.0 { 32768.0 } else { 32767.0 }) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}
