use super::submit_edit_then_recall;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn edit_recall_only_runs_after_confirmed_submission() {
    let order = Arc::new(Mutex::new(Vec::new()));
    for accepted in [false, true] {
        order.lock().unwrap().clear();
        let submit_order = order.clone();
        let recall_order = order.clone();
        let result = submit_edit_then_recall(
            async move {
                submit_order.lock().unwrap().push("submit");
                if accepted {
                    Ok(())
                } else {
                    Err("超时".into())
                }
            },
            || async move {
                recall_order.lock().unwrap().push("recall");
                true
            },
        )
        .await;
        if accepted {
            assert_eq!(result, Ok(true));
            assert_eq!(*order.lock().unwrap(), ["submit", "recall"]);
        } else {
            assert!(result.is_err());
            assert_eq!(*order.lock().unwrap(), ["submit"]);
        }
    }
}

#[tokio::test]
async fn recall_failure_does_not_turn_accepted_submission_into_retryable_failure() {
    assert_eq!(
        submit_edit_then_recall(async { Ok(()) }, || async { false }).await,
        Ok(false)
    );
}
