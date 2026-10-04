//! Electron 录音格式：单声道、24 kHz、16 位小端 PCM WAV。

pub const SAMPLE_RATE: u32 = 24_000;
pub const MIN_RECORD_MILLIS: u64 = 200;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_device_rate_is_resampled_to_bridge_pcm_contract() {
        let mut input = vec![0.0; 48_000];
        input[0] = -2.0;
        input[2] = 2.0;
        input[4] = f32::NAN;
        let bytes = encode(&input, 48_000).unwrap();
        assert_eq!(bytes.len(), 44 + 24_000 * 2);
        assert_eq!(&bytes[22..24], &1_u16.to_le_bytes());
        assert_eq!(&bytes[24..28], &24_000_u32.to_le_bytes());
        assert_eq!(&bytes[44..50], &[0, 128, 255, 127, 0, 0]);
    }

    #[test]
    fn empty_too_short_too_long_and_invalid_rate_are_rejected_before_encoding() {
        assert!(encode(&[], 24_000).is_err());
        assert!(encode(&vec![0.0; 4799], 24_000).is_err());
        assert!(encode(&vec![0.0; 4800], 24_000).is_ok());
        assert!(encode(&vec![0.0; 24_000 * 60 + 1], 24_000).is_err());
        assert!(encode(&[0.0; 4800], 0).is_err());
    }
}
