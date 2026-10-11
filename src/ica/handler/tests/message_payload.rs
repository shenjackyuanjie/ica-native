use super::*;

#[test]
fn multi_image_payload_uses_media_array() {
    let payload = build_multi_image_message(
        -123,
        "文字[Face: 14]",
        None,
        &[],
        &[
            ("image/png".to_string(), Arc::from([1_u8, 2, 3])),
            ("image/jpeg".to_string(), Arc::from([4_u8, 5, 6])),
        ],
    );
    let value = payload.as_value();
    assert_eq!(value["content"], "文字[Face: 14]");
    assert_eq!(value["media"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["media"][0]["b64"], "data:image/png;base64,AQID");
    assert_eq!(value["media"][0]["type"], "image/png");
    assert_eq!(value["media"][1]["b64"], "data:image/jpeg;base64,BAUG");
}

#[test]
fn multi_image_payload_preserves_mentions() {
    let payload = build_multi_image_message(
        -123,
        "你好 @测试用户 ",
        None,
        &[Mention {
            user_id: 456,
            text: "@测试用户".to_string(),
        }],
        &[("image/png".to_string(), Arc::from([1_u8]))],
    );
    let value = payload.as_value();
    assert_eq!(value["at"][0]["id"], 456);
    assert_eq!(value["at"][0]["text"], "@测试用户");
    assert_eq!(value["media"].as_array().map(Vec::len), Some(1));
}
fn recording_wav() -> Vec<u8> {
    let data_length = 9600_u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_length).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&24_000_u32.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_length.to_le_bytes());
    wav.resize(44 + data_length as usize, 0);
    wav
}

#[test]
fn voice_uses_oicq_record_media_and_never_plain_file_upload() {
    let message = build_voice_message(-123, &recording_wav()).unwrap();
    assert!(message.has_base64_media(), "必须触发一次性 token HTTP 路由");
    let value = message.as_value();
    assert!(
        value.get("file").is_none(),
        "file 兜底在 Bridge 中是 image 而不是 record"
    );
    assert_eq!(value["media"].as_array().unwrap().len(), 1);
    assert_eq!(value["media"][0]["type"], "audio/wav");
    assert!(
        value["media"][0]["b64"]
            .as_str()
            .unwrap()
            .starts_with("data:audio/wav;base64,RIFF".trim_end_matches("RIFF"))
    );
    assert!(value["media"][0].get("url").is_none());
    assert!(value["media"][0].get("fid").is_none());
    assert!(message.content.is_empty());
}

#[test]
fn voice_rejects_unsafe_boundaries_before_requesting_token() {
    assert!(build_voice_message(0, &recording_wav()).is_err());
    assert!(build_voice_message(-1, &[]).is_err());
    let wav = recording_wav();
    // 格式错误、声道错误、采样率错误、长度伪造，不能当作合法录音发送。
    for offset in [0, 16, 20, 22, 24, 28, 32, 34, 36, 40] {
        let mut invalid = wav.clone();
        invalid[offset] ^= 1;
        assert!(
            build_voice_message(-1, &invalid).is_err(),
            "未拒绝字段偏移 {offset}"
        );
    }
    assert!(build_voice_message(-1, &wav[..wav.len() - 1]).is_err());
    assert!(build_voice_message(-1, &vec![0; 44 + 24_000 * 2 * 60 + 2]).is_err());
}
