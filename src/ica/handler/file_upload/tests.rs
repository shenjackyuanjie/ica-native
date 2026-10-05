use super::*;
use std::sync::{Arc, Mutex};

fn payload(value: JsonValue) -> Payload {
    Payload::Text(vec![value])
}

#[tokio::test]
async fn uploaded_offsets_are_skipped_and_remaining_chunks_keep_order() {
    let data = vec![7_u8; CHUNK_SIZE * 2 + 1];
    let calls = Arc::new(Mutex::new(Vec::new()));
    let hash = upload_file_with("fixture.bin", &data, {
        let calls = calls.clone();
        move |event, args, _| {
            let calls = calls.clone();
            let event = event.to_string();
            let offset = args
                .get(1)
                .and_then(JsonValue::as_u64)
                .map(|value| value as usize);
            calls.lock().unwrap().push((event.clone(), offset));
            std::future::ready(if event == "requestUpload" {
                Ok(payload(json!({"allSuccess": false, "uploaded": [0]})))
            } else {
                Ok(payload(json!(true)))
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(hash, sha256(&data));
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            ("requestUpload".into(), None),
            ("uploadFile".into(), Some(CHUNK_SIZE)),
            ("uploadFile".into(), Some(CHUNK_SIZE * 2)),
        ]
    );
}

#[tokio::test]
async fn complete_upload_does_not_resend_chunks() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    upload_file_with("fixture.bin", b"already uploaded", {
        let calls = calls.clone();
        move |event, _, _| {
            calls.lock().unwrap().push(event.to_string());
            std::future::ready(Ok(payload(json!({"allSuccess": true, "uploaded": []}))))
        }
    })
    .await
    .unwrap();
    assert_eq!(*calls.lock().unwrap(), ["requestUpload"]);
}

#[tokio::test]
async fn rejected_chunk_retries_three_times_then_prevents_file_reference() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let error = upload_file_with("fixture.bin", b"new file", {
        let calls = calls.clone();
        move |event, _, _| {
            calls.lock().unwrap().push(event.to_string());
            std::future::ready(if event == "requestUpload" {
                Ok(payload(json!({"allSuccess": false, "uploaded": []})))
            } else {
                Ok(payload(json!(false)))
            })
        }
    })
    .await
    .unwrap_err();
    assert!(error.contains("offset=0"));
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.as_str() == "uploadFile")
            .count(),
        3
    );
}

#[tokio::test]
async fn transport_errors_are_retried_but_never_report_false_success() {
    let attempts = Arc::new(Mutex::new(0));
    let error = upload_file_with("fixture.bin", b"new file", {
        let attempts = attempts.clone();
        move |event, _, _| {
            let attempts = attempts.clone();
            let event = event.to_string();
            std::future::ready(if event == "requestUpload" {
                Ok(payload(json!({"allSuccess": false, "uploaded": []})))
            } else {
                *attempts.lock().unwrap() += 1;
                Err(AckError {
                    event,
                    failure: crate::ica::ack::AckFailure::Timeout,
                })
            })
        }
    })
    .await
    .unwrap_err();
    assert!(error.contains("ACK 等待超时"));
    assert_eq!(*attempts.lock().unwrap(), 3);
}

#[test]
fn malformed_resume_state_is_rejected_instead_of_reuploading_from_a_guess() {
    for value in [
        json!({}),
        json!({"allSuccess": "no", "uploaded": []}),
        json!({"allSuccess": false, "uploaded": "0"}),
        json!({"allSuccess": false, "uploaded": [-1]}),
    ] {
        assert!(matches!(
            parse_upload_status(&payload(value)).unwrap_err().failure,
            crate::ica::ack::AckFailure::InvalidResponse(_)
        ));
    }
}
