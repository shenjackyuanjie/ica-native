use super::{
    forward_values_summary, forward_wrapper_depth, is_forward_error_response,
    normalize_forward_values,
};
use serde_json::json;

#[test]
fn 保留普通消息数组() {
    let messages = vec![json!({ "_id": "1" }), json!({ "_id": "2" })];

    assert_eq!(normalize_forward_values(messages.clone()), messages);
}

#[test]
fn 保留单条消息对象() {
    let messages = vec![json!({ "_id": "1" })];

    assert_eq!(normalize_forward_values(messages.clone()), messages);
}

#[test]
fn 展开多层单元素数组包装() {
    let message = json!({ "_id": "284840486|914572", "content": "测试" });

    assert_eq!(
        normalize_forward_values(vec![json!([[message.clone()]])]),
        vec![message]
    );
}

#[test]
fn 识别主请求错误响应以决定资源回退() {
    assert!(is_forward_error_response(&[
        json!({ "_id": 0, "senderId": 0 })
    ]));
    assert!(!is_forward_error_response(&[
        json!({ "_id": 1, "senderId": 0 })
    ]));
    assert!(!is_forward_error_response(&[json!({ "_id": 0 })]));
    assert!(!is_forward_error_response(&[json!({ "senderId": 0 })]));
    assert!(!is_forward_error_response(&[
        json!({ "_id": "0", "senderId": 0 })
    ]));
    assert!(!is_forward_error_response(&[
        json!({ "_id": 0, "senderId": 0 }),
        json!({ "_id": 1, "senderId": 1 }),
    ]));
}

#[test]
fn 展开空数组包装() {
    assert!(normalize_forward_values(vec![json!([[]])]).is_empty());
}

#[test]
fn 统计额外数组包装层数() {
    assert_eq!(forward_wrapper_depth(&[json!([[{ "_id": "1" }]])]), 2);
    assert_eq!(forward_wrapper_depth(&[json!({ "_id": "1" })]), 0);
}

#[test]
fn 调试摘要不包含字段值() {
    let values = vec![json!({
        "_id": "敏感消息标识",
        "content": "敏感正文",
        "file": { "url": "https://example.com/?rkey=敏感令牌" },
        "senderId": 123456
    })];

    let summary = forward_values_summary(&values);
    assert!(summary.contains("_id=字符串"));
    assert!(summary.contains("senderId=数字"));
    assert!(summary.contains("content"));
    assert!(!summary.contains("敏感消息标识"));
    assert!(!summary.contains("敏感正文"));
    assert!(!summary.contains("敏感令牌"));
    assert!(!summary.contains("123456"));
}
