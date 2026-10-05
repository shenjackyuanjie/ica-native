use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn text(value: JsonValue) -> Payload {
    Payload::Text(vec![value])
}

#[tokio::test]
async fn immediate_and_duplicate_ack_complete_exactly_once() {
    let pending = start_with("fixture", Duration::from_secs(1), |completion| async move {
        assert!(completion.complete(text(json!("first"))));
        assert!(!completion.complete(text(json!("second"))));
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        payload_first(&pending.receive().await.unwrap()),
        Some(json!("first"))
    );
}

#[tokio::test]
async fn independent_requests_can_complete_out_of_order_without_crosstalk() {
    let slots = Arc::new(Mutex::new(Vec::new()));
    let first = start_with("first", Duration::from_secs(1), {
        let slots = slots.clone();
        move |completion| async move {
            slots.lock().unwrap().push(completion);
            Ok(())
        }
    })
    .await
    .unwrap();
    let second = start_with("second", Duration::from_secs(1), {
        let slots = slots.clone();
        move |completion| async move {
            slots.lock().unwrap().push(completion);
            Ok(())
        }
    })
    .await
    .unwrap();
    let completions = std::mem::take(&mut *slots.lock().unwrap());
    assert!(completions[1].complete(text(json!(2))));
    assert!(completions[0].complete(text(json!(1))));
    assert_eq!(
        payload_first(&second.receive().await.unwrap()),
        Some(json!(2))
    );
    assert_eq!(
        payload_first(&first.receive().await.unwrap()),
        Some(json!(1))
    );
}

#[tokio::test]
async fn send_failure_emit_timeout_closed_callback_and_ack_timeout_are_distinct() {
    let send = match start_with("send", Duration::from_secs(1), |_| async { Err(()) }).await {
        Err(error) => error,
        Ok(_) => panic!("发送失败不能创建待等待 ACK"),
    };
    assert_eq!(send.failure, AckFailure::Send);

    let emit_timeout =
        match start_with("emit", Duration::from_millis(15), |completion| async move {
            let _keep_callback_alive = completion;
            std::future::pending::<Result<(), ()>>().await
        })
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("注册超时不能创建待等待 ACK"),
        };
    assert_eq!(emit_timeout.failure, AckFailure::Timeout);

    let closed = start_with("closed", Duration::from_secs(1), |completion| async move {
        drop(completion);
        Ok(())
    })
    .await
    .unwrap()
    .receive()
    .await
    .unwrap_err();
    assert_eq!(closed.failure, AckFailure::Closed);

    let slot = Arc::new(Mutex::new(None));
    let pending = start_with("late", Duration::from_millis(15), {
        let slot = slot.clone();
        move |completion| async move {
            *slot.lock().unwrap() = Some(completion);
            Ok(())
        }
    })
    .await
    .unwrap();
    let timeout = pending.receive().await.unwrap_err();
    assert_eq!(timeout.failure, AckFailure::Timeout);
    assert!(
        !slot
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .complete(text(json!("late")))
    );
}

#[tokio::test]
async fn dropping_waiter_invalidates_late_callback() {
    let slot = Arc::new(Mutex::new(None));
    let pending = start_with("cancelled", Duration::from_secs(1), {
        let slot = slot.clone();
        move |completion| async move {
            *slot.lock().unwrap() = Some(completion);
            Ok(())
        }
    })
    .await
    .unwrap();
    drop(pending);
    assert!(
        !slot
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .complete(text(json!("late")))
    );
}

#[test]
fn payload_normalization_and_nonempty_string_keep_bridge_contract() {
    assert_eq!(
        payload_values(&text(json!([1, 2]))),
        vec![json!(1), json!(2)]
    );
    assert_eq!(
        payload_values(&Payload::Text(vec![json!(1), json!(2)])),
        vec![json!(1), json!(2)]
    );
    assert_eq!(
        nonempty_string(&text(json!("token")), "token").unwrap(),
        "token"
    );
    assert!(matches!(
        nonempty_string(&text(json!("")), "token")
            .unwrap_err()
            .failure,
        AckFailure::InvalidResponse(_)
    ));
}
