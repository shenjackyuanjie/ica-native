#[cfg(test)]
#[path = "tests/message_payload.rs"]
mod tests;

use crate::ica::types::message::{ImageAttachment, Mention, ReplyMessage, SendMessage};
use std::sync::Arc;

pub fn build_multi_image_message(
    room_id: i64,
    content: &str,
    reply_to: Option<&ReplyMessage>,
    mentions: &[Mention],
    images: &[(String, Arc<[u8]>)],
) -> SendMessage {
    let mut message = SendMessage::new(content.to_string(), room_id, reply_to.cloned());
    message.set_mentions(mentions);
    for (file_type, bytes) in images {
        message.add_img(bytes, file_type);
    }
    message
}

/// 仅接收本地录音器生成的规范 PCM WAV：24 kHz / 单声道 / 16 位，0.2 至 60 秒。
///
/// oicqAdapter 的 media 分支将 data:audio… 转为 record。不能只设置 file：
/// 当前 Bridge 的 file 兜底会构造 image，且普通文件上传根本不是语音协议。
/// WAV 由 oicq 在 Bridge 侧通过 FFmpeg 转为 AMR，不宣称在客户端编码了 SILK。
pub fn build_voice_message(room_id: i64, wav: &[u8]) -> Result<SendMessage, String> {
    const MIN_DATA_BYTES: usize = 24_000 * 2 / 5;
    const MAX_DATA_BYTES: usize = 24_000 * 2 * 60;
    if room_id == 0 {
        return Err("语音发送目标无效".to_string());
    }
    if wav.len() < 44 + MIN_DATA_BYTES || wav.len() > 44 + MAX_DATA_BYTES {
        return Err("语音必须为 0.2 至 60 秒的录音".to_string());
    }
    let u16_at = |offset| u16::from_le_bytes([wav[offset], wav[offset + 1]]);
    let u32_at = |offset| {
        u32::from_le_bytes([
            wav[offset],
            wav[offset + 1],
            wav[offset + 2],
            wav[offset + 3],
        ])
    };
    let valid = &wav[..4] == b"RIFF"
        && &wav[8..16] == b"WAVEfmt "
        && &wav[36..40] == b"data"
        && u32_at(4) as usize == wav.len() - 8
        && u32_at(16) == 16
        && u16_at(20) == 1
        && u16_at(22) == 1
        && u32_at(24) == 24_000
        && u32_at(28) == 48_000
        && u16_at(32) == 2
        && u16_at(34) == 16
        && u32_at(40) as usize == wav.len() - 44
        && (wav.len() - 44).is_multiple_of(2);
    if !valid {
        return Err("录音格式无效，需要 24 kHz 单声道 16 位 PCM WAV".to_string());
    }
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let mut message = SendMessage::new(String::new(), room_id, None);
    message.media.push(ImageAttachment {
        b64: Some(format!("data:audio/wav;base64,{}", STANDARD.encode(wav))),
        url: None,
        file_type: Some("audio/wav".to_string()),
        fid: None,
        order: None,
    });
    Ok(message)
}
