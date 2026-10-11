//! 消息载荷构建：把 GUI 侧的输入组装成 `SendMessage`。
//!
//! 这里只负责"拼消息"，不做发送；多图与语音两类载荷的构造规则集中在同一文件，
//! 因为它们都涉及 Bridge 对 media 字段的特殊约定：
//! - 多图：按顺序 `add_img`，每张图一个 media 项；
//! - 语音：伪装成一张 `audio/wav` 的 media，靠 Bridge 侧转码成真正的语音记录。
//!
//! 构造通常在 `spawn_blocking` 中完成（Base64 编码耗时），因此这些函数都是纯函数，
//! 不访问 socket 或全局状态。

#[cfg(test)]
#[path = "tests/message_payload.rs"]
mod tests;

use crate::ica::types::message::{ImageAttachment, Mention, ReplyMessage, SendMessage};
use std::sync::Arc;

/// 构造一条带多张图片的消息。
///
/// 图片按传入顺序追加（`add_img`），`reply_to` 与 `mentions` 一并写入；
/// 返回的消息尚未发送，调用方决定走 socket 还是 HTTP 通道。
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
///
/// 长度上下界按 24 kHz / 16 位 / 单声道折算：0.2 秒约 9600 字节，60 秒约 2880000 字节，
/// 都不含 44 字节的 WAV 头。校验失败时返回中文错误文本，由调用方转成命令失败事件。
pub fn build_voice_message(room_id: i64, wav: &[u8]) -> Result<SendMessage, String> {
    const MIN_DATA_BYTES: usize = 24_000 * 2 / 5;
    const MAX_DATA_BYTES: usize = 24_000 * 2 * 60;
    if room_id == 0 {
        return Err("语音发送目标无效".to_string());
    }
    if wav.len() < 44 + MIN_DATA_BYTES || wav.len() > 44 + MAX_DATA_BYTES {
        return Err("语音必须为 0.2 至 60 秒的录音".to_string());
    }
    // 以下按 WAV 文件头的固定偏移逐字段核对，偏移写死是因为只接受这一种规范文件，
    // 遇到带 LIST 等额外块的录音器输出宁可拒收，也不猜结构。
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
        // 文件头声明的长度必须与实际长度吻合，防止截断的文件被当成完整录音。
        && u32_at(4) as usize == wav.len() - 8
        // fmt 块长度必须是 16（PCM 无扩展块）。
        && u32_at(16) == 16
        // fmt 块：PCM(1)、单声道、24 kHz 采样、48000 字节/秒、2 字节/帧、16 位。
        && u16_at(20) == 1
        && u16_at(22) == 1
        && u32_at(24) == 24_000
        && u32_at(28) == 48_000
        && u16_at(32) == 2
        && u16_at(34) == 16
        // data 块长度同样要与实际吻合，且采样数必须是整数个 16 位样本。
        && u32_at(40) as usize == wav.len() - 44
        && (wav.len() - 44).is_multiple_of(2);
    if !valid {
        return Err("录音格式无效，需要 24 kHz 单声道 16 位 PCM WAV".to_string());
    }
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let mut message = SendMessage::new(String::new(), room_id, None);
    // 语音消息没有文字内容，只有一个 audio/wav 的 media 项；
    // 用 data URL 而不是 file 字段，才能让 Bridge 走到 media 转码分支。
    message.media.push(ImageAttachment {
        b64: Some(format!("data:audio/wav;base64,{}", STANDARD.encode(wav))),
        url: None,
        file_type: Some("audio/wav".to_string()),
        fid: None,
        order: None,
    });
    Ok(message)
}
