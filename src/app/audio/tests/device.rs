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
